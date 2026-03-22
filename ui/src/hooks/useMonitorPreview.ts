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
    const client = pipeline?.client ?? null;
    if (client) {
      if (client.gateway_path) {
        const currentUrl = useStreamStore.getState().serverUrl;
        if (currentUrl) {
          previewSetServerUrl(updateUrlPath(currentUrl, client.gateway_path));
        }
      }
      if (client.watch?.broadcast) {
        previewSetOutputBroadcast(client.watch.broadcast);
      }
    }

    // Media types default to both enabled unless the client section
    // explicitly declares which types the pipeline outputs.
    const outputsAudio = client?.watch?.audio ?? true;
    const outputsVideo = client?.watch?.video ?? true;
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
