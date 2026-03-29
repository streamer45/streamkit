// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import { Volume2, VolumeX } from 'lucide-react';
import React, { useEffect, useCallback, useRef, useState } from 'react';
import { useNavigate, useLocation } from 'react-router-dom';
import { useShallow } from 'zustand/shallow';

import ConfirmModal from '@/components/ConfirmModal';
import { MSEPlayer } from '@/components/MSEPlayer';
import { VolumeSlider } from '@/components/OutputPreviewPanel';
import { PipelineSelectionSection } from '@/components/stream/PipelineSelectionSection';
import { TelemetryTimeline as TelemetryTimelineComponent } from '@/components/TelemetryTimeline';
import {
  ViewContainer,
  ContentArea,
  ContentWrapper,
  BottomSpacer,
  Section,
  SectionTitle,
  InfoBox,
  InfoContent,
  InfoTitle,
  TechnicalDetailsToggle,
  TechnicalDetails,
} from '@/components/ui/ViewLayout';
import { useAudioControls } from '@/hooks/useAudioControls';
import { useStreamViewState } from '@/hooks/useStreamViewState';
import { useVideoCanvas } from '@/hooks/useVideoCanvas';
import { useWebSocket } from '@/hooks/useWebSocket';
import { getApiUrl } from '@/services/base';
import { listDynamicSamples } from '@/services/samples';
import { createSession } from '@/services/sessions';
import { useSchemaStore, ensureSchemasLoaded } from '@/stores/schemaStore';
import type { Event } from '@/types/types';
import { getLogger } from '@/utils/logger';
import {
  extractMoqPeerSettings,
  updateUrlPath,
  type MoqPeerSettings,
} from '@/utils/moqPeerSettings';
import { orderSamplePipelinesSystemFirst } from '@/utils/samplePipelineOrdering';

import type { CameraStatus } from '../stores/streamStore';
import { useStreamStore } from '../stores/streamStore';

const logger = getLogger('StreamView');

/**
 * Resolves the server URL for a pipeline's MoQ settings.
 *
 * - Relay pipelines use the relay URL directly.
 * - Gateway pipelines apply the gateway path to the original config URL
 *   (not the current serverUrl, which may have been overwritten by a
 *   previous relay selection).
 *
 * Returns the resolved URL or undefined if no update is needed.
 */
function resolveServerUrl(settings: MoqPeerSettings): string | undefined {
  if (settings.relayUrl) return settings.relayUrl;
  if (settings.gatewayPath) {
    const baseUrl = useStreamStore.getState().configServerUrl;
    if (baseUrl) return updateUrlPath(baseUrl, settings.gatewayPath);
  }
  return undefined;
}

const ConnectionControlsRow = styled.div`
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 16px;
  flex-wrap: wrap;
  min-width: 0;
`;

const ConnectionHint = styled.div`
  color: var(--sk-text-muted);
  font-size: 13px;
  min-width: 0;
  flex: 1 1 220px;

  @media (max-width: 900px) {
    flex-basis: 100%;
  }
`;

const InputGroup = styled.div`
  display: flex;
  flex-direction: column;
  gap: 8px;
`;

const Label = styled.label`
  font-size: 14px;
  font-weight: 600;
  color: var(--sk-text);
`;

const Input = styled.input`
  padding: 12px;
  font-size: 14px;
  background: var(--sk-bg);
  color: var(--sk-text);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  font-family: inherit;

  &:focus {
    outline: none;
    border-color: var(--sk-primary);
  }

  &::placeholder {
    color: var(--sk-text-muted);
  }
`;

const Button = styled.button<{ variant?: 'primary' | 'secondary'; disabled?: boolean }>`
  padding: 8px 16px;
  font-size: 14px;
  font-weight: 600;
  color: ${(props) => {
    if (props.disabled) return 'var(--sk-text-muted)';
    return props.variant === 'primary' ? 'var(--sk-primary-contrast)' : 'var(--sk-text)';
  }};
  background: ${(props) => {
    if (props.disabled) return 'var(--sk-hover-bg)';
    return props.variant === 'primary' ? 'var(--sk-primary)' : 'var(--sk-panel-bg)';
  }};
  border: 1px solid
    ${(props) => {
      if (props.disabled) return 'var(--sk-border)';
      return props.variant === 'primary' ? 'var(--sk-primary)' : 'var(--sk-border)';
    }};
  border-radius: 6px;
  cursor: ${(props) => (props.disabled ? 'not-allowed' : 'pointer')};
  transition: none;

  &:hover:not(:disabled) {
    background: ${(props) =>
      props.variant === 'primary' ? 'var(--sk-primary-hover)' : 'var(--sk-hover-bg)'};
    border-color: ${(props) =>
      props.variant === 'primary' ? 'var(--sk-primary-hover)' : 'var(--sk-border-strong)'};
  }
`;

const StatusIndicator = styled.div<{ status: 'disconnected' | 'connecting' | 'connected' }>`
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  font-size: 14px;
  font-weight: 600;
  color: var(--sk-text);
  flex-shrink: 0;

  &::before {
    content: '';
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: ${(props) => {
      switch (props.status) {
        case 'connected':
          return '#4caf50';
        case 'connecting':
          return '#ff9800';
        case 'disconnected':
          return '#f44336';
      }
    }};
  }
`;

const ControlButton = styled.button<{ active?: boolean }>`
  padding: 8px 16px;
  background: ${(props) => (props.active ? 'var(--sk-primary)' : 'var(--sk-panel-bg)')};
  color: ${(props) => (props.active ? 'var(--sk-primary-contrast)' : 'var(--sk-text)')};
  border: 1px solid ${(props) => (props.active ? 'var(--sk-primary)' : 'var(--sk-border)')};
  border-radius: 6px;
  cursor: pointer;
  font-size: 14px;
  transition: none;

  &:hover {
    background: ${(props) => (props.active ? 'var(--sk-primary-hover)' : 'var(--sk-hover-bg)')};
  }
`;

const VideoContainer = styled.div`
  position: relative;

  &:hover .sk-fullscreen-btn {
    opacity: 1;
  }

  /* Normal (non-fullscreen) canvas preview cap. */
  canvas {
    max-height: 480px;
  }

  &:fullscreen {
    display: flex;
    align-items: center;
    justify-content: center;
    background: #000;
  }

  &:fullscreen .sk-fullscreen-btn {
    opacity: 0.5;
  }

  &:fullscreen .sk-fullscreen-btn:hover {
    opacity: 1;
  }

  &:fullscreen canvas {
    max-width: 100vw;
    max-height: 100vh;
  }
`;

const FullscreenButton = styled.button`
  position: absolute;
  top: 8px;
  right: 8px;
  padding: 4px 8px;
  background: rgba(0, 0, 0, 0.6);
  color: #fff;
  border: 1px solid rgba(255, 255, 255, 0.3);
  border-radius: 4px;
  cursor: pointer;
  font-size: 12px;
  opacity: 0;
  transition: opacity 0.2s;
  z-index: 1;

  &:hover {
    background: rgba(0, 0, 0, 0.8);
  }
`;

const ErrorMessage = styled.div`
  padding: 12px 16px;
  background: rgba(244, 67, 54, 0.1);
  border: 1px solid rgba(244, 67, 54, 0.3);
  border-radius: 6px;
  color: #f44336;
  font-size: 14px;
`;

const ModeToggle = styled.div`
  display: flex;
  background: var(--sk-bg);
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  padding: 4px;
  gap: 4px;
`;

const ModeButton = styled.button<{ active: boolean }>`
  padding: 8px 16px;
  font-size: 14px;
  font-weight: 600;
  color: ${(props) => (props.active ? 'var(--sk-primary-contrast)' : 'var(--sk-text-muted)')};
  background: ${(props) => (props.active ? 'var(--sk-primary)' : 'transparent')};
  border: none;
  border-radius: 6px;
  cursor: pointer;
  transition: none;

  &:hover:not(:disabled) {
    background: ${(props) => (props.active ? 'var(--sk-primary)' : 'var(--sk-hover-bg)')};
    color: ${(props) => (props.active ? 'var(--sk-primary-contrast)' : 'var(--sk-text)')};
  }

  &:disabled {
    cursor: not-allowed;
    opacity: 0.6;
  }
`;

const Checkbox = styled.label`
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 14px;
  color: var(--sk-text);
  cursor: pointer;

  input {
    width: 16px;
    height: 16px;
    accent-color: var(--sk-primary);
    cursor: pointer;
  }

  &[data-disabled='true'] {
    opacity: 0.5;
    cursor: not-allowed;

    input {
      cursor: not-allowed;
    }
  }
`;

const DirectModeInfo = styled.div`
  padding: 16px;
  background: var(--sk-bg);
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  color: var(--sk-text-muted);
  font-size: 13px;
  line-height: 1.5;
`;

const StreamView: React.FC = () => {
  const [showTechnicalDetails, setShowTechnicalDetails] = React.useState<boolean>(false);
  const [destroyConfirmOpen, setDestroyConfirmOpen] = React.useState<boolean>(false);
  const [destroyConfirmLoading, setDestroyConfirmLoading] = React.useState<boolean>(false);
  const navigate = useNavigate();
  const location = useLocation();

  // State for pipeline selection and session creation
  const viewState = useStreamViewState();

  // WebSocket for session events
  const { onMessage, send: sendWs } = useWebSocket();

  // Use Zustand store for persistent state
  const {
    status,
    connectionMode,
    serverUrl,
    moqToken,
    inputBroadcast,
    outputBroadcast,
    enablePublish,
    enableWatch,
    isMicEnabled,
    micStatus,
    isCameraEnabled,
    cameraStatus,
    watchStatus,
    pipelineNeedsAudio,
    pipelineNeedsVideo,
    videoSourceType,
    connectingStep,
    errorMessage,
    configLoaded,
    activeSessionId,
    activeSessionName,
    activePipelineName,
    videoRenderer,
    audioEmitter,
    publishBroadcasts,
    msePath,
    isSecondaryCameraEnabled,
    secondaryCameraStatus,
    setServerUrl,
    setMoqToken,
    setInputBroadcast,
    setOutputBroadcast,
    setConnectionMode,
    setEnablePublish,
    setEnableWatch,
    setPipelineMediaTypes,
    setPipelineOutputTypes,
    setIsExternalRelay,
    setVideoSourceType,
    setTracks,
    setMsePath,
    setActiveSession,
    clearActiveSession,
    loadConfig,
    connect,
    disconnect,
    toggleMicrophone,
    toggleCamera,
    toggleSecondaryCamera,
  } = useStreamStore(
    useShallow((s) => ({
      status: s.status,
      connectionMode: s.connectionMode,
      serverUrl: s.serverUrl,
      moqToken: s.moqToken,
      inputBroadcast: s.inputBroadcast,
      outputBroadcast: s.outputBroadcast,
      enablePublish: s.enablePublish,
      enableWatch: s.enableWatch,
      isMicEnabled: s.isMicEnabled,
      micStatus: s.micStatus,
      isCameraEnabled: s.isCameraEnabled,
      cameraStatus: s.cameraStatus,
      watchStatus: s.watchStatus,
      pipelineNeedsAudio: s.pipelineNeedsAudio,
      pipelineNeedsVideo: s.pipelineNeedsVideo,
      videoSourceType: s.videoSourceType,
      connectingStep: s.connectingStep,
      errorMessage: s.errorMessage,
      configLoaded: s.configLoaded,
      activeSessionId: s.activeSessionId,
      activeSessionName: s.activeSessionName,
      activePipelineName: s.activePipelineName,
      videoRenderer: s.videoRenderer,
      audioEmitter: s.audioEmitter,
      publishBroadcasts: s.publishBroadcasts,
      msePath: s.msePath,
      isSecondaryCameraEnabled: s.isSecondaryCameraEnabled,
      secondaryCameraStatus: s.secondaryCameraStatus,
      setServerUrl: s.setServerUrl,
      setMoqToken: s.setMoqToken,
      setInputBroadcast: s.setInputBroadcast,
      setOutputBroadcast: s.setOutputBroadcast,
      setConnectionMode: s.setConnectionMode,
      setEnablePublish: s.setEnablePublish,
      setEnableWatch: s.setEnableWatch,
      setPipelineMediaTypes: s.setPipelineMediaTypes,
      setPipelineOutputTypes: s.setPipelineOutputTypes,
      setIsExternalRelay: s.setIsExternalRelay,
      setVideoSourceType: s.setVideoSourceType,
      setTracks: s.setTracks,
      setMsePath: s.setMsePath,
      setActiveSession: s.setActiveSession,
      clearActiveSession: s.clearActiveSession,
      loadConfig: s.loadConfig,
      connect: s.connect,
      disconnect: s.disconnect,
      toggleMicrophone: s.toggleMicrophone,
      toggleCamera: s.toggleCamera,
      toggleSecondaryCamera: s.toggleSecondaryCamera,
    }))
  );

  const isStreaming = status === 'connected';

  const videoContainerRef = useRef<HTMLDivElement>(null);
  const [isFullscreen, setIsFullscreen] = useState(false);

  // Track fullscreen state so the button label can reflect it.
  useEffect(() => {
    const handler = () => setIsFullscreen(Boolean(document.fullscreenElement));
    document.addEventListener('fullscreenchange', handler);
    document.addEventListener('webkitfullscreenchange', handler);
    return () => {
      document.removeEventListener('fullscreenchange', handler);
      document.removeEventListener('webkitfullscreenchange', handler);
    };
  }, []);

  // ── MSE playback state ──
  // When `msePath` is set and a session is active, fetch from the MSE
  // endpoint and pass the response stream + content type to MSEPlayer.
  const [mseStream, setMseStream] = useState<ReadableStream<Uint8Array> | null>(null);
  const [mseContentType, setMseContentType] = useState<string>('video/webm');
  const [mseError, setMseError] = useState<string | null>(null);
  const mseAbortRef = useRef<AbortController | null>(null);

  useEffect(() => {
    if (!activeSessionId || !msePath) {
      setMseStream(null);
      setMseError(null);
      return;
    }

    const abort = new AbortController();
    mseAbortRef.current = abort;

    const startMseFetch = async () => {
      try {
        const apiUrl = getApiUrl();
        const url = `${apiUrl}/mse/${activeSessionId}${msePath}`;
        logger.info(`Starting MSE fetch: ${url}`);

        const response = await fetch(url, {
          signal: abort.signal,
          credentials: 'include',
        });

        if (!response.ok) {
          throw new Error(`MSE endpoint returned ${response.status}: ${response.statusText}`);
        }

        const ct = response.headers.get('content-type') || 'video/webm';
        setMseContentType(ct);
        setMseStream(response.body);
        setMseError(null);
      } catch (err) {
        if (abort.signal.aborted) return;
        const msg = err instanceof Error ? err.message : 'Failed to connect to MSE stream';
        logger.error('MSE fetch failed:', msg);
        setMseError(msg);
      }
    };

    // Small delay to let the pipeline start and register the MSE endpoint.
    const timer = setTimeout(() => void startMseFetch(), 1500);

    return () => {
      clearTimeout(timer);
      abort.abort();
      mseAbortRef.current = null;
      setMseStream(null);
      setMseError(null);
    };
  }, [activeSessionId, msePath]);

  // Get node definitions for YAML autocomplete
  const nodeDefinitions = useSchemaStore((s) => s.nodeDefinitions);

  const { canvasRef: videoCanvasRef, aspectRatio: canvasAspectRatio } =
    useVideoCanvas(videoRenderer);

  const { muted, volume, toggleMute, changeVolume } = useAudioControls(audioEmitter);

  // Ensure schemas are loaded for autocomplete
  useEffect(() => {
    ensureSchemasLoaded();
  }, []);

  // Load server config on mount
  useEffect(() => {
    if (!configLoaded) {
      loadConfig();
    }
  }, [configLoaded, loadConfig]);

  // Validate active session still exists when navigating to this view
  useEffect(() => {
    const validateSession = async () => {
      if (activeSessionId) {
        try {
          const { listSessions } = await import('@/services/sessions');
          const sessions = await listSessions();
          const sessionExists = sessions.some((s) => s.id === activeSessionId);

          if (!sessionExists) {
            // Session was deleted while we were away, clear it
            if (status === 'connected' || status === 'connecting') {
              disconnect();
            }
            clearActiveSession();
          }
        } catch (error) {
          logger.error('Failed to validate session:', error);
        }
      }
    };

    // Validate on every navigation to this view
    validateSession();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [location.pathname]); // Trigger when route changes to /stream

  // Listen for session destroyed events to sync with Monitor view (when view is active)
  useEffect(() => {
    const unsubscribe = onMessage((message) => {
      if (message.type === 'event') {
        const event = message as Event;
        if (event.payload.event === 'sessiondestroyed') {
          // If the destroyed session matches our active session, clear it
          if (activeSessionId === event.payload.session_id) {
            // If currently streaming, disconnect first
            if (status === 'connected' || status === 'connecting') {
              disconnect();
            }

            clearActiveSession();
          }
        }
      }
    });

    return unsubscribe;
  }, [onMessage, activeSessionId, status, clearActiveSession, disconnect]);

  // Load dynamic pipeline samples
  useEffect(() => {
    const loadSamples = async () => {
      try {
        viewState.setSamplesLoading(true);
        viewState.setSamplesError(null);
        const samples = await listDynamicSamples();
        const orderedSamples = orderSamplePipelinesSystemFirst(samples);
        viewState.setSamples(orderedSamples);

        // Auto-select first template if available and apply its MoQ
        // settings so the stream store (pipelineNeedsVideo, etc.) matches
        // the selected template.  Without this, clicking an already-selected
        // radio item won't fire onValueChange, leaving the store defaults.
        if (orderedSamples.length > 0 && !viewState.selectedTemplateId) {
          const first = orderedSamples[0];
          viewState.setSelectedTemplateId(first.id);
          viewState.setPipelineYaml(first.yaml);

          const moqSettings = extractMoqPeerSettings(first.yaml);
          if (moqSettings) {
            const resolvedUrl = resolveServerUrl(moqSettings);
            if (resolvedUrl) setServerUrl(resolvedUrl);
            if (moqSettings.inputBroadcast) {
              setInputBroadcast(moqSettings.inputBroadcast);
            }
            if (moqSettings.outputBroadcast) {
              setOutputBroadcast(moqSettings.outputBroadcast);
            }
            setEnablePublish(moqSettings.hasInputBroadcast);
            setPipelineMediaTypes(moqSettings.needsAudioInput, moqSettings.needsVideoInput);
            setPipelineOutputTypes(moqSettings.outputsAudio, moqSettings.outputsVideo);
            setIsExternalRelay(moqSettings.isExternalRelay);
            setVideoSourceType(moqSettings.videoSourceType);
            setTracks(moqSettings.tracks, moqSettings.publishBroadcasts);
            setMsePath(moqSettings.msePath ?? null);
          }
        }
      } catch (error) {
        logger.error('Failed to load dynamic samples:', error);
        viewState.setSamplesError(
          error instanceof Error ? error.message : 'Failed to load pipeline templates'
        );
      } finally {
        viewState.setSamplesLoading(false);
      }
    };

    loadSamples();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Handle template selection
  const handleTemplateSelect = useCallback(
    (templateId: string) => {
      const template = viewState.samples.find((s) => s.id === templateId);
      if (template) {
        viewState.setSelectedTemplateId(templateId);
        viewState.setPipelineYaml(template.yaml);

        // Auto-adjust connection settings based on moq_peer node in the pipeline
        const moqSettings = extractMoqPeerSettings(template.yaml);
        if (moqSettings) {
          const resolvedUrl = resolveServerUrl(moqSettings);
          if (resolvedUrl) setServerUrl(resolvedUrl);
          // Update broadcast names if specified
          if (moqSettings.inputBroadcast) {
            setInputBroadcast(moqSettings.inputBroadcast);
          }
          if (moqSettings.outputBroadcast) {
            setOutputBroadcast(moqSettings.outputBroadcast);
          }
          // Auto-toggle publish based on whether pipeline expects a publisher.
          // Receive-only pipelines (no input_broadcast) skip microphone access.
          setEnablePublish(moqSettings.hasInputBroadcast);

          // Tell the store which devices the pipeline actually needs so that
          // connect() only requests the relevant browser permissions.
          setPipelineMediaTypes(moqSettings.needsAudioInput, moqSettings.needsVideoInput);

          // Tell the store which media types the pipeline outputs to subscribers
          // so that connect() only creates the relevant watch-side components.
          setPipelineOutputTypes(moqSettings.outputsAudio, moqSettings.outputsVideo);

          // Flag whether this pipeline uses an external relay so that
          // performConnect can skip the broadcast-announcement wait in
          // gateway mode.
          setIsExternalRelay(moqSettings.isExternalRelay);

          // Set the video source type so the connect flow creates the right
          // capture source (camera vs screen).
          setVideoSourceType(moqSettings.videoSourceType);

          // Pass tracks and broadcast names for multi-broadcast support.
          setTracks(moqSettings.tracks, moqSettings.publishBroadcasts);

          // MSE output path — when set, StreamView renders an MSEPlayer
          // instead of (or alongside) the MoQ canvas.
          setMsePath(moqSettings.msePath ?? null);
        }
      }
    },
    [
      viewState,
      setServerUrl,
      setInputBroadcast,
      setOutputBroadcast,
      setEnablePublish,
      setPipelineMediaTypes,
      setPipelineOutputTypes,
      setIsExternalRelay,
      setVideoSourceType,
      setTracks,
      setMsePath,
    ]
  );

  // Handle session creation
  const handleCreateSession = useCallback(async () => {
    if (!viewState.pipelineYaml) {
      viewState.setSessionCreationError('Please select a pipeline template');
      return;
    }

    try {
      viewState.setSessionCreationStatus('creating');
      viewState.setSessionCreationError(null);

      logger.info('Creating session');
      const result = await createSession(viewState.sessionName || null, viewState.pipelineYaml);

      // Get the selected template name for display
      const template = viewState.samples.find((s) => s.id === viewState.selectedTemplateId);

      // Store in persistent Zustand store
      setActiveSession(
        result.session_id,
        result.name || 'Unnamed Session',
        template?.name || 'Unknown Pipeline'
      );

      viewState.setSessionCreationStatus('success');
      logger.info('Session created successfully');

      // Optionally try connecting after session creation, but don't block session creation.
      if (status === 'disconnected' && serverUrl.trim()) {
        void (async () => {
          try {
            const ok = await connect();
            if (!ok) {
              logger.warn('Auto-connect after session creation did not succeed');
            }
          } catch (error) {
            logger.error('MoQ connection attempt after session creation failed:', error);
            viewState.setSessionCreationError(
              error instanceof Error ? error.message : 'Connection failed after session creation'
            );
          }
        })();
      }
    } catch (error) {
      logger.error('Failed to create session:', error);
      viewState.setSessionCreationError(
        error instanceof Error ? error.message : 'Failed to create session'
      );
      viewState.setSessionCreationStatus('error');
    }
  }, [viewState, serverUrl, setActiveSession, connect, status]);

  const handleDestroySession = useCallback(() => {
    if (!activeSessionId) return;
    setDestroyConfirmOpen(true);
  }, [activeSessionId]);

  const confirmDestroySession = useCallback(async () => {
    if (!activeSessionId) return;

    try {
      setDestroyConfirmLoading(true);

      if (status === 'connected' || status === 'connecting') {
        disconnect();
      }

      await sendWs({
        type: 'request',
        payload: { action: 'destroysession', session_id: activeSessionId },
      });

      clearActiveSession();
      viewState.setSessionCreationStatus('idle');
      viewState.setSessionCreationError(null);
      setDestroyConfirmOpen(false);
    } catch (error) {
      logger.error('Failed to destroy session:', error);
      viewState.setSessionCreationError(
        error instanceof Error ? error.message : 'Failed to destroy session'
      );
    } finally {
      setDestroyConfirmLoading(false);
    }
  }, [activeSessionId, clearActiveSession, disconnect, sendWs, status, viewState]);

  // Determine if Connect button should be disabled
  // In session mode: require a session; in direct mode: just need URL and at least one stream direction
  const canConnect =
    connectionMode === 'session'
      ? activeSessionId !== null && configLoaded && serverUrl.trim().length > 0
      : configLoaded && serverUrl.trim().length > 0 && (enablePublish || enableWatch);

  // Handle navigation to Monitor view
  const handleViewInMonitor = useCallback(() => {
    if (activeSessionId) {
      navigate('/monitor', { state: { sessionId: activeSessionId } });
    }
  }, [navigate, activeSessionId]);

  const statusText = {
    disconnected: 'Disconnected',
    connecting: 'Connecting...',
    connected: 'Connected',
  };

  const micStatusText = {
    disabled: 'Mic: disabled',
    requesting: 'Mic: requesting permission…',
    ready: 'Mic: ready',
    error: 'Mic: error',
  };

  const cameraStatusText: Record<CameraStatus, string> =
    videoSourceType === 'screen'
      ? {
          disabled: 'Screen: disabled',
          requesting: 'Screen: requesting permission…',
          ready: 'Screen: ready',
          error: 'Screen: error',
        }
      : {
          disabled: 'Camera: disabled',
          requesting: 'Camera: requesting permission…',
          ready: 'Camera: ready',
          error: 'Camera: error',
        };

  const watchStatusText = {
    disabled: 'Watch: disabled',
    offline: 'Watch: offline',
    loading: 'Watch: loading…',
    live: 'Watch: live',
  };

  const connectingStepText: Record<string, string> = {
    devices: 'Requesting device access',
    relay: 'Connecting to relay',
    pipeline: 'Waiting for pipeline',
  };

  return (
    <ViewContainer data-testid="stream-view">
      <ConfirmModal
        isOpen={destroyConfirmOpen}
        title="Destroy session?"
        message={`Destroy "${activeSessionName || activeSessionId || 'this session'}"? This stops the running pipeline so you can create a new one.`}
        confirmLabel="Destroy Session"
        cancelLabel="Cancel"
        onConfirm={confirmDestroySession}
        onCancel={() => setDestroyConfirmOpen(false)}
        isLoading={destroyConfirmLoading}
      />
      <ContentArea>
        <ContentWrapper>
          <InfoBox>
            <InfoContent>
              <InfoTitle>Real-Time Streaming with Dynamic Pipelines</InfoTitle>
              <div>
                This view runs StreamKit dynamic pipelines as long-lived sessions for real-time
                media processing. Create a session from a template (or edit the YAML), then connect
                to start streaming.
              </div>
              <div>
                In this demo, your browser publishes microphone audio over MoQ (WebTransport) and
                subscribes to the processed output broadcast.
              </div>
            </InfoContent>

            <InfoContent>
              <div>
                <strong>Quick start:</strong> Select a pipeline template, optionally edit the YAML,
                create a session, then connect.
              </div>
            </InfoContent>

            <TechnicalDetailsToggle onClick={() => setShowTechnicalDetails(!showTechnicalDetails)}>
              {showTechnicalDetails ? '▼' : '▶'} Technical Details
            </TechnicalDetailsToggle>

            {showTechnicalDetails && (
              <TechnicalDetails>
                <div>
                  <strong>Architecture:</strong> A control plane manages the running graph while
                  nodes process media on the data plane, allowing changes without restarting the
                  session.
                </div>
                <div>
                  <strong>State and Stats:</strong> Nodes report lifecycle state (Initializing,
                  Ready, Running, Recovering, Degraded, Failed, Stopped) and live counters; the
                  Monitor view shows them in real time.
                </div>
                <div>
                  <strong>YAML Format:</strong> Dynamic pipelines use the explicit{' '}
                  <code>nodes:</code> format with <code>needs:</code> dependencies, giving you full
                  control over complex DAG topologies beyond simple linear chains.
                </div>
                <div>
                  <strong>This Demo:</strong> The pipeline typically subscribes to an{' '}
                  <code>input</code> broadcast via <code>transport::moq::subscriber</code>, then
                  publishes the processed audio to an <code>output</code> broadcast via{' '}
                  <code>transport::moq::publisher</code>.
                </div>
              </TechnicalDetails>
            )}
          </InfoBox>

          {errorMessage && <ErrorMessage>{errorMessage}</ErrorMessage>}

          <Section>
            <SectionTitle>Connection Mode</SectionTitle>
            <ModeToggle>
              <ModeButton
                active={connectionMode === 'session'}
                onClick={() => {
                  setConnectionMode('session');
                  // Re-apply the selected template's MoQ settings that
                  // were overridden by Direct Connect mode.
                  if (viewState.selectedTemplateId) {
                    handleTemplateSelect(viewState.selectedTemplateId);
                  }
                }}
                disabled={status !== 'disconnected'}
              >
                Session
              </ModeButton>
              <ModeButton
                active={connectionMode === 'direct'}
                onClick={() => {
                  setConnectionMode('direct');
                  // Direct mode has no pipeline YAML, so default to both media types
                  setPipelineMediaTypes(true, true);
                  setPipelineOutputTypes(true, true);
                  // Direct mode connects to a relay without a skit pipeline,
                  // so there is no external relay announcement to wait for.
                  setIsExternalRelay(false);
                  // Direct mode always uses camera (no pipeline to specify screen).
                  setVideoSourceType('camera');
                }}
                disabled={status !== 'disconnected'}
              >
                Direct Connect
              </ModeButton>
            </ModeToggle>
            {connectionMode === 'direct' && (
              <DirectModeInfo>
                <strong>Direct Connect</strong> allows you to connect to any MoQ broadcast without
                creating a StreamKit session. Use this to subscribe to external broadcasts, test
                relay connectivity, or publish audio to arbitrary endpoints.
              </DirectModeInfo>
            )}
          </Section>

          {connectionMode === 'session' && (
            <PipelineSelectionSection
              samples={viewState.samples}
              samplesLoading={viewState.samplesLoading}
              samplesError={viewState.samplesError}
              selectedTemplateId={viewState.selectedTemplateId}
              pipelineYaml={viewState.pipelineYaml}
              sessionName={viewState.sessionName}
              sessionCreationStatus={viewState.sessionCreationStatus}
              sessionCreationError={viewState.sessionCreationError}
              activeSessionId={activeSessionId}
              activeSessionName={activeSessionName}
              activePipelineName={activePipelineName}
              streamStatus={status}
              onTemplateSelect={handleTemplateSelect}
              onPipelineYamlChange={viewState.setPipelineYaml}
              onSessionNameChange={viewState.setSessionName}
              onCreateSession={handleCreateSession}
              onDisconnect={disconnect}
              onDestroySession={handleDestroySession}
              onViewInMonitor={handleViewInMonitor}
              nodeDefinitions={nodeDefinitions}
            />
          )}

          <Section>
            <SectionTitle>Connection & Controls</SectionTitle>
            <ConnectionControlsRow>
              <StatusIndicator status={status}>{statusText[status]}</StatusIndicator>
              {status === 'disconnected' ? (
                <Button variant="primary" onClick={connect} disabled={!canConnect}>
                  Connect & Stream
                </Button>
              ) : (
                <Button variant="secondary" onClick={disconnect}>
                  Disconnect
                </Button>
              )}
              {status === 'disconnected' && !canConnect && (
                <ConnectionHint>
                  {connectionMode === 'session'
                    ? '← Create a session first'
                    : '← Enable at least one stream direction'}
                </ConnectionHint>
              )}
              {isStreaming && enablePublish && (
                <>
                  {pipelineNeedsAudio && (
                    <ControlButton active={isMicEnabled} onClick={toggleMicrophone}>
                      {isMicEnabled ? '🎤 Microphone On' : '🔇 Microphone Off'}
                    </ControlButton>
                  )}
                  {pipelineNeedsVideo && (
                    <ControlButton active={isCameraEnabled} onClick={toggleCamera}>
                      {videoSourceType === 'screen'
                        ? isCameraEnabled
                          ? '🖥️ Screen Share On'
                          : '🖥️ Screen Share Off'
                        : isCameraEnabled
                          ? '📷 Camera On'
                          : '📷 Camera Off'}
                    </ControlButton>
                  )}
                  {publishBroadcasts.length > 1 && secondaryCameraStatus !== 'disabled' && (
                    <ControlButton
                      active={isSecondaryCameraEnabled}
                      onClick={toggleSecondaryCamera}
                    >
                      {isSecondaryCameraEnabled ? '📷 Camera 2 On' : '📷 Camera 2 Off'}
                    </ControlButton>
                  )}
                </>
              )}
            </ConnectionControlsRow>

            {connectionMode === 'direct' && status === 'disconnected' && (
              <div style={{ display: 'flex', gap: '24px', marginBottom: '8px' }}>
                <Checkbox data-disabled={status !== 'disconnected'}>
                  <input
                    type="checkbox"
                    checked={enableWatch}
                    onChange={(e) => setEnableWatch(e.target.checked)}
                    disabled={status !== 'disconnected'}
                  />
                  Subscribe (Watch)
                </Checkbox>
                <Checkbox data-disabled={status !== 'disconnected'}>
                  <input
                    type="checkbox"
                    checked={enablePublish}
                    onChange={(e) => setEnablePublish(e.target.checked)}
                    disabled={status !== 'disconnected'}
                  />
                  Publish (Mic)
                </Checkbox>
              </div>
            )}

            {isStreaming && (
              <div style={{ color: 'var(--sk-text-muted)', fontSize: '14px', padding: '8px 0' }}>
                {connectionMode === 'direct'
                  ? `Connected: ${[enableWatch && 'watching', enablePublish && 'publishing'].filter(Boolean).join(' and ')}`
                  : isMicEnabled
                    ? 'Your audio is being streamed and will be echoed back'
                    : 'Enable your microphone to start streaming'}
              </div>
            )}

            {(status === 'connecting' || status === 'connected') && (
              <div style={{ color: 'var(--sk-text-muted)', fontSize: '13px', padding: '4px 0' }}>
                {status === 'connected'
                  ? 'Relay: connected'
                  : connectingStep
                    ? 'Connecting — ' + (connectingStepText[connectingStep] ?? connectingStep)
                    : 'Connecting…'}{' '}
                • {watchStatusText[watchStatus]}
                {pipelineNeedsAudio && <> • {micStatusText[micStatus]}</>}
                {pipelineNeedsVideo && <> • {cameraStatusText[cameraStatus]}</>}
                {secondaryCameraStatus !== 'disabled' && (
                  <>
                    {' '}
                    • Camera 2:{' '}
                    {secondaryCameraStatus === 'ready'
                      ? 'ready'
                      : secondaryCameraStatus === 'requesting'
                        ? 'requesting…'
                        : secondaryCameraStatus}
                  </>
                )}
              </div>
            )}

            <InputGroup>
              <Label htmlFor="server-url">MoQ Gateway URL (WebTransport)</Label>
              <Input
                id="server-url"
                type="text"
                value={serverUrl}
                onChange={(e) => setServerUrl(e.target.value)}
                placeholder="http://127.0.0.1:4545/moq"
                disabled={status !== 'disconnected'}
              />
            </InputGroup>

            <InputGroup>
              <Label htmlFor="moq-token">MoQ Token (optional on localhost)</Label>
              <Input
                id="moq-token"
                type="password"
                value={moqToken}
                onChange={(e) => setMoqToken(e.target.value)}
                placeholder="Paste MoQ JWT (sent as ?jwt=...)"
                disabled={status !== 'disconnected'}
              />
            </InputGroup>

            {(connectionMode === 'session' || enablePublish) && (
              <InputGroup>
                <Label htmlFor="input-broadcast">
                  {connectionMode === 'direct'
                    ? 'Publish Broadcast'
                    : 'Input Broadcast (Client → Server)'}
                </Label>
                <Input
                  id="input-broadcast"
                  type="text"
                  value={inputBroadcast}
                  onChange={(e) => setInputBroadcast(e.target.value)}
                  placeholder="input"
                  disabled={status !== 'disconnected'}
                />
              </InputGroup>
            )}

            {(connectionMode === 'session' || enableWatch) && (
              <InputGroup>
                <Label htmlFor="output-broadcast">
                  {connectionMode === 'direct'
                    ? 'Watch Broadcast'
                    : 'Output Broadcast (Server → Client)'}
                </Label>
                <Input
                  id="output-broadcast"
                  type="text"
                  value={outputBroadcast}
                  onChange={(e) => setOutputBroadcast(e.target.value)}
                  placeholder="output"
                  disabled={status !== 'disconnected'}
                />
              </InputGroup>
            )}
          </Section>

          {isStreaming && videoRenderer && !msePath && (
            <Section>
              <SectionTitle>Video</SectionTitle>
              <VideoContainer ref={videoContainerRef}>
                <FullscreenButton
                  className="sk-fullscreen-btn"
                  onClick={() => {
                    const el = videoContainerRef.current;
                    if (!el) return;
                    if (document.fullscreenElement) {
                      document.exitFullscreen().catch(() => {});
                    } else {
                      // requestFullscreen can reject in background tabs or
                      // outside a user gesture; swallow to avoid unhandled
                      // promise rejection.
                      const rfs =
                        el.requestFullscreen ??
                        (el as unknown as { webkitRequestFullscreen?: () => Promise<void> })
                          .webkitRequestFullscreen;
                      rfs?.call(el).catch(() => {});
                    }
                  }}
                >
                  {isFullscreen ? 'Exit Fullscreen' : 'Fullscreen'}
                </FullscreenButton>
                <canvas
                  ref={videoCanvasRef}
                  style={{
                    display: 'block',
                    width: 'auto',
                    maxWidth: '100%',
                    margin: '0 auto',
                    borderRadius: 6,
                    background: '#000',
                    aspectRatio: canvasAspectRatio,
                  }}
                />
                {audioEmitter && (
                  <div
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: 8,
                      marginTop: 8,
                      padding: '4px 0',
                    }}
                  >
                    <ControlButton
                      onClick={toggleMute}
                      title={muted ? 'Unmute' : 'Mute'}
                      style={{ padding: '4px 8px', fontSize: 12 }}
                    >
                      {muted ? <VolumeX size={14} /> : <Volume2 size={14} />}
                    </ControlButton>
                    <VolumeSlider
                      type="range"
                      min={0}
                      max={1}
                      step={0.05}
                      value={muted ? 0 : volume}
                      onChange={(e) => changeVolume(Number(e.target.value))}
                      style={{ width: 120 }}
                      title={`Volume: ${Math.round((muted ? 0 : volume) * 100)}%`}
                    />
                  </div>
                )}
              </VideoContainer>
            </Section>
          )}

          {msePath && activeSessionId && (
            <Section>
              <SectionTitle>Video (MSE)</SectionTitle>
              {mseError && <ErrorMessage>{mseError}</ErrorMessage>}
              {mseStream && (
                <MSEPlayer
                  stream={mseStream}
                  contentType={mseContentType}
                  onError={(msg) => {
                    logger.error('MSE playback error:', msg);
                    setMseError(msg);
                  }}
                />
              )}
              {!mseStream && !mseError && (
                <div style={{ color: 'var(--sk-text-muted)', fontSize: '13px', padding: '12px 0' }}>
                  Connecting to MSE stream...
                </div>
              )}
            </Section>
          )}

          {connectionMode === 'session' && activeSessionId && (
            <Section>
              <SectionTitle>Telemetry</SectionTitle>
              <div style={{ height: 360 }}>
                <TelemetryTimelineComponent sessionId={activeSessionId} />
              </div>
              <div style={{ color: 'var(--sk-text-muted)', fontSize: '13px' }}>
                Tip: Use <strong>View in Monitor</strong> for full graph state/stats.
              </div>
            </Section>
          )}
          <BottomSpacer />
        </ContentWrapper>
      </ContentArea>
    </ViewContainer>
  );
};

export default StreamView;
