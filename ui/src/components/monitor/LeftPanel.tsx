// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Left sidebar panel for the Monitor View.
 *
 * Contains the session list (with search) and a "Nodes Library" tab
 * for drag-and-drop node insertion.
 */

import { useState } from 'react';
import type React from 'react';

import {
  LeftPanelAside,
  SessionsContainer,
  SessionSearchInput,
  SearchWrapper,
  SessionListWrapper,
  LoadingText,
  SessionList,
  EmptyStateText,
  NodesLibraryContainer,
} from '@/components/monitor/MonitorView.styles';
import { SessionItem } from '@/components/monitor/SessionItem';
import NodePalette from '@/components/NodePalette';
import { TabsContent, TabsList, TabsRoot, TabsTrigger } from '@/components/ui/Tabs';
import type { NodeDefinition } from '@/types/types';

// Props

interface LeftPanelProps {
  isLoadingSessions: boolean;
  sessions: { id: string; name: string | null; created_at: string }[];
  selectedSessionId: string | null;
  onSessionClick: (id: string) => void;
  onSessionDelete: (id: string) => void;
  nodeDefinitions: NodeDefinition[];
  onDragStart: (event: React.DragEvent, nodeType: string) => void;
  pluginKinds: Set<string>;
  pluginTypes: Map<string, 'wasm' | 'native'>;
}

// Component

export const LeftPanel = ({
  isLoadingSessions,
  sessions,
  selectedSessionId,
  onSessionClick,
  onSessionDelete,
  nodeDefinitions,
  onDragStart,
  pluginKinds,
  pluginTypes,
}: LeftPanelProps) => {
  const [activeTab, setActiveTab] = useState<'sessions' | 'add'>('sessions');
  const [searchQuery, setSearchQuery] = useState('');

  const query = searchQuery.trim().toLowerCase();
  const filteredSessions = !query
    ? sessions
    : sessions.filter(
        (session) =>
          session.id.toLowerCase().includes(query) ||
          (session.name && session.name.toLowerCase().includes(query))
      );

  return (
    <LeftPanelAside>
      <TabsRoot
        value={activeTab}
        onValueChange={(value) => setActiveTab(value as 'sessions' | 'add')}
      >
        <TabsList>
          <TabsTrigger value="sessions">Sessions</TabsTrigger>
          <TabsTrigger value="add" disabled={!selectedSessionId}>
            Nodes Library
          </TabsTrigger>
        </TabsList>

        <TabsContent value="sessions">
          <SessionsContainer data-testid="sessions-list">
            {isLoadingSessions ? (
              <LoadingText>Loading sessions...</LoadingText>
            ) : sessions.length === 0 ? (
              <EmptyStateText>No active sessions</EmptyStateText>
            ) : (
              <>
                {sessions.length >= 5 && (
                  <SearchWrapper>
                    <SessionSearchInput
                      type="text"
                      placeholder="Search sessions..."
                      value={searchQuery}
                      onChange={(e) => setSearchQuery(e.target.value)}
                    />
                  </SearchWrapper>
                )}
                <SessionListWrapper>
                  {filteredSessions.length === 0 ? (
                    <EmptyStateText>No matching sessions</EmptyStateText>
                  ) : (
                    <SessionList>
                      {filteredSessions.map((session) => (
                        <li key={session.id}>
                          <SessionItem
                            session={session}
                            isActive={selectedSessionId === session.id}
                            onClick={onSessionClick}
                            onDelete={onSessionDelete}
                          />
                        </li>
                      ))}
                    </SessionList>
                  )}
                </SessionListWrapper>
              </>
            )}
          </SessionsContainer>
        </TabsContent>

        <TabsContent value="add">
          <NodesLibraryContainer>
            {selectedSessionId ? (
              nodeDefinitions.length === 0 ? (
                <EmptyStateText>Loading node definitions…</EmptyStateText>
              ) : (
                <NodePalette
                  nodeDefinitions={nodeDefinitions}
                  onDragStart={onDragStart}
                  pluginKinds={pluginKinds}
                  pluginTypes={pluginTypes}
                />
              )
            ) : (
              <EmptyStateText>Select a session to add nodes</EmptyStateText>
            )}
          </NodesLibraryContainer>
        </TabsContent>
      </TabsRoot>
    </LeftPanelAside>
  );
};
