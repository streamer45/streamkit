// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { afterEach, describe, expect, it, vi } from 'vitest';

import { canUseMseForMimeType, isFragmentedMp4, normalizeMimeType } from './mse';

function mp4Box(type: string, body: Uint8Array): Uint8Array {
  const out = new Uint8Array(8 + body.length);
  new DataView(out.buffer).setUint32(0, out.length);
  for (let i = 0; i < 4; i++) out[4 + i] = type.charCodeAt(i);
  out.set(body, 8);
  return out;
}

function concatBytes(...parts: Uint8Array[]): Uint8Array {
  const total = parts.reduce((n, p) => n + p.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const p of parts) {
    out.set(p, offset);
    offset += p.length;
  }
  return out;
}

function streamOf(bytes: Uint8Array, chunkSize = bytes.length): ReadableStream<Uint8Array> {
  return new ReadableStream({
    start(controller) {
      for (let i = 0; i < bytes.length; i += chunkSize) {
        controller.enqueue(bytes.slice(i, i + chunkSize));
      }
      controller.close();
    },
  });
}

const FTYP = mp4Box('ftyp', new TextEncoder().encode('isom'));

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
    // Pins the current contract: when MediaSource is present but feature-detection
    // is unavailable, mse.ts optimistically returns true (any MIME type). If that
    // policy is ever tightened to "unknown support => false", this test must update.
    vi.stubGlobal('MediaSource', {});
    expect(canUseMseForMimeType('audio/mp4')).toBe(true);
  });
});

describe('isFragmentedMp4', () => {
  it('returns true when moov contains an mvex box (fragmented init segment)', async () => {
    const moov = mp4Box(
      'moov',
      concatBytes(mp4Box('mvhd', new Uint8Array(8)), mp4Box('mvex', new Uint8Array(0)))
    );
    const bytes = concatBytes(FTYP, moov);
    expect(await isFragmentedMp4(streamOf(bytes))).toBe(true);
  });

  it('returns true when a top-level moof box is present', async () => {
    const bytes = concatBytes(FTYP, mp4Box('moof', new Uint8Array(16)));
    expect(await isFragmentedMp4(streamOf(bytes))).toBe(true);
  });

  it('returns false for a regular moov+mdat file (unfragmented)', async () => {
    const moov = mp4Box('moov', mp4Box('mvhd', new Uint8Array(8)));
    const bytes = concatBytes(FTYP, moov, mp4Box('mdat', new Uint8Array(64)));
    expect(await isFragmentedMp4(streamOf(bytes))).toBe(false);
  });

  it('returns false when mdat precedes moov (moov-at-end layout) without reading the mdat payload', async () => {
    const bytes = concatBytes(FTYP, mp4Box('mdat', new Uint8Array(64)));
    expect(await isFragmentedMp4(streamOf(bytes))).toBe(false);
  });

  it('classifies correctly when boxes are split across stream chunk boundaries', async () => {
    const moov = mp4Box(
      'moov',
      concatBytes(mp4Box('mvhd', new Uint8Array(8)), mp4Box('mvex', new Uint8Array(0)))
    );
    const bytes = concatBytes(FTYP, moov);
    expect(await isFragmentedMp4(streamOf(bytes, 3))).toBe(true);
  });

  it('returns false for a truncated/empty stream', async () => {
    expect(await isFragmentedMp4(streamOf(new Uint8Array(0)))).toBe(false);
  });
});
