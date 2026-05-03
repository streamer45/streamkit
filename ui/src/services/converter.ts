// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { getLogger } from '@/utils/logger';
import { canUseMseForMimeType } from '@/utils/mse';

import { fetchApi } from './base';

const logger = getLogger('converter');

export interface ConversionResult {
  success: boolean;
  error?: string;
  mediaUrl?: string;
  contentType?: string;
  responseStream?: ReadableStream<Uint8Array>; // Stream for MSE-based playback or JSON streaming
  useStreaming?: boolean; // Whether to use streaming (MSE or JSON)
}

export type OutputMode = 'download' | 'playback';

export type WebmPlaybackStrategy = 'auto' | 'mse' | 'blob';

export interface ConvertFileOptions {
  webmPlayback?: WebmPlaybackStrategy;
}

function createWrappedStream(
  reader: ReadableStreamDefaultReader<Uint8Array>,
  signal: AbortSignal | undefined,
  streamType: string
): ReadableStream<Uint8Array> {
  if (signal) {
    signal.addEventListener('abort', () => {
      logger.debug(`Abort signal received, cancelling ${streamType} reader`);
      reader.cancel().catch(() => {
        // Ignore errors when cancelling
      });
    });
  }

  return new ReadableStream({
    async start(controller) {
      try {
        while (true) {
          const { done, value } = await reader.read();
          if (done) {
            controller.close();
            break;
          }
          controller.enqueue(value);
        }
      } catch (error) {
        // If aborted or errored, close the stream
        controller.error(error);
        reader.cancel().catch(() => {
          // Ignore errors when cancelling
        });
      }
    },
    cancel() {
      // When the stream is cancelled, cancel the underlying reader
      logger.debug(`${streamType} stream cancelled, closing connection`);
      reader.cancel().catch(() => {
        // Ignore errors when cancelling
      });
    },
  });
}

function handleStreamingResponse(
  response: Response,
  contentType: string,
  signal: AbortSignal | undefined,
  options: ConvertFileOptions | undefined
): ConversionResult | null {
  const isJSON = contentType.includes('application/json');
  const isWebM = contentType.includes('webm');
  const isMp4 = contentType.includes('video/mp4') || contentType.includes('audio/mp4');

  if (isJSON && response.body) {
    logger.info('Using streaming for JSON output');
    const reader = response.body.getReader();
    const wrappedStream = createWrappedStream(reader, signal, 'JSON');

    return {
      success: true,
      responseStream: wrappedStream,
      contentType,
      useStreaming: true,
    };
  }

  if (isWebM && response.body) {
    const webmStrategy: WebmPlaybackStrategy = options?.webmPlayback ?? 'auto';
    const allowWebmStreaming = webmStrategy !== 'blob';
    const canStreamWebm = allowWebmStreaming && canUseMseForMimeType(contentType);

    if (!canStreamWebm) {
      logger.info('Falling back to blob playback for WebM (MSE unavailable or unsupported)');
      return null;
    }

    logger.info('Using MSE streaming for WebM playback');
    const reader = response.body.getReader();
    const wrappedStream = createWrappedStream(reader, signal, 'WebM');

    return {
      success: true,
      responseStream: wrappedStream,
      contentType,
      useStreaming: true,
    };
  }

  if (isMp4 && response.body) {
    if (!canUseMseForMimeType(contentType)) {
      logger.info('Falling back to blob playback for MP4 (MSE unavailable or unsupported)');
      return null;
    }

    logger.info('Using MSE streaming for MP4 (fMP4) playback');
    const reader = response.body.getReader();
    const wrappedStream = createWrappedStream(reader, signal, 'MP4');

    return {
      success: true,
      responseStream: wrappedStream,
      contentType,
      useStreaming: true,
    };
  }

  return null;
}

async function handleBlobPlayback(
  response: Response,
  contentType: string
): Promise<ConversionResult> {
  const blob = await response.blob();
  logger.debug('Downloaded blob size:', blob.size);

  const mediaUrl = URL.createObjectURL(blob);
  logger.debug('Created media URL for playback');

  return {
    success: true,
    mediaUrl,
    contentType,
    useStreaming: false,
  };
}

async function handleDownload(
  response: Response,
  contentType: string,
  mediaFile: File | null
): Promise<ConversionResult> {
  const blob = await response.blob();
  logger.debug('Downloaded blob size:', blob.size);

  const extension = getExtensionFromContentType(contentType);
  let outputFileName: string;
  if (mediaFile) {
    const baseName = mediaFile.name.includes('.')
      ? mediaFile.name.substring(0, mediaFile.name.lastIndexOf('.'))
      : mediaFile.name;
    outputFileName = `${baseName}_converted${extension}`;
  } else {
    outputFileName = `output${extension}`;
  }

  downloadBlob(blob, outputFileName);

  logger.info('Download triggered:', outputFileName);

  return { success: true };
}

export type UploadField = { field: string; file: File };

export async function convertFile(
  pipelineYaml: string,
  uploads: UploadField[] | null,
  mode: OutputMode = 'download',
  signal?: AbortSignal,
  options?: ConvertFileOptions
): Promise<ConversionResult> {
  try {
    const formData = new FormData();
    formData.append('config', new Blob([pipelineYaml], { type: 'text/yaml' }));

    const files = uploads ?? [];
    for (const upload of files) {
      formData.append(upload.field, upload.file);
    }

    logger.info('Starting conversion:', {
      uploads: files.length,
      fileNames: files.map((f) => f.file.name),
      fileSizes: files.map((f) => f.file.size),
      pipelineLength: pipelineYaml.length,
    });

    const response = await fetchApi('/api/v1/process', {
      method: 'POST',
      body: formData,
      signal,
    });

    if (!response.ok) {
      const errorText = await response.text();
      logger.error('Conversion failed:', {
        status: response.status,
        statusText: response.statusText,
        error: errorText,
      });
      const errorSuffix = errorText ? ` - ${errorText}` : '';
      return {
        success: false,
        error: `Conversion failed: ${response.statusText}${errorSuffix}`,
      };
    }

    const contentType = response.headers.get('Content-Type') || 'application/octet-stream';
    logger.info('Conversion successful, content type:', contentType);

    if (mode === 'playback') {
      const streamingResult = handleStreamingResponse(response, contentType, signal, options);
      if (streamingResult) {
        return streamingResult;
      }

      return handleBlobPlayback(response, contentType);
    }

    const primaryFile = files[0]?.file ?? null;
    return handleDownload(response, contentType, primaryFile);
  } catch (error) {
    logger.error('Conversion error:', error);
    return {
      success: false,
      error: error instanceof Error ? error.message : 'Unknown error occurred',
    };
  }
}

export function getExtensionFromContentType(contentType: string): string {
  const typeMap: Record<string, string> = {
    'audio/ogg': '.ogg',
    'audio/opus': '.opus',
    'audio/mpeg': '.mp3',
    'audio/wav': '.wav',
    'audio/webm': '.webm',
    'audio/flac': '.flac',
    'audio/mp4': '.m4a',
    'application/ogg': '.ogg',
    'application/json': '.json',
    'video/mp4': '.mp4',
    'video/webm': '.webm',
    'video/ogg': '.ogv',
  };

  if (typeMap[contentType]) {
    return typeMap[contentType];
  }

  for (const [type, ext] of Object.entries(typeMap)) {
    if (contentType.startsWith(type)) {
      return ext;
    }
  }

  if (contentType.includes('audio')) {
    return '.ogg';
  }

  if (contentType === 'application/octet-stream') {
    return '.ogg';
  }

  return '.bin';
}

function downloadBlob(blob: Blob, fileName: string): void {
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = fileName;
  document.body.appendChild(link);
  link.click();
  document.body.removeChild(link);

  setTimeout(() => {
    URL.revokeObjectURL(url);
  }, 100);
}
