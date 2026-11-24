use crate::providers::{Connection, Provider};
use anyhow::{Result, ensure};
use reqwest::{Client, header::HeaderMap};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fmt,
    io::{self, Write},
    sync::Mutex,
    time::{Duration, Instant},
};

pub const MAX_FALLBACK_MODELS: usize = 4;
pub type FailedModels = BTreeSet<(String, Option<String>)>;

#[derive(Debug)]
pub struct CatalogRateLimit(pub Duration);

impl fmt::Display for CatalogRateLimit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Model catalog returned HTTP 429")
    }
}

impl std::error::Error for CatalogRateLimit {}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FallbackSettings {
    pub enabled: bool,
    pub models: Vec<FallbackModel>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FallbackModel {
    pub url: String,
    pub model: String,
    pub token_env: String,
    pub context_limit: u32,
    pub verified_tools: bool,
    pub free_open: bool,
}

impl FallbackModel {
    pub fn connection(&self) -> Result<Connection> {
        Connection::new(Provider::OpenAiCompatible, &self.url, &self.token_env)
    }
}

impl FallbackSettings {
    pub fn validate(&self) -> Result<()> {
        if self.enabled {
            ensure!(
                (1..=MAX_FALLBACK_MODELS).contains(&self.models.len()),
                "Add one to {MAX_FALLBACK_MODELS} fallback models"
            );
            for entry in &self.models {
                entry.connection()?;
                ensure!(
                    !entry.model.trim().is_empty(),
                    "Enter each fallback model ID"
                );
                ensure!(
                    entry.free_open,
                    "Confirm each fallback model is free and openly licensed"
                );
            }
        }
        Ok(())
    }

    pub async fn pick(
        &self,
        client: &Client,
        cooldowns: &ModelCooldowns,
        failed: &FailedModels,
        input_budget: u64,
        needs_tools: bool,
    ) -> Option<(Connection, &str, u64)> {
        let candidates: Vec<_> = self
            .models
            .iter()
            .take(MAX_FALLBACK_MODELS)
            .filter_map(|entry| {
                let model = entry.model.trim();
                if model.is_empty() || !entry.free_open {
                    return None;
                }
                let connection = entry.connection().ok()?;
                let origin = connection.origin();
                let failed_here = failed.iter().any(|(failed_origin, failed_model)| {
                    failed_origin == &origin && failed_model.as_deref().is_none_or(|id| id == model)
                });
                (!failed_here).then_some((entry, connection, model))
            })
            .collect();
        let names: Vec<_> = candidates.iter().map(|(_, _, model)| *model).collect();
        let mut catalogs = Vec::new();
        for (index, (entry, connection, model)) in candidates.iter().enumerate() {
            if cooldowns.active(connection) {
                continue;
            }
            let cached = if let Some(previous) =
                catalogs.iter().position(|(source, _)| source == connection)
            {
                &catalogs[previous].1
            } else {
                let catalog = match tokio::time::timeout(
                    Duration::from_secs(5),
                    connection.fallback_catalog(client, &names),
                )
                .await
                {
                    Ok(Ok(catalog)) => Some(catalog),
                    Ok(Err(error)) => {
                        if let Some(rate_limit) = error.downcast_ref::<CatalogRateLimit>() {
                            cooldowns.limit(connection, rate_limit.0);
                        }
                        None
                    }
                    Err(_) => None,
                };
                let index = catalogs.len();
                catalogs.push((connection.clone(), catalog));
                &catalogs[index].1
            };
            let Some(catalog) = cached else { continue };
            let metadata = catalog.get(index).copied().flatten().unwrap_or_default();
            let limit = match (metadata.context_window, entry.context_limit) {
                (Some(reported), 0) => reported,
                (Some(reported), manual) => reported.min(u64::from(manual)),
                (None, manual) => u64::from(manual),
            };
            if !metadata.paid
                && limit >= input_budget
                && (!needs_tools || metadata.supports_tools.unwrap_or(entry.verified_tools))
            {
                return Some((connection.clone(), model, limit));
            }
        }
        None
    }
}

#[derive(Default)]
pub struct ModelCooldowns(Mutex<Vec<(String, Instant)>>);

impl ModelCooldowns {
    pub fn active(&self, connection: &Connection) -> bool {
        let origin = connection.origin();
        let now = Instant::now();
        let mut entries = self.0.lock().unwrap_or_else(|error| error.into_inner());
        entries.retain(|(_, until)| *until > now);
        entries.iter().any(|(key, _)| key == &origin)
    }

    pub fn limit(&self, connection: &Connection, duration: Duration) {
        const MAX_COOLDOWNS: usize = 2 * MAX_FALLBACK_MODELS;
        let origin = connection.origin();
        let now = Instant::now();
        let until = now.checked_add(duration).unwrap_or(now + DEFAULT_COOLDOWN);
        let mut entries = self.0.lock().unwrap_or_else(|error| error.into_inner());
        entries.retain(|(key, expiry)| key != &origin && *expiry > now);
        if entries.len() == MAX_COOLDOWNS {
            entries.remove(0);
        }
        entries.push((origin, until));
    }
}

const DEFAULT_COOLDOWN: Duration = Duration::from_secs(60);

pub fn retry_after(headers: &HeaderMap) -> Duration {
    let Some(value) = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
    else {
        return DEFAULT_COOLDOWN;
    };
    if let Ok(seconds) = value.parse() {
        return Duration::from_secs(seconds);
    }
    chrono::DateTime::parse_from_rfc2822(value).map_or(DEFAULT_COOLDOWN, |date| {
        (date.to_utc() - chrono::Utc::now())
            .to_std()
            .unwrap_or_default()
    })
}

// Count the serialized request without buffering it. One token per byte plus
// an output reserve is deliberately conservative across model tokenizers.
pub fn context_budget(messages: &[Value], tools: &[Value]) -> Result<u64> {
    struct Bytes(u64);
    impl Write for Bytes {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len() as u64);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut budget = Bytes(2_048);
    serde_json::to_writer(&mut budget, messages)?;
    serde_json::to_writer(&mut budget, tools)?;
    Ok(budget.0)
}
