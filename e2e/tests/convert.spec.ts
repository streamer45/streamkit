// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import * as fs from "fs";
import * as path from "path";

import { test, expect } from "@playwright/test";

import { ensureLoggedIn, getAuthHeaders } from "./auth-helpers";
import {
  type ConsoleErrorCollector,
  createConsoleErrorCollector,
  verifyAudioPlayback,
} from "./test-helpers";

const repoRoot = path.resolve(import.meta.dirname, "..", "..");
const sampleOggPath = path.join(
  repoRoot,
  "samples",
  "audio",
  "system",
  "sample.ogg",
);
const mixingYaml = fs.readFileSync(
  path.join(repoRoot, "samples", "pipelines", "oneshot", "mixing.yml"),
  "utf8",
);

test.describe("Convert View - Audio Mixing Pipeline", () => {
  let collector: ConsoleErrorCollector;

  test.beforeEach(async ({ page }) => {
    collector = createConsoleErrorCollector(page);
    await page.goto("/convert");
    await ensureLoggedIn(page);
    if (!page.url().includes("/convert")) {
      await page.goto("/convert");
    }
    await expect(page.getByTestId("convert-view")).toBeVisible();
  });

  test("API: POST /api/v1/process with mixing pipeline returns audio", async ({
    page,
    baseURL,
  }) => {
    const audioBase64 = fs.readFileSync(sampleOggPath).toString("base64");
    const authHeaders = getAuthHeaders();

    const result = await page.evaluate(
      async ({ url, yaml, audio, headers }) => {
        const formData = new FormData();
        formData.append("config", yaml);
        const bytes = Uint8Array.from(atob(audio), (c) => c.charCodeAt(0));
        formData.append(
          "media",
          new Blob([bytes], { type: "audio/ogg" }),
          "sample.ogg",
        );

        const controller = new AbortController();
        const timeoutId = setTimeout(() => controller.abort(), 30_000);

        try {
          const response = await fetch(`${url}/api/v1/process`, {
            method: "POST",
            body: formData,
            headers,
            signal: controller.signal,
          });

          const contentType = response.headers.get("content-type") ?? "";
          const reader = response.body!.getReader();
          const { value } = await reader.read();
          reader.cancel();

          return {
            status: response.status,
            contentType,
            firstChunkSize: value?.length ?? 0,
          };
        } finally {
          clearTimeout(timeoutId);
        }
      },
      {
        url: baseURL,
        yaml: mixingYaml,
        audio: audioBase64,
        headers: authHeaders,
      },
    );

    expect(result.status, `Process request failed: ${result.status}`).toBe(200);
    expect(
      result.contentType.includes("audio/") ||
        result.contentType.includes("video/webm") ||
        result.contentType.includes("application/octet"),
      `Unexpected Content-Type: ${result.contentType}`,
    ).toBeTruthy();
    expect(result.firstChunkSize).toBeGreaterThan(0);
  });

  test("UI: select mixing template, upload file, convert, verify audio player", async ({
    page,
  }) => {
    await expect(page.getByText("1. Select Pipeline Template")).toBeVisible();

    const templateCard = page.getByText("Audio Mixing (Upload + Music Track)", {
      exact: true,
    });
    await expect(templateCard).toBeVisible({ timeout: 10_000 });
    await templateCard.click();

    await expect(page.locator('input[type="file"]').first()).toBeAttached();
    await page
      .locator('input[type="file"]')
      .first()
      .setInputFiles(sampleOggPath);

    await expect(page.getByText("sample.ogg")).toBeVisible();

    const convertButton = page.getByRole("button", { name: /Convert File/i });
    await expect(convertButton).toBeEnabled();
    await convertButton.click();

    await expect(page.getByText("Converted Audio")).toBeVisible({
      timeout: 60_000,
    });

    const playback = await verifyAudioPlayback(page);
    expect(playback.found, "Audio element not found on page").toBe(true);
    expect(playback.duration, "Audio has no duration").toBeGreaterThan(0);

    const unexpected = collector.getUnexpected();
    expect(
      unexpected,
      `Unexpected console errors: ${unexpected.join("; ")}`,
    ).toHaveLength(0);
  });

  test("UI: select mixing template, use existing asset, convert, verify audio player", async ({
    page,
  }) => {
    await expect(page.getByText("1. Select Pipeline Template")).toBeVisible();

    const templateCard = page.getByText("Audio Mixing (Upload + Music Track)", {
      exact: true,
    });
    await expect(templateCard).toBeVisible({ timeout: 10_000 });
    await templateCard.click();

    const assetModeButton = page.getByRole("button", {
      name: /Select Existing Asset/i,
    });
    await expect(assetModeButton).toBeVisible();
    await assetModeButton.click();

    const assetRadioGroup = page.locator(
      '[aria-label="Audio asset selection"]',
    );
    await expect(assetRadioGroup).toBeVisible({ timeout: 10_000 });

    const firstAsset = assetRadioGroup.locator("label").first();
    await expect(firstAsset).toBeVisible();
    await firstAsset.click();

    const convertButton = page.getByRole("button", { name: /Convert File/i });
    await expect(convertButton).toBeEnabled();
    await convertButton.click();

    await expect(page.getByText("Converted Audio")).toBeVisible({
      timeout: 60_000,
    });

    const playback = await verifyAudioPlayback(page);
    expect(playback.found, "Audio element not found on page").toBe(true);
    expect(playback.duration, "Audio has no duration").toBeGreaterThan(0);

    const unexpected = collector.getUnexpected();
    expect(
      unexpected,
      `Unexpected console errors: ${unexpected.join("; ")}`,
    ).toHaveLength(0);
  });
});
