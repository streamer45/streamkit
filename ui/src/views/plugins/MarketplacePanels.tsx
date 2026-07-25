// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import React from 'react';

import { Button } from '@/components/ui/Button';
import { CheckboxWithLabel } from '@/components/ui/Checkbox';
import type { MarketplaceIndex, MarketplacePluginDetails } from '@/types/marketplace';
import type { PluginSummary } from '@/types/types';

import {
  DetailsDescription,
  DetailsHeader,
  DetailsLoadingOverlay,
  DetailsSection,
  DetailsTitle,
  EmptyState,
  KeyLabel,
  KeyValue,
  KeyValueGrid,
  LicenseLink,
  MarketplaceList,
  MarketplaceListDescription,
  MarketplaceListItem,
  MarketplaceListTitle,
  NoticeBox,
  PluginBadge,
  Row,
  Section,
  SectionDivider,
  SectionTitle,
  Select,
  SignatureValue,
  SubSectionLabel,
  Subtle,
  WarningBox,
} from '../PluginsView.styles';
import { formatBytes } from './marketplaceFormatters';
import { MarketplaceModelsSection } from './MarketplaceModelsSection';

type MarketplaceListPanelProps = {
  loading: boolean;
  index: MarketplaceIndex | null;
  selectedPluginId: string | null;
  onSelect: (pluginId: string) => void;
};

export const MarketplaceListPanel: React.FC<MarketplaceListPanelProps> = ({
  loading,
  index,
  selectedPluginId,
  onSelect,
}) => {
  return (
    <Section>
      <SectionTitle>Marketplace</SectionTitle>
      {loading && <Subtle>Loading marketplace...</Subtle>}
      {!loading && index?.plugins.length === 0 && <EmptyState>No plugins found.</EmptyState>}
      <MarketplaceList>
        {index?.plugins.map((plugin) => (
          <MarketplaceListItem
            key={plugin.id}
            $active={plugin.id === selectedPluginId}
            onClick={() => onSelect(plugin.id)}
          >
            <MarketplaceListTitle>{plugin.name ?? plugin.id}</MarketplaceListTitle>
            <MarketplaceListDescription>
              {plugin.description ?? 'No description provided.'}
            </MarketplaceListDescription>
          </MarketplaceListItem>
        ))}
      </MarketplaceList>
    </Section>
  );
};

const buildInstallBlockedReasons = ({
  canLoadPlugin,
  signatureVerified,
  requiresLicenseAcceptance,
  licenseAccepted,
  missingModelSelection,
  allowNativeMarketplace,
  installedPlugin,
  installModels,
  hasModels,
  versionMismatch,
  installedVersion,
  selectedVersion,
}: {
  canLoadPlugin: boolean;
  signatureVerified: boolean;
  requiresLicenseAcceptance: boolean;
  licenseAccepted: boolean;
  missingModelSelection: boolean;
  allowNativeMarketplace: boolean;
  installedPlugin: PluginSummary | null;
  installModels: boolean;
  hasModels: boolean;
  versionMismatch: boolean;
  installedVersion: string | null;
  selectedVersion: string | null;
}) => {
  const reasons: string[] = [];
  if (!canLoadPlugin) {
    reasons.push('Insufficient permissions to install plugins.');
  }
  if (!signatureVerified) {
    reasons.push('Plugin signature is not verified.');
  }
  if (requiresLicenseAcceptance && !licenseAccepted) {
    reasons.push('License acceptance required.');
  }
  if (missingModelSelection) {
    reasons.push('Select at least one model to download.');
  }
  if (!allowNativeMarketplace) {
    reasons.push('Native marketplace installs are disabled in server config.');
  }
  if (installedPlugin) {
    if (!installModels) {
      reasons.push('Plugin already installed. Select models to download or uninstall first.');
    } else if (!hasModels) {
      reasons.push('Plugin already installed and has no models to download.');
    } else if (versionMismatch && installedVersion) {
      reasons.push(
        `Installed version ${installedVersion} does not match selected version ${selectedVersion}. Select ${installedVersion} to download models.`
      );
    }
  }
  return reasons;
};

const getInstallLabel = ({
  installing,
  installedPlugin,
  installModels,
  hasModels,
}: {
  installing: boolean;
  installedPlugin: PluginSummary | null;
  installModels: boolean;
  hasModels: boolean;
}) => {
  if (installing) return 'Starting...';
  if (!installedPlugin) return 'Install';
  if (installModels && hasModels) return 'Download models';
  return 'Installed';
};

const MarketplaceInstalledNotice: React.FC<{
  installedPlugin: PluginSummary | null;
  installedVersion: string | null;
  versionMismatch: boolean;
}> = ({ installedPlugin, installedVersion, versionMismatch }) => {
  if (!installedPlugin) return null;
  return (
    <NoticeBox>
      Installed{installedVersion ? ` (version ${installedVersion})` : ''}.
      {versionMismatch && installedVersion && (
        <> Select version {installedVersion} to download models.</>
      )}
    </NoticeBox>
  );
};

const MarketplaceInstallWarnings: React.FC<{
  canInstall: boolean;
  installBlockedReasons: string[];
}> = ({ canInstall, installBlockedReasons }) => {
  if (canInstall || installBlockedReasons.length === 0) return null;
  return (
    <WarningBox>
      <ul style={{ margin: 0, paddingLeft: 16 }}>
        {installBlockedReasons.map((reason) => (
          <li key={reason}>{reason}</li>
        ))}
      </ul>
    </WarningBox>
  );
};

type MarketplaceDetailsPanelProps = {
  details: MarketplacePluginDetails | null;
  selectedVersion: string | null;
  selectedAccelerator: string;
  onAcceleratorChange: (value: string) => void;
  loading: boolean;
  licenseAccepted: boolean;
  requiresLicenseAcceptance: boolean;
  installModels: boolean;
  hasModels: boolean;
  hasModelSelection: boolean;
  hasGatedModels: boolean;
  missingModelSelection: boolean;
  installedPlugin: PluginSummary | null;
  selectedModelIds: string[];
  onModelToggle: (modelId: string, checked: boolean) => void;
  canLoadPlugin: boolean;
  canInstall: boolean;
  installing: boolean;
  onVersionChange: (value: string) => void;
  onLicenseAccepted: (value: boolean) => void;
  onInstall: () => void;
};

export const MarketplaceDetailsPanel: React.FC<MarketplaceDetailsPanelProps> = ({
  details,
  selectedVersion,
  selectedAccelerator,
  onAcceleratorChange,
  loading,
  licenseAccepted,
  requiresLicenseAcceptance,
  installModels,
  hasModels,
  hasModelSelection,
  hasGatedModels,
  missingModelSelection,
  installedPlugin,
  selectedModelIds,
  onModelToggle,
  canLoadPlugin,
  canInstall,
  installing,
  onVersionChange,
  onLicenseAccepted,
  onInstall,
}) => {
  if (!details && loading) return <MarketplaceDetailsLoading />;
  if (!details) return <MarketplaceDetailsEmpty />;

  const installedVersion = installedPlugin?.version ?? null;
  const versionMismatch =
    Boolean(installedVersion) && Boolean(selectedVersion) && installedVersion !== selectedVersion;
  const installBlockedReasons = buildInstallBlockedReasons({
    canLoadPlugin,
    signatureVerified: details.signature.verified === true,
    requiresLicenseAcceptance,
    licenseAccepted,
    missingModelSelection,
    allowNativeMarketplace: details.manifest.kind !== 'native' || details.allow_native_marketplace,
    installedPlugin,
    installModels,
    hasModels,
    versionMismatch,
    installedVersion,
    selectedVersion,
  });
  const installLabel = getInstallLabel({ installing, installedPlugin, installModels, hasModels });

  return (
    <DetailsSection>
      {loading && <DetailsLoadingOverlay>Loading plugin details...</DetailsLoadingOverlay>}
      <SectionTitle>Details</SectionTitle>
      <MarketplaceDetailsHeader details={details} />
      <MarketplaceInstalledNotice
        installedPlugin={installedPlugin}
        installedVersion={installedVersion}
        versionMismatch={versionMismatch}
      />
      <MarketplaceDetailsFields
        details={details}
        selectedVersion={selectedVersion}
        onVersionChange={onVersionChange}
        selectedAccelerator={selectedAccelerator}
        onAcceleratorChange={onAcceleratorChange}
      />
      <MarketplaceNativeNotice
        kind={details.manifest.kind}
        allowNativeMarketplace={details.allow_native_marketplace}
      />
      <MarketplaceLicenseAcceptance
        enabled={requiresLicenseAcceptance}
        checked={licenseAccepted}
        onChange={onLicenseAccepted}
      />
      <MarketplaceModelsSection
        hasModels={hasModels}
        hasModelSelection={hasModelSelection}
        hasGatedModels={hasGatedModels}
        models={details.manifest.models}
        selectedModelIds={selectedModelIds}
        onModelToggle={onModelToggle}
      />
      <MarketplaceInstallWarnings
        canInstall={canInstall}
        installBlockedReasons={installBlockedReasons}
      />

      <Row>
        <Button variant="primary" onClick={onInstall} disabled={!canInstall || installing}>
          {installLabel}
        </Button>
      </Row>
    </DetailsSection>
  );
};

const MarketplaceDetailsLoading: React.FC = () => (
  <DetailsSection>
    <SectionTitle>Details</SectionTitle>
    <Subtle>Loading plugin details...</Subtle>
  </DetailsSection>
);

const MarketplaceDetailsEmpty: React.FC = () => (
  <DetailsSection>
    <SectionTitle>Details</SectionTitle>
    <EmptyState>Select a plugin to view.</EmptyState>
  </DetailsSection>
);

const MarketplaceDetailsHeader: React.FC<{ details: MarketplacePluginDetails }> = ({ details }) => (
  <DetailsHeader>
    <DetailsTitle>{details.manifest.name ?? details.manifest.id}</DetailsTitle>
    {details.manifest.description && (
      <DetailsDescription>{details.manifest.description}</DetailsDescription>
    )}
    <Row>
      <PluginBadge $variant={details.manifest.kind}>{details.manifest.kind}</PluginBadge>
    </Row>
  </DetailsHeader>
);

type MarketplaceDetailsFieldsProps = {
  details: MarketplacePluginDetails;
  selectedVersion: string | null;
  onVersionChange: (value: string) => void;
  selectedAccelerator: string;
  onAcceleratorChange: (value: string) => void;
};

// The canonical bundle is always the CPU build; `variants` carries additional
// accelerator-specific builds (e.g. CUDA).
export const manifestAccelerators = (manifest: MarketplacePluginDetails['manifest']): string[] => {
  const variants = manifest.variants ?? [];
  const accelerators = ['cpu', ...variants.map((variant) => variant.accelerator.toLowerCase())];
  return [...new Set(accelerators)];
};

const MarketplaceDetailsFields: React.FC<MarketplaceDetailsFieldsProps> = ({
  details,
  selectedVersion,
  onVersionChange,
  selectedAccelerator,
  onAcceleratorChange,
}) => {
  const signatureLabel = details.signature.verified
    ? `\u2713 Verified (${details.signature.key_id ?? 'trusted key'})`
    : `\u26A0 Unverified (${details.signature.error ?? 'unknown'})`;
  const modelFileCount = details.manifest.models.reduce((count, model) => {
    if (model.source === 'huggingface') {
      return count + model.files.length;
    }
    return count + 1;
  }, 0);

  return (
    <KeyValueGrid>
      <KeyLabel>Version</KeyLabel>
      <KeyValue>
        <Select
          value={selectedVersion ?? details.version.version}
          onChange={(event) => onVersionChange(event.target.value)}
        >
          {details.plugin.versions.map((version) => (
            <option key={version.version} value={version.version}>
              {version.version}
            </option>
          ))}
        </Select>
      </KeyValue>
      <MarketplaceAcceleratorRow
        manifest={details.manifest}
        selectedAccelerator={selectedAccelerator}
        onAcceleratorChange={onAcceleratorChange}
      />
      <KeyLabel>Node kind</KeyLabel>
      <KeyValue>{details.manifest.node_kind}</KeyValue>
      <KeyLabel>Entry point</KeyLabel>
      <KeyValue>{details.manifest.entrypoint}</KeyValue>
      <KeyLabel>Bundle size</KeyLabel>
      <KeyValue>
        {formatBytes(details.manifest.bundle?.size_bytes ?? undefined) || 'Unknown'}
      </KeyValue>
      <KeyLabel>Signature</KeyLabel>
      <KeyValue>
        <SignatureValue $verified={details.signature.verified}>{signatureLabel}</SignatureValue>
      </KeyValue>
      <KeyLabel>License</KeyLabel>
      <KeyValue>
        {details.manifest.license_url ? (
          <LicenseLink
            href={details.manifest.license_url}
            target="_blank"
            rel="noopener noreferrer"
          >
            {details.manifest.license || details.manifest.license_url}
          </LicenseLink>
        ) : (
          details.manifest.license || 'Unknown'
        )}
      </KeyValue>
      {details.manifest.models.length > 0 && (
        <>
          <KeyLabel>Models</KeyLabel>
          <KeyValue>{modelFileCount} files</KeyValue>
        </>
      )}
      <MarketplaceCompatibilityRows compatibility={details.manifest.compatibility} />
    </KeyValueGrid>
  );
};

const MarketplaceAcceleratorRow: React.FC<{
  manifest: MarketplacePluginDetails['manifest'];
  selectedAccelerator: string;
  onAcceleratorChange: (value: string) => void;
}> = ({ manifest, selectedAccelerator, onAcceleratorChange }) => {
  const accelerators = manifestAccelerators(manifest);
  if (accelerators.length < 2) return null;
  return (
    <>
      <KeyLabel>Accelerator</KeyLabel>
      <KeyValue>
        <Select
          value={selectedAccelerator}
          onChange={(event) => onAcceleratorChange(event.target.value)}
        >
          <option value="">Auto-detect</option>
          {accelerators.map((accelerator) => (
            <option key={accelerator} value={accelerator}>
              {accelerator}
            </option>
          ))}
        </Select>
      </KeyValue>
    </>
  );
};

const MarketplaceCompatibilityRows: React.FC<{
  compatibility: MarketplacePluginDetails['manifest']['compatibility'];
}> = ({ compatibility }) => {
  if (!compatibility) return null;
  return (
    <>
      {compatibility.streamkit && (
        <>
          <KeyLabel>StreamKit</KeyLabel>
          <KeyValue>{compatibility.streamkit}</KeyValue>
        </>
      )}
      {compatibility.os?.length ? (
        <>
          <KeyLabel>OS</KeyLabel>
          <KeyValue>{compatibility.os.join(', ')}</KeyValue>
        </>
      ) : null}
      {compatibility.arch?.length ? (
        <>
          <KeyLabel>Arch</KeyLabel>
          <KeyValue>{compatibility.arch.join(', ')}</KeyValue>
        </>
      ) : null}
    </>
  );
};

const MarketplaceNativeNotice: React.FC<{
  kind: MarketplacePluginDetails['manifest']['kind'];
  allowNativeMarketplace: boolean;
}> = ({ kind, allowNativeMarketplace }) => {
  if (kind !== 'native') return null;
  if (!allowNativeMarketplace) {
    return (
      <WarningBox>
        Native marketplace installs are disabled in server config. Set{' '}
        <code>allow_native_marketplace = true</code> to enable.
      </WarningBox>
    );
  }
  return <NoticeBox>Native plugins run in-process with full server access.</NoticeBox>;
};

const MarketplaceLicenseAcceptance: React.FC<{
  enabled: boolean;
  checked: boolean;
  onChange: (value: boolean) => void;
}> = ({ enabled, checked, onChange }) => {
  if (!enabled) return null;
  return (
    <>
      <SectionDivider />
      <SubSectionLabel>License</SubSectionLabel>
      <CheckboxWithLabel
        id="plugin-license-accept"
        label="I understand the plugin and model license terms."
        checked={checked}
        onCheckedChange={(value) => onChange(Boolean(value))}
      />
    </>
  );
};
