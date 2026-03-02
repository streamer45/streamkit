// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Visual canvas for the video compositor node.
 *
 * Renders a scaled representation of the compositor output with draggable,
 * resizable layer boxes. Each layer type (video input, text overlay, image
 * overlay) is rendered differently:
 *
 * - Video input layers: colored rectangles with a unique hue per layer
 * - Text overlays: render the actual text content at scaled font size
 * - Image overlays: rectangles with an image icon and crosshatch pattern
 *
 * All interaction is handled via pointer events; visual updates during drag
 * use direct DOM manipulation (refs) so React never re-renders mid-interaction.
 */

import styled from '@emotion/styled';
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import type {
  LayerState,
  TextOverlayState,
  ImageOverlayState,
  ResizeHandle,
} from '@/hooks/useCompositorLayers';

// ── Hue generation ──────────────────────────────────────────────────────────

/** Golden-angle-based hue to maximise visual separation between layers */
function layerHue(index: number): number {
  return (index * 137.508) % 360;
}

// ── Styled components ───────────────────────────────────────────────────────

const CanvasOuter = styled.div`
  width: 100%;
  position: relative;
  overflow: hidden;
  border: 1px solid var(--sk-border);
  border-radius: 4px;
  background: var(--sk-sidebar-bg);
`;

const CanvasInner = styled.div`
  position: relative;
  transform-origin: top left;
  background: #1a1a2e;
  overflow: hidden;
`;

const LayerBox = styled.div`
  position: absolute;
  box-sizing: border-box;
  cursor: move;
  user-select: none;
  will-change: transform, left, top, width, height;
  touch-action: none;
  transform-origin: center center;
`;

const LayerLabel = styled.div`
  position: absolute;
  top: 2px;
  left: 4px;
  font-size: 10px;
  font-weight: 600;
  color: rgba(255, 255, 255, 0.95);
  text-shadow: 0 1px 3px rgba(0, 0, 0, 0.9);
  pointer-events: none;
  white-space: nowrap;
  z-index: 2;
`;

/** Text content rendered inside text overlay layers */
const TextContent = styled.div`
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  pointer-events: none;
  overflow: hidden;
  z-index: 1;
`;

/** Inline text editing textarea shown on double-click (supports multiline) */
const InlineTextInput = styled.textarea`
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  background: rgba(0, 0, 0, 0.6);
  border: 2px solid var(--sk-primary);
  border-radius: 2px;
  color: #fff;
  font-weight: 600;
  text-align: center;
  outline: none;
  z-index: 3;
  box-sizing: border-box;
  resize: none;
  overflow: hidden;
  white-space: pre-wrap;
  word-break: break-word;
  line-height: 1.2;
  padding: 4px;
  font-family: inherit;
`;

const LayerDimensions = styled.div`
  position: absolute;
  bottom: 2px;
  right: 4px;
  font-size: 9px;
  color: rgba(255, 255, 255, 0.6);
  text-shadow: 0 1px 2px rgba(0, 0, 0, 0.8);
  pointer-events: none;
  font-variant-numeric: tabular-nums;
  z-index: 2;
`;

const HANDLE_SIZE = 8;

const ResizeHandleDiv = styled.div<{ position: ResizeHandle }>`
  position: absolute;
  width: ${HANDLE_SIZE}px;
  height: ${HANDLE_SIZE}px;
  background: var(--sk-primary);
  border: 1px solid rgba(255, 255, 255, 0.8);
  border-radius: 2px;
  z-index: 10;
  touch-action: none;

  ${(props) => {
    const half = -HANDLE_SIZE / 2;
    switch (props.position) {
      case 'nw':
        return `top: ${half}px; left: ${half}px; cursor: nw-resize;`;
      case 'n':
        return `top: ${half}px; left: 50%; margin-left: ${half}px; cursor: n-resize;`;
      case 'ne':
        return `top: ${half}px; right: ${half}px; cursor: ne-resize;`;
      case 'e':
        return `top: 50%; margin-top: ${half}px; right: ${half}px; cursor: e-resize;`;
      case 'se':
        return `bottom: ${half}px; right: ${half}px; cursor: se-resize;`;
      case 's':
        return `bottom: ${half}px; left: 50%; margin-left: ${half}px; cursor: s-resize;`;
      case 'sw':
        return `bottom: ${half}px; left: ${half}px; cursor: sw-resize;`;
      case 'w':
        return `top: 50%; margin-top: ${half}px; left: ${half}px; cursor: w-resize;`;
    }
  }}
`;

const EmptyState = styled.div`
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  min-height: 80px;
  color: var(--sk-text-muted);
  font-size: 11px;
`;

// ── Resize handles ──────────────────────────────────────────────────────────

const HANDLES: ResizeHandle[] = ['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w'];

const ResizeHandles: React.FC<{
  layerId: string;
  onResizeStart: (layerId: string, handle: ResizeHandle, e: React.PointerEvent) => void;
}> = React.memo(({ layerId, onResizeStart }) => (
  <>
    {HANDLES.map((h) => (
      <ResizeHandleDiv
        key={h}
        position={h}
        className="nodrag nopan"
        onPointerDown={(e) => onResizeStart(layerId, h, e)}
      />
    ))}
  </>
));
ResizeHandles.displayName = 'ResizeHandles';

// ── Video input layer ───────────────────────────────────────────────────────

const VideoLayer: React.FC<{
  layer: LayerState;
  index: number;
  isSelected: boolean;
  onPointerDown: (layerId: string, e: React.PointerEvent) => void;
  onResizeStart: (layerId: string, handle: ResizeHandle, e: React.PointerEvent) => void;
  layerRef: (el: HTMLDivElement | null) => void;
}> = React.memo(({ layer, index, isSelected, onPointerDown, onResizeStart, layerRef }) => {
  const handlePointerDown = useCallback(
    (e: React.PointerEvent) => {
      onPointerDown(layer.id, e);
    },
    [layer.id, onPointerDown]
  );

  const hue = layerHue(index);
  const borderColor = isSelected ? 'var(--sk-primary)' : `hsla(${hue}, 70%, 65%, 0.8)`;
  const bgColor = isSelected ? `hsla(${hue}, 60%, 50%, 0.25)` : `hsla(${hue}, 60%, 50%, 0.15)`;

  return (
    <LayerBox
      ref={layerRef}
      className="nodrag nopan"
      style={{
        left: layer.x,
        top: layer.y,
        width: layer.width,
        height: layer.height,
        opacity: layer.visible ? layer.opacity : 0.2,
        transform: layer.rotationDegrees !== 0 ? `rotate(${layer.rotationDegrees}deg)` : undefined,
        zIndex: layer.zIndex + 1,
        border: `2px ${layer.visible ? 'solid' : 'dashed'} ${borderColor}`,
        background: bgColor,
        filter: layer.visible ? undefined : 'grayscale(0.6)',
      }}
      onPointerDown={handlePointerDown}
    >
      <LayerLabel>{layer.id}</LayerLabel>
      <LayerDimensions>
        {Math.round(layer.width)}&times;{Math.round(layer.height)}
      </LayerDimensions>
      {isSelected && <ResizeHandles layerId={layer.id} onResizeStart={onResizeStart} />}
    </LayerBox>
  );
});
VideoLayer.displayName = 'VideoLayer';

// ── Text overlay layer ──────────────────────────────────────────────────────

const TextOverlayLayer: React.FC<{
  overlay: TextOverlayState;
  index: number;
  isSelected: boolean;
  scale: number;
  onPointerDown: (layerId: string, e: React.PointerEvent) => void;
  onTextEdit?: (id: string, updates: Partial<Omit<TextOverlayState, 'id'>>) => void;
  layerRef: (el: HTMLDivElement | null) => void;
}> = React.memo(({ overlay, index, isSelected, scale, onPointerDown, onTextEdit, layerRef }) => {
  const [editing, setEditing] = useState(false);
  const [editText, setEditText] = useState(overlay.text);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const cancelledRef = useRef(false);
  const committedRef = useRef(false);

  // Issue #1 fix: when the layer is deselected while editing, commit the edit.
  const prevSelectedRef = useRef(isSelected);
  useEffect(() => {
    if (prevSelectedRef.current && !isSelected && editing) {
      // Layer was deselected while editing – commit
      if (!cancelledRef.current && !committedRef.current) {
        committedRef.current = true;
        if (editText.trim() && editText !== overlay.text && onTextEdit) {
          onTextEdit(overlay.id, { text: editText.trim() });
        }
      }
      setEditing(false);
    }
    prevSelectedRef.current = isSelected;
  }, [isSelected, editing, editText, overlay.id, overlay.text, onTextEdit]);

  const handlePointerDown = useCallback(
    (e: React.PointerEvent) => {
      if (editing) return; // don't start drag while editing
      onPointerDown(overlay.id, e);
    },
    [overlay.id, onPointerDown, editing]
  );

  const handleDoubleClick = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      e.preventDefault();
      if (!onTextEdit) return;
      setEditText(overlay.text);
      cancelledRef.current = false;
      committedRef.current = false;
      setEditing(true);
      // Focus the textarea after React renders it
      requestAnimationFrame(() => inputRef.current?.focus());
    },
    [onTextEdit, overlay.text]
  );

  const commitEdit = useCallback(() => {
    if (cancelledRef.current) return;
    if (committedRef.current) return; // guard against double-fire
    committedRef.current = true;
    setEditing(false);
    if (editText.trim() && editText !== overlay.text && onTextEdit) {
      onTextEdit(overlay.id, { text: editText.trim() });
    }
  }, [editText, overlay.id, overlay.text, onTextEdit]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      e.stopPropagation();
      // Ctrl/Cmd+Enter commits; plain Enter inserts newline (textarea default)
      if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        commitEdit();
      }
      if (e.key === 'Escape') {
        cancelledRef.current = true;
        setEditing(false);
      }
    },
    [commitEdit]
  );

  const hue = layerHue(index + 100); // offset from video layers
  const borderColor = isSelected ? 'var(--sk-primary)' : `hsla(${hue}, 70%, 65%, 0.8)`;
  const bgColor = isSelected ? `hsla(${hue}, 60%, 50%, 0.25)` : `hsla(${hue}, 60%, 50%, 0.12)`;

  const [r, g, b, a] = overlay.color;
  const textColor = `rgba(${r}, ${g}, ${b}, ${(a ?? 255) / 255})`;

  return (
    <LayerBox
      ref={layerRef}
      className="nodrag nopan"
      style={{
        left: overlay.x,
        top: overlay.y,
        width: overlay.width,
        height: overlay.height,
        opacity: overlay.visible ? overlay.opacity : 0.2,
        zIndex: overlay.zIndex ?? 100 + index,
        border: `2px dashed ${borderColor}`,
        background: bgColor,
        filter: overlay.visible ? undefined : 'grayscale(0.6)',
        transform:
          overlay.rotationDegrees !== 0 ? `rotate(${overlay.rotationDegrees}deg)` : undefined,
      }}
      onPointerDown={handlePointerDown}
      onDoubleClick={handleDoubleClick}
    >
      <LayerLabel>text_{index}</LayerLabel>
      {editing ? (
        <InlineTextInput
          ref={inputRef}
          className="nodrag nopan"
          value={editText}
          onChange={(e) => setEditText(e.target.value)}
          onBlur={commitEdit}
          onKeyDown={handleKeyDown}
          style={{ fontSize: Math.max(10, overlay.fontSize * scale * 0.6) }}
        />
      ) : (
        <TextContent>
          <span
            style={{
              fontSize: Math.max(8, overlay.fontSize * scale),
              color: textColor,
              fontWeight: 600,
              textShadow: '0 1px 3px rgba(0,0,0,0.7)',
              lineHeight: 1.2,
              textAlign: 'center',
              wordBreak: 'break-word',
              whiteSpace: 'pre-wrap',
              maxWidth: '100%',
              padding: '2px 4px',
              boxSizing: 'border-box',
            }}
          >
            {overlay.text}
          </span>
        </TextContent>
      )}
    </LayerBox>
  );
});
TextOverlayLayer.displayName = 'TextOverlayLayer';

// ── Image overlay layer ─────────────────────────────────────────────────────

const ImageOverlayLayer: React.FC<{
  overlay: ImageOverlayState;
  index: number;
  isSelected: boolean;
  onPointerDown: (layerId: string, e: React.PointerEvent) => void;
  onResizeStart: (layerId: string, handle: ResizeHandle, e: React.PointerEvent) => void;
  layerRef: (el: HTMLDivElement | null) => void;
}> = React.memo(({ overlay, index, isSelected, onPointerDown, onResizeStart, layerRef }) => {
  const handlePointerDown = useCallback(
    (e: React.PointerEvent) => {
      onPointerDown(overlay.id, e);
    },
    [overlay.id, onPointerDown]
  );

  const hue = layerHue(index + 200); // offset from text overlays
  const borderColor = isSelected ? 'var(--sk-primary)' : `hsla(${hue}, 70%, 65%, 0.8)`;
  const bgColor = isSelected ? `hsla(${hue}, 60%, 50%, 0.25)` : `hsla(${hue}, 60%, 50%, 0.12)`;

  // Build a data-URI for the image thumbnail.  The overlay stores raw
  // base64 — we detect the MIME type from the first bytes of the decoded
  // header (magic-byte prefixes in base64 encoding).
  const imgSrc = useMemo(() => {
    if (!overlay.dataBase64) return undefined;
    let mime = 'image/jpeg'; // default fallback
    if (overlay.dataBase64.startsWith('iVBOR')) mime = 'image/png';
    else if (overlay.dataBase64.startsWith('R0lGOD')) mime = 'image/gif';
    else if (overlay.dataBase64.startsWith('UklGR')) mime = 'image/webp';
    return `data:${mime};base64,${overlay.dataBase64}`;
  }, [overlay.dataBase64]);

  return (
    <LayerBox
      ref={layerRef}
      className="nodrag nopan"
      style={{
        left: overlay.x,
        top: overlay.y,
        width: overlay.width,
        height: overlay.height,
        opacity: overlay.visible ? overlay.opacity : 0.2,
        zIndex: overlay.zIndex ?? 200 + index,
        border: `2px solid ${borderColor}`,
        background: bgColor,
        filter: overlay.visible ? undefined : 'grayscale(0.6)',
      }}
      onPointerDown={handlePointerDown}
    >
      {imgSrc && (
        <img
          src={imgSrc}
          alt={`Image overlay ${index}`}
          style={{
            position: 'absolute',
            inset: 0,
            width: '100%',
            height: '100%',
            objectFit: 'contain',
            pointerEvents: 'none',
            opacity: 0.85,
          }}
        />
      )}
      <LayerLabel>IMG #{index}</LayerLabel>
      {isSelected && <ResizeHandles layerId={overlay.id} onResizeStart={onResizeStart} />}
    </LayerBox>
  );
});
ImageOverlayLayer.displayName = 'ImageOverlayLayer';

// ── Main canvas ─────────────────────────────────────────────────────────────

export interface CompositorCanvasProps {
  canvasWidth: number;
  canvasHeight: number;
  layers: LayerState[];
  textOverlays?: TextOverlayState[];
  imageOverlays?: ImageOverlayState[];
  selectedLayerId: string | null;
  onSelectLayer: (id: string | null) => void;
  onLayerPointerDown: (layerId: string, e: React.PointerEvent) => void;
  onResizePointerDown: (layerId: string, handle: ResizeHandle, e: React.PointerEvent) => void;
  onTextEdit?: (id: string, updates: Partial<Omit<TextOverlayState, 'id'>>) => void;
  layerRefs: React.MutableRefObject<Map<string, HTMLDivElement>>;
  disabled?: boolean;
}

export const CompositorCanvas: React.FC<CompositorCanvasProps> = React.memo(
  ({
    canvasWidth,
    canvasHeight,
    layers,
    textOverlays = [],
    imageOverlays = [],
    selectedLayerId,
    onSelectLayer,
    onLayerPointerDown,
    onResizePointerDown,
    onTextEdit,
    layerRefs,
    disabled,
  }) => {
    const outerRef = useRef<HTMLDivElement>(null);
    const [scale, setScale] = useState(1);

    // Recompute scale when container resizes
    useEffect(() => {
      const outer = outerRef.current;
      if (!outer) return;

      const observer = new ResizeObserver((entries) => {
        for (const entry of entries) {
          const containerWidth = entry.contentRect.width;
          if (containerWidth > 0 && canvasWidth > 0) {
            setScale(containerWidth / canvasWidth);
          }
        }
      });

      observer.observe(outer);
      return () => observer.disconnect();
    }, [canvasWidth]);

    // Issue #1 fix: blur any active element (e.g. inline text input) before
    // deselecting so that the input's onBlur → commitEdit fires reliably.
    const handlePaneClick = useCallback(() => {
      if (document.activeElement instanceof HTMLElement) {
        document.activeElement.blur();
      }
      onSelectLayer(null);
    }, [onSelectLayer]);

    const setLayerRef = useCallback(
      (layerId: string) => (el: HTMLDivElement | null) => {
        if (el) {
          layerRefs.current.set(layerId, el);
        } else {
          layerRefs.current.delete(layerId);
        }
      },
      [layerRefs]
    );

    const noopPointerDown = useCallback(() => {}, []);
    const noopResizeStart = useCallback(
      (() => {}) as (id: string, handle: ResizeHandle, e: React.PointerEvent) => void,
      []
    );

    const hasContent = layers.length > 0 || textOverlays.length > 0 || imageOverlays.length > 0;

    return (
      <CanvasOuter ref={outerRef} className="nodrag nopan">
        <CanvasInner
          data-canvas-width={canvasWidth}
          style={{
            width: canvasWidth,
            height: canvasHeight,
            transform: `scale(${scale})`,
            // CSS transform doesn't affect layout box — use negative margin
            // to collapse the unscaled height so the outer container fits tightly.
            marginBottom: canvasHeight * (scale - 1),
          }}
          onPointerDown={disabled ? undefined : handlePaneClick}
        >
          {!hasContent ? (
            <EmptyState>No layers configured</EmptyState>
          ) : (
            <>
              {layers.map((layer, i) => (
                <VideoLayer
                  key={layer.id}
                  layer={layer}
                  index={i}
                  isSelected={selectedLayerId === layer.id}
                  onPointerDown={disabled ? noopPointerDown : onLayerPointerDown}
                  onResizeStart={disabled ? noopResizeStart : onResizePointerDown}
                  layerRef={setLayerRef(layer.id)}
                />
              ))}
              {textOverlays.map((overlay, i) => (
                <TextOverlayLayer
                  key={overlay.id}
                  overlay={overlay}
                  index={i}
                  isSelected={selectedLayerId === overlay.id}
                  scale={scale}
                  onPointerDown={disabled ? noopPointerDown : onLayerPointerDown}
                  onTextEdit={disabled ? undefined : onTextEdit}
                  layerRef={setLayerRef(overlay.id)}
                />
              ))}
              {imageOverlays.map((overlay, i) => (
                <ImageOverlayLayer
                  key={overlay.id}
                  overlay={overlay}
                  index={i}
                  isSelected={selectedLayerId === overlay.id}
                  onPointerDown={disabled ? noopPointerDown : onLayerPointerDown}
                  onResizeStart={disabled ? noopResizeStart : onResizePointerDown}
                  layerRef={setLayerRef(overlay.id)}
                />
              ))}
            </>
          )}
        </CanvasInner>
      </CanvasOuter>
    );
  }
);

CompositorCanvas.displayName = 'CompositorCanvas';
