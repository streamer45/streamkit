// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import * as RadixSelect from '@radix-ui/react-select';
import React from 'react';

// Select Trigger (the button that opens the dropdown)
export const SelectTrigger = styled(RadixSelect.Trigger)`
  display: inline-flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 8px 12px;
  background: var(--sk-bg);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  color: var(--sk-text);
  font-size: 14px;
  font-family: inherit;
  cursor: pointer;
  min-width: 150px;
  transition: all 0.15s ease;

  &:hover {
    border-color: var(--sk-primary);
    background: var(--sk-hover-bg);
  }

  &:focus-visible {
    outline: none;
    border-color: var(--sk-primary);
    box-shadow: 0 0 0 2px rgba(14, 165, 233, 0.2);
  }

  &[data-placeholder] {
    color: var(--sk-text-muted);
  }

  &[data-disabled] {
    opacity: 0.5;
    cursor: not-allowed;
  }
`;

// Chevron icon
export const SelectIcon = styled(RadixSelect.Icon)`
  color: var(--sk-text-muted);
  display: flex;
  align-items: center;
`;

// Content (dropdown menu)
export const SelectContent = styled(RadixSelect.Content)`
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  box-shadow: 0 8px 24px var(--sk-shadow);
  padding: 4px;
  z-index: 1000;
  max-height: 300px;
  overflow: auto;

  animation: slideDownAndFade 0.15s ease-out;

  @keyframes slideDownAndFade {
    from {
      opacity: 0;
      transform: translateY(-2px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }
`;

// Viewport wrapper
export const SelectViewport = styled(RadixSelect.Viewport)`
  padding: 0;
`;

// Individual option item
export const SelectItem = styled(RadixSelect.Item)`
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px 8px 28px;
  border-radius: 4px;
  color: var(--sk-text);
  font-size: 14px;
  cursor: pointer;
  position: relative;
  outline: none;
  user-select: none;

  &[data-highlighted] {
    background: var(--sk-hover-bg);
  }

  &[data-disabled] {
    opacity: 0.5;
    pointer-events: none;
  }
`;

// Item indicator (checkmark)
export const SelectItemIndicator = styled(RadixSelect.ItemIndicator)`
  position: absolute;
  left: 8px;
  display: flex;
  align-items: center;
  color: var(--sk-primary);

  svg {
    width: 12px;
    height: 12px;
  }
`;

// Item text
export const SelectItemText = styled(RadixSelect.ItemText)``;

// Separator
export const SelectSeparator = styled(RadixSelect.Separator)`
  height: 1px;
  background: var(--sk-border);
  margin: 4px 0;
`;

// Label for option groups
export const SelectLabel = styled(RadixSelect.Label)`
  padding: 6px 12px;
  font-size: 12px;
  font-weight: 600;
  color: var(--sk-text-muted);
`;

// Convenience wrapper component
interface SelectOptionProps {
  value: string;
  children: React.ReactNode;
  disabled?: boolean;
}

export const SelectOption: React.FC<SelectOptionProps> = ({ value, children, disabled }) => {
  return (
    <SelectItem value={value} disabled={disabled}>
      <SelectItemIndicator>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3">
          <polyline points="20 6 9 17 4 12" />
        </svg>
      </SelectItemIndicator>
      <SelectItemText>{children}</SelectItemText>
    </SelectItem>
  );
};

// Chevron component
export const ChevronIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
    <polyline points="6 9 12 15 18 9" />
  </svg>
);

// Re-export Radix primitives
export const Select = RadixSelect.Root;
export const SelectValue = RadixSelect.Value;
export const SelectGroup = RadixSelect.Group;
export const SelectPortal = RadixSelect.Portal;
