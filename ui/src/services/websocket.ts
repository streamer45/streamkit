// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { v4 as uuidv4 } from 'uuid';

import { getLocalConfigRev, resetAllConfigRevs } from '@/hooks/useConfigRev';
import {
  batchWriteNodeStates,
  batchWriteNodeStats,
  seedPipelineAtoms,
  writeNodeViewData,
  writeSessionConnected,
  clearSessionAtoms,
  resetSessionParams,
  writeNodeParams,
} from '@/stores/sessionAtoms';
import { useSessionStore } from '@/stores/sessionStore';
import { useTelemetryStore, parseTelemetryEvent } from '@/stores/telemetryStore';
import type { Request, Response, Event, MessageType, NodeState, NodeStats } from '@/types/types';
import { getBasePathname } from '@/utils/baseHref';
import { getLogger } from '@/utils/logger';

const logger = getLogger('websocket');

type MessageHandler = (message: Response | Event) => void;
type ConnectionStatusHandler = (connected: boolean) => void;
type WsEventPayload = Event['payload'];
type SessionDestroyedPayload = Extract<WsEventPayload, { event: 'sessiondestroyed' }>;
type NodeStateChangedPayload = Extract<WsEventPayload, { event: 'nodestatechanged' }>;
type NodeStatsUpdatedPayload = Extract<WsEventPayload, { event: 'nodestatsupdated' }>;
type NodeParamsChangedPayload = Extract<WsEventPayload, { event: 'nodeparamschanged' }>;
type NodeAddedPayload = Extract<WsEventPayload, { event: 'nodeadded' }>;
type NodeRemovedPayload = Extract<WsEventPayload, { event: 'noderemoved' }>;
type ConnectionAddedPayload = Extract<WsEventPayload, { event: 'connectionadded' }>;
type ConnectionRemovedPayload = Extract<WsEventPayload, { event: 'connectionremoved' }>;
type NodeTelemetryPayload = Extract<WsEventPayload, { event: 'nodetelemetry' }>;
type NodeViewDataUpdatedPayload = Extract<WsEventPayload, { event: 'nodeviewdataupdated' }>;
type RuntimeSchemasUpdatedPayload = Extract<WsEventPayload, { event: 'runtimeschemasupdated' }>;

interface PendingRequest {
  resolve: (response: Response) => void;
  reject: (error: Error) => void;
  timeout: number;
}

export class WebSocketService {
  private ws: WebSocket | null = null;
  private url: string;
  private reconnectAttempts = 0;
  private maxReconnectAttempts = 10;
  private reconnectTimeout: number | null = null;
  private messageHandlers: Set<MessageHandler> = new Set();
  private connectionStatusHandlers: Set<ConnectionStatusHandler> = new Set();
  private pendingRequests: Map<string, PendingRequest> = new Map();
  private messageQueue: Request[] = [];
  private isIntentionallyClosed = false;
  private subscribedSessions: Set<string> = new Set();

  /** Stable sender nonce for this WebSocket session.
   *  Generated on connect, reset on reconnect.  Used in `_sender` fields
   *  so the client can distinguish its own stale echoes from other clients'
   *  updates.  Exposed via `getClientNonce()`. */
  private clientNonce: string = uuidv4();

  // Buffer node-state and node-stats updates that arrive in rapid
  // succession (e.g. during session initialisation) and flush them as a
  // single store mutation at the next animation frame.
  private pendingNodeStates: Map<string, Map<string, NodeState>> = new Map();
  private pendingNodeStats: Map<string, Map<string, NodeStats>> = new Map();
  private batchFlushRafId: number | null = null;

  constructor(url: string) {
    this.url = url;
  }

  connect(): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      logger.debug('Already connected, skipping reconnect');
      return;
    }

    if (this.ws?.readyState === WebSocket.CONNECTING) {
      logger.debug('Connection already in progress');
      return;
    }

    this.isIntentionallyClosed = false;

    try {
      logger.info('Creating new WebSocket connection to:', this.url);
      this.ws = new WebSocket(this.url);

      this.ws.onopen = () => {
        logger.info('Connected (onopen fired)');
        this.reconnectAttempts = 0;
        // Server resets its config_rev on restart, so our per-node
        // counters must reset alongside the nonce — otherwise a stale
        // localRev would gate fresh server view-data as a self-echo.
        this.clientNonce = uuidv4();
        resetAllConfigRevs();
        this.notifyConnectionStatus(true);
        this.flushMessageQueue();
        this.resubscribeToSessions();
      };

      this.ws.onmessage = (event) => {
        try {
          const message = JSON.parse(event.data) as Response | Event;
          this.handleMessage(message);
        } catch (error) {
          logger.error('Failed to parse message:', error);
        }
      };

      this.ws.onerror = (error) => {
        logger.error('Error:', error);
      };

      this.ws.onclose = (event) => {
        logger.info('Disconnected (onclose fired)', {
          code: event.code,
          reason: event.reason,
          wasClean: event.wasClean,
        });
        this.notifyConnectionStatus(false);

        if (!this.isIntentionallyClosed) {
          this.scheduleReconnect();
        }
      };

      logger.debug('Event handlers attached, readyState:', this.ws.readyState);
    } catch (error) {
      logger.error('Failed to create connection:', error);
      this.scheduleReconnect();
    }
  }

  private scheduleReconnect(): void {
    if (this.reconnectAttempts >= this.maxReconnectAttempts) {
      logger.error('Max reconnection attempts reached');
      return;
    }

    const delay = Math.min(1000 * Math.pow(2, this.reconnectAttempts), 30000);
    this.reconnectAttempts++;

    logger.info(`Reconnecting in ${delay}ms (attempt ${this.reconnectAttempts})`);

    this.reconnectTimeout = window.setTimeout(() => {
      this.connect();
    }, delay);
  }

  private resubscribeToSessions(): void {
    // Re-subscribe to all sessions after reconnection.
    // Re-call initSession for each one to ensure the session entry exists
    // in the store — if the entry was cleared during the disconnect window,
    // events arriving before the next RAF flush would be silently dropped.
    this.subscribedSessions.forEach((sessionId) => {
      logger.info('Re-subscribing to session:', sessionId);
      writeSessionConnected(sessionId, true);
      useSessionStore.getState().initSession(sessionId, true);
      this.fetchPipeline(sessionId);
    });
  }

  /** Fetch the pipeline for a session via WS and populate the session store
   *  + Jotai atoms.  Used on initial subscribe and on reconnect. */
  private fetchPipeline(sessionId: string): void {
    this.send({
      type: 'request' as MessageType,
      correlation_id: uuidv4(),
      payload: {
        action: 'getpipeline' as const,
        session_id: sessionId,
      },
    })
      .then((response) => {
        const payload = response.payload as import('@/types/types').ResponsePayload;
        if ('pipeline' in payload && payload.pipeline) {
          useSessionStore.getState().setPipeline(sessionId, payload.pipeline);
          seedPipelineAtoms(sessionId, payload.pipeline);
        }
      })
      .catch((err) => {
        logger.error('Failed to fetch pipeline for session', sessionId, ':', err);
      });
  }

  private handleMessage(message: Response | Event): void {
    if (message.type === 'response' && message.correlation_id) {
      const pending = this.pendingRequests.get(message.correlation_id);
      if (pending) {
        clearTimeout(pending.timeout);
        this.pendingRequests.delete(message.correlation_id);
        pending.resolve(message as Response);
        return;
      }
    }

    if (message.type === 'event') {
      this.handleEvent(message as Event);
    }

    this.messageHandlers.forEach((handler) => {
      try {
        handler(message);
      } catch (error) {
        logger.error('Message handler error:', error);
      }
    });
  }

  private handleEvent(event: Event): void {
    const payload = event.payload;
    switch (payload.event) {
      case 'sessiondestroyed':
        this.handleSessionDestroyed(payload);
        break;
      case 'nodestatechanged':
        this.handleNodeStateChanged(payload);
        break;
      case 'nodestatsupdated':
        this.handleNodeStatsUpdated(payload);
        break;
      case 'nodeparamschanged':
        this.handleNodeParamsChanged(payload);
        break;
      case 'nodeadded':
        this.handleNodeAdded(payload);
        break;
      case 'noderemoved':
        this.handleNodeRemoved(payload);
        break;
      case 'connectionadded':
        this.handleConnectionAdded(payload);
        break;
      case 'connectionremoved':
        this.handleConnectionRemoved(payload);
        break;
      case 'nodetelemetry':
        this.handleNodeTelemetry(payload);
        break;
      case 'nodeviewdataupdated':
        this.handleNodeViewDataUpdated(payload);
        break;
      case 'runtimeschemasupdated':
        this.handleRuntimeSchemasUpdated(payload);
        break;
      default:
        break;
    }
  }

  private handleSessionDestroyed(payload: SessionDestroyedPayload): void {
    this.subscribedSessions.delete(payload.session_id);
    // Discard any buffered updates for this session so the RAF flush
    // doesn't needlessly process stale entries.
    this.pendingNodeStates.delete(payload.session_id);
    this.pendingNodeStats.delete(payload.session_id);
    clearSessionAtoms(payload.session_id);
    useSessionStore.getState().clearSession(payload.session_id);
    useTelemetryStore.getState().clearSession(payload.session_id);
  }

  private handleNodeStateChanged(payload: NodeStateChangedPayload): void {
    const { session_id, node_id, state } = payload;
    let sessionMap = this.pendingNodeStates.get(session_id);
    if (!sessionMap) {
      sessionMap = new Map();
      this.pendingNodeStates.set(session_id, sessionMap);
    }
    sessionMap.set(node_id, state);
    this.scheduleBatchFlush();
  }

  private handleNodeStatsUpdated(payload: NodeStatsUpdatedPayload): void {
    const { session_id, node_id, stats } = payload;
    let sessionMap = this.pendingNodeStats.get(session_id);
    if (!sessionMap) {
      sessionMap = new Map();
      this.pendingNodeStats.set(session_id, sessionMap);
    }
    sessionMap.set(node_id, stats);
    this.scheduleBatchFlush();
  }

  /**
   * Schedule a `requestAnimationFrame` callback to flush buffered
   * node-state and node-stats updates.  Unlike `queueMicrotask` (which
   * drains after each macrotask), RAF coalesces updates across *all*
   * WebSocket `onmessage` macrotasks that arrive within a single
   * animation frame (~16 ms at 60 fps).  This dramatically reduces the
   * number of Zustand `set()` calls — and therefore React re-renders —
   * during session load where many state events arrive in a burst.
   */
  private scheduleBatchFlush(): void {
    if (this.batchFlushRafId !== null) return;
    this.batchFlushRafId = requestAnimationFrame(() => this.flushBatchedUpdates());
  }

  private flushBatchedUpdates(): void {
    this.batchFlushRafId = null;

    const stateUpdates = new Map<string, Record<string, NodeState>>();
    for (const [sessionId, updates] of this.pendingNodeStates) {
      stateUpdates.set(sessionId, Object.fromEntries(updates));
    }
    this.pendingNodeStates.clear();

    const statsUpdates = new Map<string, Record<string, NodeStats>>();
    for (const [sessionId, updates] of this.pendingNodeStats) {
      statsUpdates.set(sessionId, Object.fromEntries(updates));
    }
    this.pendingNodeStats.clear();

    if (stateUpdates.size > 0 || statsUpdates.size > 0) {
      batchWriteNodeStates(stateUpdates);
      batchWriteNodeStats(statsUpdates);
    }
  }

  private handleNodeParamsChanged(payload: NodeParamsChangedPayload): void {
    const { session_id, node_id, params } = payload;

    if (params && typeof params === 'object' && !Array.isArray(params)) {
      const p = params as Record<string, unknown>;

      // Stale echo-back gate: if this event originated from us and the rev
      // is <= our local counter, it's a stale echo — skip it.
      const sender = typeof p._sender === 'string' ? p._sender : undefined;
      const rev = typeof p._rev === 'number' ? p._rev : undefined;
      if (sender && sender === this.clientNonce && rev !== undefined) {
        const localRev = getLocalConfigRev(node_id);
        if (rev <= localRev) {
          return;
        }
      }

      // Strip transient sync metadata before writing to local state.
      const cleaned: Record<string, unknown> = {};
      for (const [k, v] of Object.entries(p)) {
        if (!k.startsWith('_')) {
          cleaned[k] = v;
        }
      }

      writeNodeParams(node_id, cleaned, session_id);
    }
  }

  private handleNodeAdded(payload: NodeAddedPayload): void {
    const { session_id, node_id, kind, params } = payload;
    useSessionStore
      .getState()
      .addNode(session_id, node_id, { kind, params, state: 'Initializing' });
  }

  private handleNodeRemoved(payload: NodeRemovedPayload): void {
    const { session_id, node_id } = payload;
    useSessionStore.getState().removeNode(session_id, node_id);
  }

  private handleConnectionAdded(payload: ConnectionAddedPayload): void {
    const { session_id, from_node, from_pin, to_node, to_pin } = payload;
    useSessionStore.getState().addConnection(session_id, { from_node, from_pin, to_node, to_pin });
  }

  private handleConnectionRemoved(payload: ConnectionRemovedPayload): void {
    const { session_id, from_node, from_pin, to_node, to_pin } = payload;
    useSessionStore
      .getState()
      .removeConnection(session_id, { from_node, from_pin, to_node, to_pin });
  }

  private handleNodeViewDataUpdated(payload: NodeViewDataUpdatedPayload): void {
    const { session_id, node_id, data } = payload;
    writeNodeViewData(session_id, node_id, data);
    // Keep Zustand write for consumers not yet migrated to Jotai atoms.
    useSessionStore.getState().updateNodeViewData(session_id, node_id, data);
  }

  private handleRuntimeSchemasUpdated(payload: RuntimeSchemasUpdatedPayload): void {
    const { session_id, node_id, schema } = payload;
    useSessionStore.getState().updateRuntimeSchema(session_id, node_id, schema);
  }

  private handleNodeTelemetry(payload: NodeTelemetryPayload): void {
    const telemetryEvent = parseTelemetryEvent({
      session_id: payload.session_id,
      node_id: payload.node_id,
      type_id: payload.type_id,
      data: payload.data ?? {},
      timestamp_us: payload.timestamp_us != null ? Number(payload.timestamp_us) : undefined,
      timestamp: payload.timestamp,
    });
    useTelemetryStore.getState().addEvent(telemetryEvent);
  }

  send(request: Request): Promise<Response> {
    return new Promise((resolve, reject) => {
      const correlationId = request.correlation_id || uuidv4();
      const requestWithId = { ...request, correlation_id: correlationId };

      const timeout = window.setTimeout(() => {
        this.pendingRequests.delete(correlationId);
        reject(new Error('Request timeout'));
      }, 5000); // 5 second timeout

      this.pendingRequests.set(correlationId, { resolve, reject, timeout });

      if (this.ws?.readyState === WebSocket.OPEN) {
        try {
          this.ws.send(JSON.stringify(requestWithId));
        } catch (error) {
          clearTimeout(timeout);
          this.pendingRequests.delete(correlationId);
          reject(error);
        }
      } else {
        // Queue message if not connected
        this.messageQueue.push(requestWithId);
        logger.debug('Message queued (not connected), readyState:', this.ws?.readyState);
      }
    });
  }

  sendFireAndForget(request: Request): void {
    const requestWithId = { ...request, correlation_id: request.correlation_id || uuidv4() };

    if (this.ws?.readyState === WebSocket.OPEN) {
      try {
        this.ws.send(JSON.stringify(requestWithId));
      } catch (error) {
        logger.error('Failed to send fire-and-forget message:', error);
      }
    } else {
      // Queue message if not connected
      this.messageQueue.push(requestWithId);
      logger.debug(
        'Fire-and-forget message queued (not connected), readyState:',
        this.ws?.readyState
      );
    }
  }

  private flushMessageQueue(): void {
    if (this.ws?.readyState !== WebSocket.OPEN) {
      return;
    }

    logger.debug(`Flushing ${this.messageQueue.length} queued messages`);

    while (this.messageQueue.length > 0) {
      const message = this.messageQueue.shift();
      if (message) {
        try {
          this.ws.send(JSON.stringify(message));
        } catch (error) {
          logger.error('Failed to send queued message:', error);
        }
      }
    }
  }

  subscribeToSession(sessionId: string): void {
    this.subscribedSessions.add(sessionId);
    // Ensure the session store entry exists before updating connection status.
    const isConnected = this.ws?.readyState === WebSocket.OPEN;
    logger.debug(
      'Subscribing to session',
      sessionId,
      'ws readyState:',
      this.ws?.readyState,
      'connected:',
      isConnected
    );
    writeSessionConnected(sessionId, isConnected);
    useSessionStore.getState().initSession(sessionId, isConnected);

    // Fetch the initial pipeline state over the same WS connection.
    // This avoids the HTTP/WS race where REST returns stale data that
    // the WS event stream has already superseded.
    this.fetchPipeline(sessionId);
  }

  unsubscribeFromSession(sessionId: string): void {
    this.subscribedSessions.delete(sessionId);
    // Keep the session entry so the Monitor session list can display the latest known status
    // even when a session is not actively selected/subscribed.
    writeSessionConnected(sessionId, false);
    resetSessionParams(sessionId);
    useSessionStore.getState().setConnected(sessionId, false);
  }

  onMessage(handler: MessageHandler): () => void {
    this.messageHandlers.add(handler);
    return () => {
      this.messageHandlers.delete(handler);
    };
  }

  onConnectionStatus(handler: ConnectionStatusHandler): () => void {
    this.connectionStatusHandlers.add(handler);
    // Immediately notify of current status
    const currentStatus = this.ws?.readyState === WebSocket.OPEN;
    logger.debug(
      'onConnectionStatus registered, current readyState:',
      this.ws?.readyState,
      'status:',
      currentStatus
    );
    handler(currentStatus);
    return () => {
      this.connectionStatusHandlers.delete(handler);
    };
  }

  private notifyConnectionStatus(connected: boolean): void {
    logger.debug('Connection status changed:', connected, 'readyState:', this.ws?.readyState);

    this.subscribedSessions.forEach((sessionId) => {
      logger.debug('Updating connection status for session', sessionId, ':', connected);
      writeSessionConnected(sessionId, connected);
      useSessionStore.getState().setConnected(sessionId, connected);
    });

    this.connectionStatusHandlers.forEach((handler) => {
      try {
        handler(connected);
      } catch (error) {
        logger.error('Connection status handler error:', error);
      }
    });
  }

  close(): void {
    this.isIntentionallyClosed = true;

    if (this.reconnectTimeout) {
      clearTimeout(this.reconnectTimeout);
      this.reconnectTimeout = null;
    }

    // Clear all pending requests
    this.pendingRequests.forEach((pending) => {
      clearTimeout(pending.timeout);
      pending.reject(new Error('WebSocket closed'));
    });
    this.pendingRequests.clear();

    // Cancel any pending RAF and clear batch buffers.
    if (this.batchFlushRafId !== null) {
      cancelAnimationFrame(this.batchFlushRafId);
      this.batchFlushRafId = null;
    }
    this.pendingNodeStates.clear();
    this.pendingNodeStats.clear();

    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }

    this.subscribedSessions.clear();
    this.messageHandlers.clear();
    this.connectionStatusHandlers.clear();
  }

  isConnected(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }

  /** Return the current sender nonce for this WebSocket session.
   *  Used by the config revision system to stamp outgoing params with
   *  `_sender` so the client can identify its own echoes. */
  getClientNonce(): string {
    return this.clientNonce;
  }
}

// Singleton instance
let wsInstance: WebSocketService | null = null;

export function getWebSocketService(): WebSocketService {
  if (!wsInstance) {
    // In development, Vite replaces this with the value from the config.
    // In production, it will be undefined and the fallback logic will be used.
    const devWsUrl = import.meta.env.VITE_WS_URL;

    const wsUrl =
      (devWsUrl
        ? (() => {
            // Keep cookie-based auth working in dev when mixing localhost and 127.0.0.1.
            // Cookies are keyed by hostname; if the UI is on localhost but the WS URL uses
            // 127.0.0.1 (or vice-versa), the session cookie won't be sent.
            try {
              const url = new URL(devWsUrl);
              const isLoopback = (host: string) => host === 'localhost' || host === '127.0.0.1';
              if (isLoopback(url.hostname) && isLoopback(window.location.hostname)) {
                url.hostname = window.location.hostname;
                return url.toString();
              }
            } catch {
              // Ignore and fall back to the raw value.
            }
            return devWsUrl;
          })()
        : undefined) ||
      (() => {
        // Fallback for production: check for <base> tag to handle subpath deployments
        const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
        const host = window.location.host;
        const basePathname = getBasePathname();
        if (basePathname) return `${protocol}//${host}${basePathname}/api/v1/control`;

        // No base tag - root deployment
        return `${protocol}//${host}/api/v1/control`;
      })();

    logger.info('Creating singleton instance with URL:', wsUrl);
    wsInstance = new WebSocketService(wsUrl);
    wsInstance.connect();
  }
  return wsInstance;
}
