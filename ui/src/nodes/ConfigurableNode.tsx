// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import * as Tooltip from '@radix-ui/react-tooltip';
import React from 'react';

import { NodeFrame } from '@/components/node/NodeFrame';
import { LiveBadge, LiveDot } from '@/components/ui/LiveIndicator';
import { useNumericSlider } from '@/hooks/useNumericSlider';
import { areNodePropsEqual } from '@/nodes/nodePropsEqual';
import { BooleanToggleControl, TextInputControl } from '@/nodes/SchemaControls';
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

const ParamCount = styled.div`
  padding: 4px 0;
  font-size: 12px;
  color: var(--sk-text-muted);
  text-align: center;
  border-top: 1px solid var(--sk-border);
`;

const SliderGroup = styled.div`
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

const SliderLabel = styled.div`
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  font-weight: 600;
  color: var(--sk-text);
`;

const SliderLabelText = styled.span`
  flex: 0 0 auto;
`;

const SliderDescription = styled.div`
  font-size: 11px;
  color: var(--sk-text-muted);
`;

const SliderValue = styled.span`
  font-variant-numeric: tabular-nums;
  color: var(--sk-text-muted);
  margin-left: auto;
  flex: 0 0 auto;
`;

const TooltipContent = styled(Tooltip.Content)`
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  padding: 8px 12px;
  box-shadow: 0 4px 12px var(--sk-shadow);
  font-size: 11px;
  z-index: 1000;
  max-width: 250px;
  color: var(--sk-text);
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
  showLiveIndicator?: boolean;
  isTunable: boolean;
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
  showLiveIndicator = false,
  isTunable,
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
      <SliderLabel>
        <SliderLabelText className="code-font">{paramKey}</SliderLabelText>
        {showLiveIndicator && isTunable && (
          <Tooltip.Provider delayDuration={300}>
            <Tooltip.Root>
              <Tooltip.Trigger asChild>
                <LiveBadge size="small">
                  <LiveDot size="small" />
                  LIVE
                </LiveBadge>
              </Tooltip.Trigger>
              <Tooltip.Portal>
                <TooltipContent side="top" sideOffset={5}>
                  Changes apply immediately to the running pipeline
                  <Tooltip.Arrow style={{ fill: 'var(--sk-border)' }} />
                </TooltipContent>
              </Tooltip.Portal>
            </Tooltip.Root>
          </Tooltip.Provider>
        )}
        <SliderValue>{formattedValue}</SliderValue>
      </SliderLabel>
      {schema?.description && <SliderDescription>{schema.description}</SliderDescription>}
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
  const schema = data.paramSchema as JsonSchema | undefined;
  const properties = schema?.properties ?? {};
  const totalParams = Object.keys(properties).length;

  const sliderConfigs = extractSliderConfigs(schema);
  const toggleConfigs = extractToggleConfigs(schema);
  const textConfigs = extractTextConfigs(schema);
  const hasControls =
    toggleConfigs.length > 0 || sliderConfigs.length > 0 || textConfigs.length > 0;

  // Detect bidirectional nodes using the bidirectional property from node definition
  const isBidirectional = data.definition?.bidirectional ?? false;

  // Show live indicator when node is in an active session (has sessionId)
  // This prevents the LIVE badge from showing in design view (which has no sessionId)
  const showLiveIndicator = !!data.onParamChange && !!data.sessionId;

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
      state={data.state}
      sessionId={data.sessionId}
      isBidirectional={isBidirectional}
    >
      {hasControls && (
        <SliderGroup>
          {toggleConfigs.map((config) => (
            <BooleanToggleControl
              key={config.key}
              nodeId={id}
              sessionId={data.sessionId}
              config={config}
              params={data.params}
              showLiveIndicator={showLiveIndicator}
            />
          ))}
          {sliderConfigs.map(({ key, path, schema: schemaProp, min, max, step, tunable }) => (
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
              params={data.params}
              onParamChange={data.onParamChange}
              showLiveIndicator={showLiveIndicator}
              isTunable={tunable}
            />
          ))}
          {textConfigs.map((config) => (
            <TextInputControl
              key={config.key}
              nodeId={id}
              sessionId={data.sessionId}
              config={config}
              params={data.params}
              showLiveIndicator={showLiveIndicator}
            />
          ))}
        </SliderGroup>
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
