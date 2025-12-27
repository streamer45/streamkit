// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import * as RadixCheckbox from '@radix-ui/react-checkbox';
import React from 'react';

// Checkbox Root
export const CheckboxRoot = styled(RadixCheckbox.Root)`
  width: 18px;
  height: 18px;
  border: 2px solid var(--sk-border);
  border-radius: 4px;
  background: var(--sk-bg);
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: all 0.15s ease;
  flex-shrink: 0;

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
    background: var(--sk-primary);
    border-color: var(--sk-primary);
  }

  &[data-disabled] {
    opacity: 0.5;
    cursor: not-allowed;
  }
`;

// Checkbox Indicator (checkmark)
export const CheckboxIndicator = styled(RadixCheckbox.Indicator)`
  color: var(--sk-panel-bg);
  display: flex;
  align-items: center;
  justify-content: center;

  svg {
    width: 12px;
    height: 12px;
    stroke-width: 3;
  }
`;

// Label wrapper for checkbox + label combinations
export const CheckboxLabel = styled.label`
  display: flex;
  align-items: center;
  gap: 8px;
  cursor: pointer;
  font-size: 14px;
  color: var(--sk-text);
  user-select: none;

  &:has([data-disabled]) {
    cursor: not-allowed;
    opacity: 0.6;
  }
`;

// Convenience component that combines checkbox with label
interface CheckboxWithLabelProps {
  id?: string;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  disabled?: boolean;
  label: React.ReactNode;
}

export const CheckboxWithLabel: React.FC<CheckboxWithLabelProps> = ({
  id,
  checked,
  onCheckedChange,
  disabled = false,
  label,
}) => {
  return (
    <CheckboxLabel htmlFor={id}>
      <CheckboxRoot id={id} checked={checked} onCheckedChange={onCheckedChange} disabled={disabled}>
        <CheckboxIndicator>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor">
            <polyline points="20 6 9 17 4 12" />
          </svg>
        </CheckboxIndicator>
      </CheckboxRoot>
      <span>{label}</span>
    </CheckboxLabel>
  );
};

// Re-export Radix primitives
export const Checkbox = RadixCheckbox.Root;
