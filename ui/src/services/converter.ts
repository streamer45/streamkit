// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Service for executing oneshot (stateless) pipeline conversions
 */

import { getLogger } from '@/utils/logger';
import { canUseMseForMimeType } from '@/utils/mse';

import { getApiUrl } from './base';

const logger = getLogger('converter');

export interface ConversionResult {
  success: boolean;
  error?: string;
  audioUrl?: string;
  contentType?: string;
  responseStream?: ReadableStream<Uint8Array>; // Stream for MSE-based playback or JSON streaming
  useStreaming?: boolean; // Whether to use streaming (MSE or JSON)
}

export type OutputMode = 'download' | 'playback';

export type WebmPlaybackStrategy = 'auto' | 'mse' | 'blob';

export interface ConvertFileOptions {
  webmPlayback?: WebmPlaybackStrategy;
}

/**
 * Creates a wrapped readable stream with proper cancellation handling
 */
function createWrappedStream(
  reader: ReadableStreamDefaultReader<Uint8Array>,
  signal: AbortSignal | undefined,
  streamType: string
): ReadableStream<Uint8Array> {
  // Listen for abort signal and cancel the reader
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

/**
 * Handles streaming response for JSON or WebM content
 */
function handleStreamingResponse(
  response: Response,
  contentType: string,
  signal: AbortSignal | undefined,
  options: ConvertFileOptions | undefined
): ConversionResult | null {
  const isJSON = contentType.includes('application/json');
  const isWebM = contentType.includes('webm');

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

  return null;
}

/**
 * Handles blob-based playback for non-streaming formats
 */
async function handleBlobPlayback(
  response: Response,
  contentType: string
): Promise<ConversionResult> {
  const blob = await response.blob();
  logger.debug('Downloaded blob size:', blob.size);

  const audioUrl = URL.createObjectURL(blob);
  logger.debug('Created audio URL for playback');

  return {
    success: true,
    audioUrl,
    contentType,
    useStreaming: false,
  };
}

/**
 * Handles download mode by triggering a browser download
 */
async function handleDownload(
  response: Response,
  contentType: string,
  mediaFile: File | null
): Promise<ConversionResult> {
  const blob = await response.blob();
  logger.debug('Downloaded blob size:', blob.size);

  // Generate a filename based on the original file
  const originalName = mediaFile?.name || 'converted_audio';
  const extension = getExtensionFromContentType(contentType);
  const baseName = originalName.includes('.')
    ? originalName.substring(0, originalName.lastIndexOf('.'))
    : originalName;
  const outputFileName = `${baseName}_converted${extension}`;

  // Trigger download
  downloadBlob(blob, outputFileName);

  logger.info('Download triggered:', outputFileName);

  return { success: true };
}

/**
 * Executes a oneshot pipeline conversion
 * @param pipelineYaml - The YAML pipeline configuration
 * @param mediaFile - The media file to process (optional if pipeline includes file_read)
 * @param mode - Output mode: 'download' or 'playback'
 * @param signal - Optional AbortSignal to cancel the request
 * @returns A promise that resolves when the conversion is complete
 */
export async function convertFile(
  pipelineYaml: string,
  mediaFile: File | null,
  mode: OutputMode = 'download',
  signal?: AbortSignal,
  options?: ConvertFileOptions
): Promise<ConversionResult> {
  try {
    // Build the multipart form data
    const formData = new FormData();
    formData.append('config', new Blob([pipelineYaml], { type: 'text/yaml' }));

    // Only append media file if provided (not needed for asset-based pipelines)
    if (mediaFile) {
      formData.append('media', mediaFile);
    }

    // Determine the API URL
    const apiUrl = getApiUrl();
    const processEndpoint = `${apiUrl}/api/v1/process`;

    logger.info('Starting conversion:', {
      endpoint: processEndpoint,
      fileName: mediaFile?.name || '(asset-based)',
      fileSize: mediaFile?.size || 0,
      pipelineLength: pipelineYaml.length,
    });

    // Make the request
    const response = await fetch(processEndpoint, {
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

    // Get the content type from the response
    const contentType = response.headers.get('Content-Type') || 'application/octet-stream';
    logger.info('Conversion successful, content type:', contentType);

    if (mode === 'playback') {
      // Try streaming response first (for JSON/WebM)
      const streamingResult = handleStreamingResponse(response, contentType, signal, options);
      if (streamingResult) {
        return streamingResult;
      }

      // Fall back to blob-based playback for other formats
      return handleBlobPlayback(response, contentType);
    }

    // Handle download mode
    return handleDownload(response, contentType, mediaFile);
  } catch (error) {
    logger.error('Conversion error:', error);
    return {
      success: false,
      error: error instanceof Error ? error.message : 'Unknown error occurred',
    };
  }
}

/**
 * Maps content type to file extension
 * Exported for use in download handling across the app
 */
export function getExtensionFromContentType(contentType: string): string {
  const typeMap: Record<string, string> = {
    'audio/ogg': '.ogg',
    'audio/opus': '.opus',
    'audio/mpeg': '.mp3',
    'audio/wav': '.wav',
    'audio/webm': '.webm',
    'audio/flac': '.flac',
    'application/ogg': '.ogg',
    'application/json': '.json',
    'video/mp4': '.mp4',
    'video/webm': '.webm',
    'video/ogg': '.ogv',
  };

  // Try exact match first
  if (typeMap[contentType]) {
    return typeMap[contentType];
  }

  // Try prefix match (e.g., "audio/ogg; codecs=opus")
  for (const [type, ext] of Object.entries(typeMap)) {
    if (contentType.startsWith(type)) {
      return ext;
    }
  }

  // Check if it's an audio type and default to .ogg
  if (contentType.includes('audio')) {
    return '.ogg';
  }

  // Last resort: check if application/octet-stream, likely audio
  if (contentType === 'application/octet-stream') {
    return '.ogg';
  }

  // Final fallback
  return '.bin';
}

/**
 * Triggers a browser download for a blob
 */
function downloadBlob(blob: Blob, fileName: string): void {
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = fileName;
  document.body.appendChild(link);
  link.click();
  document.body.removeChild(link);

  // Clean up the object URL after a short delay
  setTimeout(() => {
    URL.revokeObjectURL(url);
  }, 100);
}
