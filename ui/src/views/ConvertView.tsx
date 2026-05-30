// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import styled from '@emotion/styled';
import { load as loadYaml } from 'js-yaml';
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { AssetSelector } from '@/components/converter/AssetSelector';
import { ConversionProgress } from '@/components/converter/ConversionProgress';
import { FileUpload } from '@/components/converter/FileUpload';
import { JsonStreamDisplay } from '@/components/converter/JsonStreamDisplay';
import { PipelineEditor } from '@/components/converter/PipelineEditor';
import { TemplateSelector } from '@/components/converter/TemplateSelector';
import { TranscriptionDisplay } from '@/components/converter/TranscriptionDisplay';
import { CustomAudioPlayer } from '@/components/CustomAudioPlayer';
import { MSEPlayer } from '@/components/MSEPlayer';
import { RadioGroupRoot, RadioWithLabel } from '@/components/ui/RadioGroup';
import {
  ViewContainer,
  ContentArea,
  ContentWrapper,
  BottomSpacer,
  Section,
  SectionTitle,
  InfoBox,
  InfoContent,
  InfoTitle,
  TechnicalDetailsToggle,
  TechnicalDetails,
} from '@/components/ui/ViewLayout';
import { useConvertViewState } from '@/hooks/useConvertViewState';
import { useAudioAssets } from '@/services/assets';
import {
  convertFile,
  getExtensionFromContentType,
  type OutputMode,
  type UploadField,
} from '@/services/converter';
import { listSamples } from '@/services/samples';
import { ensureSchemasLoaded, useSchemaStore } from '@/stores/schemaStore';
import {
  extractClientFromParsed,
  parseAcceptToFormats,
  parseClientFromYaml,
} from '@/utils/clientSection';
import { viewsLogger } from '@/utils/logger';
import { orderSamplePipelinesSystemFirst } from '@/utils/samplePipelineOrdering';
import { injectFileReadNode } from '@/utils/yamlPipeline';

type HttpInputField = { name: string; required: boolean };

const resolveUploadFields = (httpInputFields: HttpInputField[]): HttpInputField[] =>
  httpInputFields.length > 0 ? httpInputFields : [{ name: 'media', required: true }];

const normalizeHttpInputField = (entry: unknown): HttpInputField | null => {
  if (typeof entry === 'string' && entry.trim()) {
    return { name: entry.trim(), required: true };
  }
  if (entry && typeof entry === 'object' && 'name' in (entry as Record<string, unknown>)) {
    const name = String((entry as Record<string, unknown>).name ?? '').trim();
    if (!name) return null;
    const required = (entry as Record<string, unknown>).required;
    return { name, required: typeof required === 'boolean' ? required : true };
  }
  return null;
};

const isRecord = (value: unknown): value is Record<string, unknown> =>
  Boolean(value) && typeof value === 'object' && !Array.isArray(value);

const extractFieldsFromNode = (
  label: string,
  node: Record<string, unknown>,
  defaultField: string | null
): HttpInputField[] => {
  const params = isRecord(node.params) ? (node.params as Record<string, unknown>) : {};
  const fieldsVal = params.fields;

  if (Array.isArray(fieldsVal)) {
    return fieldsVal
      .map((entry) => normalizeHttpInputField(entry))
      .filter((f): f is HttpInputField => Boolean(f));
  }

  const fieldVal = typeof params.field === 'string' ? params.field.trim() : '';
  if (fieldVal) {
    const required = typeof params.required === 'boolean' ? (params.required as boolean) : true;
    return [{ name: fieldVal, required }];
  }

  const fallback = defaultField ?? label;
  return fallback ? [{ name: fallback, required: defaultField ? false : true }] : [];
};

const deriveHttpInputFieldsFromParsed = (
  parsed: Record<string, unknown> | null | undefined
): { fields: HttpInputField[]; hasHttpInput: boolean } => {
  try {
    if (!parsed || typeof parsed !== 'object') return { fields: [], hasHttpInput: false };

    if (isRecord(parsed.nodes)) {
      const httpEntries = Object.entries(parsed.nodes).filter(
        ([, node]) => isRecord(node) && node.kind === 'streamkit::http_input'
      );
      if (httpEntries.length === 0) return { fields: [], hasHttpInput: false };

      const defaultField = httpEntries.length === 1 ? 'media' : null;
      const fields = httpEntries.flatMap(([label, node]) =>
        extractFieldsFromNode(label, node as Record<string, unknown>, defaultField)
      );

      const unique = new Map<string, HttpInputField>();
      fields.forEach((f) => unique.set(f.name, f));
      return { fields: Array.from(unique.values()), hasHttpInput: true };
    }

    if (Array.isArray(parsed.steps)) {
      const hasHttpInput = parsed.steps.some(
        (s) => isRecord(s) && typeof s.kind === 'string' && s.kind === 'streamkit::http_input'
      );
      if (hasHttpInput) {
        return { fields: [{ name: 'media', required: true }], hasHttpInput: true };
      }
    }

    return { fields: [], hasHttpInput: false };
  } catch {
    return { fields: [], hasHttpInput: false };
  }
};

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

const resolveTextField = (fields: HttpInputField[]): HttpInputField | null => {
  if (fields.length === 0) {
    return null;
  }
  const textField = fields.find((field) => field.name.toLowerCase() === 'text');
  return textField ?? fields[0];
};

const splitTtsFields = (
  fields: HttpInputField[]
): { textField: HttpInputField | null; extraFields: HttpInputField[] } => {
  const textField = resolveTextField(fields);
  if (!textField) {
    return { textField: null, extraFields: [] };
  }
  return {
    textField,
    extraFields: fields.filter((field) => field.name !== textField.name),
  };
};

const buildNoInputUploads = (fields: HttpInputField[]): UploadField[] => {
  if (fields.length === 0) return [];

  const blob = new Blob([''], { type: 'application/octet-stream' });
  const file = new File([blob], 'empty', { type: 'application/octet-stream' });
  return [{ field: fields[0].name, file }];
};

const buildTtsUploads = (
  fields: HttpInputField[],
  textInput: string,
  fieldUploads: Record<string, File | null>
): UploadField[] | null => {
  if (!textInput.trim()) {
    return null;
  }

  const { textField, extraFields } = splitTtsFields(fields);
  const textFieldName = textField?.name ?? fields[0]?.name;
  if (!textFieldName) {
    return null;
  }

  const uploads: UploadField[] = [];
  const blob = new Blob([textInput], { type: 'text/plain' });
  const file = new File([blob], 'input.txt', { type: 'text/plain' });
  uploads.push({ field: textFieldName, file });

  for (const field of extraFields) {
    const fieldFile = fieldUploads[field.name];
    if (!fieldFile) {
      if (field.required) return null;
      continue;
    }
    uploads.push({ field: field.name, file: fieldFile });
  }

  return uploads;
};

const buildUploadModeUploads = (
  fields: HttpInputField[],
  fieldUploads: Record<string, File | null>,
  selectedFile: File | null
): UploadField[] | null => {
  if (fields.length > 1) {
    const uploads: UploadField[] = [];
    for (const field of fields) {
      const file = fieldUploads[field.name];
      if (!file) {
        if (field.required) return null;
        continue;
      }
      uploads.push({ field: field.name, file });
    }
    return uploads;
  }

  if (!selectedFile) {
    return null;
  }
  return [{ field: fields[0].name, file: selectedFile }];
};

const assetMatchesTag = (assetId: string, tag: string): boolean => {
  if (tag === 'speech') return assetId.toLowerCase().startsWith('speech_');
  if (tag === 'music') return assetId.toLowerCase().startsWith('music_');
  if (tag.startsWith('id:'))
    return assetId.toLowerCase() === tag.slice('id:'.length).trim().toLowerCase();
  return false;
};

/**
 * Generates a CLI command for running the pipeline with curl + ffplay
 */
const generateCliCommand = (
  templateId: string,
  isNoInput: boolean,
  isTTS: boolean,
  fields: HttpInputField[],
  serverUrl: string = 'http://127.0.0.1:4545'
): string => {
  const configPath = `samples/pipelines/${templateId}.yml`;
  const activeFields = fields.length > 0 ? fields : [{ name: 'media', required: true }];

  if (isNoInput) {
    return `curl --no-buffer \\
  -F config=@${configPath} \\
  -F media= \\
  ${serverUrl}/api/v1/process -o - | ffplay -f webm -i -`;
  }

  if (isTTS) {
    const { textField, extraFields } = splitTtsFields(activeFields);
    const textFieldName = textField?.name ?? 'media';
    const extraLines = extraFields
      .map((field) => `  -F ${field.name}=@your-${field.name}.wav \\`)
      .join('\n');
    const extraBlock = extraLines ? `${extraLines}\n` : '';
    return `echo "Your text here" | curl --no-buffer \\
  -F config=@${configPath} \\
  -F '${textFieldName}=@-;type=text/plain' \\
${extraBlock}  ${serverUrl}/api/v1/process -o - | ffplay -f webm -i -`;
  }

  if (activeFields.length > 1) {
    if (
      templateId.endsWith('oneshot/dual_upload_mixing') &&
      activeFields.some((f) => f.name === 'track_a') &&
      activeFields.some((f) => f.name === 'track_b')
    ) {
      return `curl --no-buffer \\
  -F config=@${configPath} \\
  -F track_a=@samples/audio/system/speech_2m.opus \\
  -F "track_b=@samples/audio/system/THE LADY IS A TRAMP.opus" \\
  ${serverUrl}/api/v1/process | ffplay -nodisp -autoexit -f webm -i -`;
    }

    const fieldLines = activeFields.map((f) => `  -F ${f.name}=@your-${f.name}.ogg \\`).join('\n');

    return `curl --no-buffer \\
  -F config=@${configPath} \\
${fieldLines}
  ${serverUrl}/api/v1/process -o - | ffplay -f webm -i -`;
  }

  return `curl --no-buffer \\
  -F config=@${configPath} \\
  -F media=@your-audio-file.ogg \\
  ${serverUrl}/api/v1/process -o - | ffplay -f webm -i -`;
};

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
  } = useConvertViewState();

  const [httpInputFields, setHttpInputFields] = useState<HttpInputField[]>([]);
  const [hasHttpInput, setHasHttpInput] = useState(false);
  const [fieldUploads, setFieldUploads] = useState<Record<string, File | null>>({});
  const [cliCopied, setCliCopied] = useState(false);
  const [msePlaybackError, setMsePlaybackError] = useState<string | null>(null);
  const [mseFallbackLoading, setMseFallbackLoading] = useState<boolean>(false);

  const parsedPipelineYaml = useMemo(() => {
    try {
      const parsed = loadYaml(pipelineYaml) as Record<string, unknown> | null;
      return parsed && typeof parsed === 'object' ? parsed : null;
    } catch {
      return null;
    }
  }, [pipelineYaml]);

  const client = useMemo(() => extractClientFromParsed(parsedPipelineYaml), [parsedPipelineYaml]);
  const isTranscriptionPipeline = client?.output?.type === 'transcription';
  const isTTSPipeline = client?.input?.type === 'text';
  const isNoInputPipeline = client?.input?.type === 'none' || client?.input?.type === 'trigger';
  const isVideoPipeline = client?.output?.type === 'video';

  const cliCommand = useMemo(() => {
    if (!selectedTemplateId) return '';
    return generateCliCommand(
      selectedTemplateId,
      isNoInputPipeline,
      isTTSPipeline,
      httpInputFields
    );
  }, [selectedTemplateId, isNoInputPipeline, isTTSPipeline, httpInputFields]);

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

  const resultsRef = useRef<HTMLDivElement | null>(null);

  const audioRef = useRef<HTMLAudioElement | null>(null);

  const nodeDefinitions = useSchemaStore((s) => s.nodeDefinitions);

  useEffect(() => {
    ensureSchemasLoaded();
  }, []);

  useEffect(() => {
    if ((mediaUrl || mediaStream) && resultsRef.current) {
      const timeoutId = setTimeout(() => {
        resultsRef.current?.scrollIntoView({ behavior: 'smooth', block: 'start' });
      }, 100);
      return () => clearTimeout(timeoutId);
    }
  }, [mediaUrl, mediaStream]);

  const { data: audioAssets = [], isLoading: assetsLoading } = useAudioAssets();

  const getFieldHint = useCallback(
    (fieldName: string): { accept?: string; hint?: string } => {
      const hints = client?.input?.field_hints;
      if (hints) {
        const fh = hints[fieldName];
        if (fh) {
          const formats = parseAcceptToFormats(fh.accept);
          if (formats && formats.length > 0) {
            const accept = formats.map((f) => `.${f}`).join(',');
            const label = formats.map((f) => f.toUpperCase()).join(', ');
            return { accept, hint: `Expected format${formats.length > 1 ? 's' : ''}: ${label}` };
          }
          if (fh.accept) return { accept: fh.accept };
        }
      }
      const topFormats = parseAcceptToFormats(client?.input?.accept);
      if (topFormats && topFormats.length > 0) {
        const accept = topFormats.map((f) => `.${f}`).join(',');
        const label = topFormats.map((f) => f.toUpperCase()).join(', ');
        return { accept, hint: `Expected format${topFormats.length > 1 ? 's' : ''}: ${label}` };
      }
      return {};
    },
    [client]
  );

  const assetFormats = useMemo(() => parseAcceptToFormats(client?.input?.accept), [client]);
  const inputAssetTags = client?.input?.asset_tags ?? null;
  const filteredAssets = useMemo(() => {
    if (!pipelineYaml) return audioAssets;

    if (httpInputFields.length > 1) {
      if (!assetFormats) return audioAssets;
      return audioAssets.filter((asset) => assetFormats.includes(asset.format.toLowerCase()));
    }

    if (!assetFormats && !inputAssetTags) {
      viewsLogger.debug('No specific format required, showing all assets');
      return audioAssets;
    }

    viewsLogger.debug('Expected formats:', assetFormats, 'Total assets:', audioAssets.length);

    const formatFiltered = assetFormats
      ? audioAssets.filter((asset) => assetFormats.includes(asset.format.toLowerCase()))
      : audioAssets;

    const tagFiltered = inputAssetTags
      ? formatFiltered.filter((asset) =>
          inputAssetTags.some((tag) => assetMatchesTag(asset.id, tag))
        )
      : formatFiltered;

    viewsLogger.debug('Filtered to', tagFiltered.length, 'compatible assets');

    return tagFiltered;
  }, [audioAssets, pipelineYaml, httpInputFields.length, assetFormats, inputAssetTags]);

  useEffect(() => {
    if (selectedAssetId && !filteredAssets.some((asset) => asset.id === selectedAssetId)) {
      viewsLogger.debug('Selected asset not compatible with pipeline, clearing selection');
      setSelectedAssetId('');
    }
  }, [filteredAssets, selectedAssetId, setSelectedAssetId]);

  useEffect(() => {
    const { fields, hasHttpInput: hasHttp } = deriveHttpInputFieldsFromParsed(parsedPipelineYaml);
    setHasHttpInput(hasHttp);
    setHttpInputFields(fields);
    setFieldUploads((prev) => {
      const next: Record<string, File | null> = {};
      fields.forEach((f) => {
        next[f.name] = prev[f.name] ?? null;
      });
      return next;
    });
  }, [parsedPipelineYaml]);

  useEffect(() => {
    if ((isTranscriptionPipeline || isTTSPipeline) && outputMode !== 'playback') {
      setOutputMode('playback');
    }
  }, [isTranscriptionPipeline, isTTSPipeline, outputMode, setOutputMode]);

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

  useEffect(() => {
    if (inputMode === 'upload' && selectedTemplateId) {
      const selectedSample = samples.find((s) => s.id === selectedTemplateId);
      if (selectedSample) {
        setPipelineYaml(selectedSample.yaml);
      }
    }
  }, [inputMode, selectedTemplateId, samples, setPipelineYaml]);

  useEffect(() => {
    const fetchSamples = async () => {
      try {
        setSamplesLoading(true);
        setSamplesError(null);
        const fetchedSamples = await listSamples();

        const oneshotSamples = fetchedSamples.filter((sample) => sample.mode === 'oneshot');
        const orderedSamples = orderSamplePipelinesSystemFirst(oneshotSamples);
        setSamples(orderedSamples);

        if (orderedSamples.length > 0) {
          const defaultSample = orderedSamples[0];
          setSelectedTemplateId(defaultSample.id);
          setPipelineYaml(defaultSample.yaml);
        }
      } catch (error) {
        viewsLogger.error('Failed to fetch samples:', error);
        setSamplesError(error instanceof Error ? error.message : 'Failed to load sample pipelines');
      } finally {
        setSamplesLoading(false);
      }
    };

    fetchSamples();
  }, [setSamples, setSamplesLoading, setSamplesError, setSelectedTemplateId, setPipelineYaml]);

  const handleTemplateSelect = (templateId: string) => {
    const sample = samples.find((s) => s.id === templateId);
    if (sample) {
      setSelectedTemplateId(templateId);

      setSelectedAssetId('');

      setPipelineYaml(sample.yaml);
      const templateClient = parseClientFromYaml(sample.yaml);
      if (templateClient?.output?.type === 'transcription') {
        setOutputMode('playback');
      }
    }
  };

  const prepareUploads = useCallback(async (): Promise<UploadField[] | null> => {
    if (isNoInputPipeline || (!client && !hasHttpInput)) {
      return buildNoInputUploads(httpInputFields);
    }

    const fields = resolveUploadFields(httpInputFields);

    if (isTTSPipeline) {
      return buildTtsUploads(fields, textInput, fieldUploads);
    }

    if (inputMode === 'upload') {
      return buildUploadModeUploads(fields, fieldUploads, selectedFile);
    }

    if (!selectedAssetId || !hasHttpInput) {
      return [];
    }
    return [];
  }, [
    client,
    fieldUploads,
    httpInputFields,
    inputMode,
    isNoInputPipeline,
    isTTSPipeline,
    hasHttpInput,
    selectedAssetId,
    selectedFile,
    textInput,
  ]);

  const cleanupPreviousState = useCallback(() => {
    if (mediaUrl && !useStreaming) {
      URL.revokeObjectURL(mediaUrl);
    }
    setMediaUrl(null);
    setMediaContentType(null);
    setMediaStream(null);
    setUseStreaming(false);
    setMsePlaybackError(null);
    setMseFallbackLoading(false);
  }, [mediaUrl, setMediaContentType, setMediaStream, setMediaUrl, setUseStreaming, useStreaming]);

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
          setStreamKey((prev) => prev + 1);
          setMediaStream(result.responseStream);
          setMediaContentType(result.contentType || null);
          setUseStreaming(true);

          // Different message for JSON transcription vs audio streaming
          if (isJSON) {
            setConversionMessage(
              isTranscriptionPipeline
                ? 'Transcription in progress! Results will appear below as they are generated.'
                : 'Streaming JSON output… Results will appear below as they are generated.'
            );
          } else {
            const mediaKind = result.contentType?.startsWith('video/') ? 'video' : 'audio';
            setConversionMessage(`Streaming ${mediaKind}... Click Cancel to stop.`);
          }
          // Keep processing state for cancellation
        } else if (result.mediaUrl) {
          // Use blob URL for other formats
          setMediaUrl(result.mediaUrl);
          setMediaContentType(result.contentType || null);
          const mediaKind = result.contentType?.startsWith('video/') ? 'video' : 'audio';
          setConversionMessage(`Conversion complete! You can now play the ${mediaKind} below.`);
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
      setMediaContentType,
      setMediaStream,
      setMediaUrl,
      setConversionMessage,
      setConversionStatus,
      setStreamKey,
      setUseStreaming,
    ]
  );

  // eslint-disable-next-line max-statements -- Co-locates conversion state + error/cancel handling
  const handleConvert = async () => {
    const uploads = await prepareUploads();
    if (uploads === null) return;

    cleanupPreviousState();

    const controller = new AbortController();
    setAbortController(controller);

    setConversionStatus('processing');
    setConversionMessage('');

    try {
      const playback = outputMode === 'playback' ? 'auto' : 'blob';
      const result = await convertFile(pipelineYaml, uploads, outputMode, controller.signal, {
        playback,
      });

      if (result.success) {
        handleConversionSuccess(result);
      } else {
        setConversionStatus('error');
        setAbortController(null);
        setConversionMessage(result.error || 'An unknown error occurred during conversion.');

        setTimeout(() => {
          setConversionStatus('idle');
          setConversionMessage('');
        }, 8000);
      }
    } catch (error) {
      const isAbortError = error instanceof Error && error.name === 'AbortError';
      const isAbortRelated = error instanceof DOMException && error.name === 'AbortError';

      if (isAbortError || isAbortRelated) {
        viewsLogger.info('Conversion cancelled by user (caught AbortError)');
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
        abortController.abort();
      } catch (err) {
        viewsLogger.debug('Error aborting (expected):', err);
      }

      if (mediaUrl && !useStreaming) {
        URL.revokeObjectURL(mediaUrl);
      }
      setMediaUrl(null);
      setMediaStream(null);
      setMediaContentType(null);
      setUseStreaming(false);
      setMsePlaybackError(null);
      setMseFallbackLoading(false);

      setConversionStatus('idle');
      setAbortController(null);
      setConversionMessage('Conversion cancelled');
      setTimeout(() => {
        setConversionMessage('');
      }, 3000);
    }
  };

  /** Build onComplete / onCancel callbacks for a streaming result (transcription or audio). */
  const makeStreamCallbacks = useCallback(
    (label: string) => ({
      onComplete: () => {
        viewsLogger.info(`${label} stream complete`);
        setConversionStatus('success');
        setAbortController(null);
        setConversionMessage(`${label} complete!`);
        setTimeout(() => {
          setConversionStatus('idle');
          setConversionMessage('');
        }, 5000);
      },
      onCancel: () => {
        viewsLogger.debug(`${label} cancelled callback`);
        setAbortController((currentController) => {
          if (currentController) {
            setConversionStatus('idle');
            setConversionMessage(`${label} cancelled`);
            setTimeout(() => {
              setConversionMessage('');
            }, 3000);
            return null;
          }
          return currentController;
        });
      },
    }),
    [setConversionStatus, setAbortController, setConversionMessage]
  );

  const transcriptionCallbacks = useMemo(
    () => makeStreamCallbacks('Transcription'),
    [makeStreamCallbacks]
  );
  const mediaStreamCallbacks = useMemo(
    () => makeStreamCallbacks('Streaming'),
    [makeStreamCallbacks]
  );

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

    const uploads = await prepareUploads();
    if (uploads === null) return;

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
      const result = await convertFile(pipelineYaml, uploads, 'playback', controller.signal, {
        playback: 'blob',
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
    prepareUploads,
    setAbortController,
    setConversionMessage,
    setConversionStatus,
  ]);

  const uploadFields =
    httpInputFields.length > 0 ? httpInputFields : [{ name: 'media', required: true }];
  const isMultiUpload = uploadFields.length > 1;
  const { extraFields: ttsExtraFields } = splitTtsFields(uploadFields);
  const ttsMissingRequiredUploads = ttsExtraFields.some(
    (field) => field.required && !fieldUploads[field.name]
  );
  const singleUploadHint = uploadFields.length > 0 ? getFieldHint(uploadFields[0].name) : {};

  const handleDownloadAudio = () => {
    if (!mediaUrl) return;

    const extension = mediaContentType ? getExtensionFromContentType(mediaContentType) : '.ogg';

    const primaryUpload =
      isMultiUpload && inputMode === 'upload'
        ? (uploadFields.map((f) => fieldUploads[f.name]).find((f): f is File => Boolean(f)) ?? null)
        : selectedFile;

    let outputFileName: string;
    if (inputMode === 'upload' && primaryUpload) {
      const originalName = primaryUpload.name;
      const baseName = originalName.includes('.')
        ? originalName.substring(0, originalName.lastIndexOf('.'))
        : originalName;
      outputFileName = `${baseName}_converted${extension}`;
    } else if (inputMode === 'asset' && selectedAssetId) {
      const selectedAsset = audioAssets.find((a) => a.id === selectedAssetId);
      outputFileName = selectedAsset
        ? `${selectedAsset.name}_converted${extension}`
        : `output${extension}`;
    } else {
      outputFileName = `output${extension}`;
    }

    const link = document.createElement('a');
    link.href = mediaUrl;
    link.download = outputFileName;
    document.body.appendChild(link);
    link.click();
    document.body.removeChild(link);
  };

  const handleInputModeChange = (mode: 'upload' | 'asset') => {
    setInputMode(mode);
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
        ? textInput.trim() !== '' && !ttsMissingRequiredUploads
        : (() => {
            if (!hasHttpInput) {
              return true;
            }

            if (isMultiUpload) {
              return uploadFields.every((f) => (f.required ? !!fieldUploads[f.name] : true));
            }

            if (inputMode === 'upload') {
              return selectedFile !== null;
            }

            if (inputMode === 'asset') {
              return selectedAssetId !== '';
            }

            return false;
          })());

  return (
    <ViewContainer data-testid="convert-view">
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
                  {ttsExtraFields.length > 0 && (
                    <div style={{ marginTop: '12px' }}>
                      <TextAreaLabel>Additional uploads:</TextAreaLabel>
                      {ttsExtraFields.map((field) => {
                        const hint = getFieldHint(field.name);
                        return (
                          <div key={field.name} style={{ marginTop: '8px' }}>
                            <TextAreaLabel>
                              {field.name}
                              {!field.required ? ' (optional)' : ''}
                            </TextAreaLabel>
                            <FileUpload
                              file={fieldUploads[field.name] ?? null}
                              onFileSelect={(file) =>
                                setFieldUploads((prev) => ({ ...prev, [field.name]: file }))
                              }
                              accept={hint.accept}
                              hint={hint.hint}
                            />
                          </div>
                        );
                      })}
                    </div>
                  )}
                </TextInputContainer>
              ) : (
                <>
                  {!isMultiUpload && (
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
                        disabled={isMultiUpload}
                      >
                        Select Existing Asset
                      </ModeButton>
                    </InputModeSwitcher>
                  )}

                  {inputMode === 'upload' || isMultiUpload ? (
                    isMultiUpload ? (
                      <div>
                        <p>
                          This pipeline expects multiple uploads. For each field, choose an upload
                          or pick an existing asset.
                        </p>
                        {uploadFields.map((field) => {
                          const hint = getFieldHint(field.name);
                          return (
                            <div key={field.name} style={{ marginBottom: '12px' }}>
                              <TextAreaLabel>
                                {field.name}
                                {!field.required ? ' (optional)' : ''}
                              </TextAreaLabel>
                              <FileUpload
                                file={fieldUploads[field.name] ?? null}
                                onFileSelect={(file) =>
                                  setFieldUploads((prev) => ({ ...prev, [field.name]: file }))
                                }
                                accept={hint.accept}
                                hint={hint.hint}
                              />
                            </div>
                          );
                        })}
                      </div>
                    ) : (
                      <FileUpload
                        file={selectedFile}
                        onFileSelect={setSelectedFile}
                        accept={singleUploadHint.accept}
                        hint={singleUploadHint.hint}
                      />
                    )
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
                        <span>{isVideoPipeline ? '🎬' : '🎵'}</span>
                        <span>{isVideoPipeline ? 'Play Video' : 'Play Audio'}</span>
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
                {isNoInputPipeline && inputMode !== 'asset'
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

          {(mediaUrl || mediaStream) && (
            <div ref={resultsRef}>
              {mediaContentType?.includes('application/json') && mediaStream ? (
                isTranscriptionPipeline ? (
                  <TranscriptionDisplay
                    key={streamKey}
                    stream={mediaStream}
                    onComplete={transcriptionCallbacks.onComplete}
                    onCancel={transcriptionCallbacks.onCancel}
                  />
                ) : (
                  <JsonStreamDisplay
                    key={streamKey}
                    stream={mediaStream}
                    onComplete={transcriptionCallbacks.onComplete}
                    onCancel={transcriptionCallbacks.onCancel}
                  />
                )
              ) : (
                <AudioPlayerContainer>
                  <AudioPlayerTitle>
                    {mediaContentType?.startsWith('video/') ? 'Converted Video' : 'Converted Audio'}
                  </AudioPlayerTitle>
                  {useStreaming && mediaStream && mediaContentType ? (
                    <MSEPlayer
                      stream={mediaStream}
                      contentType={mediaContentType}
                      onComplete={mediaStreamCallbacks.onComplete}
                      onCancel={mediaStreamCallbacks.onCancel}
                      onError={handleMsePlaybackError}
                    />
                  ) : mediaUrl ? (
                    mediaContentType?.startsWith('video/') ? (
                      <video
                        src={mediaUrl}
                        controls
                        autoPlay
                        style={{
                          width: '100%',
                          maxHeight: 480,
                          borderRadius: 6,
                          background: '#000',
                        }}
                        aria-label="Converted video playback"
                      />
                    ) : (
                      <>
                        <HiddenAudio
                          ref={audioRef}
                          src={mediaUrl}
                          preload="auto"
                          aria-label="Converted audio playback"
                        >
                          Your browser does not support the audio element.
                        </HiddenAudio>
                        <CustomAudioPlayer audioRef={audioRef} autoPlay />
                      </>
                    )
                  ) : null}
                  {mediaUrl && (
                    <DownloadLink onClick={handleDownloadAudio}>
                      {mediaContentType?.startsWith('video/')
                        ? 'Download Video File'
                        : 'Download Audio File'}
                    </DownloadLink>
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
