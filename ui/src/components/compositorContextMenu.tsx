// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Right-click context menu for compositor canvas layers.
 *
 * Provides quick actions for layer ordering (bring to front / send to back)
 * and deletion (text/image overlays only). Positioned at the cursor and
 * dismissed on click-outside, Escape, or scroll.
 */

import styled from '@emotion/styled';
import React, { useCallback, useEffect, useRef } from 'react';

import type { LayerKind } from '@/hooks/useCompositorLayers';
import type { CompositorEntry } from '@/nodes/compositorNodeParts';

// ── Styled components ───────────────────────────────────────────────────────

const MenuOverlay = styled.div`
  position: fixed;
  inset: 0;
  z-index: 9999;
`;

const MenuContainer = styled.div`
  position: fixed;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 4px;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.15);
  z-index: 10000;
  min-width: 140px;
  overflow: hidden;
`;

const MenuItem = styled.button`
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 6px 10px;
  border: none;
  background: none;
  color: var(--sk-text);
  cursor: pointer;
  font-size: 11px;
  text-align: left;
  pointer-events: auto;

  &:hover {
    background: var(--sk-overlay-medium);
  }
`;

const MenuDivider = styled.div`
  height: 1px;
  background: var(--sk-border);
  margin: 2px 0;
`;

// ── Types ───────────────────────────────────────────────────────────────────

export interface ContextMenuState {
  layerId: string;
  layerKind: LayerKind;
  x: number;
  y: number;
}

export interface CompositorContextMenuProps {
  menu: ContextMenuState;
  entries: CompositorEntry[];
  onReorderLayers: (entries: Array<{ id: string; kind: LayerKind; zIndex: number }>) => void;
  onRemoveText: (id: string) => void;
  onRemoveImage: (id: string) => void;
  onClose: () => void;
}

// ── Component ───────────────────────────────────────────────────────────────

export const CompositorContextMenu: React.FC<CompositorContextMenuProps> = ({
  menu,
  entries,
  onReorderLayers,
  onRemoveText,
  onRemoveImage,
  onClose,
}) => {
  const menuRef = useRef<HTMLDivElement>(null);

  // Close on Escape or scroll
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    const handleScroll = () => onClose();
    document.addEventListener('keydown', handleKeyDown);
    document.addEventListener('scroll', handleScroll, true);
    return () => {
      document.removeEventListener('keydown', handleKeyDown);
      document.removeEventListener('scroll', handleScroll, true);
    };
  }, [onClose]);

  const handleBringToFront = useCallback(() => {
    const maxZ = entries.reduce((max, e) => Math.max(max, e.zIndex), 0);
    onReorderLayers([{ id: menu.layerId, kind: menu.layerKind, zIndex: maxZ + 1 }]);
    onClose();
  }, [entries, menu.layerId, menu.layerKind, onReorderLayers, onClose]);

  const handleSendToBack = useCallback(() => {
    // Shift all other layers up by 1 and set this one to 0
    const updates: Array<{ id: string; kind: LayerKind; zIndex: number }> = [];
    for (const entry of entries) {
      if (entry.id === menu.layerId) {
        updates.push({ id: entry.id, kind: entry.kind, zIndex: 0 });
      } else {
        updates.push({ id: entry.id, kind: entry.kind, zIndex: entry.zIndex + 1 });
      }
    }
    onReorderLayers(updates);
    onClose();
  }, [entries, menu.layerId, onReorderLayers, onClose]);

  const handleDelete = useCallback(() => {
    if (menu.layerKind === 'text') onRemoveText(menu.layerId);
    else if (menu.layerKind === 'image') onRemoveImage(menu.layerId);
    onClose();
  }, [menu.layerId, menu.layerKind, onRemoveText, onRemoveImage, onClose]);

  const canDelete = menu.layerKind === 'text' || menu.layerKind === 'image';

  return (
    <MenuOverlay onPointerDown={onClose}>
      <MenuContainer
        ref={menuRef}
        style={{ left: menu.x, top: menu.y }}
        onPointerDown={(e) => e.stopPropagation()}
      >
        <MenuItem onClick={handleBringToFront}>Bring to Front</MenuItem>
        <MenuItem onClick={handleSendToBack}>Send to Back</MenuItem>
        {canDelete && (
          <>
            <MenuDivider />
            <MenuItem onClick={handleDelete}>Delete</MenuItem>
          </>
        )}
      </MenuContainer>
    </MenuOverlay>
  );
};

CompositorContextMenu.displayName = 'CompositorContextMenu';
