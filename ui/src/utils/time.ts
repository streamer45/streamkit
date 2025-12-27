// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Format uptime from a timestamp to a human-readable string with fixed width
 */
export function formatUptime(createdAt: string): string {
  const start = new Date(createdAt);
  const now = new Date();
  const diffMs = now.getTime() - start.getTime();

  const seconds = Math.floor(diffMs / 1000);
  const minutes = Math.floor(seconds / 60);
  const hours = Math.floor(minutes / 60);
  const days = Math.floor(hours / 24);

  if (days > 0) {
    const h = String(hours % 24).padStart(2, '0');
    const m = String(minutes % 60).padStart(2, '0');
    return `${days}d ${h}h ${m}m`;
  }
  if (hours > 0) {
    const h = String(hours).padStart(2, '0');
    const m = String(minutes % 60).padStart(2, '0');
    const s = String(seconds % 60).padStart(2, '0');
    return `${h}h ${m}m ${s}s`;
  }
  if (minutes > 0) {
    const m = String(minutes).padStart(2, '0');
    const s = String(seconds % 60).padStart(2, '0');
    return `${m}m ${s}s`;
  }
  return `${String(seconds).padStart(2, '0')}s`;
}

/**
 * Format a timestamp to a short time string
 */
export function formatTime(timestamp: string): string {
  const date = new Date(timestamp);
  return date.toLocaleTimeString(undefined, {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  });
}

/**
 * Format a timestamp to a short date and time string
 */
export function formatDateTime(timestamp: string): string {
  const date = new Date(timestamp);
  return date.toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}
