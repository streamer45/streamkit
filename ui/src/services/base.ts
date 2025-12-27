// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Base service utilities shared across all service modules
 */

import { getBaseHrefWithoutTrailingSlash } from '../utils/baseHref';

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
    return apiBase;
  }

  const baseHref = getBaseHrefWithoutTrailingSlash();
  if (baseHref) return baseHref;

  // No base tag - use origin for root deployment
  return window.location.origin;
}
