use crate::{
    agent::{TokenUsage, ToolPhase, TurnSelection},
    history::ChatHistory,
    model_fallback::FallbackModel,
    providers::{FallbackMetadata, Provider, ThinkingLevel, ThinkingSetting},
    tools::{
        ApprovalRule, Mcp, McpToolId, ModelMode, ModelProfile, ModelSettings, Skill, ToolActivity,
        VmSettings,
    },
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
    MoveChat {
        chat_id: String,
        archive: bool,
    },
    ListSkillsAndMcpTools,
    PickerCatalog,
    ModelCapabilities,
    SavedApprovals,
    ForgetApproval {
        rule: ApprovalRule,
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
    TestMcp {
        server: Mcp,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selection: Option<TurnSelection>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking: Option<ThinkingSetting>,
    },
    Decision {
        id: u64,
        accept: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remember: Option<ApprovalScope>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        archived: Option<Vec<ChatSummary>>,
    },
    ModelSettings {
        models: ModelSettings,
    },
    VmSettings {
        settings: Box<VmSettings>,
    },
    SkillsAndMcpTools {
        skills: Vec<String>,
        tools: Vec<McpToolId>,
        warnings: Vec<String>,
    },
    PickerCatalog {
        skills: Vec<String>,
        tools: Vec<McpToolId>,
        warnings: Vec<String>,
    },
    ModelCapabilities {
        mode: ModelMode,
        provider: Option<Provider>,
        context_window: Option<u64>,
        thinking: std::result::Result<Vec<ThinkingLevel>, String>,
    },
    SavedApprovals {
        rules: Vec<ApprovalRule>,
    },
    Models {
        names: Vec<String>,
    },
    FallbackModelMetadata {
        metadata: Option<FallbackMetadata>,
    },
    McpTools {
        names: Vec<String>,
    },
    Skill {
        name: String,
        skill: Skill,
    },
    Status {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        update: Option<ChatUpdate>,
    },
    Warning {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        update: Option<ChatUpdate>,
    },
    Approval {
        id: u64,
        description: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rule: Option<ApprovalRule>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
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

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalScope {
    Exact,
    OtherOperands,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatUpdate {
    Tool {
        activity: ToolActivity,
        phase: ToolPhase,
    },
    Skill {
        name: String,
    },
    Usage {
        tokens: Option<TokenUsage>,
    },
    ModelSwitch {
        provider: Provider,
        context_window: Option<u64>,
    },
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
