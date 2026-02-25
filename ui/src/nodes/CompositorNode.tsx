// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import * as Tooltip from '@radix-ui/react-tooltip';
import React from 'react';

import { CompositorCanvas } from '@/components/CompositorCanvas';
import { NodeFrame } from '@/components/node/NodeFrame';
import { useCompositorLayers } from '@/hooks/useCompositorLayers';
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

const TooltipContent = styled(Tooltip.Content)`
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  padding: 8px 12px;
  box-shadow: 0 4px 12px var(--sk-shadow);
  font-size: 11px;
  z-index: 1000;
  max-width: 250px;
  color: var(--sk-text);
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

const LayerCount = styled.div`
  font-size: 11px;
  color: var(--sk-text-muted);
  text-align: center;
  padding: 2px 0;
`;

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

// ── Selected layer controls ─────────────────────────────────────────────────

const SelectedLayerControls: React.FC<{
  layers: {
    id: string;
    x: number;
    y: number;
    width: number;
    height: number;
    opacity: number;
    zIndex: number;
    rotationDegrees: number;
  }[];
  selectedLayerId: string | null;
  onOpacityChange: (layerId: string, opacity: number) => void;
  onRotationChange: (layerId: string, degrees: number) => void;
  onZIndexChange: (layerId: string, zIndex: number) => void;
  disabled: boolean;
}> = React.memo(
  ({ layers, selectedLayerId, onOpacityChange, onRotationChange, onZIndexChange, disabled }) => {
    const selectedLayer = layers.find((l) => l.id === selectedLayerId);

    if (!selectedLayer) {
      return (
        <NoSelectionText>
          {layers.length > 0 ? 'Click a layer to edit' : 'No layers configured'}
        </NoSelectionText>
      );
    }

    return (
      <LayerControls>
        <LayerInfoRow>
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
            onChange={(e) => onOpacityChange(selectedLayer.id, Number.parseFloat(e.target.value))}
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
            onChange={(e) => onRotationChange(selectedLayer.id, Number.parseFloat(e.target.value))}
            disabled={disabled}
            className="nodrag nopan"
          />
          <ControlValue>{selectedLayer.rotationDegrees.toFixed(0)}&deg;</ControlValue>
        </ControlRow>

        <ControlRow>
          <ControlLabel>Z-Index</ControlLabel>
          <SliderInput
            type="range"
            min="-10"
            max="10"
            step="1"
            value={selectedLayer.zIndex}
            onChange={(e) => onZIndexChange(selectedLayer.id, Number.parseInt(e.target.value, 10))}
            disabled={disabled}
            className="nodrag nopan"
          />
          <ControlValue>{selectedLayer.zIndex}</ControlValue>
        </ControlRow>
      </LayerControls>
    );
  }
);
SelectedLayerControls.displayName = 'SelectedLayerControls';

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
    layerRefs,
  } = useCompositorLayers({
    nodeId: id,
    sessionId: data.sessionId,
    canvasWidth,
    canvasHeight,
    params: data.params ?? {},
    onConfigChange: data.onConfigChange,
    isStaged: data.isStaged,
  });

  const disabled = !data.onConfigChange;

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
                <Tooltip.Provider delayDuration={300}>
                  <Tooltip.Root>
                    <Tooltip.Trigger asChild>
                      <LiveIndicator style={{ marginLeft: 6 }}>
                        <LiveDot />
                        LIVE
                      </LiveIndicator>
                    </Tooltip.Trigger>
                    <Tooltip.Portal>
                      <TooltipContent side="top" sideOffset={5}>
                        Layer changes apply immediately to the running pipeline
                        <Tooltip.Arrow style={{ fill: 'var(--sk-border)' }} />
                      </TooltipContent>
                    </Tooltip.Portal>
                  </Tooltip.Root>
                </Tooltip.Provider>
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
            selectedLayerId={selectedLayerId}
            onSelectLayer={selectLayer}
            onLayerPointerDown={handleLayerPointerDown}
            onResizePointerDown={handleResizePointerDown}
            layerRefs={layerRefs}
            disabled={disabled}
          />
        </CanvasSection>

        <SelectedLayerControls
          layers={layers}
          selectedLayerId={selectedLayerId}
          onOpacityChange={updateLayerOpacity}
          onRotationChange={updateLayerRotation}
          onZIndexChange={updateLayerZIndex}
          disabled={disabled}
        />

        <LayerCount>
          {layers.length} layer{layers.length !== 1 ? 's' : ''}
        </LayerCount>
      </CompositorWrapper>
    </NodeFrame>
  );
});

CompositorNode.displayName = 'CompositorNode';

export default CompositorNode;
