mod llama_cpp;
mod ollama;
mod openai_compatible;

use anyhow::{Context, Result, ensure};
use reqwest::{Client, Method, RequestBuilder, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::env;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    LlamaCpp,
    Ollama,
    OpenAiCompatible,
}

impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Self::LlamaCpp => "llama.cpp",
            Self::Ollama => "Ollama",
            Self::OpenAiCompatible => "OpenAI-compatible",
        }
    }
}

pub struct ThinkingLevel {
    pub label: String,
    pub setting: ThinkingSetting,
}

pub struct ModelCapabilities {
    pub thinking: Result<Vec<ThinkingLevel>>,
    pub context_window: Option<u64>,
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
pub struct FallbackMetadata {
    pub context_window: Option<u64>,
    pub supports_tools: Option<bool>,
    pub paid: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ThinkingSetting {
    Effort(String),
    Toggle(bool),
}

impl ThinkingLevel {
    fn named(effort: &str) -> Self {
        Self {
            label: if effort == "none" { "Off" } else { effort }.into(),
            setting: ThinkingSetting::Effort(effort.into()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Connection {
    pub provider: Provider,
    base_url: Url,
    token_env: String,
}

impl Connection {
    pub fn origin(&self) -> String {
        self.base_url.origin().ascii_serialization()
    }

    pub fn new(provider: Provider, base_url: &str, token_env: &str) -> Result<Self> {
        let mut base_url = Url::parse(base_url.trim()).context("Enter a server URL")?;
        ensure!(
            matches!(base_url.scheme(), "http" | "https") && base_url.host_str().is_some(),
            "Server URL needs HTTP or HTTPS"
        );
        ensure!(
            base_url.username().is_empty()
                && base_url.password().is_none()
                && base_url.query().is_none()
                && base_url.fragment().is_none(),
            "Enter a base URL without credentials, a query, or a fragment"
        );
        let path = base_url.path().trim_end_matches('/');
        let path = match provider {
            Provider::OpenAiCompatible => path,
            _ => path.strip_suffix("/v1").unwrap_or(path),
        }
        .to_owned();
        base_url.set_path(&path);
        Ok(Self {
            provider,
            base_url,
            token_env: token_env.trim().into(),
        })
    }

    fn endpoint(&self, route: &str) -> Url {
        let mut url = self.base_url.clone();
        url.set_path(&format!(
            "{}/{route}",
            self.base_url.path().trim_end_matches('/')
        ));
        url
    }

    fn request(&self, client: &Client, method: Method, url: Url) -> Result<RequestBuilder> {
        let request = client.request(method, url);
        if self.token_env.is_empty() {
            return Ok(request);
        }
        let token = env::var(&self.token_env)
            .with_context(|| format!("Set {} before launching Pluto", self.token_env))?;
        Ok(request.bearer_auth(token))
    }

    pub async fn models(&self, client: &Client) -> Result<Vec<String>> {
        let mut models = match self.provider {
            Provider::LlamaCpp => llama_cpp::models(self, client).await?,
            Provider::Ollama => ollama::models(self, client).await?,
            Provider::OpenAiCompatible => openai_compatible::models(self, client, "models").await?,
        };
        models.retain(|name| !name.trim().is_empty());
        models.sort_unstable();
        models.dedup();
        Ok(models)
    }

    pub async fn capabilities(&self, client: &Client, model: &str) -> Result<ModelCapabilities> {
        match self.provider {
            Provider::LlamaCpp => llama_cpp::capabilities(self, client, model).await,
            Provider::Ollama => Ok(ModelCapabilities {
                thinking: ollama::thinking(self, client, model).await,
                context_window: None,
            }),
            Provider::OpenAiCompatible => Ok(ModelCapabilities {
                thinking: Ok(Vec::new()),
                context_window: openai_compatible::context_window(self, client, model).await,
            }),
        }
    }

    pub async fn fallback_metadata(
        &self,
        client: &Client,
        model: &str,
    ) -> Result<Option<FallbackMetadata>> {
        Ok(self
            .fallback_catalog(client, &[model])
            .await?
            .pop()
            .flatten())
    }

    pub async fn fallback_catalog(
        &self,
        client: &Client,
        models: &[&str],
    ) -> Result<Vec<Option<FallbackMetadata>>> {
        openai_compatible::fallback_catalog(self, client, models).await
    }

    pub fn chat_request(
        &self,
        client: &Client,
        model: &str,
        messages: &[Value],
        tools: &[Value],
        thinking: Option<&ThinkingSetting>,
    ) -> Result<RequestBuilder> {
        let route = match self.provider {
            Provider::OpenAiCompatible => "chat/completions",
            _ => "v1/chat/completions",
        };
        #[derive(Serialize)]
        struct ChatBody<'a> {
            model: &'a str,
            messages: &'a [Value],
            stream: bool,
            #[serde(skip_serializing_if = "<[_]>::is_empty")]
            tools: &'a [Value],
            #[serde(flatten)]
            options: Value,
        }
        let mut options = json!({});
        match thinking {
            Some(ThinkingSetting::Effort(effort)) => options["reasoning_effort"] = json!(effort),
            Some(ThinkingSetting::Toggle(enabled)) => {
                llama_cpp::set_thinking(&mut options, *enabled)
            }
            None => {}
        }
        Ok(self
            .request(client, Method::POST, self.endpoint(route))?
            .json(&ChatBody {
                model,
                messages,
                stream: false,
                tools,
                options,
            }))
    }
}

async fn metadata<T: serde::de::DeserializeOwned>(request: RequestBuilder) -> Result<T> {
    let response = request
        .send()
        .await
        .context("Could not reach the model server")?;
    ensure!(
        response.status().is_success(),
        "Model server returned HTTP {}",
        response.status()
    );
    response
        .json()
        .await
        .context("Invalid model server metadata")
}
