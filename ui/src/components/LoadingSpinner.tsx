// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React from 'react';

const SpinnerContainer = styled.div`
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 12px;
  padding: 24px;
`;

const Spinner = styled.div`
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  width: 32px;
  height: 32px;
  border: 3px solid var(--sk-border);
  border-top-color: var(--sk-primary);
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
`;

const LoadingText = styled.div`
  color: var(--sk-text-muted);
  font-size: 14px;
  text-align: center;
  line-height: 1.5;
`;

interface LoadingSpinnerProps {
  message?: string;
  className?: string;
}

/**
 * Reusable loading spinner with optional message
 */
export const LoadingSpinner: React.FC<LoadingSpinnerProps> = ({ message, className }) => {
  return (
    <SpinnerContainer className={className}>
      <Spinner />
      {message && <LoadingText>{message}</LoadingText>}
    </SpinnerContainer>
  );
};
