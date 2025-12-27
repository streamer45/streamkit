// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React, { useState } from 'react';

import { Button as BaseButton } from '@/components/ui/Button';
import { componentsLogger } from '@/utils/logger';

const StyledCopyButton = styled(BaseButton)`
  position: absolute;
  top: 4px;
  right: 4px;
  padding: 4px;
  z-index: 10;

  &.copied {
    color: var(--sk-primary);
    border-color: var(--sk-primary);
  }

  svg {
    width: 14px;
    height: 14px;
  }
`;

interface CopyButtonProps {
  text: string;
  className?: string;
}

export const CopyButton: React.FC<CopyButtonProps> = ({ text, className }) => {
  const [copied, setCopied] = useState(false);

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch (err) {
      componentsLogger.error('Failed to copy:', err);
    }
  };

  return (
    <StyledCopyButton
      variant="icon"
      size="small"
      onClick={handleCopy}
      className={`${className || ''} ${copied ? 'copied' : ''}`}
      title={copied ? 'Copied!' : 'Copy to clipboard'}
    >
      {copied ? (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
          <polyline points="20 6 9 17 4 12" />
        </svg>
      ) : (
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
          <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
          <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
        </svg>
      )}
    </StyledCopyButton>
  );
};
