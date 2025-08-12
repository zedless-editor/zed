use std::sync::Arc;

use anyhow::{Context as _, Result};
use chrono::{DateTime, Utc};
use util::ResultExt;

use crate::db::Database;
use crate::executor::Executor;
use crate::{AppState, Config};

const GITHUB_REQUESTS_PER_HOUR_LIMIT: usize = 5_000;
const SLEEP_DURATION_BETWEEN_USERS: std::time::Duration = std::time::Duration::from_millis(
    (GITHUB_REQUESTS_PER_HOUR_LIMIT as f64 / 60. / 60. * 1000.) as u64,
);

struct UserBackfiller {
    config: Config,
    github_access_token: Arc<str>,
    db: Arc<Database>,
    http_client: reqwest::Client,
    executor: Executor,
}

impl UserBackfiller {
    fn new(
        config: Config,
        github_access_token: Arc<str>,
        db: Arc<Database>,
        executor: Executor,
    ) -> Self {
        Self {
            config,
            github_access_token,
            db,
            http_client: reqwest::Client::new(),
            executor,
        }
    }
}

#[derive(serde::Deserialize)]
struct GithubUser {
    id: i32,
    created_at: DateTime<Utc>,
    name: Option<String>,
}
