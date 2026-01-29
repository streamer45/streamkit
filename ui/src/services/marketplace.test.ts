// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { beforeEach, describe, expect, it, vi } from 'vitest';

import {
  cancelMarketplaceJob,
  getMarketplaceJob,
  getMarketplacePlugin,
  installMarketplacePlugin,
  listMarketplacePlugins,
  listMarketplaceRegistries,
} from './marketplace';

vi.mock('./base', () => ({
  getApiUrl: () => 'http://localhost:4545',
  fetchApi: (path: string, options: RequestInit = {}) => {
    const normalized = path.startsWith('/') ? path : `/${path}`;
    return fetch(`http://localhost:4545${normalized}`, { ...options, credentials: 'include' });
  },
}));

const mockJsonResponse = (payload: unknown) => {
  (global.fetch as ReturnType<typeof vi.fn>).mockResolvedValue({
    ok: true,
    status: 200,
    json: async () => payload,
    text: async () => '',
  });
};

describe('marketplace service', () => {
  beforeEach(() => {
    global.fetch = vi.fn() as never;
    vi.clearAllMocks();
  });

  it('lists registries', async () => {
    mockJsonResponse([]);

    await listMarketplaceRegistries();

    expect(global.fetch).toHaveBeenCalledWith(
      'http://localhost:4545/api/v1/marketplace/registries',
      { credentials: 'include' }
    );
  });

  it('lists marketplace plugins with query', async () => {
    mockJsonResponse({ schema_version: 1, plugins: [] });

    await listMarketplacePlugins('registry-url', 'whisper');

    expect(global.fetch).toHaveBeenCalledWith(
      'http://localhost:4545/api/v1/marketplace/plugins?registry=registry-url&q=whisper',
      { credentials: 'include' }
    );
  });

  it('fetches plugin details with version', async () => {
    mockJsonResponse({});

    await getMarketplacePlugin('registry-url', 'plugin-id', '1.2.3');

    expect(global.fetch).toHaveBeenCalledWith(
      'http://localhost:4545/api/v1/marketplace/plugins/plugin-id?registry=registry-url&version=1.2.3',
      { credentials: 'include' }
    );
  });

  it('starts install job', async () => {
    mockJsonResponse({ job_id: 'job-123' });
    const request = {
      registry: 'registry-url',
      plugin_id: 'plugin-id',
      version: '1.2.3',
      install_models: false,
    };

    await installMarketplacePlugin(request);

    expect(global.fetch).toHaveBeenCalledWith('http://localhost:4545/api/v1/plugins/install', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
      },
      body: JSON.stringify(request),
      credentials: 'include',
    });
  });

  it('fetches job status', async () => {
    mockJsonResponse({});

    await getMarketplaceJob('job-123');

    expect(global.fetch).toHaveBeenCalledWith('http://localhost:4545/api/v1/jobs/job-123', {
      credentials: 'include',
    });
  });

  it('cancels job', async () => {
    mockJsonResponse({});

    await cancelMarketplaceJob('job-123');

    expect(global.fetch).toHaveBeenCalledWith('http://localhost:4545/api/v1/jobs/job-123/cancel', {
      method: 'POST',
      credentials: 'include',
    });
  });
});
