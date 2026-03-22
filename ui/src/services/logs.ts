// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { fetchApi, getApiUrl } from './base';

export interface LogResponse {
  lines: string[];
  next_offset: number;
  has_more: boolean;
  file_size: number;
}

export interface LogQueryParams {
  offset?: number;
  limit?: number;
  direction?: 'forward' | 'backward';
  filter?: string;
  level?: string;
}

/**
 * Fetch a page of log lines from the server.
 */
export async function fetchLogs(params: LogQueryParams = {}): Promise<LogResponse> {
  const searchParams = new URLSearchParams();
  if (params.offset !== undefined) searchParams.set('offset', String(params.offset));
  if (params.limit !== undefined) searchParams.set('limit', String(params.limit));
  if (params.direction) searchParams.set('direction', params.direction);
  if (params.filter) searchParams.set('filter', params.filter);
  if (params.level) searchParams.set('level', params.level);

  const qs = searchParams.toString();
  const path = qs ? `/api/v1/logs?${qs}` : '/api/v1/logs';

  const response = await fetchApi(path);

  if (!response.ok) {
    if (response.status === 404) {
      throw new Error('Log file not available. File logging may be disabled.');
    }
    throw new Error(`Failed to fetch logs: ${response.statusText}`);
  }

  return response.json();
}

/**
 * Create an EventSource for live-tailing the log file via SSE.
 */
export function createLogStream(params?: { filter?: string; level?: string }): EventSource {
  const searchParams = new URLSearchParams();
  if (params?.filter) searchParams.set('filter', params.filter);
  if (params?.level) searchParams.set('level', params.level);

  const qs = searchParams.toString();
  const apiUrl = getApiUrl();
  const base = `${apiUrl}/api/v1/logs/stream`;
  const url = qs ? `${base}?${qs}` : base;

  return new EventSource(url, { withCredentials: true });
}
