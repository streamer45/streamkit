// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Reusable "LIVE" indicator badge with a pulsing dot.
 *
 * Two sizes are provided:
 *
 * - **small** — compact variant for use inside flow-graph nodes
 *   (CompositorNode, AudioGainNode, ConfigurableNode).
 * - **default** — larger variant used in toolbar-level UI
 *   (MonitorView top-bar).
 */

import styled from '@emotion/styled';

// ── Pulsing dot ──────────────────────────────────────────────────────────────

export const LiveDot = styled.div<{ size?: 'small' | 'default' }>`
  width: ${(p) => (p.size === 'small' ? '4px' : '6px')};
  height: ${(p) => (p.size === 'small' ? '4px' : '6px')};
  border-radius: 50%;
  background: rgb(239, 68, 68);
  animation: live-pulse 2s ease-in-out infinite;
  flex-shrink: 0;

  @keyframes live-pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.5;
    }
  }
`;

// ── Badge wrapper ────────────────────────────────────────────────────────────

export const LiveBadge = styled.span<{ size?: 'small' | 'default' }>`
  display: inline-flex;
  align-items: center;
  gap: ${(p) => (p.size === 'small' ? '3px' : '4px')};
  padding: ${(p) => (p.size === 'small' ? '2px 5px' : '6px 10px')};
  background: rgba(239, 68, 68, 0.15);
  color: rgb(239, 68, 68);
  border: 1px solid rgba(239, 68, 68, 0.3);
  border-radius: ${(p) => (p.size === 'small' ? '3px' : '4px')};
  font-size: ${(p) => (p.size === 'small' ? '10px' : '13px')};
  font-weight: 600;
  letter-spacing: ${(p) => (p.size === 'small' ? '0.2px' : '0.3px')};
  line-height: 1;
  flex-shrink: 0;
  user-select: none;
`;
