use crate::db::{self, ChannelRole, NewUserParams};

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use db::Database;
use serde::{Deserialize, de::DeserializeOwned};
use std::{fs, path::Path};

use crate::Config;

/// A GitHub user.
///
/// This representation corresponds to the entries in the `seed/github_users.json` file.
#[derive(Debug, Deserialize)]
struct GithubUser {
    id: i32,
    login: String,
    email: Option<String>,
    name: Option<String>,
    created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct SeedConfig {
    /// Which users to create as admins.
    admins: Vec<String>,
    /// Which channels to create (all admins are invited to all channels).
    channels: Vec<String>,
}

pub async fn seed(config: &Config, db: &Database, force: bool) -> anyhow::Result<()> {
    let client = reqwest::Client::new();

    if !db.get_all_users(0, 1).await?.is_empty() && !force {
        return Ok(());
    }

    let seed_path = config
        .seed_path
        .as_ref()
        .context("called seed with no SEED_PATH")?;

    let seed_config = load_admins(seed_path)
        .context(format!("failed to load {}", seed_path.to_string_lossy()))?;

    let mut first_user = None;
    let mut others = vec![];

    let flag_names = ["language-models"];
    let mut flags = Vec::new();

    let existing_feature_flags = db.list_feature_flags().await?;

    for flag_name in flag_names {
        if existing_feature_flags
            .iter()
            .any(|flag| flag.flag == flag_name)
        {
            log::info!("Flag {flag_name:?} already exists");
            continue;
        }

        let flag = db
            .create_user_flag(flag_name, false)
            .await
            .unwrap_or_else(|err| panic!("failed to create flag: '{flag_name}': {err}"));
        flags.push(flag);
    }

    for channel in seed_config.channels {
        let (channel, _) = db
            .create_channel(&channel, None, first_user.unwrap())
            .await
            .context("failed to create channel")?;

        for user_id in &others {
            db.invite_channel_member(
                channel.id,
                *user_id,
                first_user.unwrap(),
                ChannelRole::Admin,
            )
            .await
            .context("failed to add user to channel")?;
        }
    }

    Ok(())
}

fn load_admins(path: impl AsRef<Path>) -> anyhow::Result<SeedConfig> {
    let file_content = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&file_content)?)
}
