// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Performance regression tests for useMonitorYaml.
 *
 * Verifies:
 * 1. Param-only pipeline changes are debounced — rapid updates produce at most
 *    one YAML regeneration after the debounce window.
 * 2. Structural changes (topoKey) via `setYamlFromTopology` cancel pending
 *    debounced regeneration and take effect immediately.
 * 3. YAML reads params exclusively from `sessionStore.pipeline` (single source
 *    of truth) — no dependency on `nodeParamsStore`.
 */

import { renderHook, act } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

import type { Pipeline } from '@/types/types';

import { useMonitorYaml } from './useMonitorYaml';

/** Build a minimal pipeline for YAML generation. */
function makePipeline(
  paramOverrides: Record<string, Record<string, unknown>> = {}
): Pipeline {
  return {
    nodes: {
      source: {
        kind: 'moq_source',
        params: { url: 'https://example.com', ...paramOverrides['source'] },
        config: {},
        state: null,
      },
      encoder: {
        kind: 'opus_encoder',
        params: { bitrate: 128000, ...paramOverrides['encoder'] },
        config: {},
        state: null,
      },
    },
    connections: [{ from_node: 'source', from_pin: 'out', to_node: 'encoder', to_pin: 'in' }],
  } as unknown as Pipeline;
}

describe('useMonitorYaml param flow', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('debounces YAML regeneration for param-only changes', () => {
    const pipeline = makePipeline();
    const topoKey = 'source,encoder|source->encoder';

    const { result, rerender } = renderHook(
      (props) => useMonitorYaml(props),
      {
        initialProps: {
          selectedSessionId: 'session-1',
          pipeline,
          topoKey,
        },
      }
    );

    // Initial render — no YAML yet (debounce hasn't fired)
    expect(result.current.yamlString).toBe('');

    // Wait for the initial debounce to fire
    act(() => {
      vi.advanceTimersByTime(350);
    });
    const initialYaml = result.current.yamlString;
    expect(initialYaml).toContain('moq_source');

    // Simulate 5 rapid param changes (slider drags) — same topoKey
    for (let i = 0; i < 5; i++) {
      const updatedPipeline = makePipeline({
        encoder: { bitrate: 128000 + (i + 1) * 1000 },
      });
      act(() => {
        rerender({
          selectedSessionId: 'session-1',
          pipeline: updatedPipeline,
          topoKey,
        });
      });
    }

    // YAML should still be the initial value — debounce not yet fired
    expect(result.current.yamlString).toBe(initialYaml);

    // Advance past debounce window
    act(() => {
      vi.advanceTimersByTime(350);
    });

    // Now YAML should reflect the LAST param value
    expect(result.current.yamlString).toContain('133000');
    expect(result.current.yamlString).not.toContain('129000');
  });

  it('setYamlFromTopology cancels pending debounce and takes effect immediately', () => {
    const pipeline = makePipeline();
    const topoKey = 'source,encoder|source->encoder';

    const { result, rerender } = renderHook(
      (props) => useMonitorYaml(props),
      {
        initialProps: {
          selectedSessionId: 'session-1',
          pipeline,
          topoKey,
        },
      }
    );

    // Fire initial debounce
    act(() => {
      vi.advanceTimersByTime(350);
    });

    // Trigger a param change (starts debounce)
    const updatedPipeline = makePipeline({ encoder: { bitrate: 256000 } });
    act(() => {
      rerender({
        selectedSessionId: 'session-1',
        pipeline: updatedPipeline,
        topoKey,
      });
    });

    // Before debounce fires, simulate a structural change via setYamlFromTopology
    const topoYaml = 'nodes:\n  new_topo: true\n';
    act(() => {
      result.current.setYamlFromTopology(topoYaml);
    });

    // YAML should be the topology YAML immediately, not the debounced param YAML
    expect(result.current.yamlString).toBe(topoYaml);

    // Even after debounce window, YAML shouldn't revert to the param-based version
    // (because the debounce was cancelled)
    act(() => {
      vi.advanceTimersByTime(350);
    });
    expect(result.current.yamlString).toBe(topoYaml);
  });

  it('clears YAML when pipeline becomes null', () => {
    const pipeline = makePipeline();
    const topoKey = 'source,encoder|source->encoder';

    const { result, rerender } = renderHook(
      (props) => useMonitorYaml(props),
      {
        initialProps: {
          selectedSessionId: 'session-1' as string | null,
          pipeline: pipeline as Pipeline | null,
          topoKey,
        },
      }
    );

    // Fire initial debounce
    act(() => {
      vi.advanceTimersByTime(350);
    });
    expect(result.current.yamlString).toContain('moq_source');

    // Set pipeline to null (session deselected)
    act(() => {
      rerender({
        selectedSessionId: null,
        pipeline: null,
        topoKey,
      });
    });

    expect(result.current.yamlString).toBe('');
  });

  it('skips regeneration when topoKey changes (topology effect handles it)', () => {
    const pipeline = makePipeline();
    const topoKey1 = 'source,encoder|source->encoder';

    const { result, rerender } = renderHook(
      (props) => useMonitorYaml(props),
      {
        initialProps: {
          selectedSessionId: 'session-1',
          pipeline,
          topoKey: topoKey1,
        },
      }
    );

    // Fire initial debounce
    act(() => {
      vi.advanceTimersByTime(350);
    });
    const initialYaml = result.current.yamlString;
    expect(initialYaml).toContain('moq_source');

    // Structural change: new node added → topoKey changes
    const newPipeline = {
      ...pipeline,
      nodes: {
        ...pipeline.nodes,
        mixer: { kind: 'mixer', params: {}, config: {}, state: null },
      },
    } as unknown as Pipeline;
    const topoKey2 = 'source,encoder,mixer|source->encoder';

    act(() => {
      rerender({
        selectedSessionId: 'session-1',
        pipeline: newPipeline,
        topoKey: topoKey2,
      });
    });

    // Even after debounce window, YAML should NOT have changed
    // (the topology effect should call setYamlFromTopology instead)
    act(() => {
      vi.advanceTimersByTime(350);
    });
    expect(result.current.yamlString).toBe(initialYaml);
  });
});
