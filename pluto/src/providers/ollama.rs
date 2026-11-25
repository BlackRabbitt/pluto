use super::{Connection, ThinkingLevel, ThinkingSetting, metadata};
use anyhow::Result;
use reqwest::{Client, Method};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct Tags {
    models: Vec<Model>,
}

#[derive(Deserialize)]
struct Model {
    name: Option<String>,
    model: Option<String>,
}

#[derive(Deserialize)]
struct ModelInfo {
    thinking: Option<Thinking>,
}

#[derive(Deserialize)]
struct Thinking {
    values: Vec<ThinkingValue>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ThinkingValue {
    Named(String),
    Enabled(bool),
}

pub(super) async fn models(connection: &Connection, client: &Client) -> Result<Vec<String>> {
    let request = connection.request(client, Method::GET, connection.endpoint("api/tags"))?;
    let tags: Tags = metadata(request).await?;
    Ok(tags
        .models
        .into_iter()
        .filter_map(|model| model.name.or(model.model))
        .collect())
}

pub(super) async fn thinking(
    connection: &Connection,
    client: &Client,
    model: &str,
) -> Result<Vec<ThinkingLevel>> {
    let request = connection
        .request(client, Method::POST, connection.endpoint("api/show"))?
        .json(&json!({ "model": model }));
    let info: ModelInfo = metadata(request).await?;
    let values = info
        .thinking
        .map(|thinking| thinking.values)
        .unwrap_or_default();
    if matches!(values.as_slice(), [ThinkingValue::Enabled(false)]) {
        return Ok(Vec::new());
    }
    Ok(values
        .into_iter()
        .map(|value| match value {
            ThinkingValue::Named(level) => ThinkingLevel::named(&level),
            ThinkingValue::Enabled(false) => ThinkingLevel::named("none"),
            // Ollama maps a recognized effort to true for boolean-only models.
            ThinkingValue::Enabled(true) => ThinkingLevel {
                label: "On".into(),
                setting: ThinkingSetting::Effort("low".into()),
            },
        })
        .collect())
}
