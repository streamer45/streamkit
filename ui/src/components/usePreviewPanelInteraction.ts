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
const DEFAULT_ASPECT_RATIO = 16 / 9;

interface PanelInteraction {
  pos: { right: number; bottom: number };
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

/** @param aspectRatio  CSS aspect-ratio string from the video stream (e.g. "640 / 480").
 *  Used to scale vertical resizes proportionally.  Falls back to 16:9 when unknown. */
export function usePreviewPanelInteraction(aspectRatio?: string): PanelInteraction {
  const [pos, setPos] = useState({ right: 16, bottom: 16 });
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

  // Parse the CSS aspect-ratio string ("W / H") into a numeric ratio.
  // Falls back to 16:9 when the stream dimensions are not yet known.
  const numericRatio = React.useMemo(() => {
    if (!aspectRatio) return DEFAULT_ASPECT_RATIO;
    const parts = aspectRatio.split('/').map((s) => Number(s.trim()));
    if (parts.length === 2 && parts[0] > 0 && parts[1] > 0) return parts[0] / parts[1];
    return DEFAULT_ASPECT_RATIO;
  }, [aspectRatio]);

  const resizeRef = useRef<{
    startX: number;
    startY: number;
    origWidth: number;
    origY: number;
    edge: 'left' | 'top' | 'right' | 'bottom';
  } | null>(null);

  // Track active document listeners so the cleanup effect can remove them on unmount.
  const activeListenersRef = useRef<{
    move: (e: PointerEvent) => void;
    up: (e: PointerEvent) => void;
  } | null>(null);

  const handleResizeStart = useCallback(
    (edge: 'left' | 'top' | 'right' | 'bottom', e: React.PointerEvent) => {
      e.preventDefault();
      e.stopPropagation();
      resizeRef.current = {
        startX: e.clientX,
        startY: e.clientY,
        origWidth: panelWidth,
        origY: pos.bottom,
        edge,
      };

      const handleResizeMove = (ev: PointerEvent) => {
        if (!resizeRef.current) return;
        const { edge: curEdge, startX, startY, origWidth, origY } = resizeRef.current;
        const isHorizontal = curEdge === 'left' || curEdge === 'right';
        const sign = curEdge === 'left' || curEdge === 'top' ? -1 : 1;
        const rawDelta = isHorizontal
          ? sign * (ev.clientX - startX)
          : sign * (ev.clientY - startY) * numericRatio;

        const newWidth = Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, origWidth + rawDelta));
        setPanelWidth(newWidth);

        if (curEdge === 'bottom') {
          const widthDelta = newWidth - origWidth;
          setPos((prev) => ({ ...prev, bottom: Math.max(0, origY - widthDelta / numericRatio) }));
        }
      };

      const handleResizeUp = () => {
        resizeRef.current = null;
        document.removeEventListener('pointermove', handleResizeMove);
        document.removeEventListener('pointerup', handleResizeUp);
        activeListenersRef.current = null;
      };

      document.addEventListener('pointermove', handleResizeMove);
      document.addEventListener('pointerup', handleResizeUp);
      activeListenersRef.current = { move: handleResizeMove, up: handleResizeUp };
    },
    [panelWidth, pos, numericRatio]
  );

  const handleDragStart = useCallback(
    (e: React.PointerEvent) => {
      if ((e.target as HTMLElement).closest('button')) return;
      e.preventDefault();
      e.stopPropagation();
      dragRef.current = {
        startX: e.clientX,
        startY: e.clientY,
        origX: pos.right,
        origY: pos.bottom,
      };

      const handleMove = (ev: PointerEvent) => {
        if (!dragRef.current) return;
        setPos({
          right: Math.max(0, dragRef.current.origX - (ev.clientX - dragRef.current.startX)),
          bottom: Math.max(0, dragRef.current.origY - (ev.clientY - dragRef.current.startY)),
        });
      };

      const handleUp = () => {
        dragRef.current = null;
        document.removeEventListener('pointermove', handleMove);
        document.removeEventListener('pointerup', handleUp);
        activeListenersRef.current = null;
      };

      document.addEventListener('pointermove', handleMove);
      document.addEventListener('pointerup', handleUp);
      activeListenersRef.current = { move: handleMove, up: handleUp };
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

  // Clean up document listeners if the component unmounts mid-drag/resize.
  useEffect(() => {
    return () => {
      if (activeListenersRef.current) {
        document.removeEventListener('pointermove', activeListenersRef.current.move);
        document.removeEventListener('pointerup', activeListenersRef.current.up);
      }
    };
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
    : { right: pos.right, bottom: pos.bottom, width: collapsed ? undefined : panelWidth };

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
