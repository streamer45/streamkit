// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Shared right panel component for Design and Monitor views.
 * Displays either an Inspector pane (for node details), YAML pane (for pipeline YAML),
 * or Telemetry timeline (in Monitor view).
 */

import styled from '@emotion/styled';
import type { Node } from '@xyflow/react';
import React from 'react';

import InspectorPane from '@/panes/InspectorPane';
import YamlPane from '@/panes/YamlPane';
import type { NodeDefinition } from '@/types/types';

import { TelemetryTimeline } from './TelemetryTimeline';
import { TabsContent, TabsList, TabsRoot, TabsTrigger } from './ui/Tabs';

const RightPanelWrapper = styled.aside`
  position: relative;
  height: 100%;
  width: 100%;
  border-left: 1px solid var(--sk-border);
  display: flex;
  flex-direction: column;
  background-color: var(--sk-sidebar-bg);
  color: var(--sk-text);
`;

export type RightPaneView = 'yaml' | 'inspector' | 'telemetry';

interface PipelineRightPaneProps {
  selectedNode: Node<{ label: string; kind: string; params: Record<string, unknown> }> | null;
  selectedNodeDefinition: NodeDefinition | null;
  selectedNodeLabel?: string;
  rightPaneView: RightPaneView;
  setRightPaneView: (view: RightPaneView) => void;
  yamlString: string;
  yamlError?: string;
  onYamlChange?: (yaml: string) => void;
  onParamChange: (nodeId: string, paramName: string, value: unknown) => void;
  onLabelChange: (nodeId: string, newLabel: string) => void;
  nodeDefinitions?: NodeDefinition[];
  readOnly?: boolean;
  yamlReadOnly?: boolean;
  /** When true (Monitor view), non-tunable params are disabled in inspector */
  isMonitorView?: boolean;
  /** Session ID for telemetry (only used in Monitor view) */
  sessionId?: string;
}

export const PipelineRightPane: React.FC<PipelineRightPaneProps> = React.memo(
  ({
    selectedNode,
    selectedNodeDefinition,
    selectedNodeLabel,
    rightPaneView,
    setRightPaneView,
    yamlString,
    yamlError,
    onYamlChange,
    onParamChange,
    onLabelChange,
    nodeDefinitions,
    readOnly = false,
    yamlReadOnly,
    isMonitorView = false,
    sessionId,
  }) => {
    const showTelemetry = isMonitorView && sessionId;

    return (
      <RightPanelWrapper>
        <TabsRoot
          value={rightPaneView}
          onValueChange={(value) => setRightPaneView(value as RightPaneView)}
        >
          <TabsList>
            {selectedNode && <TabsTrigger value="inspector">Inspector</TabsTrigger>}
            <TabsTrigger value="yaml">YAML</TabsTrigger>
            {showTelemetry && <TabsTrigger value="telemetry">Telemetry</TabsTrigger>}
          </TabsList>
          <TabsContent value="inspector">
            {selectedNode && selectedNodeDefinition && (
              <InspectorPane
                node={selectedNode}
                nodeDefinition={selectedNodeDefinition}
                onParamChange={onParamChange}
                onLabelChange={onLabelChange}
                readOnly={readOnly}
                isMonitorView={isMonitorView}
              />
            )}
          </TabsContent>
          <TabsContent value="yaml">
            <YamlPane
              yaml={yamlString}
              onChange={onYamlChange}
              error={yamlError}
              nodeDefinitions={nodeDefinitions}
              highlightNodeLabel={selectedNodeLabel}
              readOnly={yamlReadOnly ?? readOnly}
            />
          </TabsContent>
          {showTelemetry && (
            <TabsContent value="telemetry">
              <TelemetryTimeline sessionId={sessionId} />
            </TabsContent>
          )}
        </TabsRoot>
      </RightPanelWrapper>
    );
  }
);
