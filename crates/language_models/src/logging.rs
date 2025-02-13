use anthropic::{AnthropicError, ANTHROPIC_API_URL};
use anyhow::{anyhow, Context as _, Result};
use gpui::BackgroundExecutor;
use http_client::{AsyncBody, HttpClient, Method, Request as HttpRequest};
use std::env;
use std::sync::Arc;
use util::ResultExt;

use crate::provider::anthropic::PROVIDER_ID as ANTHROPIC_PROVIDER_ID;
