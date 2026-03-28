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
 * - Preview teardown when the selected session changes or the component unmounts
 * - Loading and error states
 * - Cancellation of in-flight startPreview requests on session switch
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

/** Tear down the server-side preview and disconnect the MoQ subscription. */
async function cleanupPreview(
  previewIdRef: React.MutableRefObject<string | null>,
  previewSessionIdRef: React.MutableRefObject<string | null>,
  previewDisconnect: () => void
) {
  if (previewIdRef.current && previewSessionIdRef.current) {
    try {
      await stopPreview(previewSessionIdRef.current, previewIdRef.current);
    } catch {
      // Best-effort cleanup — the preview may already be gone.
    }
    previewIdRef.current = null;
    previewSessionIdRef.current = null;
  }
  // Always call disconnect — it's safe when already disconnected and
  // avoids stale-closure bugs with previewStatus.
  previewDisconnect();
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
  // MoQ connection establishment happens after the API call returns,
  // so there's an intermediate "connecting" state that should still
  // show a loading indicator to avoid a brief flicker back to the
  // "Preview" button.
  const isMoqConnecting = previewStatus === 'connecting';

  // Track the active preview ID so we can tear it down on the server.
  const previewIdRef = useRef<string | null>(null);
  // Track the session the preview belongs to.
  const previewSessionIdRef = useRef<string | null>(null);

  // AbortController cancels an in-flight startPreview API call when the
  // user switches sessions before the call completes.
  const abortControllerRef = useRef<AbortController | null>(null);

  // Session-change cleanup: fires when selectedSessionId changes to a
  // different value (including null). Also handles component unmount.
  const prevSelectedSessionIdRef = useRef(selectedSessionId);
  useEffect(() => {
    const prev = prevSelectedSessionIdRef.current;
    prevSelectedSessionIdRef.current = selectedSessionId;

    if (prev && prev !== selectedSessionId) {
      // Abort any in-flight startPreview request
      abortControllerRef.current?.abort();
      abortControllerRef.current = null;

      // Fire-and-forget: React effect cleanup cannot return a Promise, so
      // we intentionally void the async cleanup here.  The user-facing
      // handleStartPreview path correctly awaits its own cleanup before
      // starting a new preview — this path only runs on session switch
      // where the old session is no longer relevant.
      void cleanupPreview(previewIdRef, previewSessionIdRef, previewDisconnect);
      setPreviewError(null);
      setIsPreviewLoading(false);
    }

    // Cleanup on unmount — fire-and-forget for the same reason as above.
    // disconnect() is safe when already disconnected.
    return () => {
      abortControllerRef.current?.abort();
      abortControllerRef.current = null;
      void cleanupPreview(previewIdRef, previewSessionIdRef, previewDisconnect);
    };
  }, [selectedSessionId, previewDisconnect]);

  const handleStartPreview = useCallback(async () => {
    if (!selectedSessionId) return;

    // Clean up any existing server-side preview before creating a new one.
    // This handles the case where the MoQ connection dropped but the
    // server-side preview subgraph is still active.
    // Await the stop to avoid racing with the new startPreview — if the
    // server processes start before stop, the old preview could count
    // toward the limit and cause a spurious 409.
    if (previewIdRef.current && previewSessionIdRef.current) {
      try {
        await stopPreview(previewSessionIdRef.current, previewIdRef.current);
      } catch {
        // Best-effort cleanup — proceed even if the old preview is gone.
      }
      previewIdRef.current = null;
      previewSessionIdRef.current = null;
    }

    // Cancel any previous in-flight request
    abortControllerRef.current?.abort();
    const controller = new AbortController();
    abortControllerRef.current = controller;

    setIsPreviewLoading(true);
    setPreviewError(null);

    try {
      // Configure for watch-only mode
      previewSetEnablePublish(false);
      previewSetEnableWatch(true);
      if (!previewConfigLoaded) {
        await previewLoadConfig();
      }

      // Check for cancellation after the config load await
      if (controller.signal.aborted) return;

      // Ask the server to inject a preview subgraph.
      // Pass the abort signal so the HTTP request is cancelled immediately
      // when the user switches sessions, avoiding a server-side subgraph
      // that we'd have to tear down after the fact.
      const result = await startPreview(selectedSessionId, undefined, undefined, controller.signal);

      // Check for cancellation after the API await — if the session
      // changed while we were waiting, tear down the just-created preview.
      if (controller.signal.aborted) {
        stopPreview(selectedSessionId, result.preview_id).catch(() => {});
        return;
      }

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
      // Ignore abort errors — cleanup already happened
      if (controller.signal.aborted) return;

      const message = err instanceof Error ? err.message : 'Failed to start preview';
      setPreviewError(message);
      // Clean up partial state
      if (previewIdRef.current && previewSessionIdRef.current) {
        stopPreview(previewSessionIdRef.current, previewIdRef.current).catch(() => {});
        previewIdRef.current = null;
        previewSessionIdRef.current = null;
      }
    } finally {
      if (!controller.signal.aborted) {
        setIsPreviewLoading(false);
      }
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
    // Cancel any in-flight start request
    abortControllerRef.current?.abort();
    abortControllerRef.current = null;

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
    // Disconnect the MoQ watch subscription — always call disconnect;
    // it's safe when already disconnected and avoids stale-closure bugs.
    previewDisconnect();
    setPreviewError(null);
  }, [previewDisconnect]);

  return {
    isPreviewConnected,
    isPreviewLoading: isPreviewLoading || isMoqConnecting,
    previewError,
    handleStartPreview,
    handleStopPreview,
  };
}
