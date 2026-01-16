// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Base service utilities shared across all service modules
 */

import { getBaseHrefWithoutTrailingSlash, getBasePathname } from '../utils/baseHref';

/**
 * Gets the API base URL (handles both dev and production)
 *
 * In development, uses VITE_API_BASE to make direct requests to backend (no proxy).
 * This ensures client disconnects are properly detected by the backend.
 * In production, checks for <base> tag to handle subpath deployments.
 *
 * @returns The base URL for API requests (without trailing slash)
 */
export function getApiUrl(): string {
  // In development, VITE_API_BASE is set to direct backend URL (bypassing Vite proxy)
  // This ensures client disconnects are properly detected by the backend
  // In production, VITE_API_BASE is undefined, so we fall through to <base> tag logic
  const apiBase = import.meta.env.VITE_API_BASE;
  if (apiBase !== undefined) {
    // Cookie auth uses `SameSite=Strict`, which requires the UI and API to be the same "site"
    // (scheme + registrable domain), not necessarily the same origin.
    //
    // In local development it's common to mix `localhost` and `127.0.0.1`. Those are treated as
    // different sites by browsers, which breaks cookie-based auth flows. When both sides are
    // loopback, rewrite the API hostname to match the current UI hostname.
    try {
      const url = new URL(apiBase);
      const isLoopback = (host: string) => host === 'localhost' || host === '127.0.0.1';

      if (isLoopback(url.hostname) && isLoopback(window.location.hostname)) {
        url.hostname = window.location.hostname;
        return url.toString().replace(/\/$/, '');
      }
    } catch {
      // If parsing fails, fall back to the raw value.
    }

    return apiBase;
  }

  const baseHref = getBaseHrefWithoutTrailingSlash();
  if (baseHref) return baseHref;

  // No base tag - use origin for root deployment
  return window.location.origin;
}

function ensureLeadingSlash(path: string): string {
  return path.startsWith('/') ? path : `/${path}`;
}

/**
 * Fetch helper for StreamKit API calls.
 *
 * - Always sets `credentials: 'include'` so cookie auth works in dev (cross-origin)
 * - Redirects to `/login` on 401 (except when already on the login route)
 */
export async function fetchApi(path: string, options: RequestInit = {}): Promise<Response> {
  const apiUrl = getApiUrl();
  const url = `${apiUrl}${ensureLeadingSlash(path)}`;

  const response = await fetch(url, {
    ...options,
    credentials: 'include',
  });

  if (response.status === 401) {
    const basePathname = getBasePathname();
    const loginPath = `${basePathname}/login`;
    if (window.location.pathname !== loginPath) {
      window.location.assign(loginPath);
    }
  }

  return response;
}
