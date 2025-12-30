// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Wait for the server to become healthy by polling the /healthz endpoint.
 */
export async function waitForHealth(
  baseUrl: string,
  timeoutMs: number = 30000,
  intervalMs: number = 500
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  const healthUrl = `${baseUrl}/healthz`;

  while (Date.now() < deadline) {
    try {
      const response = await fetch(healthUrl);
      if (response.ok) {
        const data = (await response.json()) as { status?: string };
        if (data.status === 'ok') {
          return;
        }
      }
    } catch {
      // Server not ready yet, continue polling
    }
    await new Promise((resolve) => setTimeout(resolve, intervalMs));
  }

  throw new Error(`Server health check timed out after ${timeoutMs}ms`);
}
