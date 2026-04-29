// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Styled components for NodePalette search and filter UI.
 *
 * Extracted to keep the main component under the 500-line max-lines limit.
 */

import styled from '@emotion/styled';

export const SearchWrapper = styled.div`
  position: relative;
  margin-top: 8px;
`;

export const SearchIcon = styled.div`
  position: absolute;
  left: 8px;
  top: 50%;
  transform: translateY(-50%);
  color: var(--sk-text-muted);
  pointer-events: none;
  display: flex;
  align-items: center;
`;

export const ClearButton = styled.button`
  position: absolute;
  right: 6px;
  top: 50%;
  transform: translateY(-50%);
  background: none;
  border: none;
  color: var(--sk-text-muted);
  cursor: pointer;
  padding: 2px;
  display: flex;
  align-items: center;
  border-radius: 4px;

  &:hover {
    color: var(--sk-text);
    background: var(--sk-hover-bg);
  }
`;

export const SearchInput = styled.input`
  width: 100%;
  padding: 6px 28px 6px 30px;
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  background: var(--sk-panel-bg);
  color: var(--sk-text);
  font-size: 12px;
  outline: none;
  box-sizing: border-box;

  &::placeholder {
    color: var(--sk-text-muted);
  }

  &:focus {
    border-color: var(--sk-primary);
  }
`;

export const FilterChipsRow = styled.div`
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  margin-top: 6px;
`;

export const FilterChip = styled.button<{ $active: boolean }>`
  padding: 2px 8px;
  border-radius: 999px;
  border: 1px solid ${(props) => (props.$active ? 'var(--sk-primary)' : 'var(--sk-border)')};
  background: ${(props) => (props.$active ? 'var(--sk-primary)' : 'transparent')};
  color: ${(props) => (props.$active ? 'var(--sk-text-white)' : 'var(--sk-text-muted)')};
  font-size: 10px;
  font-weight: 600;
  cursor: pointer;
  transition: none;
  text-transform: capitalize;
  user-select: none;

  &:hover {
    border-color: var(--sk-primary);
  }
`;

export const CategoryBreadcrumb = styled.span`
  font-size: 10px;
  color: var(--sk-text-muted);
  font-weight: 400;
`;
