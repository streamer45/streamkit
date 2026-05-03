// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useState } from 'react';

import type { ConversionStatus } from '@/components/converter/ConversionProgress';
import type { OutputMode } from '@/services/converter';
import type { SamplePipeline } from '@/types/generated/api-types';

/** Grouped state for the ConvertView component. */
export function useConvertViewState() {
  const [samples, setSamples] = useState<SamplePipeline[]>([]);
  const [samplesLoading, setSamplesLoading] = useState<boolean>(true);
  const [samplesError, setSamplesError] = useState<string | null>(null);

  const [inputMode, setInputMode] = useState<'upload' | 'asset'>('upload');
  const [selectedFile, setSelectedFile] = useState<File | null>(null);
  const [selectedAssetId, setSelectedAssetId] = useState<string>('');

  const [pipelineYaml, setPipelineYaml] = useState<string>('');
  const [selectedTemplateId, setSelectedTemplateId] = useState<string>('');
  const [textInput, setTextInput] = useState<string>('');

  const [conversionStatus, setConversionStatus] = useState<ConversionStatus>('idle');
  const [conversionMessage, setConversionMessage] = useState<string>('');
  const [outputMode, setOutputMode] = useState<OutputMode>('playback');
  const [abortController, setAbortController] = useState<AbortController | null>(null);

  const [mediaUrl, setMediaUrl] = useState<string | null>(null);
  const [mediaContentType, setMediaContentType] = useState<string | null>(null);
  const [mediaStream, setMediaStream] = useState<ReadableStream<Uint8Array> | null>(null);
  const [useStreaming, setUseStreaming] = useState<boolean>(false);
  const [streamKey, setStreamKey] = useState<number>(0);

  const [showTechnicalDetails, setShowTechnicalDetails] = useState<boolean>(false);

  return {
    samples,
    setSamples,
    samplesLoading,
    setSamplesLoading,
    samplesError,
    setSamplesError,
    inputMode,
    setInputMode,
    selectedFile,
    setSelectedFile,
    selectedAssetId,
    setSelectedAssetId,
    pipelineYaml,
    setPipelineYaml,
    selectedTemplateId,
    setSelectedTemplateId,
    textInput,
    setTextInput,
    conversionStatus,
    setConversionStatus,
    conversionMessage,
    setConversionMessage,
    outputMode,
    setOutputMode,
    abortController,
    setAbortController,
    mediaUrl,
    setMediaUrl,
    mediaContentType,
    setMediaContentType,
    mediaStream,
    setMediaStream,
    useStreaming,
    setUseStreaming,
    streamKey,
    setStreamKey,
    showTechnicalDetails,
    setShowTechnicalDetails,
  };
}
