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

// Helper: Seek to the live edge and start playback.
// For live MSE streams the buffered ranges may be fragmented (e.g. audio
// and video starting at different times).  Seeking to buffered.start(0)
// might land on a tiny range with too little data.  Instead, seek to
// near the END of the last contiguous range (the true live edge) with a
// small rewind so the browser has forward buffer for smooth playback.
// Returns true if the seek was performed (buffered data available).
function seekToLiveEdgeAndPlay(media: HTMLMediaElement, sourceBuffer: SourceBuffer): boolean {
  const buffered = sourceBuffer.buffered;
  if (buffered.length === 0) {
    return false;
  }
  // Find the last contiguous range and check it's large enough.
  // The init segment creates a tiny range (0.00-0.02) with no real media.
  // Don't seek until there's at least 2 seconds of contiguous data.
  const lastIdx = buffered.length - 1;
  const lastStart = buffered.start(lastIdx);
  const lastEnd = buffered.end(lastIdx);
  if (lastEnd - lastStart < 2) {
    return false; // Not enough contiguous data yet — caller should retry.
  }
  // Seek to ~2s behind the live edge of the last contiguous range.
  const target = Math.max(lastStart, lastEnd - 2);
  if (Math.abs(media.currentTime - target) > 1) {
    componentsLogger.info(`MSEPlayer: Seeking to live edge at ${target.toFixed(2)}s (buffer ends at ${lastEnd.toFixed(2)}s)`);
    media.currentTime = target;
  }
  if (media.paused) {
    media.play().catch((err) => {
      componentsLogger.warn('Autoplay failed, user interaction may be required:', err);
    });
  }
  return true;
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

  // Wait for the buffer to finish updating before appending more.
  // Listen for both updateend (success) and error (decode/append failure)
  // so that SourceBuffer errors surface instead of hanging silently.
  await new Promise<void>((resolve, reject) => {
    sourceBuffer.addEventListener(
      'updateend',
      () => resolve(),
      { once: true }
    );
    sourceBuffer.addEventListener(
      'error',
      () => reject(new Error('SourceBuffer append failed (decode or format error)')),
      { once: true }
    );
  });
}

// How many seconds of buffered data to keep behind currentTime.
// Data older than this is evicted to prevent unbounded memory growth.
const BUFFER_EVICT_BEHIND_S = 10;

// How far (seconds) the player can fall behind the live edge before
// an automatic re-seek is triggered.
const LIVE_EDGE_MAX_DRIFT_S = 8;

// Minimum interval (ms) between React status updates to avoid
// re-rendering the component on every incoming chunk.
const STATUS_UPDATE_INTERVAL_MS = 1000;

// Helper: Evict buffered data that is too far behind currentTime.
// Calling sourceBuffer.remove() is async — returns a promise that
// resolves on the next `updateend`.
async function evictOldBufferData(
  sourceBuffer: SourceBuffer,
  currentTime: number,
): Promise<void> {
  if (sourceBuffer.updating || sourceBuffer.buffered.length === 0) return;
  const start = sourceBuffer.buffered.start(0);
  const evictEnd = currentTime - BUFFER_EVICT_BEHIND_S;
  if (evictEnd <= start + 1) return; // nothing meaningful to evict
  componentsLogger.debug(
    `MSEPlayer: Evicting buffer [${start.toFixed(2)}s - ${evictEnd.toFixed(2)}s] (currentTime=${currentTime.toFixed(2)}s)`
  );
  sourceBuffer.remove(start, evictEnd);
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
  isAborted: () => boolean,
  onFirstChunk?: () => void,
): Promise<void> {
  let totalBytes = 0;
  let firstChunkSeen = false;
  let playbackStarted = false;
  let lastDiagLog = 0;
  let lastStatusUpdate = 0;

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

    // Throttle React status updates to avoid re-rendering on every chunk.
    const now = Date.now();
    if (now - lastStatusUpdate >= STATUS_UPDATE_INTERVAL_MS) {
      lastStatusUpdate = now;
      setStatus(`Streaming... ${(totalBytes / 1024).toFixed(1)} KB`);
    }

    // Check media source and element state before appending.
    // If the MediaSource closed unexpectedly (e.g. SourceBuffer decode error
    // transitioned it to 'ended'), throw so the caller can surface the error
    // instead of leaving the player in a perpetual loading state.
    if (mediaSource.readyState !== 'open' || hasRealMediaError(media)) {
      componentsLogger.warn('Media source not ready or element error, stopping stream');
      reader.cancel();
      throw new Error(
        `Media source closed unexpectedly (readyState: ${mediaSource.readyState})`
      );
    }

    await processStreamChunk(value, sourceBuffer);

    // Start stall timer after the first chunk so we detect hangs.
    if (!firstChunkSeen && totalBytes > 0) {
      firstChunkSeen = true;
      onFirstChunk?.();
    }

    // Evict old buffered data to bound memory usage.
    if (playbackStarted) {
      await evictOldBufferData(sourceBuffer, media.currentTime);
    }

    // Periodic diagnostics + live-edge tracking.
    if (now - lastDiagLog > 5000 && sourceBuffer.buffered.length > 0) {
      lastDiagLog = now;
      const ranges = [];
      for (let i = 0; i < sourceBuffer.buffered.length; i++) {
        ranges.push(`${sourceBuffer.buffered.start(i).toFixed(2)}-${sourceBuffer.buffered.end(i).toFixed(2)}`);
      }
      componentsLogger.info(
        `MSEPlayer diag: currentTime=${media.currentTime.toFixed(2)} paused=${media.paused} ` +
        `readyState=${media.readyState} buffered=[${ranges.join(', ')}] bytes=${(totalBytes / 1024).toFixed(0)}KB`
      );

      // Re-seek to the live edge if the player drifts too far behind.
      if (playbackStarted) {
        const lastIdx = sourceBuffer.buffered.length - 1;
        const liveEdge = sourceBuffer.buffered.end(lastIdx);
        if (liveEdge - media.currentTime > LIVE_EDGE_MAX_DRIFT_S) {
          const target = liveEdge - 2;
          componentsLogger.info(
            `MSEPlayer: Drifted ${(liveEdge - media.currentTime).toFixed(1)}s behind live edge, ` +
            `re-seeking to ${target.toFixed(2)}s`
          );
          media.currentTime = target;
        }
      }
    }

    // Seek to the live edge once media data is actually buffered.
    // The first few chunks (init segment, Cluster preamble) don't produce
    // buffered ranges — only SimpleBlock data does.  Keep trying on each
    // chunk until the seek succeeds.
    if (!playbackStarted && seekToLiveEdgeAndPlay(media, sourceBuffer)) {
      playbackStarted = true;
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

  // Stable refs for callbacks — prevents the main useEffect from re-running
  // (and tearing down the MediaSource) when the parent re-renders with new
  // inline function references (e.g. on fullscreen toggle).
  const onCompleteRef = useRef(onComplete);
  const onCancelRef = useRef(onCancel);
  const onErrorRef = useRef(onError);
  onCompleteRef.current = onComplete;
  onCancelRef.current = onCancel;
  onErrorRef.current = onError;
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
        onErrorRef.current?.(message);
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
      if (stallTimerHandle) { clearTimeout(stallTimerHandle); stallTimerHandle = null; }
      setIsReadyToPlay(true);
      // Retry autoplay — the initial play() call may have fired before
      // enough data was buffered, leaving the element paused.
      if (media.paused) {
        media.play().catch((err) => {
          componentsLogger.warn('Autoplay on canplay failed:', err);
        });
      }
    };
    media.addEventListener('canplay', handleCanPlay);

    // Stall timer handle — set inside handleSourceOpen, cleared on canplay or cleanup.
    let stallTimerHandle: ReturnType<typeof setTimeout> | null = null;

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

        // Use the content type from the server (set by the pipeline YAML).
        // This declares all expected codecs (e.g. "vp9,opus") — the backend
        // muxer must wait for all inputs before producing the init segment.
        const mseContentType = normalizeMimeType(contentType);
        componentsLogger.debug('MSEPlayer: Using MIME type:', mseContentType);
        const sourceBuffer = mediaSource.addSourceBuffer(mseContentType);

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

        // Stall detection: if data is flowing but canplay never fires within
        // 15 seconds, log diagnostics and retry the live-edge seek.  Don't kill
        // the stream — live pipelines with MoQ input can take 20+ seconds to
        // accumulate enough contiguous data for canplay.
        const startStallTimer = () => {
          if (stallTimerHandle) return;
          stallTimerHandle = setTimeout(() => {
            if (aborted) return;
            const readyState = media.readyState;
            const buffered = sourceBuffer.buffered;
            const ranges = [];
            for (let i = 0; i < buffered.length; i++) {
              ranges.push(`${buffered.start(i).toFixed(2)}-${buffered.end(i).toFixed(2)}`);
            }
            const diag = `readyState=${readyState}, buffered=[${ranges.join(', ')}]`;
            componentsLogger.warn(`MSEPlayer: canplay not fired after 15s — retrying seek (${diag})`);
            // Retry seeking to the live edge — buffer may have grown since the first attempt.
            seekToLiveEdgeAndPlay(media, sourceBuffer);
          }, 15_000);
        };

        // Stream the media data
        await streamMediaData(
          reader,
          sourceBuffer,
          media,
          mediaSource,
          setStatus,
          onCompleteRef.current,
          () => aborted,
          startStallTimer,
        );

        // Cancel stall timer on normal completion.
        if (stallTimerHandle) { clearTimeout(stallTimerHandle); stallTimerHandle = null; }

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
          onCancelRef.current?.();
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

      // Cancel stall detection timer.
      if (stallTimerHandle) { clearTimeout(stallTimerHandle); stallTimerHandle = null; }

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
    // Callbacks are accessed via refs so they don't trigger effect re-runs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stream, contentType, isVideo]);

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
