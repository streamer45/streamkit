// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';

export const Container = styled.div`
  box-sizing: border-box;
  width: 100%;
  min-width: 0;
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  background: var(--sk-bg);
`;

export const ContentArea = styled.div`
  flex: 1;
  overflow-y: auto;
  display: flex;
  justify-content: center;
  min-width: 0;
  min-height: 0;
`;

export const ContentWrapper = styled.div`
  width: 100%;
  max-width: 1200px;
  padding: 40px;
  box-sizing: border-box;

  @media (max-width: 768px) {
    padding: 24px;
  }
`;

export const BottomSpacer = styled.div`
  height: 40px;
  flex-shrink: 0;

  @media (max-width: 768px) {
    height: 24px;
  }
`;

export const Card = styled.div`
  box-sizing: border-box;
  width: 100%;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 12px;
  padding: 24px;
  display: flex;
  flex-direction: column;
  gap: 18px;
  min-width: 0;
`;

export const TitleRow = styled.div`
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-wrap: wrap;
`;

export const Title = styled.h1`
  margin: 0;
  font-size: 20px;
  font-weight: 700;
  color: var(--sk-text);
`;

export const Subtle = styled.div`
  color: var(--sk-text-muted);
  font-size: 13px;
`;

export const Grid = styled.div`
  box-sizing: border-box;
  display: grid;
  grid-template-columns: 1fr;
  gap: 18px;
  min-width: 0;

  @media (min-width: 980px) {
    grid-template-columns: 1fr 1fr;
  }
`;

export const Section = styled.section`
  box-sizing: border-box;
  border: 1px solid var(--sk-border);
  border-radius: 12px;
  padding: 16px;
  background: var(--sk-bg);
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-width: 0;
  min-height: 0;
`;

export const SectionTitle = styled.h2`
  margin: 0;
  font-size: 14px;
  font-weight: 700;
  color: var(--sk-text);
`;

export const Row = styled.div`
  display: flex;
  gap: 10px;
  flex-wrap: wrap;
  align-items: center;
`;

export const Label = styled.label`
  display: flex;
  flex-direction: column;
  gap: 6px;
  font-size: 12px;
  color: var(--sk-text-muted);
  min-width: 200px;
  flex: 1 1 240px;
`;

export const Input = styled.input`
  padding: 10px 12px;
  border-radius: 10px;
  border: 1px solid var(--sk-border);
  background: var(--sk-panel-bg);
  color: var(--sk-text);
  font-size: 13px;
`;

export const TextArea = styled.textarea`
  box-sizing: border-box;
  width: 100%;
  padding: 10px 12px;
  border-radius: 10px;
  border: 1px solid var(--sk-border);
  background: var(--sk-panel-bg);
  color: var(--sk-text);
  font-size: 12px;
  min-height: 90px;
  resize: vertical;
  font-family:
    ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, 'Liberation Mono', 'Courier New',
    monospace;
`;

export const TextAreaWithCopyWrapper = styled.div`
  position: relative;
  width: 100%;
`;

export const TextAreaWithCopy = styled(TextArea)`
  padding-right: 44px;
`;

export const Select = styled.select`
  padding: 10px 12px;
  border-radius: 10px;
  border: 1px solid var(--sk-border);
  background: var(--sk-panel-bg);
  color: var(--sk-text);
  font-size: 13px;
`;

export const ErrorBox = styled.div`
  padding: 12px;
  border-radius: 10px;
  border: 1px solid var(--sk-border);
  background: color-mix(in srgb, var(--sk-danger) 10%, transparent);
  color: var(--sk-text);
  font-size: 13px;
`;

export const SuccessBox = styled.div`
  padding: 12px;
  border-radius: 10px;
  border: 1px solid var(--sk-border);
  background: color-mix(in srgb, var(--sk-primary) 12%, transparent);
  color: var(--sk-text);
  font-size: 13px;
  display: flex;
  flex-direction: column;
  gap: 8px;
`;

export const NoticeBox = styled.div`
  padding: 12px;
  border-radius: 10px;
  border: 1px solid var(--sk-border);
  background: color-mix(in srgb, var(--sk-primary) 8%, transparent);
  color: var(--sk-text);
  font-size: 13px;
`;

export const TableWrapper = styled.div`
  box-sizing: border-box;
  width: 100%;
  max-width: 100%;
  min-width: 0;
  overflow-x: auto;
`;

export const Table = styled.table`
  width: 100%;
  table-layout: fixed;
  border-collapse: collapse;
  font-size: 12px;
  color: var(--sk-text);

  th,
  td {
    text-align: left;
    padding: 10px 8px;
    border-bottom: 1px solid var(--sk-border);
    vertical-align: top;
    overflow-wrap: anywhere;
  }

  th {
    color: var(--sk-text-muted);
    font-weight: 700;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
`;

export const FilterRow = styled.div`
  display: flex;
  gap: 10px;
  margin-bottom: 12px;
`;

export const SearchInput = styled.input`
  flex: 1;
  padding: 10px 12px;
  border-radius: 10px;
  border: 1px solid var(--sk-border);
  background: var(--sk-panel-bg);
  color: var(--sk-text);
  font-size: 13px;

  &:focus {
    outline: none;
    border-color: var(--sk-primary);
    box-shadow: var(--sk-focus-ring);
  }

  &::placeholder {
    color: var(--sk-text-muted);
  }
`;

export const TableHeaderCell = styled.th<{ $isSortable?: boolean }>`
  position: relative;
  cursor: ${(props) => (props.$isSortable ? 'pointer' : 'default')};
  user-select: none;
  transition: background 0.15s ease;
  white-space: nowrap;

  &:hover {
    background: ${(props) => (props.$isSortable ? 'var(--sk-hover-bg)' : 'transparent')};
  }
`;

export const HeaderContent = styled.div`
  display: flex;
  align-items: center;
  gap: 6px;
  justify-content: space-between;
`;

export const HeaderButton = styled.button`
  all: unset;
  display: flex;
  align-items: center;
  gap: 6px;
  justify-content: space-between;
  width: 100%;
  cursor: pointer;
`;

export const SortIcon = styled.span`
  display: inline-flex;
  align-items: center;
  color: var(--sk-primary);
  flex-shrink: 0;
`;

export const JtiCell = styled.div`
  font-family:
    ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, 'Liberation Mono', 'Courier New',
    monospace;
  font-size: 11px;
  width: 100%;
  max-width: 100%;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
`;

export const Badge = styled.span<{ $variant?: 'success' | 'danger' | 'warning' | 'neutral' }>`
  display: inline-block;
  padding: 2px 8px;
  border-radius: 4px;
  font-size: 11px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.03em;
  white-space: nowrap;

  ${(props) => {
    switch (props.$variant) {
      case 'success':
        return `
          background: color-mix(in srgb, var(--sk-success) 15%, transparent);
          color: var(--sk-success);
        `;
      case 'danger':
        return `
          background: color-mix(in srgb, var(--sk-danger) 15%, transparent);
          color: var(--sk-danger);
        `;
      case 'warning':
        return `
          background: color-mix(in srgb, var(--sk-warning) 15%, transparent);
          color: var(--sk-warning);
        `;
      default:
        return `
          background: var(--sk-hover-bg);
          color: var(--sk-text-muted);
        `;
    }
  }}
`;

export const ResizeHandle = styled.div<{ $isResizing?: boolean }>`
  position: absolute;
  right: 0;
  top: 0;
  height: 100%;
  width: 8px;
  background: ${(props) => (props.$isResizing ? 'var(--sk-primary)' : 'transparent')};
  cursor: col-resize;
  user-select: none;
  touch-action: none;
  z-index: 2;
  opacity: ${(props) => (props.$isResizing ? 1 : 0.2)};
  transition: opacity 0.15s ease;

  &:hover {
    opacity: 1;
    background: var(--sk-primary);
  }
`;
