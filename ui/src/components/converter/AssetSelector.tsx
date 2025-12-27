// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React from 'react';

import { RadioGroupRoot, RadioItem, RadioIndicator, RadioLabel } from '@/components/ui/RadioGroup';
import type { AudioAsset } from '@/types/generated/api-types';

const SelectorContainer = styled.div`
  width: 100%;
`;

const AssetGrid = styled.div`
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(250px, 1fr));
  gap: 16px;
`;

const AssetCard = styled(RadioLabel)`
  padding: 20px;
  background: var(--sk-panel-bg);
  border: 2px solid var(--sk-border);
  border-radius: 8px;
  cursor: pointer;
  text-align: left;
  display: flex;
  gap: 12px;
  transition: none;
  align-items: flex-start;

  &:hover {
    border-color: var(--sk-border-strong);
    background: var(--sk-hover-bg);
  }

  &:has([data-state='checked']) {
    background: var(--sk-primary);
    color: var(--sk-primary-contrast);
    border-color: var(--sk-primary);
  }

  &:has([data-state='checked']):hover {
    background: var(--sk-primary-hover);
    border-color: var(--sk-primary-hover);
  }
`;

const AssetContent = styled.div`
  display: flex;
  flex-direction: column;
  gap: 8px;
  flex: 1;
`;

const AssetName = styled.div`
  font-weight: 600;
  font-size: 16px;
`;

const AssetMetadata = styled.div`
  font-size: 13px;
  line-height: 1.4;
  color: inherit;
  opacity: 0.9;
  display: flex;
  flex-direction: column;
  gap: 4px;
`;

const AssetLicense = styled.div`
  font-size: 12px;
  line-height: 1.3;
  color: inherit;
  opacity: 0.8;
  font-style: italic;
  margin-top: 4px;
  padding-top: 8px;
  border-top: 1px solid currentColor;
  border-opacity: 0.2;
`;

const EmptyState = styled.div`
  padding: 40px;
  text-align: center;
  color: var(--sk-text-muted);
  font-size: 14px;
  grid-column: 1 / -1;
`;

interface AssetSelectorProps {
  assets: AudioAsset[];
  selectedAssetId: string;
  onAssetSelect: (assetId: string) => void;
  isLoading?: boolean;
}

export const AssetSelector: React.FC<AssetSelectorProps> = ({
  assets,
  selectedAssetId,
  onAssetSelect,
  isLoading = false,
}) => {
  if (isLoading) {
    return (
      <SelectorContainer>
        <EmptyState>Loading audio assets...</EmptyState>
      </SelectorContainer>
    );
  }

  if (assets.length === 0) {
    return (
      <SelectorContainer>
        <EmptyState>
          No audio assets available. Upload assets from the Design view's audio library.
        </EmptyState>
      </SelectorContainer>
    );
  }

  const formatFileSize = (bytes: bigint): string => {
    const kb = Number(bytes) / 1024;
    if (kb < 1024) {
      return `${kb.toFixed(1)} KB`;
    }
    return `${(kb / 1024).toFixed(1)} MB`;
  };

  return (
    <SelectorContainer>
      <RadioGroupRoot
        value={selectedAssetId}
        onValueChange={onAssetSelect}
        aria-label="Audio asset selection"
      >
        <AssetGrid>
          {assets.map((asset) => (
            <AssetCard key={asset.id} htmlFor={`asset-${asset.id}`}>
              <RadioItem value={asset.id} id={`asset-${asset.id}`}>
                <RadioIndicator />
              </RadioItem>
              <AssetContent>
                <AssetName>{asset.name}</AssetName>
                <AssetMetadata>
                  <div>
                    {asset.format.toUpperCase()} • {formatFileSize(asset.size_bytes)}
                  </div>
                  {asset.is_system && <div>System Asset</div>}
                </AssetMetadata>
                {asset.license && <AssetLicense>{asset.license}</AssetLicense>}
              </AssetContent>
            </AssetCard>
          ))}
        </AssetGrid>
      </RadioGroupRoot>
    </SelectorContainer>
  );
};
