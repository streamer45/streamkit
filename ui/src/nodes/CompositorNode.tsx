// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import { Eye, EyeOff, Image, Plus, Type, X } from 'lucide-react';
import React, { useCallback, useEffect, useRef, useState } from 'react';

import { CompositorCanvas } from '@/components/CompositorCanvas';
import { NodeFrame } from '@/components/node/NodeFrame';
import { SKTooltip } from '@/components/Tooltip';
import { useCompositorLayers } from '@/hooks/useCompositorLayers';
import type { TextOverlayState, ImageOverlayState, LayerKind } from '@/hooks/useCompositorLayers';
import { setCompositorSelection } from '@/hooks/useCompositorSelection';
import type { InputPin, OutputPin, NodeState, NodeStats, NodeDefinition } from '@/types/types';
import { nodesLogger } from '@/utils/logger';

// ── Styled components ───────────────────────────────────────────────────────

const CompositorWrapper = styled.div`
  border-top: 1px solid var(--sk-border);
  padding-top: 4px;
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

const LiveIndicator = styled.span`
  display: inline-flex;
  align-items: center;
  gap: 3px;
  padding: 2px 5px;
  background: rgba(239, 68, 68, 0.15);
  color: rgb(239, 68, 68);
  border: 1px solid rgba(239, 68, 68, 0.3);
  border-radius: 3px;
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.2px;
  flex-shrink: 0;
  user-select: none;
`;

const LiveDot = styled.div`
  width: 4px;
  height: 4px;
  border-radius: 50%;
  background: rgb(239, 68, 68);
  animation: pulse 2s ease-in-out infinite;
  flex-shrink: 0;

  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.5;
    }
  }
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

const ZIndexRow = styled.div`
  display: flex;
  align-items: center;
  gap: 4px;
  font-size: 11px;
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

const StackButton = styled.button`
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  padding: 0;
  border: 1px solid var(--sk-border);
  border-radius: 3px;
  background: var(--sk-input-bg);
  color: var(--sk-text-muted);
  cursor: pointer;
  font-size: 12px;
  line-height: 1;
  pointer-events: auto;
  flex-shrink: 0;

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

const OverlaySection = styled.div`
  border-top: 1px solid var(--sk-border);
  padding: 4px 0;
`;

const OverlaySectionHeader = styled.div`
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 2px 0;
  font-size: 11px;
  color: var(--sk-text-muted);
  font-weight: 600;
`;

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

const OverlayItem = styled.div`
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 3px 0;
  font-size: 11px;
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

const OverlayTextInput = styled.input`
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

  &:focus {
    border-color: var(--sk-primary);
  }
`;

const OverlayNumInput = styled(NumericInput)`
  width: 40px;
  font-size: 10px;
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

// ── Overlay management components ───────────────────────────────────────────

const OverlayList: React.FC<{
  textOverlays: TextOverlayState[];
  imageOverlays: ImageOverlayState[];
  onAddText: (text: string) => void;
  onUpdateText: (id: string, updates: Partial<Omit<TextOverlayState, 'id'>>) => void;
  onRemoveText: (id: string) => void;
  onAddImage: (dataBase64: string) => void;
  onRemoveImage: (id: string) => void;
  disabled: boolean;
}> = React.memo(
  ({
    textOverlays,
    imageOverlays,
    onAddText,
    onUpdateText,
    onRemoveText,
    onAddImage,
    onRemoveImage,
    disabled,
  }) => {
    const [menuOpen, setMenuOpen] = useState(false);
    const [editingId, setEditingId] = useState<string | null>(null);
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

        // Validate image type
        if (!file.type.startsWith('image/')) return;

        const reader = new FileReader();
        reader.onload = () => {
          const result = reader.result as string;
          // Strip the data:image/...;base64, prefix
          const base64 = result.split(',')[1];
          if (base64) onAddImage(base64);
        };
        reader.readAsDataURL(file);
        e.target.value = '';
      },
      [onAddImage]
    );

    const totalOverlays = textOverlays.length + imageOverlays.length;

    return (
      <OverlaySection>
        <OverlaySectionHeader>
          <span>Overlays ({totalOverlays})</span>
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
        </OverlaySectionHeader>

        <HiddenFileInput
          ref={fileInputRef}
          type="file"
          accept="image/png,image/jpeg,image/webp,image/gif"
          onChange={handleImageFileChange}
        />

        {textOverlays.map((o) => (
          <React.Fragment key={o.id}>
            <OverlayItem>
              <OverlayIcon>
                <Type size={11} />
              </OverlayIcon>
              <OverlayLabel
                title={`text_${textOverlays.indexOf(o)}`}
                style={{ cursor: disabled ? 'default' : 'pointer' }}
                onClick={() => !disabled && setEditingId(editingId === o.id ? null : o.id)}
              >
                {`text_${textOverlays.indexOf(o)}`}
              </OverlayLabel>
              <SKTooltip content="Remove text overlay">
                <RemoveButton
                  disabled={disabled}
                  className="nodrag nopan"
                  onClick={() => onRemoveText(o.id)}
                >
                  <X size={12} />
                </RemoveButton>
              </SKTooltip>
            </OverlayItem>
            {editingId === o.id && (
              <>
                <OverlayEditRow>
                  <OverlayTextInput
                    value={o.text}
                    onChange={(e) => onUpdateText(o.id, { text: e.target.value })}
                    placeholder="Text content"
                    disabled={disabled}
                    className="nodrag nopan"
                  />
                </OverlayEditRow>
                <OverlayEditRow>
                  <span style={{ color: 'var(--sk-text-muted)' }}>Size</span>
                  <OverlayNumInput
                    type="number"
                    value={o.fontSize}
                    onChange={(e) => {
                      const v = Number.parseInt(e.target.value, 10);
                      if (!Number.isNaN(v) && v > 0) onUpdateText(o.id, { fontSize: v });
                    }}
                    disabled={disabled}
                    className="nodrag nopan"
                  />
                  <span style={{ color: 'var(--sk-text-muted)' }}>Opacity</span>
                  <OverlayNumInput
                    type="number"
                    min="0"
                    max="1"
                    step="0.1"
                    value={o.opacity}
                    onChange={(e) => {
                      const v = Number.parseFloat(e.target.value);
                      if (!Number.isNaN(v))
                        onUpdateText(o.id, { opacity: Math.max(0, Math.min(1, v)) });
                    }}
                    disabled={disabled}
                    className="nodrag nopan"
                  />
                </OverlayEditRow>
              </>
            )}
          </React.Fragment>
        ))}

        {imageOverlays.map((o) => (
          <OverlayItem key={o.id}>
            <OverlayIcon>
              <Image size={11} />
            </OverlayIcon>
            <OverlayLabel>Image {o.id.replace('img_', '#')}</OverlayLabel>
            <SKTooltip content="Remove image overlay">
              <RemoveButton
                disabled={disabled}
                className="nodrag nopan"
                onClick={() => onRemoveImage(o.id)}
              >
                <X size={12} />
              </RemoveButton>
            </SKTooltip>
          </OverlayItem>
        ))}

        {totalOverlays === 0 && <NoSelectionText>No overlays added</NoSelectionText>}
      </OverlaySection>
    );
  }
);
OverlayList.displayName = 'OverlayList';

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

// ── Unified layer list ──────────────────────────────────────────────────────

const UnifiedLayerList: React.FC<{
  layers: {
    id: string;
    x: number;
    y: number;
    width: number;
    height: number;
    opacity: number;
    zIndex: number;
    rotationDegrees: number;
    visible: boolean;
  }[];
  textOverlays: TextOverlayState[];
  imageOverlays: ImageOverlayState[];
  selectedLayerId: string | null;
  onSelectLayer: (id: string | null) => void;
  onOpacityChange: (layerId: string, opacity: number) => void;
  onRotationChange: (layerId: string, degrees: number) => void;
  onZIndexChange: (layerId: string, zIndex: number) => void;
  onToggleVisibility: (layerId: string) => void;
  onAddText: (text: string) => void;
  onUpdateText: (id: string, updates: Partial<Omit<TextOverlayState, 'id'>>) => void;
  onRemoveText: (id: string) => void;
  onAddImage: (dataBase64: string) => void;
  onRemoveImage: (id: string) => void;
  disabled: boolean;
}> = React.memo(
  ({
    layers,
    textOverlays,
    imageOverlays,
    selectedLayerId,
    onSelectLayer,
    onOpacityChange,
    onRotationChange,
    onZIndexChange,
    onToggleVisibility,
    onAddText,
    onUpdateText,
    onRemoveText,
    onAddImage,
    onRemoveImage,
    disabled,
  }) => {
    // Build a unified list of all layers sorted by z-index (highest first for
    // a "top-to-bottom" visual stack). Text overlays get implicit z-index
    // 100+n, image overlays 200+n.
    const entries: UnifiedLayerEntry[] = React.useMemo(() => {
      const all: UnifiedLayerEntry[] = [];

      for (const l of layers) {
        all.push({ id: l.id, kind: 'video', label: l.id, zIndex: l.zIndex, visible: l.visible });
      }
      textOverlays.forEach((o, i) => {
        all.push({
          id: o.id,
          kind: 'text',
          label: `text_${i}`,
          zIndex: 100 + i,
          visible: o.visible,
        });
      });
      imageOverlays.forEach((o, i) => {
        all.push({
          id: o.id,
          kind: 'image',
          label: `Image #${i}`,
          zIndex: 200 + i,
          visible: o.visible,
        });
      });

      // Sort highest z-index first (top of visual stack at the top of the list)
      all.sort((a, b) => b.zIndex - a.zIndex);
      return all;
    }, [layers, textOverlays, imageOverlays]);

    const selectedLayer = layers.find((l) => l.id === selectedLayerId);

    // Compute stack navigation for video layers
    const sortedVideoByZ = [...layers].sort((a, b) => a.zIndex - b.zIndex);
    const stackIndex = selectedLayer
      ? sortedVideoByZ.findIndex((l) => l.id === selectedLayer.id)
      : -1;
    const isBottommost = stackIndex === 0;
    const isTopmost = stackIndex === sortedVideoByZ.length - 1;

    // Add overlay menu state
    const [menuOpen, setMenuOpen] = useState(false);
    const fileInputRef = useRef<HTMLInputElement>(null);

    // Find selected text overlay for inline editing controls
    const selectedTextOverlay = textOverlays.find((o) => o.id === selectedLayerId);

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
          if (base64) onAddImage(base64);
        };
        reader.readAsDataURL(file);
        e.target.value = '';
      },
      [onAddImage]
    );

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

        {entries.map((entry) => (
          <LayerListItem
            key={entry.id}
            isSelected={entry.id === selectedLayerId}
            isHidden={!entry.visible}
            className="nodrag nopan"
            onClick={() => onSelectLayer(entry.id === selectedLayerId ? null : entry.id)}
          >
            <OverlayIcon>{iconForKind(entry.kind)}</OverlayIcon>
            <OverlayLabel style={{ fontWeight: entry.id === selectedLayerId ? 600 : 400 }}>
              {entry.label}
            </OverlayLabel>
            <span
              style={{
                fontSize: 9,
                color: 'var(--sk-text-muted)',
                fontVariantNumeric: 'tabular-nums',
                flexShrink: 0,
              }}
            >
              z:{entry.zIndex}
            </span>
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
        ))}

        {/* Issue #6: visual separator between layer list and per-layer controls */}
        {/* Controls for the selected video layer */}
        {selectedLayer && (
          <>
            <LayerInfoRow
              style={{ marginTop: 4, paddingTop: 6, borderTop: '1px solid var(--sk-border)' }}
            >
              <LayerName>{selectedLayer.id}</LayerName>
              <LayerPosition>
                ({Math.round(selectedLayer.x)}, {Math.round(selectedLayer.y)})
              </LayerPosition>
            </LayerInfoRow>

            <ControlRow>
              <ControlLabel>Opacity</ControlLabel>
              <SliderInput
                type="range"
                min="0"
                max="1"
                step="0.01"
                value={selectedLayer.opacity}
                onChange={(e) =>
                  onOpacityChange(selectedLayer.id, Number.parseFloat(e.target.value))
                }
                disabled={disabled}
                className="nodrag nopan"
              />
              <ControlValue>{(selectedLayer.opacity * 100).toFixed(0)}%</ControlValue>
            </ControlRow>

            <ControlRow>
              <ControlLabel>Rotation</ControlLabel>
              <SliderInput
                type="range"
                min="-180"
                max="180"
                step="1"
                value={selectedLayer.rotationDegrees}
                onChange={(e) =>
                  onRotationChange(selectedLayer.id, Number.parseFloat(e.target.value))
                }
                disabled={disabled}
                className="nodrag nopan"
              />
              <ControlValue>{selectedLayer.rotationDegrees.toFixed(0)}&deg;</ControlValue>
            </ControlRow>

            <ZIndexRow>
              <ControlLabel>Order</ControlLabel>
              <SKTooltip content="Send backward">
                <StackButton
                  disabled={disabled || isBottommost}
                  className="nodrag nopan"
                  onClick={() => {
                    if (isBottommost) return;
                    const below = sortedVideoByZ[stackIndex - 1];
                    onZIndexChange(selectedLayer.id, below.zIndex - 1);
                  }}
                >
                  ▼
                </StackButton>
              </SKTooltip>
              <NumericInput
                type="number"
                value={selectedLayer.zIndex}
                onChange={(e) => {
                  const val = Number.parseInt(e.target.value, 10);
                  if (!Number.isNaN(val)) onZIndexChange(selectedLayer.id, val);
                }}
                disabled={disabled}
                className="nodrag nopan"
              />
              <SKTooltip content="Bring forward">
                <StackButton
                  disabled={disabled || isTopmost}
                  className="nodrag nopan"
                  onClick={() => {
                    if (isTopmost) return;
                    const above = sortedVideoByZ[stackIndex + 1];
                    onZIndexChange(selectedLayer.id, above.zIndex + 1);
                  }}
                >
                  ▲
                </StackButton>
              </SKTooltip>
            </ZIndexRow>
          </>
        )}

        {/* Controls for the selected text overlay */}
        {/* Issue #6: visual separator + Issue #7: opacity/rotation sliders for text */}
        {selectedTextOverlay && (
          <>
            <LayerInfoRow
              style={{ marginTop: 4, paddingTop: 6, borderTop: '1px solid var(--sk-border)' }}
            >
              <LayerName>Text</LayerName>
              <LayerPosition>
                ({Math.round(selectedTextOverlay.x)}, {Math.round(selectedTextOverlay.y)})
              </LayerPosition>
            </LayerInfoRow>
            <OverlayEditRow style={{ paddingLeft: 0 }}>
              <OverlayTextInput
                value={selectedTextOverlay.text}
                onChange={(e) => onUpdateText(selectedTextOverlay.id, { text: e.target.value })}
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
                    onUpdateText(selectedTextOverlay.id, { fontSize: v });
                }}
                disabled={disabled}
                className="nodrag nopan"
              />
            </OverlayEditRow>

            {/* Issue #7: opacity slider for text layers (same as video layers) */}
            <ControlRow>
              <ControlLabel>Opacity</ControlLabel>
              <SliderInput
                type="range"
                min="0"
                max="1"
                step="0.01"
                value={selectedTextOverlay.opacity}
                onChange={(e) =>
                  onUpdateText(selectedTextOverlay.id, {
                    opacity: Number.parseFloat(e.target.value),
                  })
                }
                disabled={disabled}
                className="nodrag nopan"
              />
              <ControlValue>{(selectedTextOverlay.opacity * 100).toFixed(0)}%</ControlValue>
            </ControlRow>

            {/* Issue #7: rotation slider for text layers (same as video layers) */}
            <ControlRow>
              <ControlLabel>Rotation</ControlLabel>
              <SliderInput
                type="range"
                min="-180"
                max="180"
                step="1"
                value={selectedTextOverlay.rotationDegrees}
                onChange={(e) =>
                  onUpdateText(selectedTextOverlay.id, {
                    rotationDegrees: Number.parseFloat(e.target.value),
                  })
                }
                disabled={disabled}
                className="nodrag nopan"
              />
              <ControlValue>{selectedTextOverlay.rotationDegrees.toFixed(0)}&deg;</ControlValue>
            </ControlRow>
          </>
        )}
      </LayerControls>
    );
  }
);
UnifiedLayerList.displayName = 'UnifiedLayerList';

// ── Main compositor node ────────────────────────────────────────────────────

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
    updateLayerZIndex,
    toggleLayerVisibility,
    layerRefs,
    textOverlays,
    imageOverlays,
    addTextOverlay,
    updateTextOverlay,
    removeTextOverlay,
    addImageOverlay,
    removeImageOverlay,
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

  // Broadcast compositor layer selection for YAML highlighting
  useEffect(() => {
    setCompositorSelection(selected ? data.label : null, selectedLayerId);
    return () => setCompositorSelection(null, null);
  }, [selected, data.label, selectedLayerId]);

  // Show live indicator when node is in an active session and is not staged
  const showLiveIndicator = !data.isStaged && !!data.onConfigChange && !!data.sessionId;

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
                  <LiveIndicator style={{ marginLeft: 6 }}>
                    <LiveDot />
                    LIVE
                  </LiveIndicator>
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
          layers={layers}
          textOverlays={textOverlays}
          imageOverlays={imageOverlays}
          selectedLayerId={selectedLayerId}
          onSelectLayer={selectLayer}
          onOpacityChange={updateLayerOpacity}
          onRotationChange={updateLayerRotation}
          onZIndexChange={updateLayerZIndex}
          onToggleVisibility={toggleLayerVisibility}
          onAddText={addTextOverlay}
          onUpdateText={updateTextOverlay}
          onRemoveText={removeTextOverlay}
          onAddImage={addImageOverlay}
          onRemoveImage={removeImageOverlay}
          disabled={disabled}
        />
      </CompositorWrapper>
    </NodeFrame>
  );
});

CompositorNode.displayName = 'CompositorNode';

export default CompositorNode;
