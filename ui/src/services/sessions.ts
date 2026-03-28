// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Service for managing sessions
 */

import type { SessionInfo } from '@/types/types';
import { getLogger } from '@/utils/logger';

import { fetchApi } from './base';

const logger = getLogger('sessions');

interface CreateSessionRequest {
  name: string | null;
  yaml: string;
}

interface CreateSessionResponse {
  session_id: string;
  name: string | null;
  created_at: string;
}

// ── Preview types ──────────────────────────────────────────────────────

export interface PreviewResponse {
  preview_id: string;
  gateway_path: string;
  broadcast: string;
  audio: boolean;
  video: boolean;
}

/**
 * Lists all active sessions
 * @returns A promise that resolves to an array of sessions
 */
export async function listSessions(signal?: AbortSignal): Promise<SessionInfo[]> {
  const response = await fetchApi('/api/v1/sessions', {
    method: 'GET',
    headers: {
      'Content-Type': 'application/json',
    },
    signal,
  });

  if (!response.ok) {
    throw new Error(`Failed to fetch sessions: ${response.statusText}`);
  }

  return response.json();
}

/**
 * Creates a new session with a pipeline from YAML
 * @param name - Optional session name
 * @param yaml - Pipeline definition in YAML format
 * @returns A promise that resolves to the created session info
 */
export async function createSession(
  name: string | null,
  yaml: string
): Promise<CreateSessionResponse> {
  logger.info('Creating session:', name || '(unnamed)');

  const request: CreateSessionRequest = {
    name: name && name.trim() ? name.trim() : null,
    yaml,
  };

  const response = await fetchApi('/api/v1/sessions', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(request),
  });

  if (!response.ok) {
    const errorText = await response.text();
    logger.error('Failed to create session:', {
      name,
      status: response.status,
      statusText: response.statusText,
      error: errorText,
    });
    throw new Error(errorText || `Failed to create session: ${response.statusText}`);
  }

  const result: CreateSessionResponse = await response.json();
  logger.info('Created session:', result.session_id, result.name || '(unnamed)');

  return result;
}

// ── Preview API ────────────────────────────────────────────────────────

/**
 * Starts an engine-native preview for a session by injecting a preview
 * subgraph into the running pipeline.
 */
export async function startPreview(
  sessionId: string,
  tapNode?: string,
  tapPin?: string
): Promise<PreviewResponse> {
  const body: Record<string, string> = {};
  if (tapNode) body.tap_node = tapNode;
  if (tapPin) body.tap_pin = tapPin;

  const response = await fetchApi(`/api/v1/sessions/${encodeURIComponent(sessionId)}/preview`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });

  if (!response.ok) {
    const errorText = await response.text();
    throw new Error(errorText || `Failed to start preview: ${response.statusText}`);
  }

  return response.json();
}

/**
 * Stops and tears down an active preview.
 */
export async function stopPreview(sessionId: string, previewId: string): Promise<void> {
  const response = await fetchApi(
    `/api/v1/sessions/${encodeURIComponent(sessionId)}/preview/${encodeURIComponent(previewId)}`,
    {
      method: 'DELETE',
      headers: { 'Content-Type': 'application/json' },
    }
  );

  if (!response.ok) {
    const errorText = await response.text();
    throw new Error(errorText || `Failed to stop preview: ${response.statusText}`);
  }
}
