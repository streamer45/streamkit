// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, expect, it, vi } from 'vitest';

import type { useStreamViewState } from '@/hooks/useStreamViewState';
import type { SamplePipeline } from '@/types/generated/api-types';

import { loadAndApplySamples } from './StreamView';

// StreamView transitively imports the stream store, which pulls in the
// WebTransport-backed @moq/* libraries; stub them so the module loads in jsdom.
vi.mock('@moq/hang', () => ({ default: {} }));
vi.mock('@moq/watch', () => ({ default: {}, Broadcast: vi.fn() }));
vi.mock('@moq/publish', () => ({ default: {}, Broadcast: vi.fn() }));
vi.mock('@moq/signals', () => ({ Effect: vi.fn() }));

const listDynamicSamples = vi.fn();
vi.mock('@/services/samples', () => ({
  listDynamicSamples: () => listDynamicSamples(),
}));

const sample = (id: string): SamplePipeline =>
  ({ id, name: id, yaml: `client:\n  gateway_path: /moq/${id}\n` }) as SamplePipeline;

function makeViewState(): ReturnType<typeof useStreamViewState> {
  return {
    selectedTemplateId: null,
    setSamples: vi.fn(),
    setSamplesLoading: vi.fn(),
    setSamplesError: vi.fn(),
    setSelectedTemplateId: vi.fn(),
    setPipelineYaml: vi.fn(),
  } as unknown as ReturnType<typeof useStreamViewState>;
}

describe('loadAndApplySamples (cold-load ordering)', () => {
  it('defers the first derive until config is ready', async () => {
    listDynamicSamples.mockResolvedValue([sample('a')]);
    const viewState = makeViewState();
    const deriveMoqFromYaml = vi.fn();

    let resolveConfig!: () => void;
    const configReady = new Promise<void>((resolve) => {
      resolveConfig = resolve;
    });

    const done = loadAndApplySamples(viewState, deriveMoqFromYaml, configReady);

    // The sample list resolves first; the derive must wait for config so the
    // server URL is never resolved against an empty configServerUrl (#604).
    await Promise.resolve();
    expect(viewState.setPipelineYaml).toHaveBeenCalledWith(sample('a').yaml);
    expect(deriveMoqFromYaml).not.toHaveBeenCalled();

    resolveConfig();
    await done;
    expect(deriveMoqFromYaml).toHaveBeenCalledWith(sample('a').yaml);
  });
});
