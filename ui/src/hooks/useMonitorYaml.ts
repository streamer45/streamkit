// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * YAML synchronisation for the Monitor view.
 *
 * Regenerates YAML when the active pipeline changes (unless the user
 * is actively editing). YAML is read-only in the monitor view — all
 * mutations go through direct WebSocket calls.
 *
 * Extracted from MonitorViewContent to isolate the regeneration effect.
 */

import { dump } from 'js-yaml';
import { useState, useEffect } from 'react';

import { useNodeParamsStore } from '@/stores/nodeParamsStore';
import type { Connection, Pipeline } from '@/types/types';
import { topoLevelsFromPipeline, orderedNamesFromLevels } from '@/utils/dag';

interface UseMonitorYamlOptions {
  selectedSessionId: string | null;
  pipeline: Pipeline | null | undefined;
}

export function useMonitorYaml({ selectedSessionId, pipeline }: UseMonitorYamlOptions) {
  const [yamlString, setYamlString] = useState<string>('');

  // ── YAML regeneration effect ──────────────────────────────────────────

  useEffect(() => {
    if (!pipeline) {
      setYamlString('');
      return;
    }

    const yamlObject: { nodes: Record<string, unknown> } = { nodes: {} };

    const { levels, sortedLevels } = topoLevelsFromPipeline(pipeline);
    const sortedNames = orderedNamesFromLevels(levels, sortedLevels);

    for (const nodeName of sortedNames) {
      const apiNode = pipeline.nodes[nodeName];
      if (!apiNode) continue;

      const needs = pipeline.connections
        .filter((c: Connection) => c.to_node === nodeName)
        .map((c: Connection) => c.from_node);

      const nodeConfig: Record<string, unknown> = { kind: apiNode.kind };

      const overrides = useNodeParamsStore
        .getState()
        .getParamsForNode(nodeName, selectedSessionId ?? undefined);
      const mergedParams = { ...(apiNode.params || {}), ...(overrides || {}) };
      if (Object.keys(mergedParams).length > 0) {
        nodeConfig['params'] = mergedParams;
      }

      if (needs.length === 1) {
        nodeConfig['needs'] = needs[0];
      } else if (needs.length > 1) {
        nodeConfig['needs'] = needs;
      }

      yamlObject.nodes[nodeName] = nodeConfig;
    }

    setYamlString(dump(yamlObject, { skipInvalid: true }));
  }, [pipeline, selectedSessionId]);

  /** Set YAML from the topology effect (external caller). */
  const setYamlFromTopology = setYamlString;

  return {
    yamlString,
    setYamlFromTopology,
  };
}
