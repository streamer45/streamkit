// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import * as RadixAccordion from '@radix-ui/react-accordion';

// Accordion Root
export const AccordionRoot = styled(RadixAccordion.Root)`
  width: 100%;
`;

// Accordion Item
export const AccordionItem = styled(RadixAccordion.Item)`
  border-bottom: 1px solid var(--sk-border);

  &:first-of-type {
    border-top: 1px solid var(--sk-border);
  }
`;

// Accordion Trigger (Header/Button)
export const AccordionTrigger = styled(RadixAccordion.Trigger)`
  width: 100%;
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 16px;
  background: transparent;
  border: none;
  cursor: pointer;
  font-size: 14px;
  font-weight: 600;
  color: var(--sk-text);
  font-family: inherit;
  text-align: left;
  transition: background-color 0.15s ease;

  &:hover {
    background: var(--sk-hover-bg);
  }

  &:focus-visible {
    outline: none;
    background: var(--sk-hover-bg);
    box-shadow: inset 0 0 0 2px var(--sk-primary);
  }

  &[data-state='open'] svg {
    transform: rotate(180deg);
  }

  svg {
    transition: transform 0.2s ease;
    flex-shrink: 0;
    color: var(--sk-text-muted);
  }
`;

// Chevron icon component
export const AccordionChevron = () => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
    <polyline points="6 9 12 15 18 9" />
  </svg>
);

// Accordion Content
export const AccordionContent = styled(RadixAccordion.Content)`
  overflow: hidden;
  color: var(--sk-text);

  &[data-state='open'] {
    animation: slideDown 0.2s ease-out;
  }

  &[data-state='closed'] {
    animation: slideUp 0.2s ease-out;
  }

  @keyframes slideDown {
    from {
      height: 0;
      opacity: 0;
    }
    to {
      height: var(--radix-accordion-content-height);
      opacity: 1;
    }
  }

  @keyframes slideUp {
    from {
      height: var(--radix-accordion-content-height);
      opacity: 1;
    }
    to {
      height: 0;
      opacity: 0;
    }
  }
`;

// Content inner wrapper for padding
export const AccordionContentInner = styled.div`
  padding: 12px 16px;
`;

// Re-export Radix primitives
export const Accordion = RadixAccordion.Root;
export const AccordionHeader = RadixAccordion.Header;
