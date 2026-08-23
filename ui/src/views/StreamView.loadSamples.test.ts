// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { useStreamViewState } from '@/hooks/useStreamViewState';
import { useStreamStore } from '@/stores/streamStore';
import type { SamplePipeline } from '@/types/generated/api-types';

import { loadAndApplySamples } from './streamSamples';

// streamSamples imports the stream store, which pulls in the WebTransport-backed
// @moq/* libraries; stub them so the module loads in jsdom.
vi.mock('@moq/net', () => ({
  default: {},
  Connection: { Reload: vi.fn() },
  Path: { from: vi.fn() },
}));
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
    selectedTemplateId: '',
    setSamples: vi.fn(),
    setSamplesLoading: vi.fn(),
    setSamplesError: vi.fn(),
    setSelectedTemplateId: vi.fn(),
    setPipelineYaml: vi.fn(),
  } as unknown as ReturnType<typeof useStreamViewState>;
}

describe('loadAndApplySamples (cold-load ordering)', () => {
  beforeEach(() => {
    listDynamicSamples.mockReset();
  });

  afterEach(() => {
    useStreamStore.setState({ configLoaded: false });
  });

  it('defers the first derive until config finishes loading', async () => {
    listDynamicSamples.mockResolvedValue([sample('a')]);
    const viewState = makeViewState();
    const deriveMoqFromYaml = vi.fn();

    let resolveConfig!: () => void;
    const loadConfig = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolveConfig = () => resolve();
        })
    );
    useStreamStore.setState({ configLoaded: false, loadConfig });

    const done = loadAndApplySamples(viewState, deriveMoqFromYaml);

    // The sample list resolves first, but nothing is applied until config
    // loads, so the derive never resolves against an empty configServerUrl (#604).
    await Promise.resolve();
    expect(loadConfig).toHaveBeenCalledTimes(1);
    expect(viewState.setPipelineYaml).not.toHaveBeenCalled();
    expect(deriveMoqFromYaml).not.toHaveBeenCalled();

    resolveConfig();
    await done;
    expect(viewState.setPipelineYaml).toHaveBeenCalledWith(sample('a').yaml);
    expect(deriveMoqFromYaml).toHaveBeenCalledWith(sample('a').yaml);
  });

  it('does not reload config when it is already loaded (warm load)', async () => {
    listDynamicSamples.mockResolvedValue([sample('a')]);
    const viewState = makeViewState();
    const deriveMoqFromYaml = vi.fn();

    const loadConfig = vi.fn(() => Promise.resolve());
    useStreamStore.setState({ configLoaded: true, loadConfig });

    await loadAndApplySamples(viewState, deriveMoqFromYaml);

    expect(loadConfig).not.toHaveBeenCalled();
    expect(deriveMoqFromYaml).toHaveBeenCalledWith(sample('a').yaml);
  });

  it('reports an error and stops loading when the sample fetch fails', async () => {
    listDynamicSamples.mockRejectedValue(new Error('boom'));
    const viewState = makeViewState();
    const deriveMoqFromYaml = vi.fn();

    useStreamStore.setState({ configLoaded: true, loadConfig: vi.fn() });

    await loadAndApplySamples(viewState, deriveMoqFromYaml);

    expect(viewState.setSamplesError).toHaveBeenCalledWith('boom');
    expect(viewState.setSamplesLoading).toHaveBeenLastCalledWith(false);
    expect(deriveMoqFromYaml).not.toHaveBeenCalled();
  });
});
