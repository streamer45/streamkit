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

function hasRealMediaError(media: HTMLMediaElement): boolean {
  return !!media.error && !media.error.message?.includes('Empty src attribute');
}

function startPlaybackFromBeginning(media: HTMLMediaElement, sourceBuffer: SourceBuffer): boolean {
  const buffered = sourceBuffer.buffered;
  if (buffered.length === 0) {
    return false;
  }
  // Wait until there's at least a small amount of contiguous data.
  const rangeEnd = buffered.end(0);
  if (rangeEnd < 0.5) {
    return false;
  }
  componentsLogger.info(
    `MSEPlayer: Starting playback from beginning (buffered to ${rangeEnd.toFixed(2)}s)`
  );
  if (media.paused) {
    media.play().catch((err) => {
      componentsLogger.warn('Autoplay failed, user interaction may be required:', err);
    });
  }
  return true;
}

function isCancellationError(err: unknown): boolean {
  const isCancellation =
    err instanceof TypeError && (err.message.includes('cancel') || err.message.includes('Cancel'));
  const isAbortError = err instanceof Error && err.name === 'AbortError';
  return isCancellation || isAbortError;
}

function handleStreamCompletion(
  totalBytes: number,
  mediaSource: MediaSource,
  setStatus: (status: string) => void,
  onComplete?: () => void
): void {
  setStatus(`Completed (${(totalBytes / 1024).toFixed(1)} KB)`);
  if (mediaSource.readyState === 'open') {
    mediaSource.endOfStream();
  }
  onComplete?.();
}

async function processStreamChunk(value: Uint8Array, sourceBuffer: SourceBuffer): Promise<void> {
  const buffer = new Uint8Array(value);
  sourceBuffer.appendBuffer(buffer);

  await new Promise<void>((resolve, reject) => {
    const onUpdateEnd = () => {
      sourceBuffer.removeEventListener('error', onError);
      resolve();
    };
    const onError = () => {
      sourceBuffer.removeEventListener('updateend', onUpdateEnd);
      reject(new Error('SourceBuffer append failed (decode or format error)'));
    };
    sourceBuffer.addEventListener('updateend', onUpdateEnd, { once: true });
    sourceBuffer.addEventListener('error', onError, { once: true });
  });
}

// Minimum interval (ms) between React status updates to avoid
// re-rendering the component on every incoming chunk.
const STATUS_UPDATE_INTERVAL_MS = 1000;

/** Log periodic stream diagnostics and force-resume paused playback
 *  when there is enough forward buffer (MSE demuxers sometimes pause
 *  on timestamp irregularities from cross-track clamping). */
function logStreamDiagnostics(
  sourceBuffer: SourceBuffer,
  media: HTMLMediaElement,
  totalBytes: number,
  playbackStarted: boolean
): void {
  const ranges = [];
  for (let i = 0; i < sourceBuffer.buffered.length; i++) {
    ranges.push(
      `${sourceBuffer.buffered.start(i).toFixed(2)}-${sourceBuffer.buffered.end(i).toFixed(2)}`
    );
  }
  const lastEnd = sourceBuffer.buffered.end(sourceBuffer.buffered.length - 1);
  const fwdBuf = lastEnd - media.currentTime;

  componentsLogger.info(
    `MSEPlayer diag: currentTime=${media.currentTime.toFixed(2)} paused=${media.paused} ` +
      `readyState=${media.readyState} rate=${media.playbackRate} ` +
      `buffered=[${ranges.join(', ')}] fwdBuf=${fwdBuf.toFixed(1)}s bytes=${(totalBytes / 1024).toFixed(0)}KB`
  );

  if (playbackStarted && media.paused && fwdBuf > 1.0) {
    componentsLogger.info(`MSEPlayer: force-resuming (paused with ${fwdBuf.toFixed(1)}s fwdBuf)`);
    media.play().catch(() => {});
  }
}

/** Create stall/resume event handlers that detect buffering events,
 *  log diagnostics, and skip past buffer gaps when the player is stuck. */
function setupStallResumeDiagnostics(
  media: HTMLMediaElement,
  sourceBuffer: SourceBuffer
): { handleWaiting: () => void; handlePlaying: () => void } {
  let lastWaitingTime = 0;

  const handleWaiting = () => {
    lastWaitingTime = performance.now();
    const ranges = [];
    for (let i = 0; i < sourceBuffer.buffered.length; i++) {
      ranges.push(
        `${sourceBuffer.buffered.start(i).toFixed(2)}-${sourceBuffer.buffered.end(i).toFixed(2)}`
      );
    }
    componentsLogger.warn(
      `MSEPlayer: WAITING (buffering) at currentTime=${media.currentTime.toFixed(2)} ` +
        `readyState=${media.readyState} buffered=[${ranges.join(', ')}]`
    );

    // If stuck at a buffer gap, skip past it immediately.
    const buffered = sourceBuffer.buffered;
    for (let i = 0; i < buffered.length - 1; i++) {
      const gapStart = buffered.end(i);
      const gapEnd = buffered.start(i + 1);
      if (media.currentTime >= gapStart - 0.5 && media.currentTime <= gapEnd + 0.1) {
        componentsLogger.info(
          `MSEPlayer: skipping buffer gap [${gapStart.toFixed(2)}-${gapEnd.toFixed(2)}], ` +
            `seeking to ${(gapEnd + 0.1).toFixed(2)}s`
        );
        media.currentTime = gapEnd + 0.1;
        if (media.paused) {
          media.play().catch(() => {});
        }
        return;
      }
    }
  };

  const handlePlaying = () => {
    if (lastWaitingTime > 0) {
      const stallMs = performance.now() - lastWaitingTime;
      componentsLogger.info(`MSEPlayer: resumed after ${stallMs.toFixed(0)}ms stall`);
      lastWaitingTime = 0;
    }
  };

  return { handleWaiting, handlePlaying };
}

/** Create a stall-detection timer that retries playback if canplay doesn't
 *  fire within 15 seconds.  Returns start/clear helpers and exposes the
 *  handle so the outer cleanup can cancel it. */
function createStallTimer(
  media: HTMLMediaElement,
  sourceBuffer: SourceBuffer,
  isAborted: () => boolean
): {
  start: () => void;
  clear: () => void;
  handle: { current: ReturnType<typeof setTimeout> | null };
} {
  const ref: { current: ReturnType<typeof setTimeout> | null } = { current: null };
  const start = () => {
    if (ref.current) return;
    ref.current = setTimeout(() => {
      if (isAborted()) return;
      const readyState = media.readyState;
      const buffered = sourceBuffer.buffered;
      const ranges = [];
      for (let i = 0; i < buffered.length; i++) {
        ranges.push(`${buffered.start(i).toFixed(2)}-${buffered.end(i).toFixed(2)}`);
      }
      const diag = `readyState=${readyState}, buffered=[${ranges.join(', ')}]`;
      componentsLogger.warn(`MSEPlayer: canplay not fired after 15s — retrying seek (${diag})`);
      startPlaybackFromBeginning(media, sourceBuffer);
    }, 15_000);
  };
  const clear = () => {
    if (ref.current) {
      clearTimeout(ref.current);
      ref.current = null;
    }
  };
  return { start, clear, handle: ref };
}

/** Set up all event listeners for the MSE streaming session (stall/resume
 *  diagnostics, media error handler).  Returns a cleanup function. */
function attachStreamListeners(
  media: HTMLMediaElement,
  sourceBuffer: SourceBuffer,
  reader: ReadableStreamDefaultReader<Uint8Array>,
  setErrorAndNotify: (msg: string) => void,
  setAborted: () => void
): { cleanup: () => void } {
  const { handleWaiting, handlePlaying } = setupStallResumeDiagnostics(media, sourceBuffer);
  media.addEventListener('waiting', handleWaiting);
  media.addEventListener('playing', handlePlaying);

  const handleMediaError = createMediaErrorHandler(media, setErrorAndNotify, setAborted, reader);
  media.addEventListener('error', handleMediaError);

  return {
    cleanup: () => {
      media.removeEventListener('error', handleMediaError);
      media.removeEventListener('waiting', handleWaiting);
      media.removeEventListener('playing', handlePlaying);
    },
  };
}

/** Process a single chunk from the stream: append to buffer, handle first-chunk
 *  callback, run periodic diagnostics, and attempt playback start. */
async function processChunkAndUpdateState(
  value: Uint8Array,
  sourceBuffer: SourceBuffer,
  media: HTMLMediaElement,
  state: {
    totalBytes: number;
    firstChunkSeen: boolean;
    playbackStarted: boolean;
    lastDiagLog: number;
  },
  now: number,
  onFirstChunk?: () => void
): Promise<void> {
  await processStreamChunk(value, sourceBuffer);

  // Start stall timer after the first chunk so we detect hangs.
  if (!state.firstChunkSeen && state.totalBytes > 0) {
    state.firstChunkSeen = true;
    componentsLogger.info(
      `MSEPlayer: first chunk received, ${state.totalBytes}B, t=${performance.now().toFixed(0)}ms`
    );
    onFirstChunk?.();
  }

  // Periodic diagnostics.
  if (now - state.lastDiagLog > 5000 && sourceBuffer.buffered.length > 0) {
    state.lastDiagLog = now;
    logStreamDiagnostics(sourceBuffer, media, state.totalBytes, state.playbackStarted);
  }

  // Start playback once media data is actually buffered.
  if (!state.playbackStarted && startPlaybackFromBeginning(media, sourceBuffer)) {
    state.playbackStarted = true;
  }
}

async function streamMediaData(
  reader: ReadableStreamDefaultReader<Uint8Array>,
  sourceBuffer: SourceBuffer,
  media: HTMLMediaElement,
  mediaSource: MediaSource,
  setStatus: (status: string) => void,
  onComplete: (() => void) | undefined,
  isAborted: () => boolean,
  onFirstChunk?: () => void
): Promise<void> {
  const state = {
    totalBytes: 0,
    firstChunkSeen: false,
    playbackStarted: false,
    lastDiagLog: 0,
  };
  let lastStatusUpdate = 0;

  while (true) {
    if (hasRealMediaError(media)) break;

    const { done, value } = await reader.read();

    if (done) {
      handleStreamCompletion(state.totalBytes, mediaSource, setStatus, onComplete);
      break;
    }

    if (isAborted()) {
      reader.cancel();
      break;
    }

    state.totalBytes += value.length;

    // Throttle React status updates to avoid re-rendering on every chunk.
    const now = Date.now();
    if (now - lastStatusUpdate >= STATUS_UPDATE_INTERVAL_MS) {
      lastStatusUpdate = now;
      setStatus(`Streaming... ${(state.totalBytes / 1024).toFixed(1)} KB`);
    }

    if (mediaSource.readyState !== 'open' || hasRealMediaError(media)) {
      componentsLogger.warn('Media source not ready or element error, stopping stream');
      reader.cancel();
      throw new Error(`Media source closed unexpectedly (readyState: ${mediaSource.readyState})`);
    }

    await processChunkAndUpdateState(value, sourceBuffer, media, state, now, onFirstChunk);
  }
}

/**
 * MSE-based media player for oneshot/convert pipeline streams.
 *
 * Uses Media Source Extensions to progressively play a WebM or fMP4
 * ReadableStream (typically from a POST response body) that cannot be
 * addressed by URL.  Automatically detects audio vs video from contentType.
 *
 * For live streaming over HTTP (chunked transfer), use NativeStreamPlayer
 * instead — it uses a plain `<video>` element with a URL source.
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

    if (!('MediaSource' in window)) {
      setErrorAndNotify('Media Source Extensions not supported in this browser');
      return;
    }

    const handleCanPlay = () => {
      componentsLogger.info('MSEPlayer: Media can play - hiding loading overlay');
      if (stallTimerRef?.current) {
        clearTimeout(stallTimerRef.current);
        stallTimerRef.current = null;
      }
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

    // Stall timer — created inside handleSourceOpen, cleared on canplay or cleanup.
    let stallTimerRef: { current: ReturnType<typeof setTimeout> | null } | null = null;

    let aborted = false;
    let abortedDueToPlaybackError = false;
    const mediaSource = new MediaSource();
    mediaSourceRef.current = mediaSource;

    const objectUrl = URL.createObjectURL(mediaSource);
    media.src = objectUrl;

    const handleSourceOpen = async () => {
      if (aborted) return;

      try {
        setStatus('Opening media source...');

        // Content type from server declares expected codecs (e.g. "vp9,opus") — the backend
        // muxer must wait for all inputs before producing the init segment.
        const mseContentType = normalizeMimeType(contentType);
        componentsLogger.debug('MSEPlayer: Using MIME type:', mseContentType);
        const sourceBuffer = mediaSource.addSourceBuffer(mseContentType);

        const mediaKind = isVideo ? 'video' : 'audio';
        setStatus(`Streaming ${mediaKind}...`);

        if (stream.locked) {
          componentsLogger.warn('MSEPlayer: Stream is already locked, skipping');
          setErrorAndNotify('Stream is already locked. Please try again.');
          return;
        }

        // Read chunks from the stream and append to source buffer
        const reader = stream.getReader();
        readerRef.current = reader; // Store reader for cleanup

        const listeners = attachStreamListeners(
          media,
          sourceBuffer,
          reader,
          setErrorAndNotify,
          () => {
            aborted = true;
            abortedDueToPlaybackError = true;
          }
        );

        // Stall detection: if data is flowing but canplay never fires within
        // 15 seconds, log diagnostics and retry playback from the beginning.
        const stallTimer = createStallTimer(media, sourceBuffer, () => aborted);
        stallTimerRef = stallTimer.handle;

        // Stream the media data
        await streamMediaData(
          reader,
          sourceBuffer,
          media,
          mediaSource,
          setStatus,
          onCompleteRef.current,
          () => aborted,
          stallTimer.start
        );

        // Cancel stall timer on normal completion.
        stallTimer.clear();
        listeners.cleanup();
      } catch (err) {
        if (abortedDueToPlaybackError) {
          // Playback failed (e.g., decode error). Caller can decide how to fall back.
          return;
        }
        if (isCancellationError(err) || aborted) {
          componentsLogger.info('MSEPlayer: Stream cancelled/aborted by user');
          onCancelRef.current?.();
        } else {
          componentsLogger.error('MSEPlayer: Streaming error:', err);
          setErrorAndNotify(err instanceof Error ? err.message : 'Unknown error');
        }
      } finally {
        readerRef.current = null;
      }
    };

    mediaSource.addEventListener('sourceopen', handleSourceOpen);

    return () => {
      componentsLogger.debug('MSEPlayer: Cleanup called - cancelling stream reader');
      aborted = true;

      // Cancel stall detection timer.
      if (stallTimerRef?.current) {
        clearTimeout(stallTimerRef.current);
        stallTimerRef.current = null;
      }

      if (readerRef.current) {
        try {
          readerRef.current.cancel('Component unmounting');
          componentsLogger.debug('MSEPlayer: Reader cancelled');
        } catch (err) {
          componentsLogger.warn('MSEPlayer: Error cancelling reader:', err);
        }
      }

      media.removeEventListener('canplay', handleCanPlay);

      if (mediaSource.readyState === 'open') {
        try {
          mediaSource.endOfStream();
        } catch {
          // Ignore errors during cleanup
        }
      }

      URL.revokeObjectURL(objectUrl);
      media.src = '';

      errorNotifiedRef.current = false;
    };
    // Callbacks are accessed via refs so they don't trigger effect re-runs.
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
