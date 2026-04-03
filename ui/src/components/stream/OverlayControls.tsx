// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import { throttle } from 'lodash-es';
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { Section, SectionTitle } from '@/components/ui/ViewLayout';
import { useTuneNode } from '@/hooks/useTuneNode';
import type { ControlConfig } from '@/types/types';
import { parseClientFromYaml } from '@/utils/clientSection';
import { buildParamUpdate } from '@/utils/controlProps';

// ---------------------------------------------------------------------------
// Props
// ---------------------------------------------------------------------------

interface OverlayControlsProps {
  pipelineYaml: string;
  sessionId: string;
}

// ---------------------------------------------------------------------------
// Styled components
// ---------------------------------------------------------------------------

const ControlsContainer = styled.div`
  display: flex;
  flex-direction: column;
  gap: 16px;
`;

const GroupHeading = styled.h3`
  font-size: 14px;
  font-weight: 600;
  color: var(--sk-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.05em;
  margin: 0;
`;

const ControlRow = styled.div`
  display: flex;
  align-items: center;
  gap: 12px;
  min-height: 36px;
`;

const ControlLabel = styled.label`
  font-size: 14px;
  font-weight: 500;
  color: var(--sk-text);
  min-width: 140px;
  flex-shrink: 0;
`;

const ToggleTrack = styled.button<{ checked: boolean }>`
  position: relative;
  width: 44px;
  height: 24px;
  border-radius: 12px;
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
    left: ${(props) => (props.checked ? '21px' : '2px')};
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: ${(props) =>
      props.checked ? 'var(--sk-primary-contrast)' : 'var(--sk-text-muted)'};
    transition: left 0.15s;
  }
`;

const TextInput = styled.input`
  padding: 8px 12px;
  font-size: 14px;
  background: var(--sk-bg);
  color: var(--sk-text);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  font-family: inherit;
  flex: 1;
  min-width: 0;

  &:focus {
    outline: none;
    border-color: var(--sk-primary);
  }

  &::placeholder {
    color: var(--sk-text-muted);
  }
`;

const SliderWrapper = styled.div`
  display: flex;
  align-items: center;
  gap: 8px;
  flex: 1;
  min-width: 0;
`;

const Slider = styled.input`
  flex: 1;
  min-width: 80px;
  accent-color: var(--sk-primary);
`;

const SliderValue = styled.span`
  font-size: 13px;
  font-weight: 600;
  color: var(--sk-text);
  min-width: 36px;
  text-align: right;
  font-variant-numeric: tabular-nums;
`;

const ActionButton = styled.button`
  padding: 8px 16px;
  font-size: 14px;
  font-weight: 600;
  color: var(--sk-text);
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  cursor: pointer;
  transition: none;

  &:hover {
    background: var(--sk-hover-bg);
    border-color: var(--sk-border-strong);
  }

  &:active {
    background: var(--sk-primary);
    color: var(--sk-primary-contrast);
    border-color: var(--sk-primary);
  }
`;

// ---------------------------------------------------------------------------
// Debounce delay for text inputs (ms)
// ---------------------------------------------------------------------------
const TEXT_DEBOUNCE_MS = 300;

// Throttle delay for slider updates (ms)
const SLIDER_THROTTLE_MS = 100;

// ---------------------------------------------------------------------------
// Individual control widgets
// ---------------------------------------------------------------------------

const ToggleControl: React.FC<{
  control: ControlConfig;
  onSend: (value: unknown) => void;
}> = ({ control, onSend }) => {
  const [checked, setChecked] = useState<boolean>(() => {
    if (typeof control.default === 'boolean') return control.default;
    return false;
  });

  const handleToggle = useCallback(() => {
    const next = !checked;
    setChecked(next);
    onSend(next);
  }, [checked, onSend]);

  return <ToggleTrack checked={checked} onClick={handleToggle} aria-label={control.label} />;
};

const TextControl: React.FC<{
  control: ControlConfig;
  onSend: (value: unknown) => void;
}> = ({ control, onSend }) => {
  const [text, setText] = useState<string>(() => {
    if (typeof control.default === 'string') return control.default;
    return '';
  });

  // Store onSend in a ref so the debounce closure is stable across re-renders
  // and pending timers always call the latest callback.
  const onSendRef = useRef(onSend);
  useEffect(() => {
    onSendRef.current = onSend;
  }, [onSend]);

  const timerRef = useRef<ReturnType<typeof setTimeout>>();

  const debouncedSend = useCallback((value: string) => {
    clearTimeout(timerRef.current);
    timerRef.current = setTimeout(() => onSendRef.current(value), TEXT_DEBOUNCE_MS);
  }, []);

  // Clean up any pending timer on unmount.
  useEffect(() => () => clearTimeout(timerRef.current), []);

  const handleChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const value = e.target.value;
      setText(value);
      debouncedSend(value);
    },
    [debouncedSend]
  );

  return <TextInput value={text} onChange={handleChange} placeholder={control.label} />;
};

const NumberControl: React.FC<{
  control: ControlConfig;
  onSend: (value: unknown) => void;
}> = ({ control, onSend }) => {
  const min = control.min ?? 0;
  const max = control.max ?? 100;
  const step = control.step ?? 1;
  const defaultValue = typeof control.default === 'number' ? control.default : min;

  const [localValue, setLocalValue] = useState<number>(defaultValue);
  const isDraggingRef = useRef(false);

  // Store onSend in a ref so the throttle closure is stable and always
  // calls the latest callback without recreating the throttle function.
  const onSendRef = useRef(onSend);
  useEffect(() => {
    onSendRef.current = onSend;
  }, [onSend]);

  const throttledSend = useMemo(
    () =>
      throttle((value: number) => onSendRef.current(value), SLIDER_THROTTLE_MS, {
        leading: true,
        trailing: true,
      }),
    []
  );

  useEffect(() => () => throttledSend.cancel(), [throttledSend]);

  const handleChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const raw = Number.parseFloat(e.target.value);
      const clamped = Math.min(Math.max(Number.isFinite(raw) ? raw : min, min), max);
      setLocalValue(clamped);
      throttledSend(clamped);
    },
    [min, max, throttledSend]
  );

  const handlePointerDown = useCallback((e: React.PointerEvent<HTMLInputElement>) => {
    isDraggingRef.current = true;
    e.stopPropagation();
    e.currentTarget.setPointerCapture?.(e.pointerId);
  }, []);

  const handlePointerUp = useCallback(
    (e: React.PointerEvent<HTMLInputElement>) => {
      isDraggingRef.current = false;
      e.stopPropagation();
      e.currentTarget.releasePointerCapture?.(e.pointerId);
      throttledSend.flush?.();
    },
    [throttledSend]
  );

  // Format display: show integers without decimals, floats with up to 2.
  const display = Number.isInteger(step) ? Math.round(localValue) : localValue.toFixed(2);

  return (
    <SliderWrapper>
      <Slider
        type="range"
        min={min}
        max={max}
        step={step}
        value={localValue}
        onChange={handleChange}
        onPointerDown={handlePointerDown}
        onPointerUp={handlePointerUp}
      />
      <SliderValue>{display}</SliderValue>
    </SliderWrapper>
  );
};

const ButtonControl: React.FC<{
  control: ControlConfig;
  onSend: (value: unknown) => void;
}> = ({ control, onSend }) => {
  const handleClick = useCallback(() => {
    onSend(control.value ?? true);
  }, [control.value, onSend]);

  return <ActionButton onClick={handleClick}>{control.label}</ActionButton>;
};

// ---------------------------------------------------------------------------
// Main component
// ---------------------------------------------------------------------------

/** Groups controls by their `group` field. Ungrouped controls come first. */
function groupControls(controls: ControlConfig[]): Map<string | null, ControlConfig[]> {
  const groups = new Map<string | null, ControlConfig[]>();
  for (const c of controls) {
    const key = c.group ?? null;
    const list = groups.get(key);
    if (list) {
      list.push(c);
    } else {
      groups.set(key, [c]);
    }
  }
  return groups;
}

const OverlayControls: React.FC<OverlayControlsProps> = ({ pipelineYaml, sessionId }) => {
  const { tuneNodeConfig } = useTuneNode(sessionId);

  // Parse controls from the pipeline YAML's client section.
  const controls: ControlConfig[] = useMemo(
    () => parseClientFromYaml(pipelineYaml)?.controls ?? [],
    [pipelineYaml]
  );

  // Build a stable send callback per control (keyed by node+property).
  const makeSend = useCallback(
    (control: ControlConfig) => (value: unknown) => {
      const update = buildParamUpdate(control.property, value);
      tuneNodeConfig(control.node, update);
    },
    [tuneNodeConfig]
  );

  const grouped = useMemo(() => groupControls(controls), [controls]);

  if (controls.length === 0) return null;

  return (
    <Section>
      <SectionTitle>Pipeline Controls</SectionTitle>
      <ControlsContainer>
        {Array.from(grouped.entries()).map(([groupName, items]) => (
          <React.Fragment key={groupName ?? '__ungrouped'}>
            {groupName && <GroupHeading>{groupName}</GroupHeading>}
            {items.map((control) => {
              const key = `${control.node}:${control.property}`;
              const send = makeSend(control);
              return (
                <ControlRow key={key}>
                  <ControlLabel>{control.label}</ControlLabel>
                  {control.type === 'toggle' && <ToggleControl control={control} onSend={send} />}
                  {control.type === 'text' && <TextControl control={control} onSend={send} />}
                  {control.type === 'number' && <NumberControl control={control} onSend={send} />}
                  {control.type === 'button' && <ButtonControl control={control} onSend={send} />}
                </ControlRow>
              );
            })}
          </React.Fragment>
        ))}
      </ControlsContainer>
    </Section>
  );
};

export default OverlayControls;
