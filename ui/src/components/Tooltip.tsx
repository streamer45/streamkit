// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import * as RadixTooltip from '@radix-ui/react-tooltip';
import React from 'react';

const TooltipContent = styled(RadixTooltip.Content)`
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  color: var(--sk-text);
  font-family: var(--sk-font-ui);
  font-size: 12px;
  padding: 6px 8px;
  border-radius: 6px;
  box-shadow: 0 8px 24px var(--sk-shadow);
  z-index: 10000;
`;

const TooltipArrow = styled(RadixTooltip.Arrow)`
  fill: var(--sk-panel-bg);
`;

export const TooltipProvider = RadixTooltip.Provider;

export function SKTooltip({
  content,
  children,
  side = 'top',
  onOpenChange,
  open,
  defaultOpen,
  delayDuration = 300,
}: {
  content: React.ReactNode;
  children: React.ReactElement;
  side?: 'top' | 'bottom' | 'left' | 'right';
  onOpenChange?: (open: boolean) => void;
  open?: boolean;
  defaultOpen?: boolean;
  delayDuration?: number;
}) {
  if (!content) {
    return children;
  }
  return (
    <RadixTooltip.Root
      delayDuration={delayDuration}
      open={open}
      defaultOpen={defaultOpen}
      onOpenChange={onOpenChange}
    >
      <RadixTooltip.Trigger asChild>{children}</RadixTooltip.Trigger>
      <RadixTooltip.Portal>
        <TooltipContent side={side} sideOffset={6}>
          {content}
          <TooltipArrow width={10} height={5} />
        </TooltipContent>
      </RadixTooltip.Portal>
    </RadixTooltip.Root>
  );
}
