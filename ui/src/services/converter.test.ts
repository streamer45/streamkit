// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';

import { convertFile, getExtensionFromContentType } from './converter';
import { FTYP, concatBytes, mp4Box, streamOf } from '../test/mp4Fixtures';

const FRAGMENTED_MP4_HEAD = concatBytes(FTYP, mp4Box('moov', mp4Box('mvex', new Uint8Array(0))));
const UNFRAGMENTED_MP4_HEAD = concatBytes(FTYP, mp4Box('mdat', new Uint8Array(32)));

// Each clone() returns an independent stream rather than modelling real
// Response.clone() tee semantics (one shared source). That is sufficient for
// routing assertions but means these tests cannot exercise tee back-pressure;
// the probe-error case below drives the body factory to a rejecting stream.
function mp4Response(
  bytes: Uint8Array,
  contentType: string,
  makeBody: () => ReadableStream<Uint8Array> = () => streamOf(bytes)
): Response {
  const blob = new Blob([], { type: contentType });
  return {
    ok: true,
    headers: new Headers({ 'Content-Type': contentType }),
    body: makeBody(),
    clone: () => mp4Response(bytes, contentType, makeBody),
    blob: vi.fn().mockResolvedValue(blob),
  } as unknown as Response;
}

// Mock document if not defined (for download tests)
if (typeof document === 'undefined') {
  global.document = {
    querySelector: vi.fn(),
    createElement: vi.fn(),
    body: {
      appendChild: vi.fn(),
      removeChild: vi.fn(),
    },
  } as never;
}

// Mock dependencies
vi.mock('@/utils/logger', () => ({
  getLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    error: vi.fn(),
  }),
}));

vi.mock('./base', () => ({
  getApiUrl: () => 'http://localhost:4545',
  fetchApi: (path: string, options: RequestInit = {}) => {
    const normalized = path.startsWith('/') ? path : `/${path}`;
    return fetch(`http://localhost:4545${normalized}`, { ...options, credentials: 'include' });
  },
}));

describe('converter service', () => {
  const MOCK_YAML = 'steps:\n  - id: test\n    kind: core::passthrough';
  const MOCK_FILE = new File(['test content'], 'test.ogg', { type: 'audio/ogg' });
  const MOCK_UPLOAD = [{ field: 'media', file: MOCK_FILE }];

  beforeEach(() => {
    global.fetch = vi.fn() as never;
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  describe('convertFile - Streaming (JSON)', () => {
    it('should handle JSON streaming for transcription', async () => {
      const mockBody = new ReadableStream({
        start(controller) {
          controller.enqueue(new TextEncoder().encode('{"text": "hello"}\n'));
          controller.close();
        },
      });

      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
        ok: true,
        headers: new Headers({ 'Content-Type': 'application/json' }),
        body: mockBody,
      } as Response);

      const result = await convertFile(MOCK_YAML, MOCK_UPLOAD, 'playback');

      expect(result.success).toBe(true);
      expect(result.useStreaming).toBe(true);
      expect(result.responseStream).toBeDefined();
      expect(result.contentType).toBe('application/json');
    });

    it('should wrap JSON stream with proper cancellation handling', async () => {
      const mockReader = {
        read: vi
          .fn()
          .mockResolvedValueOnce({ done: false, value: new Uint8Array([1, 2, 3]) })
          .mockResolvedValueOnce({ done: true, value: undefined }),
        cancel: vi.fn().mockResolvedValue(undefined),
      };

      const mockBody = {
        getReader: () => mockReader,
      };

      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
        ok: true,
        headers: new Headers({ 'Content-Type': 'application/json' }),
        body: mockBody as never,
      } as unknown as Response);

      const abortController = new AbortController();
      const result = await convertFile(MOCK_YAML, MOCK_UPLOAD, 'playback', abortController.signal);

      expect(result.success).toBe(true);
      expect(result.responseStream).toBeDefined();

      // The wrapped stream supports cancellation via the returned reader
      // (abort signal cancellation is tested in other tests)
    });
  });

  describe('convertFile - MSE Streaming (WebM)', () => {
    it('should handle WebM streaming for MSE playback', async () => {
      vi.stubGlobal('MediaSource', {
        isTypeSupported: vi.fn().mockReturnValue(true),
      });

      const mockBody = new ReadableStream({
        start(controller) {
          controller.enqueue(new Uint8Array([0x1a, 0x45, 0xdf, 0xa3])); // WebM header
          controller.close();
        },
      });

      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
        ok: true,
        headers: new Headers({ 'Content-Type': 'audio/webm' }),
        body: mockBody,
      } as Response);

      const result = await convertFile(MOCK_YAML, MOCK_UPLOAD, 'playback');

      expect(result.success).toBe(true);
      expect(result.useStreaming).toBe(true);
      expect(result.responseStream).toBeDefined();
      expect(result.contentType).toBe('audio/webm');
    });

    it('should wrap WebM stream with cancellation support', async () => {
      vi.stubGlobal('MediaSource', {
        isTypeSupported: vi.fn().mockReturnValue(true),
      });

      const mockReader = {
        read: vi.fn().mockResolvedValue({ done: true, value: undefined }),
        cancel: vi.fn().mockResolvedValue(undefined),
      };

      const mockBody = {
        getReader: () => mockReader,
      };

      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
        ok: true,
        headers: new Headers({ 'Content-Type': 'video/webm' }),
        body: mockBody as never,
      } as unknown as Response);

      const abortController = new AbortController();
      const result = await convertFile(MOCK_YAML, MOCK_UPLOAD, 'playback', abortController.signal);

      expect(result.responseStream).toBeDefined();

      // Stream should be cancellable without throwing
      if (result.responseStream) {
        const reader = result.responseStream.getReader();
        await expect(reader.cancel()).resolves.toBeUndefined();
      }
    });
  });

  // Regression for the AV1 WebM oneshot streaming bug: the muxer used to
  // advertise the bare `codecs="av1"` token, which real browsers reject via
  // MediaSource.isTypeSupported, silently demoting progressive MSE playback to
  // full-response blob buffering. These tests emulate Chrome's acceptance (bare
  // `av1` rejected, RFC 6381 `av01.P.LLT.DD` accepted) and pin the routing.
  describe('convertFile - WebM AV1 codec-string routing', () => {
    // Mirrors Chrome: only `opus`, `vp9`, and full `av01.*` codec tokens are
    // valid; the bare `av1` token is not.
    function browserIsTypeSupported(type: string): boolean {
      const codecs = type.match(/codecs="([^"]*)"/)?.[1]?.split(',') ?? [];
      return codecs.every((c) => c === 'opus' || c === 'vp9' || /^av01\./.test(c));
    }

    function webmResponse(contentType: string): Response {
      const body = new ReadableStream({
        start(controller) {
          controller.enqueue(new Uint8Array([0x1a, 0x45, 0xdf, 0xa3]));
          controller.close();
        },
      });
      return {
        ok: true,
        headers: new Headers({ 'Content-Type': contentType }),
        body,
        blob: vi.fn().mockResolvedValue(new Blob([], { type: contentType })),
      } as unknown as Response;
    }

    it('uses MSE streaming for a valid av01 WebM codec string', async () => {
      vi.stubGlobal('MediaSource', { isTypeSupported: browserIsTypeSupported });
      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(
        webmResponse('video/webm; codecs="av01.0.08M.08"')
      );

      const result = await convertFile(MOCK_YAML, MOCK_UPLOAD, 'playback');

      expect(result.useStreaming).toBe(true);
      expect(result.responseStream).toBeDefined();
    });

    it('falls back to blob playback for the invalid bare "av1" codec string', async () => {
      vi.stubGlobal('MediaSource', { isTypeSupported: browserIsTypeSupported });
      global.URL.createObjectURL = vi.fn().mockReturnValue('blob:mock-url');
      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(
        webmResponse('video/webm; codecs="av1"')
      );

      const result = await convertFile(MOCK_YAML, MOCK_UPLOAD, 'playback');

      expect(result.useStreaming).toBe(false);
      expect(result.responseStream).toBeUndefined();
    });
  });

  describe('convertFile - Blob Playback (Fallback)', () => {
    it('should fall back to blob playback for non-streaming formats', async () => {
      const mockBlob = new Blob(['audio data'], { type: 'audio/ogg' });
      global.URL.createObjectURL = vi.fn().mockReturnValue('blob:mock-url');

      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
        ok: true,
        headers: new Headers({ 'Content-Type': 'audio/ogg' }),
        blob: vi.fn().mockResolvedValue(mockBlob),
      } as never);

      const result = await convertFile(MOCK_YAML, MOCK_UPLOAD, 'playback');

      expect(result.success).toBe(true);
      expect(result.useStreaming).toBe(false);
      expect(result.mediaUrl).toBe('blob:mock-url');
      expect(result.contentType).toBe('audio/ogg');
    });
  });

  describe('convertFile - Download Mode', () => {
    it('should trigger browser download in download mode', async () => {
      const mockBlob = new Blob(['audio data'], { type: 'audio/opus' });
      global.URL.createObjectURL = vi.fn().mockReturnValue('blob:download-url');
      global.URL.revokeObjectURL = vi.fn();

      const mockLink = {
        href: '',
        download: '',
        click: vi.fn(),
      };
      const appendChildSpy = vi
        .spyOn(document.body, 'appendChild')
        .mockImplementation(() => null as never);
      const removeChildSpy = vi
        .spyOn(document.body, 'removeChild')
        .mockImplementation(() => null as never);
      const realCreate = document.createElement.bind(document);
      vi.spyOn(document, 'createElement').mockImplementation((tag: string) =>
        tag === 'a' ? (mockLink as never) : realCreate(tag)
      );

      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
        ok: true,
        headers: new Headers({ 'Content-Type': 'audio/opus' }),
        blob: vi.fn().mockResolvedValue(mockBlob),
      } as never);

      const result = await convertFile(MOCK_YAML, MOCK_UPLOAD, 'download');

      expect(result.success).toBe(true);
      expect(mockLink.href).toBe('blob:download-url');
      expect(mockLink.download).toBe('test_converted.opus');
      expect(mockLink.click).toHaveBeenCalled();
      expect(appendChildSpy).toHaveBeenCalled();
      expect(removeChildSpy).toHaveBeenCalled();

      // URL cleanup happens asynchronously (timing not critical for test)
    });

    it('should generate filename from original file', async () => {
      const mockBlob = new Blob(['audio data']);
      const mockLink = {
        href: '',
        download: '',
        click: vi.fn(),
      };

      vi.spyOn(document.body, 'appendChild').mockImplementation(() => null as never);
      vi.spyOn(document.body, 'removeChild').mockImplementation(() => null as never);
      const realCreate = document.createElement.bind(document);
      vi.spyOn(document, 'createElement').mockImplementation((tag: string) =>
        tag === 'a' ? (mockLink as never) : realCreate(tag)
      );
      global.URL.createObjectURL = vi.fn().mockReturnValue('blob:url');

      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
        ok: true,
        headers: new Headers({ 'Content-Type': 'audio/wav' }),
        blob: vi.fn().mockResolvedValue(mockBlob),
      } as never);

      const file = new File(['content'], 'my-audio.ogg', { type: 'audio/ogg' });
      await convertFile(MOCK_YAML, [{ field: 'media', file }], 'download');

      expect(mockLink.download).toBe('my-audio_converted.wav');
    });
  });

  describe('convertFile - Abort Handling', () => {
    it('should support aborting request with AbortSignal', async () => {
      const abortController = new AbortController();

      (fetch as ReturnType<typeof vi.fn>).mockImplementation(() => {
        return new Promise((_, reject) => {
          abortController.signal.addEventListener('abort', () => {
            reject(new DOMException('Aborted', 'AbortError'));
          });
        });
      });

      const resultPromise = convertFile(MOCK_YAML, MOCK_UPLOAD, 'playback', abortController.signal);
      abortController.abort();

      const result = await resultPromise;

      expect(result.success).toBe(false);
      expect(result.error).toContain('Aborted');
    });

    it('should pass AbortSignal to fetch', async () => {
      const abortController = new AbortController();

      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
        ok: true,
        headers: new Headers({ 'Content-Type': 'audio/ogg' }),
        blob: vi.fn().mockResolvedValue(new Blob()),
      } as never);

      await convertFile(MOCK_YAML, MOCK_UPLOAD, 'download', abortController.signal);

      expect(fetch).toHaveBeenCalledWith(
        'http://localhost:4545/api/v1/process',
        expect.objectContaining({
          signal: abortController.signal,
        })
      );
    });
  });

  describe('convertFile - Error Handling', () => {
    it('should handle HTTP error responses', async () => {
      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
        ok: false,
        status: 400,
        statusText: 'Bad Request',
        text: vi.fn().mockResolvedValue('Invalid pipeline configuration'),
      } as never);

      const result = await convertFile(MOCK_YAML, MOCK_UPLOAD, 'playback');

      expect(result.success).toBe(false);
      expect(result.error).toContain('Bad Request');
      expect(result.error).toContain('Invalid pipeline configuration');
    });

    it('should handle network errors', async () => {
      (fetch as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('Network error'));

      const result = await convertFile(MOCK_YAML, MOCK_UPLOAD, 'playback');

      expect(result.success).toBe(false);
      expect(result.error).toContain('Network error');
    });

    it('should handle unknown errors', async () => {
      (fetch as ReturnType<typeof vi.fn>).mockRejectedValue('Unknown error');

      const result = await convertFile(MOCK_YAML, MOCK_UPLOAD, 'playback');

      expect(result.success).toBe(false);
      expect(result.error).toBe('Unknown error occurred');
    });
  });

  describe('convertFile - Request Formation', () => {
    it('should send YAML and media file as FormData', async () => {
      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
        ok: true,
        headers: new Headers({ 'Content-Type': 'audio/ogg' }),
        blob: vi.fn().mockResolvedValue(new Blob()),
      } as never);

      await convertFile(MOCK_YAML, MOCK_UPLOAD, 'download');

      expect(fetch).toHaveBeenCalledWith(
        'http://localhost:4545/api/v1/process',
        expect.objectContaining({
          method: 'POST',
          body: expect.any(FormData),
        })
      );

      const callArgs = (fetch as ReturnType<typeof vi.fn>).mock.calls[0];
      const formData = callArgs[1]?.body as FormData;

      expect(formData.get('config')).toBeInstanceOf(Blob);

      const mediaFile = formData.get('media') as File;
      expect(mediaFile).toBeInstanceOf(File);
      expect(mediaFile.name).toBe('test.ogg');
      expect(mediaFile.type).toBe('audio/ogg');
    });

    it('should omit media file if not provided (asset-based pipeline)', async () => {
      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
        ok: true,
        headers: new Headers({ 'Content-Type': 'audio/ogg' }),
        blob: vi.fn().mockResolvedValue(new Blob()),
      } as never);

      await convertFile(MOCK_YAML, null, 'download');

      const callArgs = (fetch as ReturnType<typeof vi.fn>).mock.calls[0];
      const formData = callArgs[1]?.body as FormData;

      expect(formData.get('config')).toBeInstanceOf(Blob);
      expect(formData.get('media')).toBeNull();
    });
  });

  describe('getExtensionFromContentType', () => {
    it('should map common audio content types', () => {
      expect(getExtensionFromContentType('audio/ogg')).toBe('.ogg');
      expect(getExtensionFromContentType('audio/opus')).toBe('.opus');
      expect(getExtensionFromContentType('audio/mpeg')).toBe('.mp3');
      expect(getExtensionFromContentType('audio/wav')).toBe('.wav');
      expect(getExtensionFromContentType('audio/webm')).toBe('.webm');
      expect(getExtensionFromContentType('audio/flac')).toBe('.flac');
    });

    it('should map video content types', () => {
      expect(getExtensionFromContentType('video/mp4')).toBe('.mp4');
      expect(getExtensionFromContentType('video/webm')).toBe('.webm');
      expect(getExtensionFromContentType('video/ogg')).toBe('.ogv');
    });

    it('should map MP4 audio content types', () => {
      expect(getExtensionFromContentType('audio/mp4')).toBe('.m4a');
      expect(getExtensionFromContentType('audio/mp4; codecs="opus"')).toBe('.m4a');
    });

    it('should handle content types with codecs', () => {
      expect(getExtensionFromContentType('audio/ogg; codecs=opus')).toBe('.ogg');
      expect(getExtensionFromContentType('audio/webm; codecs=vorbis')).toBe('.webm');
    });

    it('should default to .ogg for unknown audio types', () => {
      expect(getExtensionFromContentType('audio/x-custom')).toBe('.ogg');
      expect(getExtensionFromContentType('audio/unknown')).toBe('.ogg');
    });

    it('should handle application/octet-stream', () => {
      expect(getExtensionFromContentType('application/octet-stream')).toBe('.ogg');
    });

    it('should default to .bin for completely unknown types', () => {
      expect(getExtensionFromContentType('application/pdf')).toBe('.bin');
      expect(getExtensionFromContentType('text/plain')).toBe('.bin');
      expect(getExtensionFromContentType('')).toBe('.bin');
    });

    it('should handle JSON content type', () => {
      expect(getExtensionFromContentType('application/json')).toBe('.json');
    });
  });

  describe('convertFile - MSE Streaming (MP4)', () => {
    it('should use MSE streaming for fragmented MP4 (fMP4)', async () => {
      vi.stubGlobal('MediaSource', {
        isTypeSupported: vi.fn().mockReturnValue(true),
      });

      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(
        mp4Response(FRAGMENTED_MP4_HEAD, 'video/mp4')
      );

      const result = await convertFile(MOCK_YAML, MOCK_UPLOAD, 'playback');

      expect(result.success).toBe(true);
      expect(result.useStreaming).toBe(true);
      expect(result.contentType).toBe('video/mp4');
    });

    it('should fall back to blob for unfragmented MP4 even when MSE supports the codec', async () => {
      vi.stubGlobal('MediaSource', {
        isTypeSupported: vi.fn().mockReturnValue(true),
      });
      global.URL.createObjectURL = vi.fn().mockReturnValue('blob:mp4-unfragmented');

      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(
        mp4Response(UNFRAGMENTED_MP4_HEAD, 'audio/mp4; codecs="opus"')
      );

      const result = await convertFile(MOCK_YAML, MOCK_UPLOAD, 'playback');

      expect(result.success).toBe(true);
      expect(result.useStreaming).toBe(false);
      expect(result.mediaUrl).toBe('blob:mp4-unfragmented');
    });

    it('should fall back to blob for MP4 when MSE is unavailable', async () => {
      vi.stubGlobal('MediaSource', undefined);
      global.URL.createObjectURL = vi.fn().mockReturnValue('blob:mp4-url');

      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(
        mp4Response(FRAGMENTED_MP4_HEAD, 'video/mp4')
      );

      const result = await convertFile(MOCK_YAML, MOCK_UPLOAD, 'playback');

      expect(result.success).toBe(true);
      expect(result.useStreaming).toBe(false);
      expect(result.mediaUrl).toBe('blob:mp4-url');
    });

    it('should force blob playback for MP4 when playback strategy is "blob"', async () => {
      vi.stubGlobal('MediaSource', {
        isTypeSupported: vi.fn().mockReturnValue(true),
      });
      global.URL.createObjectURL = vi.fn().mockReturnValue('blob:mp4-forced');

      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(
        mp4Response(FRAGMENTED_MP4_HEAD, 'audio/mp4; codecs="opus"')
      );

      const result = await convertFile(MOCK_YAML, MOCK_UPLOAD, 'playback', undefined, {
        playback: 'blob',
      });

      expect(result.success).toBe(true);
      expect(result.useStreaming).toBe(false);
      expect(result.mediaUrl).toBe('blob:mp4-forced');
    });

    it('should fall back to blob when the fragmentation probe stream errors', async () => {
      vi.stubGlobal('MediaSource', {
        isTypeSupported: vi.fn().mockReturnValue(true),
      });
      global.URL.createObjectURL = vi.fn().mockReturnValue('blob:mp4-probe-error');

      const rejectingBody = () =>
        new ReadableStream<Uint8Array>({
          pull(controller) {
            controller.error(new Error('probe stream error'));
          },
        });

      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue(
        mp4Response(FRAGMENTED_MP4_HEAD, 'audio/mp4; codecs="opus"', rejectingBody)
      );

      const result = await convertFile(MOCK_YAML, MOCK_UPLOAD, 'playback');

      expect(result.success).toBe(true);
      expect(result.useStreaming).toBe(false);
      expect(result.mediaUrl).toBe('blob:mp4-probe-error');
    });
  });

  describe('convertFile - WebM blob strategy', () => {
    it('should force blob playback when playback is "blob"', async () => {
      vi.stubGlobal('MediaSource', {
        isTypeSupported: vi.fn().mockReturnValue(true),
      });

      const mockBlob = new Blob(['webm data'], { type: 'audio/webm' });
      global.URL.createObjectURL = vi.fn().mockReturnValue('blob:webm-blob-url');

      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
        ok: true,
        headers: new Headers({ 'Content-Type': 'audio/webm' }),
        body: new ReadableStream(),
        blob: vi.fn().mockResolvedValue(mockBlob),
      } as never);

      const result = await convertFile(MOCK_YAML, MOCK_UPLOAD, 'playback', undefined, {
        playback: 'blob',
      });

      expect(result.success).toBe(true);
      expect(result.useStreaming).toBe(false);
    });
  });

  describe('convertFile - Multiple uploads', () => {
    it('should include all upload fields in FormData', async () => {
      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
        ok: true,
        headers: new Headers({ 'Content-Type': 'audio/ogg' }),
        blob: vi.fn().mockResolvedValue(new Blob()),
      } as never);

      const file1 = new File(['a'], 'audio.ogg', { type: 'audio/ogg' });
      const file2 = new File(['b'], 'image.png', { type: 'image/png' });
      const uploads = [
        { field: 'media', file: file1 },
        { field: 'overlay', file: file2 },
      ];

      await convertFile(MOCK_YAML, uploads, 'download');

      const callArgs = (fetch as ReturnType<typeof vi.fn>).mock.calls[0];
      const formData = callArgs[1]?.body as FormData;

      expect(formData.get('media')).toBeInstanceOf(File);
      expect(formData.get('overlay')).toBeInstanceOf(File);
    });
  });

  describe('convertFile - HTTP error with empty body', () => {
    it('should handle error response with empty text', async () => {
      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
        ok: false,
        status: 500,
        statusText: 'Internal Server Error',
        text: vi.fn().mockResolvedValue(''),
      } as never);

      const result = await convertFile(MOCK_YAML, MOCK_UPLOAD, 'playback');

      expect(result.success).toBe(false);
      expect(result.error).toBe('Conversion failed: Internal Server Error');
    });
  });

  describe('convertFile - Download with no file', () => {
    it('should use default filename when no file provided', async () => {
      const mockBlob = new Blob(['data']);
      const mockLink = { href: '', download: '', click: vi.fn() };

      vi.spyOn(document.body, 'appendChild').mockImplementation(() => null as never);
      vi.spyOn(document.body, 'removeChild').mockImplementation(() => null as never);
      const realCreate = document.createElement.bind(document);
      vi.spyOn(document, 'createElement').mockImplementation((tag: string) =>
        tag === 'a' ? (mockLink as never) : realCreate(tag)
      );
      global.URL.createObjectURL = vi.fn().mockReturnValue('blob:url');

      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
        ok: true,
        headers: new Headers({ 'Content-Type': 'audio/wav' }),
        blob: vi.fn().mockResolvedValue(mockBlob),
      } as never);

      await convertFile(MOCK_YAML, null, 'download');

      expect(mockLink.download).toBe('output.wav');
    });
  });
});
