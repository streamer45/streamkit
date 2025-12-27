// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Shared hook for numeric slider controls with throttled updates.
 * Handles local state, store synchronization, drag tracking, and throttled updates.
 */

import { throttle } from 'lodash-es';
import { useState, useEffect, useMemo, useRef } from 'react';

import { useNodeParamsStore, selectNodeParam } from '@/stores/nodeParamsStore';

export interface UseNumericSliderOptions {
  nodeId: string;
  sessionId?: string;
  paramKey: string;
  min: number;
  max: number;
  step: number;
  defaultValue: number;
  propValue?: number;
  onParamChange?: (nodeId: string, paramName: string, value: unknown) => void;
  /**
   * Transform the value before sending it to onParamChange.
   * For example, use Math.round for integer types.
   */
  transformValue?: (value: number) => number;
  /**
   * Throttle delay in milliseconds (default: 100ms)
   */
  throttleMs?: number;
}

export interface UseNumericSliderResult {
  /**
   * The current local value to display in the slider
   */
  localValue: number;
  /**
   * Handler for slider onChange event
   */
  handleChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
  /**
   * Handler for slider onPointerDown event
   */
  handlePointerDown: (event: React.PointerEvent<HTMLInputElement>) => void;
  /**
   * Handler for slider onPointerUp event
   */
  handlePointerUp: (event: React.PointerEvent<HTMLInputElement>) => void;
  /**
   * Whether the slider should be disabled (no onParamChange provided)
   */
  disabled: boolean;
}

const clampValue = (value: number, min: number, max: number) => Math.min(Math.max(value, min), max);

/**
 * Custom hook for managing numeric slider state with throttled updates
 */
export const useNumericSlider = (options: UseNumericSliderOptions): UseNumericSliderResult => {
  const {
    nodeId,
    sessionId,
    paramKey,
    min,
    max,
    step,
    defaultValue,
    propValue,
    onParamChange,
    transformValue,
    throttleMs = 100,
  } = options;

  // Get stored value from Zustand store
  const storedValue = useNodeParamsStore(selectNodeParam(nodeId, paramKey, sessionId)) as
    | number
    | undefined;

  // Determine effective value: stored > prop > default
  const effectiveValue = (() => {
    if (typeof storedValue === 'number' && Number.isFinite(storedValue)) {
      return clampValue(storedValue, min, max);
    }
    if (typeof propValue === 'number' && Number.isFinite(propValue)) {
      return clampValue(propValue, min, max);
    }
    return clampValue(defaultValue, min, max);
  })();

  // Local state for immediate UI feedback
  const [localValue, setLocalValue] = useState(effectiveValue);

  // Refs for tracking drag state and local value
  const isDraggingRef = useRef(false);
  const localValueRef = useRef(localValue);

  // Keep localValueRef in sync
  useEffect(() => {
    localValueRef.current = localValue;
  }, [localValue]);

  // Sync local value with effective value when not dragging
  useEffect(() => {
    if (isDraggingRef.current) {
      return;
    }
    const epsilon = step > 0 ? step / 50 : 0.0001;
    if (Math.abs(localValueRef.current - effectiveValue) > epsilon) {
      setLocalValue(effectiveValue);
    }
  }, [effectiveValue, step]);

  // Create throttled update function
  const throttledChange = useMemo(() => {
    if (!onParamChange) {
      return null;
    }
    return throttle(
      (value: number) => {
        const transformedValue = transformValue ? transformValue(value) : value;
        onParamChange(nodeId, paramKey, transformedValue);
      },
      throttleMs,
      { leading: true, trailing: true }
    );
  }, [nodeId, onParamChange, paramKey, transformValue, throttleMs]);

  // Cancel throttled function on unmount
  useEffect(
    () => () => {
      throttledChange?.cancel();
    },
    [throttledChange]
  );

  // Handler for slider value change
  const handleChange = (event: React.ChangeEvent<HTMLInputElement>) => {
    const raw = Number.parseFloat(event.target.value);
    const clamped = clampValue(Number.isFinite(raw) ? raw : min, min, max);
    setLocalValue(clamped);
    throttledChange?.(clamped);
  };

  // Handler for pointer down (start dragging)
  const handlePointerDown = (event: React.PointerEvent<HTMLInputElement>) => {
    isDraggingRef.current = true;
    event.stopPropagation();
    event.currentTarget.setPointerCapture?.(event.pointerId);
  };

  // Handler for pointer up (stop dragging)
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
