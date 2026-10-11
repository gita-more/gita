//! Shared API state.

use crate::auth::Auth;
use crate::search::SearchService;
use gita_ai::AiEngine;
use gita_db::Db;
use gita_events::EventBus;
use gita_git::GitManager;
use std::sync::Arc;

/// Shared state for all API handlers.
#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub auth: Arc<Auth>,
    pub git: Arc<GitManager>,
    pub bus: EventBus,
    pub ai: Arc<AiEngine>,
    pub search: Arc<SearchService>,
    pub llm_config: gita_ai::LlmConfig,
}

impl AppState {
    pub fn new(
        db: Db,
        auth: Auth,
        git: GitManager,
        bus: EventBus,
        ai: AiEngine,
        search: SearchService,
        llm_config: gita_ai::LlmConfig,
    ) -> Self {
        Self {
            db,
            auth: Arc::new(auth),
            git: Arc::new(git),
            bus,
            ai: Arc::new(ai),
            search: Arc::new(search),
            llm_config,
        }
    }
}