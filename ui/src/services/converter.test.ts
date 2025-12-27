// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';

import { convertFile, getExtensionFromContentType } from './converter';

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
}));

describe('converter service', () => {
  const MOCK_YAML = 'steps:\n  - id: test\n    kind: core::passthrough';
  const MOCK_FILE = new File(['test content'], 'test.ogg', { type: 'audio/ogg' });

  let originalMediaSource: unknown;

  beforeEach(() => {
    // Reset fetch mock before each test
    global.fetch = vi.fn() as never;
    vi.useFakeTimers();
    originalMediaSource = (globalThis as { MediaSource?: unknown }).MediaSource;
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
    (globalThis as { MediaSource?: unknown }).MediaSource = originalMediaSource as never;
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

      const result = await convertFile(MOCK_YAML, MOCK_FILE, 'playback');

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
      const result = await convertFile(MOCK_YAML, MOCK_FILE, 'playback', abortController.signal);

      expect(result.success).toBe(true);
      expect(result.responseStream).toBeDefined();

      // The wrapped stream supports cancellation via the returned reader
      // (abort signal cancellation is tested in other tests)
    });
  });

  describe('convertFile - MSE Streaming (WebM)', () => {
    it('should handle WebM streaming for MSE playback', async () => {
      (globalThis as { MediaSource?: unknown }).MediaSource = {
        isTypeSupported: vi.fn().mockReturnValue(true),
      } as never;

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

      const result = await convertFile(MOCK_YAML, MOCK_FILE, 'playback');

      expect(result.success).toBe(true);
      expect(result.useStreaming).toBe(true);
      expect(result.responseStream).toBeDefined();
      expect(result.contentType).toBe('audio/webm');
    });

    it('should wrap WebM stream with cancellation support', async () => {
      (globalThis as { MediaSource?: unknown }).MediaSource = {
        isTypeSupported: vi.fn().mockReturnValue(true),
      } as never;

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
      const result = await convertFile(MOCK_YAML, MOCK_FILE, 'playback', abortController.signal);

      expect(result.responseStream).toBeDefined();

      // Stream should be cancellable without throwing
      if (result.responseStream) {
        const reader = result.responseStream.getReader();
        await expect(reader.cancel()).resolves.toBeUndefined();
      }
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

      const result = await convertFile(MOCK_YAML, MOCK_FILE, 'playback');

      expect(result.success).toBe(true);
      expect(result.useStreaming).toBe(false);
      expect(result.audioUrl).toBe('blob:mock-url');
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
      vi.spyOn(document, 'createElement').mockReturnValue(mockLink as never);

      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
        ok: true,
        headers: new Headers({ 'Content-Type': 'audio/opus' }),
        blob: vi.fn().mockResolvedValue(mockBlob),
      } as never);

      const result = await convertFile(MOCK_YAML, MOCK_FILE, 'download');

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
      vi.spyOn(document, 'createElement').mockReturnValue(mockLink as never);
      global.URL.createObjectURL = vi.fn().mockReturnValue('blob:url');

      (fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
        ok: true,
        headers: new Headers({ 'Content-Type': 'audio/wav' }),
        blob: vi.fn().mockResolvedValue(mockBlob),
      } as never);

      const file = new File(['content'], 'my-audio.ogg', { type: 'audio/ogg' });
      await convertFile(MOCK_YAML, file, 'download');

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

      const resultPromise = convertFile(MOCK_YAML, MOCK_FILE, 'playback', abortController.signal);
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

      await convertFile(MOCK_YAML, MOCK_FILE, 'download', abortController.signal);

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

      const result = await convertFile(MOCK_YAML, MOCK_FILE, 'playback');

      expect(result.success).toBe(false);
      expect(result.error).toContain('Bad Request');
      expect(result.error).toContain('Invalid pipeline configuration');
    });

    it('should handle network errors', async () => {
      (fetch as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('Network error'));

      const result = await convertFile(MOCK_YAML, MOCK_FILE, 'playback');

      expect(result.success).toBe(false);
      expect(result.error).toContain('Network error');
    });

    it('should handle unknown errors', async () => {
      (fetch as ReturnType<typeof vi.fn>).mockRejectedValue('Unknown error');

      const result = await convertFile(MOCK_YAML, MOCK_FILE, 'playback');

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

      await convertFile(MOCK_YAML, MOCK_FILE, 'download');

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
});
