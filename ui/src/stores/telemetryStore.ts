// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { create } from 'zustand';

/**
 * A telemetry event received from the backend.
 */
export interface TelemetryEvent {
  /** The session this event belongs to */
  sessionId: string;
  /** The node that emitted this event */
  nodeId: string;
  /** Packet type identifier (e.g., "core::telemetry/event@1") */
  typeId: string;
  /** Event type extracted from data (e.g., "stt.result", "vad.start") */
  eventType: string;
  /** Event payload */
  data: Record<string, unknown>;
  /** Timestamp in microseconds since UNIX epoch (if available) */
  timestampUs?: number;
  /** RFC 3339 formatted timestamp */
  timestamp: string;
  /** Correlation ID for grouping related events */
  correlationId?: string;
  /** Turn ID for voice agent conversation grouping */
  turnId?: string;
  /** Latency in milliseconds (for span events) */
  latencyMs?: number;
  /** Unique ID for this event instance (generated client-side) */
  id: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

interface TelemetrySessionData {
  /** Ring buffer of events (oldest first) */
  events: TelemetryEvent[];
  /** Maximum number of events to keep per session */
  maxEvents: number;
  /** Whether telemetry is enabled for this session */
  enabled: boolean;
}

interface TelemetryStore {
  /** Telemetry data by session ID */
  sessions: Map<string, TelemetrySessionData>;

  /** Default max events per session */
  defaultMaxEvents: number;

  // Actions
  addEvent: (event: TelemetryEvent) => void;
  clearSession: (sessionId: string) => void;
  setEnabled: (sessionId: string, enabled: boolean) => void;
  setMaxEvents: (sessionId: string, maxEvents: number) => void;
  getEvents: (sessionId: string) => TelemetryEvent[];
  getEventsByTurn: (sessionId: string, turnId: string) => TelemetryEvent[];
  getEventsByNode: (sessionId: string, nodeId: string) => TelemetryEvent[];
  getEventsByType: (sessionId: string, eventType: string) => TelemetryEvent[];
}

// Generate a unique event ID
let eventCounter = 0;
function generateEventId(): string {
  return `evt-${Date.now()}-${++eventCounter}`;
}

export const useTelemetryStore = create<TelemetryStore>((set, get) => ({
  sessions: new Map(),
  defaultMaxEvents: 100,

  addEvent: (event) =>
    set((prev) => {
      const sessionData = prev.sessions.get(event.sessionId);

      if (sessionData && !sessionData.enabled) {
        // Telemetry disabled for this session, skip
        return prev;
      }

      const maxEvents = sessionData?.maxEvents ?? prev.defaultMaxEvents;
      const currentEvents = sessionData?.events ?? [];

      // Add new event and trim to maxEvents (ring buffer behavior)
      const newEvents = [...currentEvents, event];
      if (newEvents.length > maxEvents) {
        newEvents.splice(0, newEvents.length - maxEvents);
      }

      const newSessions = new Map(prev.sessions);
      newSessions.set(event.sessionId, {
        events: newEvents,
        maxEvents,
        enabled: sessionData?.enabled ?? true,
      });

      return { sessions: newSessions };
    }),

  clearSession: (sessionId) =>
    set((prev) => {
      const newSessions = new Map(prev.sessions);
      newSessions.delete(sessionId);
      return { sessions: newSessions };
    }),

  setEnabled: (sessionId, enabled) =>
    set((prev) => {
      const sessionData = prev.sessions.get(sessionId);
      const newSessions = new Map(prev.sessions);
      newSessions.set(sessionId, {
        events: sessionData?.events ?? [],
        maxEvents: sessionData?.maxEvents ?? prev.defaultMaxEvents,
        enabled,
      });
      return { sessions: newSessions };
    }),

  setMaxEvents: (sessionId, maxEvents) =>
    set((prev) => {
      const sessionData = prev.sessions.get(sessionId);
      const currentEvents = sessionData?.events ?? [];

      // Trim events if necessary
      const trimmedEvents =
        currentEvents.length > maxEvents
          ? currentEvents.slice(currentEvents.length - maxEvents)
          : currentEvents;

      const newSessions = new Map(prev.sessions);
      newSessions.set(sessionId, {
        events: trimmedEvents,
        maxEvents,
        enabled: sessionData?.enabled ?? true,
      });
      return { sessions: newSessions };
    }),

  getEvents: (sessionId) => {
    const sessionData = get().sessions.get(sessionId);
    return sessionData?.events ?? [];
  },

  getEventsByTurn: (sessionId, turnId) => {
    const events = get().getEvents(sessionId);
    return events.filter((e) => e.turnId === turnId);
  },

  getEventsByNode: (sessionId, nodeId) => {
    const events = get().getEvents(sessionId);
    return events.filter((e) => e.nodeId === nodeId);
  },

  getEventsByType: (sessionId, eventType) => {
    const events = get().getEvents(sessionId);
    return events.filter((e) => e.eventType === eventType);
  },
}));

/**
 * Parse a raw WebSocket telemetry event payload into a TelemetryEvent.
 */
export function parseTelemetryEvent(payload: {
  session_id: string;
  node_id: string;
  type_id: string;
  data: unknown;
  timestamp_us?: number;
  timestamp: string;
}): TelemetryEvent {
  const rawData = payload.data;
  const data: Record<string, unknown> = isRecord(rawData) ? rawData : { value: rawData };
  const eventType = typeof data.event_type === 'string' ? data.event_type : 'unknown';
  const correlationId = typeof data.correlation_id === 'string' ? data.correlation_id : undefined;
  const turnId = typeof data.turn_id === 'string' ? data.turn_id : undefined;
  const latencyMs = typeof data.latency_ms === 'number' ? data.latency_ms : undefined;

  return {
    id: generateEventId(),
    sessionId: payload.session_id,
    nodeId: payload.node_id,
    typeId: payload.type_id,
    eventType,
    data,
    timestampUs: payload.timestamp_us,
    timestamp: payload.timestamp,
    correlationId,
    turnId,
    latencyMs,
  };
}
