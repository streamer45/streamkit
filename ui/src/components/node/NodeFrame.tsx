// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React from 'react';

import { NodeStateIndicator } from '@/components/NodeStateIndicator';
import type {
  InputPin,
  OutputPin,
  NodeState,
  NodeStats,
  PacketType,
  NodeDefinition,
} from '@/types/types';

import { PinRow } from './PinRow';
import { PlaceholderPinRow } from './PlaceholderPinRow';

const NodeWrapper = styled.div<{ selected?: boolean; minWidth: number; isDraft?: boolean }>`
  background: var(--sk-panel-bg);
  border: 2px ${(props) => (props.isDraft ? 'dashed' : 'solid')}
    ${(props) =>
      props.selected
        ? 'var(--sk-primary)'
        : props.isDraft
          ? 'var(--sk-warning, var(--sk-text-muted))'
          : 'var(--sk-border-strong)'};
  border-radius: 8px;
  padding: 8px;
  min-width: ${(props) => props.minWidth}px;
  display: flex;
  flex-direction: column;
  gap: 4px;
  box-shadow: ${(props) =>
    props.selected ? 'var(--sk-focus-ring)' : `0 2px 8px var(--sk-shadow)`};
  outline: ${(props) => (props.selected ? '2px solid var(--sk-primary)' : 'none')};
  outline-offset: 2px;
  color: var(--sk-text);
  opacity: ${(props) => (props.isDraft ? 0.85 : 1)};
`;

const BidirectionalWrapper = styled.div<{
  selected?: boolean;
  minWidth: number;
  isDraft?: boolean;
}>`
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 8px;
  min-width: ${(props) => props.minWidth}px;
  background: var(--sk-panel-bg);
  border: 2px ${(props) => (props.isDraft ? 'dashed' : 'solid')}
    ${(props) =>
      props.selected
        ? 'var(--sk-primary)'
        : props.isDraft
          ? 'var(--sk-warning, var(--sk-text-muted))'
          : 'var(--sk-border-strong)'};
  border-radius: 8px;
  box-shadow: ${(props) =>
    props.selected ? 'var(--sk-focus-ring)' : `0 2px 8px var(--sk-shadow)`};
  outline: ${(props) => (props.selected ? '2px solid var(--sk-primary)' : 'none')};
  outline-offset: 2px;
  color: var(--sk-text);
  opacity: ${(props) => (props.isDraft ? 0.85 : 1)};
`;

const DraftBanner = styled.div`
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 4px 6px;
  background: var(--sk-warning-bg, rgba(255, 170, 0, 0.12));
  color: var(--sk-warning, var(--sk-text));
  border: 1px dashed var(--sk-warning, var(--sk-text-muted));
  border-radius: 4px;
  font-size: 11px;
  line-height: 1.3;
`;

const DraftBadge = styled.span`
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.5px;
  font-size: 10px;
  padding: 1px 5px;
  border-radius: 3px;
  background: var(--sk-warning, var(--sk-text-muted));
  color: var(--sk-panel-bg);
  flex-shrink: 0;
`;

const DraftMessage = styled.span`
  font-family: var(--sk-font-mono, ui-monospace, monospace);
  word-break: break-word;
`;

const BidirectionalNodesRow = styled.div`
  display: flex;
  gap: 0;
  flex: 1;
  align-items: center;
`;

const BidirectionalHalf = styled.div<{ side: 'entry' | 'exit' }>`
  background: transparent;
  padding: 8px;
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 4px;
  color: var(--sk-text);
  position: relative;

  ${(props) =>
    props.side === 'entry'
      ? `
    border-right: 1px dashed var(--sk-border);
  `
      : `
    border-left: 1px dashed var(--sk-border);
  `}
`;

const BidirectionalLabel = styled.div`
  font-size: 10px;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.5px;
  color: var(--sk-primary);
  text-align: center;
  padding: 2px 0;
`;

const Header = styled.div`
  font-weight: bold;
  text-align: center;
  padding: 4px;
  background-color: var(--sk-sidebar-bg);
  border-radius: 4px;
  border: 2px dashed var(--sk-muted);
  position: relative;
`;

const StateIndicatorWrapper = styled.div`
  position: absolute;
  top: 4px;
  right: 4px;
  pointer-events: auto;
`;

const Label = styled.div`
  font-weight: bold;
  text-align: center;
  width: 100%;
  padding: 2px 0;
  color: var(--sk-text);
`;

const Kind = styled.div`
  font-size: 10px;
  color: var(--sk-text-muted);
  margin-top: 0;
`;

/** Draft state for a node that has been dropped on the canvas but not
 *  yet committed via `addnode` because required schema params are still
 *  missing.  Rendered with a dashed border and a banner listing the
 *  outstanding fields. */
export type DraftNodeState = {
  missingRequired: string[];
};

type NodeFrameProps = {
  id: string;
  label: string;
  kind: string;
  selected?: boolean;
  minWidth?: number;
  inputs?: InputPin[];
  outputs?: OutputPin[];
  nodeDefinition?: NodeDefinition;
  state?: NodeState;
  stats?: NodeStats;
  children?: React.ReactNode;
  isBidirectional?: boolean;
  sessionId?: string; // For fetching live stats
  /** When set, render the node as an unsubmitted draft (dashed border +
   *  "Draft" banner listing the missing required params). */
  draft?: DraftNodeState;
};

// Helper: Check if node definition has dynamic pins
function hasDynamicPins(pins?: Array<InputPin | OutputPin>): boolean {
  return (
    pins?.some((pin) => typeof pin.cardinality === 'object' && 'Dynamic' in pin.cardinality) ??
    false
  );
}

// Helper: Filter out dynamic template pins
function filterRuntimePins<T extends InputPin | OutputPin>(pins?: T[]): T[] {
  return (
    pins?.filter((pin) => !(typeof pin.cardinality === 'object' && 'Dynamic' in pin.cardinality)) ??
    []
  );
}

// Helper: Infer packet type for ghost pins
function inferGhostPacketType(
  runtimePins: InputPin[] | OutputPin[],
  nodeDefinitionPins: Array<InputPin | OutputPin> | undefined,
  isInput: boolean
): PacketType {
  if (isInput) {
    const inputPins = runtimePins as InputPin[];
    return (
      (inputPins.length > 0 && inputPins[0].accepts_types[0]) ||
      (nodeDefinitionPins as InputPin[])?.[0]?.accepts_types[0] ||
      ('RawAudio' as PacketType)
    );
  } else {
    const outputPins = runtimePins as OutputPin[];
    return (
      (outputPins.length > 0 && outputPins[0].produces_type) ||
      (nodeDefinitionPins as OutputPin[])?.[0]?.produces_type ||
      ('RawAudio' as PacketType)
    );
  }
}

// Sub-component: Bidirectional node layout
const BidirectionalNodeLayout: React.FC<{
  id: string;
  label: string;
  kind: string;
  selected?: boolean;
  minWidth: number;
  inputs: InputPin[];
  outputs: OutputPin[];
  state?: NodeState;
  stats?: NodeStats;
  sessionId?: string;
  draft?: DraftNodeState;
  children?: React.ReactNode;
}> = ({
  id,
  label,
  kind,
  selected,
  minWidth,
  inputs,
  outputs,
  state,
  stats,
  sessionId,
  draft,
  children,
}) => (
  <BidirectionalWrapper
    selected={selected}
    minWidth={minWidth}
    isDraft={!!draft}
    className="drag-handle nopan"
  >
    {/* Centered header with node name and type */}
    <Header>
      {state && !draft && (
        <StateIndicatorWrapper>
          <NodeStateIndicator state={state} stats={stats} nodeId={id} sessionId={sessionId} />
        </StateIndicatorWrapper>
      )}
      <Label className="code-font">{label}</Label>
      <Kind>({kind})</Kind>
    </Header>
    {draft && <DraftBannerSection draft={draft} />}

    {/* Two halves side by side */}
    <BidirectionalNodesRow>
      {/* Sink Half (Left) - Consumes data from the pipeline */}
      <BidirectionalHalf side="entry">
        <BidirectionalLabel>SINK</BidirectionalLabel>
        <PinRow nodeId={id} side="left" pins={inputs} isInput />
      </BidirectionalHalf>

      {/* Source Half (Right) - Produces data to the pipeline */}
      <BidirectionalHalf side="exit">
        <BidirectionalLabel>SOURCE</BidirectionalLabel>
        <PinRow nodeId={id} side="right" pins={outputs} isInput={false} />
      </BidirectionalHalf>
    </BidirectionalNodesRow>

    {/* Params displayed below both halves */}
    {children}
  </BidirectionalWrapper>
);

// Sub-component: Node header
const NodeHeader: React.FC<{
  id: string;
  label: string;
  kind: string;
  state?: NodeState;
  stats?: NodeStats;
  sessionId?: string;
  draft?: DraftNodeState;
}> = ({ id, label, kind, state, stats, sessionId, draft }) => (
  <Header>
    {state && !draft && (
      <StateIndicatorWrapper>
        <NodeStateIndicator state={state} stats={stats} nodeId={id} sessionId={sessionId} />
      </StateIndicatorWrapper>
    )}
    <Label className="code-font">{label}</Label>
    <Kind>({kind})</Kind>
  </Header>
);

// Sub-component: Draft banner shown above node controls when the node
// has not yet been committed because required params are missing.
const DraftBannerSection: React.FC<{ draft: DraftNodeState }> = ({ draft }) => {
  const { missingRequired } = draft;
  const message =
    missingRequired.length > 0 ? `needs ${missingRequired.join(', ')}` : 'configuring\u2026';
  return (
    <DraftBanner role="status" aria-label={`Draft node — ${message}`}>
      <DraftBadge>Draft</DraftBadge>
      <DraftMessage className="code-font">{message}</DraftMessage>
    </DraftBanner>
  );
};

// Sub-component: Normal node layout with dynamic pin support
const NormalNodeLayout: React.FC<{
  id: string;
  label: string;
  kind: string;
  selected?: boolean;
  minWidth: number;
  inputs: InputPin[];
  outputs: OutputPin[];
  nodeDefinition?: NodeDefinition;
  state?: NodeState;
  stats?: NodeStats;
  sessionId?: string;
  draft?: DraftNodeState;
  children?: React.ReactNode;
}> = ({
  id,
  label,
  kind,
  selected,
  minWidth,
  inputs,
  outputs,
  nodeDefinition,
  state,
  stats,
  sessionId,
  draft,
  children,
}) => {
  // Show ghost pins for nodes that have any dynamic cardinality pins in their definition
  const showInputGhost = hasDynamicPins(nodeDefinition?.inputs);
  const showOutputGhost = hasDynamicPins(nodeDefinition?.outputs);

  // Filter out Dynamic template pins from runtime pins (they shouldn't appear as real pins)
  const runtimeInputs = filterRuntimePins(inputs);
  const runtimeOutputs = filterRuntimePins(outputs);

  // For ghost pins, try to infer the packet type from existing pins or from the definition
  const ghostInputType = showInputGhost
    ? inferGhostPacketType(runtimeInputs, nodeDefinition?.inputs, true)
    : undefined;
  const ghostOutputType = showOutputGhost
    ? inferGhostPacketType(runtimeOutputs, nodeDefinition?.outputs, false)
    : undefined;

  // Calculate total pins including ghost for proper spacing
  const totalInputPins = runtimeInputs.length + (showInputGhost ? 1 : 0);
  const totalOutputPins = runtimeOutputs.length + (showOutputGhost ? 1 : 0);

  return (
    <NodeWrapper
      selected={selected}
      minWidth={minWidth}
      isDraft={!!draft}
      className="drag-handle nopan"
    >
      {/* Show real pins AND ghost pin for inputs */}
      {runtimeInputs.length > 0 && (
        <PinRow nodeId={id} side="top" pins={runtimeInputs} isInput totalPins={totalInputPins} />
      )}
      {showInputGhost && (
        <PlaceholderPinRow
          side="top"
          isInput
          packetType={ghostInputType}
          pinIndex={runtimeInputs.length}
          totalPins={totalInputPins}
        />
      )}

      <NodeHeader
        id={id}
        label={label}
        kind={kind}
        state={state}
        stats={stats}
        sessionId={sessionId}
        draft={draft}
      />

      {draft && <DraftBannerSection draft={draft} />}

      {children}

      {/* Show real pins AND ghost pin for outputs */}
      {runtimeOutputs.length > 0 && (
        <PinRow
          nodeId={id}
          side="bottom"
          pins={runtimeOutputs}
          isInput={false}
          totalPins={totalOutputPins}
        />
      )}
      {showOutputGhost && (
        <PlaceholderPinRow
          side="bottom"
          isInput={false}
          packetType={ghostOutputType}
          pinIndex={runtimeOutputs.length}
          totalPins={totalOutputPins}
        />
      )}
    </NodeWrapper>
  );
};

export const NodeFrame: React.FC<NodeFrameProps> = ({
  id,
  label,
  kind,
  selected,
  minWidth = 200,
  inputs = [],
  outputs = [],
  nodeDefinition,
  state,
  stats,
  children,
  isBidirectional = false,
  sessionId,
  draft,
}) => {
  if (isBidirectional) {
    return (
      <BidirectionalNodeLayout
        id={id}
        label={label}
        kind={kind}
        selected={selected}
        minWidth={minWidth}
        inputs={inputs}
        outputs={outputs}
        state={state}
        stats={stats}
        sessionId={sessionId}
        draft={draft}
      >
        {children}
      </BidirectionalNodeLayout>
    );
  }

  return (
    <NormalNodeLayout
      id={id}
      label={label}
      kind={kind}
      selected={selected}
      minWidth={minWidth}
      inputs={inputs}
      outputs={outputs}
      nodeDefinition={nodeDefinition}
      state={state}
      stats={stats}
      sessionId={sessionId}
      draft={draft}
    >
      {children}
    </NormalNodeLayout>
  );
};
