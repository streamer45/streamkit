// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useState } from 'react';

import type { ConversionStatus } from '@/components/converter/ConversionProgress';
import type { OutputMode } from '@/services/converter';
import type { SamplePipeline } from '@/types/generated/api-types';

/**
 * Custom hook to manage ConvertView state
 * Groups related state together to reduce component complexity
 *
 * Note: Setters from useState are guaranteed to be stable by React
 * and will never change between renders, so they're safe to use in
 * dependency arrays without causing unnecessary re-renders.
 */
export function useConvertViewState() {
  // Sample templates state
  const [samples, setSamples] = useState<SamplePipeline[]>([]);
  const [samplesLoading, setSamplesLoading] = useState<boolean>(true);
  const [samplesError, setSamplesError] = useState<string | null>(null);

  // Input mode and selection state
  const [inputMode, setInputMode] = useState<'upload' | 'asset'>('upload');
  const [selectedFile, setSelectedFile] = useState<File | null>(null);
  const [selectedAssetId, setSelectedAssetId] = useState<string>('');

  // Pipeline state
  const [pipelineYaml, setPipelineYaml] = useState<string>('');
  const [selectedTemplateId, setSelectedTemplateId] = useState<string>('');
  const [isTranscriptionPipeline, setIsTranscriptionPipeline] = useState<boolean>(false);
  const [isTTSPipeline, setIsTTSPipeline] = useState<boolean>(false);
  const [isNoInputPipeline, setIsNoInputPipeline] = useState<boolean>(false);
  const [textInput, setTextInput] = useState<string>('');

  // Conversion state
  const [conversionStatus, setConversionStatus] = useState<ConversionStatus>('idle');
  const [conversionMessage, setConversionMessage] = useState<string>('');
  const [outputMode, setOutputMode] = useState<OutputMode>('playback');
  const [abortController, setAbortController] = useState<AbortController | null>(null);

  // Output state
  const [audioUrl, setAudioUrl] = useState<string | null>(null);
  const [audioContentType, setAudioContentType] = useState<string | null>(null);
  const [audioStream, setAudioStream] = useState<ReadableStream<Uint8Array> | null>(null);
  const [useStreaming, setUseStreaming] = useState<boolean>(false);
  const [streamKey, setStreamKey] = useState<number>(0);

  // UI state
  const [showTechnicalDetails, setShowTechnicalDetails] = useState<boolean>(false);

  return {
    // Sample templates
    samples,
    setSamples,
    samplesLoading,
    setSamplesLoading,
    samplesError,
    setSamplesError,

    // Input mode and selection
    inputMode,
    setInputMode,
    selectedFile,
    setSelectedFile,
    selectedAssetId,
    setSelectedAssetId,

    // Pipeline
    pipelineYaml,
    setPipelineYaml,
    selectedTemplateId,
    setSelectedTemplateId,
    isTranscriptionPipeline,
    setIsTranscriptionPipeline,
    isTTSPipeline,
    setIsTTSPipeline,
    isNoInputPipeline,
    setIsNoInputPipeline,
    textInput,
    setTextInput,

    // Conversion
    conversionStatus,
    setConversionStatus,
    conversionMessage,
    setConversionMessage,
    outputMode,
    setOutputMode,
    abortController,
    setAbortController,

    // Output
    audioUrl,
    setAudioUrl,
    audioContentType,
    setAudioContentType,
    audioStream,
    setAudioStream,
    useStreaming,
    setUseStreaming,
    streamKey,
    setStreamKey,

    // UI
    showTechnicalDetails,
    setShowTechnicalDetails,
  };
}
