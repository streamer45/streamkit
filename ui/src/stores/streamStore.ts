// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import * as Hang from '@moq/hang';
import * as Publish from '@moq/publish';
import { Effect } from '@moq/signals';
import * as Watch from '@moq/watch';
import { create } from 'zustand';

import {
  cleanupConnectAttempt,
  decideConnect,
  NULL_MOQ_REFS,
  performConnect,
} from './streamStoreHelpers';
import { fetchConfig } from '../services/config';
import { getLogger } from '../utils/logger';

const logger = getLogger('streamStore');

export type ConnectionStatus = 'disconnected' | 'connecting' | 'connected';
export type ConnectionMode = 'session' | 'direct';
export type WatchStatus = 'disabled' | 'offline' | 'loading' | 'live';
export type MicStatus = 'disabled' | 'requesting' | 'ready' | 'error';
export type CameraStatus = 'disabled' | 'requesting' | 'ready' | 'error';

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

  // Pipeline media-type flags (which devices the pipeline expects from the client)
  pipelineNeedsAudio: boolean;
  pipelineNeedsVideo: boolean;

  // Pipeline output-type flags (which media types the pipeline outputs to subscribers)
  pipelineOutputsAudio: boolean;
  pipelineOutputsVideo: boolean;

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
  setPipelineMediaTypes: (audio: boolean, video: boolean) => void;
  setPipelineOutputTypes: (audio: boolean, video: boolean) => void;
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
  pipelineNeedsAudio: true,
  pipelineNeedsVideo: true,
  pipelineOutputsAudio: true,
  pipelineOutputsVideo: true,
  errorMessage: '',
  configLoaded: false,

  // Active session state
  activeSessionId: null,
  activeSessionName: null,
  activePipelineName: null,

  // MoQ references
  ...NULL_MOQ_REFS,

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
  setPipelineMediaTypes: (audio, video) =>
    set({ pipelineNeedsAudio: audio, pipelineNeedsVideo: video }),
  setPipelineOutputTypes: (audio, video) =>
    set({ pipelineOutputsAudio: audio, pipelineOutputsVideo: video }),

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
      micStatus: decision.shouldPublish && state.pipelineNeedsAudio ? 'requesting' : 'disabled',
      cameraStatus: decision.shouldPublish && state.pipelineNeedsVideo ? 'requesting' : 'disabled',
    });

    return performConnect(state, decision, get, set);
  },

  disconnect: () => {
    const state = get();

    // Reuse the same teardown logic used when a connect attempt fails.
    cleanupConnectAttempt(state);

    set({
      status: 'disconnected',
      isMicEnabled: false,
      micStatus: 'disabled',
      isCameraEnabled: false,
      cameraStatus: 'disabled',
      watchStatus: 'disabled',
      errorMessage: '',
      ...NULL_MOQ_REFS,
    });
  },

  toggleMicrophone: () => {
    const state = get();

    if (state.publish?.audio) {
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
