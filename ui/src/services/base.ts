// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { getBaseHrefWithoutTrailingSlash, getBasePathname } from '../utils/baseHref';

/** API base URL: uses VITE_API_BASE in dev, <base> tag in production. */
export function getApiUrl(): string {
  const apiBase = import.meta.env.VITE_API_BASE;
  if (apiBase !== undefined) {
    // Rewrite loopback hostname to match UI so SameSite=Strict cookies work.
    try {
      const url = new URL(apiBase);
      const isLoopback = (host: string) => host === 'localhost' || host === '127.0.0.1';

      if (isLoopback(url.hostname) && isLoopback(window.location.hostname)) {
        url.hostname = window.location.hostname;
        return url.toString().replace(/\/$/, '');
      }
    } catch {
      // ignore malformed URLs
    }

    return apiBase;
  }

  const baseHref = getBaseHrefWithoutTrailingSlash();
  if (baseHref) return baseHref;

  return window.location.origin;
}

function ensureLeadingSlash(path: string): string {
  return path.startsWith('/') ? path : `/${path}`;
}

/** Fetch with credentials and automatic 401 → login redirect. */
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
