// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import React from 'react';

import { RadioGroupRoot, RadioItem, RadioIndicator } from '@/components/ui/RadioGroup';
import type { SamplePipeline } from '@/types/generated/api-types';
import type { SampleFacets, ScenarioGroup } from '@/utils/samplePipelineOrdering';
import {
  collectSampleFacets,
  compareSamplePipelinesByName,
  groupSamplePipelinesByScenario,
  matchesSamplePipelineQuery,
  sampleNeedsHardware,
} from '@/utils/samplePipelineOrdering';

import {
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
  HintButton,
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

function formatTagLabel(tag: string): string {
  return tag
    .split('-')
    .map((word) => (word ? word[0].toUpperCase() + word.slice(1) : word))
    .join(' ');
}

interface TemplateSelectorProps {
  templates: SamplePipeline[];
  selectedTemplateId: string;
  onTemplateSelect: (templateId: string) => void;
}

const ScenarioCard: React.FC<{ group: ScenarioGroup; selectedTemplateId: string }> = ({
  group,
  selectedTemplateId,
}) => {
  const { base, variants } = group;

  if (variants.length === 1) {
    return (
      <TemplateCard htmlFor={`template-${base.id}`}>
        <RadioItem value={base.id} id={`template-${base.id}`}>
          <RadioIndicator />
        </RadioItem>
        <TemplateContent>
          <TemplateHeader>
            <TemplateName>{base.name}</TemplateName>
            <TemplateBadge variant={base.is_system ? 'system' : 'user'}>
              {base.is_system ? 'System' : 'User'}
            </TemplateBadge>
          </TemplateHeader>
          {base.description && <TemplateDescription>{base.description}</TemplateDescription>}
        </TemplateContent>
      </TemplateCard>
    );
  }

  const selectedInGroup = variants.some((variant) => variant.id === selectedTemplateId);

  return (
    <GroupCard data-selected={selectedInGroup || undefined}>
      <TemplateContent>
        <TemplateHeader>
          <TemplateName>{base.name}</TemplateName>
          <TemplateBadge variant={base.is_system ? 'system' : 'user'}>
            {base.is_system ? 'System' : 'User'}
          </TemplateBadge>
        </TemplateHeader>
        {base.description && <TemplateDescription>{base.description}</TemplateDescription>}
      </TemplateContent>
      <VariantSelector role="group" aria-label={`${base.name} variants`}>
        {variants.map((variant) => (
          <VariantOption key={variant.id} value={variant.id} aria-label={variant.name}>
            {variant.variant ?? 'Default'}
          </VariantOption>
        ))}
      </VariantSelector>
    </GroupCard>
  );
};

function toScenarioGroups(samples: SamplePipeline[]): ScenarioGroup[] {
  return groupSamplePipelinesByScenario(samples)
    .slice()
    .sort((a, b) => compareSamplePipelinesByName(a.base, b.base));
}

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
            {formatTagLabel(capability)}
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
          Needs hardware
        </FacetChip>
      </FacetRow>
    )}
  </FacetBar>
);

export const TemplateSelector: React.FC<TemplateSelectorProps> = ({
  templates,
  selectedTemplateId,
  onTemplateSelect,
}) => {
  const [query, setQuery] = React.useState('');
  const [originFilter, setOriginFilter] = React.useState<'all' | 'system' | 'user'>('all');
  const [categoryFilter, setCategoryFilter] = React.useState<string | null>(null);
  const [capabilityFilter, setCapabilityFilter] = React.useState<string | null>(null);
  const [hardwareOnly, setHardwareOnly] = React.useState(false);

  const facets = React.useMemo(() => collectSampleFacets(templates), [templates]);

  const facetFiltersActive = React.useMemo(
    () => Boolean(categoryFilter || capabilityFilter || hardwareOnly),
    [categoryFilter, capabilityFilter, hardwareOnly]
  );

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

  const filteredTemplates = React.useMemo(() => {
    return templates.filter((template) => {
      if (originFilter === 'system' && !template.is_system) return false;
      if (originFilter === 'user' && template.is_system) return false;
      if (categoryFilter && template.category !== categoryFilter) return false;
      if (capabilityFilter && !(template.tags ?? []).includes(capabilityFilter)) return false;
      if (hardwareOnly && !sampleNeedsHardware(template)) return false;
      return matchesSamplePipelineQuery(template, query);
    });
  }, [templates, originFilter, categoryFilter, capabilityFilter, hardwareOnly, query]);

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

  const showHiddenSelectionHint = React.useMemo(
    () =>
      Boolean(
        selectedTemplateId &&
        selectedExists &&
        !selectedVisible &&
        (query.trim() || originFilter !== 'all' || facetFiltersActive)
      ),
    [selectedTemplateId, selectedExists, selectedVisible, query, originFilter, facetFiltersActive]
  );

  const showFacetBar = React.useMemo(
    () => facets.categories.length > 0 || facets.capabilities.length > 0 || facets.hasHardware,
    [facets]
  );

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
        <HiddenSelectionHint>
          <div>Selected template is hidden by your filters.</div>
          <HintButton type="button" onClick={resetFilters}>
            Clear filters
          </HintButton>
        </HiddenSelectionHint>
      )}

      <RadioGroupRoot
        value={selectedTemplateId}
        onValueChange={onTemplateSelect}
        aria-label="Pipeline template selection"
      >
        {systemGroups.length === 0 && userGroups.length === 0 && (
          <EmptyState>No pipelines match your filters.</EmptyState>
        )}

        {systemGroups.length > 0 && (
          <Section>
            <SectionHeader>
              <span>System Pipelines</span>
              <SectionCount>{systemGroups.length}</SectionCount>
            </SectionHeader>
            <TemplateGrid>
              {systemGroups.map((group) => (
                <ScenarioCard
                  key={group.key}
                  group={group}
                  selectedTemplateId={selectedTemplateId}
                />
              ))}
            </TemplateGrid>
          </Section>
        )}

        {userGroups.length > 0 && (
          <Section>
            <SectionHeader>
              <span>User Pipelines</span>
              <SectionCount>{userGroups.length}</SectionCount>
            </SectionHeader>
            <TemplateGrid>
              {userGroups.map((group) => (
                <ScenarioCard
                  key={group.key}
                  group={group}
                  selectedTemplateId={selectedTemplateId}
                />
              ))}
            </TemplateGrid>
          </Section>
        )}
      </RadioGroupRoot>
    </SelectorContainer>
  );
};
