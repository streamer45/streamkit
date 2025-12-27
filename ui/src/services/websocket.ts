// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { v4 as uuidv4 } from 'uuid';

import { useNodeParamsStore } from '@/stores/nodeParamsStore';
import { useSessionStore } from '@/stores/sessionStore';
import { useTelemetryStore, parseTelemetryEvent } from '@/stores/telemetryStore';
import type { Request, Response, Event, MessageType } from '@/types/types';
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
    // Re-subscribe to all sessions after reconnection
    this.subscribedSessions.forEach((sessionId) => {
      logger.info('Re-subscribing to session:', sessionId);
      this.send({
        type: 'request' as MessageType,
        correlation_id: uuidv4(),
        payload: {
          action: 'getpipeline' as const,
          session_id: sessionId,
        },
      });
    });
  }

  private handleMessage(message: Response | Event): void {
    // Handle responses with correlation_id
    if (message.type === 'response' && message.correlation_id) {
      const pending = this.pendingRequests.get(message.correlation_id);
      if (pending) {
        clearTimeout(pending.timeout);
        this.pendingRequests.delete(message.correlation_id);
        pending.resolve(message as Response);
        return;
      }
    }

    // Handle events
    if (message.type === 'event') {
      this.handleEvent(message as Event);
    }

    // Notify all message handlers
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
      default:
        break;
    }
  }

  private handleSessionDestroyed(payload: SessionDestroyedPayload): void {
    useTelemetryStore.getState().clearSession(payload.session_id);
  }

  private handleNodeStateChanged(payload: NodeStateChangedPayload): void {
    const { session_id, node_id, state } = payload;
    useSessionStore.getState().updateNodeState(session_id, node_id, state);
  }

  private handleNodeStatsUpdated(payload: NodeStatsUpdatedPayload): void {
    const { session_id, node_id, stats } = payload;
    useSessionStore.getState().updateNodeStats(session_id, node_id, stats);
  }

  private handleNodeParamsChanged(payload: NodeParamsChangedPayload): void {
    const { session_id, node_id, params } = payload;

    // Update session store for pipeline view
    // WARNING: This is problematic because it causes re-renders that cause issues with react flow.
    //
    // useSessionStore.getState().updateNodeParams(session_id, node_id, params as Record<string, unknown>);

    // Also update the params store used by individual node UIs
    if (params && typeof params === 'object' && !Array.isArray(params)) {
      for (const [key, value] of Object.entries(params)) {
        useNodeParamsStore.getState().setParam(node_id, key, value, session_id);
      }
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

      // Set up timeout for request
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
    // Set the connection status based on CURRENT WebSocket state
    const isConnected = this.ws?.readyState === WebSocket.OPEN;
    logger.debug(
      'Subscribing to session',
      sessionId,
      'ws readyState:',
      this.ws?.readyState,
      'connected:',
      isConnected
    );
    useSessionStore.getState().setConnected(sessionId, isConnected);
  }

  unsubscribeFromSession(sessionId: string): void {
    this.subscribedSessions.delete(sessionId);
    useSessionStore.getState().clearSession(sessionId);
    useNodeParamsStore.getState().resetSession(sessionId);
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

    // Update all subscribed sessions
    this.subscribedSessions.forEach((sessionId) => {
      logger.debug('Updating connection status for session', sessionId, ':', connected);
      useSessionStore.getState().setConnected(sessionId, connected);
    });

    // Notify handlers
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
}

// Singleton instance
let wsInstance: WebSocketService | null = null;

export function getWebSocketService(): WebSocketService {
  if (!wsInstance) {
    // In development, Vite replaces this with the value from the config.
    // In production, it will be undefined and the fallback logic will be used.
    const devWsUrl = import.meta.env.VITE_WS_URL;

    const wsUrl =
      devWsUrl ||
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
