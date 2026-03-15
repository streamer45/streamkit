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

  // Extract MoQ peer settings from the selected session's pipeline so the
  // preview connects to the correct gateway path and output broadcast.
  const handleStartPreview = useCallback(async () => {
    // Configure for watch-only mode (no publish/mic)
    previewSetEnablePublish(false);
    previewSetEnableWatch(true);
    if (!previewConfigLoaded) {
      await previewLoadConfig();
    }

    // Extract gateway_path and output_broadcast from the pipeline's moq_peer node
    let moqNodeName: string | undefined;
    const moqNode = pipeline
      ? Object.entries(pipeline.nodes).find(
          ([, n]) => n.kind === 'transport::moq::peer' && n.params
        )
      : undefined;
    if (moqNode) {
      moqNodeName = moqNode[0];
      const params = moqNode[1].params as Record<string, unknown>;
      const gatewayPath = params.gateway_path as string | undefined;
      const outputBroadcast = params.output_broadcast as string | undefined;
      // Read serverUrl at call-time via getState() rather than via a
      // hook selector — this is a standard Zustand pattern for values
      // that should be fresh when the callback fires, not stale from
      // the last render.
      const currentUrl = useStreamStore.getState().serverUrl;
      if (gatewayPath && currentUrl) {
        previewSetServerUrl(updateUrlPath(currentUrl, gatewayPath));
      }
      if (outputBroadcast) {
        previewSetOutputBroadcast(outputBroadcast);
      }
    }

    // Detect which media types the pipeline outputs by checking the kinds of
    // nodes connected to the moq_peer's input pins.
    let outputsAudio = true;
    let outputsVideo = true;
    if (pipeline && moqNodeName) {
      outputsAudio = false;
      outputsVideo = false;
      for (const conn of pipeline.connections) {
        if (conn.to_node !== moqNodeName) continue;
        const sourceNode = pipeline.nodes[conn.from_node];
        if (!sourceNode?.kind) continue;
        if (sourceNode.kind.startsWith('audio::')) outputsAudio = true;
        else if (sourceNode.kind.startsWith('video::')) outputsVideo = true;
      }
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
