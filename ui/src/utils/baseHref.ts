// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Helpers for handling <base href="..."> deployments.
 *
 * StreamKit can be hosted under a subpath (e.g., /s/session_xxx/). These helpers
 * centralize parsing so the router, HTTP API, and WebSocket URLs stay in sync.
 */

export function getBaseUrl(): URL | null {
  const baseElement = document.querySelector('base[href]');
  const baseHref = baseElement?.getAttribute('href');
  if (!baseHref) return null;

  try {
    return new URL(baseHref, window.location.origin);
  } catch {
    return null;
  }
}

export function getBasePathname(): string {
  const baseUrl = getBaseUrl();
  if (!baseUrl) return '';
  return baseUrl.pathname.replace(/\/$/, '');
}

export function getBaseHrefWithoutTrailingSlash(): string | null {
  const baseUrl = getBaseUrl();
  if (!baseUrl) return null;
  return baseUrl.href.replace(/\/$/, '');
}
