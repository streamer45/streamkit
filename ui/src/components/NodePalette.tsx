// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import { Search, X } from 'lucide-react';
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

const SearchWrapper = styled.div`
  position: relative;
  margin-top: 8px;
`;

const SearchIcon = styled.div`
  position: absolute;
  left: 8px;
  top: 50%;
  transform: translateY(-50%);
  color: var(--sk-text-muted);
  pointer-events: none;
  display: flex;
  align-items: center;
`;

const ClearButton = styled.button`
  position: absolute;
  right: 6px;
  top: 50%;
  transform: translateY(-50%);
  background: none;
  border: none;
  color: var(--sk-text-muted);
  cursor: pointer;
  padding: 2px;
  display: flex;
  align-items: center;
  border-radius: 4px;

  &:hover {
    color: var(--sk-text);
    background: var(--sk-hover-bg);
  }
`;

const SearchInput = styled.input`
  width: 100%;
  padding: 6px 28px 6px 30px;
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  background: var(--sk-panel-bg);
  color: var(--sk-text);
  font-size: 12px;
  outline: none;
  box-sizing: border-box;

  &::placeholder {
    color: var(--sk-text-muted);
  }

  &:focus {
    border-color: var(--sk-primary);
  }
`;

const FilterChipsRow = styled.div`
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  margin-top: 6px;
`;

const FilterChip = styled.button<{ $active: boolean }>`
  padding: 2px 8px;
  border-radius: 999px;
  border: 1px solid ${(props) => (props.$active ? 'var(--sk-primary)' : 'var(--sk-border)')};
  background: ${(props) => (props.$active ? 'var(--sk-primary)' : 'transparent')};
  color: ${(props) => (props.$active ? 'var(--sk-text-white)' : 'var(--sk-text-muted)')};
  font-size: 10px;
  font-weight: 600;
  cursor: pointer;
  transition: none;
  text-transform: capitalize;
  user-select: none;

  &:hover {
    border-color: var(--sk-primary);
  }
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

const CategoryBreadcrumb = styled.span`
  font-size: 10px;
  color: var(--sk-text-muted);
  font-weight: 400;
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

const PLUGIN_FILTER = '__plugin__';

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
 * NodePalette — browsable node library with search, quick filters,
 * and category drill-down.
 *
 * - Search/filter active → flat filtered list with category breadcrumbs.
 * - Otherwise → top-level categories, click to drill into child nodes.
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
  const [searchQuery, setSearchQuery] = useState('');
  const [activeFilters, setActiveFilters] = useState<Set<string>>(new Set());

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

  // Derive available top-level categories and whether plugins exist
  const { topCategories, hasPlugins } = React.useMemo(() => {
    const cats = new Set<string>();
    let foundPlugin = false;
    for (const def of sortedDefs) {
      const top = def.categories.length > 0 ? def.categories[0] : 'Uncategorized';
      cats.add(top);
      if (pluginKinds?.has(def.kind)) foundPlugin = true;
    }
    return { topCategories: [...cats].sort(), hasPlugins: foundPlugin };
  }, [sortedDefs, pluginKinds]);

  const isSearchOrFilterActive = searchQuery.trim().length > 0 || activeFilters.size > 0;

  // Filter nodes by search query and active filter chips
  const filteredDefs = React.useMemo(() => {
    if (!isSearchOrFilterActive) return sortedDefs;

    const query = searchQuery.toLowerCase().trim();

    return sortedDefs.filter((def) => {
      // Apply category/plugin filter chips
      if (activeFilters.size > 0) {
        const top = def.categories.length > 0 ? def.categories[0] : 'Uncategorized';
        const isPlugin = pluginKinds?.has(def.kind) ?? false;
        const matchesFilter =
          activeFilters.has(top) || (activeFilters.has(PLUGIN_FILTER) && isPlugin);
        if (!matchesFilter) return false;
      }

      // Apply text search
      if (query) {
        const kindMatch = def.kind.toLowerCase().includes(query);
        const descMatch = def.description?.toLowerCase().includes(query) ?? false;
        const catMatch = def.categories.some((c) => c.toLowerCase().includes(query));
        if (!kindMatch && !descMatch && !catMatch) return false;
      }

      return true;
    });
  }, [sortedDefs, searchQuery, activeFilters, pluginKinds, isSearchOrFilterActive]);

  const toggleFilter = React.useCallback((filter: string) => {
    setActiveFilters((prev) => {
      const next = new Set(prev);
      if (next.has(filter)) {
        next.delete(filter);
      } else {
        next.add(filter);
      }
      return next;
    });
  }, []);

  const subtext = onNodeClick ? 'Drag to add · Click for details' : 'Drag to add';

  type Group = { _root: NodeDefinition[]; _subs: Map<string, NodeDefinition[]> };
  const { groups, topKeys } = React.useMemo(() => {
    const g = sortedDefs.reduce<Record<string, Group>>((acc, def) => {
      const cats =
        def.categories.length > 0 ? def.categories : (['Uncategorized'] as readonly string[]);
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
    return { groups: g, topKeys: Object.keys(g).sort() };
  }, [sortedDefs]);

  const renderNodeCard = (def: NodeDefinition, showCategory?: boolean) => (
    <NodeCard
      draggable
      onDragStart={(event) => onDragStart(event, def.kind)}
      onClick={onNodeClick ? () => onNodeClick(def) : undefined}
      role="button"
      aria-label={`Add ${def.kind}`}
    >
      {showCategory && def.categories.length > 0 && (
        <CategoryBreadcrumb>{def.categories.join(' › ')}</CategoryBreadcrumb>
      )}
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
      {def.description && (
        <div
          style={{
            fontSize: '11px',
            color: 'var(--sk-text-muted)',
            userSelect: 'none',
            fontWeight: 400,
            marginBottom: 2,
          }}
        >
          {def.description}
        </div>
      )}
      <div style={{ fontSize: '10px', color: 'var(--sk-text-muted)', userSelect: 'none' }}>
        {subtext}
      </div>
    </NodeCard>
  );

  const renderList = (defs: NodeDefinition[], showCategory?: boolean) => (
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
        <li key={def.kind}>{renderNodeCard(def, showCategory)}</li>
      ))}
    </ul>
  );

  // Build filter chip options: top-level categories + "plugin" if plugins exist
  const filterChips = React.useMemo(() => {
    const chips = topCategories.map((cat) => ({ key: cat, label: cat }));
    if (hasPlugins) {
      chips.push({ key: PLUGIN_FILTER, label: 'Plugin' });
    }
    return chips;
  }, [topCategories, hasPlugins]);

  return (
    <PaneWrapper>
      <PaneHeader>
        <PaneTitle>Node Library</PaneTitle>
        <PaneSubtitle>
          {onNodeClick
            ? 'Click a category to browse, drag nodes to canvas'
            : 'Click a category to browse nodes'}
        </PaneSubtitle>
        <SearchWrapper>
          <SearchIcon>
            <Search size={14} />
          </SearchIcon>
          <SearchInput
            type="text"
            placeholder="Search nodes…"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            aria-label="Search nodes"
            data-testid="node-search-input"
          />
          {searchQuery && (
            <ClearButton onClick={() => setSearchQuery('')} aria-label="Clear search">
              <X size={12} />
            </ClearButton>
          )}
        </SearchWrapper>
        {filterChips.length > 0 && (
          <FilterChipsRow data-testid="filter-chips">
            {filterChips.map((chip) => (
              <FilterChip
                key={chip.key}
                $active={activeFilters.has(chip.key)}
                onClick={() => toggleFilter(chip.key)}
                aria-label={`Filter by ${chip.label}`}
                aria-pressed={activeFilters.has(chip.key)}
              >
                {chip.label}
              </FilterChip>
            ))}
          </FilterChipsRow>
        )}
      </PaneHeader>

      <ScrollArea>
        {/* Search/filter results — flat list with category breadcrumbs */}
        {isSearchOrFilterActive && (
          <>
            {filteredDefs.length > 0 ? (
              <>
                <div
                  style={{
                    fontSize: 11,
                    color: 'var(--sk-text-muted)',
                    padding: '4px 4px 0',
                  }}
                >
                  {filteredDefs.length} {filteredDefs.length === 1 ? 'node' : 'nodes'} found
                </div>
                {renderList(filteredDefs, true)}
              </>
            ) : (
              <div
                style={{
                  color: 'var(--sk-text-muted)',
                  textAlign: 'center',
                  padding: '24px 12px',
                  fontSize: 13,
                }}
              >
                No nodes match your search
              </div>
            )}
          </>
        )}

        {/* Top-level category view */}
        {!isSearchOrFilterActive && !selectedTop && (
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
        {!isSearchOrFilterActive && selectedTop && (
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
