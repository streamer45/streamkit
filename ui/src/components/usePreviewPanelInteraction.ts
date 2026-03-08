// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Custom hook encapsulating drag and resize interaction for the
 * floating output preview panel.
 *
 * Extracted from OutputPreviewPanel to reduce the main component's
 * cyclomatic complexity below the ESLint threshold.
 */

import React, { useCallback, useEffect, useRef, useState } from 'react';

const DEFAULT_WIDTH = 320;
const MIN_WIDTH = 180;
const MAX_WIDTH = 800;

interface PanelInteraction {
  pos: { x: number; y: number };
  panelWidth: number;
  panelRef: React.RefObject<HTMLDivElement | null>;
  collapsed: boolean;
  isFullscreen: boolean;
  toggleCollapsed: () => void;
  toggleFullscreen: () => void;
  panelStyle: React.CSSProperties;
  handleResizeStart: (edge: 'left' | 'top' | 'right' | 'bottom', e: React.PointerEvent) => void;
  handleDragStart: (e: React.PointerEvent) => void;
}

export function usePreviewPanelInteraction(): PanelInteraction {
  const [pos, setPos] = useState({ x: 16, y: 16 });
  const [panelWidth, setPanelWidth] = useState(DEFAULT_WIDTH);
  const [collapsed, setCollapsed] = useState(false);
  const [isFullscreen, setIsFullscreen] = useState(false);
  const panelRef = useRef<HTMLDivElement>(null);

  const dragRef = useRef<{
    startX: number;
    startY: number;
    origX: number;
    origY: number;
  } | null>(null);

  const resizeRef = useRef<{
    startX: number;
    startY: number;
    origWidth: number;
    origY: number;
    edge: 'left' | 'top' | 'right' | 'bottom';
  } | null>(null);

  const handleResizeStart = useCallback(
    (edge: 'left' | 'top' | 'right' | 'bottom', e: React.PointerEvent) => {
      e.preventDefault();
      e.stopPropagation();
      resizeRef.current = {
        startX: e.clientX,
        startY: e.clientY,
        origWidth: panelWidth,
        origY: pos.y,
        edge,
      };

      const handleResizeMove = (ev: PointerEvent) => {
        if (!resizeRef.current) return;
        const { edge: curEdge, startX, startY, origWidth, origY } = resizeRef.current;
        const isHorizontal = curEdge === 'left' || curEdge === 'right';
        const sign = curEdge === 'left' || curEdge === 'top' ? -1 : 1;
        const rawDelta = isHorizontal
          ? sign * (ev.clientX - startX)
          : sign * (ev.clientY - startY) * 1.78;

        const newWidth = Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, origWidth + rawDelta));
        setPanelWidth(newWidth);

        if (curEdge === 'bottom') {
          const widthDelta = newWidth - origWidth;
          setPos((prev) => ({ ...prev, y: Math.max(0, origY - widthDelta / 1.78) }));
        }
      };

      const handleResizeUp = () => {
        resizeRef.current = null;
        document.removeEventListener('pointermove', handleResizeMove);
        document.removeEventListener('pointerup', handleResizeUp);
      };

      document.addEventListener('pointermove', handleResizeMove);
      document.addEventListener('pointerup', handleResizeUp);
    },
    [panelWidth, pos]
  );

  const handleDragStart = useCallback(
    (e: React.PointerEvent) => {
      if ((e.target as HTMLElement).closest('button')) return;
      e.preventDefault();
      e.stopPropagation();
      dragRef.current = { startX: e.clientX, startY: e.clientY, origX: pos.x, origY: pos.y };

      const handleMove = (ev: PointerEvent) => {
        if (!dragRef.current) return;
        setPos({
          x: Math.max(0, dragRef.current.origX - (ev.clientX - dragRef.current.startX)),
          y: Math.max(0, dragRef.current.origY - (ev.clientY - dragRef.current.startY)),
        });
      };

      const handleUp = () => {
        dragRef.current = null;
        document.removeEventListener('pointermove', handleMove);
        document.removeEventListener('pointerup', handleUp);
      };

      document.addEventListener('pointermove', handleMove);
      document.addEventListener('pointerup', handleUp);
    },
    [pos]
  );

  const toggleCollapsed = useCallback(() => setCollapsed((prev) => !prev), []);

  const toggleFullscreen = useCallback(() => {
    if (!panelRef.current) return;
    if (!document.fullscreenElement) {
      panelRef.current
        .requestFullscreen()
        .then(() => setIsFullscreen(true))
        .catch(() => {});
    } else {
      document
        .exitFullscreen()
        .then(() => setIsFullscreen(false))
        .catch(() => {});
    }
  }, []);

  useEffect(() => {
    const handler = () => {
      if (!document.fullscreenElement) setIsFullscreen(false);
    };
    document.addEventListener('fullscreenchange', handler);
    return () => document.removeEventListener('fullscreenchange', handler);
  }, []);

  const panelStyle: React.CSSProperties = isFullscreen
    ? { right: 0, bottom: 0, width: '100%', height: '100%', borderRadius: 0 }
    : { right: pos.x, bottom: pos.y, width: collapsed ? undefined : panelWidth };

  return {
    pos,
    panelWidth,
    panelRef,
    collapsed,
    isFullscreen,
    toggleCollapsed,
    toggleFullscreen,
    panelStyle,
    handleResizeStart,
    handleDragStart,
  };
}
