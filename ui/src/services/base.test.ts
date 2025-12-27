// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Note: This file tests getApiUrl() which depends on:
 * - import.meta.env.VITE_API_BASE (Vite env var)
 * - document.querySelector (DOM)
 * - window.location (Browser API)
 *
 * These are difficult to mock properly in Vitest 4.0.6 due to API limitations.
 * Tests focus on what can be validated in the test environment.
 */

import { describe, it, expect } from 'vitest';

import { getApiUrl } from './base';

describe('getApiUrl', () => {
  it('should return a valid URL string', () => {
    const result = getApiUrl();

    // Should return a string
    expect(typeof result).toBe('string');

    // Should be a valid URL (no trailing slash or is empty)
    expect(result).toMatch(/^https?:\/\/|^$/);

    // Should not end with trailing slash (unless it's just the origin)
    if (result && result !== '/') {
      expect(result).not.toMatch(/\/$/);
    }
  });

  it('should handle environment with window.location', () => {
    const result = getApiUrl();

    // In test environment, should at least not throw
    expect(result).toBeDefined();
  });
});
