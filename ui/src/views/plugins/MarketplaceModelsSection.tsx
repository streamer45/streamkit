// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import React from 'react';

import { CheckboxWithLabel } from '@/components/ui/Checkbox';
import type { MarketplacePluginDetails } from '@/types/marketplace';

import {
  ModelMeta,
  ModelName,
  ModelRow,
  NoticeBox,
  SectionDivider,
  SubSectionLabel,
} from '../PluginsView.styles';
import { formatBytes } from './marketplaceFormatters';

type MarketplaceModelsSectionProps = {
  hasModels: boolean;
  hasModelSelection: boolean;
  hasGatedModels: boolean;
  models: MarketplacePluginDetails['manifest']['models'];
  selectedModelIds: string[];
  onModelToggle: (modelId: string, checked: boolean) => void;
};

export const MarketplaceModelsSection: React.FC<MarketplaceModelsSectionProps> = ({
  hasModels,
  hasModelSelection,
  hasGatedModels,
  models,
  selectedModelIds,
  onModelToggle,
}) => {
  if (!hasModels) return null;
  return (
    <>
      <SectionDivider />
      <SubSectionLabel>Models</SubSectionLabel>
      {hasGatedModels && (
        <NoticeBox>Gated models require a Hugging Face token configured on the server.</NoticeBox>
      )}
      <MarketplaceModelsSelection
        enabled={hasModelSelection}
        models={models}
        selectedModelIds={selectedModelIds}
        onModelToggle={onModelToggle}
      />
      {!hasModelSelection && (
        <NoticeBox>Model selection is not available for this plugin.</NoticeBox>
      )}
    </>
  );
};

const MarketplaceModelsSelection: React.FC<{
  enabled: boolean;
  models: MarketplacePluginDetails['manifest']['models'];
  selectedModelIds: string[];
  onModelToggle: (modelId: string, checked: boolean) => void;
}> = ({ enabled, models, selectedModelIds, onModelToggle }) => {
  if (!enabled) return null;

  return (
    <>
      {models.map((model, index) => {
        const modelId = model.id ?? `model-${index}`;
        const fileCount = model.source === 'huggingface' ? model.files.length : 1;
        const displayName =
          model.name || model.id || (model.source === 'huggingface' ? model.files[0] : model.url);
        const sizeLabel = formatBytes(model.expected_size_bytes ?? undefined) || 'Unknown size';
        return (
          <ModelRow key={modelId}>
            <CheckboxWithLabel
              id={`plugin-model-${modelId}`}
              label=""
              checked={Boolean(model.id) && selectedModelIds.includes(modelId)}
              onCheckedChange={(value) => onModelToggle(modelId, Boolean(value))}
            />
            <ModelName>{displayName}</ModelName>
            <ModelMeta>{sizeLabel}</ModelMeta>
            {model.license && <ModelMeta>{model.license}</ModelMeta>}
            <ModelMeta>
              {fileCount} file{fileCount === 1 ? '' : 's'}
            </ModelMeta>
          </ModelRow>
        );
      })}
    </>
  );
};
