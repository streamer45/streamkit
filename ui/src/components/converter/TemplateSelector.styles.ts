// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Styled components for TemplateSelector.
 *
 * Extracted to a separate file to keep the main component under the
 * file-length lint budget.
 */

import styled from '@emotion/styled';
import * as RadixRadioGroup from '@radix-ui/react-radio-group';

import { RadioLabel } from '@/components/ui/RadioGroup';

export const SelectorContainer = styled.div`
  width: 100%;
`;

export const Controls = styled.div`
  display: flex;
  gap: 12px;
  align-items: center;
  justify-content: space-between;
  flex-wrap: wrap;
  margin-bottom: 12px;
`;

export const SearchInput = styled.input`
  flex: 1;
  min-width: 220px;
  padding: 10px 12px;
  font-size: 14px;
  background: var(--sk-bg);
  color: var(--sk-text);
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  font-family: inherit;

  &:focus {
    outline: none;
    border-color: var(--sk-primary);
  }

  &::placeholder {
    color: var(--sk-text-muted);
  }
`;

export const FilterGroup = styled.div`
  display: inline-flex;
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  overflow: hidden;
  background: var(--sk-panel-bg);
`;

export const FilterButton = styled.button<{ active?: boolean }>`
  padding: 8px 12px;
  font-size: 13px;
  font-weight: 700;
  border: none;
  cursor: pointer;
  transition: none;
  background: ${(props) => (props.active ? 'var(--sk-primary)' : 'transparent')};
  color: ${(props) => (props.active ? 'var(--sk-primary-contrast)' : 'var(--sk-text)')};

  &:hover {
    background: ${(props) => (props.active ? 'var(--sk-primary-hover)' : 'var(--sk-hover-bg)')};
  }

  &:focus-visible {
    outline: 2px solid var(--sk-primary);
    outline-offset: -2px;
  }
`;

export const HiddenSelectionHint = styled.div`
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 12px;
  margin-bottom: 12px;
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  background: var(--sk-panel-bg);
  color: var(--sk-text-muted);
  font-size: 13px;
`;

export const HintButton = styled.button`
  border: none;
  background: none;
  color: var(--sk-primary);
  font-weight: 700;
  cursor: pointer;
  padding: 0;

  &:hover {
    color: var(--sk-primary-hover);
  }

  &:focus-visible {
    outline: 2px solid var(--sk-primary);
    outline-offset: 2px;
    border-radius: 4px;
  }
`;

export const Section = styled.div`
  display: flex;
  flex-direction: column;
  gap: 12px;
  margin-bottom: 18px;
`;

export const SectionHeader = styled.div`
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 12px;
  color: var(--sk-text-muted);
  font-size: 12px;
  font-weight: 800;
  letter-spacing: 0.08em;
  text-transform: uppercase;
`;

export const SectionCount = styled.span`
  font-weight: 700;
  letter-spacing: 0.02em;
  text-transform: none;
`;

export const EmptyState = styled.div`
  padding: 16px;
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  background: var(--sk-panel-bg);
  color: var(--sk-text-muted);
  font-size: 14px;
`;

export const TemplateGrid = styled.div`
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(250px, 1fr));
  gap: 16px;
`;

export const TemplateCard = styled(RadioLabel)`
  padding: 20px;
  background: var(--sk-panel-bg);
  border: 2px solid var(--sk-border);
  border-radius: 8px;
  cursor: pointer;
  text-align: left;
  display: flex;
  gap: 12px;
  transition: none;
  align-items: flex-start;

  &:hover {
    border-color: var(--sk-border-strong);
    background: var(--sk-hover-bg);
  }

  &:has([data-state='checked']) {
    background: var(--sk-primary);
    color: var(--sk-primary-contrast);
    border-color: var(--sk-primary);
  }

  &:has([data-state='checked']):hover {
    background: var(--sk-primary-hover);
    border-color: var(--sk-primary-hover);
  }
`;

export const TemplateContent = styled.div`
  display: flex;
  flex-direction: column;
  gap: 8px;
  flex: 1;
`;

export const TemplateHeader = styled.div`
  display: flex;
  align-items: center;
  gap: 8px;
`;

export const TemplateName = styled.div`
  font-weight: 600;
  font-size: 16px;
`;

export const TemplateBadge = styled.span<{ variant: 'system' | 'user' }>`
  font-size: 11px;
  font-weight: 700;
  padding: 3px 10px;
  border-radius: 4px;
  text-transform: uppercase;
  letter-spacing: 0.5px;
  white-space: nowrap;
  background: ${(props) => (props.variant === 'system' ? '#4caf50' : '#2196f3')};
  color: #ffffff;

  /* Adjust for selected state - use high contrast */
  [data-state='checked'] & {
    background: rgba(0, 0, 0, 0.3);
    color: #ffffff;
    border: 1px solid rgba(255, 255, 255, 0.6);
    padding: 2px 9px; /* Account for border */
  }
`;

export const TemplateDescription = styled.div`
  font-size: 13px;
  line-height: 1.4;
  color: inherit;
  opacity: 0.9;
`;

export const FacetBar = styled.div`
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin-bottom: 16px;
`;

export const FacetRow = styled.div`
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
`;

export const FacetRowLabel = styled.span`
  color: var(--sk-text-muted);
  font-size: 11px;
  font-weight: 800;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  margin-right: 4px;
`;

export const FacetChip = styled.button<{ active?: boolean }>`
  padding: 5px 12px;
  font-size: 13px;
  font-weight: 600;
  border-radius: 999px;
  cursor: pointer;
  transition: none;
  background: ${(props) => (props.active ? 'var(--sk-primary)' : 'var(--sk-panel-bg)')};
  color: ${(props) => (props.active ? 'var(--sk-primary-contrast)' : 'var(--sk-text)')};
  border: 1px solid ${(props) => (props.active ? 'var(--sk-primary)' : 'var(--sk-border)')};

  &:hover {
    border-color: ${(props) =>
      props.active ? 'var(--sk-primary-hover)' : 'var(--sk-border-strong)'};
    background: ${(props) => (props.active ? 'var(--sk-primary-hover)' : 'var(--sk-hover-bg)')};
  }

  &:focus-visible {
    outline: 2px solid var(--sk-primary);
    outline-offset: 2px;
  }
`;

// Plain div twin of TemplateCard for multi-variant groups: the card itself is
// not a single radio target, so selection happens through the variant pills.
export const GroupCard = styled.div`
  padding: 20px;
  background: var(--sk-panel-bg);
  border: 2px solid var(--sk-border);
  border-radius: 8px;
  text-align: left;
  display: flex;
  flex-direction: column;
  gap: 12px;

  &[data-selected] {
    border-color: var(--sk-primary);
  }
`;

export const VariantSelector = styled.div`
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
`;

export const VariantOption = styled(RadixRadioGroup.Item)`
  padding: 5px 12px;
  font-size: 13px;
  font-weight: 600;
  border-radius: 999px;
  cursor: pointer;
  background: var(--sk-bg);
  color: var(--sk-text);
  border: 1px solid var(--sk-border);

  &:hover {
    border-color: var(--sk-border-strong);
    background: var(--sk-hover-bg);
  }

  &[data-state='checked'] {
    background: var(--sk-primary);
    color: var(--sk-primary-contrast);
    border-color: var(--sk-primary);
  }

  &:focus-visible {
    outline: 2px solid var(--sk-primary);
    outline-offset: 2px;
  }
`;
