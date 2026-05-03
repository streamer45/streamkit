// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/** Shared hook for numeric slider controls with throttled updates. */

import { useAtomValue } from 'jotai/react';
import { throttle } from 'lodash-es';
import { useState, useEffect, useMemo, useRef } from 'react';

import { PARAM_THROTTLE_MS } from '@/constants/timing';
import { nodeParamsAtom } from '@/stores/sessionAtoms';
import { readByPath } from '@/utils/controlProps';

export interface UseNumericSliderOptions {
  nodeId: string;
  sessionId?: string;
  paramKey: string;
  /** Dot-notation path for reading/writing nested params. Defaults to `paramKey`. */
  path?: string;
  min: number;
  max: number;
  step: number;
  defaultValue: number;
  propValue?: number;
  onParamChange?: (nodeId: string, paramName: string, value: unknown) => void;
  transformValue?: (value: number) => number;
  throttleMs?: number;
}

export interface UseNumericSliderResult {
  localValue: number;
  handleChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
  handlePointerDown: (event: React.PointerEvent<HTMLInputElement>) => void;
  handlePointerUp: (event: React.PointerEvent<HTMLInputElement>) => void;
  disabled: boolean;
}

const clampValue = (value: number, min: number, max: number) => Math.min(Math.max(value, min), max);

export const useNumericSlider = (options: UseNumericSliderOptions): UseNumericSliderResult => {
  const {
    nodeId,
    sessionId,
    paramKey,
    path: pathOverride,
    min,
    max,
    step,
    defaultValue,
    propValue,
    onParamChange,
    transformValue,
    throttleMs = PARAM_THROTTLE_MS,
  } = options;

  const effectivePath = pathOverride ?? paramKey;

  // Read from Jotai per-node atom; readByPath resolves nested paths (e.g. "properties.score").
  const paramsKey = sessionId ? `${sessionId}\0${nodeId}` : nodeId;
  const nodeParams = useAtomValue(nodeParamsAtom(paramsKey));
  const storedValue = readByPath(nodeParams, effectivePath) as number | undefined;

  const effectiveValue = (() => {
    if (typeof storedValue === 'number' && Number.isFinite(storedValue)) {
      return clampValue(storedValue, min, max);
    }
    if (typeof propValue === 'number' && Number.isFinite(propValue)) {
      return clampValue(propValue, min, max);
    }
    return clampValue(defaultValue, min, max);
  })();

  const [localValue, setLocalValue] = useState(effectiveValue);
  const isDraggingRef = useRef(false);
  const localValueRef = useRef(localValue);

  useEffect(() => {
    localValueRef.current = localValue;
  }, [localValue]);

  useEffect(() => {
    if (isDraggingRef.current) {
      return;
    }
    const epsilon = step > 0 ? step / 50 : 0.0001;
    if (Math.abs(localValueRef.current - effectiveValue) > epsilon) {
      setLocalValue(effectiveValue);
    }
  }, [effectiveValue, step]);

  const throttledChange = useMemo(() => {
    if (!onParamChange) {
      return null;
    }
    return throttle(
      (value: number) => {
        const transformedValue = transformValue ? transformValue(value) : value;
        onParamChange(nodeId, effectivePath, transformedValue);
      },
      throttleMs,
      { leading: true, trailing: true }
    );
  }, [nodeId, onParamChange, effectivePath, transformValue, throttleMs]);

  useEffect(
    () => () => {
      throttledChange?.cancel();
    },
    [throttledChange]
  );

  const handleChange = (event: React.ChangeEvent<HTMLInputElement>) => {
    const raw = Number.parseFloat(event.target.value);
    const clamped = clampValue(Number.isFinite(raw) ? raw : min, min, max);
    setLocalValue(clamped);
    throttledChange?.(clamped);
  };

  const handlePointerDown = (event: React.PointerEvent<HTMLInputElement>) => {
    isDraggingRef.current = true;
    event.stopPropagation();
    event.currentTarget.setPointerCapture?.(event.pointerId);
  };

  const handlePointerUp = (event: React.PointerEvent<HTMLInputElement>) => {
    isDraggingRef.current = false;
    event.stopPropagation();
    event.currentTarget.releasePointerCapture?.(event.pointerId);
    throttledChange?.flush?.();
  };

  return {
    localValue,
    handleChange,
    handlePointerDown,
    handlePointerUp,
    disabled: !throttledChange,
  };
};
