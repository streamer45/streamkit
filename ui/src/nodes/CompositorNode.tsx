// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { Provider, useAtomValue } from 'jotai/react';
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { CompositorCanvas } from '@/components/CompositorCanvas';
import { CompositorContextMenu, type ContextMenuState } from '@/components/compositorContextMenu';
import { NodeFrame } from '@/components/node/NodeFrame';
import { SKTooltip } from '@/components/Tooltip';
import { LiveBadge, LiveDot } from '@/components/ui/LiveIndicator';
import { allLayersAtom, allTextOverlaysAtom, allImageOverlaysAtom } from '@/hooks/compositorAtoms';
import { useCompositorKeyboard } from '@/hooks/compositorKeyboard';
import { useCompositorLayers } from '@/hooks/useCompositorLayers';
import type { LayerKind } from '@/hooks/useCompositorLayers';
import { clearCompositorSelection, setCompositorSelection } from '@/hooks/useCompositorSelection';
import { useNodeStateFromAtom } from '@/hooks/useNodeAtoms';
import { areNodePropsEqual } from '@/nodes/nodePropsEqual';
import { perfOnRender } from '@/perf';
import type { InputPin, OutputPin, NodeState, NodeStats, NodeDefinition } from '@/types/types';
import { nodesLogger } from '@/utils/logger';

import { useStableEntries } from './compositorNodeEntries';
import { CompositorInspector } from './compositorNodeInspector';
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
  draft?: { missingRequired: string[]; isCreating: boolean; onPromote: () => void };
}

interface CompositorNodeProps {
  id: string;
  data: CompositorNodeData;
  selected?: boolean;
}

interface ConnectedEntryListProps {
  selectedLayerId: string | null;
  onSelectLayer: (id: string | null) => void;
  onToggleVisibility: (layerId: string) => void;
  onAddText: (text: string) => void;
  onRemoveText: (id: string) => void;
  onAddImage: (assetPath: string, naturalWidth?: number, naturalHeight?: number) => void;
  onRemoveImage: (id: string) => void;
  onReorderLayers: (entries: Array<{ id: string; kind: LayerKind; zIndex: number }>) => void;
  disabled: boolean;
}

const ConnectedEntryList: React.FC<ConnectedEntryListProps> = React.memo((props) => {
  const layers = useAtomValue(allLayersAtom);
  const textOverlays = useAtomValue(allTextOverlaysAtom);
  const imageOverlays = useAtomValue(allImageOverlaysAtom);
  const entries = useStableEntries(layers, textOverlays, imageOverlays);

  return <CompositorEntryList entries={entries} {...props} />;
});
ConnectedEntryList.displayName = 'ConnectedEntryList';

interface ConnectedContextMenuProps {
  menu: ContextMenuState;
  onReorderLayers: (entries: Array<{ id: string; kind: LayerKind; zIndex: number }>) => void;
  onRemoveText: (id: string) => void;
  onRemoveImage: (id: string) => void;
  onClose: () => void;
}

const ConnectedContextMenu: React.FC<ConnectedContextMenuProps> = React.memo((props) => {
  const layers = useAtomValue(allLayersAtom);
  const textOverlays = useAtomValue(allTextOverlaysAtom);
  const imageOverlays = useAtomValue(allImageOverlaysAtom);
  const entries = useStableEntries(layers, textOverlays, imageOverlays);

  if (!entries.some((e) => e.id === props.menu.layerId)) return null;

  return <CompositorContextMenu entries={entries} {...props} />;
});
ConnectedContextMenu.displayName = 'ConnectedContextMenu';

// ── Main compositor node ──────────────────────────────────────────────────────────────

const CompositorNode: React.FC<CompositorNodeProps> = React.memo(function CompositorNode({
  id,
  data,
  selected,
}) {
  nodesLogger.debug('CompositorNode Render:', id);
  const state = useNodeStateFromAtom(id, data.sessionId, data.state);
  const params = data.params ?? {};

  const canvasWidth = (params?.width as number) ?? 1280;
  const canvasHeight = (params?.height as number) ?? 720;

  const {
    selectedLayerId,
    selectLayer,
    handleLayerPointerDown,
    handleResizePointerDown,
    updateLayerOpacity,
    updateLayerRotation,
    toggleLayerVisibility,
    updateLayerMirror,
    updateLayerCropZoom,
    updateLayerPositionSize,
    layerRefs,
    snapGuideRefs,
    addTextOverlay,
    updateTextOverlay,
    removeTextOverlay,
    addImageOverlay,
    updateImageOverlay,
    removeImageOverlay,
    reorderLayers,
    activeInteractionRef,
    keyboardDeps,
    store,
  } = useCompositorLayers({
    nodeId: id,
    sessionId: data.sessionId,
    canvasWidth,
    canvasHeight,
    params,
    onConfigChange: data.onConfigChange,
    onParamChange: data.onParamChange,
  });

  const handleInteractionStart = useCallback(() => {
    activeInteractionRef.current = true;
  }, [activeInteractionRef]);
  const handleInteractionEnd = useCallback(() => {
    activeInteractionRef.current = false;
  }, [activeInteractionRef]);

  const disabled = !data.onConfigChange && !data.onParamChange;

  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null);
  const contextMenuHandlers = useMemo(
    () => ({
      open: (layerId: string, layerKind: LayerKind, x: number, y: number) =>
        setContextMenu({ layerId, layerKind, x, y }),
      close: () => setContextMenu(null),
    }),
    []
  );

  // ── Keyboard shortcuts ────────────────────────────────────────────────
  const compositorWrapperRef = useRef<HTMLDivElement>(null);
  useCompositorKeyboard(compositorWrapperRef, { ...keyboardDeps, disabled });

  useEffect(() => {
    if (selectedLayerId && compositorWrapperRef.current) {
      const tag = document.activeElement?.tagName;
      if (tag !== 'INPUT' && tag !== 'TEXTAREA') {
        compositorWrapperRef.current.focus({ preventScroll: true });
      }
    }
  }, [selectedLayerId]);

  const textInputRef = useRef<HTMLTextAreaElement>(null);

  const handleTextFocusRequest = useCallback(
    (layerId: string) => {
      selectLayer(layerId);
      requestAnimationFrame(() => {
        textInputRef.current?.focus();
        const el = textInputRef.current;
        if (el) el.selectionStart = el.selectionEnd = el.value.length;
      });
    },
    [selectLayer]
  );

  useEffect(() => {
    setCompositorSelection(data.label, selected ? selectedLayerId : null);
    return () => clearCompositorSelection(data.label);
  }, [selected, data.label, selectedLayerId]);

  const showLiveIndicator = !!data.onConfigChange && !!data.sessionId;

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

  const sidePanelContent = useMemo(
    () => (
      <>
        <ConnectedEntryList
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
          updateLayerOpacity={updateLayerOpacity}
          updateLayerRotation={updateLayerRotation}
          updateLayerMirror={updateLayerMirror}
          updateLayerCropZoom={updateLayerCropZoom}
          updateLayerPositionSize={updateLayerPositionSize}
          updateTextOverlay={updateTextOverlay}
          updateImageOverlay={updateImageOverlay}
          textInputRef={textInputRef}
          disabled={disabled}
          onInteractionStart={handleInteractionStart}
          onInteractionEnd={handleInteractionEnd}
        />
      </>
    ),
    [
      selectedLayerId,
      selectLayer,
      toggleLayerVisibility,
      addTextOverlay,
      removeTextOverlay,
      addImageOverlay,
      removeImageOverlay,
      reorderLayers,
      disabled,
      updateLayerOpacity,
      updateLayerRotation,
      updateLayerMirror,
      updateLayerCropZoom,
      updateLayerPositionSize,
      updateTextOverlay,
      updateImageOverlay,
      handleInteractionStart,
      handleInteractionEnd,
    ]
  );

  const canvasSectionContent = useMemo(
    () => (
      <>
        {canvasHeaderContent}

        <CompositorCanvas
          canvasWidth={canvasWidth}
          canvasHeight={canvasHeight}
          onSelectLayer={selectLayer}
          onLayerPointerDown={handleLayerPointerDown}
          onResizePointerDown={handleResizePointerDown}
          onTextFocusRequest={disabled ? undefined : handleTextFocusRequest}
          onLayerContextMenu={disabled ? undefined : contextMenuHandlers.open}
          layerRefs={layerRefs}
          snapGuideRefs={snapGuideRefs}
          disabled={disabled}
        />
        {contextMenu && (
          <ConnectedContextMenu
            menu={contextMenu}
            onReorderLayers={reorderLayers}
            onRemoveText={removeTextOverlay}
            onRemoveImage={removeImageOverlay}
            onClose={contextMenuHandlers.close}
          />
        )}
      </>
    ),
    [
      canvasHeaderContent,
      canvasWidth,
      canvasHeight,
      selectLayer,
      handleLayerPointerDown,
      handleResizePointerDown,
      disabled,
      handleTextFocusRequest,
      contextMenuHandlers,
      layerRefs,
      snapGuideRefs,
      contextMenu,
      reorderLayers,
      removeTextOverlay,
      removeImageOverlay,
    ]
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
      state={state}
      sessionId={data.sessionId}
      draft={data.draft}
    >
      <Provider store={store}>
        <CompositorOuterWrapper
          ref={compositorWrapperRef}
          tabIndex={-1}
          data-testid="compositor-keyboard-target"
        >
          <CompositorWrapper>
            <CanvasSection>
              {/* SidePanel first in DOM order so Playwright's getByText().first()
                  matches layer-list text before identically-named canvas labels.
                  position:absolute means DOM order has no visual effect. */}
              <SidePanel className="nodrag nopan">{sidePanelContent}</SidePanel>
              {canvasSectionContent}
            </CanvasSection>
          </CompositorWrapper>
        </CompositorOuterWrapper>
      </Provider>
    </NodeFrame>
  );

  if (import.meta.env.DEV) {
    return (
      <React.Profiler id="CompositorNode" onRender={perfOnRender}>
        {nodeContent}
      </React.Profiler>
    );
  }

  return nodeContent;
}, areNodePropsEqual);

CompositorNode.displayName = 'CompositorNode';

export default CompositorNode;
