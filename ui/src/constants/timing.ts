// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Central timing constants for throttled server updates.
 *
 * These values balance responsiveness against network/server load.
 * They apply to all controls that send parameter changes to the server
 * during continuous interactions (slider drags, compositor layer edits, etc.).
 */

/**
 * Throttle interval for parameter updates sent to the server via WebSocket
 * during continuous interactions (slider drags, compositor opacity/rotation, etc.).
 *
 * 33ms ≈ 30 updates/sec — perceptually smooth for slider-driven changes
 * while leaving headroom for network RTT and server processing.
 */
export const PARAM_THROTTLE_MS = 33;

/**
 * Debounce delay for text input controls on node cards.
 *
 * 300ms gives the user time to finish typing before sending the value
 * to the server, avoiding excessive partial updates.
 */
export const TEXT_DEBOUNCE_MS = 300;
