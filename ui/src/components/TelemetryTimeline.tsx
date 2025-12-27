// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React, { useMemo } from 'react';
import { useShallow } from 'zustand/shallow';

import { useTelemetryStore, type TelemetryEvent } from '@/stores/telemetryStore';

const TimelineContainer = styled.div`
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--sk-panel-bg);
  border-top: 1px solid var(--sk-border);
  overflow: hidden;
`;

const TimelineHeader = styled.div`
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 12px;
  border-bottom: 1px solid var(--sk-border);
  flex-shrink: 0;
`;

const TimelineTitle = styled.h3`
  margin: 0;
  font-size: 13px;
  font-weight: 600;
  color: var(--sk-text);
`;

const EventCount = styled.span`
  font-size: 11px;
  color: var(--sk-text-muted);
  background: var(--sk-bg);
  padding: 2px 8px;
  border-radius: 10px;
`;

const EventList = styled.div`
  flex: 1;
  overflow-y: auto;
  padding: 8px;
  display: flex;
  flex-direction: column;
  gap: 4px;
`;

const EventCard = styled.div<{ eventType: string }>`
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 8px 10px;
  background: var(--sk-bg);
  border-radius: 6px;
  border-left: 3px solid ${(props) => getEventColor(props.eventType)};
  font-size: 12px;

  &:hover {
    background: var(--sk-hover-bg);
  }
`;

const EventHeader = styled.div`
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
`;

const EventType = styled.span`
  font-weight: 600;
  color: var(--sk-text);
`;

const EventTime = styled.span`
  font-size: 11px;
  color: var(--sk-text-muted);
  font-family: monospace;
`;

const EventMeta = styled.div`
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
`;

const MetaBadge = styled.span<{ variant?: 'default' | 'latency' | 'node' }>`
  font-size: 10px;
  padding: 1px 6px;
  border-radius: 3px;
  background: ${(props) => {
    switch (props.variant) {
      case 'latency':
        return 'var(--sk-warning-bg, rgba(234, 179, 8, 0.15))';
      case 'node':
        return 'var(--sk-info-bg, rgba(59, 130, 246, 0.15))';
      default:
        return 'var(--sk-bg)';
    }
  }};
  color: ${(props) => {
    switch (props.variant) {
      case 'latency':
        return 'var(--sk-warning, #eab308)';
      case 'node':
        return 'var(--sk-info, #3b82f6)';
      default:
        return 'var(--sk-text-muted)';
    }
  }};
  border: 1px solid currentColor;
`;

const EventPreview = styled.div`
  font-size: 11px;
  color: var(--sk-text-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 100%;
`;

const EmptyState = styled.div`
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  height: 100%;
  color: var(--sk-text-muted);
  font-size: 13px;
  gap: 8px;
  padding: 20px;
  text-align: center;
`;

const EmptyIcon = styled.span`
  font-size: 32px;
  opacity: 0.5;
`;

// Event type to color mapping
function getEventColor(eventType: string): string {
  if (eventType.startsWith('vad.')) return '#10b981'; // Green for VAD
  if (eventType.startsWith('stt.')) return '#3b82f6'; // Blue for STT
  if (eventType.startsWith('llm.')) return '#8b5cf6'; // Purple for LLM
  if (eventType.startsWith('tts.')) return '#f59e0b'; // Orange for TTS
  if (eventType.startsWith('audio.')) return '#06b6d4'; // Cyan for audio
  if (eventType.startsWith('text.')) return '#ec4899'; // Pink for text
  return '#6b7280'; // Gray for unknown
}

// Format timestamp for display
function formatEventTime(timestamp: string): string {
  try {
    const date = new Date(timestamp);
    return date.toLocaleTimeString('en-US', {
      hour12: false,
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
      fractionalSecondDigits: 3,
    });
  } catch {
    return timestamp;
  }
}

// Get a preview of the event data
function getEventPreview(event: TelemetryEvent): string | null {
  const { data } = event;

  // Text preview for STT results
  if (data.text_preview) {
    return String(data.text_preview);
  }

  // Segment count for STT
  if (data.segment_count !== undefined) {
    return `${data.segment_count} segment(s)`;
  }

  // VAD events
  if (event.eventType === 'vad.start' || event.eventType === 'vad.speech_start') {
    return 'Speech started';
  }
  if (event.eventType === 'vad.end' || event.eventType === 'vad.speech_end') {
    return 'Speech ended';
  }

  // Audio levels
  if (data.rms !== undefined) {
    const rms = Number(data.rms);
    const peak = Number(data.peak ?? 0);
    return `RMS: ${rms.toFixed(3)}, Peak: ${peak.toFixed(3)}`;
  }

  // LLM events
  if (data.model) {
    return `Model: ${data.model}`;
  }

  return null;
}

interface TelemetryTimelineProps {
  sessionId: string;
  maxHeight?: number | string;
}

export const TelemetryTimeline: React.FC<TelemetryTimelineProps> = ({ sessionId, maxHeight }) => {
  const events = useTelemetryStore(
    useShallow((state) => state.sessions.get(sessionId)?.events ?? [])
  );

  // Reverse to show newest first
  const reversedEvents = useMemo(() => [...events].reverse(), [events]);

  return (
    <TimelineContainer style={{ maxHeight }}>
      <TimelineHeader>
        <TimelineTitle>Telemetry</TimelineTitle>
        <EventCount>{events.length} events</EventCount>
      </TimelineHeader>

      <EventList>
        {reversedEvents.length === 0 ? (
          <EmptyState>
            <EmptyIcon>📡</EmptyIcon>
            <div>No telemetry events yet</div>
            <div style={{ fontSize: '11px' }}>
              Events from `core::telemetry_out`, `core::telemetry_tap`, and script spans will appear
              here
            </div>
          </EmptyState>
        ) : (
          reversedEvents.map((event) => <TelemetryEventCard key={event.id} event={event} />)
        )}
      </EventList>
    </TimelineContainer>
  );
};

interface TelemetryEventCardProps {
  event: TelemetryEvent;
}

const TelemetryEventCard: React.FC<TelemetryEventCardProps> = React.memo(({ event }) => {
  const preview = getEventPreview(event);

  return (
    <EventCard eventType={event.eventType}>
      <EventHeader>
        <EventType>{event.eventType}</EventType>
        <EventTime>{formatEventTime(event.timestamp)}</EventTime>
      </EventHeader>

      <EventMeta>
        <MetaBadge variant="node">{event.nodeId}</MetaBadge>
        {event.latencyMs !== undefined && (
          <MetaBadge variant="latency">{event.latencyMs}ms</MetaBadge>
        )}
        {event.turnId && <MetaBadge>turn: {event.turnId.slice(0, 8)}</MetaBadge>}
      </EventMeta>

      {preview && <EventPreview title={preview}>{preview}</EventPreview>}
    </EventCard>
  );
});

TelemetryEventCard.displayName = 'TelemetryEventCard';

export default TelemetryTimeline;
