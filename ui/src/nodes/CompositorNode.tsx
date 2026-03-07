// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import * as RadixSlider from '@radix-ui/react-slider';
import { Eye, EyeOff, GripVertical, Image, Plus, RotateCcw, Type, X } from 'lucide-react';
import { Reorder } from 'motion/react';
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { CompositorCanvas } from '@/components/CompositorCanvas';
import { NodeFrame } from '@/components/node/NodeFrame';
import { SKTooltip } from '@/components/Tooltip';
import { LiveBadge, LiveDot } from '@/components/ui/LiveIndicator';
import { useCompositorLayers } from '@/hooks/useCompositorLayers';
import type { TextOverlayState, ImageOverlayState, LayerKind } from '@/hooks/useCompositorLayers';
import { setCompositorSelection } from '@/hooks/useCompositorSelection';
import type { InputPin, OutputPin, NodeState, NodeStats, NodeDefinition } from '@/types/types';
import { nodesLogger } from '@/utils/logger';

// ── Friendly label helpers ──────────────────────────────────────────────────

/** Convert technical layer IDs to user-friendly labels.
 *  e.g. "in_0" -> "Input 0", "text_1" -> "Text 1" */
function friendlyLabel(id: string, kind: LayerKind, index?: number): string {
  switch (kind) {
    case 'video': {
      const match = id.match(/(\d+)$/);
      return `Input ${match ? match[1] : (index ?? 0)}`;
    }
    case 'text':
      return `Text ${index ?? 0}`;
    case 'image':
      return `Image ${index ?? 0}`;
    default:
      return id;
  }
}

// ── Styled components ───────────────────────────────────────────────────────

/** Outer wrapper that positions the node body and the side inspector panel */
const CompositorOuterWrapper = styled.div`
  position: relative;
  display: flex;
  align-items: flex-start;
`;

const CompositorWrapper = styled.div`
  border-top: 1px solid var(--sk-border);
  padding: 8px 6px 4px;
  display: flex;
  flex-direction: column;
  gap: 6px;
  flex: 1;
  min-width: 0;
`;

const CanvasSection = styled.div`
  display: flex;
  flex-direction: column;
  gap: 4px;
`;

const CanvasHeader = styled.div`
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 11px;
`;

const CanvasLabel = styled.span`
  color: var(--sk-text-muted);
`;

const ResolutionLabel = styled.span`
  font-variant-numeric: tabular-nums;
  color: var(--sk-text-muted);
  font-size: 10px;
`;

const ControlRow = styled.div`
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 11px;
`;

const ControlLabel = styled.span`
  color: var(--sk-text-muted);
  min-width: 52px;
  flex-shrink: 0;
`;

const ControlValue = styled.span`
  font-variant-numeric: tabular-nums;
  color: var(--sk-text);
  min-width: 36px;
  text-align: right;
  flex-shrink: 0;
`;

const NumericInput = styled.input`
  width: 44px;
  padding: 3px 4px;
  font-size: 11px;
  font-variant-numeric: tabular-nums;
  text-align: center;
  border: 1px solid var(--sk-border);
  border-radius: 4px;
  background: var(--sk-input-bg);
  color: var(--sk-text);
  outline: none;
  pointer-events: auto;
  transition: border-color 0.15s;

  &:focus {
    border-color: var(--sk-primary);
    box-shadow: 0 0 0 1px var(--sk-primary);
  }

  &:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }

  /* Hide spinners */
  &::-webkit-inner-spin-button,
  &::-webkit-outer-spin-button {
    -webkit-appearance: none;
    margin: 0;
  }
  -moz-appearance: textfield;
`;

const LayerInfoRow = styled.div`
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 11px;
  padding: 2px 0;
`;

const NoSelectionText = styled.div`
  font-size: 11px;
  color: var(--sk-text-muted);
  text-align: center;
  padding: 4px 0;
`;

// ── Overlay management styled components ────────────────────────────────────

const AddOverlayButton = styled.button`
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 6px;
  border: 1px solid var(--sk-border);
  border-radius: 3px;
  background: var(--sk-input-bg);
  color: var(--sk-text-muted);
  cursor: pointer;
  font-size: 10px;
  pointer-events: auto;

  &:hover:not(:disabled) {
    background: var(--sk-overlay-medium);
    color: var(--sk-text);
    border-color: var(--sk-primary);
  }

  &:disabled {
    cursor: not-allowed;
    opacity: 0.4;
  }
`;

const AddMenu = styled.div`
  position: absolute;
  right: 0;
  top: 100%;
  margin-top: 2px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 4px;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.15);
  z-index: 10;
  min-width: 120px;
  overflow: hidden;
`;

const AddMenuItem = styled.button`
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 6px 10px;
  border: none;
  background: none;
  color: var(--sk-text);
  cursor: pointer;
  font-size: 11px;
  text-align: left;
  pointer-events: auto;

  &:hover {
    background: var(--sk-overlay-medium);
  }
`;

const OverlayLabel = styled.span`
  flex: 1;
  color: var(--sk-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
`;

const OverlayIcon = styled.span`
  display: inline-flex;
  align-items: center;
  color: var(--sk-text-muted);
  flex-shrink: 0;
`;

const RemoveButton = styled.button`
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  padding: 0;
  border: none;
  border-radius: 2px;
  background: none;
  color: var(--sk-text-muted);
  cursor: pointer;
  pointer-events: auto;
  flex-shrink: 0;
  opacity: 0;
  transition: opacity 0.15s;

  &:hover {
    background: var(--sk-danger-alpha, rgba(220, 38, 38, 0.15));
    color: var(--sk-danger);
  }
`;

const HiddenFileInput = styled.input`
  display: none;
`;

const OverlayEditRow = styled.div`
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 2px 0;
  font-size: 10px;
`;

const OverlayTextInput = styled.textarea`
  flex: 1;
  padding: 2px 4px;
  font-size: 11px;
  border: 1px solid var(--sk-border);
  border-radius: 3px;
  background: var(--sk-input-bg);
  color: var(--sk-text);
  outline: none;
  min-width: 0;
  pointer-events: auto;
  resize: vertical;
  min-height: 22px;
  max-height: 80px;
  font-family: inherit;
  line-height: 1.3;
  white-space: pre-wrap;
  word-break: break-word;

  &:focus {
    border-color: var(--sk-primary);
  }
`;

const OverlayNumInput = styled(NumericInput)`
  width: 40px;
  font-size: 10px;
`;

const ColorInput = styled.input`
  width: 28px;
  height: 22px;
  padding: 0;
  border: 1px solid var(--sk-border);
  border-radius: 3px;
  background: none;
  cursor: pointer;
  pointer-events: auto;
  flex-shrink: 0;

  &::-webkit-color-swatch-wrapper {
    padding: 1px;
  }
  &::-webkit-color-swatch {
    border: none;
    border-radius: 2px;
  }
`;

/** Convert [R, G, B, A] to a hex color string (#rrggbb) for <input type="color"> */
function rgbaToHex(color: [number, number, number, number]): string {
  const [r, g, b] = color;
  return `#${r.toString(16).padStart(2, '0')}${g.toString(16).padStart(2, '0')}${b.toString(16).padStart(2, '0')}`;
}

/** Convert a hex color string (#rrggbb) + alpha byte -> [R, G, B, A] */
function hexToRgba(hex: string, alpha: number): [number, number, number, number] {
  const r = Number.parseInt(hex.slice(1, 3), 16);
  const g = Number.parseInt(hex.slice(3, 5), 16);
  const b = Number.parseInt(hex.slice(5, 7), 16);
  return [r, g, b, alpha];
}

/** Available named fonts matching the server's bundled font set. */
const FONT_OPTIONS = [
  { value: 'dejavu-sans', label: 'DejaVu Sans' },
  { value: 'dejavu-serif', label: 'DejaVu Serif' },
  { value: 'dejavu-sans-mono', label: 'DejaVu Sans Mono' },
  { value: 'dejavu-sans-bold', label: 'DejaVu Sans Bold' },
  { value: 'dejavu-serif-bold', label: 'DejaVu Serif Bold' },
  { value: 'dejavu-sans-mono-bold', label: 'DejaVu Mono Bold' },
] as const;

const FontSelect = styled.select`
  flex: 1;
  padding: 2px 4px;
  font-size: 10px;
  border: 1px solid var(--sk-border);
  border-radius: 3px;
  background: var(--sk-panel-bg);
  color: var(--sk-text);
  color-scheme: dark light;
  outline: none;
  min-width: 0;
  pointer-events: auto;
  cursor: pointer;

  option {
    background: var(--sk-panel-bg);
    color: var(--sk-text);
  }

  &:focus {
    border-color: var(--sk-primary);
  }

  &:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }
`;

const VisibilityButton = styled.button`
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  padding: 0;
  border: none;
  border-radius: 2px;
  background: none;
  color: var(--sk-text-muted);
  cursor: pointer;
  pointer-events: auto;
  flex-shrink: 0;

  &:hover {
    color: var(--sk-text);
  }
`;

const LayerListItem = styled.div<{ isSelected?: boolean; isHidden?: boolean }>`
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 3px 4px;
  font-size: 11px;
  border-radius: 3px;
  cursor: pointer;
  pointer-events: auto;
  opacity: ${(p) => (p.isHidden ? 0.45 : 1)};
  background: ${(p) => (p.isSelected ? 'var(--sk-overlay-medium)' : 'transparent')};
  border: 1px solid ${(p) => (p.isSelected ? 'var(--sk-primary)' : 'transparent')};

  &:hover {
    background: var(--sk-overlay-medium);
  }

  /* Show remove button on hover */
  &:hover .layer-remove-btn {
    opacity: 1;
  }
`;

/** Unified entry representing any layer kind for sorting / display */
interface UnifiedLayerEntry {
  id: string;
  kind: LayerKind;
  label: string;
  zIndex: number;
  visible: boolean;
}

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

// ── Rotation presets ────────────────────────────────────────────────────────

const ROTATION_PRESETS = [0, 90, 180, 270] as const;

const RotationPresetsRow = styled.div`
  display: flex;
  align-items: center;
  gap: 3px;
`;

const PresetButton = styled.button<{ isActive?: boolean }>`
  padding: 2px 6px;
  font-size: 10px;
  font-variant-numeric: tabular-nums;
  border: 1px solid ${(p) => (p.isActive ? 'var(--sk-primary)' : 'var(--sk-border)')};
  border-radius: 3px;
  background: ${(p) => (p.isActive ? 'var(--sk-overlay-medium)' : 'transparent')};
  color: ${(p) => (p.isActive ? 'var(--sk-primary)' : 'var(--sk-text-muted)')};
  cursor: pointer;
  pointer-events: auto;
  transition: all 0.1s;
  flex: 1;

  &:hover:not(:disabled) {
    border-color: var(--sk-primary);
    color: var(--sk-text);
  }

  &:disabled {
    cursor: not-allowed;
    opacity: 0.4;
  }
`;

const ResetButton = styled.button`
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  padding: 0;
  border: 1px solid var(--sk-border);
  border-radius: 3px;
  background: transparent;
  color: var(--sk-text-muted);
  cursor: pointer;
  pointer-events: auto;
  flex-shrink: 0;
  transition: all 0.1s;

  &:hover:not(:disabled) {
    border-color: var(--sk-primary);
    color: var(--sk-text);
  }

  &:disabled {
    cursor: not-allowed;
    opacity: 0.4;
  }
`;

// ── Side Inspector Panel ────────────────────────────────────────────────────

const SidePanel = styled.div`
  position: absolute;
  left: 100%;
  top: 0;
  width: 280px;
  margin-left: 8px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  padding: 8px;
  display: flex;
  flex-direction: column;
  gap: 0;
  box-shadow: 0 2px 8px var(--sk-shadow, rgba(0, 0, 0, 0.15));
  pointer-events: auto;
  z-index: 5;
`;

const SidePanelDivider = styled.div`
  height: 1px;
  background: var(--sk-border);
  margin: 8px 0;
`;

const InspectorControls = styled.div`
  display: flex;
  flex-direction: column;
  gap: 8px;
`;

const InspectorHeader = styled.div`
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding-bottom: 4px;
  border-bottom: 1px solid var(--sk-border);
`;

const InspectorTitle = styled.span`
  font-size: 11px;
  font-weight: 600;
  color: var(--sk-primary);
`;

const InspectorPosition = styled.span`
  font-variant-numeric: tabular-nums;
  color: var(--sk-text-muted);
  font-size: 10px;
`;

const InspectorSection = styled.div`
  display: flex;
  flex-direction: column;
  gap: 4px;
`;

const InspectorSectionLabel = styled.div`
  font-size: 10px;
  font-weight: 600;
  color: var(--sk-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.3px;
`;

// ── Reorder section (memoised to avoid cascade during opacity/rotation drags) ─

const LayerReorderSection: React.FC<{
  entries: UnifiedLayerEntry[];
  selectedLayerId: string | null;
  onSelectLayer: (id: string | null) => void;
  onToggleVisibility: (id: string) => void;
  onRemoveText: (id: string) => void;
  onRemoveImage: (id: string) => void;
  onReorderLayers: (entries: Array<{ id: string; kind: LayerKind; zIndex: number }>) => void;
  disabled: boolean;
}> = React.memo(
  ({
    entries,
    selectedLayerId,
    onSelectLayer,
    onToggleVisibility,
    onRemoveText,
    onRemoveImage,
    onReorderLayers,
    disabled,
  }) => {
    const iconForKind = (kind: LayerKind) => {
      switch (kind) {
        case 'text':
          return <Type size={11} />;
        case 'image':
          return <Image size={11} />;
        default:
          return null;
      }
    };

    const handleReorder = useCallback(
      (reordered: UnifiedLayerEntry[]) => {
        const maxZ = reordered.length - 1;
        const updates: Array<{ id: string; kind: LayerKind; zIndex: number }> = [];
        for (let i = 0; i < reordered.length; i++) {
          const entry = reordered[i];
          const newZ = maxZ - i;
          if (entry.zIndex !== newZ) {
            updates.push({ id: entry.id, kind: entry.kind, zIndex: newZ });
          }
        }
        if (updates.length > 0) onReorderLayers(updates);
      },
      [onReorderLayers]
    );

    return (
      <Reorder.Group
        axis="y"
        values={entries}
        onReorder={handleReorder}
        as="div"
        style={{ listStyle: 'none', padding: 0, margin: 0 }}
      >
        {entries.map((entry) => (
          <Reorder.Item
            key={entry.id}
            value={entry}
            as="div"
            style={{ listStyle: 'none' }}
            dragListener={!disabled}
          >
            <LayerListItem
              isSelected={entry.id === selectedLayerId}
              isHidden={!entry.visible}
              className="nodrag nopan"
              onClick={() => onSelectLayer(entry.id === selectedLayerId ? null : entry.id)}
            >
              <GripVertical
                size={11}
                style={{
                  color: 'var(--sk-text-muted)',
                  cursor: disabled ? 'not-allowed' : 'grab',
                  flexShrink: 0,
                  opacity: 0.5,
                }}
              />
              <SKTooltip content={entry.visible ? 'Hide layer' : 'Show layer'}>
                <VisibilityButton
                  className="nodrag nopan"
                  onClick={(e) => {
                    e.stopPropagation();
                    onToggleVisibility(entry.id);
                  }}
                >
                  {entry.visible ? <Eye size={12} /> : <EyeOff size={12} />}
                </VisibilityButton>
              </SKTooltip>
              <OverlayIcon>{iconForKind(entry.kind)}</OverlayIcon>
              <OverlayLabel style={{ fontWeight: entry.id === selectedLayerId ? 600 : 400 }}>
                {entry.label}
              </OverlayLabel>
              {(entry.kind === 'text' || entry.kind === 'image') && (
                <SKTooltip content="Remove layer">
                  <RemoveButton
                    disabled={disabled}
                    className="nodrag nopan layer-remove-btn"
                    onClick={(e) => {
                      e.stopPropagation();
                      if (entry.kind === 'text') onRemoveText(entry.id);
                      else onRemoveImage(entry.id);
                    }}
                  >
                    <X size={12} />
                  </RemoveButton>
                </SKTooltip>
              )}
            </LayerListItem>
          </Reorder.Item>
        ))}
      </Reorder.Group>
    );
  }
);
LayerReorderSection.displayName = 'LayerReorderSection';

// ── Node data interface ─────────────────────────────────────────────────────

interface CompositorNodeData {
  label: string;
  kind: string;
  params: Record<string, unknown>;
  inputs: InputPin[];
  outputs: OutputPin[];
  nodeDefinition?: NodeDefinition;
  state?: NodeState;
  stats?: NodeStats;
  definition?: { bidirectional?: boolean };
  onParamChange?: (nodeId: string, paramName: string, value: unknown) => void;
  onConfigChange?: (nodeId: string, config: Record<string, unknown>) => void;
  sessionId?: string;
  isStaged?: boolean;
}

interface CompositorNodeProps {
  id: string;
  data: CompositorNodeData;
  selected?: boolean;
}

// ── Side inspector property controls ────────────────────────────────────────

/** Inspector controls for the selected layer's properties.
 *  Rendered inside the SidePanel below the layer list. */
// ── Memoized inspector sub-sections ─────────────────────────────────────────
//
// Each section is individually memoized so that during slider drags only the
// section whose value is changing re-renders.  The inactive section, preset
// buttons, tooltips, and children all bail out via React.memo.

const InspectorHeaderSection: React.FC<{
  name: string;
  x: number;
  y: number;
}> = React.memo(({ name, x, y }) => (
  <InspectorHeader>
    <InspectorTitle>{name}</InspectorTitle>
    <InspectorPosition>
      ({Math.round(x)}, {Math.round(y)})
    </InspectorPosition>
  </InspectorHeader>
));
InspectorHeaderSection.displayName = 'InspectorHeaderSection';

const OpacityControl: React.FC<{
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

const RotationControl: React.FC<{
  rotationDegrees: number;
  onChange: (value: number) => void;
  disabled: boolean;
}> = React.memo(({ rotationDegrees, onChange, disabled }) => {
  /** Normalise rotation to the 0..359 range so preset matching works
   *  regardless of whether the backend stores -180..180 or 0..360. */
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

// ── Unified layer list ──────────────────────────────────────────────────────
//
// Receives only stable props (entries + callbacks) so React.memo bails out
// during opacity / rotation slider drags.  The property-controls section
// that actually needs the changing values is rendered by CompositorNode
// directly (outside this component).

const UnifiedLayerList: React.FC<{
  entries: UnifiedLayerEntry[];
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
    // Add overlay menu state
    const [menuOpen, setMenuOpen] = useState(false);
    const fileInputRef = useRef<HTMLInputElement>(null);
    const menuRef = useRef<HTMLDivElement>(null);

    // Close menu on outside click
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
          // Detect natural dimensions so the initial rect preserves
          // the source aspect ratio.
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
UnifiedLayerList.displayName = 'UnifiedLayerList';

// ── Main compositor node ────────────────────────────────────────────────────

/** Build a structurally-stable unified entry list from the three layer
 *  sources.  Returns the previous array reference when the derived entries
 *  haven't changed, which lets downstream React.memo components bail out
 *  during opacity / rotation drags (those fields are not in entries). */
function useStableEntries(
  layers: { id: string; zIndex: number; visible: boolean }[],
  textOverlays: TextOverlayState[],
  imageOverlays: ImageOverlayState[]
): UnifiedLayerEntry[] {
  const prevRef = useRef<UnifiedLayerEntry[]>([]);
  return useMemo(() => {
    const all: UnifiedLayerEntry[] = [];
    for (const l of layers) {
      all.push({
        id: l.id,
        kind: 'video',
        label: friendlyLabel(l.id, 'video'),
        zIndex: l.zIndex,
        visible: l.visible,
      });
    }
    textOverlays.forEach((o, i) => {
      all.push({
        id: o.id,
        kind: 'text',
        label: friendlyLabel(o.id, 'text', i),
        zIndex: o.zIndex,
        visible: o.visible,
      });
    });
    imageOverlays.forEach((o, i) => {
      all.push({
        id: o.id,
        kind: 'image',
        label: friendlyLabel(o.id, 'image', i),
        zIndex: o.zIndex,
        visible: o.visible,
      });
    });
    all.sort((a, b) => b.zIndex - a.zIndex);

    const prev = prevRef.current;
    if (
      prev.length === all.length &&
      prev.every(
        (p, i) =>
          p.id === all[i].id &&
          p.kind === all[i].kind &&
          p.zIndex === all[i].zIndex &&
          p.visible === all[i].visible
      )
    ) {
      return prev;
    }
    prevRef.current = all;
    return all;
  }, [layers, textOverlays, imageOverlays]);
}

const CompositorNode: React.FC<CompositorNodeProps> = React.memo(({ id, data, selected }) => {
  nodesLogger.debug('CompositorNode Render:', id);

  const canvasWidth = (data.params?.width as number) ?? 1280;
  const canvasHeight = (data.params?.height as number) ?? 720;

  const {
    layers,
    selectedLayerId,
    selectLayer,
    handleLayerPointerDown,
    handleResizePointerDown,
    updateLayerOpacity,
    updateLayerRotation,
    toggleLayerVisibility,
    layerRefs,
    textOverlays,
    imageOverlays,
    addTextOverlay,
    updateTextOverlay,
    removeTextOverlay,
    addImageOverlay,
    updateImageOverlay,
    removeImageOverlay,
    reorderLayers,
  } = useCompositorLayers({
    nodeId: id,
    sessionId: data.sessionId,
    canvasWidth,
    canvasHeight,
    params: data.params ?? {},
    onConfigChange: data.onConfigChange,
    onParamChange: data.onParamChange,
    isStaged: data.isStaged,
  });

  const disabled = !data.onConfigChange && !data.onParamChange;

  // Structurally-stable entries list -- same reference during opacity/rotation
  // drags so UnifiedLayerList's React.memo bails out.
  const entries = useStableEntries(layers, textOverlays, imageOverlays);

  // Broadcast compositor layer selection for YAML highlighting
  useEffect(() => {
    setCompositorSelection(selected ? data.label : null, selectedLayerId);
    return () => setCompositorSelection(null, null);
  }, [selected, data.label, selectedLayerId]);

  // Show live indicator when node is in an active session and is not staged
  const showLiveIndicator = !data.isStaged && !!data.onConfigChange && !!data.sessionId;

  // Selected layer data for property controls
  const selectedLayer = layers.find((l) => l.id === selectedLayerId);
  const selectedTextOverlay = textOverlays.find((o) => o.id === selectedLayerId);
  const selectedImageOverlay = imageOverlays.find((o) => o.id === selectedLayerId);
  // Determine the kind of the selected layer once — this only changes when
  // selection changes or layers are added/removed, NOT on every slider tick.
  const selectedLayerKind = useMemo(() => {
    if (layers.some((l) => l.id === selectedLayerId)) return 'video' as const;
    if (textOverlays.some((o) => o.id === selectedLayerId)) return 'text' as const;
    if (imageOverlays.some((o) => o.id === selectedLayerId)) return 'image' as const;
    return null;
  }, [selectedLayerId, layers, textOverlays, imageOverlays]);

  // Memoize callbacks for LayerInspector — stable references prevent
  // React.memo on LayerInspector from being defeated during slider drags.
  // These depend on selectedLayerKind (a string) instead of the full arrays,
  // so they stay stable during opacity/rotation changes.
  const handleSelectedOpacityChange = useCallback(
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
  const handleSelectedRotationChange = useCallback(
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

  // Derive friendly name for selected layer in inspector
  const selectedLayerName = useMemo(() => {
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

  // Memoize text overlay children so the children prop doesn't defeat
  // React.memo on LayerInspector during opacity/rotation slider drags.
  const textInspectorChildren = useMemo(() => {
    if (!selectedTextOverlay) return null;
    return (
      <>
        <InspectorSection>
          <InspectorSectionLabel>Content</InspectorSectionLabel>
          <OverlayEditRow>
            <OverlayTextInput
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

  // Selected layer props for the inspector (stable object during same selection)
  const inspectorProps = useMemo(() => {
    if (selectedLayer)
      return {
        x: selectedLayer.x,
        y: selectedLayer.y,
        opacity: selectedLayer.opacity,
        rotationDegrees: selectedLayer.rotationDegrees,
      };
    if (selectedTextOverlay)
      return {
        x: selectedTextOverlay.x,
        y: selectedTextOverlay.y,
        opacity: selectedTextOverlay.opacity,
        rotationDegrees: selectedTextOverlay.rotationDegrees,
      };
    if (selectedImageOverlay)
      return {
        x: selectedImageOverlay.x,
        y: selectedImageOverlay.y,
        opacity: selectedImageOverlay.opacity,
        rotationDegrees: selectedImageOverlay.rotationDegrees,
      };
    return null;
  }, [selectedLayer, selectedTextOverlay, selectedImageOverlay]);

  return (
    <NodeFrame
      id={id}
      label={data.label}
      kind={data.kind}
      selected={selected}
      minWidth={320}
      inputs={data.inputs}
      outputs={data.outputs}
      nodeDefinition={data.nodeDefinition}
      state={data.state}
      sessionId={data.sessionId}
    >
      <CompositorOuterWrapper>
        <CompositorWrapper>
          <CanvasSection>
            <CanvasHeader>
              <CanvasLabel>
                Compositor
                {showLiveIndicator && (
                  <SKTooltip content="Layer changes apply immediately to the running pipeline">
                    <LiveBadge size="small" style={{ marginLeft: 6 }}>
                      <LiveDot size="small" />
                      LIVE
                    </LiveBadge>
                  </SKTooltip>
                )}
              </CanvasLabel>
              <ResolutionLabel>
                {canvasWidth}&times;{canvasHeight}
              </ResolutionLabel>
            </CanvasHeader>

            <CompositorCanvas
              canvasWidth={canvasWidth}
              canvasHeight={canvasHeight}
              layers={layers}
              textOverlays={textOverlays}
              imageOverlays={imageOverlays}
              selectedLayerId={selectedLayerId}
              onSelectLayer={selectLayer}
              onLayerPointerDown={handleLayerPointerDown}
              onResizePointerDown={handleResizePointerDown}
              onTextEdit={disabled ? undefined : updateTextOverlay}
              layerRefs={layerRefs}
              disabled={disabled}
            />
          </CanvasSection>
        </CompositorWrapper>

        {/* Side panel: layer list (always) + inspector controls (when selected) */}
        <SidePanel className="nodrag nopan">
          <UnifiedLayerList
            entries={entries}
            selectedLayerId={selectedLayerId}
            onSelectLayer={selectLayer}
            onToggleVisibility={toggleLayerVisibility}
            onAddText={addTextOverlay}
            onRemoveText={removeTextOverlay}
            onAddImage={addImageOverlay}
            onRemoveImage={removeImageOverlay}
            onReorderLayers={reorderLayers}
            disabled={disabled}
          />

          {inspectorProps && (
            <>
              <SidePanelDivider />
              <InspectorControls>
                <InspectorHeaderSection
                  name={selectedLayerName}
                  x={inspectorProps.x}
                  y={inspectorProps.y}
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
              </InspectorControls>
            </>
          )}
        </SidePanel>
      </CompositorOuterWrapper>
    </NodeFrame>
  );
});

CompositorNode.displayName = 'CompositorNode';

export default CompositorNode;
