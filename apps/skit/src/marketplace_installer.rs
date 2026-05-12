// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt::Write,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use futures::StreamExt;
use reqwest::Client;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

use crate::{
    config::{MarketplaceHostPolicy, MarketplaceSchemePolicy, PluginConfig},
    marketplace::{MinisignVerifier, PluginKind, RegistryClient, RegistryIndex},
    marketplace_security::{origin_key, validated_get_response, MarketplaceUrlPolicy, OriginKey},
    permissions::Permissions,
    plugin_paths,
    plugin_records::{
        active_dir as plugin_active_dir, record_path as plugin_record_path, ActivePluginRecord,
    },
    plugins::{PluginSummary, PluginType, SharedUnifiedPluginManager},
};

const STEP_DOWNLOAD_MANIFEST: &str = "download_manifest";
const STEP_VERIFY_SIGNATURE: &str = "verify_signature";
const STEP_DOWNLOAD_BUNDLE: &str = "download_bundle";
const STEP_EXTRACT_BUNDLE: &str = "extract_bundle";
const STEP_ACTIVATE: &str = "activate";
const STEP_LOAD_PLUGIN: &str = "load_plugin";
const STEP_DOWNLOAD_MODELS: &str = "download_models";

const REGISTRY_TIMEOUT_SECS: u64 = 20;
const REGISTRY_INDEX_TTL_SECS: u64 = 60;
const REGISTRY_MANIFEST_TTL_SECS: u64 = 60;
const MAX_JOB_HISTORY: usize = 200;
const DOWNLOAD_CONNECT_TIMEOUT_SECS: u64 = 30;
const DOWNLOAD_READ_TIMEOUT_SECS: u64 = 60;

#[derive(Debug, Clone, Deserialize)]
pub struct InstallPluginRequest {
    pub registry: String,
    pub plugin_id: String,
    pub version: Option<String>,
    #[serde(default)]
    pub install_models: bool,
    #[serde(default)]
    pub model_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct JobProgress {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_done: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items_done: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_item: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_bytes_per_sec: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobStep {
    pub name: String,
    pub status: StepStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<JobProgress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobInfo {
    pub status: JobStatus,
    pub started_at_ms: Option<u128>,
    pub updated_at_ms: u128,
    pub summary: String,
    pub steps: Vec<JobStep>,
}

#[derive(Clone)]
pub struct InstallJobQueue {
    state: Arc<Mutex<InstallQueueState>>,
    installer: Arc<PluginInstaller>,
}

impl InstallJobQueue {
    /// Creates a new install queue with registry and verification settings.
    ///
    /// # Errors
    ///
    /// Returns an error if the registry client or verifier cannot be initialized.
    pub fn new(
        config: &PluginConfig,
        plugin_manager: SharedUnifiedPluginManager,
        plugin_asset_registry: crate::plugin_assets::PluginAssetRegistry,
    ) -> Result<Self> {
        let registry_client = RegistryClient::new(
            Duration::from_secs(REGISTRY_TIMEOUT_SECS),
            Duration::from_secs(REGISTRY_INDEX_TTL_SECS),
            Duration::from_secs(REGISTRY_MANIFEST_TTL_SECS),
        )?;
        let verifier = MinisignVerifier::from_trusted_pubkeys(&config.trusted_pubkeys)?;
        let models_dir = config
            .models_dir
            .as_ref()
            .map(|dir| dir.trim())
            .filter(|dir| !dir.is_empty())
            .map_or_else(|| PathBuf::from("models"), PathBuf::from);
        let huggingface_token = config
            .huggingface_token
            .as_ref()
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty());
        if matches!(
            config.marketplace.security.marketplace_scheme_policy,
            MarketplaceSchemePolicy::AllowHttp
        ) || matches!(
            config.marketplace.security.marketplace_host_policy,
            MarketplaceHostPolicy::AllowPrivate
        ) {
            tracing::warn!(
                allow_http = matches!(
                    config.marketplace.security.marketplace_scheme_policy,
                    MarketplaceSchemePolicy::AllowHttp
                ),
                allow_private_hosts = matches!(
                    config.marketplace.security.marketplace_host_policy,
                    MarketplaceHostPolicy::AllowPrivate
                ),
                "Marketplace URL policy allows non-default schemes or private hosts; intended for development"
            );
        }
        let marketplace_policy = MarketplaceUrlPolicy::from_config(config);
        let installer = PluginInstaller::new(
            registry_client,
            verifier,
            plugin_manager,
            plugin_asset_registry,
            PluginInstallerSettings {
                plugin_dir: PathBuf::from(&config.directory),
                models_dir,
                huggingface_token,
                allow_native_marketplace: config.marketplace.allow_native_marketplace,
                allow_model_urls: config.marketplace.security.allow_model_urls,
                marketplace_policy,
                registries: config.registries.clone(),
            },
        )?;
        Ok(Self {
            state: Arc::new(Mutex::new(InstallQueueState::default())),
            installer: Arc::new(installer),
        })
    }

    pub fn registries(&self) -> Vec<String> {
        self.installer.registries.clone()
    }

    pub fn registry_client(&self) -> RegistryClient {
        self.installer.registry_client.clone()
    }

    pub fn verifier(&self) -> MinisignVerifier {
        self.installer.verifier.clone()
    }

    pub async fn enqueue(&self, request: InstallPluginRequest, permissions: Permissions) -> String {
        let job_id = Uuid::new_v4().to_string();
        let steps = install_steps();
        let info = JobInfo {
            status: JobStatus::Queued,
            started_at_ms: None,
            updated_at_ms: now_ms(),
            summary: "Queued".to_string(),
            steps,
        };
        let job = InstallJob { info, cancel: CancellationToken::new(), request, permissions };

        let mut start_worker = false;
        let mut state = self.state.lock().await;
        state.jobs.insert(job_id.clone(), job);
        state.queue.push_back(job_id.clone());
        state.job_order.push_back(job_id.clone());
        state.prune_jobs();

        if !state.worker_running {
            state.worker_running = true;
            start_worker = true;
        }
        drop(state);

        if start_worker {
            let queue = self.clone();
            tokio::spawn(async move {
                queue.run_worker().await;
            });
        }

        job_id
    }

    pub async fn get_job(&self, job_id: &str) -> Option<JobInfo> {
        let state = self.state.lock().await;
        state.jobs.get(job_id).map(|job| job.info.clone())
    }

    pub async fn cancel_job(&self, job_id: &str) -> Option<JobInfo> {
        let mut state = self.state.lock().await;
        let mut remove_from_queue = false;
        {
            let job = state.jobs.get_mut(job_id)?;
            match job.info.status {
                JobStatus::Queued => {
                    job.cancel.cancel();
                    job.info.status = JobStatus::Cancelled;
                    job.info.summary = "Cancelled".to_string();
                    job.info.updated_at_ms = now_ms();
                    remove_from_queue = true;
                },
                JobStatus::Running => {
                    job.cancel.cancel();
                    job.info.summary = "Cancelling".to_string();
                    job.info.updated_at_ms = now_ms();
                },
                JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled => {},
            }
        }
        if remove_from_queue {
            state.queue.retain(|id| id != job_id);
        }
        state.jobs.get(job_id).map(|job| job.info.clone())
    }

    pub fn is_registry_configured(&self, registry: &str) -> bool {
        self.installer.is_registry_configured(registry)
    }

    async fn run_worker(self) {
        loop {
            let Some(context) = self.next_job().await else {
                let mut state = self.state.lock().await;
                state.worker_running = false;
                drop(state);
                break;
            };

            if context.cancel.is_cancelled() {
                self.mark_cancelled(&context.job_id, "Cancelled").await;
                continue;
            }

            let tracker = JobTracker { job_id: context.job_id.clone(), queue: self.clone() };

            tracker.set_status(JobStatus::Running, "Starting install").await;
            let result = self
                .installer
                .install(
                    context.request,
                    context.permissions,
                    tracker.clone(),
                    context.cancel.clone(),
                )
                .await;

            match result {
                Ok(()) => {
                    tracker.set_status(JobStatus::Succeeded, "Install completed").await;
                },
                Err(InstallError::Cancelled) => {
                    tracker.mark_cancelled("Cancelled").await;
                },
                Err(InstallError::Other(err)) => {
                    tracing::error!(job_id = %context.job_id, error = %err, "Install job failed");
                    tracker.set_status(JobStatus::Failed, format!("Install failed: {err}")).await;
                },
            }
        }
    }

    async fn next_job(&self) -> Option<JobContext> {
        loop {
            let mut state = self.state.lock().await;
            let job_id = state.queue.pop_front()?;
            let job = state.jobs.get_mut(&job_id)?;
            if matches!(job.info.status, JobStatus::Cancelled) {
                continue;
            }
            job.info.status = JobStatus::Running;
            job.info.started_at_ms = Some(now_ms());
            job.info.updated_at_ms = now_ms();
            job.info.summary = "Running".to_string();
            let request = job.request.clone();
            let permissions = job.permissions.clone();
            let cancel = job.cancel.clone();
            drop(state);
            return Some(JobContext { job_id, request, permissions, cancel });
        }
    }

    async fn update_job<F>(&self, job_id: &str, mut update: F)
    where
        F: FnMut(&mut JobInfo),
    {
        let mut state = self.state.lock().await;
        if let Some(job) = state.jobs.get_mut(job_id) {
            update(&mut job.info);
            job.info.updated_at_ms = now_ms();
        }
    }

    async fn mark_cancelled(&self, job_id: &str, summary: &str) {
        self.update_job(job_id, |info| {
            info.status = JobStatus::Cancelled;
            info.summary = summary.to_string();
            for step in &mut info.steps {
                if matches!(step.status, StepStatus::Running) {
                    step.status = StepStatus::Failed;
                    step.error = Some("Cancelled".to_string());
                }
            }
        })
        .await;
    }
}

#[derive(Default)]
struct InstallQueueState {
    jobs: HashMap<String, InstallJob>,
    queue: VecDeque<String>,
    job_order: VecDeque<String>,
    worker_running: bool,
}

impl InstallQueueState {
    fn prune_jobs(&mut self) {
        if self.jobs.len() <= MAX_JOB_HISTORY {
            return;
        }

        // Enforce the cap by pruning terminal jobs first, then the oldest queued jobs.
        let mut pruned: HashSet<String> = HashSet::new();

        for job_id in &self.job_order {
            if self.jobs.len().saturating_sub(pruned.len()) <= MAX_JOB_HISTORY {
                break;
            }
            let should_prune = self.jobs.get(job_id).is_none_or(|job| {
                matches!(
                    job.info.status,
                    JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled
                )
            });
            if should_prune {
                pruned.insert(job_id.clone());
            }
        }

        for job_id in &self.job_order {
            if self.jobs.len().saturating_sub(pruned.len()) <= MAX_JOB_HISTORY {
                break;
            }
            let should_prune = self
                .jobs
                .get(job_id)
                .is_none_or(|job| matches!(job.info.status, JobStatus::Queued));
            if should_prune {
                pruned.insert(job_id.clone());
            }
        }

        if pruned.is_empty() {
            return;
        }

        for job_id in &pruned {
            self.jobs.remove(job_id);
        }
        self.queue.retain(|job_id| !pruned.contains(job_id));
        self.job_order.retain(|job_id| !pruned.contains(job_id));
    }
}

struct InstallJob {
    info: JobInfo,
    cancel: CancellationToken,
    request: InstallPluginRequest,
    permissions: Permissions,
}

#[derive(Clone)]
struct JobTracker {
    job_id: String,
    queue: InstallJobQueue,
}

impl JobTracker {
    async fn set_status<S: Into<String>>(&self, status: JobStatus, summary: S) {
        let summary = summary.into();
        self.queue
            .update_job(&self.job_id, |info| {
                info.status = status.clone();
                info.summary.clone_from(&summary);
            })
            .await;
    }

    async fn start_step(&self, step_name: &str) {
        self.queue
            .update_job(&self.job_id, |info| {
                if let Some(step) = info.steps.iter_mut().find(|step| step.name == step_name) {
                    step.status = StepStatus::Running;
                    step.error = None;
                    step.progress = None;
                }
            })
            .await;
    }

    async fn succeed_step(&self, step_name: &str) {
        self.queue
            .update_job(&self.job_id, |info| {
                if let Some(step) = info.steps.iter_mut().find(|step| step.name == step_name) {
                    step.status = StepStatus::Succeeded;
                }
            })
            .await;
    }

    async fn fail_step(&self, step_name: &str, error: String) {
        tracing::error!(job_id = %self.job_id, step = %step_name, error = %error, "Install step failed");
        self.queue
            .update_job(&self.job_id, |info| {
                if let Some(step) = info.steps.iter_mut().find(|step| step.name == step_name) {
                    step.status = StepStatus::Failed;
                    step.error = Some(error.clone());
                }
            })
            .await;
    }

    async fn update_progress(&self, step_name: &str, progress: JobProgress) {
        self.queue
            .update_job(&self.job_id, |info| {
                if let Some(step) = info.steps.iter_mut().find(|step| step.name == step_name) {
                    step.progress = Some(progress.clone());
                }
            })
            .await;
    }

    async fn mark_cancelled(&self, summary: &str) {
        self.queue.mark_cancelled(&self.job_id, summary).await;
    }
}

struct JobContext {
    job_id: String,
    request: InstallPluginRequest,
    permissions: Permissions,
    cancel: CancellationToken,
}

#[derive(Clone)]
struct PluginInstaller {
    registry_client: RegistryClient,
    download_client: Client,
    verifier: MinisignVerifier,
    plugin_manager: SharedUnifiedPluginManager,
    plugin_asset_registry: crate::plugin_assets::PluginAssetRegistry,
    plugin_dir: PathBuf,
    models_dir: PathBuf,
    huggingface_token: Option<String>,
    allow_native_marketplace: bool,
    allow_model_urls: bool,
    marketplace_policy: MarketplaceUrlPolicy,
    registries: Vec<String>,
}

struct PluginInstallerSettings {
    plugin_dir: PathBuf,
    models_dir: PathBuf,
    huggingface_token: Option<String>,
    allow_native_marketplace: bool,
    allow_model_urls: bool,
    marketplace_policy: MarketplaceUrlPolicy,
    registries: Vec<String>,
}

struct DownloadModelRequest<'a> {
    url: &'a str,
    target_path: &'a Path,
    display_name: &'a str,
    items_done: u64,
    items_total: u64,
    expected_size: Option<u64>,
    expected_sha256: Option<&'a str>,
    bearer_token: Option<&'a str>,
    registry_origin: Option<OriginKey>,
}

impl PluginInstaller {
    fn new(
        registry_client: RegistryClient,
        verifier: MinisignVerifier,
        plugin_manager: SharedUnifiedPluginManager,
        plugin_asset_registry: crate::plugin_assets::PluginAssetRegistry,
        settings: PluginInstallerSettings,
    ) -> Result<Self> {
        let download_client = Client::builder()
            .connect_timeout(Duration::from_secs(DOWNLOAD_CONNECT_TIMEOUT_SECS))
            .read_timeout(Duration::from_secs(DOWNLOAD_READ_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("Failed to build bundle HTTP client")?;
        Ok(Self {
            registry_client,
            download_client,
            verifier,
            plugin_manager,
            plugin_asset_registry,
            plugin_dir: settings.plugin_dir,
            models_dir: settings.models_dir,
            huggingface_token: settings.huggingface_token,
            allow_native_marketplace: settings.allow_native_marketplace,
            allow_model_urls: settings.allow_model_urls,
            marketplace_policy: settings.marketplace_policy,
            registries: settings.registries,
        })
    }

    fn is_registry_configured(&self, registry: &str) -> bool {
        self.registries.iter().any(|entry| entry == registry)
    }

    async fn install(
        &self,
        request: InstallPluginRequest,
        permissions: Permissions,
        tracker: JobTracker,
        cancel: CancellationToken,
    ) -> Result<(), InstallError> {
        let registry_url = self.resolve_registry(&request.registry)?;
        let plugin_id = request.plugin_id.trim().to_string();
        if plugin_id.is_empty() {
            return Err(anyhow!("Plugin id must not be empty").into());
        }

        tracker.start_step(STEP_DOWNLOAD_MANIFEST).await;
        if let Err(err) = plugin_paths::validate_path_component("plugin id", &plugin_id) {
            tracker.fail_step(STEP_DOWNLOAD_MANIFEST, err.to_string()).await;
            return Err(err.into());
        }

        let registry_url =
            match self.marketplace_policy.validate_url("registry index", &registry_url, None).await
            {
                Ok(url) => url,
                Err(err) => {
                    tracker.fail_step(STEP_DOWNLOAD_MANIFEST, err.to_string()).await;
                    return Err(err.into());
                },
            };
        let registry_origin = match origin_key(&registry_url) {
            Ok(origin) => origin,
            Err(err) => {
                tracker.fail_step(STEP_DOWNLOAD_MANIFEST, err.to_string()).await;
                return Err(err.into());
            },
        };

        let index = match self
            .registry_client
            .fetch_index_with_policy(&registry_url, &self.marketplace_policy, &registry_origin)
            .await
        {
            Ok(index) => index,
            Err(err) => {
                tracker.fail_step(STEP_DOWNLOAD_MANIFEST, err.to_string()).await;
                return Err(err.into());
            },
        };
        let version_entry =
            match select_registry_version(&index, &plugin_id, request.version.as_deref()) {
                Ok(entry) => entry,
                Err(err) => {
                    tracker.fail_step(STEP_DOWNLOAD_MANIFEST, err.to_string()).await;
                    return Err(err.into());
                },
            };
        tracker.succeed_step(STEP_DOWNLOAD_MANIFEST).await;

        tracker.start_step(STEP_VERIFY_SIGNATURE).await;
        let manifest_url = match self
            .marketplace_policy
            .validate_url("manifest url", &version_entry.manifest_url, Some(&registry_origin))
            .await
        {
            Ok(url) => url,
            Err(err) => {
                tracker.fail_step(STEP_VERIFY_SIGNATURE, err.to_string()).await;
                return Err(err.into());
            },
        };
        let signature_url_raw = version_entry
            .signature_url
            .clone()
            .unwrap_or_else(|| format!("{}.minisig", manifest_url.as_str()));
        let signature_url = match self
            .marketplace_policy
            .validate_url("signature url", &signature_url_raw, Some(&registry_origin))
            .await
        {
            Ok(url) => url,
            Err(err) => {
                tracker.fail_step(STEP_VERIFY_SIGNATURE, err.to_string()).await;
                return Err(err.into());
            },
        };
        let manifest_raw = match self
            .registry_client
            .fetch_manifest_raw_with_policy(
                &manifest_url,
                &self.marketplace_policy,
                &registry_origin,
            )
            .await
        {
            Ok(raw) => raw,
            Err(err) => {
                tracker.fail_step(STEP_VERIFY_SIGNATURE, err.to_string()).await;
                return Err(err.into());
            },
        };
        let signature_text = match self
            .registry_client
            .fetch_text_with_policy(
                "signature url",
                &signature_url,
                &self.marketplace_policy,
                &registry_origin,
            )
            .await
        {
            Ok(text) => text,
            Err(err) => {
                tracker.fail_step(STEP_VERIFY_SIGNATURE, err.to_string()).await;
                return Err(err.into());
            },
        };
        if let Err(err) = self.verifier.verify(manifest_raw.bytes.as_ref(), &signature_text) {
            tracker.fail_step(STEP_VERIFY_SIGNATURE, err.to_string()).await;
            return Err(err.into());
        }

        let manifest = manifest_raw.manifest;
        if manifest.id != plugin_id {
            let manifest_id = manifest.id.as_str();
            let requested_id = plugin_id.as_str();
            let error = anyhow!(
                "Manifest plugin id '{manifest_id}' does not match requested id '{requested_id}'"
            )
            .to_string();
            tracker.fail_step(STEP_VERIFY_SIGNATURE, error.clone()).await;
            return Err(anyhow!(error).into());
        }

        if manifest.version != version_entry.version {
            let manifest_version = manifest.version.as_str();
            let requested_version = version_entry.version.as_str();
            let error = anyhow!(
                "Manifest version '{manifest_version}' does not match requested version '{requested_version}'"
            )
            .to_string();
            tracker.fail_step(STEP_VERIFY_SIGNATURE, error.clone()).await;
            return Err(anyhow!(error).into());
        }

        if let Err(err) = plugin_paths::validate_path_component("plugin id", &manifest.id) {
            tracker.fail_step(STEP_VERIFY_SIGNATURE, err.to_string()).await;
            return Err(err.into());
        }

        if let Err(err) = plugin_paths::validate_path_component("plugin version", &manifest.version)
        {
            tracker.fail_step(STEP_VERIFY_SIGNATURE, err.to_string()).await;
            return Err(err.into());
        }

        if let Err(err) = validate_manifest_compatibility(&manifest) {
            tracker.fail_step(STEP_VERIFY_SIGNATURE, err.to_string()).await;
            return Err(err.into());
        }

        let namespaced_kind = namespaced_kind(&manifest);
        if !permissions.is_plugin_allowed(&namespaced_kind) {
            let error = format!("Plugin '{namespaced_kind}' is not allowed by policy");
            tracker.fail_step(STEP_VERIFY_SIGNATURE, error.clone()).await;
            return Err(anyhow!(error).into());
        }

        if matches!(manifest.kind, PluginKind::Native) && !self.allow_native_marketplace {
            let error = "Native marketplace installs are disabled".to_string();
            tracker.fail_step(STEP_VERIFY_SIGNATURE, error.clone()).await;
            return Err(anyhow!(error).into());
        }

        tracker.succeed_step(STEP_VERIFY_SIGNATURE).await;

        Self::ensure_not_cancelled(&cancel)?;

        self.handle_bundle_install(
            &request,
            &manifest,
            &tracker,
            &cancel,
            &registry_origin,
            &namespaced_kind,
        )
        .await?;

        Self::ensure_not_cancelled(&cancel)?;

        self.handle_model_downloads(&request, &manifest, &tracker, &cancel, &registry_origin)
            .await?;

        Ok(())
    }

    async fn handle_bundle_install(
        &self,
        request: &InstallPluginRequest,
        manifest: &crate::marketplace::PluginManifest,
        tracker: &JobTracker,
        cancel: &CancellationToken,
        registry_origin: &OriginKey,
        namespaced_kind: &str,
    ) -> Result<(), InstallError> {
        Self::ensure_not_cancelled(cancel)?;
        tracker.start_step(STEP_DOWNLOAD_BUNDLE).await;
        let base_real = match plugin_paths::ensure_base_dir(&self.plugin_dir).await {
            Ok(base_real) => base_real,
            Err(err) => {
                tracker.fail_step(STEP_DOWNLOAD_BUNDLE, err.to_string()).await;
                return Err(err.into());
            },
        };

        let bundle_dir = self.plugin_dir.join("bundles").join(&manifest.id).join(&manifest.version);
        if bundle_dir.exists() {
            if !request.install_models {
                let err = anyhow!("Bundle version '{}' is already installed", manifest.version);
                tracker.fail_step(STEP_DOWNLOAD_BUNDLE, err.to_string()).await;
                return Err(err.into());
            }
            if let Err(err) =
                plugin_paths::ensure_existing_dir_under(&base_real, &bundle_dir, "bundle").await
            {
                tracker.fail_step(STEP_DOWNLOAD_BUNDLE, err.to_string()).await;
                return Err(err.into());
            }
            tracker.succeed_step(STEP_DOWNLOAD_BUNDLE).await;
            for step in [STEP_EXTRACT_BUNDLE, STEP_ACTIVATE, STEP_LOAD_PLUGIN] {
                Self::mark_step_succeeded(tracker, step).await;
            }
            return Ok(());
        }

        let bundle_path = match self
            .download_bundle(manifest, tracker, cancel, registry_origin, &base_real)
            .await
        {
            Ok(path) => path,
            Err(InstallError::Cancelled) => return Err(InstallError::Cancelled),
            Err(InstallError::Other(err)) => {
                tracker.fail_step(STEP_DOWNLOAD_BUNDLE, err.to_string()).await;
                return Err(InstallError::Other(err));
            },
        };
        tracker.succeed_step(STEP_DOWNLOAD_BUNDLE).await;

        Self::ensure_not_cancelled(cancel)?;

        tracker.start_step(STEP_EXTRACT_BUNDLE).await;
        let bundle_dir = match self.extract_bundle(manifest, &bundle_path, &base_real, cancel).await
        {
            Ok(dir) => dir,
            Err(InstallError::Cancelled) => return Err(InstallError::Cancelled),
            Err(InstallError::Other(err)) => {
                tracker.fail_step(STEP_EXTRACT_BUNDLE, err.to_string()).await;
                return Err(InstallError::Other(err));
            },
        };
        tracker.succeed_step(STEP_EXTRACT_BUNDLE).await;

        // Write a plugin.yml into the bundle directory so that
        // `read_local_plugin_manifest` can rediscover asset types on server
        // restart.  The marketplace manifest is JSON; the local loader expects
        // YAML, so we serialize the manifest here.
        if let Err(err) = write_manifest_yml(manifest, &bundle_dir).await {
            tracing::warn!(
                plugin_id = %manifest.id,
                error = %err,
                "Failed to write plugin.yml into bundle directory; \
                 asset types may not survive restart"
            );
        }

        Self::ensure_not_cancelled(cancel)?;

        tracker.start_step(STEP_ACTIVATE).await;
        let entrypoint_path = match self.activate_bundle(manifest, &bundle_dir, &base_real).await {
            Ok(path) => path,
            Err(InstallError::Cancelled) => return Err(InstallError::Cancelled),
            Err(InstallError::Other(err)) => {
                tracker.fail_step(STEP_ACTIVATE, err.to_string()).await;
                return Err(InstallError::Other(err));
            },
        };
        tracker.succeed_step(STEP_ACTIVATE).await;

        Self::ensure_not_cancelled(cancel)?;

        tracker.start_step(STEP_LOAD_PLUGIN).await;
        match self.load_plugin(manifest, &entrypoint_path, namespaced_kind).await {
            Ok(_) => {
                tracker.succeed_step(STEP_LOAD_PLUGIN).await;
            },
            Err(InstallError::Cancelled) => return Err(InstallError::Cancelled),
            Err(InstallError::Other(err)) => {
                tracker.fail_step(STEP_LOAD_PLUGIN, err.to_string()).await;
                return Err(InstallError::Other(err));
            },
        }

        Ok(())
    }

    async fn handle_model_downloads(
        &self,
        request: &InstallPluginRequest,
        manifest: &crate::marketplace::PluginManifest,
        tracker: &JobTracker,
        cancel: &CancellationToken,
        registry_origin: &OriginKey,
    ) -> Result<(), InstallError> {
        Self::ensure_not_cancelled(cancel)?;
        tracker.start_step(STEP_DOWNLOAD_MODELS).await;

        if request.model_ids.is_some() && !request.install_models {
            let err = anyhow!("Model selection requires install_models=true");
            tracker.fail_step(STEP_DOWNLOAD_MODELS, err.to_string()).await;
            return Err(InstallError::Other(err));
        }
        if request.install_models && !manifest.models.is_empty() {
            match self
                .download_models(
                    manifest,
                    request.model_ids.as_deref(),
                    tracker,
                    cancel,
                    Some(registry_origin),
                )
                .await
            {
                Ok(()) => {
                    tracker.succeed_step(STEP_DOWNLOAD_MODELS).await;
                },
                Err(InstallError::Cancelled) => return Err(InstallError::Cancelled),
                Err(InstallError::Other(err)) => {
                    tracker.fail_step(STEP_DOWNLOAD_MODELS, err.to_string()).await;
                    return Err(InstallError::Other(err));
                },
            }
        } else {
            tracker.succeed_step(STEP_DOWNLOAD_MODELS).await;
        }

        Ok(())
    }

    fn ensure_not_cancelled(cancel: &CancellationToken) -> Result<(), InstallError> {
        if cancel.is_cancelled() {
            Err(InstallError::Cancelled)
        } else {
            Ok(())
        }
    }

    async fn mark_step_succeeded(tracker: &JobTracker, step_name: &str) {
        tracker.start_step(step_name).await;
        tracker.succeed_step(step_name).await;
    }

    fn resolve_registry(&self, registry: &str) -> Result<String> {
        if self.registries.is_empty() {
            return Err(anyhow!("No registries are configured"));
        }
        if self.registries.iter().any(|entry| entry == registry) {
            return Ok(registry.to_string());
        }
        Err(anyhow!("Registry '{registry}' is not configured"))
    }

    async fn download_bundle(
        &self,
        manifest: &crate::marketplace::PluginManifest,
        tracker: &JobTracker,
        cancel: &CancellationToken,
        registry_origin: &OriginKey,
        base_real: &Path,
    ) -> Result<PathBuf, InstallError> {
        let bundle = manifest.bundle.as_ref().ok_or_else(|| {
            InstallError::Other(anyhow!("Plugin manifest missing required `bundle` section"))
        })?;
        let bundle_url = self
            .marketplace_policy
            .validate_url("bundle url", &bundle.url, Some(registry_origin))
            .await?;
        let cache_dir = self.plugin_dir.join("cache").join(&manifest.id).join(&manifest.version);
        plugin_paths::ensure_dir_under(base_real, &cache_dir, "cache").await?;

        let file_name = bundle_url
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .filter(|name| !name.is_empty())
            .unwrap_or("bundle.tar.zst");
        plugin_paths::validate_path_component("bundle file name", file_name)?;

        let bundle_path = cache_dir.join(file_name);
        let temp_path = cache_dir.join(format!(".download-{file_name}"));

        let mut hash_mismatch = false;
        let download_result: Result<(), InstallError> = async {
            // Check for existing partial download to enable resume.
            let existing_len = tokio::fs::metadata(&temp_path).await.ok().map(|m| m.len());
            let resume_from = existing_len.filter(|&len| len > 0);

            let (response, final_url) = validated_get_response(
                &self.download_client,
                &self.marketplace_policy,
                "bundle url",
                &bundle_url,
                Some(registry_origin),
                None,
                resume_from,
            )
            .await?;
            let response = response
                .error_for_status()
                .with_context(|| format!("Bundle download failed for {final_url}"))?;

            let is_partial = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
            let total_bytes = if is_partial {
                // For 206 responses, content_length is the remaining bytes.
                response.content_length().map(|cl| cl + resume_from.unwrap_or(0))
            } else {
                response.content_length()
            };
            let mut stream = response.bytes_stream();

            let (mut file, mut hasher, mut bytes_done) = if is_partial {
                let offset = resume_from.unwrap_or(0);
                // Re-hash existing bytes for SHA256 continuity.
                let existing_data = tokio::fs::read(&temp_path).await.with_context(|| {
                    format!(
                        "Failed to read existing temp file {temp_path}",
                        temp_path = temp_path.display()
                    )
                })?;
                let mut hasher = Sha256::new();
                hasher.update(&existing_data);
                let file = tokio::fs::OpenOptions::new()
                    .append(true)
                    .open(&temp_path)
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to open bundle file for append {temp_path}",
                            temp_path = temp_path.display()
                        )
                    })?;
                (file, hasher, offset)
            } else {
                let file = tokio::fs::File::create(&temp_path).await.with_context(|| {
                    format!(
                        "Failed to create bundle file {temp_path}",
                        temp_path = temp_path.display()
                    )
                })?;
                (file, Sha256::new(), 0u64)
            };

            while let Some(chunk) = stream.next().await {
                let chunk = chunk.with_context(|| "Failed to read bundle download stream")?;
                if cancel.is_cancelled() {
                    let _ = file.flush().await;
                    return Err(InstallError::Cancelled);
                }
                file.write_all(&chunk).await.with_context(|| {
                    format!("Failed to write bundle {temp_path}", temp_path = temp_path.display())
                })?;
                hasher.update(&chunk);
                bytes_done = bytes_done.saturating_add(chunk.len() as u64);

                let progress = JobProgress {
                    bytes_done: Some(bytes_done),
                    bytes_total: total_bytes,
                    ..JobProgress::default()
                };
                tracker.update_progress(STEP_DOWNLOAD_BUNDLE, progress).await;
            }

            file.flush().await.with_context(|| {
                format!("Failed to flush bundle {temp_path}", temp_path = temp_path.display())
            })?;
            file.sync_all().await.with_context(|| {
                format!("Failed to sync bundle {temp_path}", temp_path = temp_path.display())
            })?;

            let actual_hash = to_hex(&hasher.finalize());
            if !actual_hash.eq_ignore_ascii_case(&bundle.sha256) {
                let expected = bundle.sha256.as_str();
                let actual = actual_hash.as_str();
                hash_mismatch = true;
                return Err(
                    anyhow!("Bundle hash mismatch: expected {expected}, got {actual}").into()
                );
            }

            Ok(())
        }
        .await;

        if let Err(err) = download_result {
            // Only delete temp file on hash mismatch; keep it for network errors
            // and cancellations so the next attempt can resume from the partial file.
            if hash_mismatch {
                let _ = tokio::fs::remove_file(&temp_path).await;
            }
            return Err(err);
        }

        if let Err(err) = tokio::fs::rename(&temp_path, &bundle_path).await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(anyhow!(
                "Failed to activate bundle download {bundle_path}: {err}",
                bundle_path = bundle_path.display()
            )
            .into());
        }

        Ok(bundle_path)
    }

    async fn extract_bundle(
        &self,
        manifest: &crate::marketplace::PluginManifest,
        bundle_path: &Path,
        base_real: &Path,
        cancel: &CancellationToken,
    ) -> Result<PathBuf, InstallError> {
        if cancel.is_cancelled() {
            return Err(InstallError::Cancelled);
        }

        let bundles_root = self.plugin_dir.join("bundles").join(&manifest.id);
        plugin_paths::ensure_dir_under(base_real, &bundles_root, "bundles").await?;

        let bundle_dir = bundles_root.join(&manifest.version);
        if bundle_dir.exists() {
            let version = manifest.version.as_str();
            return Err(anyhow!("Bundle version '{version}' is already installed").into());
        }

        let temp_id = Uuid::new_v4();
        let temp_dir = bundles_root.join(format!(".tmp-{temp_id}"));
        tokio::fs::create_dir_all(&temp_dir).await.with_context(|| {
            format!("Failed to create temp dir {temp_dir}", temp_dir = temp_dir.display())
        })?;

        let bundle_path = bundle_path.to_path_buf();
        let temp_dir_clone = temp_dir.clone();
        let entrypoint = manifest.entrypoint.clone();
        let plugin_kind = manifest.kind.clone();
        let cancel_clone = cancel.clone();

        let extraction = tokio::task::spawn_blocking(move || -> Result<(), anyhow::Error> {
            validate_entrypoint(&entrypoint)?;
            let file = std::fs::File::open(&bundle_path).with_context(|| {
                format!("Failed to open bundle {bundle_path}", bundle_path = bundle_path.display())
            })?;
            let reader: Box<dyn std::io::Read> =
                match bundle_path.extension().and_then(|ext| ext.to_str()) {
                    Some("zst") => Box::new(zstd::stream::read::Decoder::new(file)?),
                    Some("gz") => Box::new(flate2::read::GzDecoder::new(file)),
                    Some("tar") | None => Box::new(file),
                    Some(other) => {
                        return Err(anyhow!("Unsupported bundle extension '{other}'"));
                    },
                };
            safe_extract_archive(reader, &temp_dir_clone, Some(&cancel_clone))?;

            let entrypoint_path = temp_dir_clone.join(&entrypoint);
            validate_entrypoint_for_kind(&plugin_kind, &entrypoint_path)?;

            Ok(())
        })
        .await
        .context("Bundle extraction task failed");

        if cancel.is_cancelled() {
            let _ = tokio::fs::remove_dir_all(&temp_dir).await;
            return Err(InstallError::Cancelled);
        }

        match extraction {
            Ok(Ok(())) => {},
            Ok(Err(err)) => {
                let _ = tokio::fs::remove_dir_all(&temp_dir).await;
                if cancel.is_cancelled() {
                    return Err(InstallError::Cancelled);
                }
                return Err(err.into());
            },
            Err(err) => {
                let _ = tokio::fs::remove_dir_all(&temp_dir).await;
                return Err(err.into());
            },
        }

        if cancel.is_cancelled() {
            let _ = tokio::fs::remove_dir_all(&temp_dir).await;
            return Err(InstallError::Cancelled);
        }

        tokio::fs::rename(&temp_dir, &bundle_dir).await.with_context(|| {
            format!("Failed to activate bundle {bundle_dir}", bundle_dir = bundle_dir.display())
        })?;

        Ok(bundle_dir)
    }

    async fn activate_bundle(
        &self,
        manifest: &crate::marketplace::PluginManifest,
        bundle_dir: &Path,
        base_real: &Path,
    ) -> Result<PathBuf, InstallError> {
        let active_dir = plugin_active_dir(&self.plugin_dir);
        plugin_paths::ensure_dir_under(base_real, &active_dir, "active").await?;

        let entrypoint_path = bundle_dir.join(&manifest.entrypoint);
        let record = ActivePluginRecord {
            plugin_id: manifest.id.clone(),
            version: manifest.version.clone(),
            node_kind: manifest.node_kind.clone(),
            kind: manifest.kind.clone(),
            entrypoint: entrypoint_path.to_string_lossy().into_owned(),
            installed_at_ms: now_ms(),
        };
        let record_path = plugin_record_path(&self.plugin_dir, &manifest.id)?;
        let payload = serde_json::to_vec_pretty(&record)
            .context("Failed to serialize active plugin record")?;
        tokio::fs::write(&record_path, payload).await.with_context(|| {
            format!(
                "Failed to write active plugin record {record_path}",
                record_path = record_path.display()
            )
        })?;

        Ok(entrypoint_path)
    }

    async fn load_plugin(
        &self,
        manifest: &crate::marketplace::PluginManifest,
        entrypoint_path: &Path,
        expected_kind: &str,
    ) -> Result<PluginSummary, InstallError> {
        let plugin_type = match manifest.kind {
            PluginKind::Wasm => PluginType::Wasm,
            PluginKind::Native => PluginType::Native,
        };
        let entrypoint_path = entrypoint_path.to_path_buf();
        let manager = Arc::clone(&self.plugin_manager);
        let expected_kind_owned = expected_kind.to_string();

        let unloaded = tokio::task::spawn_blocking({
            let manager = Arc::clone(&manager);
            let expected_kind = expected_kind_owned.clone();
            move || -> anyhow::Result<bool> {
                let mut mgr = manager.blocking_lock();
                let unloaded = if mgr.is_plugin_loaded(&expected_kind) {
                    mgr.unload_plugin(&expected_kind, false)?;
                    true
                } else {
                    false
                };
                drop(mgr);
                Ok(unloaded)
            }
        })
        .await
        .context("Plugin unload task failed")??;
        if unloaded {
            info!(plugin = %expected_kind_owned, "Unloaded existing plugin before install");
            // Clear stale asset types so they are re-registered from the new manifest.
            self.plugin_asset_registry.unregister_plugin(&manifest.id).await;
        }

        let summary = tokio::task::spawn_blocking(move || {
            let mut mgr = manager.blocking_lock();
            mgr.load_from_path(plugin_type, entrypoint_path)
        })
        .await
        .context("Plugin load task failed")??;

        if summary.kind != expected_kind {
            let manager = Arc::clone(&self.plugin_manager);
            let summary_kind = summary.kind.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let mut mgr = manager.blocking_lock();
                let _ = mgr.unload_plugin(&summary_kind, true);
            })
            .await;

            let actual_kind = summary.kind.as_str();
            return Err(anyhow!(
                "Loaded plugin kind '{actual_kind}' does not match manifest kind '{expected_kind}'"
            )
            .into());
        }

        // Register asset types declared by this plugin's manifest.
        if !manifest.assets.is_empty() {
            self.plugin_asset_registry
                .register(&manifest.id, expected_kind, &manifest.assets)
                .await;
        }

        Ok(summary)
    }

    async fn download_models(
        &self,
        manifest: &crate::marketplace::PluginManifest,
        model_ids: Option<&[String]>,
        tracker: &JobTracker,
        cancel: &CancellationToken,
        registry_origin: Option<&OriginKey>,
    ) -> Result<(), InstallError> {
        tokio::fs::create_dir_all(&self.models_dir).await.with_context(|| {
            format!(
                "Failed to create models dir {models_dir}",
                models_dir = self.models_dir.display()
            )
        })?;

        let selected_models = select_models(&manifest.models, model_ids)?;
        let items_total = selected_models
            .iter()
            .map(|model| match &model.source {
                crate::marketplace::ModelSource::Huggingface { files, .. } => files.len() as u64,
                crate::marketplace::ModelSource::Url { .. } => 1,
            })
            .sum::<u64>();

        let mut items_done = 0u64;

        for model in selected_models {
            if cancel.is_cancelled() {
                return Err(InstallError::Cancelled);
            }

            let expected_bytes = if model.expected_size_bytes.is_some() && items_total == 1 {
                model.expected_size_bytes
            } else {
                None
            };

            match &model.source {
                crate::marketplace::ModelSource::Huggingface { repo_id, revision, files } => {
                    if model.gated && self.huggingface_token.is_none() {
                        return Err(anyhow!(
                            "Hugging Face token is required for gated model downloads"
                        )
                        .into());
                    }
                    let revision = revision.as_deref().unwrap_or("main");
                    for file in files {
                        if cancel.is_cancelled() {
                            return Err(InstallError::Cancelled);
                        }
                        let file_path = Path::new(file);
                        if !is_safe_relative_path(file_path) {
                            return Err(anyhow!("Invalid model file path '{file}'").into());
                        }
                        let expected_sha256 = model
                            .file_checksums
                            .get(file.as_str())
                            .map(String::as_str)
                            .or(if files.len() == 1 { model.sha256.as_deref() } else { None });
                        let target_path = self.models_dir.join(file_path);
                        let display_name = file.as_str();
                        let url = huggingface_model_url(repo_id, revision, file)?;
                        self.download_model_file(
                            DownloadModelRequest {
                                url: &url,
                                target_path: &target_path,
                                display_name,
                                items_done,
                                items_total,
                                expected_size: expected_bytes,
                                expected_sha256,
                                bearer_token: self.huggingface_token.as_deref(),
                                registry_origin: None,
                            },
                            tracker,
                            cancel,
                        )
                        .await?;
                        items_done = items_done.saturating_add(1);
                    }
                },
                crate::marketplace::ModelSource::Url { url } => {
                    if cancel.is_cancelled() {
                        return Err(InstallError::Cancelled);
                    }
                    if !self.allow_model_urls {
                        return Err(
                            anyhow!("Model URL downloads are disabled by configuration").into()
                        );
                    }
                    let parsed = self
                        .marketplace_policy
                        .validate_url("model url", url, registry_origin)
                        .await?;
                    let file_name = parsed
                        .path_segments()
                        .and_then(|mut segments| segments.next_back())
                        .filter(|name| !name.is_empty())
                        .ok_or_else(|| anyhow!("Could not determine file name from model URL"))?;
                    let file_path = Path::new(file_name);
                    if !is_safe_relative_path(file_path) {
                        return Err(anyhow!("Invalid model file name '{file_name}'").into());
                    }
                    let target_path = self.models_dir.join(file_path);
                    let display_name = file_name;
                    self.download_model_file(
                        DownloadModelRequest {
                            url: parsed.as_str(),
                            target_path: &target_path,
                            display_name,
                            items_done,
                            items_total,
                            expected_size: expected_bytes,
                            expected_sha256: model.sha256.as_deref(),
                            bearer_token: None,
                            registry_origin: registry_origin.cloned(),
                        },
                        tracker,
                        cancel,
                    )
                    .await?;
                    items_done = items_done.saturating_add(1);
                },
            }

            tracker
                .update_progress(
                    STEP_DOWNLOAD_MODELS,
                    JobProgress {
                        items_done: Some(items_done),
                        items_total: Some(items_total),
                        ..JobProgress::default()
                    },
                )
                .await;
        }

        Ok(())
    }

    async fn download_model_file(
        &self,
        request: DownloadModelRequest<'_>,
        tracker: &JobTracker,
        cancel: &CancellationToken,
    ) -> Result<(), InstallError> {
        let DownloadModelRequest {
            url,
            target_path,
            display_name,
            items_done,
            items_total,
            expected_size,
            expected_sha256,
            bearer_token,
            registry_origin,
        } = request;
        if target_path.exists() {
            self.maybe_extract_model_archive(target_path, cancel).await?;
            tracker
                .update_progress(
                    STEP_DOWNLOAD_MODELS,
                    JobProgress {
                        items_done: Some(items_done.saturating_add(1)),
                        items_total: Some(items_total),
                        current_item: Some(display_name.to_owned()),
                        ..JobProgress::default()
                    },
                )
                .await;
            return Ok(());
        }

        if let Some(parent) = target_path.parent() {
            tokio::fs::create_dir_all(parent).await.with_context(|| {
                format!("Failed to create model dir {parent}", parent = parent.display())
            })?;
        }

        let parsed = self
            .marketplace_policy
            .validate_url("model url", url, registry_origin.as_ref())
            .await?;

        let temp_path = target_path.with_extension("download-part");
        let mut hash_mismatch = false;
        let download_result: Result<(), InstallError> = async {
            // Check for existing partial download to enable resume.
            let existing_len = tokio::fs::metadata(&temp_path).await.ok().map(|m| m.len());
            let resume_from = existing_len.filter(|&len| len > 0);

            let (response, final_url) = validated_get_response(
                &self.download_client,
                &self.marketplace_policy,
                "model url",
                &parsed,
                registry_origin.as_ref(),
                bearer_token,
                resume_from,
            )
            .await?;
            let response = response
                .error_for_status()
                .with_context(|| format!("Model download failed for {final_url}"))?;

            let is_partial = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
            let total_bytes = if is_partial {
                response.content_length().map(|cl| cl + resume_from.unwrap_or(0))
            } else {
                response.content_length()
            }
            .or(expected_size);
            let mut stream = response.bytes_stream();

            let (mut file, mut hasher, mut bytes_done) = if is_partial {
                let offset = resume_from.unwrap_or(0);
                // Re-hash existing bytes for SHA256 continuity.
                let existing_data = tokio::fs::read(&temp_path).await.with_context(|| {
                    format!(
                        "Failed to read existing temp file {temp_path}",
                        temp_path = temp_path.display()
                    )
                })?;
                let h = expected_sha256.map(|_| {
                    let mut hasher = Sha256::new();
                    hasher.update(&existing_data);
                    hasher
                });
                let file = tokio::fs::OpenOptions::new()
                    .append(true)
                    .open(&temp_path)
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to open model file for append {temp_path}",
                            temp_path = temp_path.display()
                        )
                    })?;
                (file, h, offset)
            } else {
                let file = tokio::fs::File::create(&temp_path).await.with_context(|| {
                    format!(
                        "Failed to create model file {temp_path}",
                        temp_path = temp_path.display()
                    )
                })?;
                let h = expected_sha256.map(|_| Sha256::new());
                (file, h, 0u64)
            };

            while let Some(chunk) = stream.next().await {
                let chunk = chunk.with_context(|| "Failed to read model download stream")?;
                if cancel.is_cancelled() {
                    let _ = file.flush().await;
                    return Err(InstallError::Cancelled);
                }
                file.write_all(&chunk).await.with_context(|| {
                    format!(
                        "Failed to write model file {temp_path}",
                        temp_path = temp_path.display()
                    )
                })?;
                if let Some(ref mut hasher) = hasher {
                    hasher.update(&chunk);
                }
                bytes_done = bytes_done.saturating_add(chunk.len() as u64);

                tracker
                    .update_progress(
                        STEP_DOWNLOAD_MODELS,
                        JobProgress {
                            bytes_done: Some(bytes_done),
                            bytes_total: total_bytes,
                            items_done: Some(items_done),
                            items_total: Some(items_total),
                            current_item: Some(display_name.to_owned()),
                            ..JobProgress::default()
                        },
                    )
                    .await;
            }

            file.flush().await.with_context(|| {
                format!("Failed to flush model file {temp_path}", temp_path = temp_path.display())
            })?;
            file.sync_all().await.with_context(|| {
                format!("Failed to sync model file {temp_path}", temp_path = temp_path.display())
            })?;

            if let (Some(expected_hash), Some(hasher)) = (expected_sha256, hasher) {
                let actual_hash = to_hex(&hasher.finalize());
                if !actual_hash.eq_ignore_ascii_case(expected_hash) {
                    hash_mismatch = true;
                    return Err(anyhow!(
                        "Model hash mismatch: expected {expected_hash}, got {actual_hash}"
                    )
                    .into());
                }
            }

            Ok(())
        }
        .await;

        if let Err(err) = download_result {
            if hash_mismatch {
                let _ = tokio::fs::remove_file(&temp_path).await;
            }
            return Err(err);
        }

        if let Err(err) = tokio::fs::rename(&temp_path, target_path).await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(anyhow!(
                "Failed to move model file to {target_path}: {err}",
                target_path = target_path.display()
            )
            .into());
        }

        self.maybe_extract_model_archive(target_path, cancel).await?;

        Ok(())
    }

    async fn maybe_extract_model_archive(
        &self,
        archive_path: &Path,
        cancel: &CancellationToken,
    ) -> Result<(), InstallError> {
        let Some(kind) = model_archive_kind(archive_path) else {
            return Ok(());
        };
        if let Some(dir) = model_archive_dir(archive_path, &self.models_dir) {
            if dir.exists() {
                return Ok(());
            }
        }
        if cancel.is_cancelled() {
            return Err(InstallError::Cancelled);
        }
        tokio::fs::create_dir_all(&self.models_dir).await.with_context(|| {
            format!(
                "Failed to create models dir {models_dir}",
                models_dir = self.models_dir.display()
            )
        })?;

        let archive_path = archive_path.to_path_buf();
        let models_dir = self.models_dir.clone();
        let cancel_clone = cancel.clone();
        let extraction = tokio::task::spawn_blocking(move || -> Result<(), anyhow::Error> {
            let file = std::fs::File::open(&archive_path).with_context(|| {
                format!(
                    "Failed to open model archive {archive_path}",
                    archive_path = archive_path.display()
                )
            })?;
            let reader: Box<dyn std::io::Read> = match kind {
                ModelArchiveKind::TarZst => Box::new(zstd::stream::read::Decoder::new(file)?),
                ModelArchiveKind::TarGz => Box::new(flate2::read::GzDecoder::new(file)),
                ModelArchiveKind::TarBz2 => Box::new(bzip2::read::BzDecoder::new(file)),
                ModelArchiveKind::Tar => Box::new(file),
            };
            safe_extract_archive(reader, &models_dir, Some(&cancel_clone))?;
            Ok(())
        })
        .await
        .context("Model archive extraction task failed");

        if cancel.is_cancelled() {
            return Err(InstallError::Cancelled);
        }

        match extraction {
            Ok(Ok(())) => Ok(()),
            Ok(Err(err)) | Err(err) => Err(err.into()),
        }
    }
}

#[derive(Debug)]
enum InstallError {
    Cancelled,
    Other(anyhow::Error),
}

impl From<anyhow::Error> for InstallError {
    fn from(err: anyhow::Error) -> Self {
        Self::Other(err)
    }
}

fn install_steps() -> Vec<JobStep> {
    vec![
        JobStep {
            name: STEP_DOWNLOAD_MANIFEST.to_string(),
            status: StepStatus::Pending,
            progress: None,
            error: None,
        },
        JobStep {
            name: STEP_VERIFY_SIGNATURE.to_string(),
            status: StepStatus::Pending,
            progress: None,
            error: None,
        },
        JobStep {
            name: STEP_DOWNLOAD_BUNDLE.to_string(),
            status: StepStatus::Pending,
            progress: None,
            error: None,
        },
        JobStep {
            name: STEP_EXTRACT_BUNDLE.to_string(),
            status: StepStatus::Pending,
            progress: None,
            error: None,
        },
        JobStep {
            name: STEP_ACTIVATE.to_string(),
            status: StepStatus::Pending,
            progress: None,
            error: None,
        },
        JobStep {
            name: STEP_LOAD_PLUGIN.to_string(),
            status: StepStatus::Pending,
            progress: None,
            error: None,
        },
        JobStep {
            name: STEP_DOWNLOAD_MODELS.to_string(),
            status: StepStatus::Pending,
            progress: None,
            error: None,
        },
    ]
}

fn select_registry_version<'a>(
    index: &'a RegistryIndex,
    plugin_id: &str,
    version: Option<&str>,
) -> Result<&'a crate::marketplace::RegistryPluginVersion> {
    let plugin = index
        .plugins
        .iter()
        .find(|entry| entry.id == plugin_id)
        .ok_or_else(|| anyhow!("Plugin '{plugin_id}' not found in registry"))?;
    let version = match version {
        Some(v) => v,
        None => plugin.latest.as_deref().ok_or_else(|| {
            anyhow!("Registry does not specify a latest version for '{plugin_id}'")
        })?,
    };
    plugin
        .versions
        .iter()
        .find(|entry| entry.version == version)
        .ok_or_else(|| anyhow!("Version '{version}' not found for plugin '{plugin_id}'"))
}

fn select_models<'a>(
    models: &'a [crate::marketplace::ModelSpec],
    model_ids: Option<&[String]>,
) -> Result<Vec<&'a crate::marketplace::ModelSpec>> {
    let Some(model_ids) = model_ids else {
        return Ok(models.iter().collect());
    };
    if model_ids.is_empty() {
        return Err(anyhow!("Model selection cannot be empty"));
    }

    let mut by_id = HashMap::new();
    for model in models {
        if let Some(id) = model.id.as_deref() {
            if by_id.insert(id, model).is_some() {
                return Err(anyhow!("Duplicate model id '{id}' in manifest"));
            }
        }
    }
    if by_id.is_empty() {
        return Err(anyhow!("Model selection requires manifest models to include ids"));
    }

    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    for id in model_ids {
        if !seen.insert(id.as_str()) {
            continue;
        }
        let Some(model) = by_id.get(id.as_str()) else {
            return Err(anyhow!("Unknown model id '{id}'"));
        };
        selected.push(*model);
    }

    Ok(selected)
}

fn namespaced_kind(manifest: &crate::marketplace::PluginManifest) -> String {
    match manifest.kind {
        PluginKind::Wasm => format!("plugin::wasm::{node_kind}", node_kind = manifest.node_kind),
        PluginKind::Native => {
            format!("plugin::native::{node_kind}", node_kind = manifest.node_kind)
        },
    }
}

fn validate_manifest_compatibility(manifest: &crate::marketplace::PluginManifest) -> Result<()> {
    let Some(compatibility) = &manifest.compatibility else {
        return Ok(());
    };

    if !compatibility.os.is_empty() {
        let current_os = std::env::consts::OS;
        if !compatibility.os.iter().any(|entry| entry.eq_ignore_ascii_case(current_os)) {
            return Err(anyhow!("Plugin is not compatible with OS '{current_os}'"));
        }
    }

    if !compatibility.arch.is_empty() {
        let current_arch = std::env::consts::ARCH;
        if !compatibility.arch.iter().any(|entry| entry.eq_ignore_ascii_case(current_arch)) {
            return Err(anyhow!("Plugin is not compatible with architecture '{current_arch}'"));
        }
    }

    if let Some(requirement) = compatibility.streamkit.as_deref() {
        let requirement = VersionReq::parse(requirement).with_context(|| {
            format!("Invalid streamkit compatibility requirement '{requirement}'")
        })?;
        let current =
            Version::parse(env!("CARGO_PKG_VERSION")).context("Invalid StreamKit version")?;
        if !requirement.matches(&current) {
            return Err(anyhow!(
                "Plugin requires StreamKit {requirement}, current version is {current}"
            ));
        }
    }

    Ok(())
}

fn validate_entrypoint(entrypoint: &str) -> Result<()> {
    let path = Path::new(entrypoint);
    if path.as_os_str().is_empty() {
        return Err(anyhow!("Entrypoint must not be empty"));
    }
    if path.is_absolute() {
        return Err(anyhow!("Entrypoint must be a relative path"));
    }
    if !is_safe_relative_path(path) {
        return Err(anyhow!("Entrypoint contains invalid path segments"));
    }
    Ok(())
}

fn validate_entrypoint_for_kind(kind: &PluginKind, entrypoint_path: &Path) -> Result<()> {
    let extension = entrypoint_path.extension().and_then(|ext| ext.to_str());
    match kind {
        PluginKind::Wasm => {
            if extension != Some("wasm") {
                return Err(anyhow!("WASM entrypoint must have .wasm extension"));
            }
        },
        PluginKind::Native => {
            if extension != Some("so") && extension != Some("dylib") && extension != Some("dll") {
                return Err(anyhow!("Native entrypoint must be a shared library"));
            }
        },
    }
    Ok(())
}

fn safe_extract_archive<R: std::io::Read>(
    reader: R,
    dest: &Path,
    cancel: Option<&CancellationToken>,
) -> Result<()> {
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries().context("Failed to read bundle archive")? {
        if let Some(cancel) = cancel {
            if cancel.is_cancelled() {
                return Err(anyhow!("Bundle extraction cancelled"));
            }
        }
        let mut entry = entry.context("Failed to read archive entry")?;
        let path = entry.path().context("Failed to read entry path")?.to_path_buf();
        if !is_safe_relative_path(&path) {
            let path_display = path.display();
            return Err(anyhow!("Unsafe path in bundle: {path_display}"));
        }

        let entry_type = entry.header().entry_type();
        if entry_type == tar::EntryType::Symlink || entry_type == tar::EntryType::Link {
            return Err(anyhow!("Symlinks and hardlinks are not allowed in bundles"));
        }

        let target = dest.join(&path);
        if entry_type != tar::EntryType::Directory {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("Failed to create parent directory {parent}", parent = parent.display())
                })?;
            }
        }
        entry
            .unpack(&target)
            .with_context(|| format!("Failed to extract {target}", target = target.display()))?;
    }

    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum ModelArchiveKind {
    Tar,
    TarGz,
    TarBz2,
    TarZst,
}

fn model_archive_kind(path: &Path) -> Option<ModelArchiveKind> {
    let ext = path.extension()?.to_str()?;
    if ext.eq_ignore_ascii_case("tar") {
        return Some(ModelArchiveKind::Tar);
    }
    if ext.eq_ignore_ascii_case("tgz") {
        return Some(ModelArchiveKind::TarGz);
    }
    if ext.eq_ignore_ascii_case("tbz2") {
        return Some(ModelArchiveKind::TarBz2);
    }
    if ext.eq_ignore_ascii_case("tzst") {
        return Some(ModelArchiveKind::TarZst);
    }
    if ext.eq_ignore_ascii_case("gz")
        && path
            .file_stem()
            .and_then(|stem| Path::new(stem).extension())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("tar"))
    {
        return Some(ModelArchiveKind::TarGz);
    }
    if ext.eq_ignore_ascii_case("bz2")
        && path
            .file_stem()
            .and_then(|stem| Path::new(stem).extension())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("tar"))
    {
        return Some(ModelArchiveKind::TarBz2);
    }
    if ext.eq_ignore_ascii_case("zst")
        && path
            .file_stem()
            .and_then(|stem| Path::new(stem).extension())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("tar"))
    {
        return Some(ModelArchiveKind::TarZst);
    }
    None
}

fn model_archive_dir(path: &Path, base_dir: &Path) -> Option<PathBuf> {
    let kind = model_archive_kind(path)?;
    let file_stem = path.file_stem()?.to_str()?;
    let base = match kind {
        ModelArchiveKind::Tar => file_stem.to_string(),
        ModelArchiveKind::TarGz | ModelArchiveKind::TarBz2 | ModelArchiveKind::TarZst => {
            let stem_path = Path::new(file_stem);
            if stem_path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("tar")) {
                stem_path.file_stem()?.to_str()?.to_string()
            } else {
                file_stem.to_string()
            }
        },
    };
    Some(base_dir.join(base))
}

/// Write the marketplace manifest as `plugin.yml` inside the bundle directory.
///
/// [`crate::plugin_assets::read_local_plugin_manifest`] searches for YAML
/// manifests next to the plugin library file.  Marketplace bundles ship with
/// `manifest.json` (or nothing at all), so on server restart the asset-type
/// declarations would be lost.  Writing a `plugin.yml` next to the
/// entrypoint closes that gap.
async fn write_manifest_yml(
    manifest: &crate::marketplace::PluginManifest,
    bundle_dir: &Path,
) -> Result<()> {
    anyhow::ensure!(
        !manifest.entrypoint.is_empty(),
        "Cannot write plugin.yml: manifest entrypoint is empty"
    );

    // Place the file next to the entrypoint so `read_local_plugin_manifest`
    // (which searches relative to the library path) will find it regardless
    // of whether the entrypoint is at the bundle root or nested.
    let entrypoint_dir = bundle_dir.join(&manifest.entrypoint);
    let yml_dir = entrypoint_dir.parent().unwrap_or(bundle_dir);
    let yml_path = yml_dir.join("plugin.yml");
    let yaml = serde_saphyr::to_string(manifest).context("Failed to serialize manifest to YAML")?;
    tokio::fs::write(&yml_path, yaml.as_bytes())
        .await
        .with_context(|| format!("Failed to write {}", yml_path.display()))?;
    Ok(())
}

fn is_safe_relative_path(path: &Path) -> bool {
    if path.is_absolute() {
        return false;
    }
    path.components().all(|component| match component {
        Component::Normal(_) | Component::CurDir => true,
        Component::ParentDir | Component::RootDir | Component::Prefix(_) => false,
    })
}

fn huggingface_model_url(repo_id: &str, revision: &str, file: &str) -> Result<String> {
    let mut url = reqwest::Url::parse("https://huggingface.co")?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|()| anyhow!("Failed to build Hugging Face model URL"))?;
        for segment in repo_id.split('/').filter(|segment| !segment.is_empty()) {
            segments.push(segment);
        }
        segments.push("resolve");
        for segment in revision.split('/').filter(|segment| !segment.is_empty()) {
            segments.push(segment);
        }
        for segment in file.split('/').filter(|segment| !segment.is_empty()) {
            segments.push(segment);
        }
    }
    Ok(url.to_string())
}

fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |duration| duration.as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{anyhow, bail, Context, Result};
    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::plugins::UnifiedPluginManager;
    use axum::{routing::get, Router};
    use bytes::Bytes;
    use sha2::Sha256;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;
    use tokio::task::JoinHandle;

    fn make_job(status: JobStatus) -> InstallJob {
        InstallJob {
            info: JobInfo {
                status,
                started_at_ms: None,
                updated_at_ms: now_ms(),
                summary: "test".to_string(),
                steps: install_steps(),
            },
            cancel: CancellationToken::new(),
            request: InstallPluginRequest {
                registry: "registry".to_string(),
                plugin_id: "plugin".to_string(),
                version: None,
                install_models: false,
                model_ids: None,
            },
            permissions: Permissions::default(),
        }
    }

    #[test]
    fn prune_jobs_drops_oldest_queued_when_over_cap() {
        let mut state = InstallQueueState::default();
        let total = MAX_JOB_HISTORY + 2;

        for idx in 0..total {
            let is_queued = idx != 0;
            let status = if is_queued { JobStatus::Queued } else { JobStatus::Running };
            let job_id = format!("job-{idx}");
            state.jobs.insert(job_id.clone(), make_job(status));
            state.job_order.push_back(job_id.clone());
            if is_queued {
                state.queue.push_back(job_id);
            }
        }

        state.prune_jobs();

        assert_eq!(state.jobs.len(), MAX_JOB_HISTORY);
        assert!(state.jobs.contains_key("job-0"));
        assert!(!state.jobs.contains_key("job-1"));
        assert!(!state.jobs.contains_key("job-2"));
        assert_eq!(state.queue.len(), MAX_JOB_HISTORY - 1);
    }

    fn test_plugin_manager(plugin_dir: &Path) -> Result<SharedUnifiedPluginManager> {
        let resource_manager =
            Arc::new(streamkit_core::ResourceManager::new(streamkit_core::ResourcePolicy {
                keep_loaded: true,
                max_memory_mb: None,
            }));
        let engine =
            Arc::new(streamkit_engine::Engine::with_resource_manager(resource_manager.clone()));
        let wasm_dir = plugin_dir.join("wasm");
        let native_dir = plugin_dir.join("native");
        let manager = UnifiedPluginManager::new(
            engine,
            resource_manager,
            plugin_dir.to_path_buf(),
            wasm_dir,
            native_dir,
            Some(std::time::Duration::from_mins(5)),
        )?;
        Ok(Arc::new(tokio::sync::Mutex::new(manager)))
    }

    async fn start_file_server(
        payload: Bytes,
    ) -> Result<(std::net::SocketAddr, oneshot::Sender<()>, JoinHandle<Result<()>>)> {
        start_file_server_with_path("/model.bin", payload).await
    }

    async fn start_file_server_with_path(
        path: &str,
        payload: Bytes,
    ) -> Result<(std::net::SocketAddr, oneshot::Sender<()>, JoinHandle<Result<()>>)> {
        let path = path.to_string();
        let app = Router::new().route(
            path.as_str(),
            get(move || {
                let payload = payload.clone();
                async move {
                    ([(axum::http::header::CONTENT_TYPE, "application/octet-stream")], payload)
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app.into_make_service())
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await
                .context("serve test server")?;
            Ok(())
        });
        Ok((addr, shutdown_tx, handle))
    }

    fn test_manifest(
        models: Vec<crate::marketplace::ModelSpec>,
    ) -> crate::marketplace::PluginManifest {
        crate::marketplace::PluginManifest {
            schema_version: 1,
            id: "test".to_string(),
            name: None,
            version: "1.0.0".to_string(),
            node_kind: "test".to_string(),
            kind: PluginKind::Native,
            description: None,
            license: None,
            license_url: None,
            homepage: None,
            repository: None,
            entrypoint: "libtest.so".to_string(),
            bundle: Some(crate::marketplace::PluginBundle {
                url: "http://example.com/bundle.tar.zst".to_string(),
                sha256: "deadbeef".to_string(),
                size_bytes: None,
            }),
            compatibility: None,
            models,
            assets: Vec::new(),
        }
    }

    #[tokio::test]
    async fn download_model_from_url() -> Result<()> {
        let payload = Bytes::from_static(b"model-bytes");
        let (addr, shutdown_tx, server_handle) = match start_file_server(payload.clone()).await {
            Ok(values) => values,
            Err(err) => {
                if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
                    if io_err.kind() == std::io::ErrorKind::PermissionDenied {
                        tracing::warn!(error = %err, "Skipping model download test");
                        return Ok(());
                    }
                }
                return Err(err);
            },
        };
        let url = format!("http://{addr}/model.bin");

        let temp_dir = tempfile::tempdir()?;
        let plugin_dir = temp_dir.path().join("plugins");
        tokio::fs::create_dir_all(&plugin_dir).await?;

        let mut hasher = Sha256::new();
        hasher.update(&payload);
        let hash = to_hex(&hasher.finalize());

        let config = PluginConfig {
            directory: plugin_dir.to_string_lossy().to_string(),
            native_call_timeout_secs: Some(300),
            http_management: crate::config::PluginHttpConfig { allow_http_management: false },
            marketplace: crate::config::PluginMarketplaceConfig {
                marketplace_enabled: true,
                allow_native_marketplace: true,
                security: crate::config::PluginMarketplaceSecurityConfig {
                    allow_model_urls: true,
                    marketplace_scheme_policy: crate::config::MarketplaceSchemePolicy::AllowHttp,
                    marketplace_host_policy: crate::config::MarketplaceHostPolicy::AllowPrivate,
                    marketplace_url_allowlist: vec!["http://127.0.0.1:*".to_string()],
                    ..crate::config::PluginMarketplaceSecurityConfig::default()
                },
            },
            trusted_pubkeys: Vec::new(),
            registries: Vec::new(),
            models_dir: Some(temp_dir.path().join("models").to_string_lossy().to_string()),
            huggingface_token: None,
        };

        let queue = InstallJobQueue::new(
            &config,
            test_plugin_manager(&plugin_dir)?,
            crate::plugin_assets::PluginAssetRegistry::new(),
        )?;
        let manifest = test_manifest(vec![crate::marketplace::ModelSpec {
            id: None,
            name: None,
            default: false,
            source: crate::marketplace::ModelSource::Url { url: url.clone() },
            expected_size_bytes: Some(payload.len() as u64),
            sha256: Some(hash),
            file_checksums: HashMap::new(),
            license: None,
            license_url: None,
            gated: false,
        }]);
        let tracker = JobTracker { job_id: "test".to_string(), queue: queue.clone() };
        let cancel = CancellationToken::new();

        let registry_origin =
            origin_key(&reqwest::Url::parse("https://registry.example.com/index.json")?)?;
        queue
            .installer
            .download_models(&manifest, None, &tracker, &cancel, Some(&registry_origin))
            .await
            .map_err(|err| match err {
                InstallError::Cancelled => anyhow!("download cancelled"),
                InstallError::Other(err) => err,
            })?;

        let target_path = temp_dir.path().join("models").join("model.bin");
        let downloaded = tokio::fs::read(&target_path).await?;
        assert_eq!(downloaded, payload);

        let _ = shutdown_tx.send(());
        server_handle.await.context("file server task panicked")??;

        Ok(())
    }

    #[tokio::test]
    async fn download_model_archive_extracts() -> Result<()> {
        let mut tar_bytes = Vec::new();
        {
            let encoder = bzip2::write::BzEncoder::new(&mut tar_bytes, bzip2::Compression::best());
            let mut builder = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            let contents = b"model-data";
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, "model-dir/model.txt", &contents[..])?;
            let encoder = builder.into_inner()?;
            encoder.finish()?;
        }

        let payload = Bytes::from(tar_bytes);
        let (addr, shutdown_tx, server_handle) =
            match start_file_server_with_path("/model.tar.bz2", payload.clone()).await {
                Ok(values) => values,
                Err(err) => {
                    if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
                        if io_err.kind() == std::io::ErrorKind::PermissionDenied {
                            tracing::warn!(error = %err, "Skipping model archive test");
                            return Ok(());
                        }
                    }
                    return Err(err);
                },
            };
        let url = format!("http://{addr}/model.tar.bz2");

        let temp_dir = tempfile::tempdir()?;
        let plugin_dir = temp_dir.path().join("plugins");
        tokio::fs::create_dir_all(&plugin_dir).await?;

        let mut hasher = Sha256::new();
        hasher.update(&payload);
        let hash = to_hex(&hasher.finalize());

        let config = PluginConfig {
            directory: plugin_dir.to_string_lossy().to_string(),
            native_call_timeout_secs: Some(300),
            http_management: crate::config::PluginHttpConfig { allow_http_management: false },
            marketplace: crate::config::PluginMarketplaceConfig {
                marketplace_enabled: true,
                allow_native_marketplace: true,
                security: crate::config::PluginMarketplaceSecurityConfig {
                    allow_model_urls: true,
                    marketplace_scheme_policy: crate::config::MarketplaceSchemePolicy::AllowHttp,
                    marketplace_host_policy: crate::config::MarketplaceHostPolicy::AllowPrivate,
                    marketplace_url_allowlist: vec!["http://127.0.0.1:*".to_string()],
                    ..crate::config::PluginMarketplaceSecurityConfig::default()
                },
            },
            trusted_pubkeys: Vec::new(),
            registries: Vec::new(),
            models_dir: Some(temp_dir.path().join("models").to_string_lossy().to_string()),
            huggingface_token: None,
        };

        let queue = InstallJobQueue::new(
            &config,
            test_plugin_manager(&plugin_dir)?,
            crate::plugin_assets::PluginAssetRegistry::new(),
        )?;
        let manifest = test_manifest(vec![crate::marketplace::ModelSpec {
            id: None,
            name: None,
            default: false,
            source: crate::marketplace::ModelSource::Url { url: url.clone() },
            expected_size_bytes: Some(payload.len() as u64),
            sha256: Some(hash),
            file_checksums: HashMap::new(),
            license: None,
            license_url: None,
            gated: false,
        }]);
        let tracker = JobTracker { job_id: "test".to_string(), queue: queue.clone() };
        let cancel = CancellationToken::new();

        let registry_origin =
            origin_key(&reqwest::Url::parse("https://registry.example.com/index.json")?)?;
        queue
            .installer
            .download_models(&manifest, None, &tracker, &cancel, Some(&registry_origin))
            .await
            .map_err(|err| match err {
                InstallError::Cancelled => anyhow!("download cancelled"),
                InstallError::Other(err) => err,
            })?;

        let extracted_path = temp_dir.path().join("models/model-dir/model.txt");
        let extracted = tokio::fs::read(&extracted_path).await?;
        assert_eq!(extracted, b"model-data");

        let _ = shutdown_tx.send(());
        server_handle.await.context("file server task panicked")??;

        Ok(())
    }

    #[tokio::test]
    async fn gated_models_require_token() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let plugin_dir = temp_dir.path().join("plugins");
        tokio::fs::create_dir_all(&plugin_dir).await?;

        let config = PluginConfig {
            directory: plugin_dir.to_string_lossy().to_string(),
            native_call_timeout_secs: Some(300),
            http_management: crate::config::PluginHttpConfig { allow_http_management: false },
            marketplace: crate::config::PluginMarketplaceConfig {
                marketplace_enabled: true,
                allow_native_marketplace: true,
                security: crate::config::PluginMarketplaceSecurityConfig {
                    allow_model_urls: false,
                    ..crate::config::PluginMarketplaceSecurityConfig::default()
                },
            },
            trusted_pubkeys: Vec::new(),
            registries: Vec::new(),
            models_dir: Some(temp_dir.path().join("models").to_string_lossy().to_string()),
            huggingface_token: None,
        };

        let queue = InstallJobQueue::new(
            &config,
            test_plugin_manager(&plugin_dir)?,
            crate::plugin_assets::PluginAssetRegistry::new(),
        )?;
        let manifest = test_manifest(vec![crate::marketplace::ModelSpec {
            id: None,
            name: None,
            default: false,
            source: crate::marketplace::ModelSource::Huggingface {
                repo_id: "test/repo".to_string(),
                revision: None,
                files: vec!["model.bin".to_string()],
            },
            expected_size_bytes: None,
            sha256: None,
            file_checksums: HashMap::new(),
            license: None,
            license_url: None,
            gated: true,
        }]);
        let tracker = JobTracker { job_id: "test".to_string(), queue: queue.clone() };
        let cancel = CancellationToken::new();

        let Err(err) =
            queue.installer.download_models(&manifest, None, &tracker, &cancel, None).await
        else {
            bail!("expected gated model error");
        };
        let InstallError::Other(err) = err else {
            bail!("expected InstallError::Other");
        };
        assert!(err.to_string().contains("token"));

        Ok(())
    }

    #[tokio::test]
    async fn write_manifest_yml_creates_yaml_next_to_entrypoint() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let bundle_dir = temp_dir.path().join("bundle");
        tokio::fs::create_dir_all(&bundle_dir).await?;

        let mut manifest = test_manifest(Vec::new());
        manifest.assets = vec![crate::marketplace::PluginAssetSpec {
            type_id: "test-asset".to_string(),
            label: "Test Assets".to_string(),
            extensions: vec!["txt".to_string()],
            max_size_bytes: 1024,
            content_type: crate::marketplace::AssetContentType::Text,
            icon_hint: None,
            node_param: None,
            system_dir: None,
        }];

        write_manifest_yml(&manifest, &bundle_dir).await?;

        let yml_path = bundle_dir.join("plugin.yml");
        assert!(yml_path.exists(), "plugin.yml should be created in bundle dir");

        // Verify the written YAML can be parsed back as a PluginManifest.
        let contents = tokio::fs::read_to_string(&yml_path).await?;
        let parsed: crate::marketplace::PluginManifest =
            serde_saphyr::from_str(&contents).context("Failed to parse written plugin.yml")?;
        assert_eq!(parsed.id, "test");
        assert_eq!(parsed.assets.len(), 1);
        assert_eq!(parsed.assets[0].type_id, "test-asset");

        Ok(())
    }

    #[tokio::test]
    async fn write_manifest_yml_nested_entrypoint() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let bundle_dir = temp_dir.path().join("bundle");
        let nested_dir = bundle_dir.join("lib");
        tokio::fs::create_dir_all(&nested_dir).await?;

        let mut manifest = test_manifest(Vec::new());
        manifest.entrypoint = "lib/libtest.so".to_string();

        write_manifest_yml(&manifest, &bundle_dir).await?;

        // plugin.yml should be written next to the entrypoint, not at the
        // bundle root.
        let yml_in_nested = nested_dir.join("plugin.yml");
        let yml_in_root = bundle_dir.join("plugin.yml");
        assert!(yml_in_nested.exists(), "plugin.yml should be next to entrypoint");
        assert!(!yml_in_root.exists(), "plugin.yml should NOT be at bundle root");

        Ok(())
    }
}
