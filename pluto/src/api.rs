use crate::{
    history::ChatHistory,
    model_fallback::FallbackModel,
    providers::FallbackMetadata,
    tools::{ModelMode, ModelProfile, ModelSettings, Skill, VmSettings},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::time::Duration;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    time::timeout,
};

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    History,
    OpenChat {
        chat_id: String,
    },
    ModelSettings,
    SaveModelSettings {
        previous: Box<ModelSettings>,
        models: Box<ModelSettings>,
    },
    VmSettings,
    SaveVmSettings {
        previous: Box<VmSettings>,
        settings: Box<VmSettings>,
    },
    ListModels {
        mode: ModelMode,
        profile: ModelProfile,
    },
    CheckFallbackModel {
        model: FallbackModel,
    },
    InspectSkill {
        path: String,
    },
    ImportSkill {
        text: String,
    },
    Send {
        prompt: String,
    },
    SendChat {
        chat_id: Option<String>,
        prompt: String,
    },
    Decision {
        id: u64,
        accept: bool,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    History {
        history: ChatHistory,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chat_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chats: Option<Vec<ChatSummary>>,
    },
    ModelSettings {
        models: ModelSettings,
    },
    VmSettings {
        settings: Box<VmSettings>,
    },
    Models {
        names: Vec<String>,
    },
    FallbackModelMetadata {
        metadata: Option<FallbackMetadata>,
    },
    Skill {
        name: String,
        skill: Skill,
    },
    Status {
        message: String,
    },
    Warning {
        message: String,
    },
    Approval {
        id: u64,
        description: String,
    },
    Retry {
        id: u64,
        description: String,
    },
    Complete,
    Error {
        message: String,
    },
}

#[derive(Deserialize, Serialize)]
pub struct ChatSummary {
    pub id: String,
    pub title: String,
}

pub async fn read_message<T: DeserializeOwned>(
    reader: &mut (impl AsyncBufRead + Unpin),
    limit: u64,
) -> Result<Option<T>> {
    let mut line = Vec::new();
    if reader.take(limit + 1).read_until(b'\n', &mut line).await? == 0 {
        return Ok(None);
    }
    ensure!(
        line.ends_with(b"\n") && line.len() as u64 <= limit,
        "Invalid or oversized API frame"
    );
    Ok(Some(
        serde_json::from_slice(&line).context("Invalid API message")?,
    ))
}

pub async fn write_message(
    writer: &mut (impl AsyncWrite + Unpin),
    value: &impl Serialize,
) -> Result<()> {
    let mut frame = serde_json::to_vec(value)?;
    frame.push(b'\n');
    timeout(Duration::from_secs(10), writer.write_all(&frame)).await??;
    Ok(())
}
