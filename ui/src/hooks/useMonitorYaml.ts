// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * YAML synchronisation and editing logic for the Monitor view.
 *
 * - Regenerates YAML when the active pipeline changes (unless the user
 *   is actively editing).
 * - Applies YAML edits in staging mode: parses YAML → builds pipeline →
 *   resolves dynamic pins → syncs to nodeParamsStore + stagingStore →
 *   dispatches tuneNode for live tunable params.
 *
 * Extracted from MonitorViewContent to isolate the 280-line
 * handleYamlChange callback and its regeneration effect.
 */

import { dump, load } from 'js-yaml';
import { useState, useEffect, useCallback, useRef } from 'react';

import { useToast } from '@/context/ToastContext';
import { useNodeParamsStore } from '@/stores/nodeParamsStore';
import { useStagingStore, type StagingData } from '@/stores/stagingStore';
import type {
  Connection,
  NodeDefinition,
  Pipeline,
  Node,
  InputPin,
  OutputPin,
} from '@/types/types';
import { topoLevelsFromPipeline, orderedNamesFromLevels } from '@/utils/dag';
import { deepEqual } from '@/utils/deepEqual';
import { validateValue } from '@/utils/jsonSchema';
import { viewsLogger } from '@/utils/logger';

interface UseMonitorYamlOptions {
  selectedSessionId: string | null;
  pipeline: Pipeline | null | undefined;
  isInStagingMode: boolean;
  stagedPipeline: Pipeline | null;
  stagingData: StagingData | undefined;
  nodeDefinitions: NodeDefinition[];
  tuneNode: (nodeId: string, paramName: string, value: unknown) => void;
}

export function useMonitorYaml({
  selectedSessionId,
  pipeline,
  isInStagingMode,
  stagedPipeline,
  stagingData,
  nodeDefinitions,
  tuneNode,
}: UseMonitorYamlOptions) {
  const toast = useToast();
  const [yamlString, setYamlString] = useState<string>('');
  const isEditingYamlRef = useRef(false);

  // ── YAML regeneration effect ──────────────────────────────────────────

  useEffect(() => {
    if (isEditingYamlRef.current) return;

    const activePipeline = isInStagingMode && stagedPipeline ? stagedPipeline : pipeline;
    if (!activePipeline) {
      setYamlString('');
      return;
    }

    const yamlObject: { nodes: Record<string, unknown> } = { nodes: {} };

    const { levels, sortedLevels } = topoLevelsFromPipeline(activePipeline);
    const sortedNames = orderedNamesFromLevels(levels, sortedLevels);

    for (const nodeName of sortedNames) {
      const apiNode = activePipeline.nodes[nodeName];
      if (!apiNode) continue;

      const needs = activePipeline.connections
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
  }, [pipeline, stagedPipeline, isInStagingMode, selectedSessionId]);

  // ── YAML change handler (staging mode) ────────────────────────────────

  type DynamicPinCardinality = Extract<InputPin['cardinality'], { Dynamic: { prefix: string } }>;

  const isDynamicCardinality = (
    c: InputPin['cardinality'] | OutputPin['cardinality']
  ): c is DynamicPinCardinality => typeof c === 'object' && c !== null && 'Dynamic' in c;

  const handleYamlChange = useCallback(
    // eslint-disable-next-line max-statements -- YAML edits must preserve existing pin/handle ids (e.g. mixer `in_0`), which requires a multi-step transform.
    (yaml: string) => {
      if (!isInStagingMode || !selectedSessionId || !stagingData) return;

      isEditingYamlRef.current = true;

      try {
        const parsed = load(yaml) as {
          nodes?: Record<
            string,
            {
              kind: string;
              params?: Record<string, unknown>;
              needs?: string | string[];
              ui?: unknown;
            }
          >;
        };

        if (!parsed || !parsed.nodes || typeof parsed.nodes !== 'object') {
          toast.error('Invalid YAML: Must contain a "nodes" object');
          return;
        }

        // Build nodes map
        const nodes: Record<string, Node> = {};
        Object.entries(parsed.nodes).forEach(([nodeName, nodeConfig]) => {
          nodes[nodeName] = {
            kind: nodeConfig.kind,
            params: nodeConfig.params || {},
            state: null,
          };
        });

        // Build connections from "needs" fields while preserving existing pin ids.
        const basePipelineForPins = stagingData.stagedPipeline ?? pipeline;
        const existingConnections = basePipelineForPins?.connections ?? [];

        const existingByPair = new Map<string, Connection[]>();
        for (const c of existingConnections) {
          const key = `${c.from_node}→${c.to_node}`;
          const arr = existingByPair.get(key);
          if (arr) arr.push(c);
          else existingByPair.set(key, [c]);
        }

        const parseDynamicIndex = (pin: string, prefix: string): number | null => {
          if (!pin.startsWith(prefix)) return null;
          const rest = pin.slice(prefix.length);
          if (!/^\d+$/.test(rest)) return null;
          const n = Number(rest);
          return Number.isFinite(n) ? n : null;
        };

        const getNodeDef = (nodeName: string) => {
          const kind = nodes[nodeName]?.kind;
          if (!kind) return undefined;
          return nodeDefinitions.find((d) => d.kind === kind);
        };

        const pickSourcePin = (sourceNode: string): string => {
          const def = getNodeDef(sourceNode);
          const outputs = def?.outputs ?? [];

          const outPin = outputs.find(
            (p) => p.name === 'out' && !isDynamicCardinality(p.cardinality)
          );
          if (outPin) return outPin.name;

          const concreteOutputs = outputs.filter((p) => !isDynamicCardinality(p.cardinality));
          if (concreteOutputs.length === 1) return concreteOutputs[0].name;

          return 'out';
        };

        const usedDynamicInputsByNode = new Map<string, Set<number>>();
        const noteExistingDynamicInput = (toNode: string, pinName: string) => {
          const def = getNodeDef(toNode);
          const dyn = def?.inputs.find((p) => isDynamicCardinality(p.cardinality));
          if (!dyn) return;
          if (!isDynamicCardinality(dyn.cardinality)) return;
          const prefix = dyn.cardinality.Dynamic.prefix;
          const idx = parseDynamicIndex(pinName, prefix);
          if (idx === null) return;
          let set = usedDynamicInputsByNode.get(toNode);
          if (!set) {
            set = new Set();
            usedDynamicInputsByNode.set(toNode, set);
          }
          set.add(idx);
        };

        for (const c of existingConnections) {
          noteExistingDynamicInput(c.to_node, c.to_pin);
        }

        const allocateTargetPin = (targetNode: string): string => {
          const def = getNodeDef(targetNode);
          const inputs = def?.inputs ?? [];

          const inPin = inputs.find((p) => p.name === 'in' && !isDynamicCardinality(p.cardinality));
          if (inPin) return inPin.name;

          const dyn = inputs.find((p) => isDynamicCardinality(p.cardinality));
          if (dyn && isDynamicCardinality(dyn.cardinality)) {
            const prefix = dyn.cardinality.Dynamic.prefix;
            let used = usedDynamicInputsByNode.get(targetNode);
            if (!used) {
              used = new Set();
              usedDynamicInputsByNode.set(targetNode, used);
            }
            let i = 0;
            while (used.has(i)) i++;
            used.add(i);
            return `${prefix}${i}`;
          }

          const concreteInputs = inputs.filter((p) => !isDynamicCardinality(p.cardinality));
          if (concreteInputs.length === 1) return concreteInputs[0].name;

          return 'in';
        };

        const connections: Connection[] = [];
        const consumedPerPair = new Map<string, number>();
        Object.entries(parsed.nodes).forEach(([nodeName, nodeConfig]) => {
          if (!nodeConfig.needs) return;
          const needs = Array.isArray(nodeConfig.needs) ? nodeConfig.needs : [nodeConfig.needs];
          needs.forEach((sourceNode) => {
            if (!(sourceNode in nodes) || !(nodeName in nodes)) return;

            const pairKey = `${sourceNode}→${nodeName}`;
            const existing = existingByPair.get(pairKey);
            const consumed = consumedPerPair.get(pairKey) ?? 0;

            if (existing && consumed < existing.length) {
              const reused = existing[consumed];
              consumedPerPair.set(pairKey, consumed + 1);
              connections.push(reused);
              noteExistingDynamicInput(nodeName, reused.to_pin);
              return;
            }

            const from_pin = pickSourcePin(sourceNode);
            const to_pin = allocateTargetPin(nodeName);
            connections.push({ from_node: sourceNode, from_pin, to_node: nodeName, to_pin });
          });
        });

        const newPipeline: Pipeline = {
          name: null,
          description: null,
          mode: 'dynamic',
          nodes,
          connections,
        };

        // Determine which nodes are new (not in original live pipeline)
        const liveNodeNames = new Set(Object.keys(pipeline?.nodes || {}));
        const stagedNodes = new Set<string>();
        Object.keys(nodes).forEach((name) => {
          if (!liveNodeNames.has(name)) {
            stagedNodes.add(name);
          }
        });

        // Sync params to nodeParamsStore for immediate UI updates
        const paramsStore = useNodeParamsStore.getState();
        Object.entries(nodes).forEach(([nodeName, node]) => {
          if (node.params) {
            Object.entries(node.params).forEach(([key, value]) => {
              paramsStore.setParam(nodeName, key, value, selectedSessionId ?? undefined);
            });
          }
        });

        // Update staging store
        useStagingStore.setState((state) => {
          const data = state.staging[selectedSessionId];
          if (!data) return state;

          return {
            staging: {
              ...state.staging,
              [selectedSessionId]: {
                ...data,
                stagedPipeline: newPipeline,
                stagedNodes,
                version: data.version + 1,
              },
            },
          };
        });

        // Dispatch tune events for tunable param changes in live nodes
        Object.entries(nodes).forEach(([nodeName, newNode]) => {
          if (stagedNodes.has(nodeName)) return;

          const oldNode = pipeline?.nodes[nodeName];
          if (!oldNode) return;

          const nodeDef = nodeDefinitions.find((d) => d.kind === newNode.kind);
          if (!nodeDef) return;

          const schema = nodeDef.param_schema as
            | { properties?: Record<string, { tunable?: boolean }> }
            | undefined;
          const properties = schema?.properties;
          if (!properties) return;

          const oldParams: Record<string, unknown> =
            oldNode.params && typeof oldNode.params === 'object' && !Array.isArray(oldNode.params)
              ? (oldNode.params as Record<string, unknown>)
              : {};
          const newParams: Record<string, unknown> =
            newNode.params && typeof newNode.params === 'object' && !Array.isArray(newNode.params)
              ? (newNode.params as Record<string, unknown>)
              : {};

          Object.entries(newParams).forEach(([paramKey, newValue]) => {
            const propSchema = properties[paramKey];
            if (!propSchema?.tunable) return;

            const oldValue = oldParams[paramKey];
            if (!deepEqual(oldValue, newValue)) {
              const validationError = validateValue(newValue, propSchema);
              if (validationError) {
                toast.error(`Invalid value for ${nodeName}.${paramKey}: ${validationError}`);
                return;
              }
              viewsLogger.debug(
                `YAML edit: tuning live node ${nodeName}.${paramKey} from ${JSON.stringify(oldValue)} to ${JSON.stringify(newValue)}`
              );
              tuneNode(nodeName, paramKey, newValue);
            }
          });
        });

        setTimeout(() => {
          isEditingYamlRef.current = false;
        }, 1000);
      } catch (error) {
        viewsLogger.error('Failed to parse YAML:', error);
        toast.error(`Invalid YAML: ${error instanceof Error ? error.message : String(error)}`);
        isEditingYamlRef.current = false;
      }
    },
    [isInStagingMode, selectedSessionId, stagingData, pipeline, toast, nodeDefinitions, tuneNode]
  );

  /** Set YAML from the topology effect (external caller). */
  const setYamlFromTopology = setYamlString;

  return {
    yamlString,
    setYamlFromTopology,
    handleYamlChange,
  };
}
