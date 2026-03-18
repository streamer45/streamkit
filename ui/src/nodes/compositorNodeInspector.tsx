// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Inspector panel for the compositor node.
 *
 * Reads from Jotai atoms directly to derive the selected layer, its kind,
 * inspector properties and display name.  This avoids passing large derived
 * objects through props and keeps re-renders confined to the inspector when
 * appearance-only properties (opacity, rotation) change on the selected layer.
 *
 * Individual inspector controls (OpacityControl, RotationControl, etc.) are
 * React.memo'd with primitive props, so only the control whose value actually
 * changed re-renders — e.g. an opacity drag only updates OpacityControl, not
 * RotationControl or MirrorControl.
 */

import { useAtomValue } from 'jotai/react';
import React, { useCallback, useMemo, useRef } from 'react';

import {
  selectedLayerIdAtom,
  selectedLayerKindAtom,
  layerAtoms,
  textOverlayAtoms,
  imageOverlayAtoms,
  nullLayerAtom,
  nullTextOverlayAtom,
  nullImageOverlayAtom,
  textOverlayIdsAtom,
  imageOverlayIdsAtom,
} from '@/hooks/compositorAtoms';
import type { TextOverlayState, ImageOverlayState } from '@/hooks/compositorLayerParsers';

import {
  ColorInput,
  ControlRow,
  ControlValue,
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
  CompactSliderRange,
  CompactSliderRoot,
  CompactSliderThumb,
  CompactSliderTrack,
  CropZoomControl,
  InspectorHeaderSection,
  MirrorControl,
  OpacityControl,
  RotationControl,
} from './compositorNodeWidgets';
import type { CropZoomPatch, PositionSizePatch } from './compositorNodeWidgets';

// ── Compositor inspector component ──────────────────────────────────────────

export interface CompositorInspectorProps {
  updateLayerOpacity: (id: string, v: number) => void;
  updateLayerRotation: (id: string, v: number) => void;
  updateLayerMirror: (id: string, axis: 'horizontal' | 'vertical') => void;
  updateLayerCropZoom: (id: string, patch: CropZoomPatch) => void;
  updateLayerPositionSize: (id: string, patch: PositionSizePatch) => void;
  updateTextOverlay: (id: string, patch: Partial<TextOverlayState>) => void;
  updateImageOverlay: (id: string, patch: Partial<ImageOverlayState>) => void;
  textInputRef?: React.RefObject<HTMLTextAreaElement | null>;
  disabled: boolean;
}

export const CompositorInspector: React.FC<CompositorInspectorProps> = React.memo(
  ({
    updateLayerOpacity,
    updateLayerRotation,
    updateLayerMirror,
    updateLayerCropZoom,
    updateLayerPositionSize,
    updateTextOverlay,
    updateImageOverlay,
    textInputRef: externalTextInputRef,
    disabled,
  }) => {
    // ── Read from atoms ────────────────────────────────────────────────────
    const selectedLayerId = useAtomValue(selectedLayerIdAtom);
    const selectedLayerKind = useAtomValue(selectedLayerKindAtom);
    const selectedLayer = useAtomValue(
      selectedLayerId ? layerAtoms(selectedLayerId) : nullLayerAtom
    );
    const selectedTextOverlay = useAtomValue(
      selectedLayerId ? textOverlayAtoms(selectedLayerId) : nullTextOverlayAtom
    );
    const selectedImageOverlay = useAtomValue(
      selectedLayerId ? imageOverlayAtoms(selectedLayerId) : nullImageOverlayAtom
    );
    const textOverlayIds = useAtomValue(textOverlayIdsAtom);
    const imageOverlayIds = useAtomValue(imageOverlayIdsAtom);

    // ── Fallback text input ref ────────────────────────────────────────────
    const internalTextInputRef = useRef<HTMLTextAreaElement>(null);
    const textInputRef = externalTextInputRef ?? internalTextInputRef;

    // ── Stable callbacks (must be before early return) ─────────────────────

    const handleOpacityChange = useCallback(
      (v: number) => {
        if (!selectedLayerId || !selectedLayerKind) return;
        if (selectedLayerKind === 'video') updateLayerOpacity(selectedLayerId, v);
        else if (selectedLayerKind === 'text') updateTextOverlay(selectedLayerId, { opacity: v });
        else updateImageOverlay(selectedLayerId, { opacity: v });
      },
      [
        selectedLayerId,
        selectedLayerKind,
        updateLayerOpacity,
        updateTextOverlay,
        updateImageOverlay,
      ]
    );

    const handleRotationChange = useCallback(
      (v: number) => {
        if (!selectedLayerId || !selectedLayerKind) return;
        if (selectedLayerKind === 'video') updateLayerRotation(selectedLayerId, v);
        else if (selectedLayerKind === 'text')
          updateTextOverlay(selectedLayerId, { rotationDegrees: v });
        else updateImageOverlay(selectedLayerId, { rotationDegrees: v });
      },
      [
        selectedLayerId,
        selectedLayerKind,
        updateLayerRotation,
        updateTextOverlay,
        updateImageOverlay,
      ]
    );

    const handleMirrorToggle = useCallback(
      (axis: 'horizontal' | 'vertical') => {
        if (!selectedLayerId) return;
        updateLayerMirror(selectedLayerId, axis);
      },
      [selectedLayerId, updateLayerMirror]
    );

    const handleCropZoomChange = useCallback(
      (patch: CropZoomPatch) => {
        if (!selectedLayerId || selectedLayerKind !== 'video') return;
        updateLayerCropZoom(selectedLayerId, patch);
      },
      [selectedLayerId, selectedLayerKind, updateLayerCropZoom]
    );

    const handlePositionSizeChange = useCallback(
      (patch: PositionSizePatch) => {
        if (!selectedLayerId || !selectedLayerKind) return;
        if (selectedLayerKind === 'video') updateLayerPositionSize(selectedLayerId, patch);
        else if (selectedLayerKind === 'text') updateTextOverlay(selectedLayerId, patch);
        else updateImageOverlay(selectedLayerId, patch);
      },
      [
        selectedLayerId,
        selectedLayerKind,
        updateLayerPositionSize,
        updateTextOverlay,
        updateImageOverlay,
      ]
    );

    // ── Text inspector children ────────────────────────────────────────────
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
                onChange={(e) =>
                  updateTextOverlay(selectedTextOverlay.id, { text: e.target.value })
                }
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
            <OverlayEditRow>
              <span style={{ color: 'var(--sk-text-muted)', fontSize: 10 }}>Alpha</span>
            </OverlayEditRow>
            <ControlRow>
              <CompactSliderRoot
                value={[selectedTextOverlay.color[3]]}
                onValueChange={([v]) => {
                  const [r, g, b] = selectedTextOverlay.color;
                  updateTextOverlay(selectedTextOverlay.id, { color: [r, g, b, v] });
                }}
                min={0}
                max={255}
                step={1}
                disabled={disabled}
                className="nodrag nopan"
              >
                <CompactSliderTrack>
                  <CompactSliderRange />
                </CompactSliderTrack>
                <CompactSliderThumb />
              </CompactSliderRoot>
              <ControlValue>{Math.round((selectedTextOverlay.color[3] / 255) * 100)}%</ControlValue>
            </ControlRow>
          </InspectorSection>
        </>
      );
      // textInputRef is a stable useRef — omitted from deps intentionally.
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [selectedTextOverlay, updateTextOverlay, disabled]);

    // ── Early return after all hooks ───────────────────────────────────────
    const source = selectedLayer ?? selectedTextOverlay ?? selectedImageOverlay;
    if (!source || !selectedLayerId) return null;

    // ── Derive selected layer name ─────────────────────────────────────────
    let selectedLayerName = '';
    if (selectedLayer) {
      selectedLayerName = friendlyLabel(selectedLayer.id, 'video');
    } else if (selectedTextOverlay) {
      const idx = textOverlayIds.indexOf(selectedTextOverlay.id);
      selectedLayerName = friendlyLabel(selectedTextOverlay.id, 'text', idx >= 0 ? idx : 0);
    } else if (selectedImageOverlay) {
      const idx = imageOverlayIds.indexOf(selectedImageOverlay.id);
      selectedLayerName = friendlyLabel(selectedImageOverlay.id, 'image', idx >= 0 ? idx : 0);
    }

    const dimensionsReadOnly = selectedLayerKind === 'text';

    return (
      <>
        <SidePanelDivider />
        <InspectorControls>
          <InspectorHeaderSection
            name={selectedLayerName}
            x={source.x}
            y={source.y}
            width={source.width}
            height={source.height}
            onPositionSizeChange={handlePositionSizeChange}
            disabled={disabled}
            dimensionsReadOnly={dimensionsReadOnly}
          />
          {textInspectorChildren}
          <OpacityControl
            opacity={source.opacity}
            onChange={handleOpacityChange}
            disabled={disabled}
          />
          <RotationControl
            rotationDegrees={source.rotationDegrees}
            onChange={handleRotationChange}
            disabled={disabled}
          />
          <MirrorControl
            mirrorHorizontal={source.mirrorHorizontal}
            mirrorVertical={source.mirrorVertical}
            onToggle={handleMirrorToggle}
            disabled={disabled}
          />
          {selectedLayerKind === 'video' && selectedLayer && (
            <CropZoomControl
              cropZoom={selectedLayer.cropZoom}
              cropX={selectedLayer.cropX}
              cropY={selectedLayer.cropY}
              onChange={handleCropZoomChange}
              disabled={disabled}
            />
          )}
        </InspectorControls>
      </>
    );
  }
);
CompositorInspector.displayName = 'CompositorInspector';
