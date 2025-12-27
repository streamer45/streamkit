// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Color Palette for StreamKit UI
 *
 * This file provides programmatic access to CSS custom properties defined in index.css.
 * For most use cases, prefer using CSS variables directly in styled-components.
 *
 * Usage in styled-components:
 * ```tsx
 * const StyledDiv = styled.div`
 *   color: var(--sk-text);
 *   background: var(--sk-panel-bg);
 * `;
 * ```
 *
 * Use this file when you need to:
 * - Access colors in JavaScript/TypeScript logic
 * - Generate dynamic styles based on theme
 * - Pass colors to third-party libraries that don't support CSS variables
 */

/**
 * Gets the computed value of a CSS custom property
 */
export function getCSSVariable(name: string): string {
  if (typeof window === 'undefined') return '';
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

/**
 * Color token names mapped to their CSS variable names
 */
export const ColorTokens = {
  // Base colors
  bg: '--sk-bg',
  sidebarBg: '--sk-sidebar-bg',
  panelBg: '--sk-panel-bg',
  text: '--sk-text',
  textMuted: '--sk-text-muted',
  textWhite: '--sk-text-white',
  textLight: '--sk-text-light',

  // Borders
  border: '--sk-border',
  borderStrong: '--sk-border-strong',

  // Primary colors
  primary: '--sk-primary',
  primaryContrast: '--sk-primary-contrast',

  // Semantic colors
  danger: '--sk-danger',
  success: '--sk-success',
  warning: '--sk-warning',
  info: '--sk-info',
  muted: '--sk-muted',

  // Interactive states
  hoverBg: '--sk-hover-bg',

  // Effects
  shadow: '--sk-shadow',
  focusRing: '--sk-focus-ring',

  // Status indicators
  statusInitializing: '--sk-status-initializing',
  statusRunning: '--sk-status-running',
  statusRecovering: '--sk-status-recovering',
  statusDegraded: '--sk-status-degraded',
  statusFailed: '--sk-status-failed',
  statusStopped: '--sk-status-stopped',

  // UI accents
  accentIndigo: '--sk-accent-indigo',
  accentIndigoLight: '--sk-accent-indigo-light',
  accentIndigoDark: '--sk-accent-indigo-dark',

  // Overlays
  overlayLight: '--sk-overlay-light',
  overlayMedium: '--sk-overlay-medium',
  overlayStrong: '--sk-overlay-strong',
} as const;

/**
 * Helper to get color value from CSS variable
 * @example getColor('primary') // returns computed value of --sk-primary
 */
export function getColor(token: keyof typeof ColorTokens): string {
  return getCSSVariable(ColorTokens[token]);
}

/**
 * Status state to color mapping
 */
export const StatusColors = {
  Initializing: ColorTokens.statusInitializing,
  Running: ColorTokens.statusRunning,
  Recovering: ColorTokens.statusRecovering,
  Degraded: ColorTokens.statusDegraded,
  Failed: ColorTokens.statusFailed,
  Stopped: ColorTokens.statusStopped,
} as const;

/**
 * Get the CSS variable name for a given status state
 */
export function getStatusColor(status: keyof typeof StatusColors): string {
  return StatusColors[status];
}

/**
 * Semantic color categories for easy reference
 */
export const SemanticColors = {
  success: ColorTokens.success,
  error: ColorTokens.danger,
  warning: ColorTokens.warning,
  info: ColorTokens.info,
} as const;
