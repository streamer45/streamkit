// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import * as RadixSwitch from '@radix-ui/react-switch';
import React from 'react';

// Switch Root
export const SwitchRoot = styled(RadixSwitch.Root)`
  width: 42px;
  height: 24px;
  background: var(--sk-border);
  border-radius: 9999px;
  position: relative;
  cursor: pointer;
  transition: background-color 0.15s ease;
  border: none;
  flex-shrink: 0;

  &:hover {
    background: var(--sk-border-strong);
  }

  &:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px var(--sk-primary);
  }

  &[data-state='checked'] {
    background: var(--sk-primary);
  }

  &[data-state='checked']:hover {
    background: var(--sk-primary-hover);
  }

  &[data-disabled] {
    opacity: 0.5;
    cursor: not-allowed;
  }
`;

// Switch Thumb (the sliding circle)
export const SwitchThumb = styled(RadixSwitch.Thumb)`
  display: block;
  width: 18px;
  height: 18px;
  background: var(--sk-panel-bg);
  border-radius: 9999px;
  box-shadow: 0 2px 4px rgba(0, 0, 0, 0.2);
  transition: transform 0.15s ease;
  transform: translateX(3px);
  will-change: transform;

  &[data-state='checked'] {
    transform: translateX(21px);
  }
`;

// Label wrapper for switch + label combinations
export const SwitchLabel = styled.label`
  display: flex;
  align-items: center;
  gap: 12px;
  cursor: pointer;
  font-size: 14px;
  color: var(--sk-text);
  user-select: none;

  &:has([data-disabled]) {
    cursor: not-allowed;
    opacity: 0.6;
  }
`;

// Convenience component that combines switch with label
interface SwitchWithLabelProps {
  id?: string;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  disabled?: boolean;
  label: React.ReactNode;
}

export const SwitchWithLabel: React.FC<SwitchWithLabelProps> = ({
  id,
  checked,
  onCheckedChange,
  disabled = false,
  label,
}) => {
  return (
    <SwitchLabel htmlFor={id}>
      <SwitchRoot id={id} checked={checked} onCheckedChange={onCheckedChange} disabled={disabled}>
        <SwitchThumb />
      </SwitchRoot>
      <span>{label}</span>
    </SwitchLabel>
  );
};

// Re-export Radix primitives
export const Switch = RadixSwitch.Root;
