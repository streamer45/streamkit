// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React, { useEffect, useRef, useState } from 'react';

import { componentsLogger } from '@/utils/logger';

const PlayerContainer = styled.div`
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 16px;
  background: var(--sk-bg);
  border: 1px solid var(--sk-border);
  border-radius: 8px;
`;

const ControlsRow = styled.div`
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
  min-width: 0;
`;

const PlayButton = styled.button`
  width: 40px;
  height: 40px;
  border-radius: 50%;
  border: none;
  background: var(--sk-primary);
  color: var(--sk-primary-contrast);
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: all 0.2s;
  flex-shrink: 0;

  &:hover {
    background: var(--sk-primary-hover);
    transform: scale(1.05);
  }

  &:active {
    transform: scale(0.95);
  }

  &:disabled {
    background: var(--sk-hover-bg);
    color: var(--sk-text-muted);
    cursor: not-allowed;
    transform: none;
  }

  svg {
    width: 18px;
    height: 18px;
    fill: currentColor;
  }
`;

const TimeDisplay = styled.div`
  font-size: 13px;
  color: var(--sk-text);
  font-family: var(--sk-font-code);
  min-width: 92px;
  flex-shrink: 0;
  white-space: nowrap;

  @media (max-width: 520px) {
    min-width: 76px;
  }
`;

const ProgressBarContainer = styled.div`
  flex: 1 1 220px;
  min-width: 160px;
  height: 32px;
  display: flex;
  align-items: center;
  cursor: pointer;
  position: relative;
  min-width: 0;
`;

const ProgressBarTrack = styled.div`
  width: 100%;
  height: 6px;
  background: var(--sk-border);
  border-radius: 3px;
  position: relative;
  overflow: hidden;
`;

const ProgressBarFill = styled.div<{ progress: number }>`
  height: 100%;
  background: var(--sk-primary);
  border-radius: 3px;
  width: ${(props) => props.progress * 100}%;
  transition: width 0.1s linear;
`;

const ProgressBarHandle = styled.div<{ progress: number }>`
  position: absolute;
  top: 50%;
  left: ${(props) => props.progress * 100}%;
  transform: translate(-50%, -50%);
  width: 16px;
  height: 16px;
  background: var(--sk-primary);
  border: 2px solid var(--sk-panel-bg);
  border-radius: 50%;
  box-shadow: 0 2px 4px var(--sk-shadow);
  cursor: pointer;
  transition: transform 0.2s;

  &:hover {
    transform: translate(-50%, -50%) scale(1.2);
  }
`;

const VolumeContainer = styled.div`
  display: flex;
  align-items: center;
  gap: 8px;
  flex: 0 1 160px;
  min-width: 0;
`;

const VolumeButton = styled.button`
  width: 32px;
  height: 32px;
  border-radius: 6px;
  border: none;
  background: transparent;
  color: var(--sk-text);
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: all 0.2s;

  &:hover {
    background: var(--sk-hover-bg);
  }

  svg {
    width: 20px;
    height: 20px;
    fill: currentColor;
  }
`;

const VolumeSlider = styled.input`
  flex: 1 1 80px;
  width: auto;
  min-width: 56px;
  max-width: 120px;
  height: 6px;
  border-radius: 3px;
  background: var(--sk-border);
  outline: none;
  -webkit-appearance: none;

  &::-webkit-slider-thumb {
    -webkit-appearance: none;
    appearance: none;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--sk-primary);
    cursor: pointer;
    transition: transform 0.2s;

    &:hover {
      transform: scale(1.2);
    }
  }

  &::-moz-range-thumb {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--sk-primary);
    cursor: pointer;
    border: none;
    transition: transform 0.2s;

    &:hover {
      transform: scale(1.2);
    }
  }
`;

interface CustomAudioPlayerProps {
  audioRef: React.RefObject<HTMLAudioElement | null>;
  autoPlay?: boolean;
  className?: string;
}

/**
 * Custom audio player with styled controls for dark theme
 */
export const CustomAudioPlayer: React.FC<CustomAudioPlayerProps> = ({
  audioRef,
  autoPlay = false,
  className,
}) => {
  const [isPlaying, setIsPlaying] = useState(autoPlay);
  const [currentTime, setCurrentTime] = useState(0);
  const [duration, setDuration] = useState(0);
  const [volume, setVolume] = useState(1);
  const [isMuted, setIsMuted] = useState(false);
  const progressBarRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const audio = audioRef.current;
    if (!audio) return;

    // Set initial volume
    audio.volume = volume;

    const handleTimeUpdate = () => setCurrentTime(audio.currentTime);
    const handleDurationChange = () => setDuration(audio.duration);
    const handleEnded = () => setIsPlaying(false);
    const handlePlay = () => setIsPlaying(true);
    const handlePause = () => setIsPlaying(false);

    audio.addEventListener('timeupdate', handleTimeUpdate);
    audio.addEventListener('durationchange', handleDurationChange);
    audio.addEventListener('ended', handleEnded);
    audio.addEventListener('play', handlePlay);
    audio.addEventListener('pause', handlePause);

    // Auto-play if specified
    if (autoPlay) {
      audio.play().catch((err) => {
        componentsLogger.warn('Autoplay failed:', err);
        setIsPlaying(false);
      });
    }

    return () => {
      audio.removeEventListener('timeupdate', handleTimeUpdate);
      audio.removeEventListener('durationchange', handleDurationChange);
      audio.removeEventListener('ended', handleEnded);
      audio.removeEventListener('play', handlePlay);
      audio.removeEventListener('pause', handlePause);
    };
  }, [audioRef, autoPlay, volume]);

  const togglePlayPause = () => {
    const audio = audioRef.current;
    if (!audio) return;

    if (isPlaying) {
      audio.pause();
    } else {
      audio.play().catch((err) => {
        componentsLogger.error('Play failed:', err);
      });
    }
  };

  const handleProgressBarClick = (e: React.MouseEvent<HTMLDivElement>) => {
    const audio = audioRef.current;
    const progressBar = progressBarRef.current;
    if (!audio || !progressBar) return;

    const rect = progressBar.getBoundingClientRect();
    const clickX = e.clientX - rect.left;
    const percentage = clickX / rect.width;
    audio.currentTime = percentage * duration;
  };

  const toggleMute = () => {
    const audio = audioRef.current;
    if (!audio) return;

    audio.muted = !isMuted;
    setIsMuted(!isMuted);
  };

  const handleVolumeChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const audio = audioRef.current;
    if (!audio) return;

    const newVolume = parseFloat(e.target.value);
    setVolume(newVolume);
    audio.volume = newVolume;

    if (newVolume > 0 && isMuted) {
      audio.muted = false;
      setIsMuted(false);
    }
  };

  const formatTime = (seconds: number): string => {
    if (!isFinite(seconds)) return '0:00';
    const mins = Math.floor(seconds / 60);
    const secs = Math.floor(seconds % 60);
    return `${mins}:${secs.toString().padStart(2, '0')}`;
  };

  const progress = duration > 0 ? currentTime / duration : 0;

  return (
    <PlayerContainer className={className}>
      <ControlsRow>
        <PlayButton onClick={togglePlayPause} disabled={!duration}>
          {isPlaying ? (
            // Pause icon
            <svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
              <path d="M6 4h4v16H6V4zm8 0h4v16h-4V4z" />
            </svg>
          ) : (
            // Play icon
            <svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
              <path d="M8 5v14l11-7z" />
            </svg>
          )}
        </PlayButton>

        <TimeDisplay>
          {formatTime(currentTime)} / {formatTime(duration)}
        </TimeDisplay>

        <ProgressBarContainer ref={progressBarRef} onClick={handleProgressBarClick}>
          <ProgressBarTrack>
            <ProgressBarFill progress={progress} />
          </ProgressBarTrack>
          <ProgressBarHandle progress={progress} />
        </ProgressBarContainer>

        <VolumeContainer>
          <VolumeButton onClick={toggleMute}>
            {isMuted || volume === 0 ? (
              // Muted icon
              <svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
                <path d="M16.5 12c0-1.77-1.02-3.29-2.5-4.03v2.21l2.45 2.45c.03-.2.05-.41.05-.63zm2.5 0c0 .94-.2 1.82-.54 2.64l1.51 1.51C20.63 14.91 21 13.5 21 12c0-4.28-2.99-7.86-7-8.77v2.06c2.89.86 5 3.54 5 6.71zM4.27 3L3 4.27 7.73 9H3v6h4l5 5v-6.73l4.25 4.25c-.67.52-1.42.93-2.25 1.18v2.06c1.38-.31 2.63-.95 3.69-1.81L19.73 21 21 19.73l-9-9L4.27 3zM12 4L9.91 6.09 12 8.18V4z" />
              </svg>
            ) : volume < 0.5 ? (
              // Low volume icon
              <svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
                <path d="M7 9v6h4l5 5V4l-5 5H7z" />
              </svg>
            ) : (
              // High volume icon
              <svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
                <path d="M3 9v6h4l5 5V4L7 9H3zm13.5 3c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02zM14 3.23v2.06c2.89.86 5 3.54 5 6.71s-2.11 5.85-5 6.71v2.06c4.01-.91 7-4.49 7-8.77s-2.99-7.86-7-8.77z" />
              </svg>
            )}
          </VolumeButton>
          <VolumeSlider
            type="range"
            min="0"
            max="1"
            step="0.01"
            value={isMuted ? 0 : volume}
            onChange={handleVolumeChange}
          />
        </VolumeContainer>
      </ControlsRow>
    </PlayerContainer>
  );
};
