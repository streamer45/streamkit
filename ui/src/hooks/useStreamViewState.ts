// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useState } from 'react';

import type { SamplePipeline } from '@/types/generated/api-types';

export type SessionCreationStatus = 'idle' | 'creating' | 'success' | 'error';

/**
 * Custom hook to manage StreamView state
 * Handles pipeline template selection, YAML editing, and session creation
 *
 * Note: Setters from useState are guaranteed to be stable by React
 * and will never change between renders, so they're safe to use in
 * dependency arrays without causing unnecessary re-renders.
 */
export function useStreamViewState() {
  // Sample templates state
  const [samples, setSamples] = useState<SamplePipeline[]>([]);
  const [samplesLoading, setSamplesLoading] = useState<boolean>(true);
  const [samplesError, setSamplesError] = useState<string | null>(null);

  // Pipeline state
  const [pipelineYaml, setPipelineYaml] = useState<string>('');
  const [selectedTemplateId, setSelectedTemplateId] = useState<string>('');

  // Session creation state
  const [sessionName, setSessionName] = useState<string>('');
  const [sessionCreationStatus, setSessionCreationStatus] = useState<SessionCreationStatus>('idle');
  const [sessionCreationError, setSessionCreationError] = useState<string | null>(null);

  // UI state
  const [showPipelineSection, setShowPipelineSection] = useState<boolean>(true);

  return {
    // Sample templates
    samples,
    setSamples,
    samplesLoading,
    setSamplesLoading,
    samplesError,
    setSamplesError,

    // Pipeline
    pipelineYaml,
    setPipelineYaml,
    selectedTemplateId,
    setSelectedTemplateId,

    // Session creation
    sessionName,
    setSessionName,
    sessionCreationStatus,
    setSessionCreationStatus,
    sessionCreationError,
    setSessionCreationError,

    // UI
    showPipelineSection,
    setShowPipelineSection,
  };
}
