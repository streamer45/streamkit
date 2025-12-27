// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import * as RadixDialog from '@radix-ui/react-dialog';

// Styled Components
export const DialogOverlay = styled(RadixDialog.Overlay)`
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.5);
  backdrop-filter: blur(4px);
  z-index: 1000;
  animation: overlayShow 0.15s cubic-bezier(0.16, 1, 0.3, 1);

  @keyframes overlayShow {
    from {
      opacity: 0;
    }
    to {
      opacity: 1;
    }
  }
`;

export const DialogContent = styled(RadixDialog.Content)`
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  max-width: 450px;
  width: 90%;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.3);
  z-index: 1001;
  animation: contentShow 0.15s cubic-bezier(0.16, 1, 0.3, 1);
  overflow: hidden;

  &:focus {
    outline: none;
  }

  @keyframes contentShow {
    from {
      opacity: 0;
      transform: translate(-50%, -48%) scale(0.96);
    }
    to {
      opacity: 1;
      transform: translate(-50%, -50%) scale(1);
    }
  }
`;

export const DialogHeader = styled.div`
  padding: 16px 20px;
  border-bottom: 1px solid var(--sk-border);
`;

export const DialogTitle = styled(RadixDialog.Title)`
  margin: 0;
  font-size: 16px;
  font-weight: 600;
  color: var(--sk-text);
`;

export const DialogDescription = styled(RadixDialog.Description)`
  margin: 0;
  font-size: 14px;
  color: var(--sk-text-muted);
  line-height: 1.5;
`;

export const DialogBody = styled.div`
  padding: 16px 20px;
`;

export const DialogFooter = styled.div`
  display: flex;
  gap: 8px;
  justify-content: flex-end;
  padding: 12px 20px;
  border-top: 1px solid var(--sk-border);
  background: var(--sk-sidebar-bg);
`;

// Form Components
export const FormGroup = styled.div<{ spacing?: 'normal' | 'compact' }>`
  margin-bottom: ${(props) => (props.spacing === 'compact' ? '12px' : '16px')};

  &:last-child {
    margin-bottom: 0;
  }
`;

export const Label = styled.label`
  display: block;
  margin-bottom: 8px;
  font-size: 13px;
  font-weight: 500;
  color: var(--sk-text);
`;

export const Input = styled.input`
  width: 100%;
  padding: 8px 12px;
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  background: var(--sk-bg);
  color: var(--sk-text);
  font-size: 14px;
  font-family: inherit;
  box-sizing: border-box;

  &:focus {
    outline: none;
    border-color: var(--sk-primary);
    box-shadow: 0 0 0 2px rgba(14, 165, 233, 0.1);
  }

  &:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  &::placeholder {
    color: var(--sk-text-muted);
  }
`;

export const Textarea = styled.textarea`
  width: 100%;
  padding: 8px 12px;
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  background: var(--sk-bg);
  color: var(--sk-text);
  font-size: 14px;
  font-family: inherit;
  box-sizing: border-box;
  resize: vertical;
  min-height: 80px;

  &:focus {
    outline: none;
    border-color: var(--sk-primary);
    box-shadow: 0 0 0 2px rgba(14, 165, 233, 0.1);
  }

  &:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  &::placeholder {
    color: var(--sk-text-muted);
  }
`;

// Re-export Radix components we need
export const Dialog = RadixDialog.Root;
export const DialogPortal = RadixDialog.Portal;
export const DialogClose = RadixDialog.Close;
export const DialogTrigger = RadixDialog.Trigger;
