// lifted from cloud_llm_client

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionMode {
    Normal,
    Max,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionIntent {
    UserPrompt,
    ToolResults,
    ThreadSummarization,
    ThreadContextSummarization,
    CreateFile,
    EditFile,
    InlineAssist,
    TerminalInlineAssist,
    GenerateGitCommitMessage,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionRequestStatus {
    Queued {
        position: usize,
    },
    Started,
    Failed {
        code: String,
        message: String,
        request_id: Uuid,
        /// Retry duration in seconds.
        retry_after: Option<f64>,
    },
    ToolUseLimitReached,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionEvent<T> {
    Status(CompletionRequestStatus),
    Event(T),
}

impl<T> CompletionEvent<T> {
    pub fn into_status(self) -> Option<CompletionRequestStatus> {
        match self {
            Self::Status(status) => Some(status),
            Self::Event(_) => None,
        }
    }

    pub fn into_event(self) -> Option<T> {
        match self {
            Self::Event(event) => Some(event),
            Self::Status(_) => None,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct CountTokensResponse {
    pub tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictEditsBody {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub outline: Option<String>,
    pub input_events: String,
    pub input_excerpt: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub speculated_output: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictEditsResponse {
    pub request_id: Uuid,
    pub output_excerpt: String,
}
