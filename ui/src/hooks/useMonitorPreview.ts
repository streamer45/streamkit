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

/**
 * Tear down the server-side preview and, if the preview owns the MoQ
 * connection, disconnect it.
 *
 * When the StreamView already holds an active connection and the preview
 * merely piggybacks on it (or was never started at all), calling
 * `disconnect()` would tear down the StreamView's inputs (mic, camera,
 * screen).  The `ownsConnectionRef` flag prevents that: only a preview
 * that transitioned the store from `disconnected` → `connected` is
 * allowed to disconnect on cleanup.
 */
/** @internal — exported for unit testing */
export async function cleanupPreview(
  previewIdRef: React.MutableRefObject<string | null>,
  previewSessionIdRef: React.MutableRefObject<string | null>,
  previewDisconnect: () => void,
  ownsConnectionRef: React.MutableRefObject<boolean>
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
  // Only disconnect the MoQ connection if the preview created it.
  // Disconnecting unconditionally would kill StreamView inputs (mic,
  // camera, screen) that are still publishing to the pipeline.
  if (ownsConnectionRef.current) {
    previewDisconnect();
    ownsConnectionRef.current = false;
  }
}

/** Clean up any existing server-side preview before creating a new one.
 *  Awaits the stop to avoid racing with startPreview — if the server
 *  processes start before stop, the old preview could count toward the
 *  limit and cause a spurious 409. */
async function teardownExistingPreview(
  previewIdRef: React.MutableRefObject<string | null>,
  previewSessionIdRef: React.MutableRefObject<string | null>
): Promise<void> {
  if (!previewIdRef.current || !previewSessionIdRef.current) return;
  try {
    await stopPreview(previewSessionIdRef.current, previewIdRef.current);
  } catch {
    // Best-effort cleanup — proceed even if the old preview is gone.
  }
  previewIdRef.current = null;
  previewSessionIdRef.current = null;
}

/** Apply startPreview API result to the stream store. */
function applyPreviewResult(
  result: {
    preview_id: string;
    gateway_path: string;
    broadcast: string;
    audio: boolean;
    video: boolean;
  },
  selectedSessionId: string,
  previewIdRef: React.MutableRefObject<string | null>,
  previewSessionIdRef: React.MutableRefObject<string | null>,
  setters: {
    setServerUrl: (url: string) => void;
    setOutputBroadcast: (bc: string) => void;
    setPipelineOutputTypes: (audio: boolean, video: boolean) => void;
  }
): void {
  previewIdRef.current = result.preview_id;
  previewSessionIdRef.current = selectedSessionId;

  const baseUrl = useStreamStore.getState().configServerUrl || useStreamStore.getState().serverUrl;
  if (baseUrl) {
    setters.setServerUrl(updateUrlPath(baseUrl, result.gateway_path));
  }
  setters.setOutputBroadcast(result.broadcast);
  setters.setPipelineOutputTypes(result.audio, result.video);
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

  // When the user explicitly stops a borrowed preview, the shared store
  // still reports 'connected' (the StreamView's connection stays alive).
  // This flag lets the hook report isPreviewConnected=false so the
  // MonitorView hides the panel and shows the "Preview" button again.
  const [previewDismissed, setPreviewDismissed] = useState(false);

  const isPreviewConnected = previewStatus === 'connected' && !previewDismissed;
  // MoQ connection establishment happens after the API call returns,
  // so there's an intermediate "connecting" state that should still
  // show a loading indicator to avoid a brief flicker back to the
  // "Preview" button.
  const isMoqConnecting = previewStatus === 'connecting' && !previewDismissed;

  // Track the active preview ID so we can tear it down on the server.
  const previewIdRef = useRef<string | null>(null);
  // Track the session the preview belongs to.
  const previewSessionIdRef = useRef<string | null>(null);

  // True when the preview itself transitioned the stream store from
  // 'disconnected' → 'connected'.  When false, the connection belongs
  // to the StreamView and must not be torn down by preview cleanup.
  const previewOwnsConnectionRef = useRef(false);

  // AbortController cancels an in-flight startPreview API call when the
  // user switches sessions before the call completes.
  const abortControllerRef = useRef<AbortController | null>(null);

  // Reset UI state inline during render when the selected session changes
  // (https://react.dev/learn/you-might-not-need-an-effect#adjusting-some-state-when-a-prop-changes)
  // so the new session never renders a frame with the old session's
  // error/loading/dismissed state.
  const [prevSessionIdForState, setPrevSessionIdForState] = useState(selectedSessionId);
  if (selectedSessionId !== prevSessionIdForState) {
    setPrevSessionIdForState(selectedSessionId);
    if (prevSessionIdForState) {
      setPreviewError(null);
      setIsPreviewLoading(false);
      setPreviewDismissed(false);
    }
  }

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
      void cleanupPreview(
        previewIdRef,
        previewSessionIdRef,
        previewDisconnect,
        previewOwnsConnectionRef
      );
    }

    // Cleanup on unmount — fire-and-forget for the same reason as above.
    // Only disconnect if the preview owns the connection; otherwise the
    // StreamView's inputs stay alive.
    return () => {
      abortControllerRef.current?.abort();
      abortControllerRef.current = null;
      void cleanupPreview(
        previewIdRef,
        previewSessionIdRef,
        previewDisconnect,
        previewOwnsConnectionRef
      );
    };
  }, [selectedSessionId, previewDisconnect]);

  const handleStartPreview = useCallback(async () => {
    if (!selectedSessionId) return;

    // Clean up any existing server-side preview before creating a new one.
    await teardownExistingPreview(previewIdRef, previewSessionIdRef);

    // Cancel any previous in-flight request
    abortControllerRef.current?.abort();
    const controller = new AbortController();
    abortControllerRef.current = controller;

    setIsPreviewLoading(true);
    setPreviewError(null);
    setPreviewDismissed(false);

    try {
      // Configure for watch-only mode
      previewSetEnablePublish(false);
      previewSetEnableWatch(true);
      if (!previewConfigLoaded) {
        await previewLoadConfig();
      }

      if (controller.signal.aborted) return;

      // Ask the server to inject a preview subgraph.
      const result = await startPreview(selectedSessionId, undefined, undefined, controller.signal);

      // Session changed while awaiting — tear down the just-created preview.
      if (controller.signal.aborted) {
        stopPreview(selectedSessionId, result.preview_id).catch(() => {});
        return;
      }

      applyPreviewResult(result, selectedSessionId, previewIdRef, previewSessionIdRef, {
        setServerUrl: previewSetServerUrl,
        setOutputBroadcast: previewSetOutputBroadcast,
        setPipelineOutputTypes: previewSetPipelineOutputTypes,
      });

      // Remember whether we are creating a fresh connection or piggybacking
      // on an existing StreamView connection.
      const wasDisconnected = useStreamStore.getState().status === 'disconnected';
      await previewConnect();
      previewOwnsConnectionRef.current =
        wasDisconnected && useStreamStore.getState().status === 'connected';
      if (!controller.signal.aborted) {
        setIsPreviewLoading(false);
      }
    } catch (err) {
      // Ignore abort errors — cleanup already happened
      if (controller.signal.aborted) return;

      const message = err instanceof Error ? err.message : 'Failed to start preview';
      setPreviewError(message);
      // Clean up partial state
      await teardownExistingPreview(previewIdRef, previewSessionIdRef);
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

    // Track whether a preview was actually running so we only mute audio
    // for genuine borrowed-preview teardowns, not defensive cleanup calls
    // (e.g. from handleDeleteSession when no preview was started).
    const hadActivePreview = !!(previewIdRef.current && previewSessionIdRef.current);

    // Tear down server-side preview
    if (hadActivePreview) {
      try {
        await stopPreview(previewSessionIdRef.current!, previewIdRef.current!);
      } catch {
        // Best-effort teardown; the server may have already cleaned up
      }
      previewIdRef.current = null;
      previewSessionIdRef.current = null;
    }
    // Only disconnect the MoQ connection if the preview created it.
    // Disconnecting unconditionally would kill StreamView inputs (mic,
    // camera, screen) that are still publishing to the pipeline.
    if (previewOwnsConnectionRef.current) {
      previewDisconnect();
      previewOwnsConnectionRef.current = false;
    } else if (hadActivePreview) {
      // For borrowed connections we keep the MoQ connection alive, but
      // must silence the audio emitter so the user doesn't hear audio
      // playing in the background after dismissing the preview.
      const emitter = useStreamStore.getState().audioEmitter;
      if (emitter) {
        emitter.muted.set(true);
      }
    }
    // For borrowed connections, mark the preview as dismissed so the
    // MonitorView hides the panel even though the store is still connected.
    setPreviewDismissed(true);
    setIsPreviewLoading(false);
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
