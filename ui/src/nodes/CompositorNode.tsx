// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import React, { useCallback, useEffect, useMemo, useRef } from 'react';

import { CompositorCanvas } from '@/components/CompositorCanvas';
import { NodeFrame } from '@/components/node/NodeFrame';
import { SKTooltip } from '@/components/Tooltip';
import { LiveBadge, LiveDot } from '@/components/ui/LiveIndicator';
import { useCompositorKeyboard } from '@/hooks/compositorKeyboard';
import { useCompositorLayers } from '@/hooks/useCompositorLayers';
import type { TextOverlayState } from '@/hooks/useCompositorLayers';
import { clearCompositorSelection, setCompositorSelection } from '@/hooks/useCompositorSelection';
import { perfOnRender } from '@/perf';
import type { InputPin, OutputPin, NodeState, NodeStats, NodeDefinition } from '@/types/types';
import { nodesLogger } from '@/utils/logger';

import { useStableEntries } from './compositorNodeEntries';
import {
  CompositorInspector,
  useInspectorProps,
  useSelectedLayerName,
  useSelectedMirrorToggle,
  useSelectedOpacityChange,
  useSelectedPositionSizeChange,
  useSelectedRotationChange,
  useTextInspectorChildren,
} from './compositorNodeInspector';
import {
  CanvasHeader,
  CanvasLabel,
  CanvasSection,
  CompositorOuterWrapper,
  CompositorWrapper,
  ResolutionLabel,
  SidePanel,
} from './compositorNodeParts';
import { CompositorEntryList } from './compositorNodeWidgets';

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
    updateLayerPositionSize,
    layerRefs,
    snapGuideRefs,
    textOverlays,
    imageOverlays,
    addTextOverlay,
    updateTextOverlay,
    removeTextOverlay,
    addImageOverlay,
    updateImageOverlay,
    removeImageOverlay,
    reorderLayers,
    keyboardDeps,
  } = useCompositorLayers({
    nodeId: id,
    sessionId: data.sessionId,
    canvasWidth,
    canvasHeight,
    params: data.params ?? {},
    onConfigChange: data.onConfigChange,
    onParamChange: data.onParamChange,
  });

  const disabled = !data.onConfigChange && !data.onParamChange;

  // ── Keyboard shortcuts ────────────────────────────────────────────────
  const compositorWrapperRef = useRef<HTMLDivElement>(null);
  useCompositorKeyboard(compositorWrapperRef, { ...keyboardDeps, disabled });

  // Text inspector children (includes the textInputRef for double-click focus)
  const { textInspectorChildren, textInputRef } = useTextInspectorChildren(
    textOverlays.find((o) => o.id === selectedLayerId),
    updateTextOverlay as (id: string, patch: Partial<TextOverlayState>) => void,
    disabled
  );

  const handleTextFocusRequest = useCallback(
    (layerId: string) => {
      selectLayer(layerId);
      requestAnimationFrame(() => {
        textInputRef.current?.focus();
        const el = textInputRef.current;
        if (el) el.selectionStart = el.selectionEnd = el.value.length;
      });
    },
    [selectLayer, textInputRef]
  );

  // Structurally-stable entries list -- same reference during opacity/rotation
  // drags so CompositorEntryList's React.memo bails out.
  const entries = useStableEntries(layers, textOverlays, imageOverlays);

  // Broadcast compositor layer selection for YAML highlighting
  useEffect(() => {
    setCompositorSelection(data.label, selected ? selectedLayerId : null);
    return () => clearCompositorSelection(data.label);
  }, [selected, data.label, selectedLayerId]);

  // Show live indicator when node is in an active session and is not staged
  const showLiveIndicator = !data.isStaged && !!data.onConfigChange && !!data.sessionId;

  // Selected layer data for property controls
  const selectedLayer = layers.find((l) => l.id === selectedLayerId);
  const selectedTextOverlay = textOverlays.find((o) => o.id === selectedLayerId);
  const selectedImageOverlay = imageOverlays.find((o) => o.id === selectedLayerId);

  // Determine the kind of the selected layer once
  const selectedLayerKind = useMemo(() => {
    if (layers.some((l) => l.id === selectedLayerId)) return 'video' as const;
    if (textOverlays.some((o) => o.id === selectedLayerId)) return 'text' as const;
    if (imageOverlays.some((o) => o.id === selectedLayerId)) return 'image' as const;
    return null;
  }, [selectedLayerId, layers, textOverlays, imageOverlays]);

  // Stable callbacks for inspector controls
  const handleSelectedOpacityChange = useSelectedOpacityChange(
    selectedLayerId,
    selectedLayerKind,
    updateLayerOpacity,
    updateTextOverlay,
    updateImageOverlay
  );
  const handleSelectedRotationChange = useSelectedRotationChange(
    selectedLayerId,
    selectedLayerKind,
    updateLayerRotation,
    updateTextOverlay,
    updateImageOverlay
  );
  const handleSelectedMirrorToggle = useSelectedMirrorToggle(selectedLayerId, updateLayerMirror);
  const handleSelectedPositionSizeChange = useSelectedPositionSizeChange(
    selectedLayerId,
    selectedLayerKind,
    updateLayerPositionSize,
    updateTextOverlay,
    updateImageOverlay
  );

  // Derived inspector data
  const selectedLayerName = useSelectedLayerName(
    selectedLayer,
    selectedTextOverlay,
    selectedImageOverlay,
    textOverlays,
    imageOverlays
  );
  const inspectorProps = useInspectorProps(
    selectedLayer,
    selectedTextOverlay,
    selectedImageOverlay
  );

  // Memoize the canvas header so the SKTooltip / LIVE badge subtree doesn't
  // re-render on every slider tick.
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
      <CompositorOuterWrapper ref={compositorWrapperRef} tabIndex={-1}>
        {/* Side panel rendered first in DOM order so that layer-list text
            (e.g. "Text 0") is matched before identically-named canvas labels
            by Playwright's getByText().first(). The panel uses position:absolute
            so DOM order has no effect on visual layout. */}
        <SidePanel className="nodrag nopan">
          <CompositorEntryList
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

          <CompositorInspector
            inspectorProps={inspectorProps}
            selectedLayerName={selectedLayerName}
            textInspectorChildren={textInspectorChildren}
            handleSelectedOpacityChange={handleSelectedOpacityChange}
            handleSelectedRotationChange={handleSelectedRotationChange}
            handleSelectedMirrorToggle={handleSelectedMirrorToggle}
            handleSelectedPositionSizeChange={handleSelectedPositionSizeChange}
            dimensionsReadOnly={selectedLayerKind === 'text'}
            disabled={disabled}
          />
        </SidePanel>

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
              snapGuideRefs={snapGuideRefs}
              disabled={disabled}
            />
          </CanvasSection>
        </CompositorWrapper>
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
