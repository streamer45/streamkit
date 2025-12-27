// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import * as RadixSlider from '@radix-ui/react-slider';
import React from 'react';

// Slider Root
export const SliderRoot = styled(RadixSlider.Root)`
  position: relative;
  display: flex;
  align-items: center;
  user-select: none;
  touch-action: none;
  width: 100%;
  height: 20px;

  &[data-disabled] {
    opacity: 0.5;
    cursor: not-allowed;
  }
`;

// Slider Track (the background bar)
export const SliderTrack = styled(RadixSlider.Track)`
  position: relative;
  flex-grow: 1;
  height: 4px;
  background: var(--sk-border);
  border-radius: 9999px;
`;

// Slider Range (the filled portion)
export const SliderRange = styled(RadixSlider.Range)`
  position: absolute;
  height: 100%;
  background: var(--sk-primary);
  border-radius: 9999px;
`;

// Slider Thumb (the draggable handle)
export const SliderThumb = styled(RadixSlider.Thumb)`
  display: block;
  width: 16px;
  height: 16px;
  background: var(--sk-panel-bg);
  border: 2px solid var(--sk-primary);
  border-radius: 50%;
  box-shadow: 0 2px 4px rgba(0, 0, 0, 0.2);
  cursor: grab;
  transition: all 0.15s ease;

  &:hover {
    width: 18px;
    height: 18px;
    border-width: 3px;
  }

  &:focus-visible {
    outline: none;
    box-shadow: 0 0 0 4px rgba(14, 165, 233, 0.2);
  }

  &:active {
    cursor: grabbing;
  }

  &[data-disabled] {
    cursor: not-allowed;
  }
`;

// Label wrapper for slider with label and value display
export const SliderLabel = styled.label`
  display: flex;
  flex-direction: column;
  gap: 8px;
  font-size: 14px;
  color: var(--sk-text);
  user-select: none;
`;

const SliderLabelRow = styled.div`
  display: flex;
  justify-content: space-between;
  align-items: center;
`;

const SliderValueDisplay = styled.span`
  font-weight: 600;
  color: var(--sk-primary);
  font-family: 'Fira Code', monospace;
  font-size: 13px;
`;

// Convenience component for slider with label and value display
interface SliderWithLabelProps {
  id?: string;
  value: number[];
  onValueChange: (value: number[]) => void;
  min?: number;
  max?: number;
  step?: number;
  disabled?: boolean;
  label: React.ReactNode;
  formatValue?: (value: number) => string;
}

export const SliderWithLabel: React.FC<SliderWithLabelProps> = ({
  id,
  value,
  onValueChange,
  min = 0,
  max = 100,
  step = 1,
  disabled = false,
  label,
  formatValue = (v) => v.toString(),
}) => {
  return (
    <SliderLabel htmlFor={id}>
      <SliderLabelRow>
        <span>{label}</span>
        <SliderValueDisplay>{formatValue(value[0])}</SliderValueDisplay>
      </SliderLabelRow>
      <SliderRoot
        id={id}
        value={value}
        onValueChange={onValueChange}
        min={min}
        max={max}
        step={step}
        disabled={disabled}
      >
        <SliderTrack>
          <SliderRange />
        </SliderTrack>
        <SliderThumb />
      </SliderRoot>
    </SliderLabel>
  );
};

// Re-export Radix primitives
export const Slider = RadixSlider.Root;
