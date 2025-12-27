// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import { Upload } from 'lucide-react';
import { useState, useCallback } from 'react';

import { useToast } from '@/context/ToastContext';
import { usePermissions } from '@/hooks/usePermissions';
import { useAudioAssets, useUploadAudioAsset, useDeleteAudioAsset } from '@/services/assets';
import type { AudioAsset } from '@/types/generated/api-types';

import { AudioAssetCard } from './AudioAssetCard';
import ConfirmModal from './ConfirmModal';
import { UploadDropZone } from './UploadDropZone';

const LibraryWrapper = styled.div`
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--sk-sidebar-bg);
  color: var(--sk-text);
  overflow: hidden;
`;

const LibraryHeader = styled.div`
  padding: 12px;
  border-bottom: 1px solid var(--sk-border);
  flex-shrink: 0;
`;

const HeaderRow = styled.div`
  display: flex;
  justify-content: flex-start;
  align-items: center;
  margin-bottom: 8px;
`;

const LibraryTitle = styled.h3`
  margin: 0;
  font-size: 14px;
  font-weight: 600;
  color: var(--sk-text);
`;

const FilterRow = styled.div`
  display: flex;
  gap: 8px;
`;

const SearchInput = styled.input`
  flex: 1;
  padding: 6px 10px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  color: var(--sk-text);
  font-size: 12px;

  &:focus {
    outline: none;
    border-color: var(--sk-primary);
  }

  &::placeholder {
    color: var(--sk-text-muted);
  }
`;

const FilterSelect = styled.select`
  padding: 6px 10px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  color: var(--sk-text);
  font-size: 12px;
  cursor: pointer;

  &:focus {
    outline: none;
    border-color: var(--sk-primary);
  }
`;

const AssetsList = styled.div`
  flex: 1;
  overflow-y: auto;
  padding: 8px;
  display: flex;
  flex-direction: column;
  gap: 8px;
`;

const SectionHeader = styled.div`
  font-size: 11px;
  font-weight: 700;
  color: var(--sk-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.05em;
  margin-top: 8px;
  margin-bottom: 4px;
`;

const LoadingState = styled.div`
  padding: 12px;
  text-align: center;
  font-size: 12px;
  color: var(--sk-text-muted);
`;

const ErrorState = styled.div`
  padding: 12px;
  text-align: center;
  font-size: 12px;
  color: var(--sk-danger);
`;

const UploadZoneWrapper = styled.div`
  padding: 8px;
`;

const EmptyState = styled.div`
  padding: 24px 12px;
  text-align: center;
  font-size: 12px;
  color: var(--sk-text-muted);
`;

interface AudioAssetLibraryProps {
  onDragStart?: (event: React.DragEvent, asset: AudioAsset) => void;
}

export function AudioAssetLibrary({ onDragStart }: AudioAssetLibraryProps) {
  const { can } = usePermissions();
  const toast = useToast();

  const [searchTerm, setSearchTerm] = useState('');
  const [formatFilter, setFormatFilter] = useState<string>('all');
  const [assetToDelete, setAssetToDelete] = useState<AudioAsset | null>(null);

  // Query hooks
  const { data: assets, isLoading, error } = useAudioAssets();
  const uploadMutation = useUploadAudioAsset();
  const deleteMutation = useDeleteAudioAsset();

  // Filter assets
  const filteredAssets = (assets || []).filter((asset) => {
    const matchesSearch = asset.name.toLowerCase().includes(searchTerm.toLowerCase());
    const matchesFormat =
      formatFilter === 'all' || asset.format.toLowerCase() === formatFilter.toLowerCase();
    return matchesSearch && matchesFormat;
  });

  // Separate system and user assets
  const systemAssets = filteredAssets.filter((a) => a.is_system);
  const userAssets = filteredAssets.filter((a) => !a.is_system);

  const handleFileSelect = useCallback(
    async (files: FileList) => {
      const file = files?.[0];
      if (!file) return;

      // Validate file type
      const validExtensions = ['opus', 'ogg', 'flac', 'mp3', 'wav'];
      const extension = file.name.split('.').pop()?.toLowerCase();
      if (!extension || !validExtensions.includes(extension)) {
        toast.error(`Invalid file type. Allowed: ${validExtensions.join(', ')}`);
        return;
      }

      // Validate file size (100MB max)
      const maxSize = 100 * 1024 * 1024;
      if (file.size > maxSize) {
        toast.error('File too large. Maximum size: 100MB');
        return;
      }

      try {
        await uploadMutation.mutateAsync(file);
        toast.success(`Uploaded ${file.name}`);
      } catch (error) {
        const errorMessage = error instanceof Error ? error.message : 'Failed to upload file';
        toast.error(errorMessage);
      }
    },
    [uploadMutation, toast]
  );

  // Delete handlers
  const handleDeleteClick = (asset: AudioAsset) => {
    setAssetToDelete(asset);
  };

  const handleDeleteConfirm = async () => {
    if (!assetToDelete || deleteMutation.isPending) return;

    try {
      await deleteMutation.mutateAsync(assetToDelete.id);
      toast.success(`Deleted ${assetToDelete.name}`);
      setAssetToDelete(null);
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : 'Failed to delete asset';
      toast.error(errorMessage);
      setAssetToDelete(null);
    }
  };

  const handleDeleteCancel = () => {
    setAssetToDelete(null);
  };

  if (isLoading) {
    return (
      <LibraryWrapper>
        <LibraryHeader>
          <LibraryTitle>Assets</LibraryTitle>
        </LibraryHeader>
        <LoadingState>Loading assets...</LoadingState>
      </LibraryWrapper>
    );
  }

  if (error) {
    return (
      <LibraryWrapper>
        <LibraryHeader>
          <LibraryTitle>Assets</LibraryTitle>
        </LibraryHeader>
        <ErrorState>Failed to load assets. {String(error)}</ErrorState>
      </LibraryWrapper>
    );
  }

  return (
    <LibraryWrapper>
      <LibraryHeader>
        <HeaderRow>
          <LibraryTitle>Assets</LibraryTitle>
        </HeaderRow>
        <FilterRow>
          <SearchInput
            type="text"
            placeholder="Search assets..."
            value={searchTerm}
            onChange={(e) => setSearchTerm(e.target.value)}
          />
          <FilterSelect value={formatFilter} onChange={(e) => setFormatFilter(e.target.value)}>
            <option value="all">All Formats</option>
            <option value="flac">FLAC</option>
            <option value="opus">OPUS</option>
            <option value="mp3">MP3</option>
            <option value="wav">WAV</option>
            <option value="ogg">OGG</option>
          </FilterSelect>
        </FilterRow>
      </LibraryHeader>

      {can.uploadAsset && (
        <UploadZoneWrapper>
          <UploadDropZone
            accept=".opus,.ogg,.flac,.mp3,.wav"
            disabled={uploadMutation.isPending}
            icon={<Upload size={24} />}
            text="Drop audio file here or click to browse"
            hint="Supported: OPUS, OGG, FLAC, MP3, WAV (max 100MB)"
            onFilesSelected={handleFileSelect}
          />
        </UploadZoneWrapper>
      )}

      <AssetsList>
        {filteredAssets.length === 0 && (
          <EmptyState>
            {searchTerm || formatFilter !== 'all'
              ? 'No assets match your filters'
              : 'No audio assets available'}
          </EmptyState>
        )}

        {systemAssets.length > 0 && (
          <>
            <SectionHeader>System Assets</SectionHeader>
            {systemAssets.map((asset) => (
              <AudioAssetCard
                key={asset.id}
                asset={asset}
                canDelete={can.deleteAsset}
                onDragStart={onDragStart}
              />
            ))}
          </>
        )}

        {userAssets.length > 0 && (
          <>
            <SectionHeader>User Assets</SectionHeader>
            {userAssets.map((asset) => (
              <AudioAssetCard
                key={asset.id}
                asset={asset}
                onDelete={handleDeleteClick}
                canDelete={can.deleteAsset}
                onDragStart={onDragStart}
              />
            ))}
          </>
        )}
      </AssetsList>

      {assetToDelete && (
        <ConfirmModal
          isOpen={true}
          title="Delete Audio Asset"
          message={`Are you sure you want to delete "${assetToDelete.name}"? This action cannot be undone.`}
          onConfirm={handleDeleteConfirm}
          onCancel={handleDeleteCancel}
          confirmLabel="Delete"
          cancelLabel="Cancel"
        />
      )}
    </LibraryWrapper>
  );
}
