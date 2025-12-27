// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};

use streamkit_api::Event as ApiEvent;
use streamkit_engine::Engine;

use crate::config::Config;
use crate::plugins::SharedUnifiedPluginManager;
use crate::session::SessionManager;

#[cfg(feature = "moq")]
use crate::moq_gateway::MoqGateway;

#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<Engine>,
    pub session_manager: Arc<Mutex<SessionManager>>,
    pub config: Arc<Config>,
    pub event_tx: broadcast::Sender<ApiEvent>,
    pub plugin_manager: SharedUnifiedPluginManager,
    #[cfg(feature = "moq")]
    pub moq_gateway: Option<Arc<MoqGateway>>,
}
