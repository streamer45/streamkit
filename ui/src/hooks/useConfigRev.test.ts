// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Unit tests for the per-node config revision counter module.
 *
 * These tests verify the core causal-consistency primitives:
 *   - Monotonic rev counter per node
 *   - Independent counters across nodes
 *   - Reset clears all counters
 *   - bumpConfigRev returns the new value
 */

import { describe, it, expect, beforeEach } from 'vitest';

import { getLocalConfigRev, bumpConfigRev, resetAllConfigRevs } from './useConfigRev';

beforeEach(() => {
  resetAllConfigRevs();
});

describe('useConfigRev — singleton rev counters', () => {
  it('starts at 0 for unknown nodes', () => {
    expect(getLocalConfigRev('node_a')).toBe(0);
  });

  it('bumpConfigRev increments and returns the new value', () => {
    expect(bumpConfigRev('node_a')).toBe(1);
    expect(bumpConfigRev('node_a')).toBe(2);
    expect(bumpConfigRev('node_a')).toBe(3);
    expect(getLocalConfigRev('node_a')).toBe(3);
  });

  it('counters are independent per node', () => {
    bumpConfigRev('node_a');
    bumpConfigRev('node_a');
    bumpConfigRev('node_b');

    expect(getLocalConfigRev('node_a')).toBe(2);
    expect(getLocalConfigRev('node_b')).toBe(1);
  });

  it('resetAllConfigRevs clears all counters', () => {
    bumpConfigRev('node_a');
    bumpConfigRev('node_b');

    resetAllConfigRevs();

    expect(getLocalConfigRev('node_a')).toBe(0);
    expect(getLocalConfigRev('node_b')).toBe(0);
  });
});
