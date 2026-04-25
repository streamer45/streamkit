// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { render, screen, fireEvent } from '@testing-library/react';
import { describe, it, expect, vi } from 'vitest';

import type { NodeDefinition } from '@/types/types';

import NodePalette from './NodePalette';

const makeDef = (kind: string, categories: string[], description?: string): NodeDefinition => ({
  kind,
  description: description ?? null,
  param_schema: {},
  inputs: [],
  outputs: [],
  categories,
  bidirectional: false,
});

const SAMPLE_DEFS: NodeDefinition[] = [
  makeDef('core::passthrough', ['core'], 'Forwards packets unchanged'),
  makeDef('core::file_reader', ['io', 'file'], 'Reads binary data from a file'),
  makeDef('audio::opus_encoder', ['audio', 'codecs', 'opus'], 'Opus encoder'),
  makeDef('video::compositor', ['video', 'compositing'], 'Composites video layers'),
  makeDef('transport::moq_pull', ['transport', 'moq'], 'MoQ subscriber'),
  makeDef('plugin::native::whisper', ['audio', 'speech-to-text'], 'Speech recognition'),
];

const PLUGIN_KINDS = new Set(['plugin::native::whisper']);
const PLUGIN_TYPES = new Map<string, 'wasm' | 'native'>([['plugin::native::whisper', 'native']]);

describe('NodePalette', () => {
  const defaultProps = {
    nodeDefinitions: SAMPLE_DEFS,
    onDragStart: vi.fn(),
    pluginKinds: PLUGIN_KINDS,
    pluginTypes: PLUGIN_TYPES,
  };

  it('renders the search input', () => {
    render(<NodePalette {...defaultProps} />);
    expect(screen.getByTestId('node-search-input')).toBeInTheDocument();
  });

  it('shows top-level categories when no search is active', () => {
    render(<NodePalette {...defaultProps} />);
    expect(screen.getByLabelText('Open audio')).toBeInTheDocument();
    expect(screen.getByLabelText('Open core')).toBeInTheDocument();
    expect(screen.getByLabelText('Open io')).toBeInTheDocument();
    expect(screen.getByLabelText('Open transport')).toBeInTheDocument();
    expect(screen.getByLabelText('Open video')).toBeInTheDocument();
  });

  it('shows filter chips including Plugin when plugins exist', () => {
    render(<NodePalette {...defaultProps} />);
    const chips = screen.getByTestId('filter-chips');
    expect(chips).toBeInTheDocument();
    expect(screen.getByLabelText('Filter by Plugin')).toBeInTheDocument();
  });

  it('filters nodes by search query on kind', () => {
    render(<NodePalette {...defaultProps} />);
    const input = screen.getByTestId('node-search-input');
    fireEvent.change(input, { target: { value: 'opus' } });

    expect(screen.getByText('1 node found')).toBeInTheDocument();
    expect(screen.getByText('audio::opus_encoder')).toBeInTheDocument();
    // Categories should not show as cards when search is active
    expect(screen.queryByLabelText('Open core')).not.toBeInTheDocument();
  });

  it('filters nodes by search query on description', () => {
    render(<NodePalette {...defaultProps} />);
    const input = screen.getByTestId('node-search-input');
    fireEvent.change(input, { target: { value: 'speech' } });

    expect(screen.getByText('1 node found')).toBeInTheDocument();
    expect(screen.getByText('plugin::native::whisper')).toBeInTheDocument();
  });

  it('filters nodes by search query on category', () => {
    render(<NodePalette {...defaultProps} />);
    const input = screen.getByTestId('node-search-input');
    fireEvent.change(input, { target: { value: 'compositing' } });

    expect(screen.getByText('1 node found')).toBeInTheDocument();
    expect(screen.getByText('video::compositor')).toBeInTheDocument();
  });

  it('shows "No nodes match" when search has no results', () => {
    render(<NodePalette {...defaultProps} />);
    const input = screen.getByTestId('node-search-input');
    fireEvent.change(input, { target: { value: 'zzzznonexistent' } });

    expect(screen.getByText('No nodes match your search')).toBeInTheDocument();
  });

  it('filters by category chip', () => {
    render(<NodePalette {...defaultProps} />);
    const audioChip = screen.getByLabelText('Filter by audio');
    fireEvent.click(audioChip);

    // Should show audio nodes (opus_encoder and whisper which is also in audio)
    expect(screen.getByText('2 nodes found')).toBeInTheDocument();
    expect(screen.getByText('audio::opus_encoder')).toBeInTheDocument();
    expect(screen.getByText('plugin::native::whisper')).toBeInTheDocument();
  });

  it('filters by Plugin chip', () => {
    render(<NodePalette {...defaultProps} />);
    const pluginChip = screen.getByLabelText('Filter by Plugin');
    fireEvent.click(pluginChip);

    expect(screen.getByText('1 node found')).toBeInTheDocument();
    expect(screen.getByText('plugin::native::whisper')).toBeInTheDocument();
  });

  it('combines search and filter chip', () => {
    render(<NodePalette {...defaultProps} />);
    const audioChip = screen.getByLabelText('Filter by audio');
    fireEvent.click(audioChip);
    const input = screen.getByTestId('node-search-input');
    fireEvent.change(input, { target: { value: 'opus' } });

    expect(screen.getByText('1 node found')).toBeInTheDocument();
    expect(screen.getByText('audio::opus_encoder')).toBeInTheDocument();
  });

  it('clears search when clear button is clicked', () => {
    render(<NodePalette {...defaultProps} />);
    const input = screen.getByTestId('node-search-input');
    fireEvent.change(input, { target: { value: 'opus' } });
    expect(screen.getByText('1 node found')).toBeInTheDocument();

    const clearBtn = screen.getByLabelText('Clear search');
    fireEvent.click(clearBtn);

    // Should return to category view (no "nodes found" text)
    expect(screen.queryByText(/nodes? found/)).not.toBeInTheDocument();
  });

  it('shows category breadcrumbs in search results', () => {
    render(<NodePalette {...defaultProps} />);
    const input = screen.getByTestId('node-search-input');
    fireEvent.change(input, { target: { value: 'file_reader' } });

    expect(screen.getByText('io › file')).toBeInTheDocument();
  });

  it('does not show Plugin chip when there are no plugins', () => {
    render(<NodePalette {...defaultProps} pluginKinds={new Set()} />);
    expect(screen.queryByLabelText('Filter by Plugin')).not.toBeInTheDocument();
  });

  it('toggles filter chip off on second click', () => {
    render(<NodePalette {...defaultProps} />);
    const audioChip = screen.getByLabelText('Filter by audio');
    fireEvent.click(audioChip);
    expect(screen.getByText('2 nodes found')).toBeInTheDocument();

    fireEvent.click(audioChip);
    // Should return to category view
    expect(screen.queryByText(/nodes? found/)).not.toBeInTheDocument();
  });
});
