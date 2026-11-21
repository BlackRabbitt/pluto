use super::{Connection, metadata};
use anyhow::Result;
use reqwest::{Client, Method};
use serde::Deserialize;

#[derive(Deserialize)]
struct Models {
    data: Vec<Model>,
}

#[derive(Deserialize)]
struct Model {
    id: String,
}

pub(super) async fn models(
    connection: &Connection,
    client: &Client,
    route: &str,
) -> Result<Vec<String>> {
    let request = connection.request(client, Method::GET, connection.endpoint(route))?;
    let list: Models = metadata(request).await?;
    Ok(list.data.into_iter().map(|model| model.id).collect())
}
