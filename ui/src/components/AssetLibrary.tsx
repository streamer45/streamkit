// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import { useQueries, useQueryClient } from '@tanstack/react-query';
import { Upload } from 'lucide-react';
import { useState, useCallback, useMemo } from 'react';

import { useToast } from '@/context/ToastContext';
import { usePermissions } from '@/hooks/usePermissions';
import { useAudioAssets, useUploadAudioAsset, useDeleteAudioAsset } from '@/services/assets';
import { useAssetTypes } from '@/services/assetTypes';
import { useFontAssets, useUploadFontAsset, useDeleteFontAsset } from '@/services/fontAssets';
import { useImageAssets, useUploadImageAsset, useDeleteImageAsset } from '@/services/imageAssets';
import { useUploadPluginAsset, deletePluginAsset, listPluginAssets } from '@/services/pluginAssets';
import type { AssetTypeInfo, AudioAsset, FontAsset, ImageAsset } from '@/types/generated/api-types';

import { AssetCard, type UnifiedAsset } from './AssetCard';
import ConfirmModal from './ConfirmModal';
import { UploadDropZone } from './UploadDropZone';

// ── Styled components ───────────────────────────────────────────────────────

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

const TypeFilterRow = styled.div`
  display: flex;
  gap: 2px;
  margin-bottom: 8px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  padding: 2px;
`;

const TypeButton = styled.button<{ $active: boolean }>`
  flex: 1;
  padding: 4px 6px;
  font-size: 11px;
  font-weight: 600;
  border: none;
  border-radius: 4px;
  cursor: pointer;
  transition:
    background 0.15s,
    color 0.15s;
  white-space: nowrap;

  background: ${({ $active }) => ($active ? 'var(--sk-primary)' : 'transparent')};
  color: ${({ $active }) => ($active ? 'var(--sk-text-white)' : 'var(--sk-text-muted)')};

  &:hover {
    background: ${({ $active }) => ($active ? 'var(--sk-primary)' : 'var(--sk-hover-bg)')};
    color: var(--sk-text);
  }
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

// ── Types ────────────────────────────────────────────────────────────────────

type TypeFilter = 'all' | string; // 'all' or a type_id

interface AssetLibraryProps {
  onDragStart?: (event: React.DragEvent, item: UnifiedAsset) => void;
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/** Append typed assets if the filter matches. */
function appendIfMatches<T extends 'audio' | 'image' | 'font'>(
  out: UnifiedAsset[],
  kind: T,
  data: (T extends 'audio' ? AudioAsset : T extends 'image' ? ImageAsset : FontAsset)[] | undefined,
  typeFilter: TypeFilter,
  typeId: string
): void {
  if (typeFilter !== 'all' && typeFilter !== typeId) return;
  if (!data) return;
  for (const a of data) {
    out.push({ type: kind, asset: a } as UnifiedAsset);
  }
}

/** Build the unified item list from heterogeneous query results. */
function buildUnifiedItems(
  typeFilter: TypeFilter,
  audio: AudioAsset[] | undefined,
  images: ImageAsset[] | undefined,
  fonts: FontAsset[] | undefined,
  pluginEntries: {
    data: import('@/types/generated/api-types').PluginAsset[] | undefined;
    typeInfo: AssetTypeInfo;
  }[]
): UnifiedAsset[] {
  const items: UnifiedAsset[] = [];
  appendIfMatches(items, 'audio', audio, typeFilter, 'audio');
  appendIfMatches(items, 'image', images, typeFilter, 'images');
  appendIfMatches(items, 'font', fonts, typeFilter, 'fonts');

  for (const entry of pluginEntries) {
    if (entry.data) {
      for (const a of entry.data) {
        items.push({ type: 'plugin', asset: a, typeInfo: entry.typeInfo });
      }
    }
  }
  return items;
}

/** Derive upload-zone metadata from the asset type registry. */
function getUploadConfig(
  typeFilter: TypeFilter,
  assetTypes: AssetTypeInfo[] | undefined
): { accept: string; hint: string } | null {
  if (typeFilter === 'all' || !assetTypes) return null;
  const typeInfo = assetTypes.find((t) => t.type_id === typeFilter);
  if (!typeInfo) return null;
  return {
    accept: typeInfo.extensions.map((e) => `.${e}`).join(','),
    hint: `Supported: ${typeInfo.extensions.map((e) => e.toUpperCase()).join(', ')}`,
  };
}

// ── Sub-components ───────────────────────────────────────────────────────────

function AssetListSection({
  title,
  items,
  canDelete,
  onDelete,
  onDragStart,
}: {
  title: string;
  items: UnifiedAsset[];
  canDelete: boolean;
  onDelete?: (item: UnifiedAsset) => void;
  onDragStart?: (event: React.DragEvent, item: UnifiedAsset) => void;
}) {
  if (items.length === 0) return null;
  return (
    <>
      <SectionHeader>{title}</SectionHeader>
      {items.map((item) => (
        <AssetCard
          key={`${item.type}-${item.asset.id}`}
          item={item}
          canDelete={canDelete}
          onDelete={onDelete}
          onDragStart={onDragStart}
        />
      ))}
    </>
  );
}

// ── Hooks ────────────────────────────────────────────────────────────────────

/** Encapsulates all asset queries, mutations, and derived data. */
function useAssetData(typeFilter: TypeFilter, assetTypes: AssetTypeInfo[] | undefined) {
  const selectedPluginType = useMemo(() => {
    if (!assetTypes) return null;
    return assetTypes.find((t) => t.source === 'plugin' && t.type_id === typeFilter) ?? null;
  }, [assetTypes, typeFilter]);

  // Collect all plugin types to query: either the single selected type, or all
  // plugin types when showing the "All" view.
  const pluginTypesToFetch = useMemo((): AssetTypeInfo[] => {
    if (selectedPluginType) return [selectedPluginType];
    if (typeFilter !== 'all' || !assetTypes) return [];
    return assetTypes.filter((t) => t.source === 'plugin');
  }, [selectedPluginType, typeFilter, assetTypes]);

  // Queries — skip fetches when the user has filtered to a different type.
  const shouldFetchAudio = typeFilter === 'all' || typeFilter === 'audio';
  const shouldFetchImages = typeFilter === 'all' || typeFilter === 'images';
  const shouldFetchFonts = typeFilter === 'all' || typeFilter === 'fonts';

  const audioQuery = useAudioAssets(shouldFetchAudio);
  const imageQuery = useImageAssets(shouldFetchImages);
  const fontQuery = useFontAssets(shouldFetchFonts);

  // Dynamic parallel queries for all relevant plugin types.
  const pluginQueries = useQueries({
    queries: pluginTypesToFetch.map((t) => ({
      queryKey: ['pluginAssets', t.type_id],
      queryFn: () => listPluginAssets(t.type_id),
      staleTime: 30_000,
      refetchOnWindowFocus: true,
    })),
  });

  // Mutations
  const uploadAudio = useUploadAudioAsset();
  const deleteAudio = useDeleteAudioAsset();
  const uploadImage = useUploadImageAsset();
  const deleteImage = useDeleteImageAsset();
  const uploadFont = useUploadFontAsset();
  const deleteFont = useDeleteFontAsset();
  const uploadPlugin = useUploadPluginAsset(selectedPluginType?.type_id ?? '');

  // Merge plugin query results with their type info.
  const pluginEntries = useMemo(
    () =>
      pluginTypesToFetch.map((typeInfo, i) => ({
        data: pluginQueries[i]?.data,
        typeInfo,
      })),
    [pluginTypesToFetch, pluginQueries]
  );

  const allItems = useMemo(
    () =>
      buildUnifiedItems(
        typeFilter,
        audioQuery.data,
        imageQuery.data,
        fontQuery.data,
        pluginEntries
      ),
    [typeFilter, audioQuery.data, imageQuery.data, fontQuery.data, pluginEntries]
  );

  return {
    selectedPluginType,
    allItems,
    isLoading: audioQuery.isLoading || imageQuery.isLoading || fontQuery.isLoading,
    error: audioQuery.error || imageQuery.error || fontQuery.error,
    isUploading:
      uploadAudio.isPending ||
      uploadImage.isPending ||
      uploadFont.isPending ||
      uploadPlugin.isPending,
    uploadAudio,
    uploadImage,
    uploadFont,
    uploadPlugin,
    deleteAudio,
    deleteImage,
    deleteFont,
  };
}

// ── Component ────────────────────────────────────────────────────────────────

export function AssetLibrary({ onDragStart }: AssetLibraryProps) {
  const { can } = usePermissions();
  const toast = useToast();
  const queryClient = useQueryClient();

  const [typeFilter, setTypeFilter] = useState<TypeFilter>('all');
  const [searchTerm, setSearchTerm] = useState('');
  const [formatFilter, setFormatFilter] = useState<string>('all');
  const [assetToDelete, setAssetToDelete] = useState<UnifiedAsset | null>(null);

  const { data: assetTypes } = useAssetTypes();
  const data = useAssetData(typeFilter, assetTypes);

  // ── Derived data ───────────────────────────────────────────────────────
  const filteredItems = useMemo(() => {
    const search = searchTerm.toLowerCase();
    const fmt = formatFilter.toLowerCase();
    return data.allItems.filter(
      (item) =>
        item.asset.name.toLowerCase().includes(search) &&
        (formatFilter === 'all' || item.asset.format.toLowerCase() === fmt)
    );
  }, [data.allItems, searchTerm, formatFilter]);

  const systemItems = useMemo(
    () => filteredItems.filter((i) => i.asset.is_system),
    [filteredItems]
  );
  const userItems = useMemo(() => filteredItems.filter((i) => !i.asset.is_system), [filteredItems]);

  const availableFormats = useMemo(() => {
    const fmts = new Set(data.allItems.map((i) => i.asset.format.toLowerCase()));
    return Array.from(fmts).sort();
  }, [data.allItems]);

  const uploadConfig = useMemo(
    () => getUploadConfig(typeFilter, assetTypes),
    [typeFilter, assetTypes]
  );

  // ── Handlers ───────────────────────────────────────────────────────────
  const handleFileSelect = useCallback(
    async (files: FileList) => {
      const file = files?.[0];
      if (!file) return;

      try {
        switch (typeFilter) {
          case 'audio':
            await data.uploadAudio.mutateAsync(file);
            break;
          case 'images':
            await data.uploadImage.mutateAsync(file);
            break;
          case 'fonts':
            await data.uploadFont.mutateAsync(file);
            break;
          default:
            if (data.selectedPluginType) {
              await data.uploadPlugin.mutateAsync(file);
            } else {
              toast.error('Select a specific asset type to upload');
              return;
            }
        }
        toast.success(`Uploaded ${file.name}`);
      } catch (error) {
        toast.error(error instanceof Error ? error.message : 'Upload failed');
      }
    },
    [typeFilter, data, toast]
  );

  const handleDeleteConfirm = useCallback(async () => {
    if (!assetToDelete) return;
    try {
      const id = assetToDelete.asset.id;
      switch (assetToDelete.type) {
        case 'audio':
          await data.deleteAudio.mutateAsync(id);
          break;
        case 'image':
          await data.deleteImage.mutateAsync(id);
          break;
        case 'font':
          await data.deleteFont.mutateAsync(id);
          break;
        case 'plugin': {
          const typeId = assetToDelete.asset.type_id;
          await deletePluginAsset(typeId, id);
          await queryClient.invalidateQueries({ queryKey: ['pluginAssets', typeId] });
          break;
        }
      }
      toast.success(`Deleted ${assetToDelete.asset.name}`);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : 'Delete failed');
    }
    setAssetToDelete(null);
  }, [assetToDelete, data, toast, queryClient]);

  // ── Loading / Error ────────────────────────────────────────────────────
  if (data.isLoading) {
    return (
      <LibraryWrapper>
        <LibraryHeader>
          <LibraryTitle>Assets</LibraryTitle>
        </LibraryHeader>
        <LoadingState>Loading assets...</LoadingState>
      </LibraryWrapper>
    );
  }

  if (data.error) {
    return (
      <LibraryWrapper>
        <LibraryHeader>
          <LibraryTitle>Assets</LibraryTitle>
        </LibraryHeader>
        <ErrorState>Failed to load assets. {String(data.error)}</ErrorState>
      </LibraryWrapper>
    );
  }

  const pluginTypes: AssetTypeInfo[] = assetTypes?.filter((t) => t.source === 'plugin') ?? [];

  return (
    <LibraryWrapper>
      <LibraryHeader>
        <HeaderRow>
          <LibraryTitle>Assets</LibraryTitle>
        </HeaderRow>

        <TypeFilterRow>
          <TypeButton $active={typeFilter === 'all'} onClick={() => setTypeFilter('all')}>
            All
          </TypeButton>
          <TypeButton $active={typeFilter === 'audio'} onClick={() => setTypeFilter('audio')}>
            Audio
          </TypeButton>
          <TypeButton $active={typeFilter === 'images'} onClick={() => setTypeFilter('images')}>
            Images
          </TypeButton>
          <TypeButton $active={typeFilter === 'fonts'} onClick={() => setTypeFilter('fonts')}>
            Fonts
          </TypeButton>
          {pluginTypes.map((pt) => (
            <TypeButton
              key={pt.type_id}
              $active={typeFilter === pt.type_id}
              onClick={() => setTypeFilter(pt.type_id)}
            >
              {pt.label}
            </TypeButton>
          ))}
        </TypeFilterRow>

        <FilterRow>
          <SearchInput
            type="text"
            placeholder="Search assets..."
            value={searchTerm}
            onChange={(e) => setSearchTerm(e.target.value)}
          />
          {availableFormats.length > 1 && (
            <FilterSelect value={formatFilter} onChange={(e) => setFormatFilter(e.target.value)}>
              <option value="all">All Formats</option>
              {availableFormats.map((fmt) => (
                <option key={fmt} value={fmt}>
                  {fmt.toUpperCase()}
                </option>
              ))}
            </FilterSelect>
          )}
        </FilterRow>
      </LibraryHeader>

      {can.uploadAsset && uploadConfig && (
        <UploadZoneWrapper>
          <UploadDropZone
            accept={uploadConfig.accept}
            disabled={data.isUploading}
            icon={<Upload size={24} />}
            text="Drop file here or click to browse"
            hint={uploadConfig.hint}
            onFilesSelected={handleFileSelect}
          />
        </UploadZoneWrapper>
      )}

      <AssetsList>
        {filteredItems.length === 0 && (
          <EmptyState>
            {searchTerm || formatFilter !== 'all'
              ? 'No assets match your filters'
              : 'No assets available'}
          </EmptyState>
        )}

        <AssetListSection
          title="System Assets"
          items={systemItems}
          canDelete={can.deleteAsset}
          onDragStart={onDragStart}
        />
        <AssetListSection
          title="User Assets"
          items={userItems}
          canDelete={can.deleteAsset}
          onDelete={setAssetToDelete}
          onDragStart={onDragStart}
        />
      </AssetsList>

      {assetToDelete && (
        <ConfirmModal
          isOpen={true}
          title="Delete Asset"
          message={`Are you sure you want to delete "${assetToDelete.asset.name}"? This action cannot be undone.`}
          onConfirm={handleDeleteConfirm}
          onCancel={() => setAssetToDelete(null)}
          confirmLabel="Delete"
          cancelLabel="Cancel"
        />
      )}
    </LibraryWrapper>
  );
}
