// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import * as RadixRadioGroup from '@radix-ui/react-radio-group';
import React from 'react';

// RadioGroup Root
export const RadioGroupRoot = styled(RadixRadioGroup.Root)`
  display: flex;
  flex-direction: column;
  gap: 12px;
`;

// Radio Item
export const RadioItem = styled(RadixRadioGroup.Item)`
  width: 20px;
  height: 20px;
  border: 2px solid var(--sk-border);
  border-radius: 50%;
  background: var(--sk-bg);
  cursor: pointer;
  transition: all 0.15s ease;
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: center;

  &:hover {
    border-color: var(--sk-primary);
    background: var(--sk-hover-bg);
  }

  &:focus-visible {
    outline: none;
    border-color: var(--sk-primary);
    box-shadow: 0 0 0 2px rgba(14, 165, 233, 0.2);
  }

  &[data-state='checked'] {
    border-color: var(--sk-primary);
  }

  &[data-disabled] {
    opacity: 0.5;
    cursor: not-allowed;
  }
`;

// Radio Indicator (the inner circle)
export const RadioIndicator = styled(RadixRadioGroup.Indicator)`
  display: flex;
  align-items: center;
  justify-content: center;

  &::after {
    content: '';
    display: block;
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--sk-primary);
  }
`;

// Label wrapper for radio + label combinations
export const RadioLabel = styled.label`
  display: flex;
  align-items: center;
  gap: 10px;
  cursor: pointer;
  font-size: 14px;
  color: var(--sk-text);
  user-select: none;

  &:has([data-disabled]) {
    cursor: not-allowed;
    opacity: 0.6;
  }
`;

// Convenience component for radio with label
interface RadioWithLabelProps {
  value: string;
  id?: string;
  disabled?: boolean;
  label: React.ReactNode;
}

export const RadioWithLabel: React.FC<RadioWithLabelProps> = ({ value, id, disabled, label }) => {
  return (
    <RadioLabel htmlFor={id}>
      <RadioItem value={value} id={id} disabled={disabled}>
        <RadioIndicator />
      </RadioItem>
      <span>{label}</span>
    </RadioLabel>
  );
};

// Re-export Radix primitives
export const RadioGroup = RadixRadioGroup.Root;
