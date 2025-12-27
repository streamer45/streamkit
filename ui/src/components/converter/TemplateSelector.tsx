// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React from 'react';

import { RadioGroupRoot, RadioItem, RadioIndicator, RadioLabel } from '@/components/ui/RadioGroup';
import type { SamplePipeline } from '@/types/generated/api-types';
import {
  compareSamplePipelinesByName,
  matchesSamplePipelineQuery,
} from '@/utils/samplePipelineOrdering';

const SelectorContainer = styled.div`
  width: 100%;
`;

const Controls = styled.div`
  display: flex;
  gap: 12px;
  align-items: center;
  justify-content: space-between;
  flex-wrap: wrap;
  margin-bottom: 12px;
`;

const SearchInput = styled.input`
  flex: 1;
  min-width: 220px;
  padding: 10px 12px;
  font-size: 14px;
  background: var(--sk-bg);
  color: var(--sk-text);
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  font-family: inherit;

  &:focus {
    outline: none;
    border-color: var(--sk-primary);
  }

  &::placeholder {
    color: var(--sk-text-muted);
  }
`;

const FilterGroup = styled.div`
  display: inline-flex;
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  overflow: hidden;
  background: var(--sk-panel-bg);
`;

const FilterButton = styled.button<{ active?: boolean }>`
  padding: 8px 12px;
  font-size: 13px;
  font-weight: 700;
  border: none;
  cursor: pointer;
  transition: none;
  background: ${(props) => (props.active ? 'var(--sk-primary)' : 'transparent')};
  color: ${(props) => (props.active ? 'var(--sk-primary-contrast)' : 'var(--sk-text)')};

  &:hover {
    background: ${(props) => (props.active ? 'var(--sk-primary-hover)' : 'var(--sk-hover-bg)')};
  }

  &:focus-visible {
    outline: 2px solid var(--sk-primary);
    outline-offset: -2px;
  }
`;

const HiddenSelectionHint = styled.div`
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 12px;
  margin-bottom: 12px;
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  background: var(--sk-panel-bg);
  color: var(--sk-text-muted);
  font-size: 13px;
`;

const HintButton = styled.button`
  border: none;
  background: none;
  color: var(--sk-primary);
  font-weight: 700;
  cursor: pointer;
  padding: 0;

  &:hover {
    color: var(--sk-primary-hover);
  }

  &:focus-visible {
    outline: 2px solid var(--sk-primary);
    outline-offset: 2px;
    border-radius: 4px;
  }
`;

const Section = styled.div`
  display: flex;
  flex-direction: column;
  gap: 12px;
  margin-bottom: 18px;
`;

const SectionHeader = styled.div`
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 12px;
  color: var(--sk-text-muted);
  font-size: 12px;
  font-weight: 800;
  letter-spacing: 0.08em;
  text-transform: uppercase;
`;

const SectionCount = styled.span`
  font-weight: 700;
  letter-spacing: 0.02em;
  text-transform: none;
`;

const EmptyState = styled.div`
  padding: 16px;
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  background: var(--sk-panel-bg);
  color: var(--sk-text-muted);
  font-size: 14px;
`;

const TemplateGrid = styled.div`
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(250px, 1fr));
  gap: 16px;
`;

const TemplateCard = styled(RadioLabel)`
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

const TemplateContent = styled.div`
  display: flex;
  flex-direction: column;
  gap: 8px;
  flex: 1;
`;

const TemplateHeader = styled.div`
  display: flex;
  align-items: center;
  gap: 8px;
`;

const TemplateName = styled.div`
  font-weight: 600;
  font-size: 16px;
`;

const TemplateBadge = styled.span<{ variant: 'system' | 'user' }>`
  font-size: 11px;
  font-weight: 700;
  padding: 3px 10px;
  border-radius: 4px;
  text-transform: uppercase;
  letter-spacing: 0.5px;
  white-space: nowrap;
  background: ${(props) => (props.variant === 'system' ? '#4caf50' : '#2196f3')};
  color: #ffffff;

  /* Adjust for selected state - use high contrast */
  [data-state='checked'] & {
    background: rgba(0, 0, 0, 0.3);
    color: #ffffff;
    border: 1px solid rgba(255, 255, 255, 0.6);
    padding: 2px 9px; /* Account for border */
  }
`;

const TemplateDescription = styled.div`
  font-size: 13px;
  line-height: 1.4;
  color: inherit;
  opacity: 0.9;
`;

interface TemplateSelectorProps {
  templates: SamplePipeline[];
  selectedTemplateId: string;
  onTemplateSelect: (templateId: string) => void;
}

export const TemplateSelector: React.FC<TemplateSelectorProps> = ({
  templates,
  selectedTemplateId,
  onTemplateSelect,
}) => {
  const [query, setQuery] = React.useState('');
  const [originFilter, setOriginFilter] = React.useState<'all' | 'system' | 'user'>('all');

  const resetFilters = React.useCallback(() => {
    setQuery('');
    setOriginFilter('all');
  }, []);

  const filteredTemplates = React.useMemo(() => {
    return templates.filter((template) => {
      if (originFilter === 'system' && !template.is_system) return false;
      if (originFilter === 'user' && template.is_system) return false;
      return matchesSamplePipelineQuery(template, query);
    });
  }, [templates, originFilter, query]);

  const systemTemplates = React.useMemo(() => {
    return filteredTemplates
      .filter((template) => template.is_system)
      .slice()
      .sort(compareSamplePipelinesByName);
  }, [filteredTemplates]);

  const userTemplates = React.useMemo(() => {
    return filteredTemplates
      .filter((template) => !template.is_system)
      .slice()
      .sort(compareSamplePipelinesByName);
  }, [filteredTemplates]);

  const selectedExists = React.useMemo(() => {
    return templates.some((template) => template.id === selectedTemplateId);
  }, [templates, selectedTemplateId]);

  const selectedVisible = React.useMemo(() => {
    return filteredTemplates.some((template) => template.id === selectedTemplateId);
  }, [filteredTemplates, selectedTemplateId]);

  const showHiddenSelectionHint =
    selectedTemplateId &&
    selectedExists &&
    !selectedVisible &&
    (query.trim() || originFilter !== 'all');

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
        {systemTemplates.length === 0 && userTemplates.length === 0 && (
          <EmptyState>No pipelines match your filters.</EmptyState>
        )}

        {systemTemplates.length > 0 && (
          <Section>
            <SectionHeader>
              <span>System Pipelines</span>
              <SectionCount>{systemTemplates.length}</SectionCount>
            </SectionHeader>
            <TemplateGrid>
              {systemTemplates.map((template) => (
                <TemplateCard key={template.id} htmlFor={`template-${template.id}`}>
                  <RadioItem value={template.id} id={`template-${template.id}`}>
                    <RadioIndicator />
                  </RadioItem>
                  <TemplateContent>
                    <TemplateHeader>
                      <TemplateName>{template.name}</TemplateName>
                      <TemplateBadge variant={template.is_system ? 'system' : 'user'}>
                        {template.is_system ? 'System' : 'User'}
                      </TemplateBadge>
                    </TemplateHeader>
                    <TemplateDescription>{template.description}</TemplateDescription>
                  </TemplateContent>
                </TemplateCard>
              ))}
            </TemplateGrid>
          </Section>
        )}

        {userTemplates.length > 0 && (
          <Section>
            <SectionHeader>
              <span>User Pipelines</span>
              <SectionCount>{userTemplates.length}</SectionCount>
            </SectionHeader>
            <TemplateGrid>
              {userTemplates.map((template) => (
                <TemplateCard key={template.id} htmlFor={`template-${template.id}`}>
                  <RadioItem value={template.id} id={`template-${template.id}`}>
                    <RadioIndicator />
                  </RadioItem>
                  <TemplateContent>
                    <TemplateHeader>
                      <TemplateName>{template.name}</TemplateName>
                      <TemplateBadge variant={template.is_system ? 'system' : 'user'}>
                        {template.is_system ? 'System' : 'User'}
                      </TemplateBadge>
                    </TemplateHeader>
                    <TemplateDescription>{template.description}</TemplateDescription>
                  </TemplateContent>
                </TemplateCard>
              ))}
            </TemplateGrid>
          </Section>
        )}
      </RadioGroupRoot>
    </SelectorContainer>
  );
};
