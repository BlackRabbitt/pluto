use super::{Connection, FallbackMetadata};
use crate::model_fallback::{CatalogRateLimit, retry_after};
use anyhow::{Context, Result, ensure};
use reqwest::{Client, Method};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Models {
    data: Vec<Model>,
}

#[derive(Deserialize)]
struct Model {
    id: String,
    #[serde(default)]
    context_length: Value,
    #[serde(default)]
    supported_parameters: Value,
    #[serde(default)]
    capabilities: Value,
    #[serde(default)]
    pricing: Value,
}

pub(super) async fn models(
    connection: &Connection,
    client: &Client,
    route: &str,
) -> Result<Vec<String>> {
    let list = model_list(connection, client, route).await?;
    Ok(list.data.into_iter().map(|model| model.id).collect())
}

async fn model_list(connection: &Connection, client: &Client, route: &str) -> Result<Models> {
    const MAX_CATALOG_BYTES: usize = 2 * 1024 * 1024;
    let request = connection.request(client, Method::GET, connection.endpoint(route))?;
    let mut response = request.send().await.map_err(reqwest::Error::without_url)?;
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(CatalogRateLimit(retry_after(response.headers())).into());
    }
    response
        .error_for_status_ref()
        .map_err(reqwest::Error::without_url)?;
    ensure!(
        response.status().is_success(),
        "Model catalog returned HTTP {}",
        response.status()
    );
    ensure!(
        response.content_length().unwrap_or(0) <= MAX_CATALOG_BYTES as u64,
        "Model catalog exceeds {MAX_CATALOG_BYTES} bytes"
    );
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(reqwest::Error::without_url)?
    {
        ensure!(
            chunk.len() <= MAX_CATALOG_BYTES - body.len(),
            "Model catalog exceeds {MAX_CATALOG_BYTES} bytes"
        );
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).context("Invalid model catalog")
}

pub(super) async fn fallback_catalog(
    connection: &Connection,
    client: &Client,
    models: &[&str],
) -> Result<Vec<Option<FallbackMetadata>>> {
    let list = match model_list(connection, client, "models").await {
        Ok(list) => list,
        Err(error)
            if error.downcast_ref::<reqwest::Error>().is_some_and(|error| {
                matches!(
                    error.status(),
                    Some(reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::METHOD_NOT_ALLOWED)
                )
            }) =>
        {
            return Ok(vec![None; models.len()]);
        }
        Err(error) => return Err(error),
    };
    Ok(models
        .iter()
        .map(|name| {
            list.data
                .iter()
                .find(|item| item.id == *name)
                .map(Model::fallback_metadata)
        })
        .collect())
}

impl Model {
    fn context_window(&self) -> Option<u64> {
        self.context_length.as_u64().filter(|limit| *limit > 0)
    }

    fn fallback_metadata(&self) -> FallbackMetadata {
        let supports_tools = self.capabilities["tool_calling"].as_bool().or_else(|| {
            self.supported_parameters
                .as_array()
                .or_else(|| self.capabilities.as_array())
                .map(|values| {
                    values
                        .iter()
                        .any(|value| matches!(value.as_str(), Some("tools" | "tool_calling")))
                })
        });
        FallbackMetadata {
            context_window: self.context_window(),
            supports_tools,
            paid: self.pricing.as_object().is_some_and(|prices| {
                prices.values().any(|price| {
                    price
                        .as_f64()
                        .or_else(|| price.as_str()?.parse().ok())
                        .is_some_and(|price| price != 0.0)
                })
            }),
        }
    }
}

pub(super) async fn context_window(
    connection: &Connection,
    client: &Client,
    model: &str,
) -> Option<u64> {
    model_list(connection, client, "models")
        .await
        .ok()?
        .data
        .into_iter()
        .find(|item| item.id == model)?
        .context_window()
}
