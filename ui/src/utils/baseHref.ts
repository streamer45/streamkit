// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

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
