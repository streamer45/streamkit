// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Shared pipeline YAML fixtures for compositor E2E tests.
 *
 * Each fixture is stored as a standalone `.yaml` file under `e2e/fixtures/`
 * and loaded at import time via `fs.readFileSync`.  This keeps the YAML
 * diffable, syntax-highlighted, and easy to extend without bloating the
 * TypeScript module.
 */

import { readFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const fixturesDir = resolve(__dirname, '..', 'fixtures');

function loadFixture(name: string): string {
  return readFileSync(resolve(fixturesDir, name), 'utf-8').trim();
}

/**
 * Webcam PiP compositor pipeline YAML.
 *
 * Composites the user's webcam as picture-in-picture over colorbars with a
 * text overlay.  Used by all compositor E2E tests.
 */
export const WEBCAM_PIP_YAML = loadFixture('webcam-pip.yaml');

/**
 * Webcam PiP compositor pipeline with crop/zoom on the PiP layer.
 *
 * Same as {@link WEBCAM_PIP_YAML} but the `in_1` layer has crop_zoom=2.0
 * (2× zoom), crop_x=0.3, crop_y=0.7 to exercise the virtual PTZ controls.
 */
export const WEBCAM_PIP_CROPPED_YAML = loadFixture('webcam-pip-cropped.yaml');

/**
 * Webcam PiP compositor pipeline with circular crop on the PiP layer.
 *
 * Same as {@link WEBCAM_PIP_CROPPED_YAML} but the `in_1` layer uses
 * crop_shape=circle with a square rect for a perfect circle PiP overlay
 * (Loom-style).
 */
export const WEBCAM_PIP_CIRCLE_YAML = loadFixture('webcam-pip-circle.yaml');

/**
 * Two-colorbars compositor pipeline — no webcam or MoQ peer needed.
 *
 * Two colorbars sources composited together (PiP layout) and streamed
 * via a one-way MoQ push.  Useful for tests that need to verify the
 * compositor produces video output without requiring a WebTransport
 * publish connection from the browser.
 */
export const COMPOSITOR_COLORBARS_YAML = loadFixture('compositor-colorbars.yaml');
