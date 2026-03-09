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
import * as RadixSlider from '@radix-ui/react-slider';
import { FlipHorizontal2, FlipVertical2, Image, Plus, RotateCcw, Type } from 'lucide-react';
import React, { useCallback, useEffect, useRef, useState } from 'react';

import { SKTooltip } from '@/components/Tooltip';
import type { LayerKind } from '@/hooks/useCompositorLayers';

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

// ── Radix Slider (compact) ──────────────────────────────────────────────────

const CompactSliderRoot = styled(RadixSlider.Root)`
  position: relative;
  display: flex;
  align-items: center;
  user-select: none;
  touch-action: none;
  flex: 1;
  height: 16px;
  min-width: 0;

  &[data-disabled] {
    opacity: 0.5;
    cursor: not-allowed;
  }
`;

const CompactSliderTrack = styled(RadixSlider.Track)`
  position: relative;
  flex-grow: 1;
  height: 3px;
  background: var(--sk-border);
  border-radius: 9999px;
`;

const CompactSliderRange = styled(RadixSlider.Range)`
  position: absolute;
  height: 100%;
  background: var(--sk-primary);
  border-radius: 9999px;
`;

const CompactSliderThumb = styled(RadixSlider.Thumb)`
  display: block;
  width: 12px;
  height: 12px;
  background: var(--sk-panel-bg);
  border: 2px solid var(--sk-primary);
  border-radius: 50%;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.2);
  cursor: grab;
  transition: border-color 0.1s ease;

  &:hover {
    border-color: var(--sk-primary-hover, var(--sk-primary));
  }

  &:focus-visible {
    outline: none;
    box-shadow: var(--sk-focus-ring, 0 0 0 3px rgba(14, 165, 233, 0.2));
  }

  &:active {
    cursor: grabbing;
  }

  &[data-disabled] {
    cursor: not-allowed;
  }
`;

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
}> = React.memo(({ name, x, y, width, height, onPositionSizeChange, disabled }) => (
  <InspectorHeader style={{ flexDirection: 'column', alignItems: 'stretch' }}>
    <InspectorTitle>{name}</InspectorTitle>
    <PositionSizeGrid className="nodrag nopan">
      <FieldLabel>X</FieldLabel>
      <OverlayNumInput
        type="number"
        value={Math.round(x)}
        onChange={(e) => {
          const v = Number.parseInt(e.target.value, 10);
          if (!Number.isNaN(v)) onPositionSizeChange({ x: v });
        }}
        disabled={disabled}
        className="nodrag nopan"
      />
      <FieldLabel>Y</FieldLabel>
      <OverlayNumInput
        type="number"
        value={Math.round(y)}
        onChange={(e) => {
          const v = Number.parseInt(e.target.value, 10);
          if (!Number.isNaN(v)) onPositionSizeChange({ y: v });
        }}
        disabled={disabled}
        className="nodrag nopan"
      />
      <FieldLabel>W</FieldLabel>
      <OverlayNumInput
        type="number"
        value={Math.round(width)}
        onChange={(e) => {
          const v = Number.parseInt(e.target.value, 10);
          if (!Number.isNaN(v) && v > 0) onPositionSizeChange({ width: v });
        }}
        disabled={disabled}
        className="nodrag nopan"
      />
      <FieldLabel>H</FieldLabel>
      <OverlayNumInput
        type="number"
        value={Math.round(height)}
        onChange={(e) => {
          const v = Number.parseInt(e.target.value, 10);
          if (!Number.isNaN(v) && v > 0) onPositionSizeChange({ height: v });
        }}
        disabled={disabled}
        className="nodrag nopan"
      />
    </PositionSizeGrid>
  </InspectorHeader>
));
InspectorHeaderSection.displayName = 'InspectorHeaderSection';

export const OpacityControl: React.FC<{
  opacity: number;
  onChange: (value: number) => void;
  disabled: boolean;
}> = React.memo(({ opacity, onChange, disabled }) => (
  <InspectorSection>
    <InspectorSectionLabel>Opacity</InspectorSectionLabel>
    <ControlRow>
      <CompactSliderRoot
        value={[opacity]}
        onValueChange={([v]) => onChange(v)}
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
));
OpacityControl.displayName = 'OpacityControl';

export const RotationControl: React.FC<{
  rotationDegrees: number;
  onChange: (value: number) => void;
  disabled: boolean;
}> = React.memo(({ rotationDegrees, onChange, disabled }) => {
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
            onClick={() => onChange(deg)}
          >
            {deg}&deg;
          </PresetButton>
        ))}
        <SKTooltip content="Reset to 0&deg;">
          <ResetButton disabled={disabled} onClick={() => onChange(0)} className="nodrag nopan">
            <RotateCcw size={10} />
          </ResetButton>
        </SKTooltip>
      </RotationPresetsRow>
      <ControlRow>
        <CompactSliderRoot
          value={[normalisedRotation]}
          onValueChange={([v]) => onChange(v)}
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

// ── Unified layer list ──────────────────────────────────────────────────────

export const CompositorEntryList: React.FC<{
  entries: CompositorEntry[];
  selectedLayerId: string | null;
  onSelectLayer: (id: string | null) => void;
  onToggleVisibility: (layerId: string) => void;
  onAddText: (text: string) => void;
  onRemoveText: (id: string) => void;
  onAddImage: (dataBase64: string, naturalWidth?: number, naturalHeight?: number) => void;
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

        const reader = new FileReader();
        reader.onload = () => {
          const result = reader.result as string;
          const base64 = result.split(',')[1];
          if (!base64) return;
          const img = new window.Image();
          img.onload = () => onAddImage(base64, img.naturalWidth, img.naturalHeight);
          img.onerror = () => onAddImage(base64);
          img.src = result;
        };
        reader.readAsDataURL(file);
        e.target.value = '';
      },
      [onAddImage]
    );

    return (
      <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
        <HiddenFileInput
          ref={fileInputRef}
          type="file"
          accept="image/png,image/jpeg,image/webp,image/gif"
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
