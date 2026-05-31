// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import React from 'react';

import { RadioGroupRoot, RadioItem, RadioIndicator } from '@/components/ui/RadioGroup';
import type { SamplePipeline } from '@/types/generated/api-types';
import { labelFromKey } from '@/utils/jsonSchema';
import type { SampleFacets, ScenarioGroup } from '@/utils/samplePipelineOrdering';
import {
  buildSearchHaystack,
  collectSampleFacets,
  compareSamplePipelinesByName,
  groupSamplePipelinesByScenario,
  haystackMatchesTokens,
  sampleNeedsHardware,
  tokenizeQuery,
} from '@/utils/samplePipelineOrdering';

import {
  ClearAllButton,
  Controls,
  EmptyState,
  FacetBar,
  FacetChip,
  FacetRow,
  FacetRowLabel,
  FilterButton,
  FilterGroup,
  GroupCard,
  HiddenSelectionHint,
  SearchInput,
  Section,
  SectionCount,
  SectionHeader,
  SelectorContainer,
  TemplateBadge,
  TemplateCard,
  TemplateContent,
  TemplateDescription,
  TemplateGrid,
  TemplateHeader,
  TemplateName,
  VariantOption,
  VariantSelector,
} from './TemplateSelector.styles';

interface TemplateSelectorProps {
  templates: SamplePipeline[];
  selectedTemplateId: string;
  onTemplateSelect: (templateId: string) => void;
  onSelectionHiddenChange?: (hidden: boolean) => void;
}

const ScenarioHeader: React.FC<{ sample: SamplePipeline }> = ({ sample }) => (
  <TemplateContent>
    <TemplateHeader>
      <TemplateName>{sample.name}</TemplateName>
      <TemplateBadge variant={sample.is_system ? 'system' : 'user'}>
        {sample.is_system ? 'System' : 'User'}
      </TemplateBadge>
    </TemplateHeader>
    {sample.description && <TemplateDescription>{sample.description}</TemplateDescription>}
  </TemplateContent>
);

const ScenarioCard: React.FC<{ group: ScenarioGroup; selectedTemplateId: string }> = ({
  group,
  selectedTemplateId,
}) => {
  const { base, variants } = group;

  if (variants.length === 1) {
    return (
      <TemplateCard htmlFor={`template-${base.id}`}>
        <RadioItem value={base.id} id={`template-${base.id}`} aria-label={base.name}>
          <RadioIndicator />
        </RadioItem>
        <ScenarioHeader sample={base} />
      </TemplateCard>
    );
  }

  const selectedMember = variants.find((variant) => variant.id === selectedTemplateId);

  return (
    <GroupCard data-selected={selectedMember ? true : undefined}>
      <ScenarioHeader sample={selectedMember ?? base} />
      <VariantSelector role="group" aria-label={`${base.name} variants`}>
        {variants.map((variant) => {
          const variantLabel = variant.variant ?? variant.name;
          return (
            <VariantOption key={variant.id} value={variant.id} aria-label={variantLabel}>
              {variantLabel}
            </VariantOption>
          );
        })}
      </VariantSelector>
    </GroupCard>
  );
};

function toScenarioGroups(samples: SamplePipeline[]): ScenarioGroup[] {
  return groupSamplePipelinesByScenario(samples).sort((a, b) =>
    compareSamplePipelinesByName(a.base, b.base)
  );
}

const GroupSection: React.FC<{
  title: string;
  groups: ScenarioGroup[];
  selectedTemplateId: string;
}> = ({ title, groups, selectedTemplateId }) => {
  if (groups.length === 0) return null;
  return (
    <Section>
      <SectionHeader>
        <span>{title}</span>
        <SectionCount>{groups.length}</SectionCount>
      </SectionHeader>
      <TemplateGrid>
        {groups.map((group) => (
          <ScenarioCard key={group.key} group={group} selectedTemplateId={selectedTemplateId} />
        ))}
      </TemplateGrid>
    </Section>
  );
};

interface FacetFiltersProps {
  facets: SampleFacets;
  categoryFilter: string | null;
  capabilityFilter: string | null;
  hardwareOnly: boolean;
  onToggleCategory: (category: string) => void;
  onToggleCapability: (capability: string) => void;
  onToggleHardware: () => void;
}

const FacetFilters: React.FC<FacetFiltersProps> = ({
  facets,
  categoryFilter,
  capabilityFilter,
  hardwareOnly,
  onToggleCategory,
  onToggleCapability,
  onToggleHardware,
}) => (
  <FacetBar>
    {facets.categories.length > 0 && (
      <FacetRow role="group" aria-label="Filter by category">
        <FacetRowLabel>Category</FacetRowLabel>
        {facets.categories.map((category) => (
          <FacetChip
            key={category}
            type="button"
            active={categoryFilter === category}
            aria-pressed={categoryFilter === category}
            onClick={() => onToggleCategory(category)}
          >
            {category}
          </FacetChip>
        ))}
      </FacetRow>
    )}

    {facets.capabilities.length > 0 && (
      <FacetRow role="group" aria-label="Filter by capability">
        <FacetRowLabel>Capability</FacetRowLabel>
        {facets.capabilities.map((capability) => (
          <FacetChip
            key={capability}
            type="button"
            active={capabilityFilter === capability}
            aria-pressed={capabilityFilter === capability}
            onClick={() => onToggleCapability(capability)}
          >
            {labelFromKey(capability)}
          </FacetChip>
        ))}
      </FacetRow>
    )}

    {facets.hasHardware && (
      <FacetRow role="group" aria-label="Filter by hardware requirements">
        <FacetRowLabel>Requirements</FacetRowLabel>
        <FacetChip
          type="button"
          active={hardwareOnly}
          aria-pressed={hardwareOnly}
          onClick={onToggleHardware}
        >
          Needs GPU
        </FacetChip>
      </FacetRow>
    )}
  </FacetBar>
);

export const TemplateSelector: React.FC<TemplateSelectorProps> = ({
  templates,
  selectedTemplateId,
  onTemplateSelect,
  onSelectionHiddenChange,
}) => {
  const [query, setQuery] = React.useState('');
  const [originFilter, setOriginFilter] = React.useState<'all' | 'system' | 'user'>('all');
  const [categoryFilter, setCategoryFilter] = React.useState<string | null>(null);
  const [capabilityFilter, setCapabilityFilter] = React.useState<string | null>(null);
  const [hardwareOnly, setHardwareOnly] = React.useState(false);

  const originFilteredTemplates = React.useMemo(
    () =>
      templates.filter((template) => {
        if (originFilter === 'system' && !template.is_system) return false;
        if (originFilter === 'user' && template.is_system) return false;
        return true;
      }),
    [templates, originFilter]
  );

  const facets = React.useMemo(
    () => collectSampleFacets(originFilteredTemplates),
    [originFilteredTemplates]
  );

  const haystacks = React.useMemo(() => {
    const map = new Map<string, string>();
    for (const template of templates) {
      map.set(template.id, buildSearchHaystack(template));
    }
    return map;
  }, [templates]);

  const facetFiltersActive = Boolean(categoryFilter || capabilityFilter || hardwareOnly);
  const anyFilterActive = Boolean(query.trim() || originFilter !== 'all' || facetFiltersActive);

  const resetFilters = React.useCallback(() => {
    setQuery('');
    setOriginFilter('all');
    setCategoryFilter(null);
    setCapabilityFilter(null);
    setHardwareOnly(false);
  }, []);

  const toggleCategory = React.useCallback(
    (category: string) => setCategoryFilter((current) => (current === category ? null : category)),
    []
  );

  const toggleCapability = React.useCallback(
    (capability: string) =>
      setCapabilityFilter((current) => (current === capability ? null : capability)),
    []
  );

  const toggleHardware = React.useCallback(() => setHardwareOnly((current) => !current), []);

  const queryTokens = React.useMemo(() => tokenizeQuery(query), [query]);

  const filteredTemplates = React.useMemo(() => {
    return originFilteredTemplates.filter((template) => {
      if (categoryFilter && template.category !== categoryFilter) return false;
      if (capabilityFilter && !(template.tags ?? []).includes(capabilityFilter)) return false;
      if (hardwareOnly && !sampleNeedsHardware(template)) return false;
      return haystackMatchesTokens(haystacks.get(template.id) ?? '', queryTokens);
    });
  }, [
    originFilteredTemplates,
    categoryFilter,
    capabilityFilter,
    hardwareOnly,
    haystacks,
    queryTokens,
  ]);

  const systemGroups = React.useMemo(
    () => toScenarioGroups(filteredTemplates.filter((template) => template.is_system)),
    [filteredTemplates]
  );

  const userGroups = React.useMemo(
    () => toScenarioGroups(filteredTemplates.filter((template) => !template.is_system)),
    [filteredTemplates]
  );

  const selectedExists = React.useMemo(() => {
    return templates.some((template) => template.id === selectedTemplateId);
  }, [templates, selectedTemplateId]);

  const selectedVisible = React.useMemo(() => {
    return filteredTemplates.some((template) => template.id === selectedTemplateId);
  }, [filteredTemplates, selectedTemplateId]);

  const selectionHidden = Boolean(selectedTemplateId && selectedExists && !selectedVisible);

  React.useEffect(() => {
    onSelectionHiddenChange?.(selectionHidden);
  }, [selectionHidden, onSelectionHiddenChange]);

  const showHiddenSelectionHint = selectionHidden && anyFilterActive;

  const showFacetBar =
    facets.categories.length > 0 || facets.capabilities.length > 0 || facets.hasHardware;

  return (
    <SelectorContainer>
      <Controls>
        <SearchInput
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search pipelines…"
          aria-label="Search pipeline templates"
        />
        <FilterGroup role="group" aria-label="Filter templates by origin">
          <FilterButton
            type="button"
            active={originFilter === 'all'}
            onClick={() => setOriginFilter('all')}
          >
            All
          </FilterButton>
          <FilterButton
            type="button"
            active={originFilter === 'system'}
            onClick={() => setOriginFilter('system')}
          >
            System
          </FilterButton>
          <FilterButton
            type="button"
            active={originFilter === 'user'}
            onClick={() => setOriginFilter('user')}
          >
            User
          </FilterButton>
        </FilterGroup>
        {anyFilterActive && (
          <ClearAllButton type="button" onClick={resetFilters}>
            Clear all filters
          </ClearAllButton>
        )}
      </Controls>

      {showFacetBar && (
        <FacetFilters
          facets={facets}
          categoryFilter={categoryFilter}
          capabilityFilter={capabilityFilter}
          hardwareOnly={hardwareOnly}
          onToggleCategory={toggleCategory}
          onToggleCapability={toggleCapability}
          onToggleHardware={toggleHardware}
        />
      )}

      {showHiddenSelectionHint && (
        <HiddenSelectionHint>Selected template is hidden by your filters.</HiddenSelectionHint>
      )}

      <RadioGroupRoot
        value={selectedTemplateId}
        onValueChange={onTemplateSelect}
        aria-label="Pipeline template selection"
      >
        {systemGroups.length === 0 && userGroups.length === 0 && (
          <EmptyState>No pipelines match your filters.</EmptyState>
        )}

        <GroupSection
          title="System Pipelines"
          groups={systemGroups}
          selectedTemplateId={selectedTemplateId}
        />
        <GroupSection
          title="User Pipelines"
          groups={userGroups}
          selectedTemplateId={selectedTemplateId}
        />
      </RadioGroupRoot>
    </SelectorContainer>
  );
};
