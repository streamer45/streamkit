// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

/**
 * Service for managing sessions
 */

import type { SessionInfo } from '@/types/types';
import { getLogger } from '@/utils/logger';

import { getApiUrl } from './base';

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

/**
 * Lists all active sessions
 * @returns A promise that resolves to an array of sessions
 */
export async function listSessions(): Promise<SessionInfo[]> {
  const apiUrl = getApiUrl();
  const endpoint = `${apiUrl}/api/v1/sessions`;

  const response = await fetch(endpoint, {
    method: 'GET',
    headers: {
      'Content-Type': 'application/json',
    },
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
  const apiUrl = getApiUrl();
  const endpoint = `${apiUrl}/api/v1/sessions`;

  logger.info('Creating session:', name || '(unnamed)');

  const request: CreateSessionRequest = {
    name: name && name.trim() ? name.trim() : null,
    yaml,
  };

  const response = await fetch(endpoint, {
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
