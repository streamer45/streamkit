// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { useNodePositionStore } from './nodePositionStore';

beforeEach(() => {
  useNodePositionStore.setState({ positions: {} });
  try {
    localStorage.clear();
  } catch {
    // ignore
  }
});

afterEach(() => {
  useNodePositionStore.setState({ positions: {} });
});

describe('useNodePositionStore initial state', () => {
  it('starts with no positions', () => {
    expect(useNodePositionStore.getState().positions).toEqual({});
  });
});

describe('updateNodePosition', () => {
  it('creates a session entry on first write', () => {
    useNodePositionStore.getState().updateNodePosition('s1', 'n1', { x: 10, y: 20 });

    expect(useNodePositionStore.getState().positions).toEqual({
      s1: { n1: { x: 10, y: 20 } },
    });
  });

  it('preserves other nodes in the same session', () => {
    const store = useNodePositionStore.getState();
    store.updateNodePosition('s1', 'n1', { x: 10, y: 20 });
    store.updateNodePosition('s1', 'n2', { x: 30, y: 40 });

    expect(useNodePositionStore.getState().positions.s1).toEqual({
      n1: { x: 10, y: 20 },
      n2: { x: 30, y: 40 },
    });
  });

  it('overwrites the position when the same node is updated', () => {
    const store = useNodePositionStore.getState();
    store.updateNodePosition('s1', 'n1', { x: 0, y: 0 });
    store.updateNodePosition('s1', 'n1', { x: 100, y: 200 });

    expect(useNodePositionStore.getState().positions.s1.n1).toEqual({ x: 100, y: 200 });
  });

  it('keeps sessions isolated from each other', () => {
    const store = useNodePositionStore.getState();
    store.updateNodePosition('s1', 'n1', { x: 1, y: 1 });
    store.updateNodePosition('s2', 'n1', { x: 2, y: 2 });

    const { positions } = useNodePositionStore.getState();
    expect(positions.s1.n1).toEqual({ x: 1, y: 1 });
    expect(positions.s2.n1).toEqual({ x: 2, y: 2 });
  });
});

describe('getNodePositions', () => {
  it('returns the per-session positions map', () => {
    useNodePositionStore.getState().updateNodePosition('s1', 'n1', { x: 1, y: 2 });

    expect(useNodePositionStore.getState().getNodePositions('s1')).toEqual({
      n1: { x: 1, y: 2 },
    });
  });

  it('returns an empty object for a session with no recorded positions', () => {
    expect(useNodePositionStore.getState().getNodePositions('missing')).toEqual({});
  });
});

describe('clearSession', () => {
  it('removes only the targeted session', () => {
    const store = useNodePositionStore.getState();
    store.updateNodePosition('s1', 'n1', { x: 1, y: 1 });
    store.updateNodePosition('s2', 'n1', { x: 2, y: 2 });

    store.clearSession('s1');

    expect(useNodePositionStore.getState().positions).toEqual({
      s2: { n1: { x: 2, y: 2 } },
    });
  });

  it('is a no-op when the session is not present', () => {
    useNodePositionStore.getState().updateNodePosition('s1', 'n1', { x: 1, y: 1 });

    useNodePositionStore.getState().clearSession('missing');

    expect(useNodePositionStore.getState().positions).toEqual({
      s1: { n1: { x: 1, y: 1 } },
    });
  });
});
