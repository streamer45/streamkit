// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import type { GroupImperativeHandle } from 'react-resizable-panels';
import { useShallow } from 'zustand/shallow';

import { Button } from '@/components/ui/Button';
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from '@/components/ui/resizable';
import { useMediaQuery } from '@/hooks/useMediaQuery';
import { useLayoutStore } from '@/stores/layoutStore';

interface ResizableLayoutProps {
  left: ReactNode;
  center: ReactNode;
  right?: ReactNode;
  leftLabel?: string;
  centerLabel?: string;
  rightLabel?: string;
  mobileBreakpointPx?: number;
}

const normalizePercent = (value: unknown, fallback: number) => {
  const parsed =
    typeof value === 'number'
      ? value
      : typeof value === 'string'
        ? Number.parseFloat(value)
        : Number.NaN;
  const normalized = Number.isFinite(parsed) ? parsed : fallback;
  return Math.max(0, Math.min(100, normalized));
};

export function ResizableLayout(props: ResizableLayoutProps) {
  const { mobileBreakpointPx = 900 } = props;
  const isMobile = useMediaQuery(`(max-width: ${mobileBreakpointPx}px)`);
  return isMobile ? <MobileResizableLayout {...props} /> : <DesktopResizableLayout {...props} />;
}

function MobileResizableLayout({
  left,
  center,
  right,
  leftLabel = 'Left',
  centerLabel = 'Center',
  rightLabel = 'Right',
}: ResizableLayoutProps) {
  const hasRightPanel = right != null;
  const [activeMobilePane, setActiveMobilePane] = useState<'left' | 'center' | 'right'>('center');

  useEffect(() => {
    if (activeMobilePane === 'right' && !hasRightPanel) {
      setActiveMobilePane('center');
    }
  }, [activeMobilePane, hasRightPanel]);

  const mobilePanes = useMemo(() => {
    const panes: Array<{
      id: 'left' | 'center' | 'right';
      label: string;
      node: ReactNode;
    }> = [
      { id: 'left', label: leftLabel, node: left },
      { id: 'center', label: centerLabel, node: center },
    ];

    if (right != null) {
      panes.push({ id: 'right', label: rightLabel, node: right });
    }

    return panes;
  }, [center, centerLabel, left, leftLabel, right, rightLabel]);

  const activeMobileNode = mobilePanes.find((pane) => pane.id === activeMobilePane)?.node ?? center;

  return (
    <MobileLayoutContainer>
      <MobilePaneSwitcher role="tablist" aria-label="Panels">
        {mobilePanes.map((pane) => (
          <Button
            key={pane.id}
            size="small"
            variant="secondary"
            active={pane.id === activeMobilePane}
            onClick={() => setActiveMobilePane(pane.id)}
            role="tab"
            aria-selected={pane.id === activeMobilePane}
          >
            {pane.label}
          </Button>
        ))}
      </MobilePaneSwitcher>
      <MobilePaneContainer>{activeMobileNode}</MobilePaneContainer>
    </MobileLayoutContainer>
  );
}

function DesktopResizableLayout({ left, center, right }: ResizableLayoutProps) {
  const groupRef = useRef<GroupImperativeHandle | null>(null);
  const hasRightPanel = right != null;
  const isSyncingLayoutRef = useRef(false);
  const resetSyncFlagTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const {
    leftCollapsed,
    rightCollapsed,
    leftSize,
    rightSize,
    setLeftCollapsed,
    setRightCollapsed,
    setLeftSize,
    setRightSize,
  } = useLayoutStore(
    useShallow((state) => ({
      leftCollapsed: state.leftCollapsed,
      rightCollapsed: state.rightCollapsed,
      leftSize: state.leftSize,
      rightSize: state.rightSize,
      setLeftCollapsed: state.setLeftCollapsed,
      setRightCollapsed: state.setRightCollapsed,
      setLeftSize: state.setLeftSize,
      setRightSize: state.setRightSize,
    }))
  );

  const clampSideSize = (size: number, collapsed: boolean) => {
    if (collapsed) return 0;
    return Math.max(10, Math.min(50, normalizePercent(size, 20)));
  };

  const initialLayoutRef = useRef<{
    hasRightPanel: boolean;
    left: number;
    center: number;
    right: number;
  } | null>(null);

  if (!initialLayoutRef.current || initialLayoutRef.current.hasRightPanel !== hasRightPanel) {
    let initialLeftSize = clampSideSize(leftSize, leftCollapsed);
    let initialRightSize = hasRightPanel ? clampSideSize(rightSize, rightCollapsed) : 0;
    let initialCenterSize = Math.max(0, 100 - initialLeftSize - initialRightSize);

    if (initialCenterSize < 20) {
      const deficit = 20 - initialCenterSize;
      const reduceRight = Math.min(deficit, initialRightSize);
      initialRightSize -= reduceRight;
      const reduceLeft = Math.min(deficit - reduceRight, initialLeftSize);
      initialLeftSize -= reduceLeft;
      initialCenterSize = Math.max(0, 100 - initialLeftSize - initialRightSize);
    }

    initialLayoutRef.current = {
      hasRightPanel,
      left: initialLeftSize,
      center: initialCenterSize,
      right: initialRightSize,
    };
  }

  const initialLayout = initialLayoutRef.current;

  // Memoize callbacks to prevent PanelResizeHandle from re-rendering
  const handleLeftCollapse = useCallback(() => setLeftCollapsed(true), [setLeftCollapsed]);
  const handleLeftExpand = useCallback(() => setLeftCollapsed(false), [setLeftCollapsed]);
  const handleLeftResize = useCallback(
    (size: number) => {
      if (isSyncingLayoutRef.current) return;
      if (size <= 0.5) return;
      setLeftSize(size);
    },
    [setLeftSize]
  );

  const handleRightCollapse = useCallback(() => setRightCollapsed(true), [setRightCollapsed]);
  const handleRightExpand = useCallback(() => setRightCollapsed(false), [setRightCollapsed]);
  const handleRightResize = useCallback(
    (size: number) => {
      if (isSyncingLayoutRef.current) return;
      if (size <= 0.5) return;
      setRightSize(size);
    },
    [setRightSize]
  );

  const schedulePanelSync = useCallback((fn: () => void) => {
    let canceled = false;
    let attempts = 0;

    const run = () => {
      if (canceled) return;
      try {
        fn();
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        const isGroupMissing =
          (message.includes('Group') && message.includes('not found')) ||
          message.includes('Could not find Group');
        if (isGroupMissing && attempts < 5) {
          attempts += 1;
          setTimeout(run, 0);
          return;
        }
        throw error;
      }
    };

    setTimeout(run, 0);
    return () => {
      canceled = true;
    };
  }, []);

  // Apply layout changes from store
  useEffect(() => {
    const cancelSync = schedulePanelSync(() => {
      if (!groupRef.current) return;

      const nextLeft = leftCollapsed ? 0 : normalizePercent(leftSize, 20);
      const nextRight = hasRightPanel ? (rightCollapsed ? 0 : normalizePercent(rightSize, 20)) : 0;
      const nextCenter = Math.max(0, 100 - nextLeft - nextRight);

      const nextLayout: Record<string, number> = {};
      nextLayout['left-panel'] = nextLeft;
      nextLayout['center-panel'] = nextCenter;
      if (hasRightPanel) {
        nextLayout['right-panel'] = nextRight;
      }

      const currentLayout = groupRef.current.getLayout();
      const isSameLayout = Object.entries(nextLayout).every(([panelId, nextSize]) => {
        const currentSize = currentLayout[panelId];
        if (typeof currentSize !== 'number') return false;
        return Math.abs(currentSize - nextSize) < 0.05;
      });

      if (isSameLayout) return;

      isSyncingLayoutRef.current = true;
      groupRef.current.setLayout(nextLayout);
      if (resetSyncFlagTimeoutRef.current) {
        clearTimeout(resetSyncFlagTimeoutRef.current);
      }
      resetSyncFlagTimeoutRef.current = setTimeout(() => {
        isSyncingLayoutRef.current = false;
        resetSyncFlagTimeoutRef.current = null;
      }, 0);
    });

    return () => {
      cancelSync();
      if (resetSyncFlagTimeoutRef.current) {
        clearTimeout(resetSyncFlagTimeoutRef.current);
        resetSyncFlagTimeoutRef.current = null;
      }
      isSyncingLayoutRef.current = false;
    };
  }, [leftCollapsed, leftSize, rightCollapsed, rightSize, hasRightPanel, schedulePanelSync]);

  return (
    <ResizablePanelGroup
      id="main-resizable-layout"
      ref={groupRef}
      direction="horizontal"
      style={{ width: '100%', height: '100%' }}
    >
      {/* Left panel */}
      <ResizablePanel
        minSize={10}
        maxSize={50}
        collapsible={true}
        collapsedSize={0}
        defaultSize={initialLayout.left}
        onCollapse={handleLeftCollapse}
        onExpand={handleLeftExpand}
        onResize={handleLeftResize}
        id="left-panel"
        style={{
          boxShadow: leftCollapsed ? 'none' : '2px 0 12px rgba(0, 0, 0, 0.15)',
          transition: 'box-shadow 0.15s ease',
          overflow: 'hidden',
          position: 'relative',
        }}
      >
        {leftCollapsed ? null : left}
      </ResizablePanel>

      {/* Left handle */}
      <ResizableHandle isCollapsed={leftCollapsed} onExpand={handleLeftExpand} side="left" />

      {/* Center panel */}
      <ResizablePanel
        minSize={20}
        defaultSize={initialLayout.center}
        id="center-panel"
        style={{
          overflow: 'hidden',
          position: 'relative',
        }}
      >
        {center}
      </ResizablePanel>

      {/* Right handle and panel (if right content provided) */}
      {right && (
        <>
          <ResizableHandle isCollapsed={rightCollapsed} onExpand={handleRightExpand} side="right" />
          <ResizablePanel
            minSize={10}
            maxSize={50}
            collapsible={true}
            collapsedSize={0}
            defaultSize={initialLayout.right}
            onCollapse={handleRightCollapse}
            onExpand={handleRightExpand}
            onResize={handleRightResize}
            id="right-panel"
            style={{
              boxShadow: rightCollapsed ? 'none' : '-2px 0 12px rgba(0, 0, 0, 0.15)',
              transition: 'box-shadow 0.15s ease',
              overflow: 'hidden',
              position: 'relative',
            }}
          >
            {rightCollapsed ? null : right}
          </ResizablePanel>
        </>
      )}
    </ResizablePanelGroup>
  );
}

const MobileLayoutContainer = styled.div`
  display: flex;
  flex-direction: column;
  width: 100%;
  height: 100%;
  min-height: 0;
`;

const MobilePaneSwitcher = styled.div`
  display: flex;
  gap: 8px;
  padding: 8px;
  align-items: center;
  border-bottom: 1px solid var(--sk-border);
  background-color: var(--sk-sidebar-bg);
  overflow-x: auto;
  -webkit-overflow-scrolling: touch;
`;

const MobilePaneContainer = styled.div`
  flex: 1;
  min-height: 0;
  overflow: hidden;
`;
