use anyhow::Context as _;
use chrono::NaiveDateTime;

use super::*;

impl Database {
    /// Creates a new user.
    pub async fn create_user(
        &self,
        email_address: &str,
        name: Option<&str>,
        admin: bool,
    ) -> Result<NewUserResult> {
        self.transaction(|tx| async {
            let tx = tx;
            let user = user::Entity::insert(user::ActiveModel {
                email_address: ActiveValue::set(Some(email_address.into())),
                name: ActiveValue::set(name.map(|s| s.into())),
                admin: ActiveValue::set(admin),
                metrics_id: ActiveValue::set(Uuid::new_v4()),
                ..Default::default()
            })
            .exec_with_returning(&*tx)
            .await?;

            Ok(NewUserResult {
                user_id: user.id,
                metrics_id: user.metrics_id.to_string(),
                signup_device_id: None,
                inviting_user_id: None,
            })
        })
        .await
    }

    /// Returns a user by ID. There are no access checks here, so this should only be used internally.
    pub async fn get_user_by_id(&self, id: UserId) -> Result<Option<user::Model>> {
        self.transaction(|tx| async move { Ok(user::Entity::find_by_id(id).one(&*tx).await?) })
            .await
    }

    /// Returns all users by ID. There are no access checks here, so this should only be used internally.
    pub async fn get_users_by_ids(&self, ids: Vec<UserId>) -> Result<Vec<user::Model>> {
        if ids.len() >= 10000_usize {
            return Err(anyhow!("too many users"))?;
        }
        self.transaction(|tx| async {
            let tx = tx;
            Ok(user::Entity::find()
                .filter(user::Column::Id.is_in(ids.iter().copied()))
                .all(&*tx)
                .await?)
        })
        .await
    }

    /// Returns all users flagged as staff.
    pub async fn get_staff_users(&self) -> Result<Vec<user::Model>> {
        self.transaction(|tx| async {
            let tx = tx;
            Ok(user::Entity::find()
                .filter(user::Column::Admin.eq(true))
                .all(&*tx)
                .await?)
        })
        .await
    }

    /// Returns a user by email address. There are no access checks here, so this should only be used internally.
    pub async fn get_user_by_email(&self, email: &str) -> Result<Option<User>> {
        self.transaction(|tx| async move {
            Ok(user::Entity::find()
                .filter(user::Column::EmailAddress.eq(email))
                .one(&*tx)
                .await?)
        })
        .await
    }

    /// get_all_users returns the next page of users. To get more call again with
    /// the same limit and the page incremented by 1.
    pub async fn get_all_users(&self, page: u32, limit: u32) -> Result<Vec<User>> {
        self.transaction(|tx| async move {
            Ok(user::Entity::find()
                .order_by_asc(user::Column::Id)
                .limit(limit as u64)
                .offset(page as u64 * limit as u64)
                .all(&*tx)
                .await?)
        })
        .await
    }

    /// Returns the metrics id for the user.
    pub async fn get_user_metrics_id(&self, id: UserId) -> Result<String> {
        #[derive(Copy, Clone, Debug, EnumIter, DeriveColumn)]
        enum QueryAs {
            MetricsId,
        }

        self.transaction(|tx| async move {
            let metrics_id: Uuid = user::Entity::find_by_id(id)
                .select_only()
                .column(user::Column::MetricsId)
                .into_values::<_, QueryAs>()
                .one(&*tx)
                .await?
                .context("could not find user")?;
            Ok(metrics_id.to_string())
        })
        .await
    }

    /// Sets "connected_once" on the user for analytics.
    pub async fn set_user_connected_once(&self, id: UserId, connected_once: bool) -> Result<()> {
        self.transaction(|tx| async move {
            user::Entity::update_many()
                .filter(user::Column::Id.eq(id))
                .set(user::ActiveModel {
                    connected_once: ActiveValue::set(connected_once),
                    ..Default::default()
                })
                .exec(&*tx)
                .await?;
            Ok(())
        })
        .await
    }

    /// Sets "accepted_tos_at" on the user to the given timestamp.
    pub async fn set_user_accepted_tos_at(
        &self,
        id: UserId,
        accepted_tos_at: Option<DateTime>,
    ) -> Result<()> {
        self.transaction(|tx| async move {
            user::Entity::update_many()
                .filter(user::Column::Id.eq(id))
                .set(user::ActiveModel {
                    accepted_tos_at: ActiveValue::set(accepted_tos_at),
                    ..Default::default()
                })
                .exec(&*tx)
                .await?;
            Ok(())
        })
        .await
    }

    /// hard delete the user.
    pub async fn destroy_user(&self, id: UserId) -> Result<()> {
        self.transaction(|tx| async move {
            access_token::Entity::delete_many()
                .filter(access_token::Column::UserId.eq(id))
                .exec(&*tx)
                .await?;
            user::Entity::delete_by_id(id).exec(&*tx).await?;
            Ok(())
        })
        .await
    }

    /// Find users where github_login ILIKE name_query.
    pub async fn fuzzy_search_users(&self, name_query: &str, limit: u32) -> Result<Vec<User>> {
        self.transaction(|tx| async {
            let tx = tx;
            let like_string = Self::fuzzy_like_string(name_query);
            let query = "
                SELECT users.*
                FROM users
                WHERE github_login ILIKE $1
                ORDER BY github_login <-> $2
                LIMIT $3
            ";

            Ok(user::Entity::find()
                .from_raw_sql(Statement::from_sql_and_values(
                    self.pool.get_database_backend(),
                    query,
                    vec![like_string.into(), name_query.into(), limit.into()],
                ))
                .all(&*tx)
                .await?)
        })
        .await
    }

    /// fuzzy_like_string creates a string for matching in-order using fuzzy_search_users.
    /// e.g. "cir" would become "%c%i%r%"
    pub fn fuzzy_like_string(string: &str) -> String {
        let mut result = String::with_capacity(string.len() * 2 + 1);
        for c in string.chars() {
            if c.is_alphanumeric() {
                result.push('%');
                result.push(c);
            }
        }
        result.push('%');
        result
    }

    /// Returns all feature flags.
    pub async fn list_feature_flags(&self) -> Result<Vec<feature_flag::Model>> {
        self.transaction(|tx| async move { Ok(feature_flag::Entity::find().all(&*tx).await?) })
            .await
    }

    /// Creates a new feature flag.
    pub async fn create_user_flag(&self, flag: &str, enabled_for_all: bool) -> Result<FlagId> {
        self.transaction(|tx| async move {
            let flag = feature_flag::Entity::insert(feature_flag::ActiveModel {
                flag: ActiveValue::set(flag.to_string()),
                enabled_for_all: ActiveValue::set(enabled_for_all),
                ..Default::default()
            })
            .exec(&*tx)
            .await?
            .last_insert_id;

            Ok(flag)
        })
        .await
    }

    /// Add the given user to the feature flag
    pub async fn add_user_flag(&self, user: UserId, flag: FlagId) -> Result<()> {
        self.transaction(|tx| async move {
            user_feature::Entity::insert(user_feature::ActiveModel {
                user_id: ActiveValue::set(user),
                feature_id: ActiveValue::set(flag),
            })
            .exec(&*tx)
            .await?;

            Ok(())
        })
        .await
    }

    /// Returns the active flags for the user.
    pub async fn get_user_flags(&self, user: UserId) -> Result<Vec<String>> {
        self.transaction(|tx| async move {
            #[derive(Copy, Clone, Debug, EnumIter, DeriveColumn)]
            enum QueryAs {
                Flag,
            }

            let flags_enabled_for_all = feature_flag::Entity::find()
                .filter(feature_flag::Column::EnabledForAll.eq(true))
                .select_only()
                .column(feature_flag::Column::Flag)
                .into_values::<_, QueryAs>()
                .all(&*tx)
                .await?;

            let flags_enabled_for_user = user::Model {
                id: user,
                ..Default::default()
            }
            .find_linked(user::UserFlags)
            .select_only()
            .column(feature_flag::Column::Flag)
            .into_values::<_, QueryAs>()
            .all(&*tx)
            .await?;

            let mut all_flags = HashSet::from_iter(flags_enabled_for_all);
            all_flags.extend(flags_enabled_for_user);

            Ok(all_flags.into_iter().collect())
        })
        .await
    }

    pub async fn get_users_missing_github_user_created_at(&self) -> Result<Vec<user::Model>> {
        self.transaction(|tx| async move {
            Ok(user::Entity::find()
                .filter(user::Column::GithubUserCreatedAt.is_null())
                .all(&*tx)
                .await?)
        })
        .await
    }
}
