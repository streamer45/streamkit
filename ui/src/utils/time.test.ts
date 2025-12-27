// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, it, expect } from 'vitest';

import { formatUptime, formatTime, formatDateTime } from './time';

describe('time utilities', () => {
  describe('formatUptime', () => {
    it('should format seconds correctly', () => {
      const now = Date.now();
      const createdAt = new Date(now - 15000).toISOString(); // 15 seconds ago
      const result = formatUptime(createdAt);
      expect(result).toBe('15s');
    });

    it('should format minutes and seconds correctly', () => {
      const now = Date.now();
      const createdAt = new Date(now - 5 * 60 * 1000 - 30 * 1000).toISOString(); // 5m 30s ago
      const result = formatUptime(createdAt);
      expect(result).toBe('05m 30s');
    });

    it('should format hours, minutes, and seconds correctly', () => {
      const now = Date.now();
      const createdAt = new Date(
        now - 2 * 60 * 60 * 1000 - 23 * 60 * 1000 - 45 * 1000
      ).toISOString();
      const result = formatUptime(createdAt);
      expect(result).toBe('02h 23m 45s');
    });

    it('should format days, hours, and minutes correctly', () => {
      const now = Date.now();
      const createdAt = new Date(
        now - 2 * 24 * 60 * 60 * 1000 - 3 * 60 * 60 * 1000 - 30 * 60 * 1000
      ).toISOString();
      const result = formatUptime(createdAt);
      expect(result).toBe('2d 03h 30m');
    });

    it('should pad single digits with zeros', () => {
      const now = Date.now();
      const createdAt = new Date(now - 9 * 60 * 1000 - 5 * 1000).toISOString();
      const result = formatUptime(createdAt);
      expect(result).toBe('09m 05s');
    });

    it('should handle zero seconds', () => {
      const now = Date.now();
      const createdAt = new Date(now).toISOString();
      const result = formatUptime(createdAt);
      expect(result).toBe('00s');
    });
  });

  describe('formatTime', () => {
    it('should format time in HH:MM:SS format', () => {
      const timestamp = new Date('2025-01-01T14:23:45Z').toISOString();
      const result = formatTime(timestamp);
      // The format depends on locale, but should include time components
      expect(result).toMatch(/\d{1,2}:\d{2}:\d{2}/);
    });
  });

  describe('formatDateTime', () => {
    it('should format date and time', () => {
      const timestamp = new Date('2025-01-15T14:23:45Z').toISOString();
      const result = formatDateTime(timestamp);
      // Should return a non-empty string
      expect(result).toBeTruthy();
      expect(result.length).toBeGreaterThan(0);
    });
  });
});
