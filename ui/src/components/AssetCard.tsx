// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import { Code2, Image as ImageIcon, Music2, Type } from 'lucide-react';
import React from 'react';

import type { SlintAsset } from '@/services/slintAssets';
import type { AudioAsset, FontAsset, ImageAsset } from '@/types/generated/api-types';

// ── Public types ────────────────────────────────────────────────────────────

export type AssetType = 'audio' | 'image' | 'font' | 'slint';

export type UnifiedAsset =
  | { type: 'audio'; asset: AudioAsset }
  | { type: 'image'; asset: ImageAsset }
  | { type: 'font'; asset: FontAsset }
  | { type: 'slint'; asset: SlintAsset };

interface AssetCardProps {
  item: UnifiedAsset;
  onDelete?: (item: UnifiedAsset) => void;
  canDelete: boolean;
  onDragStart?: (event: React.DragEvent, item: UnifiedAsset) => void;
}

// ── Styled components ───────────────────────────────────────────────────────

const CardWrapper = styled.div`
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 12px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  color: var(--sk-text);
  position: relative;
  cursor: grab;
  transition: none;

  &:hover {
    background: var(--sk-hover-bg);
    border-color: var(--sk-border-strong);
  }

  &:hover .delete-button {
    opacity: 1;
  }

  &:active {
    cursor: grabbing;
  }
`;

const CardHeader = styled.div`
  display: flex;
  align-items: flex-start;
  gap: 10px;
`;

const IconWrapper = styled.div<{ $bg: string; $fg: string }>`
  display: flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  background: ${({ $bg }) => $bg};
  border-radius: 6px;
  color: ${({ $fg }) => $fg};
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

const FormatBadge = styled.span<{ $color: string }>`
  font-size: 9px;
  font-weight: 700;
  padding: 2px 6px;
  border-radius: 4px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  flex-shrink: 0;
  background: ${({ $color }) => $color};
  color: white;
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

const CardMeta = styled.div`
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin-top: 2px;
`;

const MetaRow = styled.div`
  font-size: 11px;
  color: var(--sk-text-muted);
  line-height: 1.4;
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

// ── Helpers ─────────────────────────────────────────────────────────────────

export function formatFileSize(bytes: number | bigint): string {
  const numBytes = typeof bytes === 'bigint' ? Number(bytes) : bytes;
  if (numBytes < 1024) return `${numBytes} B`;
  if (numBytes < 1024 * 1024) return `${(numBytes / 1024).toFixed(1)} KB`;
  return `${(numBytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** Per-audio-format colors (matches AudioAssetCard) */
function audioFormatColor(format: string): string {
  switch (format.toLowerCase()) {
    case 'flac':
      return '#3b82f6';
    case 'opus':
      return '#10b981';
    case 'mp3':
      return '#f59e0b';
    case 'wav':
      return '#6b7280';
    case 'ogg':
      return '#ec4899';
    default:
      return '#6b7280';
  }
}

function badgeColor(item: UnifiedAsset): string {
  switch (item.type) {
    case 'audio':
      return audioFormatColor(item.asset.format);
    case 'image':
      return '#3b82f6';
    case 'font':
      return '#8b5cf6';
    case 'slint':
      return '#10b981';
  }
}

function iconForType(type: AssetType) {
  switch (type) {
    case 'audio':
      return <Music2 size={18} />;
    case 'image':
      return <ImageIcon size={18} />;
    case 'font':
      return <Type size={18} />;
    case 'slint':
      return <Code2 size={18} />;
  }
}

function iconColors(type: AssetType): { bg: string; fg: string } {
  switch (type) {
    case 'audio':
      return { bg: 'var(--sk-primary-alpha)', fg: 'var(--sk-primary)' };
    case 'image':
      return { bg: 'rgba(59,130,246,0.15)', fg: '#3b82f6' };
    case 'font':
      return { bg: 'rgba(139,92,246,0.15)', fg: '#8b5cf6' };
    case 'slint':
      return { bg: 'rgba(16,185,129,0.15)', fg: '#10b981' };
  }
}

function dragTitle(item: UnifiedAsset): string {
  switch (item.type) {
    case 'audio':
      return `Drag to add file_reader node for ${item.asset.name}`;
    case 'image':
      return `Drag to add image asset: ${item.asset.name}`;
    case 'font':
      return `Font asset: ${item.asset.name}`;
    case 'slint':
      return `Drag to add slint node for ${item.asset.name}`;
  }
}

// ── Component ───────────────────────────────────────────────────────────────

export const AssetCard = React.memo(function AssetCard({
  item,
  onDelete,
  canDelete,
  onDragStart,
}: AssetCardProps) {
  const handleDragStart = (event: React.DragEvent) => {
    onDragStart?.(event, item);
  };

  const handleDelete = (e: React.MouseEvent) => {
    e.stopPropagation();
    e.preventDefault();
    onDelete?.(item);
  };

  const { bg, fg } = iconColors(item.type);
  const isSystem = item.asset.is_system;
  const showDelete = canDelete && !isSystem && !!onDelete;

  return (
    <CardWrapper draggable onDragStart={handleDragStart} title={dragTitle(item)}>
      {showDelete && (
        <DeleteButton className="delete-button" onClick={handleDelete}>
          Delete
        </DeleteButton>
      )}

      <CardHeader>
        <IconWrapper $bg={bg} $fg={fg}>
          {iconForType(item.type)}
        </IconWrapper>
        <CardContent>
          <AssetName>
            {item.asset.name}
            <FormatBadge $color={badgeColor(item)}>{item.asset.format}</FormatBadge>
            {isSystem && <SystemBadge>System</SystemBadge>}
          </AssetName>
          <CardMeta>
            <MetaRow>
              {formatFileSize(item.asset.size_bytes)}
              {item.type === 'image' && ` · ${item.asset.width}×${item.asset.height}`}
            </MetaRow>
          </CardMeta>
        </CardContent>
      </CardHeader>

      {item.type === 'audio' && item.asset.license && (
        <LicenseInfo>{item.asset.license}</LicenseInfo>
      )}
    </CardWrapper>
  );
});
