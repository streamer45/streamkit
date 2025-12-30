// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * E2E Test Runner
 *
 * This script handles server lifecycle for E2E tests:
 * - If E2E_BASE_URL is set, runs playwright directly against that server
 * - Otherwise, starts a local skit server, waits for health, runs tests, then stops
 */

import { spawn, type ChildProcess } from 'child_process';
import * as path from 'path';
import * as fs from 'fs';
import { findFreePort } from './port';
import { waitForHealth } from './health';

const ROOT_DIR = path.resolve(import.meta.dirname, '../../..');
const MAX_LOG_BYTES = 256 * 1024;

interface ServerInfo {
  process: ChildProcess;
  baseUrl: string;
  port: number;
  stdout: string;
  stderr: string;
}

function appendBounded(buffer: string, chunk: string): string {
  const next = buffer + chunk;
  if (next.length <= MAX_LOG_BYTES) {
    return next;
  }
  return next.slice(next.length - MAX_LOG_BYTES);
}

async function startServer(): Promise<ServerInfo> {
  const port = await findFreePort();
  const baseUrl = `http://127.0.0.1:${port}`;

  // Check if UI is built
  const uiDistPath = path.join(ROOT_DIR, 'ui/dist/index.html');
  if (!fs.existsSync(uiDistPath)) {
    throw new Error(
      'UI not built. Run "cd ui && bun install && bun run build" or "just build-ui" first.'
    );
  }

  // Check if skit binary exists
  const skitPath = path.join(ROOT_DIR, 'target/debug/skit');
  if (!fs.existsSync(skitPath)) {
    throw new Error(
      'skit binary not found. Run "cargo build -p streamkit-server --bin skit" first.'
    );
  }

  console.log(`Starting skit server on port ${port}...`);

  const serverProcess = spawn(skitPath, ['serve'], {
    cwd: ROOT_DIR,
    env: {
      ...process.env,
      SK_SERVER__ADDRESS: `127.0.0.1:${port}`,
      SK_LOG__FILE_ENABLE: 'false', // Avoid writing skit.log
      RUST_LOG: 'warn',
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });

  let stdout = '';
  let stderr = '';

  // Log server output for debugging
  serverProcess.stdout?.on('data', (data: Buffer) => {
    const text = data.toString();
    stdout = appendBounded(stdout, text);
    if (process.env.DEBUG) console.log(`[skit stdout] ${text}`);
  });

  serverProcess.stderr?.on('data', (data: Buffer) => {
    const text = data.toString();
    stderr = appendBounded(stderr, text);
    if (process.env.DEBUG) console.error(`[skit stderr] ${text}`);
  });

  serverProcess.on('error', (err) => {
    console.error('Failed to start server:', err);
  });

  try {
    let onExit: ((code: number | null, signal: NodeJS.Signals | null) => void) | null = null;
    const exitedEarly = new Promise<never>((_, reject) => {
      onExit = (code, signal) => {
        reject(
          new Error(
            `skit exited before becoming healthy (code=${code ?? 'null'}, signal=${signal ?? 'null'})`
          )
        );
      };
      serverProcess.once('exit', onExit);
    });

    await Promise.race([waitForHealth(baseUrl), exitedEarly]);
    if (onExit) {
      serverProcess.off('exit', onExit);
    }
    exitedEarly.catch(() => undefined);
    console.log(`Server ready at ${baseUrl}`);
  } catch (error) {
    if (!process.env.DEBUG) {
      const trimmedStdout = stdout.trim();
      const trimmedStderr = stderr.trim();
      if (trimmedStdout) console.log(`\n[skit stdout]\n${trimmedStdout}\n`);
      if (trimmedStderr) console.error(`\n[skit stderr]\n${trimmedStderr}\n`);
    }
    await stopServer({ process: serverProcess, baseUrl, port, stdout, stderr });
    throw error;
  }

  return { process: serverProcess, baseUrl, port, stdout, stderr };
}

function stopServer(serverInfo: ServerInfo): Promise<void> {
  return new Promise((resolve) => {
    if (serverInfo.process.killed || serverInfo.process.exitCode !== null) {
      resolve();
      return;
    }

    console.log('Stopping skit server...');

    const onExit = () => {
      console.log('Server stopped.');
      resolve();
    };

    serverInfo.process.once('exit', onExit);
    serverInfo.process.kill('SIGTERM');

    setTimeout(() => {
      if (serverInfo.process.exitCode !== null) {
        return;
      }
      console.log('Force killing server...');
      serverInfo.process.kill('SIGKILL');
      setTimeout(() => {
        if (serverInfo.process.exitCode === null) {
          console.warn('Server did not exit after SIGKILL; continuing anyway.');
        }
        resolve();
      }, 2000);
    }, 5000);
  });
}

async function runPlaywright(baseUrl: string, extraArgs: string[]): Promise<number> {
  return new Promise((resolve) => {
    const args = ['playwright', 'test', ...extraArgs];
    console.log(`Running: bunx ${args.join(' ')}`);

    const playwright = spawn('bunx', args, {
      cwd: path.resolve(import.meta.dirname, '../..'),
      env: {
        ...process.env,
        E2E_BASE_URL: baseUrl,
      },
      stdio: 'inherit',
    });

    playwright.on('error', (err) => {
      console.error('Failed to run playwright:', err);
      resolve(1);
    });

    playwright.on('exit', (code) => {
      resolve(code ?? 1);
    });
  });
}

async function main(): Promise<void> {
  // Get extra args to pass to playwright (everything after --)
  const args = process.argv.slice(2);
  const dashDashIndex = args.indexOf('--');
  const playwrightArgs = dashDashIndex >= 0 ? args.slice(dashDashIndex + 1) : args;

  // Check if E2E_BASE_URL is already set (external server)
  const existingBaseUrl = process.env.E2E_BASE_URL;
  if (existingBaseUrl) {
    console.log(`Using external server at ${existingBaseUrl}`);
    const exitCode = await runPlaywright(existingBaseUrl, playwrightArgs);
    process.exit(exitCode);
  }

  // Start local server
  let serverInfo: ServerInfo | null = null;
  let exitCode = 1;

  try {
    serverInfo = await startServer();
    exitCode = await runPlaywright(serverInfo.baseUrl, playwrightArgs);
  } catch (error) {
    console.error('Error:', error);
    exitCode = 1;
  } finally {
    if (serverInfo) {
      await stopServer(serverInfo);
    }
  }

  process.exit(exitCode);
}

main();
