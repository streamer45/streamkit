// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Schema-driven controls for Monitor view node cards.
 *
 * These components render toggle switches and text inputs directly on
 * ConfigurableNode cards for parameters marked `tunable: true` in the
 * node's `param_schema`.  They complement the existing NumericSliderControl
 * (which handles number/integer params) and use the same `TuneNodeAsync`
 * wire protocol via `useTuneNode`.
 */

import styled from '@emotion/styled';
import * as Tooltip from '@radix-ui/react-tooltip';
import { useAtomValue } from 'jotai/react';
import React, { useCallback, useEffect, useRef, useState } from 'react';

import { LiveBadge, LiveDot } from '@/components/ui/LiveIndicator';
import { useTuneNode } from '@/hooks/useTuneNode';
import { nodeParamsAtom } from '@/stores/sessionAtoms';
import { buildParamUpdate, readByPath } from '@/utils/controlProps';
import type { ToggleConfig, TextConfig } from '@/utils/jsonSchema';

// ---------------------------------------------------------------------------
// Shared styled components (also used by ConfigurableNode for labels)
// ---------------------------------------------------------------------------

export const ControlLabel = styled.div`
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  font-weight: 600;
  color: var(--sk-text);
`;

export const ControlLabelText = styled.span`
  flex: 0 0 auto;
`;

export const ControlDescription = styled.div`
  font-size: 11px;
  color: var(--sk-text-muted);
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

// ---------------------------------------------------------------------------
// Toggle control styled components
// ---------------------------------------------------------------------------

const ToggleRow = styled.div`
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 4px 0;
`;

const ToggleLabel = styled.span`
  font-size: 12px;
  font-weight: 600;
  color: var(--sk-text);
`;

const ToggleTrack = styled.button<{ checked: boolean }>`
  position: relative;
  width: 36px;
  height: 20px;
  border-radius: 10px;
  border: 1px solid ${(props) => (props.checked ? 'var(--sk-primary)' : 'var(--sk-border)')};
  background: ${(props) => (props.checked ? 'var(--sk-primary)' : 'var(--sk-bg)')};
  cursor: pointer;
  padding: 0;
  transition:
    background 0.15s,
    border-color 0.15s;
  flex-shrink: 0;

  &::after {
    content: '';
    position: absolute;
    top: 2px;
    left: ${(props) => (props.checked ? '17px' : '2px')};
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: ${(props) =>
      props.checked ? 'var(--sk-primary-contrast)' : 'var(--sk-text-muted)'};
    transition: left 0.15s;
  }
`;

// ---------------------------------------------------------------------------
// Text input control styled components
// ---------------------------------------------------------------------------

const TextInputWrapper = styled.div`
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 4px 0;
`;

const CompactTextInput = styled.input`
  width: 100%;
  padding: 4px 8px;
  font-size: 12px;
  background: var(--sk-bg);
  color: var(--sk-text);
  border: 1px solid var(--sk-border);
  border-radius: 4px;
  font-family: inherit;
  pointer-events: auto;
  box-sizing: border-box;

  &:focus {
    outline: none;
    border-color: var(--sk-primary);
  }

  &::placeholder {
    color: var(--sk-text-muted);
  }

  &:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }
`;

// ---------------------------------------------------------------------------
// Debounce delay for text inputs (ms)
// ---------------------------------------------------------------------------
const TEXT_DEBOUNCE_MS = 300;

// ---------------------------------------------------------------------------
// Live indicator tooltip (shared by both controls)
// ---------------------------------------------------------------------------

const LiveIndicator: React.FC = () => (
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
);

// ---------------------------------------------------------------------------
// Boolean toggle control
// ---------------------------------------------------------------------------

interface BooleanToggleControlProps {
  nodeId: string;
  sessionId?: string;
  config: ToggleConfig;
  params: Record<string, unknown>;
  showLiveIndicator?: boolean;
}

export const BooleanToggleControl: React.FC<BooleanToggleControlProps> = ({
  nodeId,
  sessionId,
  config,
  params,
  showLiveIndicator = false,
}) => {
  const { tuneNodeConfig } = useTuneNode(sessionId ?? null);

  // Read from atom for live sync
  const paramsKey = sessionId ? `${sessionId}\0${nodeId}` : nodeId;
  const nodeParams = useAtomValue(nodeParamsAtom(paramsKey));

  // Effective value: atom > props > default
  const effectiveValue = (() => {
    const stored = readByPath(nodeParams, config.path);
    if (typeof stored === 'boolean') return stored;
    const prop = readByPath(params as Record<string, unknown>, config.path);
    if (typeof prop === 'boolean') return prop;
    if (typeof config.schema.default === 'boolean') return config.schema.default;
    return false;
  })();

  const [checked, setChecked] = useState(effectiveValue);

  // Sync with external changes
  useEffect(() => {
    setChecked(effectiveValue);
  }, [effectiveValue]);

  // Ref pattern: keep tuneNodeConfig ref stable so toggle handler identity
  // doesn't change when sessionId (rarely) changes.
  const tuneRef = useRef(tuneNodeConfig);
  useEffect(() => {
    tuneRef.current = tuneNodeConfig;
  }, [tuneNodeConfig]);

  const handleToggle = useCallback(() => {
    setChecked((prev) => {
      const next = !prev;
      tuneRef.current(nodeId, buildParamUpdate(config.path, next));
      return next;
    });
  }, [nodeId, config.path]);

  const disabled = !sessionId;

  return (
    <ToggleRow>
      <ToggleLabel className="code-font">{config.key}</ToggleLabel>
      {showLiveIndicator && <LiveIndicator />}
      <ToggleTrack
        checked={checked}
        onClick={handleToggle}
        disabled={disabled}
        aria-label={config.schema.description ?? config.key}
        className="nodrag nopan"
      />
    </ToggleRow>
  );
};

// ---------------------------------------------------------------------------
// Text input control
// ---------------------------------------------------------------------------

interface TextInputControlProps {
  nodeId: string;
  sessionId?: string;
  config: TextConfig;
  params: Record<string, unknown>;
  showLiveIndicator?: boolean;
}

export const TextInputControl: React.FC<TextInputControlProps> = ({
  nodeId,
  sessionId,
  config,
  params,
  showLiveIndicator = false,
}) => {
  const { tuneNodeConfig } = useTuneNode(sessionId ?? null);

  // Read from atom for live sync
  const paramsKey = sessionId ? `${sessionId}\0${nodeId}` : nodeId;
  const nodeParams = useAtomValue(nodeParamsAtom(paramsKey));

  // Effective value: atom > props > default
  const effectiveValue = (() => {
    const stored = readByPath(nodeParams, config.path);
    if (typeof stored === 'string') return stored;
    const prop = readByPath(params as Record<string, unknown>, config.path);
    if (typeof prop === 'string') return prop;
    if (typeof config.schema.default === 'string') return config.schema.default;
    return '';
  })();

  const [text, setText] = useState(effectiveValue);

  // Sync with external changes when not actively editing
  const isEditingRef = useRef(false);
  useEffect(() => {
    if (!isEditingRef.current) {
      setText(effectiveValue);
    }
  }, [effectiveValue]);

  // Ref pattern: keep tuneNodeConfig ref stable for the debounce closure.
  const tuneRef = useRef(tuneNodeConfig);
  useEffect(() => {
    tuneRef.current = tuneNodeConfig;
  }, [tuneNodeConfig]);

  const timerRef = useRef<ReturnType<typeof setTimeout>>(undefined);

  const debouncedSend = useCallback(
    (value: string) => {
      clearTimeout(timerRef.current);
      timerRef.current = setTimeout(() => {
        tuneRef.current(nodeId, buildParamUpdate(config.path, value));
        isEditingRef.current = false;
      }, TEXT_DEBOUNCE_MS);
    },
    [nodeId, config.path]
  );

  // Clean up pending timer on unmount.
  useEffect(() => () => clearTimeout(timerRef.current), []);

  const handleChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      isEditingRef.current = true;
      const value = e.target.value;
      setText(value);
      debouncedSend(value);
    },
    [debouncedSend]
  );

  const disabled = !sessionId;

  return (
    <TextInputWrapper>
      <ControlLabel>
        <ControlLabelText className="code-font">{config.key}</ControlLabelText>
        {showLiveIndicator && <LiveIndicator />}
      </ControlLabel>
      {config.schema.description && (
        <ControlDescription>{config.schema.description}</ControlDescription>
      )}
      <CompactTextInput
        type="text"
        value={text}
        onChange={handleChange}
        placeholder={config.schema.description ?? config.key}
        disabled={disabled}
        aria-label={config.schema.description ?? config.key}
        className="nodrag nopan"
      />
    </TextInputWrapper>
  );
};
