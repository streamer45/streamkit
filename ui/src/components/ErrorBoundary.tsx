// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import { Component, type ErrorInfo, type ReactNode } from 'react';

import { getLogger } from '../utils/logger';

const logger = getLogger('ErrorBoundary');

const ErrorContainer = styled.div`
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  height: 100vh;
  padding: 24px;
  background-color: var(--sk-bg);
  color: var(--sk-text);
`;

const ErrorTitle = styled.h1`
  font-size: 24px;
  font-weight: 600;
  margin-bottom: 16px;
  color: var(--sk-error);
`;

const ErrorMessage = styled.p`
  font-size: 14px;
  color: var(--sk-text-muted);
  margin-bottom: 24px;
  text-align: center;
  max-width: 500px;
`;

const ErrorDetails = styled.details`
  margin-bottom: 24px;
  max-width: 600px;
  width: 100%;
`;

const ErrorSummary = styled.summary`
  cursor: pointer;
  color: var(--sk-text-muted);
  font-size: 12px;
  margin-bottom: 8px;

  &:hover {
    color: var(--sk-text);
  }
`;

const ErrorStack = styled.pre`
  font-size: 11px;
  background-color: var(--sk-surface);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  padding: 12px;
  overflow-x: auto;
  white-space: pre-wrap;
  word-break: break-word;
  color: var(--sk-text-muted);
`;

const ReloadButton = styled.button`
  padding: 10px 20px;
  font-size: 14px;
  font-weight: 500;
  background-color: var(--sk-primary);
  color: white;
  border: none;
  border-radius: 6px;
  cursor: pointer;
  transition: background-color 0.2s;

  &:hover {
    background-color: var(--sk-primary-hover);
  }
`;

interface Props {
  children: ReactNode;
  fallback?: ReactNode;
}

interface State {
  hasError: boolean;
  error: Error | null;
}

/**
 * Error Boundary component that catches JavaScript errors in child components
 * and displays a fallback UI instead of crashing the entire application.
 */
export class ErrorBoundary extends Component<Props, State> {
  constructor(props: Props) {
    super(props);
    this.state = { hasError: false, error: null };
  }

  static getDerivedStateFromError(error: Error): State {
    return { hasError: true, error };
  }

  componentDidCatch(error: Error, errorInfo: ErrorInfo): void {
    logger.error('Uncaught error:', error, errorInfo.componentStack);
  }

  handleReload = (): void => {
    window.location.reload();
  };

  render(): ReactNode {
    if (this.state.hasError) {
      if (this.props.fallback) {
        return this.props.fallback;
      }

      return (
        <ErrorContainer>
          <ErrorTitle>Something went wrong</ErrorTitle>
          <ErrorMessage>
            An unexpected error occurred. Please try reloading the page. If the problem persists,
            check the browser console for more details.
          </ErrorMessage>
          {this.state.error && (
            <ErrorDetails>
              <ErrorSummary>Error details</ErrorSummary>
              <ErrorStack>
                {this.state.error.message}
                {this.state.error.stack && `\n\n${this.state.error.stack}`}
              </ErrorStack>
            </ErrorDetails>
          )}
          <ReloadButton onClick={this.handleReload}>Reload page</ReloadButton>
        </ErrorContainer>
      );
    }

    return this.props.children;
  }
}
