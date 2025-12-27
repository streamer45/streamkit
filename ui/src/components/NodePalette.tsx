// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React, { useState } from 'react';

import type { NodeDefinition, PluginType } from '@/types/types';

const PaneWrapper = styled.div`
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
`;

const PaneHeader = styled.div`
  padding: 12px;
  border-bottom: 1px solid var(--sk-border);
  flex-shrink: 0;
`;

const PaneTitle = styled.h3`
  margin: 0 0 4px 0;
  font-size: 14px;
  font-weight: 600;
  color: var(--sk-text);
`;

const PaneSubtitle = styled.p`
  margin: 0;
  font-size: 12px;
  color: var(--sk-text-muted);
`;

const NodeCard = styled.div`
  text-align: center;
  padding: 12px 8px;
  cursor: grab;
  background-color: var(--sk-panel-bg);
  border: 2px dashed var(--sk-primary);
  border-radius: 8px;
  user-select: none;
  font-weight: 600;
  color: var(--sk-primary);
  box-sizing: border-box;
  word-break: break-word;
  overflow-wrap: anywhere;
  transition: none;

  &:active {
    cursor: grabbing;
  }

  &:hover {
    border-style: solid;
    background-color: var(--sk-hover-bg);
    box-shadow: 0 2px 8px var(--sk-shadow);
  }
`;

const PluginBadge = styled.span<{ $pluginType?: 'wasm' | 'native' }>`
  background: ${(props) =>
    props.$pluginType === 'native' ? 'var(--sk-success)' : 'var(--sk-primary)'};
  color: var(--sk-text-white);
  font-size: 9px;
  font-weight: 700;
  padding: 2px 6px;
  border-radius: 999px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
`;

const CategoryCard = styled.div`
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 14px;
  cursor: pointer;
  background: var(--sk-panel-bg);
  border: 2px solid var(--sk-border);
  border-radius: 10px;
  font-weight: 700;
  color: var(--sk-text);
  box-sizing: border-box;
  user-select: none;
  transition: none;

  &:hover {
    background: var(--sk-hover-bg);
    border-color: var(--sk-border-strong);
    box-shadow: 0 4px 12px var(--sk-shadow);
  }
`;

const BackBar = styled.div`
  display: flex;
  gap: 8px;
  align-items: center;
  padding: 8px 0;
`;

const BackButton = styled.button`
  padding: 6px 10px;
  border-radius: 6px;
  border: 1px solid var(--sk-border);
  background: transparent;
  color: var(--sk-text);
  cursor: pointer;
  transition: none;

  &:hover,
  &:focus-visible {
    background: var(--sk-hover-bg);
    border-color: var(--sk-border-strong);
    outline: none;
  }
`;

const ScrollArea = styled.div`
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding-right: 6px;
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 8px;

  /* Firefox */
  scrollbar-width: thin;
  scrollbar-color: var(--sk-border) transparent;

  /* WebKit */
  &::-webkit-scrollbar {
    width: 8px;
  }
  &::-webkit-scrollbar-track {
    background: transparent;
  }
  &::-webkit-scrollbar-thumb {
    background-color: var(--sk-border);
    border-radius: 8px;
    border: 2px solid transparent;
  }
  &::-webkit-scrollbar-thumb:hover {
    background-color: var(--sk-muted);
  }
`;

interface NodePaletteProps {
  nodeDefinitions: NodeDefinition[];
  onDragStart: (event: React.DragEvent, nodeType: string) => void;
  onNodeClick?: (def: NodeDefinition) => void;
  pluginKinds?: Set<string>;
  pluginTypes?: Map<string, PluginType>;
  selectedTop?: string | null;
  onSelectedTopChange?: (top: string | null) => void;
}

/**
 * NodePalette - new behavior:
 * - Initially shows only top-level categories.
 * - Clicking a top-level category shows its child nodes (root + subcategories).
 * - A back button returns to the top-level category list.
 * - Drag & click behavior for individual nodes is preserved.
 */
const NodePalette: React.FC<NodePaletteProps> = ({
  nodeDefinitions,
  onDragStart,
  onNodeClick,
  pluginKinds,
  pluginTypes,
  selectedTop: selectedTopProp,
  onSelectedTopChange,
}) => {
  const [selectedTopState, setSelectedTopState] = useState<string | null>(null);

  // Use controlled prop if provided, otherwise fall back to internal state
  const selectedTop = selectedTopProp !== undefined ? selectedTopProp : selectedTopState;
  const setSelectedTop = (top: string | null) => {
    if (onSelectedTopChange) {
      onSelectedTopChange(top);
    } else {
      setSelectedTopState(top);
    }
  };

  const sortedDefs = React.useMemo(
    () => [...nodeDefinitions].sort((a, b) => a.kind.localeCompare(b.kind)),
    [nodeDefinitions]
  );

  const subtext = onNodeClick ? 'Drag to add · Click for details' : 'Drag to add';

  type Group = { _root: NodeDefinition[]; _subs: Map<string, NodeDefinition[]> };
  const groups = sortedDefs.reduce<Record<string, Group>>((acc, def) => {
    const cats =
      Array.isArray((def as unknown as { categories?: string[] }).categories) &&
      (def as unknown as { categories?: string[] }).categories!.length
        ? (def as unknown as { categories: string[] }).categories
        : ['Uncategorized'];
    const top = cats[0];
    const sub = cats[1] ?? null;
    if (!acc[top]) acc[top] = { _root: [], _subs: new Map() };
    if (sub) {
      const arr = acc[top]._subs.get(sub) ?? [];
      arr.push(def);
      acc[top]._subs.set(sub, arr);
    } else {
      acc[top]._root.push(def);
    }
    return acc;
  }, {});

  const topKeys = Object.keys(groups).sort();

  const renderList = (defs: NodeDefinition[]) => (
    <ul
      style={{
        listStyle: 'none',
        padding: 0,
        margin: 0,
        display: 'flex',
        flexDirection: 'column',
        gap: '8px',
      }}
    >
      {defs.map((def) => (
        <li key={def.kind}>
          <NodeCard
            draggable
            onDragStart={(event) => onDragStart(event, def.kind)}
            onClick={onNodeClick ? () => onNodeClick(def) : undefined}
            role="button"
            aria-label={`Add ${def.kind}`}
          >
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                gap: 6,
                marginBottom: 2,
                userSelect: 'none',
              }}
            >
              <div className="code-font" style={{ fontSize: '14px', userSelect: 'none' }}>
                {def.kind}
              </div>
              {pluginKinds?.has(def.kind) &&
                (() => {
                  const pluginType = pluginTypes?.get(def.kind);
                  return (
                    <PluginBadge $pluginType={pluginType} className="plugin-badge">
                      {pluginType === 'native' ? 'Native' : 'WASM'}
                    </PluginBadge>
                  );
                })()}
            </div>
            <div style={{ fontSize: '10px', color: 'var(--sk-text-muted)', userSelect: 'none' }}>
              {subtext}
            </div>
          </NodeCard>
        </li>
      ))}
    </ul>
  );

  return (
    <PaneWrapper>
      <PaneHeader>
        <PaneTitle>Node Library</PaneTitle>
        <PaneSubtitle>
          {onNodeClick
            ? 'Click a category to browse, drag nodes to canvas'
            : 'Click a category to browse nodes'}
        </PaneSubtitle>
      </PaneHeader>

      <ScrollArea>
        {/* Top-level category view */}
        {!selectedTop && (
          <div style={{ display: 'grid', gridTemplateColumns: '1fr', gap: 8, marginTop: 8 }}>
            {topKeys.map((top) => {
              const g = groups[top];
              const count =
                g._root.length +
                Array.from(g._subs.values()).reduce((acc, arr) => acc + arr.length, 0);
              return (
                <CategoryCard
                  key={top}
                  onClick={() => setSelectedTop(top)}
                  role="button"
                  aria-label={`Open ${top}`}
                >
                  <span>{top}</span>
                  <span style={{ color: 'var(--sk-text-muted)', fontWeight: 600 }}>{count}</span>
                </CategoryCard>
              );
            })}
          </div>
        )}

        {/* Selected category view */}
        {selectedTop && (
          <>
            <BackBar>
              <BackButton onClick={() => setSelectedTop(null)} aria-label="Back to categories">
                ← Back
              </BackButton>
              <div style={{ fontWeight: 700, color: 'var(--sk-text-muted)' }}>{selectedTop}</div>
            </BackBar>

            <div style={{ marginTop: 6 }}>
              {(() => {
                const g = groups[selectedTop];
                if (!g)
                  return (
                    <div style={{ color: 'var(--sk-text-muted)' }}>No nodes in this category</div>
                  );
                const subKeys = Array.from(g._subs.keys()).sort();

                return (
                  <>
                    {g._root.length > 0 && (
                      <div style={{ marginBottom: 8 }}>
                        <div
                          style={{
                            fontWeight: 600,
                            fontSize: 12,
                            color: 'var(--sk-text-muted)',
                            padding: '2px 4px',
                          }}
                        >
                          General
                        </div>
                        <div style={{ paddingLeft: 6, marginTop: 4 }}>{renderList(g._root)}</div>
                      </div>
                    )}

                    {subKeys.map((sub) => (
                      <div key={sub} style={{ marginBottom: 8 }}>
                        <div
                          style={{
                            fontWeight: 600,
                            fontSize: 12,
                            color: 'var(--sk-text-muted)',
                            padding: '2px 4px',
                          }}
                        >
                          {sub}
                        </div>
                        <div style={{ paddingLeft: 6, marginTop: 4 }}>
                          {renderList(g._subs.get(sub) ?? [])}
                        </div>
                      </div>
                    ))}

                    {g._root.length === 0 && subKeys.length === 0 && (
                      <div style={{ color: 'var(--sk-text-muted)' }}>No nodes in this category</div>
                    )}
                  </>
                );
              })()}
            </div>
          </>
        )}
      </ScrollArea>
    </PaneWrapper>
  );
};

export default React.memo(NodePalette);
