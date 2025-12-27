// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { extractJsonValues } from '@/utils/jsonStream';
import { componentsLogger } from '@/utils/logger';

import { LoadingSpinner } from '../LoadingSpinner';

const Container = styled.div`
  display: flex;
  flex-direction: column;
  gap: 16px;
  padding: 24px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 8px;
`;

const Header = styled.div`
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
`;

const Title = styled.div`
  font-weight: 600;
  color: var(--sk-text);
  font-size: 16px;
`;

const CountBadge = styled.div`
  padding: 4px 10px;
  background: var(--sk-hover-bg);
  border: 1px solid var(--sk-border);
  border-radius: 999px;
  font-size: 12px;
  font-weight: 600;
  color: var(--sk-text-muted);
`;

const List = styled.div`
  display: flex;
  flex-direction: column;
  gap: 10px;
  max-height: 520px;
  overflow-y: auto;
  padding: 4px;
`;

const Item = styled.div`
  padding: 12px;
  background: var(--sk-bg);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
`;

const ItemSummary = styled.summary`
  cursor: pointer;
  display: flex;
  align-items: baseline;
  gap: 10px;
  color: var(--sk-text);
  font-size: 14px;

  &::marker {
    color: var(--sk-text-muted);
  }
`;

const ItemKind = styled.span`
  font-weight: 700;
  color: var(--sk-text);
  font-family:
    ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, 'Liberation Mono', 'Courier New',
    monospace;
`;

const ItemHint = styled.span`
  color: var(--sk-text-muted);
  font-size: 13px;
`;

const ItemBody = styled.pre`
  margin: 10px 0 0 0;
  padding: 12px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  color: var(--sk-text);
  white-space: pre-wrap;
  word-break: break-word;
  font-size: 12px;
  line-height: 1.45;
  font-family:
    ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, 'Liberation Mono', 'Courier New',
    monospace;
`;

const LoadingContainer = styled.div`
  display: flex;
  align-items: center;
  justify-content: center;
  min-height: 200px;
`;

type JsonStreamItem =
  | { kind: 'json'; value: unknown; raw: string }
  | { kind: 'parse_error'; error: string; raw: string };

type JsonStreamDisplayItem =
  | {
      id: number;
      label: string;
      hint?: string;
      body: string;
      kind: 'json';
      value: unknown;
      raw: string;
    }
  | {
      id: number;
      label: string;
      hint?: string;
      body: string;
      kind: 'parse_error';
      error: string;
      raw: string;
    };

function safePrettyPrint(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2);
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    return `Failed to serialize value: ${message}`;
  }
}

const JsonStreamRow = React.memo(({ item }: { item: JsonStreamDisplayItem }) => {
  return (
    <Item>
      <details>
        <ItemSummary>
          <ItemKind>{item.label}</ItemKind>
          {item.hint && <ItemHint>{item.hint}</ItemHint>}
        </ItemSummary>
        <ItemBody>{item.body}</ItemBody>
      </details>
    </Item>
  );
});
JsonStreamRow.displayName = 'JsonStreamRow';

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function parseNdjsonLine(line: string): JsonStreamItem | null {
  if (!line.trim()) return null;
  try {
    return { kind: 'json', value: JSON.parse(line), raw: line };
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    return { kind: 'parse_error', error: message, raw: line };
  }
}

function summarize(item: JsonStreamItem): { label: string; hint?: string } {
  if (item.kind === 'parse_error') {
    return { label: 'ParseError', hint: item.error };
  }

  const v = item.value;

  if (!isRecord(v)) {
    return { label: typeof v };
  }

  const custom = v.Custom;
  if (isRecord(custom)) {
    const typeId = typeof custom.type_id === 'string' ? custom.type_id : 'unknown';
    const data = custom.data;
    const eventType =
      isRecord(data) && typeof data.event_type === 'string' ? String(data.event_type) : null;
    const hint = eventType ? `${typeId} · ${eventType}` : typeId;
    return { label: 'Custom', hint };
  }

  const knownKinds = ['Transcription', 'Text', 'Audio', 'Binary'] as const;
  for (const kind of knownKinds) {
    if (kind in v) return { label: kind };
  }

  const keys = Object.keys(v);
  if (keys.length === 1) return { label: keys[0] };

  return { label: 'object' };
}

async function readJsonStream(
  reader: ReadableStreamDefaultReader<string>,
  onItem: (item: JsonStreamItem) => void,
  onComplete?: () => void
): Promise<void> {
  let buffer = '';

  while (true) {
    const { done, value } = await reader.read();
    if (done) {
      onComplete?.();
      break;
    }

    buffer += value;
    const extracted = extractJsonValues(buffer);
    buffer = extracted.remainder;
    for (const jsonText of extracted.values) {
      const parsed = parseNdjsonLine(jsonText);
      if (parsed) onItem(parsed);
    }
  }

  if (buffer.trim()) {
    // If we have trailing whitespace, ignore. Otherwise, surface parse errors.
    const extracted = extractJsonValues(buffer);
    for (const jsonText of extracted.values) {
      const parsed = parseNdjsonLine(jsonText);
      if (parsed) onItem(parsed);
    }
    if (extracted.remainder.trim()) {
      const parsed = parseNdjsonLine(extracted.remainder);
      if (parsed) onItem(parsed);
    }
  }
}

interface JsonStreamDisplayProps {
  stream: ReadableStream<Uint8Array>;
  title?: string;
  onComplete?: () => void;
  onCancel?: () => void;
}

export const JsonStreamDisplay: React.FC<JsonStreamDisplayProps> = ({
  stream,
  title = 'JSON Results',
  onComplete,
  onCancel,
}) => {
  const [items, setItems] = useState<JsonStreamDisplayItem[]>([]);
  const [isLoading, setIsLoading] = useState<boolean>(true);
  const nextIdRef = useRef(1);

  const countLabel = useMemo(() => `${items.length}`, [items.length]);

  const materializeItem = useCallback((item: JsonStreamItem): JsonStreamDisplayItem => {
    const id = nextIdRef.current++;
    const { label, hint } = summarize(item);
    const body =
      item.kind === 'json'
        ? safePrettyPrint(item.value)
        : `Failed to parse JSON: ${item.error}\n\n${item.raw}`;
    return item.kind === 'json'
      ? { id, label, hint, body, kind: 'json', value: item.value, raw: item.raw }
      : { id, label, hint, body, kind: 'parse_error', error: item.error, raw: item.raw };
  }, []);

  useEffect(() => {
    const processStream = async () => {
      let reader: ReadableStreamDefaultReader<string> | null = null;
      try {
        if (stream.locked) {
          componentsLogger.warn('JsonStreamDisplay: Stream is already locked, skipping processing');
          setIsLoading(false);
          return;
        }

        const textStream = stream.pipeThrough(
          new TextDecoderStream() as unknown as ReadableWritablePair<string, Uint8Array>
        );
        reader = textStream.getReader();

        await readJsonStream(
          reader,
          (item) => {
            const displayItem = materializeItem(item);
            setItems((prev) => {
              const next = [...prev, displayItem];
              return next.length > 500 ? next.slice(next.length - 500) : next;
            });
            setIsLoading(false);
          },
          () => {
            setIsLoading(false);
            onComplete?.();
          }
        );
      } catch (error) {
        if (error instanceof Error && error.name === 'AbortError') {
          componentsLogger.info('JsonStreamDisplay: Stream aborted by user');
          onCancel?.();
        } else {
          componentsLogger.error('JsonStreamDisplay: Stream processing error:', error);
        }
        setIsLoading(false);
      } finally {
        if (reader) {
          try {
            reader.releaseLock();
          } catch {
            // ignore
          }
        }
      }
    };

    processStream();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (isLoading && items.length === 0) {
    return (
      <LoadingContainer>
        <LoadingSpinner message="Processing and streaming JSON output..." />
      </LoadingContainer>
    );
  }

  return (
    <Container>
      <Header>
        <Title>{title}</Title>
        <CountBadge>{countLabel}</CountBadge>
      </Header>

      {items.length > 0 && (
        <List>
          {items.map((item) => (
            <JsonStreamRow key={item.id} item={item} />
          ))}
        </List>
      )}
    </Container>
  );
};
