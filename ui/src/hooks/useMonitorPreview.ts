// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Hook that manages the server-managed MoQ preview connection from the
 * Monitor View.
 *
 * Encapsulates:
 * - Calling the preview REST API (start / stop)
 * - Stream store configuration for watch-only MoQ connection
 * - Preview teardown when the selected session is deselected
 * - Loading and error states
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import { useShallow } from 'zustand/shallow';

import { startPreview, stopPreview } from '@/services/sessions';
import { useStreamStore } from '@/stores/streamStore';
import { updateUrlPath } from '@/utils/moqPeerSettings';

export interface UseMonitorPreviewReturn {
  isPreviewConnected: boolean;
  isPreviewLoading: boolean;
  previewError: string | null;
  handleStartPreview: () => Promise<void>;
  handleStopPreview: () => Promise<void>;
}

export function useMonitorPreview(selectedSessionId: string | null): UseMonitorPreviewReturn {
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

  const [isPreviewLoading, setIsPreviewLoading] = useState(false);
  const [previewError, setPreviewError] = useState<string | null>(null);

  const isPreviewConnected = previewStatus === 'connected';

  // Track the active preview ID so we can tear it down on the server.
  const previewIdRef = useRef<string | null>(null);
  // Track the session the preview belongs to.
  const previewSessionIdRef = useRef<string | null>(null);

  // Tear down the MoQ preview when the selected session is deselected
  // (transitions from a value to null).
  const prevSelectedSessionIdRef = useRef(selectedSessionId);
  useEffect(() => {
    const prev = prevSelectedSessionIdRef.current;
    prevSelectedSessionIdRef.current = selectedSessionId;

    if (prev && prev !== selectedSessionId) {
      // Session changed or deselected — clean up server-side preview and MoQ connection
      if (previewIdRef.current && previewSessionIdRef.current) {
        stopPreview(previewSessionIdRef.current, previewIdRef.current).catch(() => {});
        previewIdRef.current = null;
        previewSessionIdRef.current = null;
      }
      if (previewStatus !== 'disconnected') {
        previewDisconnect();
      }
      setPreviewError(null);
    }
  }, [selectedSessionId, previewStatus, previewDisconnect]);

  const handleStartPreview = useCallback(async () => {
    if (!selectedSessionId) return;

    setIsPreviewLoading(true);
    setPreviewError(null);

    try {
      // Configure for watch-only mode
      previewSetEnablePublish(false);
      previewSetEnableWatch(true);
      if (!previewConfigLoaded) {
        await previewLoadConfig();
      }

      // Ask the server to inject a preview subgraph
      const result = await startPreview(selectedSessionId);

      previewIdRef.current = result.preview_id;
      previewSessionIdRef.current = selectedSessionId;

      // Configure the stream store with the returned gateway path
      const baseUrl =
        useStreamStore.getState().configServerUrl || useStreamStore.getState().serverUrl;
      if (baseUrl) {
        previewSetServerUrl(updateUrlPath(baseUrl, result.gateway_path));
      }
      previewSetOutputBroadcast(result.broadcast);
      previewSetPipelineOutputTypes(result.audio, result.video);

      await previewConnect();
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Failed to start preview';
      setPreviewError(message);
      // Clean up partial state
      if (previewIdRef.current && previewSessionIdRef.current) {
        stopPreview(previewSessionIdRef.current, previewIdRef.current).catch(() => {});
        previewIdRef.current = null;
        previewSessionIdRef.current = null;
      }
    } finally {
      setIsPreviewLoading(false);
    }
  }, [
    selectedSessionId,
    previewSetEnablePublish,
    previewSetEnableWatch,
    previewConfigLoaded,
    previewLoadConfig,
    previewConnect,
    previewSetServerUrl,
    previewSetOutputBroadcast,
    previewSetPipelineOutputTypes,
  ]);

  const handleStopPreview = useCallback(async () => {
    // Tear down server-side preview
    if (previewIdRef.current && previewSessionIdRef.current) {
      try {
        await stopPreview(previewSessionIdRef.current, previewIdRef.current);
      } catch {
        // Best-effort teardown; the server may have already cleaned up
      }
      previewIdRef.current = null;
      previewSessionIdRef.current = null;
    }
    // Disconnect the MoQ watch subscription
    if (previewStatus !== 'disconnected') {
      previewDisconnect();
    }
    setPreviewError(null);
  }, [previewStatus, previewDisconnect]);

  return {
    isPreviewConnected,
    isPreviewLoading,
    previewError,
    handleStartPreview,
    handleStopPreview,
  };
}
