// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Shared constants and default values for the compositor layer system.
 *
 * These values match the Rust backend defaults (config.rs) and are used
 * across compositorOverlays, compositorLayerParsers, and related modules
 * to avoid duplication.
 */

// ── Layer kind type ─────────────────────────────────────────────────────────

/** Which category a layer belongs to for drag commit routing */
export type LayerKind = 'video' | 'text' | 'image';

// ── Default spatial / visual values ─────────────────────────────────────────

export const DEFAULT_OPACITY = 1.0;
export const DEFAULT_ROTATION_DEGREES = 0;
export const DEFAULT_Z_INDEX = 0;
export const DEFAULT_MIRROR_HORIZONTAL = false;
export const DEFAULT_MIRROR_VERTICAL = false;
export const DEFAULT_VISIBLE = true;

// ── Default crop / zoom values ──────────────────────────────────────────────

export const DEFAULT_CROP_ZOOM = 1.0;
export const DEFAULT_CROP_X = 0.5;
export const DEFAULT_CROP_Y = 0.5;
export const DEFAULT_CROP_SHAPE: 'rect' | 'circle' = 'rect';

// ── Default text overlay values ─────────────────────────────────────────────

export const DEFAULT_FONT_SIZE = 24;
export const DEFAULT_FONT_NAME = 'samples/fonts/system/DejaVuSans.ttf';
export const DEFAULT_FONT_WEIGHT = 400;
export const DEFAULT_TEXT_COLOR: [number, number, number, number] = [255, 255, 255, 255];

// ── Default overlay positioning ─────────────────────────────────────────────

export const DEFAULT_OVERLAY_X = 40;
export const DEFAULT_OVERLAY_Y_BASE = 40;
export const DEFAULT_OVERLAY_Y_STEP = 50;
export const DEFAULT_TEXT_WIDTH = 300;
export const DEFAULT_TEXT_HEIGHT = 60;
