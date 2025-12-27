// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React, { useEffect, useRef, useState } from 'react';

import { componentsLogger } from '@/utils/logger';
import { normalizeMimeType } from '@/utils/mse';

import { CustomAudioPlayer } from './CustomAudioPlayer';
import { LoadingSpinner } from './LoadingSpinner';

const PlayerContainer = styled.div`
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 16px;
  background: var(--surface-secondary);
  border-radius: 8px;
  border: 1px solid var(--border-primary);
`;

const LoadingOverlay = styled.div`
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--sk-panel-bg);
  border-radius: 8px;
  z-index: 10;
`;

const AudioElement = styled.audio`
  display: none;
`;

const StatusText = styled.div`
  font-size: 12px;
  color: var(--text-secondary);
  font-family: var(--font-mono);
`;

const ErrorText = styled.div`
  color: var(--error);
  font-size: 13px;
`;

interface MSEAudioPlayerProps {
  /** The ReadableStream from the fetch response */
  stream: ReadableStream<Uint8Array>;
  /** Content type (e.g., 'audio/webm; codecs="opus"') */
  contentType: string;
  /** Optional class name */
  className?: string;
  /** Callback when stream processing is complete */
  onComplete?: () => void;
  /** Callback when stream is cancelled */
  onCancel?: () => void;
  /** Callback when MSE playback fails (so the caller can provide a fallback) */
  onError?: (message: string) => void;
}

// Helper: Create error handler for audio element
function createAudioErrorHandler(
  audio: HTMLAudioElement,
  setError: (msg: string) => void,
  onAbort: () => void,
  reader: ReadableStreamDefaultReader<Uint8Array>
): () => void {
  return () => {
    if (audio.error) {
      // Ignore "Empty src attribute" error - this is expected during cleanup
      if (audio.error.message?.includes('Empty src attribute')) {
        componentsLogger.debug('MSEAudioPlayer: Ignoring empty src error during cleanup');
        return;
      }

      const errorMsg = `Audio error: ${audio.error.message || 'Unknown media error'}`;
      componentsLogger.error('MSEAudioPlayer:', errorMsg, audio.error);
      setError(errorMsg);
      onAbort();
      reader.cancel();
    }
  };
}

// Helper: Check if audio has a real error (not cleanup error)
function hasRealAudioError(audio: HTMLAudioElement): boolean {
  return !!audio.error && !audio.error.message?.includes('Empty src attribute');
}

// Helper: Try to start playback
function tryAutoplay(audio: HTMLAudioElement): void {
  if (audio.paused) {
    audio.play().catch((err) => {
      componentsLogger.warn('Autoplay failed, user interaction may be required:', err);
    });
  }
}

// Helper: Check if error is a cancellation
function isCancellationError(err: unknown): boolean {
  const isCancellation =
    err instanceof TypeError && (err.message.includes('cancel') || err.message.includes('Cancel'));
  const isAbortError = err instanceof Error && err.name === 'AbortError';
  return isCancellation || isAbortError;
}

// Helper: Handle stream completion
function handleStreamCompletion(
  totalBytes: number,
  mediaSource: MediaSource,
  setStatus: (status: string) => void,
  onComplete?: () => void
): void {
  setStatus(`Completed (${(totalBytes / 1024).toFixed(1)} KB)`);
  // Signal end of stream
  if (mediaSource.readyState === 'open') {
    mediaSource.endOfStream();
  }
  // Call completion callback
  onComplete?.();
}

// Helper: Process stream chunk
async function processStreamChunk(value: Uint8Array, sourceBuffer: SourceBuffer): Promise<void> {
  // Create a new Uint8Array to ensure it's backed by ArrayBuffer (not SharedArrayBuffer)
  const buffer = new Uint8Array(value);
  sourceBuffer.appendBuffer(buffer);

  // Wait for the buffer to finish updating before appending more
  await new Promise<void>((resolve) => {
    sourceBuffer.addEventListener('updateend', () => resolve(), { once: true });
  });
}

// Helper: Stream reading loop
async function streamAudioData(
  reader: ReadableStreamDefaultReader<Uint8Array>,
  sourceBuffer: SourceBuffer,
  audio: HTMLAudioElement,
  mediaSource: MediaSource,
  setStatus: (status: string) => void,
  onComplete: (() => void) | undefined,
  isAborted: () => boolean
): Promise<void> {
  let totalBytes = 0;

  while (true) {
    // Check if audio element is in error state
    if (hasRealAudioError(audio)) {
      break;
    }

    const { done, value } = await reader.read();

    if (done) {
      handleStreamCompletion(totalBytes, mediaSource, setStatus, onComplete);
      break;
    }

    if (isAborted()) {
      reader.cancel();
      break;
    }

    totalBytes += value.length;
    setStatus(`Streaming... ${(totalBytes / 1024).toFixed(1)} KB)`);

    // Check media source and audio state before appending
    if (mediaSource.readyState !== 'open' || hasRealAudioError(audio)) {
      componentsLogger.warn('Media source not ready or audio error, stopping stream');
      reader.cancel();
      break;
    }

    await processStreamChunk(value, sourceBuffer);

    // Try to start playback after first chunk
    if (totalBytes > 0) {
      tryAutoplay(audio);
    }
  }
}

/**
 * MSE-based audio player for streaming WebM audio.
 * Uses Media Source Extensions to progressively load and play audio.
 */
export const MSEAudioPlayer: React.FC<MSEAudioPlayerProps> = ({
  stream,
  contentType,
  className,
  onComplete,
  onCancel,
  onError,
}) => {
  const audioRef = useRef<HTMLAudioElement>(null);
  const mediaSourceRef = useRef<MediaSource | null>(null);
  const readerRef = useRef<ReadableStreamDefaultReader<Uint8Array> | null>(null);
  const errorNotifiedRef = useRef<boolean>(false);
  const [status, setStatus] = useState<string>('Initializing...');
  const [error, setError] = useState<string | null>(null);
  const [isReadyToPlay, setIsReadyToPlay] = useState<boolean>(false);

  useEffect(() => {
    const audio = audioRef.current;
    if (!audio) return;

    const setErrorAndNotify = (message: string) => {
      setError(message);
      if (!errorNotifiedRef.current) {
        errorNotifiedRef.current = true;
        onError?.(message);
      }
    };

    // Check MSE support
    if (!('MediaSource' in window)) {
      setErrorAndNotify('Media Source Extensions not supported in this browser');
      return;
    }

    // Listen for when audio is ready to play
    const handleCanPlay = () => {
      componentsLogger.info('MSEAudioPlayer: Audio can play - hiding loading overlay');
      setIsReadyToPlay(true);
    };
    audio.addEventListener('canplay', handleCanPlay);

    let aborted = false;
    let abortedDueToPlaybackError = false;
    const mediaSource = new MediaSource();
    mediaSourceRef.current = mediaSource;

    // Create object URL for MediaSource
    const objectUrl = URL.createObjectURL(mediaSource);
    audio.src = objectUrl;

    const handleSourceOpen = async () => {
      if (aborted) return;

      try {
        setStatus('Opening media source...');

        // Add source buffer with the appropriate codec
        const normalizedContentType = normalizeMimeType(contentType);
        componentsLogger.debug('MSEAudioPlayer: Using MIME type:', normalizedContentType);
        const sourceBuffer = mediaSource.addSourceBuffer(normalizedContentType);

        setStatus('Streaming audio...');

        // Check if stream is already locked (can happen in StrictMode)
        if (stream.locked) {
          componentsLogger.warn('MSEAudioPlayer: Stream is already locked, skipping');
          setErrorAndNotify('Stream is already locked. Please try again.');
          return;
        }

        // Read chunks from the stream and append to source buffer
        const reader = stream.getReader();
        readerRef.current = reader; // Store reader for cleanup

        // Listen for audio element errors
        const handleAudioError = createAudioErrorHandler(
          audio,
          setErrorAndNotify,
          () => {
            aborted = true;
            abortedDueToPlaybackError = true;
          },
          reader
        );
        audio.addEventListener('error', handleAudioError);

        // Stream the audio data
        await streamAudioData(
          reader,
          sourceBuffer,
          audio,
          mediaSource,
          setStatus,
          onComplete,
          () => aborted
        );

        // Cleanup error listener
        audio.removeEventListener('error', handleAudioError);
      } catch (err) {
        if (abortedDueToPlaybackError) {
          // Playback failed (e.g., decode error). Caller can decide how to fall back.
          return;
        }
        // Handle cancellation errors
        if (isCancellationError(err) || aborted) {
          componentsLogger.info('MSEAudioPlayer: Stream cancelled/aborted by user');
          onCancel?.();
        } else {
          componentsLogger.error('MSEAudioPlayer: Streaming error:', err);
          setErrorAndNotify(err instanceof Error ? err.message : 'Unknown error');
        }
      } finally {
        // Clear reader ref
        readerRef.current = null;
      }
    };

    mediaSource.addEventListener('sourceopen', handleSourceOpen);

    // Cleanup
    return () => {
      componentsLogger.debug('MSEAudioPlayer: Cleanup called - cancelling stream reader');
      aborted = true;

      // Cancel the reader if it exists - this will cause the read() to reject
      if (readerRef.current) {
        try {
          readerRef.current.cancel('Component unmounting');
          componentsLogger.debug('MSEAudioPlayer: Reader cancelled');
        } catch (err) {
          componentsLogger.warn('MSEAudioPlayer: Error cancelling reader:', err);
        }
      }

      // Clean up event listener
      audio.removeEventListener('canplay', handleCanPlay);

      // Clean up media source
      if (mediaSource.readyState === 'open') {
        try {
          mediaSource.endOfStream();
        } catch {
          // Ignore errors during cleanup
        }
      }

      // Clean up object URL
      URL.revokeObjectURL(objectUrl);
      audio.src = '';

      // Reset error notification for future mounts
      errorNotifiedRef.current = false;
    };
  }, [stream, contentType, onComplete, onCancel, onError]);

  return (
    <PlayerContainer className={className}>
      {!isReadyToPlay && !error && (
        <LoadingOverlay>
          <LoadingSpinner message="Loading stream..." />
        </LoadingOverlay>
      )}
      <AudioElement ref={audioRef} preload="auto" aria-label="Streaming audio player">
        Your browser does not support the audio element.
      </AudioElement>
      <CustomAudioPlayer audioRef={audioRef} autoPlay />
      {error ? <ErrorText>Error: {error}</ErrorText> : <StatusText>{status}</StatusText>}
    </PlayerContainer>
  );
};
