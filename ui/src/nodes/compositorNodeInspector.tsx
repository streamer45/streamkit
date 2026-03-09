// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Inspector panel for the compositor node.
 *
 * Contains the text-overlay inspector children, stable callback builders
 * for opacity / rotation / mirror / position-size changes, and the
 * CompositorInspector component that renders the full inspector section.
 */

import React, { useCallback, useMemo, useRef } from 'react';

import type { TextOverlayState, ImageOverlayState, LayerState } from '@/hooks/useCompositorLayers';

import {
  ColorInput,
  FONT_OPTIONS,
  FontSelect,
  InspectorControls,
  InspectorSection,
  InspectorSectionLabel,
  OverlayEditRow,
  OverlayNumInput,
  OverlayTextInput,
  SidePanelDivider,
  friendlyLabel,
  hexToRgba,
  rgbaToHex,
} from './compositorNodeParts';
import {
  InspectorHeaderSection,
  MirrorControl,
  OpacityControl,
  RotationControl,
} from './compositorNodeWidgets';
import type { PositionSizePatch } from './compositorNodeWidgets';

// ── Inspector props derivation ──────────────────────────────────────────────

interface InspectorLayerProps {
  x: number;
  y: number;
  width: number;
  height: number;
  opacity: number;
  rotationDegrees: number;
  mirrorHorizontal: boolean;
  mirrorVertical: boolean;
}

/** Derive the common inspector props from whichever layer is selected. */
export function useInspectorProps(
  selectedLayer: LayerState | undefined,
  selectedTextOverlay: TextOverlayState | undefined,
  selectedImageOverlay: ImageOverlayState | undefined
): InspectorLayerProps | null {
  return useMemo(() => {
    if (selectedLayer)
      return {
        x: selectedLayer.x,
        y: selectedLayer.y,
        width: selectedLayer.width,
        height: selectedLayer.height,
        opacity: selectedLayer.opacity,
        rotationDegrees: selectedLayer.rotationDegrees,
        mirrorHorizontal: selectedLayer.mirrorHorizontal,
        mirrorVertical: selectedLayer.mirrorVertical,
      };
    if (selectedTextOverlay)
      return {
        x: selectedTextOverlay.x,
        y: selectedTextOverlay.y,
        width: selectedTextOverlay.width,
        height: selectedTextOverlay.height,
        opacity: selectedTextOverlay.opacity,
        rotationDegrees: selectedTextOverlay.rotationDegrees,
        mirrorHorizontal: selectedTextOverlay.mirrorHorizontal,
        mirrorVertical: selectedTextOverlay.mirrorVertical,
      };
    if (selectedImageOverlay)
      return {
        x: selectedImageOverlay.x,
        y: selectedImageOverlay.y,
        width: selectedImageOverlay.width,
        height: selectedImageOverlay.height,
        opacity: selectedImageOverlay.opacity,
        rotationDegrees: selectedImageOverlay.rotationDegrees,
        mirrorHorizontal: selectedImageOverlay.mirrorHorizontal,
        mirrorVertical: selectedImageOverlay.mirrorVertical,
      };
    return null;
  }, [selectedLayer, selectedTextOverlay, selectedImageOverlay]);
}

// ── Stable callback builders ────────────────────────────────────────────────

type LayerKindTag = 'video' | 'text' | 'image';

export function useSelectedOpacityChange(
  selectedLayerId: string | null,
  selectedLayerKind: LayerKindTag | null,
  updateLayerOpacity: (id: string, v: number) => void,
  updateTextOverlay: (id: string, patch: { opacity: number }) => void,
  updateImageOverlay: (id: string, patch: { opacity: number }) => void
): (v: number) => void {
  return useCallback(
    (v: number) => {
      if (!selectedLayerId || !selectedLayerKind) return;
      if (selectedLayerKind === 'video') {
        updateLayerOpacity(selectedLayerId, v);
      } else if (selectedLayerKind === 'text') {
        updateTextOverlay(selectedLayerId, { opacity: v });
      } else {
        updateImageOverlay(selectedLayerId, { opacity: v });
      }
    },
    [selectedLayerId, selectedLayerKind, updateLayerOpacity, updateTextOverlay, updateImageOverlay]
  );
}

export function useSelectedRotationChange(
  selectedLayerId: string | null,
  selectedLayerKind: LayerKindTag | null,
  updateLayerRotation: (id: string, v: number) => void,
  updateTextOverlay: (id: string, patch: { rotationDegrees: number }) => void,
  updateImageOverlay: (id: string, patch: { rotationDegrees: number }) => void
): (v: number) => void {
  return useCallback(
    (v: number) => {
      if (!selectedLayerId || !selectedLayerKind) return;
      if (selectedLayerKind === 'video') {
        updateLayerRotation(selectedLayerId, v);
      } else if (selectedLayerKind === 'text') {
        updateTextOverlay(selectedLayerId, { rotationDegrees: v });
      } else {
        updateImageOverlay(selectedLayerId, { rotationDegrees: v });
      }
    },
    [selectedLayerId, selectedLayerKind, updateLayerRotation, updateTextOverlay, updateImageOverlay]
  );
}

export function useSelectedMirrorToggle(
  selectedLayerId: string | null,
  updateLayerMirror: (id: string, axis: 'horizontal' | 'vertical') => void
): (axis: 'horizontal' | 'vertical') => void {
  return useCallback(
    (axis: 'horizontal' | 'vertical') => {
      if (!selectedLayerId) return;
      updateLayerMirror(selectedLayerId, axis);
    },
    [selectedLayerId, updateLayerMirror]
  );
}

export function useSelectedPositionSizeChange(
  selectedLayerId: string | null,
  selectedLayerKind: LayerKindTag | null,
  updateLayerPositionSize: (id: string, patch: PositionSizePatch) => void,
  updateTextOverlay: (id: string, patch: Partial<TextOverlayState>) => void,
  updateImageOverlay: (id: string, patch: Partial<ImageOverlayState>) => void
): (patch: PositionSizePatch) => void {
  return useCallback(
    (patch: PositionSizePatch) => {
      if (!selectedLayerId || !selectedLayerKind) return;
      if (selectedLayerKind === 'video') {
        updateLayerPositionSize(selectedLayerId, patch);
      } else if (selectedLayerKind === 'text') {
        updateTextOverlay(selectedLayerId, patch);
      } else {
        updateImageOverlay(selectedLayerId, patch);
      }
    },
    [
      selectedLayerId,
      selectedLayerKind,
      updateLayerPositionSize,
      updateTextOverlay,
      updateImageOverlay,
    ]
  );
}

// ── Selected layer name ─────────────────────────────────────────────────────

export function useSelectedLayerName(
  selectedLayer: LayerState | undefined,
  selectedTextOverlay: TextOverlayState | undefined,
  selectedImageOverlay: ImageOverlayState | undefined,
  textOverlays: TextOverlayState[],
  imageOverlays: ImageOverlayState[]
): string {
  return useMemo(() => {
    if (selectedLayer) return friendlyLabel(selectedLayer.id, 'video');
    if (selectedTextOverlay) {
      const idx = textOverlays.indexOf(selectedTextOverlay);
      return friendlyLabel(selectedTextOverlay.id, 'text', idx >= 0 ? idx : 0);
    }
    if (selectedImageOverlay) {
      const idx = imageOverlays.indexOf(selectedImageOverlay);
      return friendlyLabel(selectedImageOverlay.id, 'image', idx >= 0 ? idx : 0);
    }
    return '';
  }, [selectedLayer, selectedTextOverlay, selectedImageOverlay, textOverlays, imageOverlays]);
}

// ── Text inspector children ─────────────────────────────────────────────────

export function useTextInspectorChildren(
  selectedTextOverlay: TextOverlayState | undefined,
  updateTextOverlay: (id: string, patch: Partial<TextOverlayState>) => void,
  disabled: boolean
): {
  textInspectorChildren: React.ReactNode;
  textInputRef: React.RefObject<HTMLTextAreaElement | null>;
} {
  const textInputRef = useRef<HTMLTextAreaElement>(null);

  const textInspectorChildren = useMemo(() => {
    if (!selectedTextOverlay) return null;
    return (
      <>
        <InspectorSection>
          <InspectorSectionLabel>Content</InspectorSectionLabel>
          <OverlayEditRow>
            <OverlayTextInput
              ref={textInputRef}
              value={selectedTextOverlay.text}
              onChange={(e) => updateTextOverlay(selectedTextOverlay.id, { text: e.target.value })}
              placeholder="Text content"
              disabled={disabled}
              className="nodrag nopan"
            />
          </OverlayEditRow>
        </InspectorSection>
        <InspectorSection>
          <InspectorSectionLabel>Style</InspectorSectionLabel>
          <OverlayEditRow>
            <span style={{ color: 'var(--sk-text-muted)', fontSize: 10 }}>Size</span>
            <OverlayNumInput
              type="number"
              value={selectedTextOverlay.fontSize}
              onChange={(e) => {
                const v = Number.parseInt(e.target.value, 10);
                if (!Number.isNaN(v) && v > 0)
                  updateTextOverlay(selectedTextOverlay.id, { fontSize: v });
              }}
              disabled={disabled}
              className="nodrag nopan"
            />
          </OverlayEditRow>
          <OverlayEditRow>
            <span style={{ color: 'var(--sk-text-muted)', fontSize: 10 }}>Font</span>
            <FontSelect
              value={selectedTextOverlay.fontName}
              onChange={(e) =>
                updateTextOverlay(selectedTextOverlay.id, { fontName: e.target.value })
              }
              disabled={disabled}
              className="nodrag nopan"
            >
              {FONT_OPTIONS.map((opt) => (
                <option key={opt.value} value={opt.value}>
                  {opt.label}
                </option>
              ))}
            </FontSelect>
          </OverlayEditRow>
          <OverlayEditRow>
            <span style={{ color: 'var(--sk-text-muted)', fontSize: 10 }}>Color</span>
            <ColorInput
              type="color"
              value={rgbaToHex(selectedTextOverlay.color)}
              onChange={(e) =>
                updateTextOverlay(selectedTextOverlay.id, {
                  color: hexToRgba(e.target.value, selectedTextOverlay.color[3]),
                })
              }
              disabled={disabled}
              className="nodrag nopan"
            />
          </OverlayEditRow>
        </InspectorSection>
      </>
    );
  }, [selectedTextOverlay, updateTextOverlay, disabled]);

  return { textInspectorChildren, textInputRef };
}

// ── Compositor inspector component ──────────────────────────────────────────

export interface CompositorInspectorProps {
  inspectorProps: InspectorLayerProps | null;
  selectedLayerName: string;
  textInspectorChildren: React.ReactNode;
  handleSelectedOpacityChange: (v: number) => void;
  handleSelectedRotationChange: (v: number) => void;
  handleSelectedMirrorToggle: (axis: 'horizontal' | 'vertical') => void;
  handleSelectedPositionSizeChange: (patch: PositionSizePatch) => void;
  disabled: boolean;
}

export const CompositorInspector: React.FC<CompositorInspectorProps> = React.memo(
  ({
    inspectorProps,
    selectedLayerName,
    textInspectorChildren,
    handleSelectedOpacityChange,
    handleSelectedRotationChange,
    handleSelectedMirrorToggle,
    handleSelectedPositionSizeChange,
    disabled,
  }) => {
    if (!inspectorProps) return null;
    return (
      <>
        <SidePanelDivider />
        <InspectorControls>
          <InspectorHeaderSection
            name={selectedLayerName}
            x={inspectorProps.x}
            y={inspectorProps.y}
            width={inspectorProps.width}
            height={inspectorProps.height}
            onPositionSizeChange={handleSelectedPositionSizeChange}
            disabled={disabled}
          />
          {textInspectorChildren}
          <OpacityControl
            opacity={inspectorProps.opacity}
            onChange={handleSelectedOpacityChange}
            disabled={disabled}
          />
          <RotationControl
            rotationDegrees={inspectorProps.rotationDegrees}
            onChange={handleSelectedRotationChange}
            disabled={disabled}
          />
          <MirrorControl
            mirrorHorizontal={inspectorProps.mirrorHorizontal}
            mirrorVertical={inspectorProps.mirrorVertical}
            onToggle={handleSelectedMirrorToggle}
            disabled={disabled}
          />
        </InspectorControls>
      </>
    );
  }
);
CompositorInspector.displayName = 'CompositorInspector';
