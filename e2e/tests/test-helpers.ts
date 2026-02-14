// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import type { Page } from "@playwright/test";

const ALWAYS_BENIGN = ["ResizeObserver loop", "WebSocket connection"];

export const MOQ_BENIGN_PATTERNS = [
  "QUIC_TLS_CERTIFICATE_UNKNOWN",
  "Timed out connecting to MoQ gateway",
  "Failed to construct 'WebTransport'",
];

export interface ConsoleErrorCollector {
  readonly errors: string[];
  reset(): void;
  getUnexpected(extraBenignPatterns?: string[]): string[];
}

export function createConsoleErrorCollector(page: Page): ConsoleErrorCollector {
  const errors: string[] = [];
  page.on("console", (msg) => {
    if (msg.type() === "error") {
      errors.push(msg.text());
    }
  });
  return {
    errors,
    reset() {
      errors.length = 0;
    },
    getUnexpected(extraBenignPatterns: string[] = []) {
      const allPatterns = [...ALWAYS_BENIGN, ...extraBenignPatterns];
      return errors.filter((msg) => !allPatterns.some((p) => msg.includes(p)));
    },
  };
}

export async function verifyAudioPlayback(page: Page): Promise<{
  found: boolean;
  duration: number;
  currentTime: number;
  paused: boolean;
  readyState: number;
}> {
  return page.evaluate(async () => {
    const audio = document.querySelector("audio") as HTMLAudioElement | null;
    if (!audio)
      return {
        found: false,
        duration: 0,
        currentTime: 0,
        paused: true,
        readyState: 0,
      };

    if (audio.readyState < 1) {
      await new Promise<void>((resolve, reject) => {
        const timeout = setTimeout(
          () => reject(new Error("Audio metadata timeout")),
          10_000,
        );
        const done = () => {
          clearTimeout(timeout);
          resolve();
        };
        audio.addEventListener("loadedmetadata", done, { once: true });
        if (audio.readyState >= 1) done();
      });
    }

    try {
      await audio.play();
    } catch {
      // Autoplay may be blocked; that is fine.
    }

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

export async function installAudioContextTracker(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const w = window as Window & { __testAudioContexts?: AudioContext[] };
    w.__testAudioContexts = [];
    const Orig = window.AudioContext;
    window.AudioContext = class extends Orig {
      constructor(options?: AudioContextOptions) {
        super(options);
        w.__testAudioContexts!.push(this);
      }
    } as typeof AudioContext;
  });
}

export async function verifyAudioContextActive(page: Page): Promise<{
  count: number;
  running: number;
  maxCurrentTime: number;
}> {
  return page.evaluate(() => {
    const w = window as Window & { __testAudioContexts?: AudioContext[] };
    const contexts = w.__testAudioContexts ?? [];
    const running = contexts.filter((ctx) => ctx.state === "running");
    return {
      count: contexts.length,
      running: running.length,
      maxCurrentTime: Math.max(0, ...contexts.map((ctx) => ctx.currentTime)),
    };
  });
}
