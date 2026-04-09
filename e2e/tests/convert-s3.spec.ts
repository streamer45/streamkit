// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { execSync } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';

import { test, expect } from '@playwright/test';

import { ensureLoggedIn, getAuthHeaders } from './auth-helpers';

const repoRoot = path.resolve(import.meta.dirname, '..', '..');
const sampleOggPath = path.join(repoRoot, 'samples', 'audio', 'system', 'sample.ogg');
const transcodeToS3Yaml = fs.readFileSync(
  path.join(repoRoot, 'samples', 'pipelines', 'oneshot', 'transcode_to_s3.yml'),
  'utf8'
);

// RustFS / S3 configuration — matches docker/docker-compose.rustfs.yml defaults.
const S3_ENDPOINT = process.env.E2E_S3_ENDPOINT ?? 'http://localhost:9000';
const S3_ACCESS_KEY = process.env.SK_S3_ACCESS_KEY ?? 'rustfsadmin';
const S3_SECRET_KEY = process.env.SK_S3_SECRET_KEY ?? 'rustfsadmin';
const S3_BUCKET = 'streamkit-output';
const S3_KEY = 'transcode/output.mp4';

/** Common env for aws CLI calls. */
const awsEnv = {
  ...process.env,
  AWS_ACCESS_KEY_ID: S3_ACCESS_KEY,
  AWS_SECRET_ACCESS_KEY: S3_SECRET_KEY,
};

/**
 * Check whether the S3-compatible endpoint (RustFS) is reachable.
 */
async function isS3Available(): Promise<boolean> {
  try {
    const res = await fetch(`${S3_ENDPOINT}/health`, {
      signal: AbortSignal.timeout(3000),
    });
    return res.ok;
  } catch {
    return false;
  }
}

/**
 * Ensure the target bucket exists, creating it if necessary.
 */
function ensureBucket(): void {
  try {
    execSync(`aws --endpoint-url ${S3_ENDPOINT} s3api head-bucket --bucket ${S3_BUCKET}`, {
      env: awsEnv,
      stdio: 'ignore',
    });
  } catch {
    execSync(`aws --endpoint-url ${S3_ENDPOINT} s3 mb s3://${S3_BUCKET}`, {
      env: awsEnv,
      stdio: 'ignore',
    });
  }
}

/**
 * Get the size of an object in S3 (returns -1 if not found).
 */
function getS3ObjectSize(): number {
  try {
    const output = execSync(
      `aws --endpoint-url ${S3_ENDPOINT} s3api head-object --bucket ${S3_BUCKET} --key ${S3_KEY} --output json`,
      { env: awsEnv, encoding: 'utf8', stdio: ['pipe', 'pipe', 'ignore'] }
    );
    const parsed = JSON.parse(output) as { ContentLength?: number };
    return parsed.ContentLength ?? -1;
  } catch {
    return -1;
  }
}

/**
 * Delete an object from S3 (best-effort cleanup).
 */
function deleteS3Object(): void {
  try {
    execSync(`aws --endpoint-url ${S3_ENDPOINT} s3 rm s3://${S3_BUCKET}/${S3_KEY}`, {
      env: awsEnv,
      stdio: 'ignore',
    });
  } catch {
    // Ignore cleanup errors
  }
}

test.describe('Convert - Transcode to S3 Pipeline', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/convert');
    await ensureLoggedIn(page);
    if (!page.url().includes('/convert')) {
      await page.goto('/convert');
    }
  });

  test('API: transcode Ogg→MP4 with S3 upload returns audio and writes to object store', async ({
    page,
    baseURL,
  }) => {
    // Extended timeout — pipeline execution + S3 upload can take a while in CI.
    test.setTimeout(60_000);

    // Skip if S3 endpoint is not available.
    const s3Up = await isS3Available();
    if (!s3Up) {
      test.skip(
        true,
        `S3 endpoint not reachable at ${S3_ENDPOINT} — start RustFS: docker compose -f docker/docker-compose.rustfs.yml up -d`
      );
      return;
    }

    // Ensure bucket exists and clean up any previous test artifact.
    ensureBucket();
    deleteS3Object();

    // Read the sample Ogg file and send it through the pipeline.
    const audioBase64 = fs.readFileSync(sampleOggPath).toString('base64');
    const authHeaders = getAuthHeaders();

    const result = await page.evaluate(
      async ({ url, yaml, audio, headers }) => {
        const formData = new FormData();
        formData.append('config', yaml);
        const bytes = Uint8Array.from(atob(audio), (c) => c.charCodeAt(0));
        formData.append('media', new Blob([bytes], { type: 'audio/ogg' }), 'sample.ogg');

        const controller = new AbortController();
        const timeoutId = setTimeout(() => controller.abort(), 45_000);

        try {
          const response = await fetch(`${url}/api/v1/process`, {
            method: 'POST',
            body: formData,
            headers,
            signal: controller.signal,
          });

          const contentType = response.headers.get('content-type') ?? '';

          // Read the full response to ensure the pipeline completes
          // (including the S3 upload via passthrough).
          const blob = await response.blob();

          return {
            status: response.status,
            contentType,
            bodySize: blob.size,
          };
        } finally {
          clearTimeout(timeoutId);
        }
      },
      {
        url: baseURL,
        yaml: transcodeToS3Yaml,
        audio: audioBase64,
        headers: authHeaders,
      }
    );

    // --- Verify HTTP response ---
    expect(result.status, `Pipeline request failed with status ${result.status}`).toBe(200);
    expect(result.contentType, `Expected audio content type, got: ${result.contentType}`).toContain(
      'audio/'
    );
    expect(result.bodySize, 'Response body should contain audio data').toBeGreaterThan(0);

    // --- Verify S3 upload ---
    // The passthrough node writes data to S3 as it flows through.  The S3
    // multipart upload may still be finalizing (close()) shortly after the
    // HTTP response completes, so allow a brief retry window.
    let s3Size = -1;
    for (let attempt = 0; attempt < 10; attempt++) {
      s3Size = getS3ObjectSize();
      if (s3Size > 0) break;
      await page.waitForTimeout(500);
    }

    expect(s3Size, 'Object should exist in S3 after pipeline execution').toBeGreaterThan(0);
    expect(
      s3Size,
      `S3 object size (${s3Size}) should match HTTP response size (${result.bodySize})`
    ).toBe(result.bodySize);

    // Clean up test artifact.
    deleteS3Object();
  });
});
