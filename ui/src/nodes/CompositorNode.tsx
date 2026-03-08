// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import React, { useCallback, useEffect, useMemo, useRef } from 'react';

import { CompositorCanvas } from '@/components/CompositorCanvas';
import { NodeFrame } from '@/components/node/NodeFrame';
import { SKTooltip } from '@/components/Tooltip';
import { LiveBadge, LiveDot } from '@/components/ui/LiveIndicator';
import { useCompositorLayers } from '@/hooks/useCompositorLayers';
import type { TextOverlayState, ImageOverlayState } from '@/hooks/useCompositorLayers';
import { setCompositorSelection } from '@/hooks/useCompositorSelection';
import { perfOnRender } from '@/perf';
import type { InputPin, OutputPin, NodeState, NodeStats, NodeDefinition } from '@/types/types';
import { nodesLogger } from '@/utils/logger';

import {
  CanvasHeader,
  CanvasLabel,
  CanvasSection,
  ColorInput,
  CompositorOuterWrapper,
  CompositorWrapper,
  FONT_OPTIONS,
  FontSelect,
  InspectorControls,
  InspectorSection,
  InspectorSectionLabel,
  OverlayEditRow,
  OverlayNumInput,
  OverlayTextInput,
  ResolutionLabel,
  SidePanel,
  SidePanelDivider,
  friendlyLabel,
  hexToRgba,
  rgbaToHex,
} from './compositorNodeParts';
import type { UnifiedLayerEntry } from './compositorNodeParts';
import {
  InspectorHeaderSection,
  MirrorControl,
  OpacityControl,
  RotationControl,
  UnifiedLayerList,
} from './compositorNodeWidgets';

// ── Node data interface ─────────────────────────────────────────────────────────────

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

// ── Stable entries hook ─────────────────────────────────────────────────────────────

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

// ── Main compositor node ──────────────────────────────────────────────────────────────

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
    updateLayerMirror,
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

  // Ref to the side-panel text textarea — double-clicking a text overlay
  // on the canvas selects it and focuses this input for comfortable editing.
  const textInputRef = useRef<HTMLTextAreaElement>(null);
  const handleTextFocusRequest = useCallback(
    (layerId: string) => {
      selectLayer(layerId);
      requestAnimationFrame(() => {
        textInputRef.current?.focus();
        // Place cursor at the end so the user can immediately start typing
        const el = textInputRef.current;
        if (el) el.selectionStart = el.selectionEnd = el.value.length;
      });
    },
    [selectLayer]
  );

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
  const handleSelectedMirrorToggle = useCallback(
    (axis: 'horizontal' | 'vertical') => {
      if (!selectedLayerId) return;
      updateLayerMirror(selectedLayerId, axis);
    },
    [selectedLayerId, updateLayerMirror]
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

  // Memoize the canvas header so the SKTooltip / LIVE badge subtree doesn't
  // re-render on every slider tick.  showLiveIndicator, canvasWidth and
  // canvasHeight are all stable during opacity/rotation drags.
  const canvasHeaderContent = useMemo(
    () => (
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
    ),
    [showLiveIndicator, canvasWidth, canvasHeight]
  );

  // Selected layer props for the inspector (stable object during same selection)
  const inspectorProps = useMemo(() => {
    if (selectedLayer)
      return {
        x: selectedLayer.x,
        y: selectedLayer.y,
        opacity: selectedLayer.opacity,
        rotationDegrees: selectedLayer.rotationDegrees,
        mirrorHorizontal: selectedLayer.mirrorHorizontal,
        mirrorVertical: selectedLayer.mirrorVertical,
      };
    if (selectedTextOverlay)
      return {
        x: selectedTextOverlay.x,
        y: selectedTextOverlay.y,
        opacity: selectedTextOverlay.opacity,
        rotationDegrees: selectedTextOverlay.rotationDegrees,
        mirrorHorizontal: selectedTextOverlay.mirrorHorizontal,
        mirrorVertical: selectedTextOverlay.mirrorVertical,
      };
    if (selectedImageOverlay)
      return {
        x: selectedImageOverlay.x,
        y: selectedImageOverlay.y,
        opacity: selectedImageOverlay.opacity,
        rotationDegrees: selectedImageOverlay.rotationDegrees,
        mirrorHorizontal: selectedImageOverlay.mirrorHorizontal,
        mirrorVertical: selectedImageOverlay.mirrorVertical,
      };
    return null;
  }, [selectedLayer, selectedTextOverlay, selectedImageOverlay]);

  const nodeContent = (
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
            {canvasHeaderContent}

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
              onTextFocusRequest={disabled ? undefined : handleTextFocusRequest}
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
                <MirrorControl
                  mirrorHorizontal={inspectorProps.mirrorHorizontal}
                  mirrorVertical={inspectorProps.mirrorVertical}
                  onToggle={handleSelectedMirrorToggle}
                  disabled={disabled}
                />
              </InspectorControls>
            </>
          )}
        </SidePanel>
      </CompositorOuterWrapper>
    </NodeFrame>
  );

  // Wrap in React.Profiler in dev builds so that Layer 2 (Playwright) can
  // capture render metrics via window.__PERF_DATA__.
  if (import.meta.env.DEV) {
    return (
      <React.Profiler id="CompositorNode" onRender={perfOnRender}>
        {nodeContent}
      </React.Profiler>
    );
  }

  return nodeContent;
});

CompositorNode.displayName = 'CompositorNode';

export default CompositorNode;
