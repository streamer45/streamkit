// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { beforeEach, describe, expect, it } from 'vitest';

import { parseTelemetryEvent, useTelemetryStore, type TelemetryEvent } from './telemetryStore';

/** Factory helper to reduce boilerplate in telemetry-event tests. */
function makeEvent(overrides: Partial<TelemetryEvent> = {}): TelemetryEvent {
  return {
    id: 'evt-test-1',
    sessionId: 's1',
    nodeId: 'n1',
    typeId: 'core::telemetry/event@1',
    eventType: 'generic',
    data: {},
    timestamp: '2025-01-01T00:00:00Z',
    ...overrides,
  };
}

beforeEach(() => {
  useTelemetryStore.setState({ sessions: new Map(), defaultMaxEvents: 100 });
});

// ---------------------------------------------------------------------------
// parseTelemetryEvent
// ---------------------------------------------------------------------------

describe('parseTelemetryEvent', () => {
  const basePayload = {
    session_id: 's1',
    node_id: 'n1',
    type_id: 'core::telemetry/event@1',
    timestamp: '2025-01-01T00:00:00Z',
  };

  it('wraps non-record string data as { value: data }', () => {
    const parsed = parseTelemetryEvent({ ...basePayload, data: 'hello' });
    expect(parsed.data).toEqual({ value: 'hello' });
    expect(parsed.eventType).toBe('unknown');
  });

  it('wraps non-record number data as { value: data }', () => {
    const parsed = parseTelemetryEvent({ ...basePayload, data: 42 });
    expect(parsed.data).toEqual({ value: 42 });
  });

  it('wraps null data as { value: null }', () => {
    const parsed = parseTelemetryEvent({ ...basePayload, data: null });
    expect(parsed.data).toEqual({ value: null });
  });

  it('preserves record data and surfaces string event_type', () => {
    const parsed = parseTelemetryEvent({
      ...basePayload,
      data: { event_type: 'stt.result', text: 'hello world' },
    });
    expect(parsed.eventType).toBe('stt.result');
    expect(parsed.data).toEqual({ event_type: 'stt.result', text: 'hello world' });
  });

  it('falls back to "unknown" when event_type has wrong type', () => {
    const parsed = parseTelemetryEvent({
      ...basePayload,
      data: { event_type: 42 },
    });
    expect(parsed.eventType).toBe('unknown');
  });

  it('surfaces correlation_id when it is a string', () => {
    const parsed = parseTelemetryEvent({
      ...basePayload,
      data: { correlation_id: 'corr-1' },
    });
    expect(parsed.correlationId).toBe('corr-1');
  });

  it('ignores correlation_id when wrong type', () => {
    const parsed = parseTelemetryEvent({
      ...basePayload,
      data: { correlation_id: 123 },
    });
    expect(parsed.correlationId).toBeUndefined();
  });

  it('surfaces turn_id when it is a string', () => {
    const parsed = parseTelemetryEvent({
      ...basePayload,
      data: { turn_id: 'turn-1' },
    });
    expect(parsed.turnId).toBe('turn-1');
  });

  it('ignores turn_id when wrong type', () => {
    const parsed = parseTelemetryEvent({
      ...basePayload,
      data: { turn_id: { nested: 'no' } },
    });
    expect(parsed.turnId).toBeUndefined();
  });

  it('surfaces latency_ms when it is a number', () => {
    const parsed = parseTelemetryEvent({
      ...basePayload,
      data: { latency_ms: 12.5 },
    });
    expect(parsed.latencyMs).toBe(12.5);
  });

  it('ignores latency_ms when wrong type', () => {
    const parsed = parseTelemetryEvent({
      ...basePayload,
      data: { latency_ms: '12' },
    });
    expect(parsed.latencyMs).toBeUndefined();
  });

  it('produces a unique id per call', () => {
    const ids = Array.from(
      { length: 10 },
      () => parseTelemetryEvent({ ...basePayload, data: {} }).id
    );
    expect(new Set(ids).size).toBe(10);
  });

  it('passes sessionId, nodeId, typeId, timestamp, timestampUs through verbatim', () => {
    const parsed = parseTelemetryEvent({
      session_id: 'sess-x',
      node_id: 'node-y',
      type_id: 'core::telemetry/event@2',
      timestamp: '2026-05-16T09:44:00Z',
      timestamp_us: 1_700_000_000_000_123,
      data: {},
    });
    expect(parsed.sessionId).toBe('sess-x');
    expect(parsed.nodeId).toBe('node-y');
    expect(parsed.typeId).toBe('core::telemetry/event@2');
    expect(parsed.timestamp).toBe('2026-05-16T09:44:00Z');
    expect(parsed.timestampUs).toBe(1_700_000_000_000_123);
  });

  it('treats arrays as non-record (wraps under value)', () => {
    const parsed = parseTelemetryEvent({ ...basePayload, data: [1, 2, 3] });
    expect(parsed.data).toEqual({ value: [1, 2, 3] });
    expect(parsed.eventType).toBe('unknown');
  });
});

// ---------------------------------------------------------------------------
// useTelemetryStore.addEvent
// ---------------------------------------------------------------------------

describe('useTelemetryStore.addEvent', () => {
  it('creates a fresh session entry with defaults when none exists', () => {
    const event = makeEvent({ sessionId: 's1' });
    useTelemetryStore.getState().addEvent(event);

    const session = useTelemetryStore.getState().sessions.get('s1');
    expect(session).toBeDefined();
    expect(session?.events).toEqual([event]);
    expect(session?.maxEvents).toBe(100);
    expect(session?.enabled).toBe(true);
  });

  it('does nothing when the session is disabled', () => {
    const { setEnabled, addEvent } = useTelemetryStore.getState();
    setEnabled('s1', false);
    const before = useTelemetryStore.getState().sessions.get('s1');

    addEvent(makeEvent({ sessionId: 's1' }));

    const after = useTelemetryStore.getState().sessions.get('s1');
    expect(after?.events).toEqual([]);
    expect(after?.enabled).toBe(false);
    // Same reference signals no update fired.
    expect(after).toBe(before);
  });

  it('trims to last maxEvents in arrival order (ring buffer)', () => {
    const { setMaxEvents, addEvent, getEvents } = useTelemetryStore.getState();
    setMaxEvents('s1', 3);

    for (let i = 0; i < 5; i++) {
      addEvent(makeEvent({ sessionId: 's1', id: `evt-${i}` }));
    }

    const events = getEvents('s1');
    expect(events.map((e) => e.id)).toEqual(['evt-2', 'evt-3', 'evt-4']);
  });

  it('uses defaultMaxEvents when no per-session limit is set', () => {
    useTelemetryStore.setState({ sessions: new Map(), defaultMaxEvents: 2 });
    const { addEvent, getEvents } = useTelemetryStore.getState();

    addEvent(makeEvent({ sessionId: 's1', id: 'a' }));
    addEvent(makeEvent({ sessionId: 's1', id: 'b' }));
    addEvent(makeEvent({ sessionId: 's1', id: 'c' }));

    expect(getEvents('s1').map((e) => e.id)).toEqual(['b', 'c']);
  });
});

// ---------------------------------------------------------------------------
// useTelemetryStore.clearSession / setEnabled / setMaxEvents
// ---------------------------------------------------------------------------

describe('useTelemetryStore.clearSession', () => {
  it('removes the entire session entry', () => {
    const { addEvent, clearSession } = useTelemetryStore.getState();
    addEvent(makeEvent({ sessionId: 's1' }));
    expect(useTelemetryStore.getState().sessions.has('s1')).toBe(true);

    clearSession('s1');
    expect(useTelemetryStore.getState().sessions.has('s1')).toBe(false);
  });
});

describe('useTelemetryStore.setEnabled', () => {
  it('flips the flag without clearing existing events', () => {
    const { addEvent, setEnabled } = useTelemetryStore.getState();
    addEvent(makeEvent({ sessionId: 's1', id: 'e1' }));
    addEvent(makeEvent({ sessionId: 's1', id: 'e2' }));

    setEnabled('s1', false);

    const session = useTelemetryStore.getState().sessions.get('s1');
    expect(session?.enabled).toBe(false);
    expect(session?.events.map((e) => e.id)).toEqual(['e1', 'e2']);
  });

  it('creates a session entry with defaults if it does not yet exist', () => {
    useTelemetryStore.getState().setEnabled('s-new', false);

    const session = useTelemetryStore.getState().sessions.get('s-new');
    expect(session).toEqual({ events: [], maxEvents: 100, enabled: false });
  });
});

describe('useTelemetryStore.setMaxEvents', () => {
  it('trims existing events to the last n when events.length > n', () => {
    const { addEvent, setMaxEvents, getEvents } = useTelemetryStore.getState();
    for (let i = 0; i < 5; i++) {
      addEvent(makeEvent({ sessionId: 's1', id: `evt-${i}` }));
    }

    setMaxEvents('s1', 2);

    expect(getEvents('s1').map((e) => e.id)).toEqual(['evt-3', 'evt-4']);
    expect(useTelemetryStore.getState().sessions.get('s1')?.maxEvents).toBe(2);
  });

  it('leaves events untouched when length <= n', () => {
    const { addEvent, setMaxEvents, getEvents } = useTelemetryStore.getState();
    addEvent(makeEvent({ sessionId: 's1', id: 'a' }));
    addEvent(makeEvent({ sessionId: 's1', id: 'b' }));

    setMaxEvents('s1', 5);

    expect(getEvents('s1').map((e) => e.id)).toEqual(['a', 'b']);
    expect(useTelemetryStore.getState().sessions.get('s1')?.maxEvents).toBe(5);
  });
});

// ---------------------------------------------------------------------------
// Query helpers
// ---------------------------------------------------------------------------

describe('useTelemetryStore query helpers', () => {
  it('getEvents returns empty array for missing session', () => {
    expect(useTelemetryStore.getState().getEvents('missing')).toEqual([]);
  });

  it('getEventsByTurn filters by turnId', () => {
    const { addEvent, getEventsByTurn } = useTelemetryStore.getState();
    addEvent(makeEvent({ sessionId: 's1', id: 'a', turnId: 'turn-1' }));
    addEvent(makeEvent({ sessionId: 's1', id: 'b', turnId: 'turn-2' }));
    addEvent(makeEvent({ sessionId: 's1', id: 'c', turnId: 'turn-1' }));
    addEvent(makeEvent({ sessionId: 's1', id: 'd' }));

    expect(getEventsByTurn('s1', 'turn-1').map((e) => e.id)).toEqual(['a', 'c']);
  });

  it('getEventsByNode filters by nodeId', () => {
    const { addEvent, getEventsByNode } = useTelemetryStore.getState();
    addEvent(makeEvent({ sessionId: 's1', id: 'a', nodeId: 'n-1' }));
    addEvent(makeEvent({ sessionId: 's1', id: 'b', nodeId: 'n-2' }));
    addEvent(makeEvent({ sessionId: 's1', id: 'c', nodeId: 'n-1' }));

    expect(getEventsByNode('s1', 'n-1').map((e) => e.id)).toEqual(['a', 'c']);
  });

  it('getEventsByType filters by eventType', () => {
    const { addEvent, getEventsByType } = useTelemetryStore.getState();
    addEvent(makeEvent({ sessionId: 's1', id: 'a', eventType: 'stt.result' }));
    addEvent(makeEvent({ sessionId: 's1', id: 'b', eventType: 'vad.start' }));
    addEvent(makeEvent({ sessionId: 's1', id: 'c', eventType: 'stt.result' }));

    expect(getEventsByType('s1', 'stt.result').map((e) => e.id)).toEqual(['a', 'c']);
  });

  it('query helpers return empty array for sessions with no matching events', () => {
    const { addEvent, getEventsByTurn, getEventsByNode, getEventsByType } =
      useTelemetryStore.getState();
    addEvent(makeEvent({ sessionId: 's1', id: 'a', turnId: 'turn-1' }));

    expect(getEventsByTurn('s1', 'turn-missing')).toEqual([]);
    expect(getEventsByNode('s1', 'node-missing')).toEqual([]);
    expect(getEventsByType('s1', 'type-missing')).toEqual([]);
  });
});
