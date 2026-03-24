// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * MoQ Relay Harness
 *
 * Manages the lifecycle of a moq-relay process for e2e tests.
 * The relay is started on a dynamically assigned port and configured
 * with a self-signed certificate and no authentication.
 */

import { spawn, type ChildProcess } from 'child_process';
import * as path from 'path';
import * as fs from 'fs';
import { findFreePort } from './port';

const ROOT_DIR = path.resolve(import.meta.dirname, '../../..');
const MAX_LOG_BYTES = 128 * 1024;

export interface RelayInfo {
  process: ChildProcess;
  /** HTTP URL for the relay (e.g. `http://127.0.0.1:PORT`) */
  url: string;
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

/**
 * Resolve the moq-relay binary path.
 *
 * Checks, in order:
 * 1. `E2E_MOQ_RELAY_BIN` env var (explicit override)
 * 2. `target/moq-relay/moq-relay` (built by `just build-moq-relay`)
 * 3. Sibling `moq` repo at `../moq/target/release/moq-relay`
 *
 * Returns `null` if no binary is found.
 */
function findRelayBinary(): string | null {
  if (process.env.E2E_MOQ_RELAY_BIN) {
    const p = process.env.E2E_MOQ_RELAY_BIN;
    if (fs.existsSync(p)) return p;
    console.warn(`E2E_MOQ_RELAY_BIN set to '${p}' but file not found`);
    return null;
  }

  // Built by `just build-moq-relay`
  const targetPath = path.join(ROOT_DIR, 'target', 'moq-relay', 'moq-relay');
  if (fs.existsSync(targetPath)) return targetPath;

  // Sibling repo fallback (for local dev)
  const siblingPath = path.resolve(ROOT_DIR, '..', 'moq', 'target', 'release', 'moq-relay');
  if (fs.existsSync(siblingPath)) return siblingPath;

  return null;
}

/**
 * Wait for the relay's HTTP endpoint to become reachable.
 * We poll `GET /certificate.sha256` since moq-relay always serves it.
 */
async function waitForRelay(url: string, timeoutMs: number = 30000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  const pollUrl = `${url}/certificate.sha256`;

  while (Date.now() < deadline) {
    try {
      const response = await fetch(pollUrl);
      if (response.ok) {
        return;
      }
    } catch {
      // Not ready yet
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }

  throw new Error(`moq-relay health check timed out after ${timeoutMs}ms (url: ${pollUrl})`);
}

/**
 * Start a moq-relay instance on a free port.
 *
 * Returns `null` if the relay binary is not available (tests should skip).
 */
export async function startRelay(): Promise<RelayInfo | null> {
  const relayBin = findRelayBinary();
  if (!relayBin) {
    console.log('moq-relay binary not found — relay tests will be skipped');
    return null;
  }

  const port = await findFreePort();
  const url = `http://127.0.0.1:${port}`;

  console.log(`Starting moq-relay on port ${port} (binary: ${relayBin})...`);

  const relayProcess = spawn(relayBin, [], {
    cwd: ROOT_DIR,
    env: {
      ...process.env,
      // Bind QUIC (UDP) and HTTP (TCP) on the same port
      MOQ_SERVER_BIND: `127.0.0.1:${port}`,
      MOQ_WEB_HTTP_LISTEN: `127.0.0.1:${port}`,
      // Self-signed TLS for QUIC
      MOQ_SERVER_TLS_GENERATE: 'localhost',
      // No auth for e2e tests
      MOQ_AUTH_PUBLIC: '',
      // Quiet logging
      MOQ_LOG_LEVEL: 'warn',
      // Disable iroh
      MOQ_IROH_ENABLED: 'false',
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });

  let stdout = '';
  let stderr = '';

  relayProcess.stdout?.on('data', (data: Buffer) => {
    const text = data.toString();
    stdout = appendBounded(stdout, text);
    if (process.env.DEBUG) console.log(`[moq-relay stdout] ${text}`);
  });

  relayProcess.stderr?.on('data', (data: Buffer) => {
    const text = data.toString();
    stderr = appendBounded(stderr, text);
    if (process.env.DEBUG) console.error(`[moq-relay stderr] ${text}`);
  });

  relayProcess.on('error', (err) => {
    console.error('Failed to start moq-relay:', err);
  });

  try {
    let onExit: ((code: number | null, signal: NodeJS.Signals | null) => void) | null = null;
    const exitedEarly = new Promise<never>((_, reject) => {
      onExit = (code, signal) => {
        reject(
          new Error(
            `moq-relay exited before becoming healthy (code=${code ?? 'null'}, signal=${signal ?? 'null'})`
          )
        );
      };
      relayProcess.once('exit', onExit);
    });

    await Promise.race([waitForRelay(url), exitedEarly]);
    if (onExit) {
      relayProcess.off('exit', onExit);
    }
    exitedEarly.catch(() => undefined);
    console.log(`moq-relay ready at ${url}`);
  } catch (error) {
    if (!process.env.DEBUG) {
      const trimmedStdout = stdout.trim();
      const trimmedStderr = stderr.trim();
      if (trimmedStdout) console.log(`\n[moq-relay stdout]\n${trimmedStdout}\n`);
      if (trimmedStderr) console.error(`\n[moq-relay stderr]\n${trimmedStderr}\n`);
    }
    await stopRelay({ process: relayProcess, url, port, stdout, stderr });
    throw error;
  }

  return { process: relayProcess, url, port, stdout, stderr };
}

export function stopRelay(relayInfo: RelayInfo): Promise<void> {
  return new Promise((resolve) => {
    if (relayInfo.process.killed || relayInfo.process.exitCode !== null) {
      resolve();
      return;
    }

    console.log('Stopping moq-relay...');

    relayInfo.process.once('exit', () => {
      console.log('moq-relay stopped.');
      resolve();
    });

    relayInfo.process.kill('SIGTERM');

    setTimeout(() => {
      if (relayInfo.process.exitCode !== null) {
        return;
      }
      console.log('Force killing moq-relay...');
      relayInfo.process.kill('SIGKILL');
      setTimeout(() => {
        if (relayInfo.process.exitCode === null) {
          console.warn('moq-relay did not exit after SIGKILL; continuing anyway.');
        }
        resolve();
      }, 2000);
    }, 5000);
  });
}
