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

import { useCallback, useSyncExternalStore } from 'react';

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

const noopUnsubscribe = () => {};

/** Subscribe to an audioEmitter's muted/volume signals and return React state. */
export function useAudioControls(audioEmitter: AudioEmitterLike | null) {
  const subscribeMuted = useCallback(
    (onChange: () => void) => audioEmitter?.muted.subscribe(onChange) ?? noopUnsubscribe,
    [audioEmitter]
  );
  const subscribeVolume = useCallback(
    (onChange: () => void) => audioEmitter?.volume.subscribe(onChange) ?? noopUnsubscribe,
    [audioEmitter]
  );

  const muted = useSyncExternalStore(subscribeMuted, () => audioEmitter?.muted.peek() ?? true);
  const volume = useSyncExternalStore(subscribeVolume, () => audioEmitter?.volume.peek() ?? 0.5);

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
