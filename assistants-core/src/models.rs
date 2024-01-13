use assistants_extra::anthropic;
use async_openai::types::{
    AssistantObject, ChatCompletionFunctions, FunctionObject, MessageObject, MessageRole,
    RunObject, RunStatus, ThreadObject,
};
use redis::RedisError;
use serde::{self, Deserialize, Serialize};
use sqlx::Error as SqlxError;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use uuid::Uuid;

use crate::function_calling::ModelConfig;

#[derive(Debug)]
pub enum MyError {
    SqlxError(SqlxError),
    RedisError(RedisError),
}

impl fmt::Display for MyError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MyError::SqlxError(e) => write!(f, "SqlxError: {}", e),
            MyError::RedisError(e) => write!(f, "RedisError: {}", e),
        }
    }
}

impl Error for MyError {}

impl From<SqlxError> for MyError {
    fn from(err: SqlxError) -> MyError {
        MyError::SqlxError(err)
    }
}

impl From<RedisError> for MyError {
    fn from(err: RedisError) -> MyError {
        MyError::RedisError(err)
    }
}

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize, Clone)]
pub struct Message {
    pub inner: MessageObject,
    pub user_id: String,
}

impl Default for Message {
    fn default() -> Self {
        Self {
            inner: MessageObject {
                id: Uuid::new_v4().to_string(),
                object: String::new(),
                created_at: 0,
                thread_id: Uuid::new_v4().to_string(),
                role: MessageRole::User,
                content: Vec::new(),
                assistant_id: None,
                run_id: None,
                file_ids: vec![],
                metadata: None,
            },
            user_id: String::new(),
        }
    }
}
impl From<assistants_core::models::Message> for async_openai::types::MessageObject {
    fn from(message: assistants_core::models::Message) -> Self {
        Self {
            id: message.inner.id,
            object: message.inner.object,
            created_at: message.inner.created_at,
            thread_id: message.inner.thread_id,
            role: message.inner.role,
            content: message.inner.content,
            assistant_id: message.inner.assistant_id,
            run_id: message.inner.run_id,
            file_ids: message.inner.file_ids,
            metadata: message.inner.metadata,
        }
    }
}

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize, Clone)]
pub struct Run {
    pub inner: RunObject,
    pub user_id: String,
}

impl Default for Run {
    fn default() -> Self {
        Self {
            inner: RunObject {
                id: Uuid::new_v4().to_string(),
                object: String::new(),
                created_at: 0,
                instructions: String::new(),
                thread_id: Uuid::new_v4().to_string(),
                assistant_id: Some(Uuid::new_v4().to_string()),
                status: RunStatus::Queued,
                last_error: None,
                expires_at: Some(0),
                started_at: Some(0),
                cancelled_at: None,
                failed_at: None,
                completed_at: None,
                model: String::new(),
                tools: Vec::new(),
                file_ids: Vec::new(),
                required_action: None,
                metadata: None,
            },
            user_id: String::new(),
        }
    }
}

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct Thread {
    pub inner: ThreadObject,
    pub user_id: String,
}

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct Assistant {
    pub inner: AssistantObject,
    pub user_id: String,
}

impl Default for Assistant {
    fn default() -> Self {
        Self {
            inner: AssistantObject {
                id: Uuid::new_v4().to_string(),
                object: String::new(),
                created_at: 0,
                name: None,
                description: None,
                model: "mixtral-8x7b-instruct".to_string(), // TODO everything should default to open source llm in the future when the repo is more stable
                instructions: Some("You are a helpful assistant.".to_string()),
                tools: Vec::new(),
                file_ids: Vec::new(),
                metadata: None,
            },
            user_id: Uuid::default().to_string(),
        }
    }
}

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize, Clone)]
pub struct SubmittedToolCall {
    // TODO asnyc openai models?
    pub id: String,
    pub output: String,
    pub run_id: String,
    pub created_at: i32,
    pub user_id: String,
}

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize, Clone)]
pub struct Function {
    pub inner: FunctionObject,
    pub assistant_id: String,
    pub user_id: String,
}

// Define a struct for the input
#[derive(Debug)]
pub struct FunctionCallInput {
    pub function: Function,
    pub user_context: String,
    pub model_config: ModelConfig,
}

#[derive(Debug, sqlx::FromRow, Serialize, Deserialize)]
pub struct Chunk {
    pub id: Uuid,
    pub sequence: i32,
    pub data: String,
    pub file_id: String,
    pub start_index: i32,
    pub end_index: i32,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
    // pub embedding: Option<Vec<f32>>,
    pub created_at: i32,
}

pub struct PartialChunk {
    pub sequence: i32,
    pub data: String,
    pub start_index: i32,
    pub end_index: i32,
}

// -- internal logic

pub struct RunError {
    pub message: String,
    pub run_id: String,
    pub thread_id: String,
    pub user_id: String,
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::fmt::Debug for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self)
    }
}

impl std::error::Error for RunError {}

// Define an enum for the type of LLMAction based on the provided context
#[derive(Debug, Serialize, Deserialize)]
pub enum LLMActionType {
    #[serde(rename = "steps")]
    Steps,
    #[serde(rename = "function_calling")]
    FunctionCalling,
    #[serde(rename = "code_interpreter")]
    CodeInterpreter,
    #[serde(rename = "retrieval")]
    Retrieval,
    Unknown,
    // Add additional action types as needed
}

// Update the LLMAction struct to use the LLMActionType enum
#[derive(Debug, Serialize, Deserialize)]
pub struct LLMAction {
    pub r#type: LLMActionType,
    pub content: String,
}
