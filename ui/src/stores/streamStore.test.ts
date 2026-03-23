// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useStreamStore, type ConnectionStatus } from './streamStore';
import * as configService from '../services/config';

// Mock the Hang library to avoid import errors
vi.mock('@moq/hang', () => ({
  default: {},
  Moq: {
    Connection: {
      Reload: vi.fn(),
    },
    Path: {
      from: vi.fn(),
    },
  },
}));

vi.mock('@moq/watch', () => ({
  default: {},
  Broadcast: vi.fn(),
  Sync: vi.fn(),
  Lite: {
    Path: {
      from: vi.fn(),
    },
  },
  Audio: {
    Source: vi.fn(),
    Decoder: vi.fn(),
    Emitter: vi.fn(),
  },
  Video: {
    Source: vi.fn(),
    Decoder: vi.fn(),
    Renderer: vi.fn(),
  },
}));

vi.mock('@moq/publish', () => ({
  default: {},
  Broadcast: vi.fn(),
  Lite: {
    Path: {
      from: vi.fn(),
    },
  },
  Source: {
    Microphone: vi.fn(),
    Camera: vi.fn(),
  },
}));

// Mock config service
vi.mock('../services/config', () => ({
  fetchConfig: vi.fn(),
}));

// Mock types for Hang library objects (since we're mocking the library)
type MockCloseable = { close: () => void };
type MockPublish = {
  audio: {
    enabled: {
      set: (value: boolean) => void;
    };
  };
} & MockCloseable;
type MockMicrophone =
  | MockCloseable
  | {
      enabled: {
        set: (value: boolean) => void;
      };
    };

describe('streamStore', () => {
  beforeEach(() => {
    // Reset store to initial state
    useStreamStore.setState({
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
      watchStatus: 'disabled',
      pipelineNeedsAudio: true,
      pipelineNeedsVideo: true,
      pipelineOutputsAudio: true,
      pipelineOutputsVideo: true,
      errorMessage: '',
      configLoaded: false,
      connectAbort: null,
      connectingStep: '',
      activeSessionId: null,
      activeSessionName: null,
      activePipelineName: null,
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

    vi.clearAllMocks();
  });

  describe('initial state', () => {
    it('should start with disconnected status', () => {
      const state = useStreamStore.getState();
      expect(state.status).toBe('disconnected');
    });

    it('should have default server URL', () => {
      const state = useStreamStore.getState();
      expect(state.serverUrl).toBe('');
    });

    it('should have default broadcast names', () => {
      const state = useStreamStore.getState();
      expect(state.inputBroadcast).toBe('input');
      expect(state.outputBroadcast).toBe('output');
    });

    it('should start with mic disabled', () => {
      const state = useStreamStore.getState();
      expect(state.isMicEnabled).toBe(false);
      expect(state.micStatus).toBe('disabled');
    });

    it('should have no active session initially', () => {
      const state = useStreamStore.getState();
      expect(state.activeSessionId).toBeNull();
      expect(state.activeSessionName).toBeNull();
      expect(state.activePipelineName).toBeNull();
    });

    it('should have null connectAbort and empty connectingStep initially', () => {
      const state = useStreamStore.getState();
      expect(state.connectAbort).toBeNull();
      expect(state.connectingStep).toBe('');
    });

    it('should have null MoQ references initially', () => {
      const state = useStreamStore.getState();
      expect(state.publish).toBeNull();
      expect(state.watch).toBeNull();
      expect(state.watchSync).toBeNull();
      expect(state.audioSource).toBeNull();
      expect(state.audioDecoder).toBeNull();
      expect(state.audioEmitter).toBeNull();
      expect(state.videoSource).toBeNull();
      expect(state.videoDecoder).toBeNull();
      expect(state.videoRenderer).toBeNull();
      expect(state.connection).toBeNull();
      expect(state.microphone).toBeNull();
      expect(state.camera).toBeNull();
      expect(state.healthEffect).toBeNull();
      expect(state.watchStatus).toBe('disabled');
    });
  });

  describe('simple setters', () => {
    it('should set server URL', () => {
      const { setServerUrl } = useStreamStore.getState();
      setServerUrl('http://example.com:8080/moq');

      expect(useStreamStore.getState().serverUrl).toBe('http://example.com:8080/moq');
    });

    it('should set MoQ token', () => {
      const { setMoqToken } = useStreamStore.getState();
      setMoqToken('jwt-token-123');

      expect(useStreamStore.getState().moqToken).toBe('jwt-token-123');
    });

    it('should set input broadcast', () => {
      const { setInputBroadcast } = useStreamStore.getState();
      setInputBroadcast('custom-input');

      expect(useStreamStore.getState().inputBroadcast).toBe('custom-input');
    });

    it('should set output broadcast', () => {
      const { setOutputBroadcast } = useStreamStore.getState();
      setOutputBroadcast('custom-output');

      expect(useStreamStore.getState().outputBroadcast).toBe('custom-output');
    });

    it('should set status', () => {
      const { setStatus } = useStreamStore.getState();
      const statuses: ConnectionStatus[] = ['disconnected', 'connecting', 'connected'];

      statuses.forEach((status) => {
        setStatus(status);
        expect(useStreamStore.getState().status).toBe(status);
      });
    });

    it('should set error message', () => {
      const { setErrorMessage } = useStreamStore.getState();
      setErrorMessage('Test error message');

      expect(useStreamStore.getState().errorMessage).toBe('Test error message');
    });

    it('should set mic enabled state', () => {
      const { setIsMicEnabled } = useStreamStore.getState();

      setIsMicEnabled(true);
      expect(useStreamStore.getState().isMicEnabled).toBe(true);

      setIsMicEnabled(false);
      expect(useStreamStore.getState().isMicEnabled).toBe(false);
    });
  });

  describe('session management', () => {
    it('should set active session', () => {
      const { setActiveSession } = useStreamStore.getState();
      setActiveSession('session-123', 'My Session', 'My Pipeline');

      const state = useStreamStore.getState();
      expect(state.activeSessionId).toBe('session-123');
      expect(state.activeSessionName).toBe('My Session');
      expect(state.activePipelineName).toBe('My Pipeline');
    });

    it('should handle null session name', () => {
      const { setActiveSession } = useStreamStore.getState();
      setActiveSession('session-456', null, 'Pipeline');

      const state = useStreamStore.getState();
      expect(state.activeSessionId).toBe('session-456');
      expect(state.activeSessionName).toBeNull();
      expect(state.activePipelineName).toBe('Pipeline');
    });

    it('should clear active session', () => {
      const { setActiveSession, clearActiveSession } = useStreamStore.getState();

      // First set a session
      setActiveSession('session-789', 'Test Session', 'Test Pipeline');
      expect(useStreamStore.getState().activeSessionId).toBe('session-789');

      // Then clear it
      clearActiveSession();

      const state = useStreamStore.getState();
      expect(state.activeSessionId).toBeNull();
      expect(state.activeSessionName).toBeNull();
      expect(state.activePipelineName).toBeNull();
    });
  });

  describe('loadConfig', () => {
    it('should load config and update server URL', async () => {
      const mockConfig = {
        moqGatewayUrl: 'http://config-server.com:9000/moq',
      };

      vi.mocked(configService.fetchConfig).mockResolvedValue(mockConfig);

      const { loadConfig } = useStreamStore.getState();
      await loadConfig();

      const state = useStreamStore.getState();
      expect(state.serverUrl).toBe('http://config-server.com:9000/moq');
      expect(state.configLoaded).toBe(true);
    });

    it('should set configLoaded when no moqGatewayUrl in config', async () => {
      const mockConfig = {};

      vi.mocked(configService.fetchConfig).mockResolvedValue(mockConfig);

      const { loadConfig } = useStreamStore.getState();
      await loadConfig();

      const state = useStreamStore.getState();
      // URL should remain unset; streaming requires server config or manual entry
      expect(state.serverUrl).toBe('');
      expect(state.configLoaded).toBe(true);
      expect(state.errorMessage).toContain('moqGatewayUrl');
    });

    it('should handle config fetch error gracefully', async () => {
      vi.mocked(configService.fetchConfig).mockRejectedValue(new Error('Network error'));

      const { loadConfig } = useStreamStore.getState();
      await loadConfig();

      const state = useStreamStore.getState();
      // Should still mark as loaded (don't fail - allow manual entry)
      expect(state.configLoaded).toBe(true);
      // URL should remain unset
      expect(state.serverUrl).toBe('');
      expect(state.errorMessage).toContain('Failed to load');
    });
  });

  describe('toggleMicrophone', () => {
    it('should not do anything when publish is null', () => {
      const { toggleMicrophone } = useStreamStore.getState();

      // Should not throw
      expect(() => toggleMicrophone()).not.toThrow();

      // State should not change
      expect(useStreamStore.getState().isMicEnabled).toBe(false);
    });

    it('should toggle microphone when publish exists', () => {
      const mockPublish: MockPublish = {
        close: vi.fn(),
        audio: {
          enabled: {
            set: vi.fn(),
          },
        },
      };

      // Set up store with mock publish
      useStreamStore.setState({
        publish: mockPublish as never,
        isMicEnabled: false,
      });

      const { toggleMicrophone } = useStreamStore.getState();

      // Toggle on
      toggleMicrophone();
      expect(mockPublish.audio.enabled.set).toHaveBeenCalledWith(true);
      expect(useStreamStore.getState().isMicEnabled).toBe(true);

      // Toggle off
      toggleMicrophone();
      expect(mockPublish.audio.enabled.set).toHaveBeenCalledWith(false);
      expect(useStreamStore.getState().isMicEnabled).toBe(false);
    });
  });

  describe('setMoqRefs', () => {
    it('should store MoQ object references', () => {
      const mockRefs = {
        publish: { close: vi.fn() } as never,
        watch: { close: vi.fn() } as never,
        watchSync: { close: vi.fn() } as never,
        audioSource: { close: vi.fn() } as never,
        audioDecoder: { close: vi.fn() } as never,
        audioEmitter: { close: vi.fn() } as never,
        videoSource: { close: vi.fn() } as never,
        videoDecoder: { close: vi.fn() } as never,
        videoRenderer: { close: vi.fn() } as never,
        connection: { close: vi.fn() } as never,
        microphone: { close: vi.fn() } as never,
        camera: { close: vi.fn() } as never,
      };

      const { setMoqRefs } = useStreamStore.getState();
      setMoqRefs(mockRefs);

      const state = useStreamStore.getState();
      expect(state.publish).toBe(mockRefs.publish);
      expect(state.watch).toBe(mockRefs.watch);
      expect(state.watchSync).toBe(mockRefs.watchSync);
      expect(state.audioSource).toBe(mockRefs.audioSource);
      expect(state.audioDecoder).toBe(mockRefs.audioDecoder);
      expect(state.audioEmitter).toBe(mockRefs.audioEmitter);
      expect(state.videoSource).toBe(mockRefs.videoSource);
      expect(state.videoDecoder).toBe(mockRefs.videoDecoder);
      expect(state.videoRenderer).toBe(mockRefs.videoRenderer);
      expect(state.connection).toBe(mockRefs.connection);
      expect(state.microphone).toBe(mockRefs.microphone);
      expect(state.camera).toBe(mockRefs.camera);
    });
  });

  describe('disconnect', () => {
    /** Create a connected store populated with mock closeables. */
    function setupConnectedStore() {
      const mocks = {
        publish: { close: vi.fn() } as MockCloseable,
        watch: { close: vi.fn() } as MockCloseable,
        watchSync: { close: vi.fn() } as MockCloseable,
        audioSource: { close: vi.fn() } as MockCloseable,
        audioDecoder: { close: vi.fn() } as MockCloseable,
        audioEmitter: { close: vi.fn() } as MockCloseable,
        videoSource: { close: vi.fn() } as MockCloseable,
        videoDecoder: { close: vi.fn() } as MockCloseable,
        videoRenderer: { close: vi.fn() } as MockCloseable,
        connection: { close: vi.fn() } as MockCloseable,
        microphone: { close: vi.fn() } as MockCloseable,
      };
      useStreamStore.setState({
        status: 'connected',
        isMicEnabled: true,
        errorMessage: 'some error',
        publish: mocks.publish as never,
        watch: mocks.watch as never,
        watchSync: mocks.watchSync as never,
        audioSource: mocks.audioSource as never,
        audioDecoder: mocks.audioDecoder as never,
        audioEmitter: mocks.audioEmitter as never,
        videoSource: mocks.videoSource as never,
        videoDecoder: mocks.videoDecoder as never,
        videoRenderer: mocks.videoRenderer as never,
        connection: mocks.connection as never,
        microphone: mocks.microphone as never,
      });
      return mocks;
    }

    it('should close all MoQ resources', () => {
      const mocks = setupConnectedStore();
      useStreamStore.getState().disconnect();

      for (const mock of Object.values(mocks)) {
        expect(mock.close).toHaveBeenCalled();
      }
    });

    it('should reset state after disconnect', () => {
      setupConnectedStore();
      useStreamStore.getState().disconnect();

      const state = useStreamStore.getState();
      expect(state.status).toBe('disconnected');
      expect(state.isMicEnabled).toBe(false);
      expect(state.micStatus).toBe('disabled');
      expect(state.watchStatus).toBe('disabled');
      expect(state.healthEffect).toBeNull();
      expect(state.errorMessage).toBe('');
      expect(state.publish).toBeNull();
      expect(state.watch).toBeNull();
      expect(state.watchSync).toBeNull();
      expect(state.audioSource).toBeNull();
      expect(state.audioDecoder).toBeNull();
      expect(state.audioEmitter).toBeNull();
      expect(state.videoSource).toBeNull();
      expect(state.videoDecoder).toBeNull();
      expect(state.videoRenderer).toBeNull();
      expect(state.connection).toBeNull();
      expect(state.microphone).toBeNull();
    });

    it('should handle microphone without close method', () => {
      const mockMicrophone: MockMicrophone = {
        enabled: {
          set: vi.fn(),
        },
      };

      useStreamStore.setState({
        microphone: mockMicrophone as never,
      });

      const { disconnect } = useStreamStore.getState();

      // Should not throw
      expect(() => disconnect()).not.toThrow();

      // enabled.set should be called to disable
      expect(mockMicrophone.enabled.set).toHaveBeenCalledWith(false);
    });

    it('should safely handle null references during disconnect', () => {
      useStreamStore.setState({
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
      });

      const { disconnect } = useStreamStore.getState();

      // Should not throw
      expect(() => disconnect()).not.toThrow();

      expect(useStreamStore.getState().status).toBe('disconnected');
    });

    it('should abort in-flight connect attempt on disconnect', () => {
      const abort = new AbortController();
      const abortSpy = vi.spyOn(abort, 'abort');
      useStreamStore.setState({
        status: 'connecting',
        connectAbort: abort,
      });

      useStreamStore.getState().disconnect();

      expect(abortSpy).toHaveBeenCalled();
      const state = useStreamStore.getState();
      expect(state.status).toBe('disconnected');
      expect(state.connectAbort).toBeNull();
      expect(state.connectingStep).toBe('');
    });

    it('should clear connectingStep on disconnect', () => {
      useStreamStore.setState({
        status: 'connecting',
        connectingStep: 'devices',
      });

      useStreamStore.getState().disconnect();

      expect(useStreamStore.getState().connectingStep).toBe('');
    });
  });

  describe('connection state machine', () => {
    it('should prevent connect when already connecting', async () => {
      useStreamStore.setState({ status: 'connecting' });

      const { connect } = useStreamStore.getState();
      await connect();

      // Status should remain connecting (connect() returns early)
      expect(useStreamStore.getState().status).toBe('connecting');
    });

    it('should prevent connect when already connected', async () => {
      useStreamStore.setState({ status: 'connected' });

      const { connect } = useStreamStore.getState();
      await connect();

      // Status should remain connected (connect() returns early)
      expect(useStreamStore.getState().status).toBe('connected');
    });
  });
});
