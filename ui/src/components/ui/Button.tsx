// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Unified Button component with consistent styling and accessibility features.
 * Provides standard button variants with high-contrast hover effects.
 */

import styled from '@emotion/styled';
import React from 'react';

export type ButtonVariant = 'primary' | 'secondary' | 'danger' | 'ghost' | 'icon';
export type ButtonSize = 'small' | 'medium' | 'large';

interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  active?: boolean;
}

// Helper: Get size-specific styles
function getSizeStyles(size?: ButtonSize): string {
  switch (size) {
    case 'small':
      return `
        padding: 4px 8px;
        font-size: 12px;
      `;
    case 'large':
      return `
        padding: 10px 20px;
        font-size: 16px;
      `;
    default: // medium
      return `
        padding: 8px 16px;
        font-size: 14px;
      `;
  }
}

// Helper: Get variant-specific styles
function getVariantStyles(variant?: ButtonVariant, active?: boolean, size?: ButtonSize): string {
  switch (variant) {
    case 'primary':
      return `
        background: ${active ? 'var(--sk-primary-hover)' : 'var(--sk-primary)'};
        border-color: ${active ? 'var(--sk-primary-hover)' : 'var(--sk-primary)'};
        color: var(--sk-primary-contrast);

        &:hover:not(:disabled) {
          background: var(--sk-primary-hover);
          border-color: var(--sk-primary-hover);
        }
      `;

    case 'danger':
      return `
        background: var(--sk-danger);
        border-color: var(--sk-danger);
        color: var(--sk-text-white);

        &:hover:not(:disabled) {
          background: var(--sk-danger-hover);
          border-color: var(--sk-danger-hover);
        }
      `;

    case 'ghost':
      return `
        background: transparent;
        border-color: var(--sk-border);
        color: var(--sk-text);

        &:hover:not(:disabled) {
          background: var(--sk-hover-bg);
          border-color: var(--sk-border-strong);
        }
      `;

    case 'icon':
      return `
        padding: ${size === 'small' ? '4px' : '6px'};
        background: var(--sk-panel-bg);
        border-color: var(--sk-border);
        color: var(--sk-text);

        &:hover:not(:disabled) {
          background: var(--sk-hover-bg);
          border-color: var(--sk-primary);
        }
      `;

    default: // secondary
      return `
        background: ${active ? 'var(--sk-primary)' : 'var(--sk-panel-bg)'};
        border-color: ${active ? 'var(--sk-primary)' : 'var(--sk-border)'};
        color: ${active ? 'var(--sk-primary-contrast)' : 'var(--sk-text)'};

        &:hover:not(:disabled) {
          background: ${active ? 'var(--sk-primary-hover)' : 'var(--sk-hover-bg)'};
          border-color: ${active ? 'var(--sk-primary-hover)' : 'var(--sk-border-strong)'};
        }
      `;
  }
}

const StyledButton = styled.button<ButtonProps>`
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  border-radius: 6px;
  font-family: var(--sk-font-ui);
  font-weight: 600;
  cursor: pointer;
  white-space: nowrap;
  transition: none;
  border: 1px solid transparent;

  /* Size variants */
  ${(props) => getSizeStyles(props.size)}

  /* Style variants */
  ${(props) => getVariantStyles(props.variant, props.active, props.size)}

  /* Focus state for keyboard navigation */
  &:focus-visible {
    outline: none;
    box-shadow: var(--sk-focus-ring);
  }

  /* Disabled state */
  &:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
`;

export const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ variant = 'secondary', size = 'medium', active = false, children, ...props }, ref) => {
    return (
      <StyledButton ref={ref} variant={variant} size={size} active={active} {...props}>
        {children}
      </StyledButton>
    );
  }
);

Button.displayName = 'Button';
