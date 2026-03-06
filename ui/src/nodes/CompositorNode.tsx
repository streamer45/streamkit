// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import { Eye, EyeOff, GripVertical, Image, Plus, Type, X } from 'lucide-react';
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

// ── Styled components ───────────────────────────────────────────────────────

const CompositorWrapper = styled.div`
  border-top: 1px solid var(--sk-border);
  padding: 8px 6px 4px;
  display: flex;
  flex-direction: column;
  gap: 6px;
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

const LayerControls = styled.div`
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 4px 0;
  border-top: 1px solid var(--sk-border);
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

const SliderInput = styled.input`
  flex: 1;
  pointer-events: auto;
  cursor: pointer;
  min-width: 0;

  &:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }
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

const LayerName = styled.span`
  font-weight: 600;
  color: var(--sk-primary);
`;

const LayerPosition = styled.span`
  font-variant-numeric: tabular-nums;
  color: var(--sk-text-muted);
  font-size: 10px;
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
  padding: 2px 0 2px 20px;
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

/** Convert a hex color string (#rrggbb) + alpha byte → [R, G, B, A] */
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
`;

/** Unified entry representing any layer kind for sorting / display */
interface UnifiedLayerEntry {
  id: string;
  kind: LayerKind;
  label: string;
  zIndex: number;
  visible: boolean;
}

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
              <OverlayIcon>{iconForKind(entry.kind)}</OverlayIcon>
              <OverlayLabel style={{ fontWeight: entry.id === selectedLayerId ? 600 : 400 }}>
                {entry.label}
              </OverlayLabel>
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
              {(entry.kind === 'text' || entry.kind === 'image') && (
                <SKTooltip content="Remove layer">
                  <RemoveButton
                    disabled={disabled}
                    className="nodrag nopan"
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

// ── Shared per-layer property controls ──────────────────────────────────────

/** Shared per-layer property controls: header (name + position), opacity
 *  slider, rotation slider.  Layer-type-specific controls can be injected
 *  in two positions via `children` (between header and sliders, e.g. text
 *  input / font size). */
const LayerPropertyControls: React.FC<{
  name: string;
  x: number;
  y: number;
  opacity: number;
  rotationDegrees: number;
  onOpacityChange: (value: number) => void;
  onRotationChange: (value: number) => void;
  disabled: boolean;
  children?: React.ReactNode;
}> = React.memo(
  ({
    name,
    x,
    y,
    opacity,
    rotationDegrees,
    onOpacityChange,
    onRotationChange,
    disabled,
    children,
  }) => (
    <>
      <LayerInfoRow
        style={{ marginTop: 4, paddingTop: 6, borderTop: '1px solid var(--sk-border)' }}
      >
        <LayerName>{name}</LayerName>
        <LayerPosition>
          ({Math.round(x)}, {Math.round(y)})
        </LayerPosition>
      </LayerInfoRow>

      {children}

      <ControlRow>
        <ControlLabel>Opacity</ControlLabel>
        <SliderInput
          type="range"
          min="0"
          max="1"
          step="0.01"
          value={opacity}
          onChange={(e) => onOpacityChange(Number.parseFloat(e.target.value))}
          disabled={disabled}
          className="nodrag nopan"
        />
        <ControlValue>{(opacity * 100).toFixed(0)}%</ControlValue>
      </ControlRow>

      <ControlRow>
        <ControlLabel>Rotation</ControlLabel>
        <SliderInput
          type="range"
          min="-180"
          max="180"
          step="1"
          value={rotationDegrees}
          onChange={(e) => onRotationChange(Number.parseFloat(e.target.value))}
          disabled={disabled}
          className="nodrag nopan"
        />
        <ControlValue>{rotationDegrees.toFixed(0)}&deg;</ControlValue>
      </ControlRow>
    </>
  )
);
LayerPropertyControls.displayName = 'LayerPropertyControls';

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
      <LayerControls>
        <HiddenFileInput
          ref={fileInputRef}
          type="file"
          accept="image/png,image/jpeg,image/webp,image/gif"
          onChange={handleImageFileChange}
        />

        <LayerInfoRow>
          <ControlLabel style={{ fontWeight: 600 }}>Layers ({entries.length})</ControlLabel>
          <div style={{ position: 'relative' }}>
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
      </LayerControls>
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
      all.push({ id: l.id, kind: 'video', label: l.id, zIndex: l.zIndex, visible: l.visible });
    }
    textOverlays.forEach((o, i) => {
      all.push({
        id: o.id,
        kind: 'text',
        label: `text_${i}`,
        zIndex: o.zIndex,
        visible: o.visible,
      });
    });
    imageOverlays.forEach((o, i) => {
      all.push({
        id: o.id,
        kind: 'image',
        label: `Image #${i}`,
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

  // Structurally-stable entries list — same reference during opacity/rotation
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

  // Memoize callbacks for LayerPropertyControls
  const handleSelectedOpacityChange = useCallback(
    (v: number) => {
      if (selectedLayerId) updateLayerOpacity(selectedLayerId, v);
    },
    [selectedLayerId, updateLayerOpacity]
  );
  const handleSelectedRotationChange = useCallback(
    (v: number) => {
      if (selectedLayerId) updateLayerRotation(selectedLayerId, v);
    },
    [selectedLayerId, updateLayerRotation]
  );

  return (
    <NodeFrame
      id={id}
      label={data.label}
      kind={data.kind}
      selected={selected}
      minWidth={280}
      inputs={data.inputs}
      outputs={data.outputs}
      nodeDefinition={data.nodeDefinition}
      state={data.state}
      sessionId={data.sessionId}
    >
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
              {canvasWidth}x{canvasHeight}
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

        {/* Property controls rendered outside UnifiedLayerList so the list
            stays fully stable during opacity / rotation slider drags. */}
        {selectedLayer && (
          <LayerPropertyControls
            name={selectedLayer.id}
            x={selectedLayer.x}
            y={selectedLayer.y}
            opacity={selectedLayer.opacity}
            rotationDegrees={selectedLayer.rotationDegrees}
            onOpacityChange={handleSelectedOpacityChange}
            onRotationChange={handleSelectedRotationChange}
            disabled={disabled}
          />
        )}

        {selectedTextOverlay && (
          <LayerPropertyControls
            name="Text"
            x={selectedTextOverlay.x}
            y={selectedTextOverlay.y}
            opacity={selectedTextOverlay.opacity}
            rotationDegrees={selectedTextOverlay.rotationDegrees}
            onOpacityChange={(v) => updateTextOverlay(selectedTextOverlay.id, { opacity: v })}
            onRotationChange={(v) =>
              updateTextOverlay(selectedTextOverlay.id, { rotationDegrees: v })
            }
            disabled={disabled}
          >
            <OverlayEditRow style={{ paddingLeft: 0 }}>
              <OverlayTextInput
                value={selectedTextOverlay.text}
                onChange={(e) =>
                  updateTextOverlay(selectedTextOverlay.id, { text: e.target.value })
                }
                placeholder="Text content"
                disabled={disabled}
                className="nodrag nopan"
              />
            </OverlayEditRow>
            <OverlayEditRow style={{ paddingLeft: 0 }}>
              <span style={{ color: 'var(--sk-text-muted)' }}>Size</span>
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
            <OverlayEditRow style={{ paddingLeft: 0 }}>
              <span style={{ color: 'var(--sk-text-muted)' }}>Font</span>
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
            <OverlayEditRow style={{ paddingLeft: 0 }}>
              <span style={{ color: 'var(--sk-text-muted)' }}>Color</span>
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
          </LayerPropertyControls>
        )}

        {selectedImageOverlay && (
          <LayerPropertyControls
            name={`Image #${imageOverlays.indexOf(selectedImageOverlay)}`}
            x={selectedImageOverlay.x}
            y={selectedImageOverlay.y}
            opacity={selectedImageOverlay.opacity}
            rotationDegrees={selectedImageOverlay.rotationDegrees}
            onOpacityChange={(v) => updateImageOverlay(selectedImageOverlay.id, { opacity: v })}
            onRotationChange={(v) =>
              updateImageOverlay(selectedImageOverlay.id, { rotationDegrees: v })
            }
            disabled={disabled}
          />
        )}
      </CompositorWrapper>
    </NodeFrame>
  );
});

CompositorNode.displayName = 'CompositorNode';

export default CompositorNode;
