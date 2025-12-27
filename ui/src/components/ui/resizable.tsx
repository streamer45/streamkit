// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import { forwardRef, useCallback, useRef } from 'react';
import * as ResizablePrimitive from 'react-resizable-panels';

const StyledPanelGroup = styled(ResizablePrimitive.Group)`
  display: flex;
  height: 100%;
  width: 100%;
`;

type ResizablePanelGroupProps = Omit<
  React.ComponentProps<typeof ResizablePrimitive.Group>,
  'groupRef'
> & {
  direction?: 'horizontal' | 'vertical';
};

const ResizablePanelGroup = forwardRef<
  ResizablePrimitive.GroupImperativeHandle,
  ResizablePanelGroupProps
>(({ className, direction, orientation, ...props }, ref) => (
  <StyledPanelGroup
    className={className}
    groupRef={ref}
    orientation={orientation ?? direction}
    {...props}
  />
));

type ResizablePanelProps = Omit<
  React.ComponentProps<typeof ResizablePrimitive.Panel>,
  'onResize'
> & {
  onCollapse?: () => void;
  onExpand?: () => void;
  onResize?: (size: number) => void;
};

const toPercentString = (value: number | string | undefined) =>
  typeof value === 'number' ? `${value}%` : value;

const ResizablePanel = forwardRef<ResizablePrimitive.PanelImperativeHandle, ResizablePanelProps>(
  (
    {
      collapsible,
      collapsedSize,
      defaultSize,
      maxSize,
      minSize,
      onCollapse,
      onExpand,
      onResize,
      ...props
    },
    ref
  ) => {
    const wasCollapsedRef = useRef(false);

    const handleResize = useCallback(
      (panelSize: ResizablePrimitive.PanelSize) => {
        const nextCollapsed = panelSize.asPercentage <= 0.1;

        if (collapsible) {
          if (nextCollapsed && !wasCollapsedRef.current) {
            onCollapse?.();
          } else if (!nextCollapsed && wasCollapsedRef.current) {
            onExpand?.();
          }
        }

        wasCollapsedRef.current = nextCollapsed;
        onResize?.(panelSize.asPercentage);
      },
      [collapsible, onCollapse, onExpand, onResize]
    );

    const shouldHandleResize =
      onResize !== undefined || onCollapse !== undefined || onExpand !== undefined;

    return (
      <ResizablePrimitive.Panel
        {...props}
        collapsedSize={toPercentString(collapsedSize)}
        collapsible={collapsible}
        defaultSize={toPercentString(defaultSize)}
        maxSize={toPercentString(maxSize)}
        minSize={toPercentString(minSize)}
        onResize={shouldHandleResize ? handleResize : undefined}
        panelRef={ref}
      />
    );
  }
);

interface ResizableHandleProps extends React.ComponentProps<typeof ResizablePrimitive.Separator> {
  isCollapsed?: boolean;
  onExpand?: () => void;
  side?: 'left' | 'right';
}

const StyledResizeHandle = styled(ResizablePrimitive.Separator, {
  shouldForwardProp: (prop) => !prop.startsWith('$'),
})<{
  $isCollapsed: boolean;
}>`
  position: relative;
  width: ${(props) => (props.$isCollapsed ? '20px' : '12px')};
  height: 100%;
  background-color: transparent;
  cursor: ${(props) => (props.$isCollapsed ? 'pointer' : 'col-resize')};
  touch-action: none;
  transition: background-color 0.15s ease;

  &::after {
    content: '';
    position: absolute;
    left: 50%;
    top: 0;
    bottom: 0;
    width: ${(props) => (props.$isCollapsed ? '6px' : '4px')};
    transform: translateX(-50%);
    border-radius: 999px;
    background-color: var(--sk-border);
    transition: background-color 0.15s ease;
  }

  &[data-separator='hover']::after,
  &[data-separator='active']::after,
  &:hover::after {
    background-color: var(--sk-primary);
  }
`;

const ResizableHandle = ({
  className,
  isCollapsed = false,
  onExpand,
  ...props
}: ResizableHandleProps) => {
  const handleClick = () => {
    if (isCollapsed && onExpand) {
      onExpand();
    }
  };

  return (
    <StyledResizeHandle
      className={className}
      $isCollapsed={isCollapsed}
      onClick={handleClick}
      {...props}
    />
  );
};

export { ResizablePanelGroup, ResizablePanel, ResizableHandle };
