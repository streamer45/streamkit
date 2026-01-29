// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import React, { useCallback, useEffect, useMemo, useState } from 'react';

import { useToast } from '@/context/ToastContext';
import { usePermissions } from '@/hooks/usePermissions';
import { installMarketplacePlugin } from '@/services/marketplace';
import { ensurePluginsLoaded, reloadPlugins, usePluginStore } from '@/stores/pluginStore';
import type { JobInfo, MarketplaceIndex, MarketplacePluginDetails } from '@/types/marketplace';
import type { PluginSummary } from '@/types/types';
import { getLogger } from '@/utils/logger';

import {
  ErrorBox,
  Input,
  Label,
  MarketplaceGrid,
  NoticeBox,
  Row,
  Section,
  SectionTitle,
  Select,
} from '../PluginsView.styles';
import { computeJobProgress } from './marketplaceFormatters';
import {
  useMarketplaceDetails,
  useMarketplaceIndex,
  useMarketplaceJob,
  useMarketplaceRegistries,
} from './marketplaceHooks';
import { MarketplaceJobPanel } from './MarketplaceJobPanel';
import { MarketplaceDetailsPanel, MarketplaceListPanel } from './MarketplacePanels';

const logger = getLogger('MarketplaceTab');

type MarketplaceTabProps = {
  active: boolean;
};

const syncSelectedPluginId = (
  index: MarketplaceIndex | null,
  selectedPluginId: string | null,
  setSelectedPluginId: React.Dispatch<React.SetStateAction<string | null>>
) => {
  if (!index) {
    setSelectedPluginId(null);
    return;
  }
  if (selectedPluginId && index.plugins.some((plugin) => plugin.id === selectedPluginId)) {
    return;
  }
  setSelectedPluginId(index.plugins[0]?.id ?? null);
};

const syncInstallModels = (
  details: MarketplacePluginDetails | null,
  setInstallModels: React.Dispatch<React.SetStateAction<boolean>>
) => {
  if (!details) {
    setInstallModels(false);
    return;
  }
  setInstallModels(details.manifest.models.length > 0);
};

const defaultModelSelection = (details: MarketplacePluginDetails | null) => {
  const models = details?.manifest.models ?? [];
  if (models.length === 0) return [];
  if (!models.every((model) => model.id)) {
    return [];
  }
  const defaultIds = models.filter((model) => model.default).map((model) => model.id as string);
  if (defaultIds.length > 0) {
    return defaultIds;
  }
  return models.map((model) => model.id as string);
};

const deriveModelFlags = (details: MarketplacePluginDetails | null, selectedModelIds: string[]) => {
  const models = details?.manifest.models ?? [];
  const hasModels = models.length > 0;
  const hasModelSelection = hasModels && models.every((model) => model.id);
  const selectedModels =
    hasModelSelection && selectedModelIds.length > 0
      ? models.filter((model) => selectedModelIds.includes(model.id as string))
      : models;
  const hasGatedModels = selectedModels.some((model) => model.gated);
  const hasModelLicenses = selectedModels.some((model) => model.license || model.license_url);
  const requiresLicenseAcceptance = Boolean(
    details?.manifest.license || details?.manifest.license_url || hasModelLicenses
  );
  return { hasModels, hasModelSelection, hasGatedModels, requiresLicenseAcceptance };
};

const isJobActive = (jobInfo: JobInfo | null) =>
  jobInfo?.status === 'queued' || jobInfo?.status === 'running';

const isLicenseSatisfied = (requiresLicenseAcceptance: boolean, licenseAccepted: boolean) =>
  !requiresLicenseAcceptance || licenseAccepted;

const isModelSelectionSatisfied = (installModels: boolean, modelSelectionRequired: boolean) =>
  !installModels || !modelSelectionRequired;

const isNativeMarketplaceAllowed = (details: MarketplacePluginDetails) =>
  details.manifest.kind !== 'native' || details.allow_native_marketplace;

const isInstallAllowedForInstalled = ({
  isInstalled,
  installModels,
  hasModels,
  installedVersion,
  selectedVersion,
}: {
  isInstalled: boolean;
  installModels: boolean;
  hasModels: boolean;
  installedVersion: string | null;
  selectedVersion: string | null;
}) => {
  if (!isInstalled) return true;
  if (!installModels || !hasModels) return false;
  if (installedVersion && selectedVersion && installedVersion !== selectedVersion) return false;
  return true;
};

const computeCanInstall = ({
  details,
  canLoadPlugin,
  requiresLicenseAcceptance,
  licenseAccepted,
  jobInfo,
  installModels,
  modelSelectionRequired,
  hasModels,
  isInstalled,
  installedVersion,
  selectedVersion,
}: {
  details: MarketplacePluginDetails | null;
  canLoadPlugin: boolean;
  requiresLicenseAcceptance: boolean;
  licenseAccepted: boolean;
  jobInfo: JobInfo | null;
  installModels: boolean;
  modelSelectionRequired: boolean;
  hasModels: boolean;
  isInstalled: boolean;
  installedVersion: string | null;
  selectedVersion: string | null;
}) => {
  if (!details) return false;
  if (!canLoadPlugin) return false;
  if (details.signature.verified !== true) return false;
  if (!isLicenseSatisfied(requiresLicenseAcceptance, licenseAccepted)) return false;
  if (!isModelSelectionSatisfied(installModels, modelSelectionRequired)) return false;
  if (isJobActive(jobInfo)) return false;
  if (!isNativeMarketplaceAllowed(details)) return false;
  if (
    !isInstallAllowedForInstalled({
      isInstalled,
      installModels,
      hasModels,
      installedVersion,
      selectedVersion,
    })
  ) {
    return false;
  }
  return true;
};

const useInstalledPlugin = (
  details: MarketplacePluginDetails | null,
  active: boolean
): PluginSummary | null => {
  const installedPlugins = usePluginStore((state) => state.plugins);

  useEffect(() => {
    if (!active) return;
    ensurePluginsLoaded().catch((err) => {
      logger.error('Failed to load installed plugins', err);
    });
  }, [active]);

  return useMemo(() => {
    if (!details) return null;
    return (
      installedPlugins.find((plugin) => plugin.original_kind === details.manifest.node_kind) ?? null
    );
  }, [details, installedPlugins]);
};

const useModelFlags = (
  details: MarketplacePluginDetails | null,
  selectedModelIds: string[],
  installModels: boolean
) => {
  const flags = useMemo(
    () => deriveModelFlags(details, selectedModelIds),
    [details, selectedModelIds]
  );
  const missingModelSelection =
    installModels && flags.hasModelSelection && selectedModelIds.length === 0;
  return { ...flags, missingModelSelection };
};

const useDebouncedSearch = (initialValue: string, delayMs: number) => {
  const [searchInput, setSearchInput] = useState(initialValue);
  const [debouncedSearch, setDebouncedSearch] = useState(initialValue);

  useEffect(() => {
    const timeout = window.setTimeout(() => {
      setDebouncedSearch(searchInput.trim());
    }, delayMs);
    return () => window.clearTimeout(timeout);
  }, [searchInput, delayMs]);

  return { searchInput, setSearchInput, debouncedSearch };
};

const useJobStatus = (jobInfo: JobInfo | null) => {
  const jobProgress = useMemo(() => computeJobProgress(jobInfo), [jobInfo]);
  const jobIsActive = jobInfo?.status === 'queued' || jobInfo?.status === 'running';
  return { jobProgress, jobIsActive };
};

const startInstall = async ({
  details,
  installModels,
  selectedModelIds,
  resetJob,
  setInstalling,
  setJobId,
  toast,
}: {
  details: MarketplacePluginDetails | null;
  installModels: boolean;
  selectedModelIds: string[];
  resetJob: () => void;
  setInstalling: React.Dispatch<React.SetStateAction<boolean>>;
  setJobId: React.Dispatch<React.SetStateAction<string | null>>;
  toast: ReturnType<typeof useToast>;
}) => {
  if (!details) return;
  const hasModelSelection = details.manifest.models.every((model) => model.id);
  const modelIds =
    installModels && hasModelSelection && selectedModelIds.length > 0
      ? selectedModelIds
      : undefined;
  setInstalling(true);
  try {
    const response = await installMarketplacePlugin({
      registry: details.registry,
      plugin_id: details.manifest.id,
      version: details.version.version,
      install_models: installModels,
      model_ids: modelIds,
    });
    setJobId(response.job_id);
    resetJob();
  } catch (err) {
    const message = err instanceof Error ? err.message : 'Failed to start install.';
    toast.error(message);
  } finally {
    setInstalling(false);
  }
};

const MarketplaceTab: React.FC<MarketplaceTabProps> = ({ active }) => {
  const { can } = usePermissions();
  const toast = useToast();

  const {
    registries,
    selectedRegistry,
    setSelectedRegistry,
    loading: registriesLoading,
    loaded: registriesLoaded,
    error: registriesError,
  } = useMarketplaceRegistries(active);

  const { searchInput, setSearchInput, debouncedSearch } = useDebouncedSearch('', 300);
  const [selectedPluginId, setSelectedPluginId] = useState<string | null>(null);
  const [licenseAccepted, setLicenseAccepted] = useState(false);
  const [installModels, setInstallModels] = useState(false);
  const [selectedModelIds, setSelectedModelIds] = useState<string[]>([]);
  const [jobId, setJobId] = useState<string | null>(null);
  const [installing, setInstalling] = useState(false);

  const {
    index,
    loading: indexLoading,
    error: indexError,
  } = useMarketplaceIndex(active, selectedRegistry, debouncedSearch);

  const {
    details,
    selectedVersion,
    setSelectedVersion,
    loading: detailsLoading,
    error: detailsError,
  } = useMarketplaceDetails(active, selectedRegistry, selectedPluginId);

  useEffect(() => {
    syncSelectedPluginId(index, selectedPluginId, setSelectedPluginId);
  }, [index, selectedPluginId]);

  useEffect(() => {
    setLicenseAccepted(false);
  }, [selectedPluginId, selectedVersion]);

  useEffect(() => {
    syncInstallModels(details, setInstallModels);
    setSelectedModelIds(defaultModelSelection(details));
  }, [details, selectedVersion]);

  const jobCallbacks = useMemo(
    () => ({
      onSuccess: (info: JobInfo) => {
        toast.success('Plugin installed successfully.');
        reloadPlugins().catch((err) => logger.error('Failed to refresh plugins', err));
        logger.info(info.summary);
      },
      onFailure: (info: JobInfo) => {
        toast.error(info.summary || 'Plugin install failed.');
      },
      onCancelled: () => {
        toast.info('Plugin install cancelled.');
      },
    }),
    [toast]
  );

  const { jobInfo, jobError, cancelJob, resetJob } = useMarketplaceJob(jobId, jobCallbacks);

  const handleInstall = useCallback(() => {
    void startInstall({
      details,
      installModels,
      selectedModelIds,
      resetJob,
      setInstalling,
      setJobId,
      toast,
    });
  }, [details, installModels, selectedModelIds, resetJob, setInstalling, setJobId, toast]);

  const handleClearJob = useCallback(() => {
    setJobId(null);
    resetJob();
  }, [resetJob]);

  const handleSelectRegistry = useCallback(
    (value: string) => {
      setSelectedRegistry(value);
      setSelectedPluginId(null);
    },
    [setSelectedRegistry]
  );

  const handleSelectPlugin = useCallback((pluginId: string) => {
    setSelectedPluginId(pluginId);
  }, []);
  const handleToggleModel = useCallback((modelId: string, checked: boolean) => {
    setSelectedModelIds((current) => {
      if (checked) {
        return current.includes(modelId) ? current : [...current, modelId];
      }
      return current.filter((id) => id !== modelId);
    });
  }, []);

  const installedPlugin = useInstalledPlugin(details, active);
  const {
    hasModels,
    hasModelSelection,
    hasGatedModels,
    requiresLicenseAcceptance,
    missingModelSelection,
  } = useModelFlags(details, selectedModelIds, installModels);
  const installedVersion = installedPlugin?.version ?? null;

  const canInstall = useMemo(
    () =>
      computeCanInstall({
        details,
        canLoadPlugin: can.loadPlugin,
        requiresLicenseAcceptance,
        licenseAccepted,
        jobInfo,
        installModels,
        modelSelectionRequired: missingModelSelection,
        hasModels,
        isInstalled: Boolean(installedPlugin),
        installedVersion,
        selectedVersion,
      }),
    [
      details,
      can.loadPlugin,
      requiresLicenseAcceptance,
      licenseAccepted,
      jobInfo,
      installModels,
      missingModelSelection,
      hasModels,
      installedPlugin,
      installedVersion,
      selectedVersion,
    ]
  );

  const { jobProgress, jobIsActive } = useJobStatus(jobInfo);
  const loadingMarketplace = registriesLoading || indexLoading;
  const marketplaceError = registriesError ?? indexError ?? detailsError;

  return (
    <>
      {marketplaceError && <ErrorBox>{marketplaceError}</ErrorBox>}
      {!marketplaceError && registriesLoaded && registries.length === 0 && (
        <NoticeBox>No registries configured for this server.</NoticeBox>
      )}
      <Section>
        <SectionTitle>Registry</SectionTitle>
        <Row>
          <Label>
            Registry
            <Select
              value={selectedRegistry}
              onChange={(event) => handleSelectRegistry(event.target.value)}
              disabled={registries.length === 0}
            >
              {registries.length === 0 && <option value="">No registries</option>}
              {registries.map((registry) => (
                <option key={registry.id} value={registry.id}>
                  {registry.url}
                </option>
              ))}
            </Select>
          </Label>
          <Label>
            Search
            <Input
              value={searchInput}
              onChange={(event) => setSearchInput(event.target.value)}
              placeholder="Search plugins"
            />
          </Label>
        </Row>
      </Section>

      <MarketplaceGrid>
        <MarketplaceListPanel
          loading={loadingMarketplace}
          index={index}
          selectedPluginId={selectedPluginId}
          onSelect={handleSelectPlugin}
        />
        <MarketplaceDetailsPanel
          details={details}
          selectedVersion={selectedVersion}
          onVersionChange={setSelectedVersion}
          loading={detailsLoading}
          licenseAccepted={licenseAccepted}
          onLicenseAccepted={setLicenseAccepted}
          requiresLicenseAcceptance={requiresLicenseAcceptance}
          installModels={installModels}
          onInstallModelsChange={setInstallModels}
          hasModels={hasModels}
          hasModelSelection={hasModelSelection}
          hasGatedModels={hasGatedModels}
          missingModelSelection={missingModelSelection}
          installedPlugin={installedPlugin}
          selectedModelIds={selectedModelIds}
          onModelToggle={handleToggleModel}
          canLoadPlugin={can.loadPlugin}
          canInstall={canInstall}
          installing={installing}
          onInstall={handleInstall}
        />
      </MarketplaceGrid>

      <MarketplaceJobPanel
        jobId={jobId}
        jobInfo={jobInfo}
        jobError={jobError}
        jobProgress={jobProgress}
        jobIsActive={jobIsActive}
        onCancel={cancelJob}
        onClear={handleClearJob}
      />
    </>
  );
};

export default MarketplaceTab;
