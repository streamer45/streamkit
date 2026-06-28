// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

export type MarketplaceRegistry = {
  id: string;
  url: string;
};

export type MarketplaceIndex = {
  schema_version: number;
  plugins: MarketplacePlugin[];
};

export type MarketplacePlugin = {
  id: string;
  name?: string | null;
  description?: string | null;
  latest?: string | null;
  versions: MarketplacePluginVersion[];
};

export type MarketplacePluginVersion = {
  version: string;
  manifest_url: string;
  signature_url?: string | null;
  published_at?: string | null;
};

export type MarketplacePluginKind = 'wasm' | 'native';

export type PluginBundle = {
  url: string;
  sha256: string;
  size_bytes?: number | null;
};

export type PluginBundleVariant = {
  accelerator: string;
  url: string;
  sha256: string;
  size_bytes?: number | null;
};

export type PluginCompatibility = {
  streamkit?: string | null;
  os: string[];
  arch: string[];
};

export type ModelSource =
  | {
      source: 'huggingface';
      repo_id: string;
      revision?: string | null;
      files: string[];
    }
  | {
      source: 'url';
      url: string;
    };

export type ModelSpec = ModelSource & {
  id?: string | null;
  name?: string | null;
  default?: boolean;
  expected_size_bytes?: number | null;
  sha256?: string | null;
  file_checksums?: Record<string, string>;
  license?: string | null;
  license_url?: string | null;
  gated?: boolean;
};

export type PluginManifest = {
  schema_version: number;
  id: string;
  name?: string | null;
  version: string;
  node_kind: string;
  kind: MarketplacePluginKind;
  description?: string | null;
  license?: string | null;
  license_url?: string | null;
  homepage?: string | null;
  repository?: string | null;
  entrypoint: string;
  bundle: PluginBundle | null;
  variants?: PluginBundleVariant[];
  compatibility?: PluginCompatibility | null;
  models: ModelSpec[];
};

export type MarketplaceSignatureStatus = {
  verified: boolean;
  key_id?: string | null;
  error?: string | null;
};

export type MarketplacePluginDetails = {
  registry: string;
  plugin: MarketplacePlugin;
  version: MarketplacePluginVersion;
  manifest: PluginManifest;
  signature: MarketplaceSignatureStatus;
  allow_native_marketplace: boolean;
};

export type InstallPluginRequest = {
  registry: string;
  plugin_id: string;
  version?: string | null;
  install_models?: boolean;
  model_ids?: string[] | null;
  accelerator?: string | null;
};

export type InstallPluginResponse = {
  job_id: string;
};

export type JobStatus = 'queued' | 'running' | 'succeeded' | 'failed' | 'cancelled';

export type StepStatus = 'pending' | 'running' | 'succeeded' | 'failed';

export type JobProgress = {
  bytes_done?: number;
  bytes_total?: number;
  items_done?: number;
  items_total?: number;
  current_item?: string;
  rate_bytes_per_sec?: number;
};

export type JobStep = {
  name: string;
  status: StepStatus;
  progress?: JobProgress;
  error?: string;
};

export type JobInfo = {
  status: JobStatus;
  started_at_ms?: number | null;
  updated_at_ms: number;
  summary: string;
  steps: JobStep[];
};
