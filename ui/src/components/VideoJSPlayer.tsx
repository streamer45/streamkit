// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React, { useEffect, useRef } from 'react';
import videojs from 'video.js';
import type Player from 'video.js/dist/types/player';

import 'video.js/dist/video-js.css';

import { componentsLogger } from '@/utils/logger';

const PlayerContainer = styled.div`
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 16px;
  background: var(--surface-secondary);
  border-radius: 8px;
  border: 1px solid var(--border-primary);

  /* Video.js skin overrides to blend with StreamKit's dark UI. */
  .video-js {
    width: 100%;
    max-height: 480px;
    border-radius: 6px;
    background: #000;
    font-family: inherit;
  }

  .video-js .vjs-big-play-button {
    /* Center the big-play button. */
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
  }
`;

interface VideoJSPlayerProps {
  /** URL to stream from (e.g. the MSE HTTP endpoint). */
  src: string;
  /** MIME type of the source (e.g. 'video/webm; codecs="vp9,opus"'). */
  type?: string;
  /** Whether this is a live stream (adds liveui). */
  live?: boolean;
  /** Optional CSS class name. */
  className?: string;
  /** Called when the player encounters an unrecoverable error. */
  onError?: (message: string) => void;
}

/**
 * Thin React wrapper around Video.js.
 *
 * For live WebM streams the browser's native HTML5 `<video>` decoder
 * handles buffering, seeking, and VP9/Opus decode scheduling — avoiding
 * all the Chrome-specific MSE SourceBuffer stalling issues that plague
 * manual MSE implementations.
 */
export const VideoJSPlayer: React.FC<VideoJSPlayerProps> = ({
  src,
  type = 'video/webm',
  live = false,
  className,
  onError,
}) => {
  const containerRef = useRef<HTMLDivElement>(null);
  const playerRef = useRef<Player | null>(null);
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    // Video.js expects a <video-js> element inside our container.
    const videoEl = document.createElement('video-js');
    videoEl.classList.add('vjs-big-play-centered');
    container.appendChild(videoEl);

    const player = videojs(videoEl, {
      controls: true,
      autoplay: true,
      preload: 'auto',
      fluid: true,
      liveui: live,
      html5: {
        vhs: {
          // Disable VHS (Video.js HTTP Streaming) — we serve raw WebM,
          // not HLS/DASH manifests.
          overrideNative: false,
        },
        nativeVideoTracks: true,
        nativeAudioTracks: true,
      },
      sources: [{ src, type }],
    });

    playerRef.current = player;

    player.on('error', () => {
      const err = player.error();
      if (err) {
        const msg = `Video.js error [${err.code}]: ${err.message || 'Unknown'}`;
        componentsLogger.error('VideoJSPlayer:', msg);
        onErrorRef.current?.(msg);
      }
    });

    player.on('playing', () => {
      componentsLogger.info('VideoJSPlayer: playing');
    });

    componentsLogger.info(`VideoJSPlayer: initialized (src=${src}, live=${live})`);

    return () => {
      if (playerRef.current && !playerRef.current.isDisposed()) {
        componentsLogger.debug('VideoJSPlayer: disposing');
        playerRef.current.dispose();
        playerRef.current = null;
      }
    };
    // Re-create the player when src or live mode changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [src, live]);

  return (
    <PlayerContainer className={className}>
      <div ref={containerRef} />
    </PlayerContainer>
  );
};
