use super::{
    Connection, ModelCapabilities, ThinkingLevel, ThinkingSetting, metadata, openai_compatible,
};
use anyhow::Result;
use reqwest::{Client, Method};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct Properties {
    chat_template: Option<String>,
    chat_template_caps: Option<TemplateCapabilities>,
    default_generation_settings: Option<GenerationSettings>,
}

#[derive(Deserialize)]
struct GenerationSettings {
    n_ctx: Option<u64>,
}

#[derive(Deserialize)]
struct RenderedTemplate {
    prompt: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct TemplateCapabilities {
    reasoning_effort_levels: Vec<String>,
    reasoning_effort_rejected: Vec<String>,
}

pub(super) async fn models(connection: &Connection, client: &Client) -> Result<Vec<String>> {
    openai_compatible::models(connection, client, "v1/models").await
}

pub(super) async fn capabilities(
    connection: &Connection,
    client: &Client,
    model: &str,
) -> Result<ModelCapabilities> {
    let mut url = connection.endpoint("props");
    url.query_pairs_mut().append_pair("model", model);
    let properties: Properties = metadata(connection.request(client, Method::GET, url)?).await?;
    let context_window = properties
        .default_generation_settings
        .as_ref()
        .and_then(|settings| settings.n_ctx)
        .filter(|limit| *limit > 0);
    Ok(ModelCapabilities {
        context_window,
        thinking: thinking(connection, client, model, properties).await,
    })
}

async fn thinking(
    connection: &Connection,
    client: &Client,
    model: &str,
    properties: Properties,
) -> Result<Vec<ThinkingLevel>> {
    let capabilities = properties.chat_template_caps.unwrap_or_default();
    let levels: Vec<_> = capabilities
        .reasoning_effort_levels
        .iter()
        .filter(|level| !capabilities.reasoning_effort_rejected.contains(level))
        .map(|level| ThinkingLevel::named(level))
        .collect();
    if !levels.is_empty() {
        return Ok(levels);
    }

    let can_toggle = properties.chat_template.as_deref().is_some_and(|template| {
        template
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .any(|name| name == "enable_thinking")
    });
    if !can_toggle {
        return Ok(levels);
    }

    let (on, off) = tokio::try_join!(
        render_template(connection, client, model, true),
        render_template(connection, client, model, false)
    )?;
    if on.prompt == off.prompt {
        return Ok(levels);
    }
    Ok(vec![
        ThinkingLevel {
            label: "On".into(),
            setting: ThinkingSetting::Toggle(true),
        },
        ThinkingLevel {
            label: "Off".into(),
            setting: ThinkingSetting::Toggle(false),
        },
    ])
}

async fn render_template(
    connection: &Connection,
    client: &Client,
    model: &str,
    enabled: bool,
) -> Result<RenderedTemplate> {
    let mut body = json!({
        "model": model,
        "messages": [{"role": "user", "content": "Hello."}]
    });
    set_thinking(&mut body, enabled);
    let request = connection
        .request(client, Method::POST, connection.endpoint("apply-template"))?
        .json(&body);
    metadata(request).await
}

pub(super) fn set_thinking(body: &mut Value, enabled: bool) {
    body["chat_template_kwargs"] = json!({"enable_thinking": enabled});
    body["reasoning_format"] = json!("auto");
}
