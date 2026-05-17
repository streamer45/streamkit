// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { afterEach, describe, expect, it, vi } from 'vitest';

import { canUseMseForMimeType, normalizeMimeType } from './mse';

describe('normalizeMimeType', () => {
  it('returns the input unchanged when it already includes a codecs= parameter', () => {
    const input = 'video/webm; codecs="vp9,opus"';
    expect(normalizeMimeType(input)).toBe(input);
  });

  it('adds opus codec for bare audio/webm', () => {
    expect(normalizeMimeType('audio/webm')).toBe('audio/webm; codecs="opus"');
  });

  it('adds vp9 codec for bare video/webm', () => {
    expect(normalizeMimeType('video/webm')).toBe('video/webm; codecs="vp9"');
  });

  it('adds AAC-LC codec for bare audio/mp4', () => {
    expect(normalizeMimeType('audio/mp4')).toBe('audio/mp4; codecs="mp4a.40.2"');
  });

  it('adds AVC baseline codec for bare video/mp4', () => {
    expect(normalizeMimeType('video/mp4')).toBe('video/mp4; codecs="avc1.42c01f"');
  });

  it('falls through to return the original string for unknown content types', () => {
    expect(normalizeMimeType('application/octet-stream')).toBe('application/octet-stream');
  });

  it('returns audio/webm normalization when a charset parameter is present (substring match)', () => {
    // Implementation uses includes(), so any string containing 'audio/webm' is normalized.
    expect(normalizeMimeType('audio/webm; charset=utf-8')).toBe('audio/webm; codecs="opus"');
  });
});

describe('canUseMseForMimeType', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('returns false when MediaSource is unavailable on globalThis', () => {
    vi.stubGlobal('MediaSource', undefined);
    expect(canUseMseForMimeType('video/mp4')).toBe(false);
  });

  it('returns true when MediaSource.isTypeSupported approves the normalized type', () => {
    const isTypeSupported = vi.fn().mockReturnValue(true);
    vi.stubGlobal('MediaSource', { isTypeSupported });

    expect(canUseMseForMimeType('audio/webm')).toBe(true);
    expect(isTypeSupported).toHaveBeenCalledWith('audio/webm; codecs="opus"');
  });

  it('passes through types that already include a codecs= parameter without re-normalizing', () => {
    const isTypeSupported = vi.fn().mockReturnValue(true);
    vi.stubGlobal('MediaSource', { isTypeSupported });

    const input = 'video/mp4; codecs="avc1.640028,mp4a.40.2"';
    expect(canUseMseForMimeType(input)).toBe(true);
    expect(isTypeSupported).toHaveBeenCalledWith(input);
  });

  it('returns false when MediaSource.isTypeSupported rejects the type', () => {
    const isTypeSupported = vi.fn().mockReturnValue(false);
    vi.stubGlobal('MediaSource', { isTypeSupported });

    expect(canUseMseForMimeType('video/x-unknown')).toBe(false);
    expect(isTypeSupported).toHaveBeenCalledWith('video/x-unknown');
  });

  it('returns true when MediaSource exists but has no isTypeSupported function', () => {
    vi.stubGlobal('MediaSource', {});
    expect(canUseMseForMimeType('audio/mp4')).toBe(true);
  });
});
