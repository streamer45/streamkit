// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { Page } from '@playwright/test';

// Console error substrings that are always benign regardless of test context.
// ResizeObserver loop errors are a Chromium quirk; WebSocket reconnect chatter
// is expected when the dev-server hot-reloads.
const ALWAYS_BENIGN = ['ResizeObserver loop', 'WebSocket connection'];

// Additional benign patterns specific to MoQ/WebTransport tests.  These are
// intentionally narrow: we only suppress the *certificate-related* QUIC error
// (not the generic ERR_QUIC_PROTOCOL_ERROR) so that real transport failures
// still surface.
export const MOQ_BENIGN_PATTERNS = [
  'QUIC_TLS_CERTIFICATE_UNKNOWN',
  'Timed out connecting to MoQ gateway',
  "Failed to construct 'WebTransport'",
];

export interface ConsoleErrorCollector {
  readonly errors: string[];
  /** Clear all collected errors (useful between test phases). */
  reset(): void;
  /** Detach the console listener so subsequent teardown noise is ignored. */
  stop(): void;
  /** Return only errors that don't match any benign pattern. */
  getUnexpected(extraBenignPatterns?: string[]): string[];
}

/**
 * Attach a console-error listener to `page` and return a collector object.
 *
 * Typical usage:
 * 1. Call in `beforeEach` to start collecting.
 * 2. Assert with `getUnexpected()` once the interesting work is done.
 * 3. Call `stop()` *before* disconnect/destroy so that expected teardown
 *    errors (e.g. WebTransportError "The session is closed") are not captured.
 */
export function createConsoleErrorCollector(page: Page): ConsoleErrorCollector {
  const errors: string[] = [];
  const handler = (msg: import('@playwright/test').ConsoleMessage) => {
    if (msg.type() === 'error') {
      errors.push(msg.text());
    }
  };
  page.on('console', handler);
  return {
    errors,
    reset() {
      errors.length = 0;
    },
    stop() {
      page.removeListener('console', handler);
    },
    getUnexpected(extraBenignPatterns: string[] = []) {
      const allPatterns = [...ALWAYS_BENIGN, ...extraBenignPatterns];
      return errors.filter((msg) => !allPatterns.some((p) => msg.includes(p)));
    },
  };
}

/**
 * Run inside the browser to verify the `<audio>` element actually loaded media.
 *
 * The function waits for `loadedmetadata` (readyState >= 1) so that `duration`
 * is available, then attempts `audio.play()`.  Autoplay may be blocked by the
 * browser policy — that is fine; we mainly care that `duration > 0`, which
 * proves the pipeline produced valid audio output.
 */
export async function verifyAudioPlayback(page: Page): Promise<{
  found: boolean;
  duration: number;
  currentTime: number;
  paused: boolean;
  readyState: number;
}> {
  return page.evaluate(async () => {
    const audio = document.querySelector('audio') as HTMLAudioElement | null;
    if (!audio)
      return {
        found: false,
        duration: 0,
        currentTime: 0,
        paused: true,
        readyState: 0,
      };

    // Wait for the browser to parse enough of the media to know its duration.
    if (audio.readyState < 1) {
      await new Promise<void>((resolve, reject) => {
        const timeout = setTimeout(() => reject(new Error('Audio metadata timeout')), 10_000);
        const done = () => {
          clearTimeout(timeout);
          resolve();
        };
        audio.addEventListener('loadedmetadata', done, { once: true });
        // Guard against a race where readyState advanced before the listener attached.
        if (audio.readyState >= 1) done();
      });
    }

    try {
      await audio.play();
    } catch {
      // Autoplay may be blocked by browser policy; that is acceptable.
    }

    // Brief pause to let currentTime advance if playback started.
    await new Promise((r) => setTimeout(r, 500));

    return {
      found: true,
      duration: audio.duration,
      currentTime: audio.currentTime,
      paused: audio.paused,
      readyState: audio.readyState,
    };
  });
}

/**
 * Monkey-patch `AudioContext` before the page loads so that every instance
 * created by the app (e.g. by Hang's `Watch.Audio.Emitter`) is recorded in
 * `window.__testAudioContexts`.  This must be called *before* navigating to
 * the page (typically in `beforeEach`) because `addInitScript` runs before
 * any page script.
 *
 * Paired with {@link verifyAudioContextActive} to assert audio is actually
 * being decoded and played during MoQ streaming tests.
 */
export async function installAudioContextTracker(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const w = window as Window & { __testAudioContexts?: AudioContext[] };
    w.__testAudioContexts = [];
    const Orig = window.AudioContext;
    // Subclass preserves full AudioContext behaviour while recording every
    // instance so we can inspect state later from the test.
    window.AudioContext = class extends Orig {
      constructor(options?: AudioContextOptions) {
        super(options);
        w.__testAudioContexts!.push(this);
      }
    } as typeof AudioContext;
  });
}

/**
 * Query the tracked `AudioContext` instances (installed by
 * {@link installAudioContextTracker}) and return summary stats.
 *
 * A `running` count > 0 with `maxCurrentTime` > 0 proves the subscribe side
 * of the MoQ connection is actually receiving, decoding, and playing audio.
 */
export async function verifyAudioContextActive(page: Page): Promise<{
  count: number;
  running: number;
  maxCurrentTime: number;
}> {
  return page.evaluate(() => {
    const w = window as Window & { __testAudioContexts?: AudioContext[] };
    const contexts = w.__testAudioContexts ?? [];
    const running = contexts.filter((ctx) => ctx.state === 'running');
    return {
      count: contexts.length,
      running: running.length,
      maxCurrentTime: Math.max(0, ...contexts.map((ctx) => ctx.currentTime)),
    };
  });
}
