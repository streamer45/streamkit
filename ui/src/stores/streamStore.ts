// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import * as Hang from '@moq/hang';
import * as Publish from '@moq/publish';
import type { Signal } from '@moq/signals';
import { Effect } from '@moq/signals';
import * as Watch from '@moq/watch';
import { create } from 'zustand';

import { fetchConfig } from '../services/config';
import { getLogger } from '../utils/logger';

const logger = getLogger('streamStore');

export type ConnectionStatus = 'disconnected' | 'connecting' | 'connected';
export type ConnectionMode = 'session' | 'direct';
export type WatchStatus = 'disabled' | 'offline' | 'loading' | 'live';
export type MicStatus = 'disabled' | 'requesting' | 'ready' | 'error';
export type CameraStatus = 'disabled' | 'requesting' | 'ready' | 'error';

type ConnectDecision =
  | {
      ok: true;
      trimmedServerUrl: string;
      shouldWatch: boolean;
      shouldPublish: boolean;
    }
  | { ok: false; errorMessage: string };

type ConnectAttempt = {
  connection: Hang.Moq.Connection.Reload | null;
  healthEffect: Effect | null;
  watch: Watch.Broadcast | null;
  watchSync: Watch.Sync | null;
  audioSource: Watch.Audio.Source | null;
  audioDecoder: Watch.Audio.Decoder | null;
  audioEmitter: Watch.Audio.Emitter | null;
  videoSource: Watch.Video.Source | null;
  videoDecoder: Watch.Video.Decoder | null;
  videoRenderer: Watch.Video.Renderer | null;
  microphone: Publish.Source.Microphone | null;
  camera: Publish.Source.Camera | null;
  publish: Publish.Broadcast | null;
};

function waitForSignalValue<T>(
  signal: Signal<T>,
  predicate: (value: T) => boolean,
  timeoutMs: number,
  timeoutMessage: string
): Promise<T> {
  const initial = signal.peek();
  if (predicate(initial)) {
    return Promise.resolve(initial);
  }

  return new Promise((resolve, reject) => {
    let dispose: () => void = () => {};
    const timeoutId = setTimeout(() => {
      dispose();
      reject(new Error(timeoutMessage));
    }, timeoutMs);

    dispose = signal.subscribe((value) => {
      if (predicate(value)) {
        clearTimeout(timeoutId);
        dispose();
        resolve(value);
      }
    });
  });
}

function decideConnect(
  state: Pick<StreamState, 'connectionMode' | 'enablePublish' | 'enableWatch' | 'serverUrl'>
): ConnectDecision {
  const trimmedServerUrl = state.serverUrl.trim();
  if (!trimmedServerUrl) {
    return {
      ok: false,
      errorMessage: 'Missing MoQ Gateway URL. Configure it on the server or enter one above.',
    };
  }

  if (state.connectionMode === 'direct' && !state.enablePublish && !state.enableWatch) {
    return { ok: false, errorMessage: 'At least one of Publish or Watch must be enabled.' };
  }

  const shouldWatch = state.connectionMode === 'session' || state.enableWatch;
  const shouldPublish = state.enablePublish;

  return { ok: true, trimmedServerUrl, shouldWatch, shouldPublish };
}

function formatConnectError(error: unknown): string {
  return error instanceof Error
    ? `Connection failed: ${error.message}`
    : 'Failed to connect to MoQ server. Please check your connection and try again.';
}

function cleanupConnectAttempt(attempt: ConnectAttempt): void {
  attempt.healthEffect?.close();
  attempt.publish?.close();
  attempt.videoRenderer?.close();
  attempt.videoDecoder?.close();
  attempt.videoSource?.close();
  attempt.audioEmitter?.close();
  attempt.audioDecoder?.close();
  attempt.audioSource?.close();
  attempt.watchSync?.close();
  attempt.watch?.close();
  attempt.connection?.close();
  if (attempt.microphone) {
    if (typeof attempt.microphone.close === 'function') {
      attempt.microphone.close();
    } else if (attempt.microphone.enabled) {
      attempt.microphone.enabled.set(false);
    }
  }
  if (attempt.camera) {
    if (typeof attempt.camera.close === 'function') {
      attempt.camera.close();
    } else if (attempt.camera.enabled) {
      attempt.camera.enabled.set(false);
    }
  }
}

function setupConnectionStatusSync(
  healthEffect: Effect,
  connection: Hang.Moq.Connection.Reload,
  get: () => StreamState,
  set: (partial: Partial<StreamState>) => void
): void {
  let hadConnected = false;
  healthEffect.subscribe(connection.status, (value) => {
    const current = get().status;
    const mapped: ConnectionStatus =
      value === 'connected' ? 'connected' : value === 'connecting' ? 'connecting' : 'disconnected';

    if (value === 'connected') {
      hadConnected = true;
    }

    // Avoid immediately overriding our optimistic "connecting" state with the initial
    // connection status, which starts as "disconnected" before the internal effect runs.
    if (!hadConnected && current === 'connecting' && mapped === 'disconnected') {
      return;
    }

    set({ status: mapped });
    if (mapped === 'disconnected' && current === 'connected') {
      set({
        errorMessage:
          'Disconnected from MoQ gateway. Check the URL, relay availability, and your network.',
      });
    }
  });
}

function setupWatchPath(
  healthEffect: Effect,
  connection: Hang.Moq.Connection.Reload,
  outputBroadcast: string,
  set: (partial: Partial<StreamState>) => void
): {
  watch: Watch.Broadcast;
  watchSync: Watch.Sync;
  audioSource: Watch.Audio.Source;
  audioDecoder: Watch.Audio.Decoder;
  audioEmitter: Watch.Audio.Emitter;
  videoSource: Watch.Video.Source;
  videoDecoder: Watch.Video.Decoder;
  videoRenderer: Watch.Video.Renderer;
} {
  logger.info('Step 2: Creating watch broadcast (subscribe FIRST)');
  const watch = new Watch.Broadcast({
    connection: connection.established,
    enabled: true,
    name: Watch.Lite.Path.from(outputBroadcast),
  });

  const watchSync = new Watch.Sync();

  logger.info('Step 3: Creating audio source/decoder/emitter');
  const audioSource = new Watch.Audio.Source(watchSync, { broadcast: watch });
  const audioDecoder = new Watch.Audio.Decoder(audioSource);
  const audioEmitter = new Watch.Audio.Emitter(audioDecoder, {
    muted: false,
    volume: 0.5,
  });

  logger.info('Step 3b: Creating video source/decoder/renderer');
  const videoSource = new Watch.Video.Source(watchSync, { broadcast: watch });
  const videoDecoder = new Watch.Video.Decoder(videoSource);
  const videoRenderer = new Watch.Video.Renderer(videoDecoder);

  set({ watchStatus: watch.status.peek() });
  healthEffect.subscribe(watch.status, (value) => {
    set({ watchStatus: value });
  });

  return {
    watch,
    watchSync,
    audioSource,
    audioDecoder,
    audioEmitter,
    videoSource,
    videoDecoder,
    videoRenderer,
  };
}

function setupPublishPath(
  healthEffect: Effect,
  connection: Hang.Moq.Connection.Reload,
  inputBroadcast: string,
  set: (partial: Partial<StreamState>) => void
): {
  microphone: Publish.Source.Microphone;
  camera: Publish.Source.Camera;
  publish: Publish.Broadcast;
} {
  logger.info('Step 4: Creating microphone source');
  const microphone = new Publish.Source.Microphone({ enabled: true });

  set({ micStatus: microphone.source.peek() ? 'ready' : 'requesting' });
  healthEffect.subscribe(microphone.source, (value) => {
    set({ micStatus: value ? 'ready' : 'requesting' });
  });

  logger.info('Step 4b: Creating camera source');
  const camera = new Publish.Source.Camera({ enabled: true });

  set({ cameraStatus: camera.source.peek() ? 'ready' : 'requesting' });
  healthEffect.subscribe(camera.source, (value) => {
    set({ cameraStatus: value ? 'ready' : 'requesting' });
  });

  logger.info('Step 5: Creating publish broadcast');
  const publish = new Publish.Broadcast({
    connection: connection.established,
    enabled: true,
    name: Publish.Lite.Path.from(inputBroadcast),
    audio: {
      enabled: true,
      source: microphone.source,
    },
    video: {
      source: camera.source,
      hd: { enabled: true, config: { codec: 'vp09' } },
    },
  });

  return { microphone, camera, publish };
}

function schedulePostConnectWarnings(
  decision: Extract<ConnectDecision, { ok: true }>,
  attempt: ConnectAttempt,
  get: () => StreamState,
  set: (partial: Partial<StreamState>) => void
): void {
  if (!attempt.healthEffect) return;

  if (decision.shouldWatch && attempt.watch) {
    const watchRef = attempt.watch;
    // Use setTimeout instead of healthEffect.timeout() which fires immediately
    setTimeout(() => {
      if (get().status !== 'connected') return;
      if (watchRef.status.peek() !== 'live') {
        set({
          errorMessage: `Connected to relay, but output broadcast "${get().outputBroadcast}" is not live yet.`,
        });
      }
    }, 10_000);
  }

  if (decision.shouldPublish && attempt.microphone) {
    const microphoneRef = attempt.microphone;

    // Track if the microphone source was EVER acquired during the 10-second window.
    // This prevents false errors when the source signal transiently goes falsy.
    let wasEverReady = Boolean(microphoneRef.source.peek()) || get().micStatus === 'ready';
    attempt.healthEffect.subscribe(microphoneRef.source, (value) => {
      if (value) wasEverReady = true;
    });

    // Use setTimeout instead of healthEffect.timeout() which fires immediately
    setTimeout(() => {
      if (get().status !== 'connected') return;
      if (wasEverReady) return;
      set({
        micStatus: 'error',
        errorMessage:
          'Connected to relay, but microphone is not available. Check browser permissions and selected input device.',
      });
    }, 10_000);
  }

  if (decision.shouldPublish && attempt.camera) {
    const cameraRef = attempt.camera;

    let wasEverReady = Boolean(cameraRef.source.peek()) || get().cameraStatus === 'ready';
    attempt.healthEffect.subscribe(cameraRef.source, (value) => {
      if (value) wasEverReady = true;
    });

    setTimeout(() => {
      if (get().status !== 'connected') return;
      if (wasEverReady) return;
      set({
        cameraStatus: 'error',
        errorMessage:
          'Connected to relay, but camera is not available. Check browser permissions and selected input device.',
      });
    }, 10_000);
  }
}

interface StreamState {
  // Connection state
  status: ConnectionStatus;
  connectionMode: ConnectionMode;
  serverUrl: string;
  moqToken: string;
  inputBroadcast: string;
  outputBroadcast: string;

  // Direct mode options
  enablePublish: boolean;
  enableWatch: boolean;

  // Media state
  isMicEnabled: boolean;
  micStatus: MicStatus;
  isCameraEnabled: boolean;
  cameraStatus: CameraStatus;
  watchStatus: WatchStatus;

  // Error state
  errorMessage: string;

  // Config state
  configLoaded: boolean;

  // Active session state (persisted)
  activeSessionId: string | null;
  activeSessionName: string | null;
  activePipelineName: string | null;

  // MoQ references (stored but not serialized)
  publish: Publish.Broadcast | null;
  watch: Watch.Broadcast | null;
  watchSync: Watch.Sync | null;
  audioSource: Watch.Audio.Source | null;
  audioDecoder: Watch.Audio.Decoder | null;
  audioEmitter: Watch.Audio.Emitter | null;
  videoSource: Watch.Video.Source | null;
  videoDecoder: Watch.Video.Decoder | null;
  videoRenderer: Watch.Video.Renderer | null;
  connection: Hang.Moq.Connection.Reload | null;
  microphone: Publish.Source.Microphone | null;
  camera: Publish.Source.Camera | null;
  healthEffect: Effect | null;

  // Actions
  setServerUrl: (url: string) => void;
  setMoqToken: (token: string) => void;
  setInputBroadcast: (broadcast: string) => void;
  setOutputBroadcast: (broadcast: string) => void;
  setStatus: (status: ConnectionStatus) => void;
  setErrorMessage: (message: string) => void;
  setIsMicEnabled: (enabled: boolean) => void;
  setConnectionMode: (mode: ConnectionMode) => void;
  setEnablePublish: (enabled: boolean) => void;
  setEnableWatch: (enabled: boolean) => void;
  loadConfig: () => Promise<void>;

  // Session actions
  setActiveSession: (sessionId: string, sessionName: string | null, pipelineName: string) => void;
  clearActiveSession: () => void;

  connect: () => Promise<boolean>;
  disconnect: () => void;
  toggleMicrophone: () => void;
  toggleCamera: () => void;

  // Store references to MoQ objects
  setMoqRefs: (refs: {
    publish: Publish.Broadcast;
    watch: Watch.Broadcast;
    watchSync: Watch.Sync;
    audioSource: Watch.Audio.Source;
    audioDecoder: Watch.Audio.Decoder;
    audioEmitter: Watch.Audio.Emitter;
    videoSource: Watch.Video.Source;
    videoDecoder: Watch.Video.Decoder;
    videoRenderer: Watch.Video.Renderer;
    connection: Hang.Moq.Connection.Reload;
    microphone: Publish.Source.Microphone;
    camera: Publish.Source.Camera;
  }) => void;
}

export const useStreamStore = create<StreamState>((set, get) => ({
  // Initial state
  status: 'disconnected',
  connectionMode: 'session',
  serverUrl: '',
  moqToken: '',
  inputBroadcast: 'input',
  outputBroadcast: 'output',
  enablePublish: true,
  enableWatch: true,
  isMicEnabled: false,
  micStatus: 'disabled',
  isCameraEnabled: false,
  cameraStatus: 'disabled',
  watchStatus: 'disabled',
  errorMessage: '',
  configLoaded: false,

  // Active session state
  activeSessionId: null,
  activeSessionName: null,
  activePipelineName: null,

  // MoQ references
  publish: null,
  watch: null,
  watchSync: null,
  audioSource: null,
  audioDecoder: null,
  audioEmitter: null,
  videoSource: null,
  videoDecoder: null,
  videoRenderer: null,
  connection: null,
  microphone: null,
  camera: null,
  healthEffect: null,

  // Simple setters
  setServerUrl: (url) => set({ serverUrl: url }),
  setMoqToken: (token) => set({ moqToken: token }),
  setInputBroadcast: (broadcast) => set({ inputBroadcast: broadcast }),
  setOutputBroadcast: (broadcast) => set({ outputBroadcast: broadcast }),
  setStatus: (status) => set({ status }),
  setErrorMessage: (message) => set({ errorMessage: message }),
  setIsMicEnabled: (enabled) => set({ isMicEnabled: enabled }),
  setConnectionMode: (mode) => set({ connectionMode: mode }),
  setEnablePublish: (enabled) => set({ enablePublish: enabled }),
  setEnableWatch: (enabled) => set({ enableWatch: enabled }),

  // Session setters
  setActiveSession: (sessionId, sessionName, pipelineName) =>
    set({
      activeSessionId: sessionId,
      activeSessionName: sessionName,
      activePipelineName: pipelineName,
    }),
  clearActiveSession: () =>
    set({ activeSessionId: null, activeSessionName: null, activePipelineName: null }),

  loadConfig: async () => {
    try {
      const config = await fetchConfig();
      if (config.moqGatewayUrl) {
        set({ serverUrl: config.moqGatewayUrl, configLoaded: true });
      } else {
        set({
          configLoaded: true,
          errorMessage:
            'Streaming is not configured: server did not provide moqGatewayUrl in /api/v1/config.',
        });
      }
    } catch (error) {
      logger.error('Failed to load config:', error);
      set({
        configLoaded: true,
        errorMessage:
          'Failed to load server config from /api/v1/config. Enter a MoQ Gateway URL manually.',
      });
    }
  },

  setMoqRefs: (refs) =>
    set({
      publish: refs.publish,
      watch: refs.watch,
      watchSync: refs.watchSync,
      audioSource: refs.audioSource,
      audioDecoder: refs.audioDecoder,
      audioEmitter: refs.audioEmitter,
      videoSource: refs.videoSource,
      videoDecoder: refs.videoDecoder,
      videoRenderer: refs.videoRenderer,
      connection: refs.connection,
      microphone: refs.microphone,
      camera: refs.camera,
    }),

  connect: async () => {
    const state = get();

    if (state.status !== 'disconnected') {
      return state.status === 'connected';
    }

    const decision = decideConnect(state);
    if (!decision.ok) {
      set({ status: 'disconnected', errorMessage: decision.errorMessage });
      return false;
    }

    set({
      status: 'connecting',
      errorMessage: '',
      watchStatus: decision.shouldWatch ? 'loading' : 'disabled',
      micStatus: decision.shouldPublish ? 'requesting' : 'disabled',
      cameraStatus: decision.shouldPublish ? 'requesting' : 'disabled',
    });

    const attempt: ConnectAttempt = {
      connection: null,
      healthEffect: null,
      watch: null,
      watchSync: null,
      audioSource: null,
      audioDecoder: null,
      audioEmitter: null,
      videoSource: null,
      videoDecoder: null,
      videoRenderer: null,
      microphone: null,
      camera: null,
      publish: null,
    };

    try {
      logger.info('Step 1: Creating connection to relay server');
      const url = new URL(decision.trimmedServerUrl);
      const jwt = get().moqToken.trim();
      if (jwt) {
        url.searchParams.set('jwt', jwt);
      }
      // Create connection to relay server with auto-reconnect
      // Hang will automatically fetch certificate fingerprints from http://host:port/certificate.sha256
      attempt.connection = new Hang.Moq.Connection.Reload({
        url,
        enabled: true,
      });

      attempt.healthEffect = new Effect();
      setupConnectionStatusSync(attempt.healthEffect, attempt.connection, get, set);

      if (decision.shouldWatch) {
        const watchSetup = setupWatchPath(
          attempt.healthEffect,
          attempt.connection,
          state.outputBroadcast,
          set
        );
        attempt.watch = watchSetup.watch;
        attempt.watchSync = watchSetup.watchSync;
        attempt.audioSource = watchSetup.audioSource;
        attempt.audioDecoder = watchSetup.audioDecoder;
        attempt.audioEmitter = watchSetup.audioEmitter;
        attempt.videoSource = watchSetup.videoSource;
        attempt.videoDecoder = watchSetup.videoDecoder;
        attempt.videoRenderer = watchSetup.videoRenderer;
      }

      if (decision.shouldPublish) {
        const publishSetup = setupPublishPath(
          attempt.healthEffect,
          attempt.connection,
          state.inputBroadcast,
          set
        );
        attempt.microphone = publishSetup.microphone;
        attempt.camera = publishSetup.camera;
        attempt.publish = publishSetup.publish;
      }

      // Wait for the relay connection to actually establish before reporting success.
      await waitForSignalValue(
        attempt.connection.established,
        (value) => value !== undefined,
        12_000,
        'Timed out connecting to MoQ gateway.'
      );

      // After connection is established, warn if watch/publish don't become usable quickly.
      schedulePostConnectWarnings(decision, attempt, get, set);

      // Store all references
      set({
        publish: attempt.publish,
        watch: attempt.watch,
        watchSync: attempt.watchSync,
        audioSource: attempt.audioSource,
        audioDecoder: attempt.audioDecoder,
        audioEmitter: attempt.audioEmitter,
        videoSource: attempt.videoSource,
        videoDecoder: attempt.videoDecoder,
        videoRenderer: attempt.videoRenderer,
        connection: attempt.connection,
        microphone: attempt.microphone,
        camera: attempt.camera,
        healthEffect: attempt.healthEffect,
        status: 'connected',
        isMicEnabled: decision.shouldPublish,
        isCameraEnabled: decision.shouldPublish,
      });

      const modes = [];
      if (decision.shouldWatch) modes.push('watching');
      if (decision.shouldPublish) modes.push('publishing');
      logger.info(`Connection established: ${modes.join(' and ')}`);
      return true;
    } catch (error) {
      logger.error('Connection failed:', error);
      cleanupConnectAttempt(attempt);

      set({
        status: 'disconnected',
        watchStatus: 'disabled',
        micStatus: 'disabled',
        cameraStatus: 'disabled',
        errorMessage: formatConnectError(error),
        publish: null,
        watch: null,
        watchSync: null,
        audioSource: null,
        audioDecoder: null,
        audioEmitter: null,
        videoSource: null,
        videoDecoder: null,
        videoRenderer: null,
        connection: null,
        microphone: null,
        camera: null,
        healthEffect: null,
      });
      return false;
    }
  },

  disconnect: () => {
    const state = get();

    if (state.healthEffect) {
      state.healthEffect.close();
    }

    // Clean up all MoQ resources
    if (state.publish) {
      state.publish.close();
    }
    if (state.videoRenderer) {
      state.videoRenderer.close();
    }
    if (state.videoDecoder) {
      state.videoDecoder.close();
    }
    if (state.videoSource) {
      state.videoSource.close();
    }
    if (state.audioEmitter) {
      state.audioEmitter.close();
    }
    if (state.audioDecoder) {
      state.audioDecoder.close();
    }
    if (state.audioSource) {
      state.audioSource.close();
    }
    if (state.watchSync) {
      state.watchSync.close();
    }
    if (state.watch) {
      state.watch.close();
    }
    if (state.connection) {
      state.connection.close();
    }

    // Clean up microphone/media resources
    if (state.microphone) {
      // The microphone source manages the MediaStream internally
      // Disable it and let it clean up
      if (typeof state.microphone.close === 'function') {
        state.microphone.close();
      } else if (state.microphone.enabled) {
        // If no close method, at least disable it
        state.microphone.enabled.set(false);
      }
    }

    // Clean up camera/media resources
    if (state.camera) {
      if (typeof state.camera.close === 'function') {
        state.camera.close();
      } else if (state.camera.enabled) {
        state.camera.enabled.set(false);
      }
    }

    set({
      status: 'disconnected',
      isMicEnabled: false,
      micStatus: 'disabled',
      isCameraEnabled: false,
      cameraStatus: 'disabled',
      watchStatus: 'disabled',
      errorMessage: '',
      publish: null,
      watch: null,
      watchSync: null,
      audioSource: null,
      audioDecoder: null,
      audioEmitter: null,
      videoSource: null,
      videoDecoder: null,
      videoRenderer: null,
      connection: null,
      microphone: null,
      camera: null,
      healthEffect: null,
    });
  },

  toggleMicrophone: () => {
    const state = get();

    if (state.publish) {
      const newState = !state.isMicEnabled;
      state.publish.audio.enabled.set(newState);
      set({ isMicEnabled: newState });
    }
  },

  toggleCamera: () => {
    const state = get();

    if (state.camera) {
      const newState = !state.isCameraEnabled;
      state.camera.enabled.set(newState);
      set({ isCameraEnabled: newState });
    }
  },
}));
