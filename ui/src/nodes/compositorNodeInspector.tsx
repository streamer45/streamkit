// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useAtomValue } from 'jotai/react';
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import {
  selectedLayerIdAtom,
  selectedLayerKindAtom,
  layerAtoms,
  layerOpacityAtom,
  layerRotationAtom,
  textOverlayAtoms,
  imageOverlayAtoms,
  nullLayerAtom,
  nullTextOverlayAtom,
  nullImageOverlayAtom,
  nullOpacityAtom,
  nullRotationAtom,
  textOverlayIdsAtom,
  imageOverlayIdsAtom,
} from '@/hooks/compositorAtoms';
import type { LayerKind } from '@/hooks/compositorConstants';
import type { TextOverlayState, ImageOverlayState } from '@/hooks/compositorLayerParsers';
import { listFontAssets, loadFontAssets } from '@/services/fontAssets';
import type { FontAsset } from '@/types/generated/api-types';

import {
  DEFAULT_FONT_OPTIONS,
  ColorInput,
  ControlRow,
  ControlValue,
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
  /** Called when a continuous slider interaction starts (pointer down on slider). */
  onInteractionStart?: () => void;
  /** Called when a continuous slider interaction ends (pointer up / value commit). */
  onInteractionEnd?: () => void;
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
    onInteractionStart,
    onInteractionEnd,
  }) => {
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

    const internalTextInputRef = useRef<HTMLTextAreaElement>(null);
    const textInputRef = externalTextInputRef ?? internalTextInputRef;

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

    const source = selectedLayer ?? selectedTextOverlay ?? selectedImageOverlay;
    if (!source || !selectedLayerId) return null;

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
          {selectedLayerKind === 'text' && (
            <TextStyleSection
              selectedLayerId={selectedLayerId}
              updateTextOverlay={updateTextOverlay}
              disabled={disabled}
              onInteractionStart={onInteractionStart}
              onInteractionEnd={onInteractionEnd}
              textInputRef={textInputRef}
            />
          )}
          <ConnectedOpacityControl
            selectedLayerId={selectedLayerId}
            selectedLayerKind={selectedLayerKind}
            onChange={handleOpacityChange}
            disabled={disabled}
            onInteractionStart={onInteractionStart}
            onInteractionEnd={onInteractionEnd}
          />
          <ConnectedRotationControl
            selectedLayerId={selectedLayerId}
            selectedLayerKind={selectedLayerKind}
            onChange={handleRotationChange}
            disabled={disabled}
            onInteractionStart={onInteractionStart}
            onInteractionEnd={onInteractionEnd}
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
              cropShape={selectedLayer.cropShape}
              onChange={handleCropZoomChange}
              disabled={disabled}
              onInteractionStart={onInteractionStart}
              onInteractionEnd={onInteractionEnd}
            />
          )}
        </InspectorControls>
      </>
    );
  }
);
CompositorInspector.displayName = 'CompositorInspector';

const TextStyleSection: React.FC<{
  selectedLayerId: string;
  updateTextOverlay: (id: string, patch: Partial<TextOverlayState>) => void;
  disabled: boolean;
  onInteractionStart?: () => void;
  onInteractionEnd?: () => void;
  textInputRef: React.RefObject<HTMLTextAreaElement | null>;
}> = React.memo(
  ({
    selectedLayerId,
    updateTextOverlay,
    disabled,
    onInteractionStart,
    onInteractionEnd,
    textInputRef,
  }) => {
    const overlay = useAtomValue(textOverlayAtoms(selectedLayerId));

    const [fontAssets, setFontAssets] = useState<FontAsset[]>([]);

    useEffect(() => {
      let cancelled = false;
      listFontAssets()
        .then(async (assets) => {
          if (cancelled) return;
          setFontAssets(assets);
          await loadFontAssets(assets);
        })
        .catch(() => {
          // Silently fall back to default font options on error.
        });
      return () => {
        cancelled = true;
      };
    }, []);

    const systemFonts = useMemo(() => fontAssets.filter((a) => a.is_system), [fontAssets]);
    const userFonts = useMemo(() => fontAssets.filter((a) => !a.is_system), [fontAssets]);

    const overlayRef = useRef(overlay);
    overlayRef.current = overlay;

    const handleTextChange = useCallback(
      (e: React.ChangeEvent<HTMLTextAreaElement>) => {
        updateTextOverlay(selectedLayerId, { text: e.target.value });
      },
      [selectedLayerId, updateTextOverlay]
    );

    const handleFontSizeChange = useCallback(
      (e: React.ChangeEvent<HTMLInputElement>) => {
        const v = Number.parseInt(e.target.value, 10);
        if (!Number.isNaN(v) && v > 0) updateTextOverlay(selectedLayerId, { fontSize: v });
      },
      [selectedLayerId, updateTextOverlay]
    );

    const handleFontChange = useCallback(
      (e: React.ChangeEvent<HTMLSelectElement>) => {
        updateTextOverlay(selectedLayerId, { fontName: e.target.value });
      },
      [selectedLayerId, updateTextOverlay]
    );

    const handleColorChange = useCallback(
      (e: React.ChangeEvent<HTMLInputElement>) => {
        const ov = overlayRef.current;
        if (!ov) return;
        updateTextOverlay(selectedLayerId, {
          color: hexToRgba(e.target.value, ov.color[3]),
        });
      },
      [selectedLayerId, updateTextOverlay]
    );

    const handleAlphaChange = useCallback(
      ([v]: number[]) => {
        const ov = overlayRef.current;
        if (!ov) return;
        const [r, g, b] = ov.color;
        updateTextOverlay(selectedLayerId, { color: [r, g, b, v] });
      },
      [selectedLayerId, updateTextOverlay]
    );

    if (!overlay) return null;

    return (
      <>
        <InspectorSection>
          <InspectorSectionLabel>Content</InspectorSectionLabel>
          <OverlayEditRow>
            <OverlayTextInput
              ref={textInputRef}
              value={overlay.text}
              onChange={handleTextChange}
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
              value={overlay.fontSize}
              onChange={handleFontSizeChange}
              disabled={disabled}
              className="nodrag nopan"
            />
          </OverlayEditRow>
          <OverlayEditRow>
            <span style={{ color: 'var(--sk-text-muted)', fontSize: 10 }}>Font</span>
            <FontSelect
              value={overlay.fontName}
              onChange={handleFontChange}
              disabled={disabled}
              className="nodrag nopan"
            >
              {fontAssets.length === 0 ? (
                <optgroup label="System">
                  {DEFAULT_FONT_OPTIONS.map((opt) => (
                    <option key={opt.value} value={opt.value}>
                      {opt.label}
                    </option>
                  ))}
                </optgroup>
              ) : (
                <>
                  {systemFonts.length > 0 && (
                    <optgroup label="System">
                      {systemFonts.map((font) => (
                        <option key={font.path} value={font.path}>
                          {font.name}
                        </option>
                      ))}
                    </optgroup>
                  )}
                  {userFonts.length > 0 && (
                    <optgroup label="User">
                      {userFonts.map((font) => (
                        <option key={font.path} value={font.path}>
                          {font.name}
                        </option>
                      ))}
                    </optgroup>
                  )}
                </>
              )}
            </FontSelect>
          </OverlayEditRow>
          <OverlayEditRow>
            <span style={{ color: 'var(--sk-text-muted)', fontSize: 10 }}>Color</span>
            <ColorInput
              type="color"
              value={rgbaToHex(overlay.color)}
              onChange={handleColorChange}
              disabled={disabled}
              className="nodrag nopan"
            />
          </OverlayEditRow>
          <OverlayEditRow>
            <span style={{ color: 'var(--sk-text-muted)', fontSize: 10 }}>Alpha</span>
          </OverlayEditRow>
          <ControlRow>
            <CompactSliderRoot
              value={[overlay.color[3]]}
              onValueChange={handleAlphaChange}
              onPointerDownCapture={onInteractionStart ? () => onInteractionStart() : undefined}
              onValueCommit={onInteractionEnd ? () => onInteractionEnd() : undefined}
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
            <ControlValue>{Math.round((overlay.color[3] / 255) * 100)}%</ControlValue>
          </ControlRow>
        </InspectorSection>
      </>
    );
  }
);
TextStyleSection.displayName = 'TextStyleSection';

// Field-level atom subscriptions: Jotai's Object.is check means changing
// opacity won't re-render ConnectedRotationControl (same number returned).
const ConnectedOpacityControl: React.FC<{
  selectedLayerId: string;
  selectedLayerKind: LayerKind | null;
  onChange: (v: number) => void;
  disabled: boolean;
  onInteractionStart?: () => void;
  onInteractionEnd?: () => void;
}> = React.memo(
  ({
    selectedLayerId,
    selectedLayerKind,
    onChange,
    disabled,
    onInteractionStart,
    onInteractionEnd,
  }) => {
    const videoOpacity = useAtomValue(
      selectedLayerKind === 'video' ? layerOpacityAtom(selectedLayerId) : nullOpacityAtom
    );
    const textOverlay = useAtomValue(
      selectedLayerKind === 'text' ? textOverlayAtoms(selectedLayerId) : nullTextOverlayAtom
    );
    const imageOverlay = useAtomValue(
      selectedLayerKind === 'image' ? imageOverlayAtoms(selectedLayerId) : nullImageOverlayAtom
    );

    const opacity =
      selectedLayerKind === 'video'
        ? videoOpacity
        : (textOverlay?.opacity ?? imageOverlay?.opacity ?? 1);

    return (
      <OpacityControl
        opacity={opacity}
        onChange={onChange}
        disabled={disabled}
        onInteractionStart={onInteractionStart}
        onInteractionEnd={onInteractionEnd}
      />
    );
  }
);
ConnectedOpacityControl.displayName = 'ConnectedOpacityControl';

const ConnectedRotationControl: React.FC<{
  selectedLayerId: string;
  selectedLayerKind: LayerKind | null;
  onChange: (v: number) => void;
  disabled: boolean;
  onInteractionStart?: () => void;
  onInteractionEnd?: () => void;
}> = React.memo(
  ({
    selectedLayerId,
    selectedLayerKind,
    onChange,
    disabled,
    onInteractionStart,
    onInteractionEnd,
  }) => {
    const videoRotation = useAtomValue(
      selectedLayerKind === 'video' ? layerRotationAtom(selectedLayerId) : nullRotationAtom
    );
    const textOverlay = useAtomValue(
      selectedLayerKind === 'text' ? textOverlayAtoms(selectedLayerId) : nullTextOverlayAtom
    );
    const imageOverlay = useAtomValue(
      selectedLayerKind === 'image' ? imageOverlayAtoms(selectedLayerId) : nullImageOverlayAtom
    );

    const rotationDegrees =
      selectedLayerKind === 'video'
        ? videoRotation
        : (textOverlay?.rotationDegrees ?? imageOverlay?.rotationDegrees ?? 0);

    return (
      <RotationControl
        rotationDegrees={rotationDegrees}
        onChange={onChange}
        disabled={disabled}
        onInteractionStart={onInteractionStart}
        onInteractionEnd={onInteractionEnd}
      />
    );
  }
);
ConnectedRotationControl.displayName = 'ConnectedRotationControl';
