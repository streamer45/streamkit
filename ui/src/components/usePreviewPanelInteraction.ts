// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Custom hook encapsulating drag and resize interaction for the
 * floating output preview panel.
 *
 * Extracted from OutputPreviewPanel to reduce the main component's
 * cyclomatic complexity below the ESLint threshold.
 *
 * The panel supports free resizing from any edge — width and height are
 * tracked independently.  The video canvas inside the panel uses
 * `object-fit: contain` for natural letterboxing/pillarboxing, so the
 * stream's aspect ratio is always preserved regardless of panel shape.
 */

import React, { useCallback, useEffect, useRef, useState } from 'react';

import { componentsLogger } from '@/utils/logger';

const DEFAULT_WIDTH = 320;
const MIN_WIDTH = 180;
const MAX_WIDTH = 800;
const DEFAULT_ASPECT_RATIO = 16 / 9;

/** Chrome height: header (28px) + header border (1px) + body padding (12px). */
const CHROME_HEIGHT = 41;
const MIN_HEIGHT = 100;
const MAX_HEIGHT = 800;

/** All resize edges and corners supported by the preview panel. */
export type ResizeEdge =
  | 'left'
  | 'top'
  | 'right'
  | 'bottom'
  | 'top-left'
  | 'top-right'
  | 'bottom-left'
  | 'bottom-right';

interface PanelInteraction {
  pos: { right: number; bottom: number };
  panelWidth: number;
  panelHeight: number;
  panelRef: React.RefObject<HTMLDivElement | null>;
  collapsed: boolean;
  isFullscreen: boolean;
  toggleCollapsed: () => void;
  toggleFullscreen: () => void;
  panelStyle: React.CSSProperties;
  handleResizeStart: (edge: ResizeEdge, e: React.PointerEvent) => void;
  handleDragStart: (e: React.PointerEvent) => void;
}

/** Compute the default total panel height from a width and aspect ratio. */
function defaultPanelHeight(width: number, ar: number): number {
  return Math.round(width / ar) + CHROME_HEIGHT;
}

/** @param aspectRatio  CSS aspect-ratio string from the video stream (e.g. "640 / 480").
 *  Only used to compute the initial panel height.  Once the user resizes,
 *  their chosen dimensions are preserved and letterboxing handles any AR
 *  mismatch. */
export function usePreviewPanelInteraction(aspectRatio?: string): PanelInteraction {
  const [pos, setPos] = useState({ right: 16, bottom: 16 });
  const [panelWidth, setPanelWidth] = useState(DEFAULT_WIDTH);
  const [panelHeight, setPanelHeight] = useState(() =>
    defaultPanelHeight(DEFAULT_WIDTH, DEFAULT_ASPECT_RATIO)
  );
  const [collapsed, setCollapsed] = useState(false);
  const [isFullscreen, setIsFullscreen] = useState(false);
  const panelRef = useRef<HTMLDivElement>(null);

  // Track whether the user has explicitly resized vertically.  Before the
  // first manual resize the height auto-adjusts to match the stream AR so
  // the initial layout looks correct.
  const userResizedVertically = useRef(false);

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

  // Auto-adjust panel height when the stream aspect ratio is first detected,
  // but only if the user hasn't manually resized yet.
  useEffect(() => {
    if (!userResizedVertically.current) {
      setPanelHeight(defaultPanelHeight(panelWidth, numericRatio));
    }
  }, [numericRatio, panelWidth]);

  const resizeRef = useRef<{
    startX: number;
    startY: number;
    origWidth: number;
    origHeight: number;
    origRight: number;
    origBottom: number;
    edge: ResizeEdge;
  } | null>(null);

  // Track active document listeners so the cleanup effect can remove them on unmount.
  const activeListenersRef = useRef<{
    move: (e: PointerEvent) => void;
    up: (e: PointerEvent) => void;
  } | null>(null);

  const handleResizeStart = useCallback(
    (edge: ResizeEdge, e: React.PointerEvent) => {
      e.preventDefault();
      e.stopPropagation();
      resizeRef.current = {
        startX: e.clientX,
        startY: e.clientY,
        origWidth: panelWidth,
        origHeight: panelHeight,
        origRight: pos.right,
        origBottom: pos.bottom,
        edge,
      };

      const handleResizeMove = (ev: PointerEvent) => {
        if (!resizeRef.current) return;
        const {
          edge: curEdge,
          startX,
          startY,
          origWidth,
          origHeight,
          origRight,
          origBottom,
        } = resizeRef.current;

        const dx = ev.clientX - startX;
        const dy = ev.clientY - startY;

        // Determine which axes this edge/corner affects.
        const touchesLeft =
          curEdge === 'left' || curEdge === 'top-left' || curEdge === 'bottom-left';
        const touchesRight =
          curEdge === 'right' || curEdge === 'top-right' || curEdge === 'bottom-right';
        const touchesTop = curEdge === 'top' || curEdge === 'top-left' || curEdge === 'top-right';
        const touchesBottom =
          curEdge === 'bottom' || curEdge === 'bottom-left' || curEdge === 'bottom-right';

        if (touchesLeft) {
          // Panel extends left, right stays fixed.
          const newW = Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, origWidth - dx));
          setPanelWidth(newW);
        } else if (touchesRight) {
          // Right edge moves right, left edge stays fixed.
          const newW = Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, origWidth + dx));
          setPanelWidth(newW);
          setPos((prev) => ({
            ...prev,
            right: Math.max(0, origRight - (newW - origWidth)),
          }));
        }

        if (touchesTop) {
          // Panel extends up, bottom stays fixed.
          const newH = Math.max(MIN_HEIGHT, Math.min(MAX_HEIGHT, origHeight - dy));
          setPanelHeight(newH);
          userResizedVertically.current = true;
        } else if (touchesBottom) {
          // Bottom edge moves down, top edge stays fixed.
          const newH = Math.max(MIN_HEIGHT, Math.min(MAX_HEIGHT, origHeight + dy));
          setPanelHeight(newH);
          setPos((prev) => ({
            ...prev,
            bottom: Math.max(0, origBottom - (newH - origHeight)),
          }));
          userResizedVertically.current = true;
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
    [panelWidth, panelHeight, pos]
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
        const newRight = dragRef.current.origX - (ev.clientX - dragRef.current.startX);
        const newBottom = dragRef.current.origY - (ev.clientY - dragRef.current.startY);
        // Clamp so the panel stays within the viewport.
        const maxRight = window.innerWidth - panelWidth;
        const maxBottom = window.innerHeight - 40; // leave at least 40px visible
        setPos({
          right: Math.max(0, Math.min(maxRight, newRight)),
          bottom: Math.max(0, Math.min(maxBottom, newBottom)),
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
    [pos, panelWidth]
  );

  const toggleCollapsed = useCallback(() => setCollapsed((prev) => !prev), []);

  const toggleFullscreen = useCallback(() => {
    if (!panelRef.current) return;
    if (!document.fullscreenElement) {
      panelRef.current
        .requestFullscreen()
        .then(() => setIsFullscreen(true))
        .catch((err) => {
          componentsLogger.warn('Fullscreen request denied:', err);
        });
    } else {
      document
        .exitFullscreen()
        .then(() => setIsFullscreen(false))
        .catch((err) => {
          componentsLogger.warn('Exit fullscreen failed:', err);
        });
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
    : {
        right: pos.right,
        bottom: pos.bottom,
        width: collapsed ? undefined : panelWidth,
        height: collapsed ? undefined : panelHeight,
      };

  return {
    pos,
    panelWidth,
    panelHeight,
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
