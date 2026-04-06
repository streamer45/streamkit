// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

export function normalizeMimeType(contentType: string): string {
  if (contentType.includes('codecs=')) {
    return contentType;
  }

  if (contentType.includes('audio/webm')) {
    return 'audio/webm; codecs="opus"';
  }
  if (contentType.includes('video/webm')) {
    return 'video/webm; codecs="vp9"';
  }
  if (contentType.includes('audio/mp4')) {
    return 'audio/mp4; codecs="mp4a.40.2"';
  }
  if (contentType.includes('video/mp4')) {
    return 'video/mp4; codecs="avc1.42c01f"';
  }

  return contentType;
}

export function canUseMseForMimeType(contentType: string): boolean {
  const mediaSourceCtor = (
    globalThis as { MediaSource?: { isTypeSupported?: (t: string) => boolean } }
  ).MediaSource;

  if (!mediaSourceCtor) {
    return false;
  }

  const normalized = normalizeMimeType(contentType);
  if (typeof mediaSourceCtor.isTypeSupported === 'function') {
    return mediaSourceCtor.isTypeSupported(normalized);
  }

  return true;
}
