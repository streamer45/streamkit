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

const HiddenMediaElement = styled.audio`
  display: none;
`;

const VideoElement = styled.video`
  width: 100%;
  max-height: 480px;
  border-radius: 6px;
  background: #000;
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

interface MSEPlayerProps {
  /** The ReadableStream from the fetch response */
  stream: ReadableStream<Uint8Array>;
  /** Content type (e.g., 'audio/webm; codecs="opus"' or 'video/webm; codecs="vp9"') */
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

// Helper: Create error handler for media element
function createMediaErrorHandler(
  media: HTMLMediaElement,
  setError: (msg: string) => void,
  onAbort: () => void,
  reader: ReadableStreamDefaultReader<Uint8Array>
): () => void {
  return () => {
    if (media.error) {
      // Ignore "Empty src attribute" error - this is expected during cleanup
      if (media.error.message?.includes('Empty src attribute')) {
        componentsLogger.debug('MSEPlayer: Ignoring empty src error during cleanup');
        return;
      }

      const errorMsg = `Media error: ${media.error.message || 'Unknown media error'}`;
      componentsLogger.error('MSEPlayer:', errorMsg, media.error);
      setError(errorMsg);
      onAbort();
      reader.cancel();
    }
  };
}

// Helper: Check if media element has a real error (not cleanup error)
function hasRealMediaError(media: HTMLMediaElement): boolean {
  return !!media.error && !media.error.message?.includes('Empty src attribute');
}

// Helper: Try to start playback
function tryAutoplay(media: HTMLMediaElement): void {
  if (media.paused) {
    media.play().catch((err) => {
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
async function streamMediaData(
  reader: ReadableStreamDefaultReader<Uint8Array>,
  sourceBuffer: SourceBuffer,
  media: HTMLMediaElement,
  mediaSource: MediaSource,
  setStatus: (status: string) => void,
  onComplete: (() => void) | undefined,
  isAborted: () => boolean
): Promise<void> {
  let totalBytes = 0;

  while (true) {
    // Check if media element is in error state
    if (hasRealMediaError(media)) {
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

    // Check media source and element state before appending
    if (mediaSource.readyState !== 'open' || hasRealMediaError(media)) {
      componentsLogger.warn('Media source not ready or element error, stopping stream');
      reader.cancel();
      break;
    }

    await processStreamChunk(value, sourceBuffer);

    // Try to start playback after first chunk
    if (totalBytes > 0) {
      tryAutoplay(media);
    }
  }
}

/**
 * MSE-based media player for streaming WebM audio or video.
 * Uses Media Source Extensions to progressively load and play media.
 * Automatically detects audio vs video from contentType.
 */
export const MSEPlayer: React.FC<MSEPlayerProps> = ({
  stream,
  contentType,
  className,
  onComplete,
  onCancel,
  onError,
}) => {
  const isVideo = contentType.startsWith('video/');
  const audioRef = useRef<HTMLAudioElement>(null);
  const videoRef = useRef<HTMLVideoElement>(null);
  const mediaSourceRef = useRef<MediaSource | null>(null);
  const readerRef = useRef<ReadableStreamDefaultReader<Uint8Array> | null>(null);
  const errorNotifiedRef = useRef<boolean>(false);
  const [status, setStatus] = useState<string>('Initializing...');
  const [error, setError] = useState<string | null>(null);
  const [isReadyToPlay, setIsReadyToPlay] = useState<boolean>(false);

  useEffect(() => {
    const media = isVideo ? videoRef.current : audioRef.current;
    if (!media) return;

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

    // Listen for when media is ready to play
    const handleCanPlay = () => {
      componentsLogger.info('MSEPlayer: Media can play - hiding loading overlay');
      setIsReadyToPlay(true);
    };
    media.addEventListener('canplay', handleCanPlay);

    let aborted = false;
    let abortedDueToPlaybackError = false;
    const mediaSource = new MediaSource();
    mediaSourceRef.current = mediaSource;

    // Create object URL for MediaSource
    const objectUrl = URL.createObjectURL(mediaSource);
    media.src = objectUrl;

    const handleSourceOpen = async () => {
      if (aborted) return;

      try {
        setStatus('Opening media source...');

        // Add source buffer with the appropriate codec
        const normalizedContentType = normalizeMimeType(contentType);
        componentsLogger.debug('MSEPlayer: Using MIME type:', normalizedContentType);
        const sourceBuffer = mediaSource.addSourceBuffer(normalizedContentType);

        const mediaKind = isVideo ? 'video' : 'audio';
        setStatus(`Streaming ${mediaKind}...`);

        // Check if stream is already locked (can happen in StrictMode)
        if (stream.locked) {
          componentsLogger.warn('MSEPlayer: Stream is already locked, skipping');
          setErrorAndNotify('Stream is already locked. Please try again.');
          return;
        }

        // Read chunks from the stream and append to source buffer
        const reader = stream.getReader();
        readerRef.current = reader; // Store reader for cleanup

        // Listen for media element errors
        const handleMediaError = createMediaErrorHandler(
          media,
          setErrorAndNotify,
          () => {
            aborted = true;
            abortedDueToPlaybackError = true;
          },
          reader
        );
        media.addEventListener('error', handleMediaError);

        // Stream the media data
        await streamMediaData(
          reader,
          sourceBuffer,
          media,
          mediaSource,
          setStatus,
          onComplete,
          () => aborted
        );

        // Cleanup error listener
        media.removeEventListener('error', handleMediaError);
      } catch (err) {
        if (abortedDueToPlaybackError) {
          // Playback failed (e.g., decode error). Caller can decide how to fall back.
          return;
        }
        // Handle cancellation errors
        if (isCancellationError(err) || aborted) {
          componentsLogger.info('MSEPlayer: Stream cancelled/aborted by user');
          onCancel?.();
        } else {
          componentsLogger.error('MSEPlayer: Streaming error:', err);
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
      componentsLogger.debug('MSEPlayer: Cleanup called - cancelling stream reader');
      aborted = true;

      // Cancel the reader if it exists - this will cause the read() to reject
      if (readerRef.current) {
        try {
          readerRef.current.cancel('Component unmounting');
          componentsLogger.debug('MSEPlayer: Reader cancelled');
        } catch (err) {
          componentsLogger.warn('MSEPlayer: Error cancelling reader:', err);
        }
      }

      // Clean up event listener
      media.removeEventListener('canplay', handleCanPlay);

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
      media.src = '';

      // Reset error notification for future mounts
      errorNotifiedRef.current = false;
    };
  }, [stream, contentType, isVideo, onComplete, onCancel, onError]);

  return (
    <PlayerContainer className={className}>
      {!isReadyToPlay && !error && (
        <LoadingOverlay>
          <LoadingSpinner message="Loading stream..." />
        </LoadingOverlay>
      )}
      {isVideo ? (
        <VideoElement ref={videoRef} controls preload="auto" aria-label="Streaming video player" />
      ) : (
        <>
          <HiddenMediaElement ref={audioRef} preload="auto" aria-label="Streaming audio player">
            Your browser does not support the audio element.
          </HiddenMediaElement>
          <CustomAudioPlayer audioRef={audioRef} autoPlay />
        </>
      )}
      {error ? <ErrorText>Error: {error}</ErrorText> : <StatusText>{status}</StatusText>}
    </PlayerContainer>
  );
};

/**
 * @deprecated Use MSEPlayer instead. This alias is kept for backward compatibility.
 */
export const MSEAudioPlayer = MSEPlayer;
