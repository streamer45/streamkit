// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import { Music2 } from 'lucide-react';

import type { AudioAsset } from '@/types/generated/api-types';

interface AudioAssetCardProps {
  asset: AudioAsset;
  onDelete?: (asset: AudioAsset) => void;
  canDelete: boolean;
  onDragStart?: (event: React.DragEvent, asset: AudioAsset) => void;
}

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

const IconWrapper = styled.div`
  display: flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  background: var(--sk-primary-alpha);
  border-radius: 6px;
  color: var(--sk-primary);
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

const FormatBadge = styled.span<{ format: string }>`
  font-size: 9px;
  font-weight: 700;
  padding: 2px 6px;
  border-radius: 4px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  flex-shrink: 0;

  ${(props) => {
    switch (props.format.toLowerCase()) {
      case 'flac':
        return `
          background: #3b82f6;
          color: white;
        `;
      case 'opus':
        return `
          background: #10b981;
          color: white;
        `;
      case 'mp3':
        return `
          background: #f59e0b;
          color: white;
        `;
      case 'wav':
        return `
          background: #6b7280;
          color: white;
        `;
      case 'ogg':
        return `
          background: #ec4899;
          color: white;
        `;
      default:
        return `
          background: var(--sk-border-strong);
          color: var(--sk-text);
        `;
    }
  }}
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

/**
 * Format file size in human-readable format
 */
function formatFileSize(bytes: number | bigint): string {
  const numBytes = typeof bytes === 'bigint' ? Number(bytes) : bytes;
  if (numBytes < 1024) return `${numBytes} B`;
  if (numBytes < 1024 * 1024) return `${(numBytes / 1024).toFixed(1)} KB`;
  return `${(numBytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function AudioAssetCard({ asset, onDelete, canDelete, onDragStart }: AudioAssetCardProps) {
  const handleDragStart = (event: React.DragEvent) => {
    if (onDragStart) {
      onDragStart(event, asset);
    }
  };

  const handleDelete = (e: React.MouseEvent) => {
    e.stopPropagation();
    e.preventDefault();
    if (onDelete) {
      onDelete(asset);
    }
  };

  return (
    <CardWrapper
      draggable
      onDragStart={handleDragStart}
      title={`Drag to add file_reader node for ${asset.name}`}
    >
      {canDelete && !asset.is_system && onDelete && (
        <DeleteButton className="delete-button" onClick={handleDelete}>
          Delete
        </DeleteButton>
      )}

      <CardHeader>
        <IconWrapper>
          <Music2 size={18} />
        </IconWrapper>
        <CardContent>
          <AssetName>
            {asset.name}
            <FormatBadge format={asset.format}>{asset.format}</FormatBadge>
            {asset.is_system && <SystemBadge>System</SystemBadge>}
          </AssetName>
          <CardMeta>
            <MetaRow>{formatFileSize(asset.size_bytes)}</MetaRow>
          </CardMeta>
        </CardContent>
      </CardHeader>

      {asset.license && <LicenseInfo>{asset.license}</LicenseInfo>}
    </CardWrapper>
  );
}
