// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { renderHook, act, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { describe, it, expect, beforeEach } from 'vitest';

import { ToastProvider } from '@/context/ToastContext';
import { useSchemaStore } from '@/stores/schemaStore';
import type { NodeDefinition } from '@/types/types';

import { usePipeline } from './usePipeline';

const LOCAL_STORAGE_KEY = 'sk-pipeline-draft';

const wrapper = ({ children }: { children: ReactNode }) => (
  <ToastProvider>{children}</ToastProvider>
);

function seedDraft(nodes: Array<{ id: string; label: string; kind: string }>) {
  window.localStorage.setItem(
    LOCAL_STORAGE_KEY,
    JSON.stringify({
      nodes: nodes.map((n, i) => ({
        id: n.id,
        position: { x: i * 100, y: 0 },
        data: { label: n.label, kind: n.kind },
      })),
      edges: [],
      mode: 'dynamic',
    })
  );
}

const decoderDef: NodeDefinition = {
  kind: 'audio::decoder',
  param_schema: {},
  inputs: [],
  outputs: [],
  categories: ['audio'],
  bidirectional: false,
};

beforeEach(() => {
  window.localStorage.clear();
  useSchemaStore.getState().setNodeDefinitions([decoderDef]);
});

describe('usePipeline draft restoration', () => {
  it('restores the node id counter past the highest saved id', () => {
    seedDraft([
      { id: 'skitnode_5', label: 'audio_decoder_1', kind: 'audio::decoder' },
      { id: 'skitnode_2', label: 'audio_encoder_1', kind: 'audio::encoder' },
    ]);

    const { result } = renderHook(() => usePipeline(), { wrapper });

    expect(result.current.getId()).toBe('skitnode_6');
  });

  it('restores label counters past the highest saved suffix per kind', () => {
    seedDraft([
      { id: 'skitnode_1', label: 'audio_decoder_3', kind: 'audio::decoder' },
      { id: 'skitnode_2', label: 'audio_decoder_1', kind: 'audio::decoder' },
    ]);

    const { result } = renderHook(() => usePipeline(), { wrapper });

    expect(result.current.nextLabelForKind('audio_decoder')).toBe('audio_decoder_4');
  });

  it('hydrates nodes, edges, name and description from the saved draft', () => {
    window.localStorage.setItem(
      LOCAL_STORAGE_KEY,
      JSON.stringify({
        nodes: [
          {
            id: 'skitnode_1',
            position: { x: 0, y: 0 },
            data: { label: 'audio_decoder_1', kind: 'audio::decoder' },
          },
        ],
        edges: [{ id: 'e1', source: 'skitnode_1', target: 'skitnode_1' }],
        mode: 'oneshot',
        name: 'my pipeline',
        description: 'a draft',
      })
    );

    const { result } = renderHook(() => usePipeline(), { wrapper });

    expect(result.current.nodes).toHaveLength(1);
    expect(result.current.nodes[0].id).toBe('skitnode_1');
    expect(result.current.edges).toHaveLength(1);
    expect(result.current.mode).toBe('oneshot');
    expect(result.current.pipelineName).toBe('my pipeline');
    expect(result.current.pipelineDescription).toBe('a draft');
  });

  it('ignores malformed drafts and starts empty', () => {
    window.localStorage.setItem(LOCAL_STORAGE_KEY, '{not valid json');

    const { result } = renderHook(() => usePipeline(), { wrapper });

    expect(result.current.nodes).toHaveLength(0);
    expect(result.current.edges).toHaveLength(0);
  });
});

describe('usePipeline YAML round-trip', () => {
  it('imports YAML into canvas nodes and persists name/description', async () => {
    const { result } = renderHook(() => usePipeline(), { wrapper });

    const yaml = ['steps:', '  - kind: audio::decoder'].join('\n');
    act(() => {
      result.current.handleImportYaml(yaml, 'imported description', 'imported name');
    });

    expect(result.current.nodes).toHaveLength(1);
    expect(result.current.nodes[0].data.kind).toBe('audio::decoder');
    expect(result.current.pipelineName).toBe('imported name');
    expect(result.current.pipelineDescription).toBe('imported description');
    expect(result.current.yamlError).toBe('');

    await waitFor(() => {
      const saved = JSON.parse(window.localStorage.getItem(LOCAL_STORAGE_KEY) ?? '{}');
      expect(saved.name).toBe('imported name');
      expect(saved.nodes).toHaveLength(1);
    });
  });

  it('surfaces a parse error from edited YAML without touching the canvas', async () => {
    const { result } = renderHook(() => usePipeline(), { wrapper });

    act(() => {
      result.current.handleYamlChange('42');
    });

    await waitFor(() => expect(result.current.yamlError).not.toBe(''));
    expect(result.current.nodes).toHaveLength(0);
  });

  it('applies edited YAML to the canvas after the debounce', async () => {
    const { result } = renderHook(() => usePipeline(), { wrapper });

    const yaml = ['steps:', '  - kind: audio::decoder'].join('\n');
    act(() => {
      result.current.handleYamlChange(yaml);
    });

    await waitFor(() => expect(result.current.nodes).toHaveLength(1));
    expect(result.current.yamlError).toBe('');
  });

  it('regenerates YAML from a canvas snapshot', () => {
    const { result } = renderHook(() => usePipeline(), { wrapper });

    act(() => {
      result.current.regenerateYamlFromCanvas({
        nodes: [
          {
            id: 'skitnode_1',
            position: { x: 0, y: 0 },
            data: { label: 'decoder_1', kind: 'audio::decoder' },
          },
        ],
        edges: [],
        mode: 'dynamic',
      });
    });

    expect(result.current.yamlString).toContain('decoder_1');
    expect(result.current.yamlString).toContain('audio::decoder');
  });

  it('resets the YAML placeholder when the canvas snapshot is empty', () => {
    const { result } = renderHook(() => usePipeline(), { wrapper });

    act(() => {
      result.current.regenerateYamlFromCanvas({ nodes: [], edges: [], mode: 'dynamic' });
    });

    expect(result.current.yamlString).toContain('# Add nodes to the canvas');
  });
});

describe('usePipeline label changes', () => {
  it('renames a node through handleLabelChange', async () => {
    seedDraft([{ id: 'skitnode_1', label: 'decoder_1', kind: 'audio::decoder' }]);

    const { result } = renderHook(() => usePipeline(), { wrapper });

    act(() => {
      result.current.handleLabelChange('skitnode_1', 'renamed');
    });

    await waitFor(() => expect(result.current.nodes[0].data.label).toBe('renamed'));
  });
});
