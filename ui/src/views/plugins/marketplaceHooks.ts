// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { useCallback, useEffect, useState } from 'react';

import {
  cancelMarketplaceJob,
  getMarketplaceJob,
  getMarketplacePlugin,
  listMarketplacePlugins,
  listMarketplaceRegistries,
} from '@/services/marketplace';
import type {
  JobInfo,
  MarketplaceIndex,
  MarketplacePluginDetails,
  MarketplaceRegistry,
} from '@/types/marketplace';

export const useMarketplaceRegistries = (active: boolean) => {
  const [registries, setRegistries] = useState<MarketplaceRegistry[]>([]);
  const [selectedRegistry, setSelectedRegistry] = useState('');
  const [loading, setLoading] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!active || loaded) return;
    let cancelled = false;
    setLoading(true);
    setError(null);

    (async () => {
      try {
        const registryList = await listMarketplaceRegistries();
        if (cancelled) return;
        setRegistries(registryList);
        setSelectedRegistry((prev) => prev || registryList[0]?.id || '');
      } catch (err) {
        if (!cancelled) {
          const message = err instanceof Error ? err.message : 'Failed to load registries.';
          setError(message);
        }
      } finally {
        if (!cancelled) {
          setLoading(false);
          setLoaded(true);
        }
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [active, loaded]);

  return {
    registries,
    selectedRegistry,
    setSelectedRegistry,
    loading,
    loaded,
    error,
  };
};

export const useMarketplaceIndex = (active: boolean, registry: string, query: string) => {
  const [index, setIndex] = useState<MarketplaceIndex | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!active) return;
    if (!registry) {
      setIndex(null);
      return;
    }

    let cancelled = false;
    setLoading(true);
    setError(null);

    (async () => {
      try {
        const data = await listMarketplacePlugins(registry, query);
        if (cancelled) return;
        setIndex(data);
      } catch (err) {
        if (!cancelled) {
          const message = err instanceof Error ? err.message : 'Failed to load plugins.';
          setError(message);
          setIndex(null);
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [active, registry, query]);

  return { index, loading, error };
};

export const useMarketplaceDetails = (
  active: boolean,
  registry: string,
  pluginId: string | null
) => {
  const [details, setDetails] = useState<MarketplacePluginDetails | null>(null);
  const [selectedVersion, setSelectedVersion] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setSelectedVersion(null);
  }, [pluginId, registry]);

  useEffect(() => {
    if (!active) return;
    if (!registry || !pluginId) {
      setDetails(null);
      return;
    }

    let cancelled = false;
    setLoading(true);
    setError(null);

    (async () => {
      try {
        const pluginDetails = await getMarketplacePlugin(
          registry,
          pluginId,
          selectedVersion ?? undefined
        );
        if (cancelled) return;
        setDetails(pluginDetails);
        setSelectedVersion((prev) => prev ?? pluginDetails.version.version);
      } catch (err) {
        if (!cancelled) {
          const message = err instanceof Error ? err.message : 'Failed to load plugin details.';
          setError(message);
          setDetails(null);
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [active, registry, pluginId, selectedVersion]);

  return { details, selectedVersion, setSelectedVersion, loading, error };
};

type JobCallbacks = {
  onSuccess: (job: JobInfo) => void;
  onFailure: (job: JobInfo) => void;
  onCancelled: (job: JobInfo) => void;
};

export const useMarketplaceJob = (jobId: string | null, callbacks: JobCallbacks) => {
  const [jobInfo, setJobInfo] = useState<JobInfo | null>(null);
  const [jobError, setJobError] = useState<string | null>(null);

  useEffect(() => {
    if (!jobId) {
      setJobInfo(null);
      setJobError(null);
      return;
    }

    let cancelled = false;
    let timeoutId: number | null = null;

    const poll = async () => {
      try {
        const info = await getMarketplaceJob(jobId);
        if (cancelled) return;
        setJobInfo(info);
        setJobError(null);

        if (info.status === 'succeeded') {
          callbacks.onSuccess(info);
          return;
        }
        if (info.status === 'failed') {
          callbacks.onFailure(info);
          return;
        }
        if (info.status === 'cancelled') {
          callbacks.onCancelled(info);
          return;
        }

        timeoutId = window.setTimeout(poll, 2000);
      } catch (err) {
        if (!cancelled) {
          const message = err instanceof Error ? err.message : 'Failed to fetch job status.';
          setJobError(message);
        }
      }
    };

    poll();

    return () => {
      cancelled = true;
      if (timeoutId) window.clearTimeout(timeoutId);
    };
  }, [jobId, callbacks]);

  const cancelJob = useCallback(async () => {
    if (!jobId) return;
    try {
      const info = await cancelMarketplaceJob(jobId);
      setJobInfo(info);
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Failed to cancel job.';
      setJobError(message);
    }
  }, [jobId]);

  const resetJob = useCallback(() => {
    setJobInfo(null);
    setJobError(null);
  }, []);

  return { jobInfo, jobError, cancelJob, resetJob };
};
