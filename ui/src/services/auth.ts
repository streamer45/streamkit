// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { fetchApi } from './base';

export interface AuthMeResponse {
  authenticated: boolean;
  auth_enabled: boolean;
  role: string | null;
  jti: string | null;
}

export interface CreateTokenResponse {
  token: string;
  jti: string;
  exp: number;
  url_template?: string;
}

export interface TokenInfo {
  jti: string;
  token_type: string;
  role: string | null;
  label: string | null;
  created_at: number;
  exp: number;
  revoked: boolean;
  created_by: string;
}

export interface CreateApiTokenRequest {
  role: string;
  label?: string;
  ttl_secs?: number;
}

export interface CreateMoqTokenRequest {
  root: string;
  subscribe?: string[];
  publish?: string[];
  label?: string;
  ttl_secs?: number;
}

export async function fetchAuthMe(): Promise<AuthMeResponse> {
  const response = await fetchApi('/api/v1/auth/me', { method: 'GET' });
  if (!response.ok) {
    throw new Error(`Failed to fetch auth status: ${response.status} ${response.statusText}`);
  }
  return response.json() as Promise<AuthMeResponse>;
}

export async function loginWithToken(token: string): Promise<void> {
  const response = await fetchApi('/api/v1/auth/login', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ token }),
  });

  if (!response.ok) {
    const errorText = await response.text();
    throw new Error(errorText || `Login failed: ${response.status} ${response.statusText}`);
  }
}

export async function logout(): Promise<void> {
  const response = await fetchApi('/api/v1/auth/logout', { method: 'POST' });
  if (!response.ok) {
    const errorText = await response.text();
    throw new Error(errorText || `Logout failed: ${response.status} ${response.statusText}`);
  }
}

export async function listTokens(): Promise<TokenInfo[]> {
  const response = await fetchApi('/api/v1/auth/tokens', { method: 'GET' });
  if (!response.ok) {
    const errorText = await response.text();
    throw new Error(
      errorText || `Failed to list tokens: ${response.status} ${response.statusText}`
    );
  }
  return response.json() as Promise<TokenInfo[]>;
}

export async function createApiToken(req: CreateApiTokenRequest): Promise<CreateTokenResponse> {
  const response = await fetchApi('/api/v1/auth/tokens', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(req),
  });
  if (!response.ok) {
    const errorText = await response.text();
    throw new Error(
      errorText || `Failed to create API token: ${response.status} ${response.statusText}`
    );
  }
  return response.json() as Promise<CreateTokenResponse>;
}

export async function revokeToken(jti: string): Promise<void> {
  const response = await fetchApi(`/api/v1/auth/tokens/${encodeURIComponent(jti)}`, {
    method: 'DELETE',
  });
  if (!response.ok) {
    const errorText = await response.text();
    throw new Error(
      errorText || `Failed to revoke token: ${response.status} ${response.statusText}`
    );
  }
}

export async function createMoqToken(req: CreateMoqTokenRequest): Promise<CreateTokenResponse> {
  const response = await fetchApi('/api/v1/auth/moq-tokens', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      ...req,
      subscribe: req.subscribe ?? [],
      publish: req.publish ?? [],
    }),
  });
  if (!response.ok) {
    const errorText = await response.text();
    throw new Error(
      errorText || `Failed to create MoQ token: ${response.status} ${response.statusText}`
    );
  }
  return response.json() as Promise<CreateTokenResponse>;
}
