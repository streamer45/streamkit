// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { keyframes } from '@emotion/react';
import styled from '@emotion/styled';
import React from 'react';

const slideIn = keyframes`
  from {
    transform: translateY(-20px);
    opacity: 0;
  }
  to {
    transform: translateY(0);
    opacity: 1;
  }
`;

const ProgressContainer = styled.div`
  position: fixed;
  top: 80px;
  right: 40px;
  z-index: 1000;
  min-width: 400px;
  max-width: 500px;
  padding: 20px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.3);
  display: flex;
  flex-direction: column;
  gap: 12px;
  animation: ${slideIn} 0.3s ease-out;
`;

const ProgressText = styled.div`
  color: var(--sk-text);
  font-size: 14px;
  font-weight: 500;
  display: flex;
  align-items: center;
  gap: 10px;
`;

const StatusIcon = styled.div<{ success: boolean }>`
  font-size: 24px;
  width: 32px;
  height: 32px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: 50%;
  background: var(--sk-overlay-medium);
  color: ${(props) => (props.success ? 'var(--sk-success)' : 'var(--sk-danger)')};
`;

const StatusText = styled.div<{ success: boolean }>`
  color: var(--sk-text);
  font-size: 14px;
  font-weight: 500;
  flex: 1;
`;

export type ConversionStatus = 'idle' | 'processing' | 'success' | 'error';

interface ConversionProgressProps {
  status: ConversionStatus;
  message?: string;
}

export const ConversionProgress: React.FC<ConversionProgressProps> = ({ status, message }) => {
  // Don't show toast during processing - the button spinner is enough
  if (status === 'idle' || status === 'processing') {
    return null;
  }

  return (
    <ProgressContainer>
      {status === 'success' && (
        <ProgressText>
          <StatusIcon success={true}>✓</StatusIcon>
          <StatusText success={true}>
            {message || 'Conversion complete! Download started.'}
          </StatusText>
        </ProgressText>
      )}

      {status === 'error' && (
        <ProgressText>
          <StatusIcon success={false}>✗</StatusIcon>
          <StatusText success={false}>
            {message || 'Conversion failed. Please try again.'}
          </StatusText>
        </ProgressText>
      )}
    </ProgressContainer>
  );
};
