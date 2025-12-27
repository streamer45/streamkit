// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import * as Hang from '@moq/hang';
import type { Signal } from '@moq/signals';
import { Effect } from '@moq/signals';
import { create } from 'zustand';

import { fetchConfig } from '../services/config';
import { getLogger } from '../utils/logger';

const logger = getLogger('streamStore');

export type ConnectionStatus = 'disconnected' | 'connecting' | 'connected';
export type ConnectionMode = 'session' | 'direct';
export type WatchStatus = 'disabled' | 'offline' | 'loading' | 'live';
export type MicStatus = 'disabled' | 'requesting' | 'ready' | 'error';

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
  watch: Hang.Watch.Broadcast | null;
  audioEmitter: Hang.Watch.Audio.Emitter | null;
  microphone: Hang.Publish.Source.Microphone | null;
  publish: Hang.Publish.Broadcast | null;
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
  const shouldPublish = state.connectionMode === 'session' || state.enablePublish;

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
  attempt.audioEmitter?.close();
  attempt.watch?.close();
  attempt.connection?.close();
  if (attempt.microphone) {
    if (typeof attempt.microphone.close === 'function') {
      attempt.microphone.close();
    } else if (attempt.microphone.enabled) {
      attempt.microphone.enabled.set(false);
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
): { watch: Hang.Watch.Broadcast; audioEmitter: Hang.Watch.Audio.Emitter } {
  logger.info('Step 2: Creating watch broadcast (subscribe FIRST)');
  const watch = new Hang.Watch.Broadcast({
    connection: connection.established,
    enabled: true,
    path: Hang.Moq.Path.from(outputBroadcast),
    audio: {
      enabled: true,
      // latency: 250 as Hang.Time.Milli,
    },
  });

  logger.info('Step 3: Creating audio emitter');
  const audioEmitter = new Hang.Watch.Audio.Emitter(watch.audio, {
    muted: false,
    volume: 0.5,
  });

  set({ watchStatus: watch.status.peek() });
  healthEffect.subscribe(watch.status, (value) => {
    set({ watchStatus: value });
  });

  return { watch, audioEmitter };
}

function setupPublishPath(
  healthEffect: Effect,
  connection: Hang.Moq.Connection.Reload,
  inputBroadcast: string,
  set: (partial: Partial<StreamState>) => void
): { microphone: Hang.Publish.Source.Microphone; publish: Hang.Publish.Broadcast } {
  logger.info('Step 4: Creating microphone source');
  const microphone = new Hang.Publish.Source.Microphone({ enabled: true });

  set({ micStatus: microphone.source.peek() ? 'ready' : 'requesting' });
  healthEffect.subscribe(microphone.source, (value) => {
    set({ micStatus: value ? 'ready' : 'requesting' });
  });

  logger.info('Step 5: Creating publish broadcast');
  const publish = new Hang.Publish.Broadcast({
    connection: connection.established,
    enabled: true,
    path: Hang.Moq.Path.from(inputBroadcast),
    audio: {
      enabled: true,
      source: microphone.source,
    },
  });

  return { microphone, publish };
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
}

interface StreamState {
  // Connection state
  status: ConnectionStatus;
  connectionMode: ConnectionMode;
  serverUrl: string;
  inputBroadcast: string;
  outputBroadcast: string;

  // Direct mode options
  enablePublish: boolean;
  enableWatch: boolean;

  // Media state
  isMicEnabled: boolean;
  micStatus: MicStatus;
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
  publish: Hang.Publish.Broadcast | null;
  watch: Hang.Watch.Broadcast | null;
  audioEmitter: Hang.Watch.Audio.Emitter | null;
  connection: Hang.Moq.Connection.Reload | null;
  microphone: Hang.Publish.Source.Microphone | null;
  healthEffect: Effect | null;

  // Actions
  setServerUrl: (url: string) => void;
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

  // Store references to MoQ objects
  setMoqRefs: (refs: {
    publish: Hang.Publish.Broadcast;
    watch: Hang.Watch.Broadcast;
    audioEmitter: Hang.Watch.Audio.Emitter;
    connection: Hang.Moq.Connection.Reload;
    microphone: Hang.Publish.Source.Microphone;
  }) => void;
}

export const useStreamStore = create<StreamState>((set, get) => ({
  // Initial state
  status: 'disconnected',
  connectionMode: 'session',
  serverUrl: '',
  inputBroadcast: 'input',
  outputBroadcast: 'output',
  enablePublish: true,
  enableWatch: true,
  isMicEnabled: false,
  micStatus: 'disabled',
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
  audioEmitter: null,
  connection: null,
  microphone: null,
  healthEffect: null,

  // Simple setters
  setServerUrl: (url) => set({ serverUrl: url }),
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
      audioEmitter: refs.audioEmitter,
      connection: refs.connection,
      microphone: refs.microphone,
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
    });

    const attempt: ConnectAttempt = {
      connection: null,
      healthEffect: null,
      watch: null,
      audioEmitter: null,
      microphone: null,
      publish: null,
    };

    try {
      logger.info('Step 1: Creating connection to relay server');
      // Create connection to relay server with auto-reconnect
      // Hang will automatically fetch certificate fingerprints from http://host:port/certificate.sha256
      attempt.connection = new Hang.Moq.Connection.Reload({
        url: new URL(decision.trimmedServerUrl),
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
        attempt.audioEmitter = watchSetup.audioEmitter;
      }

      if (decision.shouldPublish) {
        const publishSetup = setupPublishPath(
          attempt.healthEffect,
          attempt.connection,
          state.inputBroadcast,
          set
        );
        attempt.microphone = publishSetup.microphone;
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
        audioEmitter: attempt.audioEmitter,
        connection: attempt.connection,
        microphone: attempt.microphone,
        healthEffect: attempt.healthEffect,
        status: 'connected',
        isMicEnabled: decision.shouldPublish,
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
        errorMessage: formatConnectError(error),
        publish: null,
        watch: null,
        audioEmitter: null,
        connection: null,
        microphone: null,
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
    if (state.audioEmitter) {
      state.audioEmitter.close();
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

    set({
      status: 'disconnected',
      isMicEnabled: false,
      micStatus: 'disabled',
      watchStatus: 'disabled',
      errorMessage: '',
      publish: null,
      watch: null,
      audioEmitter: null,
      connection: null,
      microphone: null,
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
}));
