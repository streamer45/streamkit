// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * YAML synchronisation for the Monitor view.
 *
 * YAML is **read-only** in Monitor view — all mutations go through direct
 * WebSocket calls.  This hook regenerates the YAML string whenever the
 * canonical pipeline (from `sessionStore`) changes.
 *
 * To avoid O(N·log N) YAML dumps on every RAF-batched param echo-back
 * (e.g. during rapid slider drags), regeneration is **debounced** for
 * param-only changes.  Structural changes (topology effect) bypass the
 * debounce by calling `setYamlFromTopology` directly.
 *
 * **Single source of truth**: params are read exclusively from
 * `sessionStore.pipeline.nodes[x].params`.  `nodeParamsStore` is NOT
 * consulted — it exists only as an optimistic overlay for immediate UI
 * controls (sliders, inspector).
 */

import { useState, useEffect, useRef, useCallback } from 'react';

import type { Pipeline } from '@/types/types';
import { pipelineToYaml } from '@/utils/pipelineGraph';

/** Debounce window for param-only YAML regeneration (ms). */
const YAML_REGEN_DEBOUNCE_MS = 300;

interface UseMonitorYamlOptions {
  selectedSessionId: string | null;
  pipeline: Pipeline | null | undefined;
  /** Current topology key — used to distinguish structural changes
   *  (which should regenerate immediately) from param-only changes
   *  (which are debounced). */
  topoKey: string;
}

export function useMonitorYaml({ selectedSessionId, pipeline, topoKey }: UseMonitorYamlOptions) {
  const [yamlString, setYamlString] = useState<string>('');
  const prevTopoKeyRef = useRef(topoKey);
  const debounceTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // ── Debounced YAML regeneration for param-only pipeline changes ──────
  //
  // When topoKey changes, the topology effect in MonitorView calls
  // setYamlFromTopology directly (immediate, no debounce).
  //
  // When only params change (pipeline reference changes but topoKey is
  // stable), we debounce to avoid dumping YAML on every RAF frame during
  // slider drags.

  useEffect(() => {
    if (!pipeline) {
      setYamlString('');
      return;
    }

    // Structural change — the topology effect handles YAML via
    // setYamlFromTopology.  Update our ref and skip.
    if (prevTopoKeyRef.current !== topoKey) {
      prevTopoKeyRef.current = topoKey;
      return;
    }

    // Param-only change — debounce.
    if (debounceTimerRef.current !== null) {
      clearTimeout(debounceTimerRef.current);
    }
    debounceTimerRef.current = setTimeout(() => {
      debounceTimerRef.current = null;
      setYamlString(pipelineToYaml(pipeline));
    }, YAML_REGEN_DEBOUNCE_MS);

    return () => {
      if (debounceTimerRef.current !== null) {
        clearTimeout(debounceTimerRef.current);
        debounceTimerRef.current = null;
      }
    };
  }, [pipeline, selectedSessionId, topoKey]);

  /** Set YAML from the topology effect (external caller, immediate). */
  const setYamlFromTopology = useCallback((yaml: string) => {
    // Cancel any pending debounced regeneration — structural YAML takes
    // precedence.
    if (debounceTimerRef.current !== null) {
      clearTimeout(debounceTimerRef.current);
      debounceTimerRef.current = null;
    }
    setYamlString(yaml);
  }, []);

  return {
    yamlString,
    setYamlFromTopology,
  };
}
