// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Reconnection and timer-related tests for WebSocketService
 * Split from main test file to comply with max-lines rule
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

import { useNodeParamsStore } from '@/stores/nodeParamsStore';
import { useSessionStore } from '@/stores/sessionStore';

import { WebSocketService } from './websocket';

// Mock WebSocket
class MockWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;

  readyState = MockWebSocket.OPEN;
  url: string;
  onopen: ((event: Event) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;

  constructor(url: string) {
    this.url = url;
    queueMicrotask(() => {
      if (this.readyState === MockWebSocket.OPEN) {
        this.onopen?.(new Event('open'));
      }
    });
  }

  send = vi.fn();
  close = vi.fn(() => {
    this.readyState = MockWebSocket.CLOSED;
    const closeEvent = new CloseEvent('close', {
      code: 1000,
      reason: 'Normal closure',
      wasClean: true,
    });
    this.onclose?.(closeEvent);
  });

  simulateMessage(data: string) {
    const event = new MessageEvent('message', { data });
    this.onmessage?.(event);
  }

  simulateClose(code = 1000, reason = 'Normal closure', wasClean = true) {
    this.readyState = MockWebSocket.CLOSED;
    const closeEvent = new CloseEvent('close', { code, reason, wasClean });
    this.onclose?.(closeEvent);
  }
}

// Mock that doesn't auto-connect - allows manual control over connection state
class ManualConnectMockWebSocket extends MockWebSocket {
  constructor(url: string) {
    super(url);
    this.readyState = MockWebSocket.CONNECTING;
  }

  triggerOpen() {
    this.readyState = MockWebSocket.OPEN;
    this.onopen?.(new Event('open'));
  }
}

describe('WebSocketService reconnection', () => {
  let service: WebSocketService;

  beforeEach(() => {
    useSessionStore.setState({ sessions: new Map() });
    useNodeParamsStore.setState({ paramsById: {} });
    global.WebSocket = MockWebSocket as unknown as typeof WebSocket;

    if (typeof globalThis.window === 'undefined') {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (globalThis as any).window = {
        get setTimeout() {
          return global.setTimeout;
        },
        get clearTimeout() {
          return global.clearTimeout;
        },
      };
    }

    service = new WebSocketService('ws://localhost:4545/api/v1/control');
  });

  afterEach(async () => {
    try {
      service.close();
    } catch {
      // Ignore errors during cleanup
    }
    await Promise.resolve();
    vi.restoreAllMocks();
  });

  describe('reconnection behavior', () => {
    beforeEach(() => {
      vi.useFakeTimers();
    });

    afterEach(() => {
      vi.useRealTimers();
    });

    it('should attempt reconnection with exponential backoff', async () => {
      service.connect();
      await vi.advanceTimersByTimeAsync(0);

      expect(service.isConnected()).toBe(true);

      const ws = service['ws']! as unknown as MockWebSocket;
      ws.simulateClose(1006, 'Abnormal closure', false);

      expect(service.isConnected()).toBe(false);

      await vi.advanceTimersByTimeAsync(999);
      expect(service.isConnected()).toBe(false);

      await vi.advanceTimersByTimeAsync(1);
      await vi.advanceTimersByTimeAsync(0);
      expect(service.isConnected()).toBe(true);
    });

    it('should calculate exponential backoff correctly', async () => {
      global.WebSocket = ManualConnectMockWebSocket as unknown as typeof WebSocket;

      const testService = new WebSocketService('ws://localhost:4545/api/v1/control');
      testService.connect();
      await vi.advanceTimersByTimeAsync(0);

      const ws1 = testService['ws']! as unknown as ManualConnectMockWebSocket;
      ws1.triggerOpen();
      expect(testService.isConnected()).toBe(true);

      const expectedDelays = [1000, 2000, 4000];

      for (const expectedDelay of expectedDelays) {
        const ws = testService['ws']! as unknown as ManualConnectMockWebSocket;
        ws.simulateClose(1006, 'Abnormal closure', false);
        expect(testService.isConnected()).toBe(false);

        await vi.advanceTimersByTimeAsync(expectedDelay - 1);
        expect(testService.isConnected()).toBe(false);

        await vi.advanceTimersByTimeAsync(1);
        await vi.advanceTimersByTimeAsync(0);

        const newWs = testService['ws']! as unknown as ManualConnectMockWebSocket;
        newWs.triggerOpen();
        expect(testService.isConnected()).toBe(true);
      }

      testService.close();
    });

    it('should stop reconnecting after max attempts', async () => {
      let connectionAttempts = 0;
      const FailingMockWebSocket = class extends MockWebSocket {
        constructor(url: string) {
          super(url);
          this.readyState = MockWebSocket.CONNECTING;
          connectionAttempts++;
          queueMicrotask(() => {
            this.onerror?.(new Event('error'));
            this.readyState = MockWebSocket.CLOSED;
            this.onclose?.(
              new CloseEvent('close', { code: 1006, reason: 'Connection failed', wasClean: false })
            );
          });
        }
      };
      global.WebSocket = FailingMockWebSocket as unknown as typeof WebSocket;

      const testService = new WebSocketService('ws://localhost:4545/api/v1/control');
      testService.connect();

      for (let i = 0; i < 12; i++) {
        await vi.advanceTimersByTimeAsync(30001);
        await vi.advanceTimersByTimeAsync(0);
      }

      expect(connectionAttempts).toBeLessThanOrEqual(11);

      testService.close();
    });

    it('should not reconnect if intentionally closed', async () => {
      service.connect();
      await vi.advanceTimersByTimeAsync(0);
      expect(service.isConnected()).toBe(true);

      service.close();

      expect(service.isConnected()).toBe(false);

      await vi.advanceTimersByTimeAsync(60000);
      await vi.advanceTimersByTimeAsync(0);

      expect(service.isConnected()).toBe(false);
    });

    it('should resubscribe to sessions after reconnection', async () => {
      service.connect();
      await vi.advanceTimersByTimeAsync(0);

      service.subscribeToSession('session-1');

      const ws = service['ws']! as unknown as MockWebSocket;
      ws.simulateClose(1006, 'Abnormal closure', false);

      await vi.advanceTimersByTimeAsync(1001);
      await vi.advanceTimersByTimeAsync(0);

      expect(service.isConnected()).toBe(true);

      const newWs = service['ws']! as unknown as MockWebSocket;
      const sendCalls = newWs.send.mock.calls as string[][];
      const resubscribeCall = sendCalls.find((call) => {
        const payload = JSON.parse(call[0]);
        return (
          payload.payload?.action === 'getpipeline' && payload.payload?.session_id === 'session-1'
        );
      });
      expect(resubscribeCall).toBeDefined();

      if (resubscribeCall) {
        const requestPayload = JSON.parse(resubscribeCall[0]);
        const correlationId = requestPayload.correlation_id;
        newWs.simulateMessage(
          JSON.stringify({
            type: 'response',
            correlation_id: correlationId,
            payload: { pipeline: null },
          })
        );
      }
    });
  });

  describe('timer-dependent tests', () => {
    it('should timeout request after 5 seconds', async () => {
      useSessionStore.setState({ sessions: new Map() });
      useNodeParamsStore.setState({ paramsById: {} });

      vi.useFakeTimers();

      global.WebSocket = MockWebSocket as unknown as typeof WebSocket;

      const testService = new WebSocketService('ws://localhost:4545/api/v1/control');
      testService.connect();
      await vi.advanceTimersByTimeAsync(0);

      const request = {
        type: 'request' as const,
        correlation_id: 'test-timeout',
        payload: { action: 'listnodes' as const },
      };

      let rejectionError: Error | null = null;
      const responsePromise = testService.send(request);
      responsePromise.catch((e: Error) => {
        rejectionError = e;
      });

      await vi.advanceTimersByTimeAsync(5000);

      expect(rejectionError).not.toBeNull();
      expect((rejectionError as Error | null)?.message).toBe('Request timeout');

      testService.close();
      vi.clearAllTimers();
      vi.useRealTimers();
    });

    it('should cancel reconnection timeout on close', async () => {
      useSessionStore.setState({ sessions: new Map() });
      useNodeParamsStore.setState({ paramsById: {} });

      vi.useFakeTimers();

      global.WebSocket = MockWebSocket as unknown as typeof WebSocket;

      const testService = new WebSocketService('ws://localhost:4545/api/v1/control');
      testService.connect();
      await vi.advanceTimersByTimeAsync(0);

      const ws = testService['ws']! as unknown as MockWebSocket;
      ws.simulateClose(1006, 'Abnormal closure', false);

      testService.close();

      await vi.advanceTimersByTimeAsync(5000);
      await vi.advanceTimersByTimeAsync(0);

      expect(testService.isConnected()).toBe(false);

      vi.clearAllTimers();
      vi.useRealTimers();
    });
  });

  describe('session management', () => {
    beforeEach(async () => {
      service.connect();
      await Promise.resolve();
    });

    it('should subscribe to session and update connection status', () => {
      service.subscribeToSession('session-1');

      const session = useSessionStore.getState().getSession('session-1');
      expect(session?.isConnected).toBe(true);
    });

    it('should unsubscribe from session and keep cached session', () => {
      service.subscribeToSession('session-1');
      service.unsubscribeFromSession('session-1');

      const session = useSessionStore.getState().getSession('session-1');
      expect(session?.isConnected).toBe(false);
    });

    it('should update all subscribed sessions on connection status change', () => {
      service.subscribeToSession('session-1');
      service.subscribeToSession('session-2');

      const ws = service['ws']! as unknown as MockWebSocket;
      ws.simulateClose(1006, 'Abnormal closure', false);

      const session1 = useSessionStore.getState().getSession('session-1');
      const session2 = useSessionStore.getState().getSession('session-2');

      expect(session1?.isConnected).toBe(false);
      expect(session2?.isConnected).toBe(false);
    });
  });

  describe('close', () => {
    beforeEach(async () => {
      service.connect();
      await Promise.resolve();
    });

    it('should close WebSocket connection', () => {
      service.close();

      expect(service.isConnected()).toBe(false);
    });

    it('should reject all pending requests', async () => {
      const request = {
        type: 'request' as const,
        correlation_id: 'pending',
        payload: { action: 'listnodes' as const },
      };

      const promise = service.send(request);

      service.close();

      await expect(promise).rejects.toThrow('WebSocket closed');
    });

    it('should clear all handlers', () => {
      const messageHandler = vi.fn();
      const statusHandler = vi.fn();

      service.onMessage(messageHandler);
      service.onConnectionStatus(statusHandler);

      service.close();

      const messageHandlers = service['messageHandlers'];
      const statusHandlers = service['connectionStatusHandlers'];

      expect(messageHandlers.size).toBe(0);
      expect(statusHandlers.size).toBe(0);
    });
  });

  describe('handler management', () => {
    it('should return unsubscribe function for message handlers', () => {
      const handler = vi.fn();
      const unsubscribe = service.onMessage(handler);

      unsubscribe();

      const messageHandlers = service['messageHandlers'];
      expect(messageHandlers.has(handler)).toBe(false);
    });

    it('should return unsubscribe function for connection status handlers', () => {
      const handler = vi.fn();
      const unsubscribe = service.onConnectionStatus(handler);

      unsubscribe();

      const statusHandlers = service['connectionStatusHandlers'];
      expect(statusHandlers.has(handler)).toBe(false);
    });
  });
});
