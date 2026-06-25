// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { useStreamViewState } from '@/hooks/useStreamViewState';
import { listDynamicSamples } from '@/services/samples';
import { useStreamStore } from '@/stores/streamStore';
import { getLogger } from '@/utils/logger';
import { orderSamplePipelinesSystemFirst } from '@/utils/samplePipelineOrdering';

const logger = getLogger('streamSamples');

/**
 * Load dynamic pipeline samples and auto-select the first one.
 *
 * The first derive resolves the MoQ server URL from `configServerUrl`, which
 * `loadConfig()` populates asynchronously. Config and samples are fetched in
 * parallel (`Promise.all`), so the auto-select applies the template and derives
 * its MoQ settings synchronously once config is loaded — the URL is present even
 * when the sample list resolves first (otherwise the field stays blank on a cold
 * load until the user edits the client section, issue #604).
 */
export async function loadAndApplySamples(
  viewState: ReturnType<typeof useStreamViewState>,
  deriveMoqFromYaml: (yaml: string) => void
): Promise<void> {
  const { configLoaded, loadConfig } = useStreamStore.getState();
  try {
    viewState.setSamplesLoading(true);
    viewState.setSamplesError(null);
    const [, samples] = await Promise.all([
      configLoaded ? Promise.resolve() : loadConfig(),
      listDynamicSamples(),
    ]);
    const orderedSamples = orderSamplePipelinesSystemFirst(samples);
    viewState.setSamples(orderedSamples);

    if (orderedSamples.length > 0 && !viewState.selectedTemplateId) {
      const first = orderedSamples[0];
      viewState.setSelectedTemplateId(first.id);
      viewState.setPipelineYaml(first.yaml);
      deriveMoqFromYaml(first.yaml);
    }
  } catch (error) {
    logger.error('Failed to load dynamic samples:', error);
    viewState.setSamplesError(
      error instanceof Error ? error.message : 'Failed to load pipeline templates'
    );
  } finally {
    viewState.setSamplesLoading(false);
  }
}
