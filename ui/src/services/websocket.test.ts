// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

import { useNodeParamsStore } from '@/stores/nodeParamsStore';
import { useSessionStore } from '@/stores/sessionStore';
import type { Response, Event as WsEvent } from '@/types/types';

import { WebSocketService } from './websocket';

// Mock WebSocket
class MockWebSocket {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;

  readyState = MockWebSocket.OPEN; // Start as OPEN for simplicity
  url: string;
  onopen: ((event: Event) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;

  constructor(url: string) {
    this.url = url;
    // Simulate connection synchronously using queueMicrotask
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

  // Test helpers
  simulateMessage(data: string) {
    const event = new MessageEvent('message', { data });
    this.onmessage?.(event);
  }

  simulateError() {
    this.onerror?.(new Event('error'));
  }

  simulateClose(code = 1000, reason = 'Normal closure', wasClean = true) {
    this.readyState = MockWebSocket.CLOSED;
    const closeEvent = new CloseEvent('close', { code, reason, wasClean });
    this.onclose?.(closeEvent);
  }
}

describe('WebSocketService', () => {
  let service: WebSocketService;

  beforeEach(() => {
    // Reset stores
    useSessionStore.setState({ sessions: new Map() });
    useNodeParamsStore.setState({ paramsById: {} });

    // Mock WebSocket globally
    global.WebSocket = MockWebSocket as unknown as typeof WebSocket;

    // Ensure window object exists for setTimeout/clearTimeout
    // (websocket.ts uses window.setTimeout, not global setTimeout)
    // IMPORTANT: Don't bind to global.setTimeout - use getters so fake timers work
    if (typeof globalThis.window === 'undefined') {
      // Using 'as any' for globalThis augmentation is acceptable in tests
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
    // Close the service, which rejects pending requests
    // We wrap in try-catch to handle any unhandled rejections during cleanup
    try {
      service.close();
    } catch {
      // Ignore errors during cleanup
    }
    // Allow microtask queue to flush any pending rejections
    await Promise.resolve();
    vi.restoreAllMocks();
  });

  describe('connect', () => {
    it('should create WebSocket connection', async () => {
      service.connect();

      // Wait for microtask queue to flush
      await Promise.resolve();

      expect(service.isConnected()).toBe(true);
    });

    it('should skip reconnect if already connected', async () => {
      service.connect();
      await Promise.resolve();

      // Get the current WebSocket instance (using bracket notation to access private property)
      const firstWs = service['ws'];

      service.connect(); // Second connect attempt

      // Should still be the same WebSocket instance
      const secondWs = service['ws'];
      expect(secondWs).toBe(firstWs);
    });

    it('should notify connection status handlers on connect', async () => {
      const handler = vi.fn();
      service.onConnectionStatus(handler);

      // Handler called immediately with false (service not connected yet)
      expect(handler).toHaveBeenCalledWith(false);

      service.connect();
      await Promise.resolve();

      // Handler called with true after connection
      expect(handler).toHaveBeenCalledWith(true);
    });

    it('should flush message queue after connection', async () => {
      // Queue a message before connecting
      const request = {
        type: 'request' as const,
        correlation_id: 'test-123',
        payload: { action: 'listnodes' as const },
      };

      service.send(request).catch(() => {
        // Ignore timeout
      });

      service.connect();
      await Promise.resolve();

      // Access the mock WebSocket instance (non-null assertion safe after connect)
      const ws = service['ws']!;
      expect(ws.send).toHaveBeenCalledWith(JSON.stringify(request));
    });
  });

  // Reconnection tests moved to websocket.reconnection.test.ts

  describe('message handling', () => {
    beforeEach(async () => {
      service.connect();
      await Promise.resolve();
    });

    it('should handle response with correlation ID', async () => {
      const correlationId = 'test-correlation-123';
      const request = {
        type: 'request' as const,
        correlation_id: correlationId,
        payload: { action: 'listnodes' as const },
      };

      const responsePromise = service.send(request);

      const response: Response = {
        type: 'response' as const,
        correlation_id: correlationId,
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        payload: { nodes: [] } as any,
      };

      const ws = service['ws']! as unknown as MockWebSocket;
      ws.simulateMessage(JSON.stringify(response));

      await expect(responsePromise).resolves.toEqual(response);
    });

    it('should handle nodestatechanged event', () => {
      const event: WsEvent = {
        type: 'event' as const,
        payload: {
          event: 'nodestatechanged' as const,
          session_id: 'session-1',
          node_id: 'node-1',
          state: 'Running' as const,
          // eslint-disable-next-line @typescript-eslint/no-explicit-any
        } as any,
      };

      const ws = service['ws']! as unknown as MockWebSocket;
      ws.simulateMessage(JSON.stringify(event));

      const session = useSessionStore.getState().getSession('session-1');
      expect(session?.nodeStates['node-1']).toBe('Running');
    });

    it('should handle nodestatsupdated event', () => {
      const stats = {
        received: '100', // Stats are sent as strings over WebSocket
        sent: '95',
        discarded: '5',
        errored: '2',
        duration_secs: 10.5,
      };

      const event: WsEvent = {
        type: 'event' as const,
        payload: {
          event: 'nodestatsupdated' as const,
          session_id: 'session-1',
          node_id: 'node-1',
          stats,
          // eslint-disable-next-line @typescript-eslint/no-explicit-any
        } as any,
      };

      const ws = service['ws']! as unknown as MockWebSocket;
      ws.simulateMessage(JSON.stringify(event));

      const session = useSessionStore.getState().getSession('session-1');
      expect(session?.nodeStats['node-1']).toEqual(stats);
    });

    it('should handle nodeparamschanged event', () => {
      const params = { gain: 0.5, threshold: 0.8 };

      const event: WsEvent = {
        type: 'event' as const,
        payload: {
          event: 'nodeparamschanged' as const,
          session_id: 'session-1',
          node_id: 'node-1',
          params,
          // eslint-disable-next-line @typescript-eslint/no-explicit-any
        } as any,
      };

      const ws = service['ws']! as unknown as MockWebSocket;
      ws.simulateMessage(JSON.stringify(event));

      // Should update nodeParamsStore (uses paramsById, not params)
      const nodeParams = useNodeParamsStore.getState().getParamsForNode('node-1', 'session-1');
      expect(nodeParams).toEqual(params);
    });

    it('should scope nodeparamschanged params by session', () => {
      const ws = service['ws']! as unknown as MockWebSocket;

      ws.simulateMessage(
        JSON.stringify({
          type: 'event' as const,
          payload: {
            event: 'nodeparamschanged' as const,
            session_id: 'session-a',
            node_id: 'gain',
            params: { gain: 0.5 },
          },
        } satisfies WsEvent)
      );

      ws.simulateMessage(
        JSON.stringify({
          type: 'event' as const,
          payload: {
            event: 'nodeparamschanged' as const,
            session_id: 'session-b',
            node_id: 'gain',
            params: { gain: 2.0 },
          },
        } satisfies WsEvent)
      );

      expect(useNodeParamsStore.getState().getParam('gain', 'gain', 'session-a')).toBe(0.5);
      expect(useNodeParamsStore.getState().getParam('gain', 'gain', 'session-b')).toBe(2.0);
    });

    it('should handle nodeadded event', () => {
      // Subscribe to session and initialize pipeline
      service.subscribeToSession('session-1');

      // Initialize session with empty pipeline (addNode requires pipeline to exist)
      const sessionStore = useSessionStore.getState();
      const session = sessionStore.getSession('session-1');
      if (session) {
        useSessionStore.setState({
          sessions: new Map([
            [
              'session-1',
              {
                ...session,
                pipeline: {
                  name: null,
                  description: null,
                  mode: 'dynamic',
                  nodes: {},
                  connections: [],
                },
              },
            ],
          ]),
        });
      }

      const event: WsEvent = {
        type: 'event' as const,
        payload: {
          event: 'nodeadded' as const,
          session_id: 'session-1',
          node_id: 'node-1',
          kind: 'audio::gain',
          params: { gain: 1.0 },
          // eslint-disable-next-line @typescript-eslint/no-explicit-any
        } as any,
      };

      const ws = service['ws']! as unknown as MockWebSocket;
      ws.simulateMessage(JSON.stringify(event));

      const updatedSession = useSessionStore.getState().getSession('session-1');
      expect(updatedSession?.pipeline?.nodes['node-1']).toEqual({
        kind: 'audio::gain',
        params: { gain: 1.0 },
        state: 'Initializing',
      });
    });

    it('should notify message handlers', () => {
      const handler = vi.fn();
      service.onMessage(handler);

      const response: Response = {
        type: 'response' as const,
        correlation_id: 'test-123',
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        payload: { nodes: [] } as any,
      };

      const ws = service['ws']! as unknown as MockWebSocket;
      ws.simulateMessage(JSON.stringify(response));

      expect(handler).toHaveBeenCalledWith(response);
    });
  });

  describe('message queue', () => {
    it('should queue messages when not connected', () => {
      // Don't connect yet
      const request = {
        type: 'request' as const,
        correlation_id: 'test-123',
        payload: { action: 'listnodes' as const },
      };

      service.send(request).catch(() => {
        // Ignore timeout
      });

      // Message should be queued (using bracket notation to access private property)
      const messageQueue = service['messageQueue'];
      expect(messageQueue).toHaveLength(1);

      // Clean up: clear the pending request to avoid timeout issues
      service.close();
    });

    it('should send queued messages after connection', async () => {
      // Queue messages before connecting
      const request1 = {
        type: 'request' as const,
        correlation_id: 'test-1',
        payload: { action: 'listnodes' as const },
      };

      const request2 = {
        type: 'request' as const,
        correlation_id: 'test-2',
        payload: { action: 'listsessions' as const },
      };

      service.send(request1).catch(() => {
        // Ignore
      });
      service.send(request2).catch(() => {
        // Ignore
      });

      // Now connect
      service.connect();
      await Promise.resolve();

      const ws = service['ws']! as unknown as MockWebSocket;

      expect(ws.send).toHaveBeenCalledWith(JSON.stringify(request1));
      expect(ws.send).toHaveBeenCalledWith(JSON.stringify(request2));

      // Clean up: close to clear pending requests
      service.close();
    });

    it('should handle sendFireAndForget', async () => {
      service.connect();
      await Promise.resolve();

      const request = {
        type: 'request' as const,
        correlation_id: 'fire-forget',
        payload: {
          action: 'tunenodeasync' as const,
          session_id: 's1',
          node_id: 'n1',
          params: {},
          // eslint-disable-next-line @typescript-eslint/no-explicit-any
        } as any,
      };

      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      service.sendFireAndForget(request as any);

      const ws = service['ws']! as unknown as MockWebSocket;
      expect(ws.send).toHaveBeenCalledWith(
        expect.stringContaining('"correlation_id":"fire-forget"')
      );
    });
  });

  // Session management, close, and handler management tests moved to websocket.reconnection.test.ts
});
