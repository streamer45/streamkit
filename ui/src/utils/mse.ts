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

function readBoxType(bytes: Uint8Array, offset: number): string {
  return String.fromCharCode(
    bytes[offset],
    bytes[offset + 1],
    bytes[offset + 2],
    bytes[offset + 3]
  );
}

function moovHasMvex(moovBody: Uint8Array): boolean {
  let offset = 0;
  while (offset + 8 <= moovBody.length) {
    const view = new DataView(moovBody.buffer, moovBody.byteOffset + offset, 8);
    let size = view.getUint32(0);
    const type = readBoxType(moovBody, offset + 4);
    if (type === 'mvex') {
      return true;
    }
    if (size === 1) {
      if (offset + 16 > moovBody.length) break;
      const ext = new DataView(moovBody.buffer, moovBody.byteOffset + offset + 8, 8);
      size = ext.getUint32(0) * 2 ** 32 + ext.getUint32(4);
    }
    if (size < 8) break;
    offset += size;
  }
  return false;
}

/** Pull-based byte reader over a ReadableStream that supports reading exact
 *  counts and skipping ahead without buffering skipped bytes. */
class StreamByteReader {
  private reader: ReadableStreamDefaultReader<Uint8Array>;
  private buffer = new Uint8Array(0);
  private done = false;

  constructor(stream: ReadableStream<Uint8Array>) {
    this.reader = stream.getReader();
  }

  private async pull(): Promise<boolean> {
    if (this.done) return false;
    const { value, done } = await this.reader.read();
    if (done) {
      this.done = true;
      return false;
    }
    if (value) {
      const merged = new Uint8Array(this.buffer.length + value.length);
      merged.set(this.buffer, 0);
      merged.set(value, this.buffer.length);
      this.buffer = merged;
    }
    return true;
  }

  async read(count: number): Promise<Uint8Array | null> {
    while (this.buffer.length < count) {
      if (!(await this.pull())) return null;
    }
    const out = this.buffer.slice(0, count);
    this.buffer = this.buffer.slice(count);
    return out;
  }

  async skip(count: number): Promise<boolean> {
    let remaining = count;
    const fromBuffer = Math.min(remaining, this.buffer.length);
    this.buffer = this.buffer.slice(fromBuffer);
    remaining -= fromBuffer;
    while (remaining > 0) {
      if (!(await this.pull())) return false;
      const take = Math.min(remaining, this.buffer.length);
      this.buffer = this.buffer.slice(take);
      remaining -= take;
    }
    return true;
  }

  async cancel(): Promise<void> {
    try {
      await this.reader.cancel();
    } catch {
      // Ignore cancellation races.
    }
  }
}

const MAX_BOXES_BEFORE_MEDIA = 64;

/**
 * Inspect the leading boxes of an MP4 stream to decide whether it is
 * fragmented (fMP4). MSE can only play fragmented MP4 (a `moov` containing
 * `mvex`, followed by `moof`/`mdat` fragments); a regular `moov`+`mdat` file
 * (StreamKit's MP4 "file" mode) must be played natively via a blob URL.
 *
 * `MediaSource.isTypeSupported` only reflects codec support, not container
 * fragmentation, so this byte-level check is required to route playback.
 */
export async function isFragmentedMp4(stream: ReadableStream<Uint8Array>): Promise<boolean> {
  const bytes = new StreamByteReader(stream);
  try {
    for (let i = 0; i < MAX_BOXES_BEFORE_MEDIA; i++) {
      const header = await bytes.read(8);
      if (!header) return false;

      const view = new DataView(header.buffer, header.byteOffset, 8);
      let size = view.getUint32(0);
      const type = readBoxType(header, 4);
      let headerSize = 8;

      if (size === 1) {
        const ext = await bytes.read(8);
        if (!ext) return false;
        const extView = new DataView(ext.buffer, ext.byteOffset, 8);
        size = extView.getUint32(0) * 2 ** 32 + extView.getUint32(4);
        headerSize = 16;
      }

      if (type === 'moof' || type === 'styp') return true;
      if (type === 'mdat') return false;
      if (type === 'moov') {
        const body = await bytes.read(size - headerSize);
        if (!body) return false;
        return moovHasMvex(body);
      }
      if (size === 0) return false;
      if (!(await bytes.skip(size - headerSize))) return false;
    }
    return false;
  } finally {
    await bytes.cancel();
  }
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
