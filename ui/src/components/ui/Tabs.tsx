// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import * as RadixTabs from '@radix-ui/react-tabs';

// Styled Components
export const TabsRoot = styled(RadixTabs.Root)`
  display: flex;
  flex-direction: column;
  width: 100%;
  height: 100%;
`;

export const TabsList = styled(RadixTabs.List)`
  display: flex;
  border-bottom: 1px solid var(--sk-border);
  flex-shrink: 0;
`;

export const TabsTrigger = styled(RadixTabs.Trigger)`
  flex: 1;
  padding: 10px;
  border: none;
  background: transparent;
  cursor: pointer;
  font-weight: normal;
  color: var(--sk-text);
  border-bottom: 2px solid transparent;
  margin-bottom: -1px;
  position: relative;
  font-size: 14px;
  font-family: inherit;
  transition: all 0.15s ease;

  &[data-state='active'] {
    background: var(--sk-panel-bg);
    font-weight: 600;
    border-bottom-color: var(--sk-primary);
  }

  &:hover {
    background-color: var(--sk-hover-bg);
  }

  &:focus-visible {
    outline: none;
    background-color: var(--sk-hover-bg);
    box-shadow: inset 0 0 0 2px var(--sk-primary);
  }

  &:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
`;

export const TabsContent = styled(RadixTabs.Content)`
  flex-grow: 1;
  overflow: hidden;
  outline: none;
  position: relative;
  min-height: 0;
  display: flex;
  flex-direction: column;

  &[hidden] {
    display: none;
  }

  &:focus-visible {
    outline: 2px solid var(--sk-primary);
    outline-offset: -2px;
  }
`;

// Re-export Radix primitives
export const Tabs = RadixTabs.Root;
