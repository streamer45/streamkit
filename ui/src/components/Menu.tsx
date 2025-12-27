// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import * as DropdownMenu from '@radix-ui/react-dropdown-menu';
import React from 'react';

const StyledContent = styled(DropdownMenu.Content)`
  background-color: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  box-shadow: 0 8px 24px var(--sk-shadow);
  color: var(--sk-text);
  padding: 4px;
  min-width: 160px;
  z-index: 1000;

  /* Animation */
  animation-duration: 0.15s;
  animation-timing-function: cubic-bezier(0.16, 1, 0.3, 1);

  &[data-state='open'] {
    animation-name: slideUpAndFade;
  }

  @keyframes slideUpAndFade {
    from {
      opacity: 0;
      transform: translateY(2px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }
`;

const StyledItem = styled(DropdownMenu.Item, {
  shouldForwardProp: (prop) => prop !== 'danger',
})<{ danger?: boolean }>`
  padding: 8px 12px;
  border: none;
  background: none;
  text-align: left;
  cursor: pointer;
  font-size: 14px;
  border-radius: 4px;
  white-space: nowrap;
  color: ${(props) => (props.danger ? 'var(--sk-danger)' : 'var(--sk-text)')};
  outline: none;
  user-select: none;

  &[data-highlighted] {
    background-color: var(--sk-hover-bg);
  }

  &[data-disabled] {
    opacity: 0.5;
    pointer-events: none;
  }
`;

export const MenuItem = StyledItem;

interface MenuProps {
  top?: number | false;
  left?: number | false;
  right?: number | false;
  bottom?: number | false;
  onClose: () => void;
  children: React.ReactNode;
}

export const Menu: React.FC<MenuProps> = ({ top, left, right, bottom, onClose, children }) => {
  return (
    <DropdownMenu.Root open onOpenChange={(open) => !open && onClose()}>
      <DropdownMenu.Portal>
        <StyledContent
          // Prevent Radix from positioning - we handle it manually
          style={{
            position: 'fixed',
            top: top === false ? undefined : top,
            left: left === false ? undefined : left,
            right: right === false ? undefined : right,
            bottom: bottom === false ? undefined : bottom,
            // Remove transform that Radix applies for proper cursor positioning
            transform: 'none',
          }}
          onEscapeKeyDown={onClose}
          onPointerDownOutside={onClose}
          onInteractOutside={onClose}
        >
          {children}
        </StyledContent>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
};
