// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React, { useCallback, useEffect, useRef } from 'react';

import { componentsLogger } from '@/utils/logger';

/** Maximum acceptable latency (seconds) before seeking to live edge. */
const DEFAULT_MAX_LATENCY_S = 3;
/** How often (ms) to check buffer latency in live mode. */
const LATENCY_CHECK_INTERVAL_MS = 2000;

const PlayerContainer = styled.div`
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 16px;
  background: var(--surface-secondary);
  border-radius: 8px;
  border: 1px solid var(--border-primary);

  video {
    width: 100%;
    max-height: 480px;
    border-radius: 6px;
    background: #000;
  }
`;

interface NativeStreamPlayerProps {
  /** URL to stream from (e.g. the MSE HTTP endpoint). */
  src: string;
  /** MIME type hint (used on the <source> element). */
  type?: string;
  /** Whether this is a live stream.  When true the player periodically
   *  seeks to the live edge to keep latency bounded. */
  live?: boolean;
  /** Maximum acceptable latency in seconds before auto-seeking to the
   *  live edge.  Only relevant when `live` is true.  Defaults to 3. */
  maxLatency?: number;
  /** Optional CSS class name. */
  className?: string;
  /** Called when the player encounters an unrecoverable error. */
  onError?: (message: string) => void;
}

/**
 * Lightweight `<video>` wrapper for chunked WebM streams.
 *
 * The browser's native HTML5 decoder handles VP9/Opus buffering and
 * scheduling directly — no MediaSource API, no third-party player
 * library.  For live streams, a periodic timer checks the distance
 * between `currentTime` and the end of the buffered range; when it
 * exceeds `maxLatency` the player seeks forward to keep latency low.
 */
export const NativeStreamPlayer: React.FC<NativeStreamPlayerProps> = ({
  src,
  type = 'video/webm',
  live = false,
  maxLatency = DEFAULT_MAX_LATENCY_S,
  className,
  onError,
}) => {
  const videoRef = useRef<HTMLVideoElement>(null);
  const onErrorRef = useRef(onError);
  useEffect(() => {
    onErrorRef.current = onError;
  }, [onError]);

  const seekToLiveEdge = useCallback((video: HTMLVideoElement) => {
    const buf = video.buffered;
    if (buf.length === 0) return;
    const liveEdge = buf.end(buf.length - 1);
    // Seek to slightly behind the edge so playback doesn't immediately stall.
    const target = Math.max(0, liveEdge - 0.5);
    if (video.currentTime < target) {
      componentsLogger.debug(
        `NativeStreamPlayer: seeking to live edge (${video.currentTime.toFixed(1)}s -> ${target.toFixed(1)}s, latency=${(liveEdge - video.currentTime).toFixed(1)}s)`
      );
      video.currentTime = target;
    }
  }, []);

  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;

    // Autoplay muted to satisfy browser autoplay policies — most browsers
    // block unmuted autoplay.  The native <video> controls include a mute
    // toggle so users can unmute manually once they interact with the page.
    video.muted = true;
    video.play().catch((err) => {
      componentsLogger.warn('NativeStreamPlayer: autoplay blocked:', err);
    });

    if (!live) return;

    // In live mode, periodically check latency and seek forward.
    const timer = setInterval(() => {
      const buf = video.buffered;
      if (buf.length === 0) return;
      const liveEdge = buf.end(buf.length - 1);
      const latency = liveEdge - video.currentTime;
      if (latency > maxLatency) {
        seekToLiveEdge(video);
      }
    }, LATENCY_CHECK_INTERVAL_MS);

    // Initial seek once enough data has buffered.
    const onCanPlay = () => seekToLiveEdge(video);
    video.addEventListener('canplay', onCanPlay, { once: true });

    return () => {
      clearInterval(timer);
      video.removeEventListener('canplay', onCanPlay);
    };
  }, [src, live, maxLatency, seekToLiveEdge]);

  const handleError = useCallback(() => {
    const video = videoRef.current;
    if (!video?.error) return;
    const { code, message } = video.error;
    const msg = `Video error [${code}]: ${message || 'Unknown'}`;
    componentsLogger.error('NativeStreamPlayer:', msg);
    onErrorRef.current?.(msg);
  }, []);

  return (
    <PlayerContainer className={className}>
      {/* Live/generated stream — no caption track exists to attach. */}
      <video
        ref={videoRef}
        controls
        aria-label={live ? 'Live stream player' : 'Stream player'}
        onError={handleError}
      >
        <source src={src} type={type} />
      </video>
    </PlayerContainer>
  );
};
