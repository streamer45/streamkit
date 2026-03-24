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
}

interface CompositorNodeProps {
  id: string;
  data: CompositorNodeData;
  selected?: boolean;
}

// ── Connected entry list ──────────────────────────────────────────────────────────────
// Subscribes to all layer atoms so it re-renders when entries change, but
// useStableEntries returns a stable reference during opacity/rotation drags,
// so CompositorEntryList (React.memo) bails out on those ticks.

interface ConnectedEntryListProps {
  selectedLayerId: string | null;
  onSelectLayer: (id: string | null) => void;
  onToggleVisibility: (layerId: string) => void;
  onAddText: (text: string) => void;
  onRemoveText: (id: string) => void;
  onAddImage: (dataBase64: string, naturalWidth?: number, naturalHeight?: number) => void;
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

// ── Connected context menu ────────────────────────────────────────────────────────────
// Only mounts when the context menu is open. Reads entries from atoms.

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

  const canvasWidth = (data.params?.width as number) ?? 1280;
  const canvasHeight = (data.params?.height as number) ?? 720;

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
    params: data.params ?? {},
    onConfigChange: data.onConfigChange,
    onParamChange: data.onParamChange,
  });

  // Stable interaction callbacks for suppressing stale server view data
  // during continuous slider drags (opacity, rotation, crop, text alpha).
  // These set activeInteractionRef.current which gates useServerLayoutSync.
  // On interaction end, the next server view-data tick (~16-33ms) reconciles.
  const handleInteractionStart = useCallback(() => {
    activeInteractionRef.current = true;
  }, [activeInteractionRef]);
  const handleInteractionEnd = useCallback(() => {
    activeInteractionRef.current = false;
  }, [activeInteractionRef]);

  const disabled = !data.onConfigChange && !data.onParamChange;

  // Context menu state — bundled into a single memo to stay within
  // the max-statements budget for this component.
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

  // Focus the wrapper when a layer is selected so that subsequent
  // keyboard events (arrows, Delete, Escape) bubble through the wrapper.
  // Pointer handlers call preventDefault() which suppresses the browser's
  // default focus, so we must set it explicitly.
  // Skip if an input/textarea already has focus (e.g. text inspector)
  // to avoid stealing focus from the user mid-typing.
  useEffect(() => {
    if (selectedLayerId && compositorWrapperRef.current) {
      const tag = document.activeElement?.tagName;
      if (tag !== 'INPUT' && tag !== 'TEXTAREA') {
        compositorWrapperRef.current.focus({ preventScroll: true });
      }
    }
  }, [selectedLayerId]);

  // Standalone textInputRef — passed to inspector for double-click focus
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

  // Broadcast compositor layer selection for YAML highlighting
  useEffect(() => {
    setCompositorSelection(data.label, selected ? selectedLayerId : null);
    return () => clearCompositorSelection(data.label);
  }, [selected, data.label, selectedLayerId]);

  // Show live indicator when node is in an active session
  const showLiveIndicator = !!data.onConfigChange && !!data.sessionId;

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

  // Memoize side panel content to prevent cascade re-renders through
  // intermediate styled divs when only appearance props change.
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

  // Memoize canvas section content to prevent cascade through
  // CompositorWrapper → CanvasSection styled div wrappers.
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
      state={data.state}
      sessionId={data.sessionId}
    >
      <Provider store={store}>
        <CompositorOuterWrapper
          ref={compositorWrapperRef}
          tabIndex={-1}
          data-testid="compositor-keyboard-target"
        >
          <CompositorWrapper>
            <CanvasSection>
              {/* Side panel rendered first in DOM order so that layer-list text
                  (e.g. "Text 0") is matched before identically-named canvas labels
                  by Playwright's getByText().first(). The panel uses position:absolute
                  so DOM order has no effect on visual layout. */}
              <SidePanel className="nodrag nopan">{sidePanelContent}</SidePanel>
              {canvasSectionContent}
            </CanvasSection>
          </CompositorWrapper>
        </CompositorOuterWrapper>
      </Provider>
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
}, areNodePropsEqual);

CompositorNode.displayName = 'CompositorNode';

export default CompositorNode;
