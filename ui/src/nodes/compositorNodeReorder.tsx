// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import { ChevronDown, ChevronUp, Eye, EyeOff, GripVertical, Image, Type, X } from 'lucide-react';
import { Reorder } from 'motion/react';
import React, { useCallback } from 'react';

import { SKTooltip } from '@/components/Tooltip';
import type { LayerKind } from '@/hooks/useCompositorLayers';

import {
  LayerListItem,
  OverlayIcon,
  OverlayLabel,
  RemoveButton,
  VisibilityButton,
  type CompositorEntry,
} from './compositorNodeParts';

const ZOrderButton = styled.button`
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  padding: 0;
  border: none;
  border-radius: 2px;
  background: none;
  color: var(--sk-text-muted);
  cursor: pointer;
  pointer-events: auto;
  flex-shrink: 0;
  opacity: 0;
  transition: opacity 0.15s;

  &:hover:not(:disabled) {
    background: var(--sk-overlay-medium);
    color: var(--sk-text);
  }

  &:disabled {
    cursor: not-allowed;
    opacity: 0 !important;
  }
`;

const ZBadge = styled.span`
  font-size: 9px;
  font-variant-numeric: tabular-nums;
  color: var(--sk-text-muted);
  opacity: 0.6;
  min-width: 14px;
  text-align: center;
  flex-shrink: 0;
`;

// Memoised to avoid cascade re-renders during opacity/rotation drags.
const LayerReorderSection: React.FC<{
  entries: CompositorEntry[];
  selectedLayerId: string | null;
  onSelectLayer: (id: string | null) => void;
  onToggleVisibility: (id: string) => void;
  onRemoveText: (id: string) => void;
  onRemoveImage: (id: string) => void;
  onReorderLayers: (entries: Array<{ id: string; kind: LayerKind; zIndex: number }>) => void;
  disabled: boolean;
}> = React.memo(
  ({
    entries,
    selectedLayerId,
    onSelectLayer,
    onToggleVisibility,
    onRemoveText,
    onRemoveImage,
    onReorderLayers,
    disabled,
  }) => {
    const iconForKind = (kind: LayerKind) => {
      switch (kind) {
        case 'text':
          return <Type size={11} />;
        case 'image':
          return <Image size={11} />;
        default:
          return null;
      }
    };

    const handleReorder = useCallback(
      (reordered: CompositorEntry[]) => {
        const maxZ = reordered.length - 1;
        const updates: Array<{ id: string; kind: LayerKind; zIndex: number }> = [];
        for (let i = 0; i < reordered.length; i++) {
          const entry = reordered[i];
          const newZ = maxZ - i;
          if (entry.zIndex !== newZ) {
            updates.push({ id: entry.id, kind: entry.kind, zIndex: newZ });
          }
        }
        if (updates.length > 0) onReorderLayers(updates);
      },
      [onReorderLayers]
    );

    const handleMoveUp = useCallback(
      (entryId: string) => {
        const idx = entries.findIndex((e) => e.id === entryId);
        if (idx <= 0) return;
        const above = entries[idx - 1];
        const current = entries[idx];
        onReorderLayers([
          { id: current.id, kind: current.kind, zIndex: above.zIndex },
          { id: above.id, kind: above.kind, zIndex: current.zIndex },
        ]);
      },
      [entries, onReorderLayers]
    );

    const handleMoveDown = useCallback(
      (entryId: string) => {
        const idx = entries.findIndex((e) => e.id === entryId);
        if (idx < 0 || idx >= entries.length - 1) return;
        const below = entries[idx + 1];
        const current = entries[idx];
        onReorderLayers([
          { id: current.id, kind: current.kind, zIndex: below.zIndex },
          { id: below.id, kind: below.kind, zIndex: current.zIndex },
        ]);
      },
      [entries, onReorderLayers]
    );

    return (
      <Reorder.Group
        axis="y"
        values={entries}
        onReorder={handleReorder}
        as="div"
        style={{ listStyle: 'none', padding: 0, margin: 0 }}
      >
        {entries.map((entry, idx) => (
          <Reorder.Item
            key={entry.id}
            value={entry}
            as="div"
            style={{ listStyle: 'none' }}
            dragListener={!disabled}
          >
            <LayerListItem
              isSelected={entry.id === selectedLayerId}
              isHidden={!entry.visible}
              className="nodrag nopan"
              onClick={() => onSelectLayer(entry.id === selectedLayerId ? null : entry.id)}
            >
              <GripVertical
                size={11}
                style={{
                  color: 'var(--sk-text-muted)',
                  cursor: disabled ? 'not-allowed' : 'grab',
                  flexShrink: 0,
                  opacity: 0.5,
                }}
              />
              <SKTooltip content={entry.visible ? 'Hide layer' : 'Show layer'}>
                <VisibilityButton
                  className="nodrag nopan"
                  onClick={(e) => {
                    e.stopPropagation();
                    onToggleVisibility(entry.id);
                  }}
                >
                  {entry.visible ? <Eye size={12} /> : <EyeOff size={12} />}
                </VisibilityButton>
              </SKTooltip>
              <OverlayIcon>{iconForKind(entry.kind)}</OverlayIcon>
              <ZBadge>{entry.zIndex}</ZBadge>
              <OverlayLabel style={{ fontWeight: entry.id === selectedLayerId ? 600 : 400 }}>
                {entry.label}
              </OverlayLabel>
              <SKTooltip content="Move up">
                <ZOrderButton
                  disabled={disabled || idx === 0}
                  className="nodrag nopan layer-z-btn"
                  onClick={(e) => {
                    e.stopPropagation();
                    handleMoveUp(entry.id);
                  }}
                >
                  <ChevronUp size={12} />
                </ZOrderButton>
              </SKTooltip>
              <SKTooltip content="Move down">
                <ZOrderButton
                  disabled={disabled || idx === entries.length - 1}
                  className="nodrag nopan layer-z-btn"
                  onClick={(e) => {
                    e.stopPropagation();
                    handleMoveDown(entry.id);
                  }}
                >
                  <ChevronDown size={12} />
                </ZOrderButton>
              </SKTooltip>
              {(entry.kind === 'text' || entry.kind === 'image') && (
                <SKTooltip content="Remove layer">
                  <RemoveButton
                    disabled={disabled}
                    className="nodrag nopan layer-remove-btn"
                    onClick={(e) => {
                      e.stopPropagation();
                      if (entry.kind === 'text') onRemoveText(entry.id);
                      else onRemoveImage(entry.id);
                    }}
                  >
                    <X size={12} />
                  </RemoveButton>
                </SKTooltip>
              )}
            </LayerListItem>
          </Reorder.Item>
        ))}
      </Reorder.Group>
    );
  }
);
LayerReorderSection.displayName = 'LayerReorderSection';

export default LayerReorderSection;
