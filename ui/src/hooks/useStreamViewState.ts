// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useState } from 'react';

import type { SamplePipeline } from '@/types/generated/api-types';

export type SessionCreationStatus = 'idle' | 'creating' | 'success' | 'error';

/** Grouped state for the StreamView component. */
export function useStreamViewState() {
  const [samples, setSamples] = useState<SamplePipeline[]>([]);
  const [samplesLoading, setSamplesLoading] = useState<boolean>(true);
  const [samplesError, setSamplesError] = useState<string | null>(null);

  const [pipelineYaml, setPipelineYaml] = useState<string>('');
  const [selectedTemplateId, setSelectedTemplateId] = useState<string>('');

  const [sessionName, setSessionName] = useState<string>('');
  const [sessionCreationStatus, setSessionCreationStatus] = useState<SessionCreationStatus>('idle');
  const [sessionCreationError, setSessionCreationError] = useState<string | null>(null);

  const [showPipelineSection, setShowPipelineSection] = useState<boolean>(true);

  return {
    samples,
    setSamples,
    samplesLoading,
    setSamplesLoading,
    samplesError,
    setSamplesError,
    pipelineYaml,
    setPipelineYaml,
    selectedTemplateId,
    setSelectedTemplateId,
    sessionName,
    setSessionName,
    sessionCreationStatus,
    setSessionCreationStatus,
    sessionCreationError,
    setSessionCreationError,
    showPipelineSection,
    setShowPipelineSection,
  };
}
