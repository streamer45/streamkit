// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Hook that subscribes to an audio emitter's muted/volume reactive signals
 * and returns React state + callbacks for controlling playback.
 *
 * Used by both `OutputPreviewPanel` (Monitor View) and `StreamView` to
 * provide consistent volume controls wherever a MoQ audio stream is played.
 */

import { useCallback, useEffect, useState } from 'react';

/** Minimal shape of the audio emitter expected by the hook. */
export interface AudioEmitterLike {
  muted: {
    peek(): boolean;
    set(v: boolean): void;
    subscribe(fn: (v: boolean) => void): () => void;
  };
  volume: {
    peek(): number;
    set(v: number): void;
    subscribe(fn: (v: number) => void): () => void;
  };
}

/** Subscribe to an audioEmitter's muted/volume signals and return React state. */
export function useAudioControls(audioEmitter: AudioEmitterLike | null) {
  const [muted, setMuted] = useState(() => audioEmitter?.muted.peek() ?? true);
  const [volume, setVolume] = useState(() => audioEmitter?.volume.peek() ?? 0.5);

  useEffect(() => {
    if (!audioEmitter) return;
    setMuted(audioEmitter.muted.peek());
    setVolume(audioEmitter.volume.peek());
    const unsubMuted = audioEmitter.muted.subscribe(setMuted);
    const unsubVolume = audioEmitter.volume.subscribe(setVolume);
    return () => {
      unsubMuted();
      unsubVolume();
    };
  }, [audioEmitter]);

  const toggleMute = useCallback(() => {
    if (!audioEmitter) return;
    audioEmitter.muted.set(!audioEmitter.muted.peek());
  }, [audioEmitter]);

  const changeVolume = useCallback(
    (v: number) => {
      if (!audioEmitter) return;
      audioEmitter.volume.set(v);
      // Un-mute when the user drags the slider up from zero.
      if (v > 0 && audioEmitter.muted.peek()) {
        audioEmitter.muted.set(false);
      }
    },
    [audioEmitter]
  );

  return { muted, volume, toggleMute, changeVolume };
}
