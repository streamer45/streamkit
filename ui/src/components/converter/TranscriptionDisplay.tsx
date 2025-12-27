// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React, { useEffect, useState } from 'react';

import type { TranscriptionData, TranscriptionSegment } from '@/types/generated/api-types';
import { extractJsonValues } from '@/utils/jsonStream';
import { componentsLogger } from '@/utils/logger';

import { LoadingSpinner } from '../LoadingSpinner';

// Helper: Update transcription state with new data
function updateTranscriptionState(
  transcription: TranscriptionData,
  currentLanguage: string | null,
  setSegments: React.Dispatch<React.SetStateAction<TranscriptionSegment[]>>,
  setFullText: React.Dispatch<React.SetStateAction<string>>,
  setLanguage: React.Dispatch<React.SetStateAction<string | null>>,
  setIsLoading: React.Dispatch<React.SetStateAction<boolean>>
): void {
  // Update segments
  setSegments((prev) => [...prev, ...transcription.segments]);

  // Update full text (accumulate)
  setFullText((prev) => (prev ? `${prev} ${transcription.text}` : transcription.text));

  // Update language (use first one we see)
  if (transcription.language && !currentLanguage) {
    setLanguage(transcription.language);
  }

  setIsLoading(false);
}

// Helper: Process buffered lines
function processBufferedLines(
  lines: string[],
  parseTranscriptionLine: (line: string) => TranscriptionData | null,
  currentLanguage: string | null,
  setSegments: React.Dispatch<React.SetStateAction<TranscriptionSegment[]>>,
  setFullText: React.Dispatch<React.SetStateAction<string>>,
  setLanguage: React.Dispatch<React.SetStateAction<string | null>>,
  setIsLoading: React.Dispatch<React.SetStateAction<boolean>>
): void {
  for (const line of lines) {
    const transcription = parseTranscriptionLine(line);
    if (transcription) {
      updateTranscriptionState(
        transcription,
        currentLanguage,
        setSegments,
        setFullText,
        setLanguage,
        setIsLoading
      );
    }
  }
}

// Helper: Read and process transcription stream
async function readTranscriptionStream(
  reader: ReadableStreamDefaultReader<string>,
  parseTranscriptionLine: (line: string) => TranscriptionData | null,
  language: string | null,
  setSegments: React.Dispatch<React.SetStateAction<TranscriptionSegment[]>>,
  setFullText: React.Dispatch<React.SetStateAction<string>>,
  setLanguage: React.Dispatch<React.SetStateAction<string | null>>,
  setIsLoading: React.Dispatch<React.SetStateAction<boolean>>,
  onComplete?: () => void
): Promise<void> {
  let buffer = '';

  while (true) {
    const { done, value } = await reader.read();

    if (done) {
      componentsLogger.info('TranscriptionDisplay: Stream complete');
      setIsLoading(false);
      onComplete?.();
      break;
    }

    // Append to buffer and process complete JSON values
    buffer += value;
    const extracted = extractJsonValues(buffer);
    buffer = extracted.remainder;
    processBufferedLines(
      extracted.values,
      parseTranscriptionLine,
      language,
      setSegments,
      setFullText,
      setLanguage,
      setIsLoading
    );
  }

  // Process any remaining data in buffer
  if (buffer.trim()) {
    const extracted = extractJsonValues(buffer);
    processBufferedLines(
      extracted.values,
      parseTranscriptionLine,
      language,
      setSegments,
      setFullText,
      setLanguage,
      setIsLoading
    );

    if (extracted.remainder.trim()) {
      const transcription = parseTranscriptionLine(extracted.remainder);
      if (transcription) {
        updateTranscriptionState(
          transcription,
          language,
          setSegments,
          setFullText,
          setLanguage,
          setIsLoading
        );
      }
    }
  }
}

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

const LanguageBadge = styled.div`
  padding: 4px 12px;
  background: var(--sk-primary);
  color: var(--sk-primary-contrast);
  border-radius: 4px;
  font-size: 12px;
  font-weight: 600;
  text-transform: uppercase;
`;

const Section = styled.div`
  display: flex;
  flex-direction: column;
  gap: 8px;
`;

const SectionLabel = styled.div`
  font-size: 13px;
  font-weight: 600;
  color: var(--sk-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.5px;
`;

const FullTextArea = styled.div`
  padding: 16px;
  background: var(--sk-bg);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  color: var(--sk-text);
  line-height: 1.6;
  max-height: 200px;
  overflow-y: auto;
  white-space: pre-wrap;
  word-wrap: break-word;
  font-size: 14px;
`;

const SegmentsList = styled.div`
  display: flex;
  flex-direction: column;
  gap: 12px;
  max-height: 400px;
  overflow-y: auto;
  padding: 4px;
`;

const SegmentCard = styled.div`
  padding: 12px;
  background: var(--sk-bg);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  display: flex;
  flex-direction: column;
  gap: 8px;
`;

const SegmentHeader = styled.div`
  display: flex;
  align-items: center;
  gap: 8px;
`;

const TimestampBadge = styled.div`
  padding: 4px 8px;
  background: var(--sk-hover-bg);
  border: 1px solid var(--sk-border);
  border-radius: 4px;
  font-size: 11px;
  font-weight: 600;
  color: var(--sk-text-muted);
  font-family: 'Courier New', monospace;
`;

const ConfidenceBadge = styled.div`
  padding: 4px 8px;
  background: var(--sk-hover-bg);
  border: 1px solid var(--sk-border);
  border-radius: 4px;
  font-size: 11px;
  font-weight: 600;
  color: var(--sk-text-muted);
`;

const SegmentText = styled.div`
  color: var(--sk-text);
  line-height: 1.5;
  font-size: 14px;
`;

const LoadingContainer = styled.div`
  display: flex;
  align-items: center;
  justify-content: center;
  min-height: 200px;
`;

interface TranscriptionDisplayProps {
  stream: ReadableStream<Uint8Array>;
  // Add a unique key to force remount
  processingKey?: number;
  // Callback when stream processing is complete
  onComplete?: () => void;
  // Callback when stream is cancelled
  onCancel?: () => void;
}

/**
 * Format milliseconds as MM:SS.mmm
 */
function formatTimestamp(ms: bigint | number): string {
  // Convert bigint to number for calculations
  const msNum = typeof ms === 'bigint' ? Number(ms) : ms;
  const totalSeconds = Math.floor(msNum / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  const milliseconds = msNum % 1000;

  return `${minutes.toString().padStart(2, '0')}:${seconds.toString().padStart(2, '0')}.${milliseconds.toString().padStart(3, '0')}`;
}

/**
 * Format confidence score as percentage
 */
function formatConfidence(confidence: number): string {
  return `${(confidence * 100).toFixed(1)}%`;
}

/**
 * Parses a single line of NDJSON containing a Transcription packet
 */
function parseTranscriptionLine(line: string): TranscriptionData | null {
  if (!line.trim()) {
    return null;
  }

  try {
    const parsed = JSON.parse(line);
    // Handle the packet wrapper format: {"Transcription": {...}}
    if (parsed.Transcription) {
      return parsed.Transcription as TranscriptionData;
    }
    return null;
  } catch (e) {
    componentsLogger.error('Failed to parse line:', e, line);
    return null;
  }
}

export const TranscriptionDisplay: React.FC<TranscriptionDisplayProps> = ({
  stream,
  onComplete,
  onCancel,
}) => {
  const [segments, setSegments] = useState<TranscriptionSegment[]>([]);
  const [fullText, setFullText] = useState<string>('');
  const [language, setLanguage] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState<boolean>(true);

  useEffect(() => {
    componentsLogger.debug('TranscriptionDisplay: Effect running, mounting component');

    const processStream = async () => {
      let reader: ReadableStreamDefaultReader<string> | null = null;

      try {
        componentsLogger.debug('TranscriptionDisplay: Starting stream processing');

        // Check if stream is already locked (can happen in StrictMode)
        if (stream.locked) {
          componentsLogger.warn(
            'TranscriptionDisplay: Stream is already locked, skipping processing'
          );
          setIsLoading(false);
          return;
        }

        // Use type assertion for TextDecoderStream compatibility
        const textStream = stream.pipeThrough(
          new TextDecoderStream() as unknown as ReadableWritablePair<string, Uint8Array>
        );
        reader = textStream.getReader();

        await readTranscriptionStream(
          reader,
          parseTranscriptionLine,
          language,
          setSegments,
          setFullText,
          setLanguage,
          setIsLoading,
          onComplete
        );
      } catch (error) {
        // Handle AbortError gracefully (user cancelled)
        if (error instanceof Error && error.name === 'AbortError') {
          componentsLogger.info('TranscriptionDisplay: Stream aborted by user');
          onCancel?.();
        } else {
          componentsLogger.error('TranscriptionDisplay: Stream processing error:', error);
        }
        setIsLoading(false);
      } finally {
        // Always release the reader lock
        if (reader) {
          try {
            reader.releaseLock();
          } catch {
            // Ignore errors when releasing lock
          }
        }
      }
    };

    processStream();

    // Cleanup: we don't abort here to allow the stream to complete
    // The stream will naturally finish when done, and React will clean up the component
    // This is important for StrictMode which will mount/unmount/remount
    return () => {
      componentsLogger.debug(
        'TranscriptionDisplay: Cleanup called (not aborting stream to allow completion)'
      );
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Show loading spinner until we have content
  if (isLoading && segments.length === 0) {
    return (
      <LoadingContainer>
        <LoadingSpinner message="Processing audio and generating transcription..." />
      </LoadingContainer>
    );
  }

  return (
    <Container>
      <Header>
        <Title>Transcription Results</Title>
        {language && <LanguageBadge>{language}</LanguageBadge>}
      </Header>

      {fullText && (
        <Section>
          <SectionLabel>Full Transcription</SectionLabel>
          <FullTextArea>{fullText}</FullTextArea>
        </Section>
      )}

      {segments.length > 0 && (
        <Section>
          <SectionLabel>Timestamped Segments ({segments.length})</SectionLabel>
          <SegmentsList>
            {segments.map((segment) => (
              <SegmentCard key={`${segment.start_time_ms}-${segment.end_time_ms}`}>
                <SegmentHeader>
                  <TimestampBadge>
                    {formatTimestamp(segment.start_time_ms)} -{' '}
                    {formatTimestamp(segment.end_time_ms)}
                  </TimestampBadge>
                  {segment.confidence !== null && segment.confidence !== undefined && (
                    <ConfidenceBadge>
                      Confidence: {formatConfidence(segment.confidence)}
                    </ConfidenceBadge>
                  )}
                </SegmentHeader>
                <SegmentText>{segment.text}</SegmentText>
              </SegmentCard>
            ))}
          </SegmentsList>
        </Section>
      )}
    </Container>
  );
};
