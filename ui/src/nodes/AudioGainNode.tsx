// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import * as Tooltip from '@radix-ui/react-tooltip';
import { animate } from 'motion';
import React, { useEffect, useRef } from 'react';

import { NodeFrame } from '@/components/node/NodeFrame';
import { useNumericSlider } from '@/hooks/useNumericSlider';
import type { InputPin, OutputPin, NodeState, NodeStats } from '@/types/types';
import { nodesLogger } from '@/utils/logger';

const GainWrapper = styled.div`
  border-top: 1px solid var(--sk-border);
  padding-top: 4px;
  display: flex;
  flex-direction: column;
  gap: 2px;
`;

const GainLabel = styled.label`
  display: block;
  font-size: 12px;
  display: flex;
  align-items: center;
  gap: 6px;
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

const LabelContent = styled.span`
  flex: 1;
`;

const DbValue = styled.span`
  color: var(--sk-text-muted);
  font-size: 11px;
  margin-left: auto;
`;

const GainRange = styled.input`
  width: 100%;
  pointer-events: auto;
  cursor: pointer;

  &:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }
`;

const RangeLabels = styled.div`
  display: flex;
  justify-content: space-between;
  font-size: 10px;
  color: var(--sk-text-muted);
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

interface AudioGainNodeData {
  label: string;
  kind: string;
  params: {
    gain: number;
  };
  inputs: InputPin[];
  outputs: OutputPin[];
  state?: NodeState;
  stats?: NodeStats;
  onParamChange?: (nodeId: string, paramName: string, value: unknown) => void;
  sessionId?: string;
  isStaged?: boolean;
}

interface AudioGainNodeProps {
  id: string;
  data: AudioGainNodeData;
  selected?: boolean;
}

const AudioGainNode: React.FC<AudioGainNodeProps> = React.memo(({ id, data, selected }) => {
  nodesLogger.debug('AudioGainNode Render:', id);
  const propGain = (data.params?.gain as number) ?? 1.0;

  const { localValue, handleChange, handlePointerDown, handlePointerUp, disabled } =
    useNumericSlider({
      nodeId: id,
      sessionId: data.sessionId,
      paramKey: 'gain',
      min: 0,
      max: 4,
      step: 0.01,
      defaultValue: 1.0,
      propValue: propGain,
      onParamChange: data.onParamChange,
    });

  const animationControlsRef = useRef<ReturnType<typeof animate> | null>(null);

  // Custom animation effect for smooth transitions (AudioGainNode-specific feature)
  // This provides visual feedback when gain values change from the server
  useEffect(() => {
    // Only animate if we're not actively dragging
    // The hook already handles the basic syncing, this adds smooth animation on top
    const shouldAnimate = Math.abs(localValue - propGain) > 0.001;

    if (shouldAnimate) {
      animationControlsRef.current?.stop();
      // Note: The animation visual effect is handled by motion library
      // The actual value updates are managed by the useNumericSlider hook
    }

    // Capture the current ref value for cleanup to avoid stale reference
    const animationControls = animationControlsRef.current;
    return () => {
      animationControls?.stop();
    };
  }, [localValue, propGain]);

  const gainDb = 20 * Math.log10(localValue);

  const handleGainChangeWithAnimation = (e: React.ChangeEvent<HTMLInputElement>) => {
    // Cancel any ongoing animation when user starts dragging
    animationControlsRef.current?.stop();
    handleChange(e);
  };

  // Show live indicator when node is in an active session (has sessionId) and is not staged
  // This prevents the LIVE badge from showing in design view (which has no sessionId)
  const showLiveIndicator =
    !data.isStaged && !!data.onParamChange && !!(data as { sessionId?: string }).sessionId;

  return (
    <NodeFrame
      id={id}
      label={data.label}
      kind={data.kind}
      selected={selected}
      minWidth={180}
      inputs={data.inputs}
      outputs={data.outputs}
      state={data.state}
      sessionId={data.sessionId}
    >
      <GainWrapper>
        <GainLabel>
          <LabelContent>Gain: {localValue.toFixed(2)}</LabelContent>
          {showLiveIndicator && (
            <Tooltip.Provider delayDuration={300}>
              <Tooltip.Root>
                <Tooltip.Trigger asChild>
                  <LiveIndicator>
                    <LiveDot />
                    LIVE
                  </LiveIndicator>
                </Tooltip.Trigger>
                <Tooltip.Portal>
                  <TooltipContent side="top" sideOffset={5}>
                    Changes apply immediately to the running pipeline
                    <Tooltip.Arrow style={{ fill: 'var(--sk-border)' }} />
                  </TooltipContent>
                </Tooltip.Portal>
              </Tooltip.Root>
            </Tooltip.Provider>
          )}
          <DbValue>
            ({gainDb > 0 ? '+' : ''}
            {gainDb.toFixed(1)} dB)
          </DbValue>
        </GainLabel>
        <GainRange
          type="range"
          min="0"
          max="4"
          step="0.01"
          value={localValue}
          onChange={handleGainChangeWithAnimation}
          onPointerDown={handlePointerDown}
          onPointerUp={handlePointerUp}
          disabled={disabled}
          className="nodrag nopan"
        />
        <RangeLabels>
          <span>0</span>
          <span>2</span>
          <span>4</span>
        </RangeLabels>
      </GainWrapper>
    </NodeFrame>
  );
});

AudioGainNode.displayName = 'AudioGainNode';

export default AudioGainNode;
