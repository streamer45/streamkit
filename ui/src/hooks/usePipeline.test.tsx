// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { renderHook } from '@testing-library/react';
import type { ReactNode } from 'react';
import { describe, it, expect, beforeEach } from 'vitest';

import { ToastProvider } from '@/context/ToastContext';

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

beforeEach(() => {
  window.localStorage.clear();
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
});
