// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { render, screen, fireEvent } from '@testing-library/react';
import React from 'react';
import { describe, it, expect, vi } from 'vitest';

import type { SamplePipeline } from '@/types/generated/api-types';

import { TemplateSelector } from './TemplateSelector';

function makePipeline(overrides: Partial<SamplePipeline> = {}): SamplePipeline {
  return {
    id: 'tpl-1',
    name: 'Test Pipeline',
    description: 'A test pipeline',
    yaml: 'steps: []',
    is_system: true,
    mode: 'oneshot',
    is_fragment: false,
    upload_fields: null,
    ...overrides,
  };
}

const SYSTEM_TPL = makePipeline({ id: 'sys-1', name: 'Transcribe Audio', is_system: true });
const USER_TPL = makePipeline({
  id: 'usr-1',
  name: 'My Custom Pipeline',
  is_system: false,
  description: 'Custom user pipeline',
});

describe('TemplateSelector', () => {
  const defaultProps = {
    templates: [SYSTEM_TPL, USER_TPL],
    selectedTemplateId: '',
    onTemplateSelect: vi.fn(),
  };

  it('renders system and user templates', () => {
    render(<TemplateSelector {...defaultProps} />);
    expect(screen.getByText('Transcribe Audio')).toBeInTheDocument();
    expect(screen.getByText('My Custom Pipeline')).toBeInTheDocument();
  });

  it('renders section headers with counts', () => {
    render(<TemplateSelector {...defaultProps} />);
    expect(screen.getByText('System Pipelines')).toBeInTheDocument();
    expect(screen.getByText('User Pipelines')).toBeInTheDocument();
    const counts = screen.getAllByText('1');
    expect(counts.length).toBe(2);
  });

  it('renders empty state when no templates match filters', () => {
    render(<TemplateSelector {...defaultProps} templates={[]} />);
    expect(screen.getByText('No pipelines match your filters.')).toBeInTheDocument();
  });

  it('filters by search query', () => {
    render(<TemplateSelector {...defaultProps} />);

    const searchInput = screen.getByPlaceholderText('Search pipelines…');
    fireEvent.change(searchInput, { target: { value: 'Transcribe' } });

    expect(screen.getByText('Transcribe Audio')).toBeInTheDocument();
    expect(screen.queryByText('My Custom Pipeline')).not.toBeInTheDocument();
  });

  it('filters by origin (system only)', () => {
    render(<TemplateSelector {...defaultProps} />);

    const systemButton = screen.getByRole('button', { name: 'System' });
    fireEvent.click(systemButton);

    expect(screen.getByText('Transcribe Audio')).toBeInTheDocument();
    expect(screen.queryByText('My Custom Pipeline')).not.toBeInTheDocument();
  });

  it('filters by origin (user only)', () => {
    render(<TemplateSelector {...defaultProps} />);

    const userButton = screen.getByRole('button', { name: 'User' });
    fireEvent.click(userButton);

    expect(screen.queryByText('Transcribe Audio')).not.toBeInTheDocument();
    expect(screen.getByText('My Custom Pipeline')).toBeInTheDocument();
  });

  it('shows hidden selection hint when selected template is filtered out', () => {
    render(<TemplateSelector {...defaultProps} selectedTemplateId="usr-1" />);

    const systemButton = screen.getByRole('button', { name: 'System' });
    fireEvent.click(systemButton);

    expect(screen.getByText('Selected template is hidden by your filters.')).toBeInTheDocument();
  });

  it('clears filters when hint button is clicked', () => {
    render(<TemplateSelector {...defaultProps} selectedTemplateId="usr-1" />);

    const systemButton = screen.getByRole('button', { name: 'System' });
    fireEvent.click(systemButton);

    const clearButton = screen.getByText('Clear filters');
    fireEvent.click(clearButton);

    expect(screen.getByText('Transcribe Audio')).toBeInTheDocument();
    expect(screen.getByText('My Custom Pipeline')).toBeInTheDocument();
  });

  it('calls onTemplateSelect when a template is selected', () => {
    const onSelect = vi.fn();
    render(<TemplateSelector {...defaultProps} onTemplateSelect={onSelect} />);

    const radio = screen.getByRole('radio', { name: /Transcribe Audio/i });
    fireEvent.click(radio);

    expect(onSelect).toHaveBeenCalledWith('sys-1');
  });

  it('shows empty state after search yields no results', () => {
    render(<TemplateSelector {...defaultProps} />);

    const searchInput = screen.getByPlaceholderText('Search pipelines…');
    fireEvent.change(searchInput, { target: { value: 'nonexistent query xyz' } });

    expect(screen.getByText('No pipelines match your filters.')).toBeInTheDocument();
  });

  it('renders search input with accessible label', () => {
    render(<TemplateSelector {...defaultProps} />);
    expect(screen.getByLabelText('Search pipeline templates')).toBeInTheDocument();
  });

  it('renders filter group with accessible label', () => {
    render(<TemplateSelector {...defaultProps} />);
    expect(screen.getByRole('group', { name: 'Filter templates by origin' })).toBeInTheDocument();
  });
});
