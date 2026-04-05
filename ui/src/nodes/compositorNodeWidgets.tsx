// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Memoized React sub-components for CompositorNode's inspector panel.
 *
 * Extracted from compositorNodeParts to keep each module under the
 * max-lines lint threshold while preserving identical runtime behaviour.
 */

import styled from '@emotion/styled';
import { FlipHorizontal2, FlipVertical2, Image, Plus, RotateCcw, Type } from 'lucide-react';
import React, { useCallback, useEffect, useRef, useState } from 'react';

import { SKTooltip } from '@/components/Tooltip';
import type { LayerKind } from '@/hooks/useCompositorLayers';
import { uploadImageAsset } from '@/services/imageAssets';
import { showToast } from '@/stores/toastStore';
import { getLogger } from '@/utils/logger';

import {
  AddMenu,
  AddMenuItem,
  AddOverlayButton,
  ControlLabel,
  ControlRow,
  ControlValue,
  HiddenFileInput,
  InspectorSection,
  InspectorSectionLabel,
  MirrorButton,
  MirrorToggleRow,
  NoSelectionText,
  OverlayNumInput,
  PresetButton,
  ResetButton,
  RotationPresetsRow,
  type CompositorEntry,
  InspectorHeader,
  InspectorTitle,
  LayerInfoRow,
  ROTATION_PRESETS,
} from './compositorNodeParts';
import LayerReorderSection from './compositorNodeReorder';
import {
  CompactSliderRange,
  CompactSliderRoot,
  CompactSliderThumb,
  CompactSliderTrack,
} from './compositorSliderParts';

// ── Re-export slider parts for downstream consumers ────────────────────────
export { CompactSliderRange, CompactSliderRoot, CompactSliderThumb, CompactSliderTrack };

const logger = getLogger('compositorNodeWidgets');

// ── Position / size input grid ──────────────────────────────────────────────

const PositionSizeGrid = styled.div`
  display: grid;
  grid-template-columns: auto 1fr auto 1fr;
  gap: 3px 4px;
  align-items: center;
  margin-top: 4px;
`;

const FieldLabel = styled.span`
  font-size: 10px;
  color: var(--sk-text-muted);
  font-weight: 500;
  text-align: right;
`;

// ── Memoized inspector sub-sections ─────────────────────────────────────────

export type PositionSizePatch = {
  x?: number;
  y?: number;
  width?: number;
  height?: number;
};

export const InspectorHeaderSection: React.FC<{
  name: string;
  x: number;
  y: number;
  width: number;
  height: number;
  onPositionSizeChange: (patch: PositionSizePatch) => void;
  disabled?: boolean;
  dimensionsReadOnly?: boolean;
}> = React.memo(
  ({ name, x, y, width, height, onPositionSizeChange, disabled, dimensionsReadOnly }) => {
    // Fully uncontrolled inputs — zero useState, zero extra commits.
    // DOM values are synced imperatively via refs (same zero-render
    // pattern the compositor's drag/resize system uses).
    const focusedRef = useRef<string | null>(null);
    const xRef = useRef<HTMLInputElement>(null);
    const yRef = useRef<HTMLInputElement>(null);
    const wRef = useRef<HTMLInputElement>(null);
    const hRef = useRef<HTMLInputElement>(null);

    const commit = useCallback(
      (field: string, raw: string) => {
        const v = Number.parseInt(raw, 10);
        if (Number.isNaN(v)) return;
        switch (field) {
          case 'x':
            onPositionSizeChange({ x: v });
            break;
          case 'y':
            onPositionSizeChange({ y: v });
            break;
          case 'w':
            if (v > 0) onPositionSizeChange({ width: v });
            break;
          case 'h':
            if (v > 0) onPositionSizeChange({ height: v });
            break;
        }
      },
      [onPositionSizeChange]
    );

    // Sync DOM from props — skip the focused field to preserve user editing.
    const rx = String(Math.round(x));
    const ry = String(Math.round(y));
    const rw = String(Math.round(width));
    const rh = String(Math.round(height));
    if (xRef.current && focusedRef.current !== 'x') xRef.current.value = rx;
    if (yRef.current && focusedRef.current !== 'y') yRef.current.value = ry;
    if (wRef.current && focusedRef.current !== 'w') wRef.current.value = rw;
    if (hRef.current && focusedRef.current !== 'h') hRef.current.value = rh;

    // Per-field event handlers (no state updates, just refs + commit).
    const handlers = (field: string, ro?: boolean) => ({
      onFocus: () => {
        focusedRef.current = field;
      },
      onBlur: (e: React.FocusEvent<HTMLInputElement>) => {
        focusedRef.current = null;
        if (!ro) commit(field, e.target.value);
      },
      onKeyDown: (e: React.KeyboardEvent<HTMLInputElement>) => {
        if (!ro && e.key === 'Enter') {
          commit(field, e.currentTarget.value);
          e.currentTarget.blur();
        }
      },
    });

    return (
      <InspectorHeader style={{ flexDirection: 'column', alignItems: 'stretch' }}>
        <InspectorTitle>{name}</InspectorTitle>
        <PositionSizeGrid className="nodrag nopan">
          <FieldLabel>X</FieldLabel>
          <OverlayNumInput
            ref={xRef}
            type="number"
            defaultValue={rx}
            {...handlers('x')}
            disabled={disabled}
            className="nodrag nopan"
          />
          <FieldLabel>Y</FieldLabel>
          <OverlayNumInput
            ref={yRef}
            type="number"
            defaultValue={ry}
            {...handlers('y')}
            disabled={disabled}
            className="nodrag nopan"
          />
          <FieldLabel>W</FieldLabel>
          <OverlayNumInput
            ref={wRef}
            type="number"
            defaultValue={rw}
            {...handlers('w', dimensionsReadOnly)}
            disabled={disabled}
            readOnly={dimensionsReadOnly}
            className="nodrag nopan"
          />
          <FieldLabel>H</FieldLabel>
          <OverlayNumInput
            ref={hRef}
            type="number"
            defaultValue={rh}
            {...handlers('h', dimensionsReadOnly)}
            disabled={disabled}
            readOnly={dimensionsReadOnly}
            className="nodrag nopan"
          />
        </PositionSizeGrid>
        {dimensionsReadOnly && (
          <span style={{ fontSize: 9, color: 'var(--sk-text-muted)', marginTop: 2 }}>
            Auto-sized to text content
          </span>
        )}
      </InspectorHeader>
    );
  }
);
InspectorHeaderSection.displayName = 'InspectorHeaderSection';

export const OpacityControl: React.FC<{
  opacity: number;
  onChange: (value: number) => void;
  disabled: boolean;
  onInteractionStart?: () => void;
  onInteractionEnd?: () => void;
}> = React.memo(({ opacity, onChange, disabled, onInteractionStart, onInteractionEnd }) => {
  return (
    <InspectorSection>
      <InspectorSectionLabel>Opacity</InspectorSectionLabel>
      <ControlRow>
        <CompactSliderRoot
          value={[opacity]}
          onValueChange={([v]) => {
            onChange(v);
          }}
          onPointerDownCapture={onInteractionStart ? () => onInteractionStart() : undefined}
          onValueCommit={onInteractionEnd ? () => onInteractionEnd() : undefined}
          min={0}
          max={1}
          step={0.01}
          disabled={disabled}
          className="nodrag nopan"
        >
          <CompactSliderTrack>
            <CompactSliderRange />
          </CompactSliderTrack>
          <CompactSliderThumb />
        </CompactSliderRoot>
        <ControlValue>{(opacity * 100).toFixed(0)}%</ControlValue>
      </ControlRow>
    </InspectorSection>
  );
});
OpacityControl.displayName = 'OpacityControl';

export const RotationControl: React.FC<{
  rotationDegrees: number;
  onChange: (value: number) => void;
  disabled: boolean;
  onInteractionStart?: () => void;
  onInteractionEnd?: () => void;
}> = React.memo(({ rotationDegrees, onChange, disabled, onInteractionStart, onInteractionEnd }) => {
  const normalisedRotation = ((Math.round(rotationDegrees) % 360) + 360) % 360;

  return (
    <InspectorSection>
      <InspectorSectionLabel>Rotation</InspectorSectionLabel>
      <RotationPresetsRow className="nodrag nopan">
        {ROTATION_PRESETS.map((deg) => (
          <PresetButton
            key={deg}
            isActive={normalisedRotation === deg}
            disabled={disabled}
            onClick={() => {
              onChange(deg);
            }}
          >
            {deg}&deg;
          </PresetButton>
        ))}
        <SKTooltip content="Reset to 0&deg;">
          <ResetButton
            disabled={disabled}
            onClick={() => {
              onChange(0);
            }}
            className="nodrag nopan"
          >
            <RotateCcw size={10} />
          </ResetButton>
        </SKTooltip>
      </RotationPresetsRow>
      <ControlRow>
        <CompactSliderRoot
          value={[normalisedRotation]}
          onValueChange={([v]) => {
            onChange(v);
          }}
          onPointerDownCapture={onInteractionStart ? () => onInteractionStart() : undefined}
          onValueCommit={onInteractionEnd ? () => onInteractionEnd() : undefined}
          min={0}
          max={359}
          step={1}
          disabled={disabled}
          className="nodrag nopan"
        >
          <CompactSliderTrack>
            <CompactSliderRange />
          </CompactSliderTrack>
          <CompactSliderThumb />
        </CompactSliderRoot>
        <ControlValue>{normalisedRotation}&deg;</ControlValue>
      </ControlRow>
    </InspectorSection>
  );
});
RotationControl.displayName = 'RotationControl';

export const MirrorControl: React.FC<{
  mirrorHorizontal: boolean;
  mirrorVertical: boolean;
  onToggle: (axis: 'horizontal' | 'vertical') => void;
  disabled: boolean;
}> = React.memo(({ mirrorHorizontal, mirrorVertical, onToggle, disabled }) => (
  <InspectorSection>
    <InspectorSectionLabel>Mirror</InspectorSectionLabel>
    <MirrorToggleRow className="nodrag nopan">
      <MirrorButton
        isActive={mirrorHorizontal}
        disabled={disabled}
        onClick={() => onToggle('horizontal')}
      >
        <FlipHorizontal2 size={12} /> Horizontal
      </MirrorButton>
      <MirrorButton
        isActive={mirrorVertical}
        disabled={disabled}
        onClick={() => onToggle('vertical')}
      >
        <FlipVertical2 size={12} /> Vertical
      </MirrorButton>
    </MirrorToggleRow>
  </InspectorSection>
));
MirrorControl.displayName = 'MirrorControl';

const CropSectionHeader = styled.div`
  display: flex;
  align-items: center;
  gap: 6px;
`;

export type CropZoomPatch = {
  cropX?: number;
  cropY?: number;
  cropZoom?: number;
  cropShape?: 'rect' | 'circle';
};

export const CropZoomControl: React.FC<{
  cropZoom: number;
  cropX: number;
  cropY: number;
  cropShape: 'rect' | 'circle';
  onChange: (patch: CropZoomPatch) => void;
  disabled: boolean;
  onInteractionStart?: () => void;
  onInteractionEnd?: () => void;
}> = React.memo(
  ({
    cropZoom,
    cropX,
    cropY,
    cropShape,
    onChange,
    disabled,
    onInteractionStart,
    onInteractionEnd,
  }) => {
    const panDisabled = disabled || cropZoom <= 1.0;

    return (
      <InspectorSection data-testid="crop-zoom-section">
        <CropSectionHeader>
          <InspectorSectionLabel>Crop &amp; Zoom</InspectorSectionLabel>
          <SKTooltip content="Reset to defaults">
            <ResetButton
              disabled={disabled}
              onClick={() => onChange({ cropZoom: 1.0, cropX: 0.5, cropY: 0.5, cropShape: 'rect' })}
              className="nodrag nopan"
              data-testid="crop-zoom-reset"
            >
              <RotateCcw size={12} />
            </ResetButton>
          </SKTooltip>
        </CropSectionHeader>
        <ControlRow>
          <ControlLabel>Shape</ControlLabel>
          <MirrorToggleRow className="nodrag nopan" style={{ flex: 1 }}>
            <MirrorButton
              isActive={cropShape === 'rect'}
              disabled={disabled}
              onClick={() => onChange({ cropShape: 'rect' })}
              data-testid="crop-shape-rect"
            >
              ▭ Rect
            </MirrorButton>
            <MirrorButton
              isActive={cropShape === 'circle'}
              disabled={disabled}
              onClick={() => onChange({ cropShape: 'circle' })}
              data-testid="crop-shape-circle"
            >
              ● Circle
            </MirrorButton>
          </MirrorToggleRow>
        </ControlRow>
        <ControlRow>
          <ControlLabel>Zoom</ControlLabel>
          <CompactSliderRoot
            value={[cropZoom]}
            onValueChange={([v]) => onChange({ cropZoom: v })}
            onPointerDownCapture={onInteractionStart ? () => onInteractionStart() : undefined}
            onValueCommit={onInteractionEnd ? () => onInteractionEnd() : undefined}
            min={1}
            max={4}
            step={0.1}
            disabled={disabled}
            className="nodrag nopan"
            data-testid="crop-zoom-slider"
          >
            <CompactSliderTrack>
              <CompactSliderRange />
            </CompactSliderTrack>
            <CompactSliderThumb />
          </CompactSliderRoot>
          <ControlValue data-testid="crop-zoom-value">{cropZoom.toFixed(1)}×</ControlValue>
        </ControlRow>
        <ControlRow>
          <ControlLabel>Pan X</ControlLabel>
          <CompactSliderRoot
            value={[cropX]}
            onValueChange={([v]) => onChange({ cropX: v })}
            onPointerDownCapture={onInteractionStart ? () => onInteractionStart() : undefined}
            onValueCommit={onInteractionEnd ? () => onInteractionEnd() : undefined}
            min={0}
            max={1}
            step={0.01}
            disabled={panDisabled}
            className="nodrag nopan"
            data-testid="crop-pan-x-slider"
          >
            <CompactSliderTrack>
              <CompactSliderRange />
            </CompactSliderTrack>
            <CompactSliderThumb />
          </CompactSliderRoot>
          <ControlValue>{cropX.toFixed(2)}</ControlValue>
        </ControlRow>
        <ControlRow>
          <ControlLabel>Pan Y</ControlLabel>
          <CompactSliderRoot
            value={[cropY]}
            onValueChange={([v]) => onChange({ cropY: v })}
            onPointerDownCapture={onInteractionStart ? () => onInteractionStart() : undefined}
            onValueCommit={onInteractionEnd ? () => onInteractionEnd() : undefined}
            min={0}
            max={1}
            step={0.01}
            disabled={panDisabled}
            className="nodrag nopan"
            data-testid="crop-tilt-y-slider"
          >
            <CompactSliderTrack>
              <CompactSliderRange />
            </CompactSliderTrack>
            <CompactSliderThumb />
          </CompactSliderRoot>
          <ControlValue>{cropY.toFixed(2)}</ControlValue>
        </ControlRow>
      </InspectorSection>
    );
  }
);
CropZoomControl.displayName = 'CropZoomControl';

// ── Unified layer list ──────────────────────────────────────────────────────

export const CompositorEntryList: React.FC<{
  entries: CompositorEntry[];
  selectedLayerId: string | null;
  onSelectLayer: (id: string | null) => void;
  onToggleVisibility: (layerId: string) => void;
  onAddText: (text: string) => void;
  onRemoveText: (id: string) => void;
  onAddImage: (assetPath: string, naturalWidth?: number, naturalHeight?: number) => void;
  onRemoveImage: (id: string) => void;
  onReorderLayers: (entries: Array<{ id: string; kind: LayerKind; zIndex: number }>) => void;
  disabled: boolean;
}> = React.memo(
  ({
    entries,
    selectedLayerId,
    onSelectLayer,
    onToggleVisibility,
    onAddText,
    onRemoveText,
    onAddImage,
    onRemoveImage,
    onReorderLayers,
    disabled,
  }) => {
    const [menuOpen, setMenuOpen] = useState(false);
    const fileInputRef = useRef<HTMLInputElement>(null);
    const menuRef = useRef<HTMLDivElement>(null);

    useEffect(() => {
      if (!menuOpen) return;
      const handleClickOutside = (e: MouseEvent) => {
        if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
          setMenuOpen(false);
        }
      };
      document.addEventListener('pointerdown', handleClickOutside);
      return () => document.removeEventListener('pointerdown', handleClickOutside);
    }, [menuOpen]);

    const handleAddText = useCallback(() => {
      onAddText('Text');
      setMenuOpen(false);
    }, [onAddText]);

    const handleAddImage = useCallback(() => {
      setMenuOpen(false);
      fileInputRef.current?.click();
    }, []);

    const handleImageFileChange = useCallback(
      (e: React.ChangeEvent<HTMLInputElement>) => {
        const file = e.target.files?.[0];
        if (!file) return;
        if (!file.type.startsWith('image/')) return;

        uploadImageAsset(file)
          .then((asset) => {
            onAddImage(asset.path, asset.width, asset.height);
          })
          .catch((err) => {
            logger.error('Failed to upload image asset:', err);
            const msg = err instanceof Error ? err.message : String(err);
            showToast(`Image upload failed: ${msg}`, 'error');
          })
          .finally(() => {
            e.target.value = '';
          });
      },
      [onAddImage]
    );

    return (
      <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
        <HiddenFileInput
          ref={fileInputRef}
          type="file"
          accept="image/png,image/jpeg,image/webp,image/gif,image/svg+xml"
          onChange={handleImageFileChange}
        />

        <LayerInfoRow>
          <ControlLabel style={{ fontWeight: 600 }}>Layers ({entries.length})</ControlLabel>
          <div ref={menuRef} style={{ position: 'relative' }}>
            <AddOverlayButton
              disabled={disabled}
              className="nodrag nopan"
              onClick={() => setMenuOpen((p) => !p)}
            >
              <Plus size={10} /> Add
            </AddOverlayButton>
            {menuOpen && (
              <AddMenu className="nodrag nopan">
                <AddMenuItem onClick={handleAddText}>
                  <Type size={12} /> Text
                </AddMenuItem>
                <AddMenuItem onClick={handleAddImage}>
                  <Image size={12} /> Image
                </AddMenuItem>
              </AddMenu>
            )}
          </div>
        </LayerInfoRow>

        {entries.length === 0 && <NoSelectionText>No layers configured</NoSelectionText>}

        <LayerReorderSection
          entries={entries}
          selectedLayerId={selectedLayerId}
          onSelectLayer={onSelectLayer}
          onToggleVisibility={onToggleVisibility}
          onRemoveText={onRemoveText}
          onRemoveImage={onRemoveImage}
          onReorderLayers={onReorderLayers}
          disabled={disabled}
        />
      </div>
    );
  }
);
CompositorEntryList.displayName = 'CompositorEntryList';
