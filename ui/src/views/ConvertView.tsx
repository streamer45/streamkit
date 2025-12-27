// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import React, { useEffect, useRef, useCallback, useState, useMemo } from 'react';

import { AssetSelector } from '@/components/converter/AssetSelector';
import { ConversionProgress } from '@/components/converter/ConversionProgress';
import { FileUpload } from '@/components/converter/FileUpload';
import { JsonStreamDisplay } from '@/components/converter/JsonStreamDisplay';
import { PipelineEditor } from '@/components/converter/PipelineEditor';
import { TemplateSelector } from '@/components/converter/TemplateSelector';
import { TranscriptionDisplay } from '@/components/converter/TranscriptionDisplay';
import { CustomAudioPlayer } from '@/components/CustomAudioPlayer';
import { MSEAudioPlayer } from '@/components/MSEAudioPlayer';
import { RadioGroupRoot, RadioWithLabel } from '@/components/ui/RadioGroup';
import { useConvertViewState } from '@/hooks/useConvertViewState';
import { useAudioAssets } from '@/services/assets';
import { convertFile, type OutputMode, getExtensionFromContentType } from '@/services/converter';
import { listSamples } from '@/services/samples';
import { useSchemaStore, ensureSchemasLoaded } from '@/stores/schemaStore';
import { viewsLogger } from '@/utils/logger';
import { orderSamplePipelinesSystemFirst } from '@/utils/samplePipelineOrdering';
import { injectFileReadNode } from '@/utils/yamlPipeline';

const ViewContainer = styled.div`
  height: 100%;
  display: flex;
  flex-direction: column;
  background: var(--sk-bg);
`;

const ContentArea = styled.div`
  flex: 1;
  overflow-y: auto;
  display: flex;
  justify-content: center;
  padding: 40px;
`;

const ContentWrapper = styled.div`
  width: 100%;
  max-width: 1200px;
  display: flex;
  flex-direction: column;
  gap: 32px;
`;

const BottomSpacer = styled.div`
  height: 8px;
  flex-shrink: 0;
  /* With gap: 32px from ContentWrapper, this gives us 40px total bottom spacing */
`;

const Section = styled.div`
  display: flex;
  flex-direction: column;
  gap: 16px;
  padding: 24px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 12px;
`;

const SectionTitle = styled.h2`
  font-size: 18px;
  font-weight: 600;
  color: var(--sk-text);
  margin: 0;
`;

const EditorSection = styled.div`
  display: flex;
  flex-direction: column;
  gap: 16px;
  background: transparent;
  border: none;
  border-radius: 8px;
  padding: 0;
`;

const ConvertButtonContainer = styled.div`
  display: flex;
  justify-content: center;
`;

const ConvertButton = styled.button<{ disabled: boolean; isProcessing?: boolean }>`
  padding: 14px 40px;
  font-size: 16px;
  font-weight: 600;
  color: ${(props) => {
    if (props.disabled) return 'var(--sk-text-muted)';
    if (props.isProcessing) return 'white';
    return 'var(--sk-primary-contrast)';
  }};
  background: ${(props) => {
    if (props.disabled) return 'var(--sk-hover-bg)';
    if (props.isProcessing) return 'var(--sk-danger)';
    return 'var(--sk-primary)';
  }};
  border: 1px solid
    ${(props) => {
      if (props.disabled) return 'var(--sk-border)';
      if (props.isProcessing) return 'var(--sk-danger)';
      return 'var(--sk-primary)';
    }};
  border-radius: 8px;
  cursor: ${(props) => (props.disabled ? 'not-allowed' : 'pointer')};
  min-width: 200px;
  transition: none;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;

  &:hover:not(:disabled) {
    background: ${(props) => (props.isProcessing ? 'var(--sk-danger)' : 'var(--sk-primary-hover)')};
    border-color: ${(props) =>
      props.isProcessing ? 'var(--sk-danger)' : 'var(--sk-primary-hover)'};
    opacity: ${(props) => (props.isProcessing ? '0.9' : '1')};
  }
`;

const ButtonSpinner = styled.div`
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  width: 16px;
  height: 16px;
  border: 2px solid rgba(255, 255, 255, 0.3);
  border-top-color: white;
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
`;

const InfoBox = styled.div`
  padding: 20px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-left: 4px solid var(--sk-primary);
  border-radius: 8px;
  color: var(--sk-text);
  font-size: 14px;
  line-height: 1.6;
  display: flex;
  flex-direction: column;
  gap: 16px;
`;

const InfoContent = styled.div`
  display: flex;
  flex-direction: column;
  gap: 12px;
`;

const InfoTitle = styled.h2`
  font-size: 18px;
  font-weight: 600;
  color: var(--sk-text);
  margin: 0;
`;

const TechnicalDetailsToggle = styled.button`
  padding: 8px 12px;
  background: transparent;
  color: var(--sk-text-muted);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
  transition: none;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  align-self: flex-start;

  &:hover {
    background: var(--sk-hover-bg);
    color: var(--sk-text);
    border-color: var(--sk-border-strong);
  }
`;

const TechnicalDetails = styled.div`
  padding-top: 12px;
  border-top: 1px solid var(--sk-border);
  color: var(--sk-text-muted);
  font-size: 13px;
  display: flex;
  flex-direction: column;
  gap: 12px;
`;

const CliSnippetContainer = styled.div`
  display: flex;
  flex-direction: column;
  gap: 8px;
`;

const CliSnippetHeader = styled.div`
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
`;

const CliSnippetLabel = styled.span`
  font-size: 12px;
  color: var(--sk-text-muted);
  font-weight: 500;
`;

const CopyButton = styled.button<{ copied?: boolean }>`
  padding: 4px 10px;
  font-size: 11px;
  font-weight: 500;
  color: ${(props) => (props.copied ? 'var(--sk-success)' : 'var(--sk-text-muted)')};
  background: transparent;
  border: 1px solid ${(props) => (props.copied ? 'var(--sk-success)' : 'var(--sk-border)')};
  border-radius: 4px;
  cursor: pointer;
  transition: none;

  &:hover {
    background: var(--sk-hover-bg);
    color: ${(props) => (props.copied ? 'var(--sk-success)' : 'var(--sk-text)')};
    border-color: ${(props) => (props.copied ? 'var(--sk-success)' : 'var(--sk-border-strong)')};
  }
`;

const CodeBlock = styled.pre`
  margin: 0;
  padding: 12px;
  background: var(--sk-bg);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  font-family: 'SF Mono', 'Menlo', 'Monaco', 'Consolas', monospace;
  font-size: 12px;
  line-height: 1.5;
  color: var(--sk-text);
  overflow-x: auto;
  white-space: pre-wrap;
  word-break: break-all;
`;

const OutputModeContainer = styled.div`
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 16px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 8px;
`;

const AudioPlayerContainer = styled.div`
  padding: 24px;
  background: var(--sk-panel-bg);
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  display: flex;
  flex-direction: column;
  gap: 16px;
`;

const AudioPlayerTitle = styled.div`
  font-weight: 600;
  color: var(--sk-text);
  font-size: 16px;
`;

const HiddenAudio = styled.audio`
  display: none;
`;

const DownloadLink = styled.button`
  padding: 10px 16px;
  background: var(--sk-panel-bg);
  color: var(--sk-text);
  border: 1px solid var(--sk-border);
  border-radius: 6px;
  font-weight: 600;
  cursor: pointer;
  align-self: flex-start;
  transition: none;

  &:hover {
    background: var(--sk-hover-bg);
    border-color: var(--sk-border-strong);
  }
`;

const InputModeSwitcher = styled.div`
  display: flex;
  gap: 8px;
  margin-bottom: 16px;
`;

const ModeButton = styled.button<{ active: boolean }>`
  flex: 1;
  padding: 10px 16px;
  background: ${(props) => (props.active ? 'var(--sk-primary)' : 'var(--sk-panel-bg)')};
  color: ${(props) => (props.active ? 'white' : 'var(--sk-text)')};
  border: 1px solid ${(props) => (props.active ? 'var(--sk-primary)' : 'var(--sk-border)')};
  border-radius: 8px;
  font-size: 14px;
  font-weight: 600;
  cursor: pointer;
  transition: none;

  &:hover {
    background: ${(props) => (props.active ? 'var(--sk-primary-hover)' : 'var(--sk-hover-bg)')};
    border-color: ${(props) =>
      props.active ? 'var(--sk-primary-hover)' : 'var(--sk-border-strong)'};
  }
`;

const TextInputContainer = styled.div`
  display: flex;
  flex-direction: column;
  gap: 8px;
`;

const TextAreaLabel = styled.label`
  font-size: 14px;
  font-weight: 500;
  color: var(--sk-text);
`;

const TextArea = styled.textarea`
  width: 100%;
  min-height: 150px;
  padding: 12px;
  box-sizing: border-box;
  background: var(--sk-bg);
  color: var(--sk-text);
  border: 1px solid var(--sk-border);
  border-radius: 8px;
  font-size: 14px;
  font-family: inherit;
  line-height: 1.5;
  resize: vertical;

  &:focus {
    outline: none;
    border-color: var(--sk-primary);
  }

  &::placeholder {
    color: var(--sk-text-muted);
  }
`;

const CharCounter = styled.div`
  font-size: 12px;
  color: var(--sk-text-muted);
  text-align: right;
`;

// Helper functions moved outside component (pure functions, no dependencies)

/**
 * Detects if the current pipeline is a transcription pipeline
 */
const checkIfTranscriptionPipeline = (yaml: string): boolean => {
  // A transcription pipeline is one that produces `Transcription` packets.
  // `core::json_serialize` is used by many pipelines (VAD events, etc.) so it is not a signal.
  const lowerYaml = yaml.toLowerCase();
  return (
    lowerYaml.includes('plugin::native::whisper') ||
    lowerYaml.includes('plugin::native::sensevoice') ||
    lowerYaml.includes('transcription')
  );
};

/**
 * Detects if the current pipeline generates its own input (no user input needed)
 */
const checkIfNoInputPipeline = (yaml: string): boolean => {
  const lowerYaml = yaml.toLowerCase();

  // Check if pipeline starts with a script node that uses fetch()
  // This indicates the pipeline generates its own data
  if (lowerYaml.includes('core::script') && lowerYaml.includes('fetch')) {
    return true;
  }

  return false;
};

/**
 * Detects if the current pipeline is a TTS pipeline (text input)
 */
const checkIfTTSPipeline = (yaml: string): boolean => {
  // First check if it's a no-input pipeline (takes precedence)
  if (checkIfNoInputPipeline(yaml)) {
    return false;
  }

  // A TTS pipeline for text input should have text_chunker as an early node
  // Just having TTS nodes isn't enough - the pipeline might use TTS as a component
  // in a larger audio-to-audio pipeline (like speech translation)
  const lowerYaml = yaml.toLowerCase();

  // Check for text_chunker which indicates text input processing
  if (lowerYaml.includes('text_chunker')) {
    return true;
  }

  // Additional heuristic: If we have TTS but NO audio demuxers/decoders,
  // it's likely a text input pipeline
  const hasTTS =
    lowerYaml.includes('kokoro_tts') ||
    lowerYaml.includes('piper_tts') ||
    lowerYaml.includes('text-to-speech');

  const hasAudioDemuxer = lowerYaml.includes('demux') || lowerYaml.includes('decode');

  // If we have TTS but no audio demuxer, it's a text input pipeline
  return hasTTS && !hasAudioDemuxer;
};

/**
 * Generates a CLI command for running the pipeline with curl + ffplay
 */
const generateCliCommand = (
  templateId: string,
  isNoInput: boolean,
  isTTS: boolean,
  serverUrl: string = 'http://127.0.0.1:4545'
): string => {
  // Convert template ID to file path (e.g., "oneshot/speech_to_text" -> "samples/pipelines/oneshot/speech_to_text.yml")
  const configPath = `samples/pipelines/${templateId}.yml`;

  if (isNoInput) {
    // No input needed - send empty media field
    return `curl --no-buffer \\
  -F config=@${configPath} \\
  -F media= \\
  ${serverUrl}/api/v1/process -o - | ffplay -f webm -i -`;
  }

  if (isTTS) {
    // TTS pipeline - pipe text input
    return `echo "Your text here" | curl --no-buffer \\
  -F config=@${configPath} \\
  -F 'media=@-;type=text/plain' \\
  ${serverUrl}/api/v1/process -o - | ffplay -f webm -i -`;
  }

  // Standard audio input pipeline
  return `curl --no-buffer \\
  -F config=@${configPath} \\
  -F media=@your-audio-file.ogg \\
  ${serverUrl}/api/v1/process -o - | ffplay -f webm -i -`;
};

/**
 * ConvertView - Batch file processing interface using oneshot pipelines.
 *
 * This component provides the UI for:
 * - Template selection and YAML editing
 * - File upload or asset selection
 * - Pipeline conversion with streaming support
 * - Audio playback and transcription display
 *
 */
// eslint-disable-next-line max-statements, sonarjs/cognitive-complexity -- Conversion workflow orchestration
const ConvertView: React.FC = () => {
  const {
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
    isTranscriptionPipeline,
    setIsTranscriptionPipeline,
    isTTSPipeline,
    setIsTTSPipeline,
    isNoInputPipeline,
    setIsNoInputPipeline,
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
    showTechnicalDetails,
    setShowTechnicalDetails,
  } = useConvertViewState();

  // State for CLI command copy button
  const [cliCopied, setCliCopied] = useState(false);
  const [msePlaybackError, setMsePlaybackError] = useState<string | null>(null);
  const [mseFallbackLoading, setMseFallbackLoading] = useState<boolean>(false);

  // Generate CLI command based on current template and pipeline type
  const cliCommand = useMemo(() => {
    if (!selectedTemplateId) return '';
    return generateCliCommand(selectedTemplateId, isNoInputPipeline, isTTSPipeline);
  }, [selectedTemplateId, isNoInputPipeline, isTTSPipeline]);

  // Handler for copying CLI command to clipboard
  const handleCopyCliCommand = useCallback(async () => {
    if (!cliCommand) return;
    try {
      await navigator.clipboard.writeText(cliCommand);
      setCliCopied(true);
      setTimeout(() => setCliCopied(false), 2000);
    } catch (err) {
      viewsLogger.error('Failed to copy CLI command:', err);
    }
  }, [cliCommand]);

  // Ref for auto-scrolling to results
  const resultsRef = useRef<HTMLDivElement | null>(null);

  // Ref for audio element (for custom player)
  const audioRef = useRef<HTMLAudioElement | null>(null);

  // Get node definitions for YAML autocomplete
  const nodeDefinitions = useSchemaStore((s) => s.nodeDefinitions);

  // Ensure schemas are loaded for autocomplete
  useEffect(() => {
    ensureSchemasLoaded();
  }, []);

  // Auto-scroll to results when they appear
  useEffect(() => {
    if ((audioUrl || audioStream) && resultsRef.current) {
      // Small delay to ensure content has rendered
      const timeoutId = setTimeout(() => {
        resultsRef.current?.scrollIntoView({ behavior: 'smooth', block: 'start' });
      }, 100);
      return () => clearTimeout(timeoutId);
    }
  }, [audioUrl, audioStream]);

  // Fetch audio assets
  const { data: audioAssets = [], isLoading: assetsLoading } = useAudioAssets();

  /**
   * Detects the expected input format(s) from a pipeline YAML
   * Returns an array of compatible formats, or null if any format is acceptable
   */
  const detectExpectedFormats = (yaml: string): string[] | null => {
    const lowerYaml = yaml.toLowerCase();

    // If there's no demuxer/decoder node, any format might work (e.g., passthrough pipelines)
    const hasDecoder = lowerYaml.includes('demux') || lowerYaml.includes('decode');
    if (!hasDecoder) {
      return null; // Accept all formats
    }

    const compatibleFormats: string[] = [];

    // OGG container (opus, vorbis)
    // Match patterns: ogg::demuxer, ogg_demux, opus::decoder, opus_decode
    if (
      lowerYaml.includes('ogg::demux') ||
      lowerYaml.includes('ogg_demux') ||
      lowerYaml.includes('opus::decode') ||
      lowerYaml.includes('opus_decode')
    ) {
      compatibleFormats.push('ogg', 'opus');
    }

    // FLAC
    if (lowerYaml.includes('flac')) {
      compatibleFormats.push('flac');
    }

    // WAV/PCM
    if (lowerYaml.includes('wav') || lowerYaml.includes('pcm')) {
      compatibleFormats.push('wav');
    }

    // MP3
    if (lowerYaml.includes('mp3')) {
      compatibleFormats.push('mp3');
    }

    // If we found specific formats, return them; otherwise return null (accept all)
    return compatibleFormats.length > 0 ? compatibleFormats : null;
  };

  /**
   * Detects optional asset tags for Convert view's asset picker.
   *
   * This is a UI-only hint, carried in YAML comments so it doesn't affect pipeline parsing.
   *
   * Format:
   *   # skit:input_asset_tags=speech,music
   */
  const detectInputAssetTags = (yaml: string): string[] | null => {
    const match = yaml.match(/^\s*#\s*skit:input_asset_tags\s*=\s*([^\n#]+)\s*$/im);
    if (!match?.[1]) return null;

    const tags = match[1]
      .split(',')
      .map((tag) => tag.trim().toLowerCase())
      .filter(Boolean);

    return tags.length > 0 ? tags : null;
  };

  const assetMatchesTag = (assetId: string, tag: string): boolean => {
    if (tag === 'speech') {
      return assetId.toLowerCase().startsWith('speech_');
    }

    if (tag === 'music') {
      return assetId.toLowerCase().startsWith('music_');
    }

    if (tag.startsWith('id:')) {
      return assetId.toLowerCase() === tag.slice('id:'.length).trim().toLowerCase();
    }

    return false;
  };

  // Filter assets based on pipeline's expected format
  const filteredAssets = React.useMemo(() => {
    if (!pipelineYaml || inputMode !== 'asset') {
      return audioAssets;
    }

    const expectedFormats = detectExpectedFormats(pipelineYaml);
    const inputAssetTags = detectInputAssetTags(pipelineYaml);

    // If no specific format detected, show all assets
    if (!expectedFormats && !inputAssetTags) {
      viewsLogger.debug('No specific format required, showing all assets');
      return audioAssets;
    }

    viewsLogger.debug('Expected formats:', expectedFormats, 'Total assets:', audioAssets.length);

    // Filter assets to only those with compatible formats
    const formatFiltered = expectedFormats
      ? audioAssets.filter((asset) => expectedFormats.includes(asset.format.toLowerCase()))
      : audioAssets;

    const tagFiltered = inputAssetTags
      ? formatFiltered.filter((asset) =>
          inputAssetTags.some((tag) => assetMatchesTag(asset.id, tag))
        )
      : formatFiltered;

    viewsLogger.debug('Filtered to', tagFiltered.length, 'compatible assets');

    return tagFiltered;
  }, [audioAssets, pipelineYaml, inputMode]);

  // Clear selected asset if it's no longer in the filtered list
  useEffect(() => {
    if (selectedAssetId && !filteredAssets.some((asset) => asset.id === selectedAssetId)) {
      viewsLogger.debug('Selected asset not compatible with pipeline, clearing selection');
      setSelectedAssetId('');
    }
  }, [filteredAssets, selectedAssetId, setSelectedAssetId]);

  // Watch for pipeline YAML changes and update transcription/TTS detection
  useEffect(() => {
    const isTranscription = checkIfTranscriptionPipeline(pipelineYaml);
    const isTTS = checkIfTTSPipeline(pipelineYaml);
    const isNoInput = checkIfNoInputPipeline(pipelineYaml);
    setIsTranscriptionPipeline(isTranscription);
    setIsTTSPipeline(isTTS);
    setIsNoInputPipeline(isNoInput);
    // Force playback mode for transcription pipelines
    if (isTranscription && outputMode !== 'playback') {
      setOutputMode('playback');
    }
    // TTS pipelines always output audio, so default to playback
    if (isTTS && outputMode !== 'playback') {
      setOutputMode('playback');
    }
  }, [
    pipelineYaml,
    outputMode,
    setIsTranscriptionPipeline,
    setIsTTSPipeline,
    setIsNoInputPipeline,
    setOutputMode,
  ]);

  // Update YAML when asset selection changes
  useEffect(() => {
    if (inputMode === 'asset' && selectedAssetId && selectedTemplateId) {
      const selectedAsset = audioAssets.find((a) => a.id === selectedAssetId);
      const selectedSample = samples.find((s) => s.id === selectedTemplateId);

      if (selectedAsset && selectedSample) {
        const modifiedYaml = injectFileReadNode(selectedSample.yaml, selectedAsset.path);
        setPipelineYaml(modifiedYaml);
      }
    }
  }, [selectedAssetId, inputMode, audioAssets, samples, selectedTemplateId, setPipelineYaml]);

  // Restore original YAML when switching back to upload mode
  useEffect(() => {
    if (inputMode === 'upload' && selectedTemplateId) {
      const selectedSample = samples.find((s) => s.id === selectedTemplateId);
      if (selectedSample) {
        setPipelineYaml(selectedSample.yaml);
      }
    }
  }, [inputMode, selectedTemplateId, samples, setPipelineYaml]);

  // Fetch samples on mount - intentionally empty deps to run once
  // useState setters are stable and safe to include in deps
  useEffect(() => {
    const fetchSamples = async () => {
      try {
        setSamplesLoading(true);
        setSamplesError(null);
        const fetchedSamples = await listSamples();

        // Filter to only show oneshot pipelines in convert view
        const oneshotSamples = fetchedSamples.filter((sample) => sample.mode === 'oneshot');
        const orderedSamples = orderSamplePipelinesSystemFirst(oneshotSamples);
        setSamples(orderedSamples);

        // Set default template if available
        if (orderedSamples.length > 0) {
          const defaultSample = orderedSamples[0];
          setSelectedTemplateId(defaultSample.id);
          setPipelineYaml(defaultSample.yaml);
          setIsTranscriptionPipeline(checkIfTranscriptionPipeline(defaultSample.yaml));
        }
      } catch (error) {
        viewsLogger.error('Failed to fetch samples:', error);
        setSamplesError(error instanceof Error ? error.message : 'Failed to load sample pipelines');
      } finally {
        setSamplesLoading(false);
      }
    };

    fetchSamples();
  }, [
    setSamples,
    setSamplesLoading,
    setSamplesError,
    setSelectedTemplateId,
    setPipelineYaml,
    setIsTranscriptionPipeline,
  ]);

  const handleTemplateSelect = (templateId: string) => {
    const sample = samples.find((s) => s.id === templateId);
    if (sample) {
      setSelectedTemplateId(templateId);

      // Reset asset selection when switching templates to avoid persisting state
      setSelectedAssetId('');

      // Set original YAML (asset selection will be reapplied via useEffect if needed)
      setPipelineYaml(sample.yaml);
      setIsTranscriptionPipeline(checkIfTranscriptionPipeline(sample.yaml));
      // Force playback mode for transcription pipelines
      if (checkIfTranscriptionPipeline(sample.yaml)) {
        setOutputMode('playback');
      }
    }
  };

  // Helper: Prepare input file based on pipeline type and mode
  const prepareInputFile = useCallback((): File | null => {
    if (isNoInputPipeline) {
      // No input needed - create empty file as placeholder for http_input
      const blob = new Blob([''], { type: 'application/octet-stream' });
      return new File([blob], 'empty', { type: 'application/octet-stream' });
    }

    if (isTTSPipeline) {
      // For TTS pipelines, convert text to a File object
      if (!textInput.trim()) {
        return null;
      }
      const blob = new Blob([textInput], { type: 'text/plain' });
      return new File([blob], 'input.txt', { type: 'text/plain' });
    }

    if (inputMode === 'upload') {
      if (!selectedFile) {
        return null;
      }
      return selectedFile;
    }

    // Asset mode - ensure asset is selected
    if (!selectedAssetId) {
      return null;
    }
    // YAML is already modified by useEffect, just use it directly
    return null;
  }, [inputMode, isNoInputPipeline, isTTSPipeline, selectedAssetId, selectedFile, textInput]);

  // Helper: Clean up previous conversion state
  const cleanupPreviousState = useCallback(() => {
    if (audioUrl && !useStreaming) {
      URL.revokeObjectURL(audioUrl);
    }
    setAudioUrl(null);
    setAudioContentType(null);
    setAudioStream(null);
    setUseStreaming(false);
    setMsePlaybackError(null);
    setMseFallbackLoading(false);
  }, [audioUrl, setAudioContentType, setAudioStream, setAudioUrl, setUseStreaming, useStreaming]);

  // Helper: Handle successful conversion result
  const handleConversionSuccess = useCallback(
    (result: Awaited<ReturnType<typeof convertFile>>) => {
      setMsePlaybackError(null);
      setMseFallbackLoading(false);
      const isJSON = result.contentType?.includes('application/json');
      const isStreaming = result.useStreaming && result.responseStream;

      // For streaming, keep processing status to show Cancel button
      if (!isStreaming) {
        setConversionStatus('success');
        setAbortController(null);
      }

      if (outputMode === 'playback') {
        if (isStreaming && result.responseStream) {
          // Increment stream key to force component remount with new stream
          setStreamKey((prev) => prev + 1);
          setAudioStream(result.responseStream);
          setAudioContentType(result.contentType || null);
          setUseStreaming(true);

          // Different message for JSON transcription vs audio streaming
          if (isJSON) {
            setConversionMessage(
              isTranscriptionPipeline
                ? 'Transcription in progress! Results will appear below as they are generated.'
                : 'Streaming JSON output… Results will appear below as they are generated.'
            );
          } else {
            setConversionMessage('Streaming audio... Click Cancel to stop.');
          }
          // Keep processing state for cancellation
        } else if (result.audioUrl) {
          // Use blob URL for other formats
          setAudioUrl(result.audioUrl);
          setAudioContentType(result.contentType || null);
          setConversionMessage('Conversion complete! You can now play the audio below.');
          setTimeout(() => {
            setConversionStatus('idle');
            setConversionMessage('');
          }, 5000);
        }
      } else {
        setConversionMessage('Conversion complete! Your file download should start automatically.');
        setTimeout(() => {
          setConversionStatus('idle');
          setConversionMessage('');
        }, 5000);
      }
    },
    [
      isTranscriptionPipeline,
      outputMode,
      setAbortController,
      setAudioContentType,
      setAudioStream,
      setAudioUrl,
      setConversionMessage,
      setConversionStatus,
      setStreamKey,
      setUseStreaming,
    ]
  );

  /**
   * Handles the conversion workflow end-to-end: input validation, API call, and streaming/download handling.
   */
  // eslint-disable-next-line max-statements -- Intentionally co-locates conversion state + error/cancel handling.
  const handleConvert = async () => {
    // Determine the input source
    const fileToConvert = prepareInputFile();
    if (fileToConvert === null && !selectedAssetId) {
      return; // Validation failed
    }

    // Clear previous audio URL/stream if it exists
    cleanupPreviousState();

    // Create a new AbortController for this request
    const controller = new AbortController();
    setAbortController(controller);

    setConversionStatus('processing');
    setConversionMessage('');

    try {
      const result = await convertFile(pipelineYaml, fileToConvert, outputMode, controller.signal);

      if (result.success) {
        handleConversionSuccess(result);
      } else {
        setConversionStatus('error');
        setAbortController(null);
        setConversionMessage(result.error || 'An unknown error occurred during conversion.');

        // Reset status after 8 seconds
        setTimeout(() => {
          setConversionStatus('idle');
          setConversionMessage('');
        }, 8000);
      }
    } catch (error) {
      // Check if this is an abort error (user cancelled)
      const isAbortError = error instanceof Error && error.name === 'AbortError';
      const isAbortRelated = error instanceof DOMException && error.name === 'AbortError';

      if (isAbortError || isAbortRelated) {
        viewsLogger.info('Conversion cancelled by user (caught AbortError)');
        // Only update state if we haven't already handled cancellation in handleCancel
        if (abortController) {
          setConversionStatus('idle');
          setConversionMessage('Conversion cancelled');
          setTimeout(() => {
            setConversionMessage('');
          }, 3000);
          setAbortController(null);
        } else {
          viewsLogger.debug('Cancellation already handled by handleCancel, ignoring');
        }
      } else {
        viewsLogger.error('Conversion error:', error);
        setConversionStatus('error');
        setConversionMessage(error instanceof Error ? error.message : 'An unknown error occurred');
        setTimeout(() => {
          setConversionStatus('idle');
          setConversionMessage('');
        }, 8000);
        setAbortController(null);
      }
    }
  };

  const handleCancel = () => {
    if (abortController) {
      try {
        // Abort the fetch - this will cause the convertFile promise to reject with AbortError
        abortController.abort();
      } catch (err) {
        // Ignore errors from abort() - it might already be aborted
        viewsLogger.debug('Error aborting (expected):', err);
      }

      // Clear ALL audio/stream state immediately
      // This will unmount MSEAudioPlayer/TranscriptionDisplay and trigger their cleanup
      if (audioUrl && !useStreaming) {
        URL.revokeObjectURL(audioUrl);
      }
      setAudioUrl(null);
      setAudioStream(null);
      setAudioContentType(null);
      setUseStreaming(false);
      setMsePlaybackError(null);
      setMseFallbackLoading(false);

      // Clear processing status and abort controller immediately
      setConversionStatus('idle');
      setAbortController(null);

      // Show cancellation message
      setConversionMessage('Conversion cancelled');
      setTimeout(() => {
        setConversionMessage('');
      }, 3000);
    }
  };

  const handleTranscriptionComplete = useCallback(() => {
    viewsLogger.info('Transcription stream complete');
    setConversionStatus('success');
    setAbortController(null);
    setConversionMessage('Transcription complete!');
    setTimeout(() => {
      setConversionStatus('idle');
      setConversionMessage('');
    }, 5000);
  }, [setConversionStatus, setAbortController, setConversionMessage]);

  const handleTranscriptionCancel = useCallback(() => {
    viewsLogger.debug('Transcription cancelled callback');
    // Only update if we still have an abort controller (not already handled by handleCancel)
    setAbortController((currentController) => {
      if (currentController) {
        setConversionStatus('idle');
        setConversionMessage('Transcription cancelled');
        setTimeout(() => {
          setConversionMessage('');
        }, 3000);
        return null;
      }
      return currentController;
    });
  }, [setAbortController, setConversionStatus, setConversionMessage]);

  const handleAudioStreamComplete = useCallback(() => {
    viewsLogger.info('Audio stream complete');
    setConversionStatus('success');
    setAbortController(null);
    setConversionMessage('Audio streaming complete!');
    setTimeout(() => {
      setConversionStatus('idle');
      setConversionMessage('');
    }, 5000);
  }, [setConversionStatus, setAbortController, setConversionMessage]);

  const handleAudioStreamCancel = useCallback(() => {
    viewsLogger.debug('Audio stream cancelled callback');
    // Only update if we still have an abort controller (not already handled by handleCancel)
    setAbortController((currentController) => {
      if (currentController) {
        setConversionStatus('idle');
        setConversionMessage('Audio streaming cancelled');
        setTimeout(() => {
          setConversionMessage('');
        }, 3000);
        return null;
      }
      return currentController;
    });
  }, [setAbortController, setConversionStatus, setConversionMessage]);

  const handleMsePlaybackError = useCallback(
    (message: string) => {
      setMsePlaybackError(message);
      setMseFallbackLoading(false);
      setAbortController(null);
      setConversionStatus('error');
      setConversionMessage(
        'Streaming playback failed in this browser. Use “Retry without streaming” to download then play.'
      );
      setTimeout(() => {
        setConversionStatus('idle');
        setConversionMessage('');
      }, 8000);
    },
    [setAbortController, setConversionStatus, setConversionMessage]
  );

  const handleRetryWithoutStreaming = useCallback(async () => {
    if (mseFallbackLoading) return;

    // Determine the input source
    const fileToConvert = prepareInputFile();
    if (fileToConvert === null && !selectedAssetId) {
      return;
    }

    // Abort any active streaming request (if still running)
    if (abortController) {
      try {
        abortController.abort();
      } catch (err) {
        viewsLogger.debug('Error aborting during fallback retry (expected):', err);
      }
      setAbortController(null);
    }

    cleanupPreviousState();

    const controller = new AbortController();
    setAbortController(controller);
    setConversionStatus('processing');
    setConversionMessage('Retrying playback without streaming...');
    setMseFallbackLoading(true);

    try {
      const result = await convertFile(pipelineYaml, fileToConvert, 'playback', controller.signal, {
        webmPlayback: 'blob',
      });

      if (result.success) {
        handleConversionSuccess(result);
      } else {
        setConversionStatus('error');
        setAbortController(null);
        setMseFallbackLoading(false);
        setConversionMessage(result.error || 'An unknown error occurred during conversion.');
        setTimeout(() => {
          setConversionStatus('idle');
          setConversionMessage('');
        }, 8000);
      }
    } catch (error) {
      viewsLogger.error('Fallback conversion error:', error);
      setConversionStatus('error');
      setAbortController(null);
      setMseFallbackLoading(false);
      setConversionMessage(error instanceof Error ? error.message : 'An unknown error occurred');
      setTimeout(() => {
        setConversionStatus('idle');
        setConversionMessage('');
      }, 8000);
    }
  }, [
    abortController,
    cleanupPreviousState,
    handleConversionSuccess,
    mseFallbackLoading,
    pipelineYaml,
    prepareInputFile,
    selectedAssetId,
    setAbortController,
    setConversionMessage,
    setConversionStatus,
  ]);

  const handleDownloadAudio = () => {
    if (!audioUrl) return;

    let outputFileName = 'converted_audio';

    if (inputMode === 'upload' && selectedFile) {
      const originalName = selectedFile.name;
      const baseName = originalName.includes('.')
        ? originalName.substring(0, originalName.lastIndexOf('.'))
        : originalName;
      outputFileName = `${baseName}_converted`;
    } else if (inputMode === 'asset' && selectedAssetId) {
      const selectedAsset = audioAssets.find((a) => a.id === selectedAssetId);
      if (selectedAsset) {
        outputFileName = `${selectedAsset.name}_converted`;
      }
    }

    const extension = audioContentType ? getExtensionFromContentType(audioContentType) : '.ogg';
    outputFileName += extension;

    // Create download link directly from the existing object URL
    const link = document.createElement('a');
    link.href = audioUrl;
    link.download = outputFileName;
    document.body.appendChild(link);
    link.click();
    document.body.removeChild(link);
  };

  const handleInputModeChange = (mode: 'upload' | 'asset') => {
    setInputMode(mode);
    // Clear the other mode's selection when switching
    if (mode === 'upload') {
      setSelectedAssetId('');
    } else {
      setSelectedFile(null);
    }
  };

  const canConvert =
    conversionStatus !== 'processing' &&
    (isNoInputPipeline
      ? true // No input needed for these pipelines
      : isTTSPipeline
        ? textInput.trim() !== ''
        : (inputMode === 'upload' && selectedFile !== null) ||
          (inputMode === 'asset' && selectedAssetId !== ''));

  return (
    <ViewContainer>
      <ContentArea>
        <ContentWrapper>
          <InfoBox>
            <InfoContent>
              <InfoTitle>Oneshot Pipelines (Request → Response)</InfoTitle>
              <div>
                This view runs StreamKit oneshot pipelines for file conversion and other
                request/response tasks. When you click "Convert", the server spins up a short-lived
                pipeline, streams the input through the graph, and streams the output back.
              </div>
              <div>
                Use oneshot when you want a single result (audio, JSON, or a file) rather than a
                long-running session.
              </div>
            </InfoContent>

            <TechnicalDetailsToggle onClick={() => setShowTechnicalDetails(!showTechnicalDetails)}>
              {showTechnicalDetails ? '▼' : '▶'} Technical Details
            </TechnicalDetailsToggle>

            {showTechnicalDetails && (
              <TechnicalDetails>
                <div>
                  <strong>Execution:</strong> The graph is compiled once and runs with a fixed set
                  of connections; it isn't reconfigured while processing your request.
                </div>
                <div>
                  <strong>I/O Nodes:</strong> Most templates start with{' '}
                  <code>streamkit::http_input</code> and end with{' '}
                  <code>streamkit::http_output</code>. Some also use <code>core::file_reader</code>{' '}
                  to read server-side files.
                </div>
                <div>
                  <strong>YAML Shape:</strong> Use <code>steps:</code> for simple chains, or{' '}
                  <code>nodes:</code> with <code>needs:</code> when you need branches or multiple
                  inputs.
                </div>
                {cliCommand && (
                  <CliSnippetContainer>
                    <CliSnippetHeader>
                      <CliSnippetLabel>Run via CLI (curl + ffplay):</CliSnippetLabel>
                      <CopyButton copied={cliCopied} onClick={handleCopyCliCommand}>
                        {cliCopied ? 'Copied!' : 'Copy'}
                      </CopyButton>
                    </CliSnippetHeader>
                    <CodeBlock>{cliCommand}</CodeBlock>
                  </CliSnippetContainer>
                )}
              </TechnicalDetails>
            )}
          </InfoBox>

          <Section>
            <SectionTitle>1. Select Pipeline Template</SectionTitle>
            {samplesLoading && <div>Loading sample pipelines...</div>}
            {samplesError && <div style={{ color: 'var(--sk-error)' }}>Error: {samplesError}</div>}
            {!samplesLoading && !samplesError && (
              <TemplateSelector
                templates={samples}
                selectedTemplateId={selectedTemplateId}
                onTemplateSelect={handleTemplateSelect}
              />
            )}
          </Section>

          <Section>
            <SectionTitle>2. Customize Pipeline (Optional)</SectionTitle>
            <EditorSection>
              <PipelineEditor
                value={pipelineYaml}
                onChange={setPipelineYaml}
                nodeDefinitions={nodeDefinitions}
              />
            </EditorSection>
          </Section>

          {!isNoInputPipeline && (
            <Section>
              <SectionTitle>
                3. {isTTSPipeline ? 'Enter Text to Convert to Speech' : 'Select Audio Input'}
              </SectionTitle>

              {isTTSPipeline ? (
                <TextInputContainer>
                  <TextAreaLabel htmlFor="text-input">
                    Enter the text you want to convert to speech:
                  </TextAreaLabel>
                  <TextArea
                    id="text-input"
                    value={textInput}
                    onChange={(e) => setTextInput(e.target.value)}
                    placeholder="Type or paste your text here... The text will be converted to natural-sounding speech using Kokoro TTS."
                    aria-label="Text input for TTS conversion"
                  />
                  <CharCounter>{textInput.length} characters</CharCounter>
                </TextInputContainer>
              ) : (
                <>
                  <InputModeSwitcher>
                    <ModeButton
                      active={inputMode === 'upload'}
                      onClick={() => handleInputModeChange('upload')}
                    >
                      Upload File
                    </ModeButton>
                    <ModeButton
                      active={inputMode === 'asset'}
                      onClick={() => handleInputModeChange('asset')}
                    >
                      Select Existing Asset
                    </ModeButton>
                  </InputModeSwitcher>

                  {inputMode === 'upload' ? (
                    <FileUpload file={selectedFile} onFileSelect={setSelectedFile} />
                  ) : (
                    <AssetSelector
                      assets={filteredAssets}
                      selectedAssetId={selectedAssetId}
                      onAssetSelect={setSelectedAssetId}
                      isLoading={assetsLoading}
                    />
                  )}
                </>
              )}
            </Section>
          )}

          {!isTranscriptionPipeline && !isTTSPipeline && (
            <Section>
              <SectionTitle>{isNoInputPipeline ? '3' : '4'}. Choose Output Mode</SectionTitle>
              <OutputModeContainer>
                <RadioGroupRoot
                  value={outputMode}
                  onValueChange={(value) => setOutputMode(value as OutputMode)}
                  aria-label="Output mode selection"
                >
                  <RadioWithLabel
                    value="playback"
                    label={
                      <span style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
                        <span>🎵</span>
                        <span>Play Audio</span>
                      </span>
                    }
                  />
                  <RadioWithLabel
                    value="download"
                    label={
                      <span style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
                        <span>⬇️</span>
                        <span>Download File</span>
                      </span>
                    }
                  />
                </RadioGroupRoot>
              </OutputModeContainer>
            </Section>
          )}

          <ConvertButtonContainer>
            {conversionStatus === 'processing' ? (
              <ConvertButton disabled={false} isProcessing={true} onClick={handleCancel}>
                <ButtonSpinner />
                Cancel
              </ConvertButton>
            ) : (
              <ConvertButton disabled={!canConvert} isProcessing={false} onClick={handleConvert}>
                {isNoInputPipeline
                  ? 'Generate'
                  : isTTSPipeline
                    ? 'Convert to Speech'
                    : isTranscriptionPipeline
                      ? 'Transcribe Audio'
                      : 'Convert File'}
              </ConvertButton>
            )}
          </ConvertButtonContainer>

          <ConversionProgress status={conversionStatus} message={conversionMessage} />

          {(audioUrl || audioStream) && (
            <div ref={resultsRef}>
              {audioContentType?.includes('application/json') && audioStream ? (
                isTranscriptionPipeline ? (
                  // Render transcription display for JSON content
                  // Use key to force remount when stream changes
                  <TranscriptionDisplay
                    key={streamKey}
                    stream={audioStream}
                    onComplete={handleTranscriptionComplete}
                    onCancel={handleTranscriptionCancel}
                  />
                ) : (
                  <JsonStreamDisplay
                    key={streamKey}
                    stream={audioStream}
                    onComplete={handleTranscriptionComplete}
                    onCancel={handleTranscriptionCancel}
                  />
                )
              ) : (
                // Render audio player for audio content
                <AudioPlayerContainer>
                  <AudioPlayerTitle>Converted Audio</AudioPlayerTitle>
                  {useStreaming && audioStream && audioContentType ? (
                    <MSEAudioPlayer
                      stream={audioStream}
                      contentType={audioContentType}
                      onComplete={handleAudioStreamComplete}
                      onCancel={handleAudioStreamCancel}
                      onError={handleMsePlaybackError}
                    />
                  ) : audioUrl ? (
                    <>
                      <HiddenAudio
                        ref={audioRef}
                        src={audioUrl}
                        preload="auto"
                        aria-label="Converted audio playback"
                      >
                        Your browser does not support the audio element.
                      </HiddenAudio>
                      <CustomAudioPlayer audioRef={audioRef} autoPlay />
                    </>
                  ) : null}
                  {audioUrl && (
                    <DownloadLink onClick={handleDownloadAudio}>Download Audio File</DownloadLink>
                  )}
                  {msePlaybackError && (
                    <DownloadLink onClick={handleRetryWithoutStreaming}>
                      {mseFallbackLoading ? 'Retrying…' : 'Retry without streaming'}
                    </DownloadLink>
                  )}
                </AudioPlayerContainer>
              )}
            </div>
          )}
          <BottomSpacer />
        </ContentWrapper>
      </ContentArea>
    </ViewContainer>
  );
};

export default ConvertView;
