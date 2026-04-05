// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import { Code2, File, Image, Music2, Type } from 'lucide-react';

import type {
  AssetTypeInfo,
  AudioAsset,
  FontAsset,
  ImageAsset,
  PluginAsset,
} from '@/types/generated/api-types';

// ── Types ────────────────────────────────────────────────────────────────────

/** Discriminated union so each card knows how to render its content. */
export type UnifiedAsset =
  | { type: 'audio'; asset: AudioAsset }
  | { type: 'image'; asset: ImageAsset }
  | { type: 'font'; asset: FontAsset }
  | { type: 'plugin'; asset: PluginAsset; typeInfo: AssetTypeInfo };

interface AssetCardProps {
  item: UnifiedAsset;
  onDelete?: (item: UnifiedAsset) => void;
  canDelete: boolean;
  /** Called when the user drags the card.  When absent the card is not draggable. */
  onDragStart?: (event: React.DragEvent, item: UnifiedAsset) => void;
  /** Override the default draggable check (`!!onDragStart`). */
  isDraggable?: boolean;
}

// ── Helpers ──────────────────────────────────────────────────────────────────

function formatFileSize(bytes: number | bigint): string {
  const numBytes = typeof bytes === 'bigint' ? Number(bytes) : bytes;
  if (numBytes < 1024) return `${numBytes} B`;
  if (numBytes < 1024 * 1024) return `${(numBytes / 1024).toFixed(1)} KB`;
  return `${(numBytes / (1024 * 1024)).toFixed(1)} MB`;
}

function getIcon(item: UnifiedAsset) {
  switch (item.type) {
    case 'audio':
      return <Music2 size={18} />;
    case 'image':
      return <Image size={18} />;
    case 'font':
      return <Type size={18} />;
    case 'plugin': {
      const hint = item.typeInfo.icon_hint;
      if (hint === 'code') return <Code2 size={18} />;
      if (hint === 'music') return <Music2 size={18} />;
      if (hint === 'image') return <Image size={18} />;
      if (hint === 'type') return <Type size={18} />;
      return <File size={18} />;
    }
  }
}

function getIconColor(item: UnifiedAsset): string {
  switch (item.type) {
    case 'audio':
      return 'var(--sk-primary)';
    case 'image':
      return '#10b981';
    case 'font':
      return '#8b5cf6';
    case 'plugin':
      return '#f59e0b';
  }
}

function getIconBg(item: UnifiedAsset): string {
  switch (item.type) {
    case 'audio':
      return 'var(--sk-primary-alpha)';
    case 'image':
      return 'rgba(16, 185, 129, 0.15)';
    case 'font':
      return 'rgba(139, 92, 246, 0.15)';
    case 'plugin':
      return 'rgba(245, 158, 11, 0.15)';
  }
}

function getName(item: UnifiedAsset): string {
  return item.asset.name;
}

function getFormat(item: UnifiedAsset): string {
  return item.asset.format;
}

function getSize(item: UnifiedAsset): number {
  return item.asset.size_bytes;
}

function isSystem(item: UnifiedAsset): boolean {
  return item.asset.is_system;
}

function getLicense(item: UnifiedAsset): string | undefined {
  if (item.type === 'audio' && item.asset.license) {
    return item.asset.license;
  }
  return undefined;
}

// ── Styled components ────────────────────────────────────────────────────────

const CardWrapper = styled.div<{ $draggable: boolean }>`
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 12px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  color: var(--sk-text);
  position: relative;
  cursor: ${({ $draggable }) => ($draggable ? 'grab' : 'default')};
  transition: none;

  &:hover {
    background: var(--sk-hover-bg);
    border-color: var(--sk-border-strong);
  }

  &:hover .delete-button {
    opacity: 1;
  }

  &:active {
    cursor: ${({ $draggable }) => ($draggable ? 'grabbing' : 'default')};
  }
`;

const CardHeader = styled.div`
  display: flex;
  align-items: flex-start;
  gap: 10px;
`;

const IconWrapper = styled.div<{ $bg: string; $color: string }>`
  display: flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  background: ${({ $bg }) => $bg};
  border-radius: 6px;
  color: ${({ $color }) => $color};
  flex-shrink: 0;
`;

const CardContent = styled.div`
  flex: 1;
  min-width: 0;
`;

const AssetName = styled.div`
  font-weight: 600;
  font-size: 13px;
  color: var(--sk-text);
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
`;

const FormatBadge = styled.span`
  font-size: 9px;
  font-weight: 700;
  padding: 2px 6px;
  border-radius: 4px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  flex-shrink: 0;
  background: var(--sk-border-strong);
  color: var(--sk-text);
`;

const SystemBadge = styled.span`
  background: var(--sk-primary);
  color: var(--sk-text-white);
  font-size: 9px;
  font-weight: 700;
  padding: 2px 6px;
  border-radius: 999px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  flex-shrink: 0;
`;

const MetaRow = styled.div`
  font-size: 11px;
  color: var(--sk-text-muted);
  line-height: 1.4;
  margin-top: 2px;
`;

const LicenseInfo = styled.div`
  font-size: 10px;
  color: var(--sk-text-muted);
  line-height: 1.3;
  padding-top: 4px;
  border-top: 1px solid var(--sk-border);
  white-space: pre-line;
`;

const DeleteButton = styled.button`
  position: absolute;
  top: 8px;
  right: 8px;
  padding: 4px 8px;
  background: var(--sk-danger);
  color: white;
  border: none;
  border-radius: 4px;
  font-size: 11px;
  font-weight: 600;
  cursor: pointer;
  opacity: 0;
  transition: opacity 0.15s;
  z-index: 10;
  pointer-events: auto;

  &:hover {
    opacity: 1 !important;
    background: var(--sk-danger-hover);
  }

  &:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
`;

// ── Component ────────────────────────────────────────────────────────────────

export function AssetCard({ item, onDelete, canDelete, onDragStart, isDraggable }: AssetCardProps) {
  const canDrag = isDraggable ?? !!onDragStart;

  const handleDragStart = (event: React.DragEvent) => {
    onDragStart?.(event, item);
  };

  const handleDelete = (e: React.MouseEvent) => {
    e.stopPropagation();
    e.preventDefault();
    onDelete?.(item);
  };

  const license = getLicense(item);

  return (
    <CardWrapper
      $draggable={canDrag}
      draggable={canDrag}
      onDragStart={canDrag ? handleDragStart : undefined}
      title={canDrag ? `Drag to add node for ${getName(item)}` : getName(item)}
    >
      {canDelete && !isSystem(item) && onDelete && (
        <DeleteButton className="delete-button" onClick={handleDelete}>
          Delete
        </DeleteButton>
      )}

      <CardHeader>
        <IconWrapper $bg={getIconBg(item)} $color={getIconColor(item)}>
          {getIcon(item)}
        </IconWrapper>
        <CardContent>
          <AssetName>
            {getName(item)}
            <FormatBadge>{getFormat(item)}</FormatBadge>
            {isSystem(item) && <SystemBadge>System</SystemBadge>}
          </AssetName>
          <MetaRow>{formatFileSize(getSize(item))}</MetaRow>
        </CardContent>
      </CardHeader>

      {license && <LicenseInfo>{license}</LicenseInfo>}
    </CardWrapper>
  );
}
