// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React, { createContext, useContext, useMemo } from 'react';

import { showToast, useToastStore, type ToastItem, type ToastType } from '@/stores/toastStore';

interface ToastContextValue {
  show: (message: string, type?: ToastType) => void;
  info: (message: string) => void;
  success: (message: string) => void;
  error: (message: string) => void;
}

const ToastContext = createContext<ToastContextValue | undefined>(undefined);

export const ToastProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const api = useMemo<ToastContextValue>(
    () => ({
      show: (message: string, type: ToastType = 'info') => {
        showToast(message, type);
      },
      info: (message: string) => showToast(message, 'info'),
      success: (message: string) => showToast(message, 'success'),
      error: (message: string) => showToast(message, 'error'),
    }),
    []
  );

  return (
    <ToastContext.Provider value={api}>
      {children}
      <Toaster />
    </ToastContext.Provider>
  );
};

export const useToast = (): ToastContextValue => {
  const ctx = useContext(ToastContext);
  if (!ctx) {
    throw new Error('useToast must be used within a ToastProvider');
  }
  return ctx;
};

const ToasterWrapper = styled.div`
  position: fixed;
  bottom: 20px;
  left: 50%;
  transform: translateX(-50%);
  display: flex;
  flex-direction: column;
  gap: 8px;
  z-index: 10000;
  pointer-events: none;
`;

const ToastItemWrapper = styled.div<{ type: ToastType }>`
  pointer-events: auto;
  min-width: 280px;
  max-width: 480px;
  background: var(--sk-panel-bg);
  color: var(--sk-text);
  border: 1px solid var(--sk-border);
  border-left: 4px solid
    ${(props) => {
      switch (props.type) {
        case 'success':
          return 'var(--sk-primary)';
        case 'error':
          return 'var(--sk-danger)';
        default:
          return 'var(--sk-muted)';
      }
    }};
  border-radius: 8px;
  padding: 10px 12px;
  box-shadow: 0 8px 24px var(--sk-shadow);
`;

const ToastTypeLabel = styled.span`
  text-transform: uppercase;
  font-size: 10px;
  letter-spacing: 0.5px;
  color: var(--sk-text-muted);
`;

const CloseButton = styled.button`
  border: none;
  background: transparent;
  color: var(--sk-text-muted);
  cursor: pointer;
  padding: 4px;

  &:hover,
  &:focus-visible {
    color: var(--sk-text);
    outline: none;
  }
`;

export const Toaster: React.FC<{
  toasts?: ToastItem[];
  onClose?: (id: number) => void;
}> = ({ toasts, onClose }) => {
  const storeToasts = useToastStore((s) => s.toasts);
  const removeToast = useToastStore((s) => s.removeToast);
  const effectiveToasts = toasts ?? storeToasts;
  const effectiveOnClose = onClose ?? removeToast;

  return (
    <ToasterWrapper aria-live="polite" aria-atomic="true">
      {effectiveToasts.map((t) => (
        <ToastItemWrapper key={t.id} role="status" type={t.type}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
            <ToastTypeLabel className="code-font">{t.type}</ToastTypeLabel>
            <div style={{ flex: 1 }}>{t.message}</div>
            {effectiveOnClose && (
              <CloseButton aria-label="Dismiss" onClick={() => effectiveOnClose(t.id)}>
                ✕
              </CloseButton>
            )}
          </div>
        </ToastItemWrapper>
      ))}
    </ToasterWrapper>
  );
};
