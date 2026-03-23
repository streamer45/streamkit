// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Hook that manages the MoQ preview connection from the Monitor View.
 *
 * Encapsulates:
 * - Stream store selectors for watch-only MoQ connection
 * - Preview teardown when the selected session is deselected
 * - Pipeline-aware configuration (gateway path, output broadcast, media types)
 */

import { useCallback, useEffect, useRef } from 'react';
import { useShallow } from 'zustand/shallow';

import { useStreamStore } from '@/stores/streamStore';
import type { Pipeline } from '@/types/types';
import { updateUrlPath } from '@/utils/moqPeerSettings';

interface PreviewMoqConfig {
  gatewayPath?: string;
  outputBroadcast?: string;
  outputsAudio: boolean;
  outputsVideo: boolean;
}

/**
 * Derives MoQ preview configuration from the pipeline's node graph.
 * Used as a fallback for interactively-created sessions that don't have
 * a `client` section.
 */
function deriveMoqConfigFromNodes(pipeline: Pipeline): PreviewMoqConfig {
  const config: PreviewMoqConfig = { outputsAudio: true, outputsVideo: true };

  const moqEntry = Object.entries(pipeline.nodes).find(
    ([, n]) => n.kind === 'transport::moq::peer' && n.params
  );
  if (!moqEntry) return config;

  const [moqNodeName, moqNode] = moqEntry;
  const params = moqNode.params as Record<string, unknown>;
  config.gatewayPath = params.gateway_path as string | undefined;
  config.outputBroadcast = params.output_broadcast as string | undefined;

  // Detect media types from connection graph
  config.outputsAudio = false;
  config.outputsVideo = false;
  for (const conn of pipeline.connections) {
    if (conn.to_node !== moqNodeName) continue;
    const sourceNode = pipeline.nodes[conn.from_node];
    if (!sourceNode?.kind) continue;
    if (sourceNode.kind.startsWith('audio::')) config.outputsAudio = true;
    else if (sourceNode.kind.startsWith('video::')) config.outputsVideo = true;
  }

  return config;
}

export interface UseMonitorPreviewReturn {
  isPreviewConnected: boolean;
  handleStartPreview: () => Promise<void>;
}

export function useMonitorPreview(
  selectedSessionId: string | null,
  pipeline: Pipeline | undefined | null
): UseMonitorPreviewReturn {
  const {
    status: previewStatus,
    loadConfig: previewLoadConfig,
    connect: previewConnect,
    disconnect: previewDisconnect,
    setEnablePublish: previewSetEnablePublish,
    setEnableWatch: previewSetEnableWatch,
    configLoaded: previewConfigLoaded,
    setServerUrl: previewSetServerUrl,
    setOutputBroadcast: previewSetOutputBroadcast,
    setPipelineOutputTypes: previewSetPipelineOutputTypes,
  } = useStreamStore(
    useShallow((s) => ({
      status: s.status,
      loadConfig: s.loadConfig,
      connect: s.connect,
      disconnect: s.disconnect,
      setEnablePublish: s.setEnablePublish,
      setEnableWatch: s.setEnableWatch,
      configLoaded: s.configLoaded,
      setServerUrl: s.setServerUrl,
      setOutputBroadcast: s.setOutputBroadcast,
      setPipelineOutputTypes: s.setPipelineOutputTypes,
    }))
  );

  const isPreviewConnected = previewStatus === 'connected';

  // Tear down the MoQ preview (and release camera/mic) when the selected
  // session is deselected (transitions from a value to null).  We track
  // the previous value with a ref so we don't disconnect on initial mount
  // (where selectedSessionId starts as null while the nav-state or
  // auto-select effects haven't fired yet).
  const prevSelectedSessionIdRef = useRef(selectedSessionId);
  useEffect(() => {
    const prev = prevSelectedSessionIdRef.current;
    prevSelectedSessionIdRef.current = selectedSessionId;
    if (prev && !selectedSessionId && previewStatus !== 'disconnected') {
      previewDisconnect();
    }
  }, [selectedSessionId, previewStatus, previewDisconnect]);

  // Read MoQ peer settings from the pipeline's declarative client section
  // instead of scanning the compiled node graph.
  const handleStartPreview = useCallback(async () => {
    // Configure for watch-only mode (no publish/mic)
    previewSetEnablePublish(false);
    previewSetEnableWatch(true);
    if (!previewConfigLoaded) {
      await previewLoadConfig();
    }

    // Read gateway_path and output_broadcast from the pipeline's client section.
    // Fall back to scanning the node graph for interactively-created sessions
    // that don't have a client section.
    const client = pipeline?.client ?? null;
    let gatewayPath: string | undefined;
    let outputBroadcast: string | undefined;
    let outputsAudio = true;
    let outputsVideo = true;

    if (client) {
      gatewayPath = client.gateway_path ?? undefined;
      outputBroadcast = client.watch?.broadcast;
      outputsAudio = client.watch?.audio ?? true;
      outputsVideo = client.watch?.video ?? true;
    } else if (pipeline) {
      const fallback = deriveMoqConfigFromNodes(pipeline);
      gatewayPath = fallback.gatewayPath;
      outputBroadcast = fallback.outputBroadcast;
      outputsAudio = fallback.outputsAudio;
      outputsVideo = fallback.outputsVideo;
    }

    if (gatewayPath) {
      // Use the original config URL as the base so that the preview URL
      // isn't polluted by a relay URL the user previously selected.
      const baseUrl =
        useStreamStore.getState().configServerUrl || useStreamStore.getState().serverUrl;
      if (baseUrl) {
        previewSetServerUrl(updateUrlPath(baseUrl, gatewayPath));
      }
    }
    if (outputBroadcast) {
      previewSetOutputBroadcast(outputBroadcast);
    }
    previewSetPipelineOutputTypes(outputsAudio, outputsVideo);

    await previewConnect();
  }, [
    previewSetEnablePublish,
    previewSetEnableWatch,
    previewConfigLoaded,
    previewLoadConfig,
    previewConnect,
    pipeline,
    previewSetServerUrl,
    previewSetOutputBroadcast,
    previewSetPipelineOutputTypes,
  ]);

  return { isPreviewConnected, handleStartPreview };
}
