// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import { Upload } from 'lucide-react';
import { useState, useCallback, useMemo } from 'react';

import { useToast } from '@/context/ToastContext';
import { usePermissions } from '@/hooks/usePermissions';
import { useAudioAssets, useUploadAudioAsset, useDeleteAudioAsset } from '@/services/assets';
import { useFontAssets, useUploadFontAsset, useDeleteFontAsset } from '@/services/fontAssets';
import { useImageAssets, useUploadImageAsset, useDeleteImageAsset } from '@/services/imageAssets';
import { useSlintAssets, useUploadSlintAsset, useDeleteSlintAsset } from '@/services/slintAssets';

import { AssetCard, type AssetType, type UnifiedAsset } from './AssetCard';
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

// ── Constants ───────────────────────────────────────────────────────────────

type TypeFilter = 'all' | AssetType;

const TYPE_LABELS: Record<TypeFilter, string> = {
  all: 'All',
  audio: 'Audio',
  image: 'Images',
  font: 'Fonts',
  slint: 'Slint',
};

const FORMAT_OPTIONS: Record<TypeFilter, string[]> = {
  audio: ['flac', 'opus', 'mp3', 'wav', 'ogg'],
  image: ['png', 'jpg', 'webp', 'gif'],
  font: ['ttf', 'otf'],
  slint: [],
  all: ['flac', 'opus', 'mp3', 'wav', 'ogg', 'png', 'jpg', 'webp', 'gif', 'ttf', 'otf'],
};

const ACCEPT_STRINGS: Record<TypeFilter, string> = {
  audio: '.opus,.ogg,.flac,.mp3,.wav',
  image: '.png,.jpg,.jpeg,.webp,.gif',
  font: '.ttf,.otf',
  slint: '.slint',
  all: '.opus,.ogg,.flac,.mp3,.wav,.png,.jpg,.jpeg,.webp,.gif,.ttf,.otf,.slint',
};

const UPLOAD_HINTS: Record<TypeFilter, string> = {
  audio: 'Supported: OPUS, OGG, FLAC, MP3, WAV (max 100MB)',
  image: 'Supported: PNG, JPG, WEBP, GIF (max 100MB)',
  font: 'Supported: TTF, OTF (max 100MB)',
  slint: 'Supported: .slint files (max 100MB)',
  all: 'Supported: audio, image, font, and slint files (max 100MB)',
};

const UPLOAD_TEXTS: Record<TypeFilter, string> = {
  audio: 'Drop audio file here or click to browse',
  image: 'Drop image file here or click to browse',
  font: 'Drop font file here or click to browse',
  slint: 'Drop .slint file here or click to browse',
  all: 'Drop asset file here or click to browse',
};

const EMPTY_MESSAGES: Record<TypeFilter, string> = {
  audio: 'No audio assets available',
  image: 'No image assets available',
  font: 'No font assets available',
  slint: 'No Slint assets available',
  all: 'No assets available',
};

const EXTENSION_TO_TYPE: Record<string, AssetType> = {
  opus: 'audio',
  ogg: 'audio',
  flac: 'audio',
  mp3: 'audio',
  wav: 'audio',
  png: 'image',
  jpg: 'image',
  jpeg: 'image',
  webp: 'image',
  gif: 'image',
  ttf: 'font',
  otf: 'font',
  slint: 'slint',
};

// ── Helper: check whether a type is relevant for the active filter ───────

function typeVisible(typeFilter: TypeFilter, t: AssetType): boolean {
  return typeFilter === 'all' || typeFilter === t;
}

// ── Custom hook: aggregate all asset queries + mutations ─────────────────

function useAssetQueries(typeFilter: TypeFilter) {
  const audioQuery = useAudioAssets();
  const imageQuery = useImageAssets();
  const fontQuery = useFontAssets();
  const slintQuery = useSlintAssets();

  const uploadAudio = useUploadAudioAsset();
  const uploadImage = useUploadImageAsset();
  const uploadFont = useUploadFontAsset();
  const uploadSlint = useUploadSlintAsset();

  const deleteAudio = useDeleteAudioAsset();
  const deleteImage = useDeleteImageAsset();
  const deleteFont = useDeleteFontAsset();
  const deleteSlint = useDeleteSlintAsset();

  const queries = { audio: audioQuery, image: imageQuery, font: fontQuery, slint: slintQuery };

  const isLoading = (['audio', 'image', 'font', 'slint'] as const).some(
    (t) => typeVisible(typeFilter, t) && queries[t].isLoading
  );

  const error =
    (['audio', 'image', 'font', 'slint'] as const)
      .filter((t) => typeVisible(typeFilter, t))
      .map((t) => queries[t].error)
      .find(Boolean) ?? null;

  const isUploading =
    uploadAudio.isPending || uploadImage.isPending || uploadFont.isPending || uploadSlint.isPending;

  const isDeleting =
    deleteAudio.isPending || deleteImage.isPending || deleteFont.isPending || deleteSlint.isPending;

  const uploadMutations: Record<AssetType, (file: File) => Promise<unknown>> = {
    audio: (f) => uploadAudio.mutateAsync(f),
    image: (f) => uploadImage.mutateAsync(f),
    font: (f) => uploadFont.mutateAsync(f),
    slint: (f) => uploadSlint.mutateAsync(f),
  };

  const deleteMutations: Record<AssetType, (id: string) => Promise<void>> = {
    audio: (id) => deleteAudio.mutateAsync(id),
    image: (id) => deleteImage.mutateAsync(id),
    font: (id) => deleteFont.mutateAsync(id),
    slint: (id) => deleteSlint.mutateAsync(id),
  };

  // Build unified list
  const allAssets: UnifiedAsset[] = useMemo(() => {
    const result: UnifiedAsset[] = [];
    if (typeVisible(typeFilter, 'audio')) {
      for (const a of audioQuery.data ?? []) result.push({ type: 'audio', asset: a });
    }
    if (typeVisible(typeFilter, 'image')) {
      for (const a of imageQuery.data ?? []) result.push({ type: 'image', asset: a });
    }
    if (typeVisible(typeFilter, 'font')) {
      for (const a of fontQuery.data ?? []) result.push({ type: 'font', asset: a });
    }
    if (typeVisible(typeFilter, 'slint')) {
      for (const a of slintQuery.data ?? []) result.push({ type: 'slint', asset: a });
    }
    return result;
  }, [typeFilter, audioQuery.data, imageQuery.data, fontQuery.data, slintQuery.data]);

  return {
    allAssets,
    isLoading,
    error,
    isUploading,
    isDeleting,
    uploadMutations,
    deleteMutations,
  };
}

// ── Props ───────────────────────────────────────────────────────────────────

interface AssetLibraryProps {
  onAssetDragStart?: (event: React.DragEvent, item: UnifiedAsset) => void;
}

// ── Component ───────────────────────────────────────────────────────────────

export function AssetLibrary({ onAssetDragStart }: AssetLibraryProps) {
  const { can } = usePermissions();
  const toast = useToast();

  const [typeFilter, setTypeFilter] = useState<TypeFilter>('all');
  const [searchTerm, setSearchTerm] = useState('');
  const [formatFilter, setFormatFilter] = useState<string>('all');
  const [assetToDelete, setAssetToDelete] = useState<UnifiedAsset | null>(null);

  const { allAssets, isLoading, error, isUploading, isDeleting, uploadMutations, deleteMutations } =
    useAssetQueries(typeFilter);

  // Filter by search and format
  const filteredAssets = useMemo(() => {
    return allAssets.filter((item) => {
      const matchesSearch = item.asset.name.toLowerCase().includes(searchTerm.toLowerCase());
      const matchesFormat =
        formatFilter === 'all' || item.asset.format.toLowerCase() === formatFilter.toLowerCase();
      return matchesSearch && matchesFormat;
    });
  }, [allAssets, searchTerm, formatFilter]);

  const systemAssets = useMemo(
    () => filteredAssets.filter((a) => a.asset.is_system),
    [filteredAssets]
  );
  const userAssets = useMemo(
    () => filteredAssets.filter((a) => !a.asset.is_system),
    [filteredAssets]
  );

  // Reset format filter when type changes
  const handleTypeChange = useCallback((t: TypeFilter) => {
    setTypeFilter(t);
    setFormatFilter('all');
  }, []);

  const formatOptions = FORMAT_OPTIONS[typeFilter];

  // ── Upload handler ──────────────────────────────────────────────────────

  const handleFileSelect = useCallback(
    async (files: FileList) => {
      const file = files?.[0];
      if (!file) return;

      const extension = file.name.split('.').pop()?.toLowerCase() ?? '';
      const fileType = EXTENSION_TO_TYPE[extension];

      if (!fileType) {
        toast.error(`Unsupported file type: .${extension}`);
        return;
      }

      const maxSize = 100 * 1024 * 1024;
      if (file.size > maxSize) {
        toast.error('File too large. Maximum size: 100MB');
        return;
      }

      try {
        await uploadMutations[fileType](file);
        toast.success(`Uploaded ${file.name}`);
      } catch (err) {
        toast.error(err instanceof Error ? err.message : 'Failed to upload file');
      }
    },
    [uploadMutations, toast]
  );

  // ── Delete handlers ─────────────────────────────────────────────────────

  const handleDeleteClick = useCallback((item: UnifiedAsset) => {
    setAssetToDelete(item);
  }, []);

  const handleDeleteConfirm = useCallback(async () => {
    if (!assetToDelete || isDeleting) return;

    try {
      await deleteMutations[assetToDelete.type](assetToDelete.asset.id);
      toast.success(`Deleted ${assetToDelete.asset.name}`);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : 'Failed to delete asset');
    }
    setAssetToDelete(null);
  }, [assetToDelete, isDeleting, deleteMutations, toast]);

  const handleDeleteCancel = useCallback(() => {
    setAssetToDelete(null);
  }, []);

  // ── Render ──────────────────────────────────────────────────────────────

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

        <TypeFilterRow>
          {(Object.keys(TYPE_LABELS) as TypeFilter[]).map((t) => (
            <TypeButton key={t} $active={typeFilter === t} onClick={() => handleTypeChange(t)}>
              {TYPE_LABELS[t]}
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
          {formatOptions.length > 0 && (
            <FilterSelect value={formatFilter} onChange={(e) => setFormatFilter(e.target.value)}>
              <option value="all">All Formats</option>
              {formatOptions.map((fmt) => (
                <option key={fmt} value={fmt}>
                  {fmt.toUpperCase()}
                </option>
              ))}
            </FilterSelect>
          )}
        </FilterRow>
      </LibraryHeader>

      {can.uploadAsset && (
        <UploadZoneWrapper>
          <UploadDropZone
            accept={ACCEPT_STRINGS[typeFilter]}
            disabled={isUploading}
            icon={<Upload size={24} />}
            text={UPLOAD_TEXTS[typeFilter]}
            hint={UPLOAD_HINTS[typeFilter]}
            onFilesSelected={handleFileSelect}
          />
        </UploadZoneWrapper>
      )}

      <AssetsList>
        {filteredAssets.length === 0 && (
          <EmptyState>
            {searchTerm || formatFilter !== 'all'
              ? 'No assets match your filters'
              : EMPTY_MESSAGES[typeFilter]}
          </EmptyState>
        )}

        {systemAssets.length > 0 && (
          <>
            <SectionHeader>System Assets</SectionHeader>
            {systemAssets.map((item) => (
              <AssetCard
                key={`${item.type}-${item.asset.id}`}
                item={item}
                canDelete={can.deleteAsset}
                onDragStart={onAssetDragStart}
              />
            ))}
          </>
        )}

        {userAssets.length > 0 && (
          <>
            <SectionHeader>User Assets</SectionHeader>
            {userAssets.map((item) => (
              <AssetCard
                key={`${item.type}-${item.asset.id}`}
                item={item}
                onDelete={handleDeleteClick}
                canDelete={can.deleteAsset}
                onDragStart={onAssetDragStart}
              />
            ))}
          </>
        )}
      </AssetsList>

      {assetToDelete && (
        <ConfirmModal
          isOpen={true}
          title="Delete Asset"
          message={`Are you sure you want to delete "${assetToDelete.asset.name}"? This action cannot be undone.`}
          onConfirm={handleDeleteConfirm}
          onCancel={handleDeleteCancel}
          confirmLabel="Delete"
          cancelLabel="Cancel"
        />
      )}
    </LibraryWrapper>
  );
}
