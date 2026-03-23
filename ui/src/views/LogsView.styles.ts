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
  display: flex;
  justify-content: center;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
`;

export const ContentWrapper = styled.div`
  width: 100%;
  padding: 24px 32px;
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
  min-height: 0;

  @media (max-width: 768px) {
    padding: 16px;
  }
`;

export const Card = styled.div`
  box-sizing: border-box;
  width: 100%;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 12px;
  padding: 20px;
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-width: 0;
  flex: 1;
  min-height: 0;
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

export const FilterBar = styled.div`
  display: flex;
  gap: 8px;
  align-items: center;
  flex-wrap: wrap;
`;

export const SearchInput = styled.input`
  flex: 1;
  min-width: 200px;
  padding: 8px 12px;
  border-radius: 8px;
  border: 1px solid var(--sk-border);
  background: var(--sk-bg);
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

export const LevelSelect = styled.select`
  padding: 8px 12px;
  border-radius: 8px;
  border: 1px solid var(--sk-border);
  background: var(--sk-bg);
  color: var(--sk-text);
  font-size: 13px;
`;

export const PageSizeSelect = styled.select`
  padding: 8px 12px;
  border-radius: 8px;
  border: 1px solid var(--sk-border);
  background: var(--sk-bg);
  color: var(--sk-text);
  font-size: 13px;
`;

export const LogContainer = styled.div<{ $wrap?: boolean }>`
  flex: 1;
  min-height: 120px;
  overflow-y: auto;
  overflow-x: ${(props) => (props.$wrap !== false ? 'hidden' : 'auto')};
  background: var(--sk-bg);
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  font-family:
    ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, 'Liberation Mono', 'Courier New',
    monospace;
  font-size: 12px;
  line-height: 1.5;
  padding: 8px 0;
  --log-wrap: ${(props) => (props.$wrap !== false ? 'pre-wrap' : 'pre')};
  --log-word-break: ${(props) => (props.$wrap !== false ? 'break-all' : 'normal')};
`;

export const LogLine = styled.div<{ $level?: string }>`
  white-space: var(--log-wrap, pre-wrap);
  word-break: var(--log-word-break, break-all);
  padding: 1px 12px;

  ${(props) => {
    switch (props.$level) {
      case 'error':
        return `
          color: var(--sk-danger);
          background: color-mix(in srgb, var(--sk-danger) 10%, transparent);
        `;
      case 'warn':
        return `
          color: var(--sk-warning);
          background: color-mix(in srgb, var(--sk-warning) 8%, transparent);
        `;
      case 'debug':
      case 'trace':
        return 'color: var(--sk-text-muted);';
      default:
        return 'color: var(--sk-text);';
    }
  }}
`;

export const PaginationRow = styled.div`
  display: flex;
  gap: 10px;
  align-items: center;
  justify-content: space-between;
  flex-wrap: wrap;
`;

export const PaginationInfo = styled.span`
  font-size: 12px;
  color: var(--sk-text-muted);
`;

export const EmptyState = styled.div`
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 40px;
  color: var(--sk-text-muted);
  font-size: 14px;
`;

export const ErrorBox = styled.div`
  padding: 12px;
  border-radius: 10px;
  border: 1px solid var(--sk-border);
  background: color-mix(in srgb, var(--sk-danger) 10%, transparent);
  color: var(--sk-text);
  font-size: 13px;
`;

export const LiveIndicator = styled.span<{ $active: boolean }>`
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  font-weight: 600;
  color: ${(props) => (props.$active ? 'var(--sk-success)' : 'var(--sk-text-muted)')};

  &::before {
    content: '';
    display: inline-block;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: ${(props) => (props.$active ? 'var(--sk-success)' : 'var(--sk-text-muted)')};
    ${(props) =>
      props.$active
        ? `animation: pulse 1.5s ease-in-out infinite;
           @keyframes pulse {
             0%, 100% { opacity: 1; }
             50% { opacity: 0.4; }
           }`
        : ''}
  }
`;
