// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { expect, type Page } from '@playwright/test';

// Console error substrings that are always benign regardless of test context.
// ResizeObserver loop errors are a Chromium quirk; WebSocket reconnect chatter
// is expected when the dev-server hot-reloads.
const ALWAYS_BENIGN = ['ResizeObserver loop', 'WebSocket connection'];

// Additional benign patterns specific to MoQ/WebTransport tests.  In CI with
// self-signed certs and headless Chromium, WebTransport QUIC connections can
// fail non-deterministically.  The dedicated connectivity tests (stream.spec.ts
// tests 2 & 3) handle connection failures gracefully by skipping; these patterns
// prevent unrelated session-lifecycle tests from breaking on auto-connect noise.
export const MOQ_BENIGN_PATTERNS = [
  'QUIC_TLS_CERTIFICATE_UNKNOWN',
  'ERR_QUIC_PROTOCOL_ERROR',
  'Timed out connecting to MoQ gateway',
  "Failed to construct 'WebTransport'",
  'Failed to establish a connection',
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
 * Selects a sample pipeline in the TemplateSelector.
 *
 * Ungrouped cards expose a single radio whose accessible name is the sample
 * name. Grouped scenario cards expose one radio per variant whose accessible
 * name is the variant label (which matches its visible text per WCAG 2.5.3);
 * to disambiguate identically-labelled variants across groups, pass `variant`
 * and the lookup is scoped to that card's `"<name> variants"` group.
 */
export async function selectPipelineTemplate(
  page: Page,
  name: string,
  variant?: string
): Promise<void> {
  const radio = variant
    ? page.getByRole('group', { name: `${name} variants` }).getByRole('radio', {
        name: variant,
        exact: true,
      })
    : page.getByRole('radio', { name, exact: true });
  await expect(radio.first()).toBeVisible({ timeout: 10_000 });
  await radio.first().click();
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

/**
 * Run inside the browser to verify a `<video>` element produced by MSEPlayer
 * actually loaded media and is playing.
 *
 * Similar to {@link verifyAudioPlayback} but targets video elements and also
 * returns the intrinsic video dimensions (`videoWidth` / `videoHeight`), which
 * prove the decoder produced at least one frame.
 */
export async function verifyVideoPlayback(page: Page): Promise<{
  found: boolean;
  duration: number;
  currentTime: number;
  paused: boolean;
  readyState: number;
  videoWidth: number;
  videoHeight: number;
}> {
  return page.evaluate(async () => {
    const video = document.querySelector('video') as HTMLVideoElement | null;
    if (!video)
      return {
        found: false,
        duration: 0,
        currentTime: 0,
        paused: true,
        readyState: 0,
        videoWidth: 0,
        videoHeight: 0,
      };

    // Wait for the browser to parse enough of the media to know its dimensions.
    if (video.readyState < 1) {
      await new Promise<void>((resolve, reject) => {
        const timeout = setTimeout(() => reject(new Error('Video metadata timeout')), 15_000);
        const done = () => {
          clearTimeout(timeout);
          resolve();
        };
        video.addEventListener('loadedmetadata', done, { once: true });
        // Guard against a race where readyState advanced before the listener.
        if (video.readyState >= 1) done();
      });
    }

    try {
      await video.play();
    } catch {
      // Autoplay may be blocked by browser policy; that is acceptable.
    }

    // Brief pause to let currentTime advance if playback started.
    await new Promise((r) => setTimeout(r, 500));

    return {
      found: true,
      duration: video.duration,
      currentTime: video.currentTime,
      paused: video.paused,
      readyState: video.readyState,
      videoWidth: video.videoWidth,
      videoHeight: video.videoHeight,
    };
  });
}

/**
 * Verify that a `<canvas>` element is rendering non-black frames.
 *
 * Used by the MoQ video streaming test to confirm that
 * `Hang.Watch.Video.Renderer` is actually decoding and painting VP9
 * frames onto the canvas.
 *
 * Samples a small rectangle of pixel data and checks that at least one
 * pixel has a non-zero RGB value (i.e. not pure black / transparent).
 */
export async function verifyCanvasRendering(page: Page): Promise<{
  found: boolean;
  width: number;
  height: number;
  hasNonBlackPixels: boolean;
}> {
  return page.evaluate(() => {
    const canvas = document.querySelector('canvas') as HTMLCanvasElement | null;
    if (!canvas) return { found: false, width: 0, height: 0, hasNonBlackPixels: false };

    const width = canvas.width;
    const height = canvas.height;

    // Sample the center 10x10 region (or full canvas if smaller)
    const sampleW = Math.min(10, width);
    const sampleH = Math.min(10, height);
    const x = Math.floor((width - sampleW) / 2);
    const y = Math.floor((height - sampleH) / 2);

    const ctx = canvas.getContext('2d');
    if (!ctx) {
      // Canvas may be using a WebGL context (e.g. Hang.Watch.Video.Renderer).
      // Attempt to read pixels via WebGL/WebGL2 instead.
      const gl =
        (canvas.getContext('webgl2') as WebGL2RenderingContext | null) ??
        (canvas.getContext('webgl') as WebGLRenderingContext | null);
      if (!gl) return { found: true, width, height, hasNonBlackPixels: false };

      const pixels = new Uint8Array(sampleW * sampleH * 4);
      gl.readPixels(x, y, sampleW, sampleH, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
      let hasNonBlackPixels = false;
      for (let i = 0; i < pixels.length; i += 4) {
        if (pixels[i] > 0 || pixels[i + 1] > 0 || pixels[i + 2] > 0) {
          hasNonBlackPixels = true;
          break;
        }
      }
      return { found: true, width, height, hasNonBlackPixels };
    }

    const imageData = ctx.getImageData(x, y, sampleW, sampleH);
    const data = imageData.data;

    // Check if any pixel has a non-zero R, G, or B value.
    let hasNonBlackPixels = false;
    for (let i = 0; i < data.length; i += 4) {
      if (data[i] > 0 || data[i + 1] > 0 || data[i + 2] > 0) {
        hasNonBlackPixels = true;
        break;
      }
    }

    return { found: true, width, height, hasNonBlackPixels };
  });
}
