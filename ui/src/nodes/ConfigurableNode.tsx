// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React, { useCallback, useMemo, useState } from 'react';

import { NodeFrame } from '@/components/node/NodeFrame';
import { LiveDot } from '@/components/ui/LiveIndicator';
import { useNodeStateFromAtom } from '@/hooks/useNodeAtoms';
import { useNumericSlider } from '@/hooks/useNumericSlider';
import { areNodePropsEqual } from '@/nodes/nodePropsEqual';
import {
  BooleanToggleControl,
  TextInputControl,
  ControlLabel,
  ControlLabelText,
  ControlDescription,
} from '@/nodes/SchemaControls';
import { perfOnRender } from '@/perf';
import type { InputPin, OutputPin, NodeState, NodeStats, NodeDefinition } from '@/types/types';
import { readByPath } from '@/utils/controlProps';
import {
  type JsonSchemaProperty,
  type JsonSchema,
  isFiniteNumber,
  extractSliderConfigs,
  extractToggleConfigs,
  extractTextConfigs,
  decimalPlacesFromStep,
  formatNumber,
} from '@/utils/jsonSchema';
import { nodesLogger } from '@/utils/logger';

// Module-level map so expanded state survives topology rebuilds (which
// recreate ConfigurableNode React elements, resetting useState).
const expandedState = new Map<string, boolean>();

const ParamCount = styled.div`
  padding: 4px 0;
  font-size: 12px;
  color: var(--sk-text-muted);
  text-align: center;
  border-top: 1px solid var(--sk-border);
`;

const ControlGroup = styled.div`
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 6px 0;
`;

const SliderWrapper = styled.div`
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 4px 0;
`;

const SliderValue = styled.span`
  font-variant-numeric: tabular-nums;
  color: var(--sk-text-muted);
  margin-left: auto;
  flex: 0 0 auto;
`;

const SliderInput = styled.input`
  width: 100%;
  pointer-events: auto;
  cursor: pointer;

  &:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }
`;

const SliderMarks = styled.div`
  display: flex;
  justify-content: space-between;
  font-size: 10px;
  color: var(--sk-text-muted);
  font-variant-numeric: tabular-nums;
`;

const ControlsToggleBar = styled.button`
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 6px 0;
  background: none;
  border: none;
  border-top: 1px solid var(--sk-border);
  color: var(--sk-text-muted);
  font-size: 11px;
  cursor: pointer;
  font-family: inherit;
  text-align: left;
  border-radius: 0;

  &:hover {
    color: var(--sk-text);
  }
`;

const ChevronSvg = styled.svg<{ expanded: boolean }>`
  transition: transform 0.15s ease;
  transform: rotate(${(props) => (props.expanded ? '90deg' : '0deg')});
  flex-shrink: 0;
`;

interface ConfigurableNodeData {
  label: string;
  kind: string;
  params: Record<string, unknown>;
  paramSchema: unknown;
  inputs: InputPin[];
  outputs: OutputPin[];
  nodeDefinition?: NodeDefinition;
  state?: NodeState;
  stats?: NodeStats;
  definition?: { bidirectional?: boolean };
  onParamChange?: (nodeId: string, paramName: string, value: unknown) => void;
  sessionId?: string;
  /** Set when the node has been dropped on the canvas but not yet
   *  committed via `addnode`.  See `MonitorView`/`pipelineGraph.buildNodeObject`. */
  draft?: { missingRequired: string[]; isCreating: boolean; onPromote: () => void };
}

interface ConfigurableNodeProps {
  id: string;
  data: ConfigurableNodeData;
  selected?: boolean;
}

interface NumericSliderControlProps {
  nodeId: string;
  sessionId?: string;
  paramKey: string;
  /** Dot-notation path for reading/writing nested params. Defaults to `paramKey`. */
  path?: string;
  schema: JsonSchemaProperty;
  min: number;
  max: number;
  step: number;
  params: Record<string, unknown>;
  onParamChange?: (nodeId: string, paramName: string, value: unknown) => void;
}

// Helper: Compute fallback value for slider
function computeFallbackValue(
  defaultValue: unknown,
  baseParam: unknown,
  min: number,
  max: number
): number {
  if (isFiniteNumber(defaultValue)) {
    return defaultValue;
  }
  if (isFiniteNumber(baseParam)) {
    return baseParam as number;
  }
  return (min + max) / 2;
}

// Helper: Format slider value with unit
function formatSliderValue(
  value: number,
  paramKey: string,
  step: number,
  schemaType?: string
): { formatted: string; min: string; max: string; decimals: number } {
  const decimals =
    schemaType === 'integer' ? 0 : Math.min(4, Math.max(0, decimalPlacesFromStep(step)));

  const includeSign = paramKey.toLowerCase().includes('db');
  const unit = includeSign ? ' dB' : '';

  return {
    formatted: `${formatNumber(value, decimals, includeSign)}${unit}`,
    min: '',
    max: '',
    decimals,
  };
}

// Helper: Format min/max labels
function formatMinMaxLabels(
  min: number,
  max: number,
  decimals: number,
  includeSign: boolean,
  unit: string
): { formattedMin: string; formattedMax: string } {
  return {
    formattedMin: `${formatNumber(min, decimals, includeSign)}${unit}`,
    formattedMax: `${formatNumber(max, decimals, includeSign)}${unit}`,
  };
}

// ---------------------------------------------------------------------------
// Numeric slider control
// ---------------------------------------------------------------------------

const NumericSliderControl: React.FC<NumericSliderControlProps> = ({
  nodeId,
  sessionId,
  paramKey,
  path: pathOverride,
  schema,
  min,
  max,
  step,
  params,
  onParamChange,
}) => {
  const baseParam = readByPath(params as Record<string, unknown>, pathOverride ?? paramKey);
  const defaultValue = schema?.default;

  const fallback = computeFallbackValue(defaultValue, baseParam, min, max);
  const propValue = isFiniteNumber(baseParam) ? (baseParam as number) : undefined;

  const { localValue, handleChange, handlePointerDown, handlePointerUp, disabled } =
    useNumericSlider({
      nodeId,
      sessionId,
      paramKey,
      path: pathOverride,
      min,
      max,
      step,
      defaultValue: fallback,
      propValue,
      onParamChange,
      transformValue: schema.type === 'integer' ? Math.round : undefined,
    });

  const { decimals } = formatSliderValue(localValue, paramKey, step, schema.type);
  const includeSign = paramKey.toLowerCase().includes('db');
  const unit = includeSign ? ' dB' : '';
  const formattedValue = `${formatNumber(localValue, decimals, includeSign)}${unit}`;
  const { formattedMin, formattedMax } = formatMinMaxLabels(min, max, decimals, includeSign, unit);

  return (
    <SliderWrapper>
      <ControlLabel>
        <ControlLabelText className="code-font">{paramKey}</ControlLabelText>
        <SliderValue>{formattedValue}</SliderValue>
      </ControlLabel>
      {schema?.description && <ControlDescription>{schema.description}</ControlDescription>}
      <SliderInput
        type="range"
        min={min}
        max={max}
        step={step > 0 ? step : 'any'}
        value={localValue}
        onChange={handleChange}
        onPointerDown={handlePointerDown}
        onPointerUp={handlePointerUp}
        disabled={disabled}
        className="nodrag nopan"
      />
      <SliderMarks>
        <span>{formattedMin}</span>
        <span>{formattedMax}</span>
      </SliderMarks>
    </SliderWrapper>
  );
};

const ConfigurableNode: React.FC<ConfigurableNodeProps> = React.memo(function ConfigurableNode({
  id,
  data,
  selected,
}) {
  nodesLogger.debug(
    'ConfigurableNode Render:',
    id,
    'onParamChange:',
    !!data.onParamChange,
    'onParamChange identity:',
    data.onParamChange?.toString().substring(0, 50)
  );

  const state = useNodeStateFromAtom(id, data.sessionId, data.state);
  const params = data.params;

  const schema = data.paramSchema as JsonSchema | undefined;
  const properties = schema?.properties ?? {};
  const totalParams = Object.keys(properties).length;

  const sliderConfigs = useMemo(() => extractSliderConfigs(schema), [schema]);
  const toggleConfigs = useMemo(() => extractToggleConfigs(schema), [schema]);
  const textConfigs = useMemo(() => extractTextConfigs(schema), [schema]);
  const controlCount = toggleConfigs.length + sliderConfigs.length + textConfigs.length;
  // Drafts hide the canvas-side tune controls entirely.  Those controls
  // dispatch `tunenode` directly via `useTuneNode` (see SchemaControls)
  // and so cannot route through the draft path — for a draft node the
  // engine has no entry yet and would warn "Could not tune non-existent
  // node".  Drafts are configured exclusively from the right-pane
  // Inspector, whose `onParamChange` is wired to draft-aware routing.
  const isDraft = !!data.draft;
  const hasControls = controlCount > 0 && !isDraft;

  // Detect bidirectional nodes using the bidirectional property from node definition
  const isBidirectional = data.definition?.bidirectional ?? false;

  // Show live indicator when node is in an active session (has sessionId)
  // This prevents the LIVE badge from showing in design view (which has no sessionId)
  const showLiveIndicator = !!data.onParamChange && !!data.sessionId;

  const [controlsExpanded, setControlsExpanded] = useState(() => expandedState.get(id) ?? false);
  const toggleExpanded = useCallback(() => {
    setControlsExpanded((prev) => {
      const next = !prev;
      expandedState.set(id, next);
      return next;
    });
  }, [id]);

  const content = (
    <NodeFrame
      id={id}
      label={data.label}
      kind={data.kind}
      selected={selected}
      minWidth={200}
      inputs={data.inputs}
      outputs={data.outputs}
      nodeDefinition={data.nodeDefinition}
      state={state}
      sessionId={data.sessionId}
      isBidirectional={isBidirectional}
      draft={data.draft}
    >
      {hasControls && (
        <>
          <ControlsToggleBar
            className="nodrag nopan"
            onClick={toggleExpanded}
            aria-expanded={controlsExpanded}
            aria-label={`${controlsExpanded ? 'Hide' : 'Show'} ${controlCount} controls`}
          >
            <ChevronSvg
              expanded={controlsExpanded}
              width="8"
              height="8"
              viewBox="0 0 8 8"
              fill="currentColor"
            >
              <path d="M2 1l4 3-4 3z" />
            </ChevronSvg>
            <span>
              {controlCount} control{controlCount !== 1 ? 's' : ''}
            </span>
            {showLiveIndicator && <LiveDot size="small" aria-label="Live-tunable" />}
          </ControlsToggleBar>
          {controlsExpanded && (
            <ControlGroup>
              {toggleConfigs.map((config) => (
                <BooleanToggleControl
                  key={config.key}
                  nodeId={id}
                  sessionId={data.sessionId}
                  config={config}
                  params={params}
                />
              ))}
              {sliderConfigs.map(({ key, path, schema: schemaProp, min, max, step }) => (
                <NumericSliderControl
                  key={key}
                  nodeId={id}
                  sessionId={data.sessionId}
                  paramKey={key}
                  path={path}
                  schema={schemaProp}
                  min={min}
                  max={max}
                  step={step}
                  params={params}
                  onParamChange={data.onParamChange}
                />
              ))}
              {textConfigs.map((config) => (
                <TextInputControl
                  key={config.key}
                  nodeId={id}
                  sessionId={data.sessionId}
                  config={config}
                  params={params}
                />
              ))}
            </ControlGroup>
          )}
        </>
      )}

      {totalParams > 0 ? (
        <ParamCount>
          {totalParams} parameter{totalParams !== 1 ? 's' : ''}
        </ParamCount>
      ) : (
        <ParamCount>No configurable parameters</ParamCount>
      )}
    </NodeFrame>
  );

  if (import.meta.env.DEV) {
    return (
      <React.Profiler id="ConfigurableNode" onRender={perfOnRender}>
        {content}
      </React.Profiler>
    );
  }

  return content;
}, areNodePropsEqual);

ConfigurableNode.displayName = 'ConfigurableNode';

export default ConfigurableNode;
