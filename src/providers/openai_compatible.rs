use super::{Connection, metadata};
use anyhow::Result;
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
    let request = connection.request(client, Method::GET, connection.endpoint(route))?;
    metadata(request).await
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
        .context_length
        .as_u64()
        .filter(|limit| *limit > 0)
}
