use super::{Lookup, egui, field};
use anyhow::Result;
use pluto::{
    model_fallback::{FallbackModel, FallbackSettings, MAX_FALLBACK_MODELS},
    providers::{Connection, FallbackMetadata, Provider},
    tools::{ModelMode, ModelProfile},
};
use std::future::Future;
use tokio::runtime::Runtime;

#[derive(Default)]
pub(super) struct ModelCatalog {
    source: Option<(ModelMode, Connection)>,
    models: Lookup<Vec<String>>,
}

impl ModelCatalog {
    pub(super) fn show<F: Future<Output = Result<Vec<String>>> + Send + 'static>(
        &mut self,
        ui: &mut egui::Ui,
        runtime: &Runtime,
        mode: ModelMode,
        profile: &mut ModelProfile,
        token_hint: &str,
        load: impl FnOnce(Connection, ModelProfile) -> F,
    ) {
        self.models.poll();
        model_profile_ui(ui, profile, mode, token_hint, |ui, profile| {
            let connection = profile.connection(mode).ok();
            let matches = connection.as_ref().is_some_and(|connection| {
                self.source.as_ref().is_some_and(|(source_mode, source)| {
                    *source_mode == mode && source == connection
                })
            });
            if ui
                .add_enabled(
                    connection.is_some()
                        && (!matches || !matches!(self.models, Lookup::Loading(_))),
                    egui::Button::new("Load models"),
                )
                .clicked()
                && let Some(connection) = connection
            {
                self.source = Some((mode, connection.clone()));
                self.models
                    .start(runtime, ui.ctx(), load(connection, profile.clone()));
            }
            if matches {
                catalog_result(ui, &self.models, &mut profile.name);
            }
        });
    }
}

fn model_profile_ui(
    ui: &mut egui::Ui,
    profile: &mut ModelProfile,
    mode: ModelMode,
    token_hint: &str,
    show_catalog: impl FnOnce(&mut egui::Ui, &mut ModelProfile),
) {
    let previous_provider = profile.provider;
    let previous_url = profile.url.clone();
    let previous_model = profile.name.clone();
    if mode == ModelMode::LocalOnly {
        ui.label("Runtime");
        let provider_label = previous_provider.map_or("Choose a runtime", Provider::label);
        egui::ComboBox::from_id_salt("model_provider")
            .selected_text(provider_label)
            .show_ui(ui, |ui| {
                for provider in [Provider::LlamaCpp, Provider::Ollama] {
                    ui.selectable_value(&mut profile.provider, Some(provider), provider.label());
                }
            });
        if profile.provider != previous_provider {
            profile.name.clear();
        }
    }
    let (provider, hint) = if mode == ModelMode::OpenAiCompatible {
        (
            Some(Provider::OpenAiCompatible),
            "https://provider.example/v1",
        )
    } else {
        let hint = if profile.provider == Some(Provider::Ollama) {
            "http://localhost:11434"
        } else {
            "http://localhost:8080"
        };
        (profile.provider, hint)
    };
    field(ui, "Server URL", &mut profile.url, hint);
    field(
        ui,
        "Model token environment variable",
        &mut profile.token_env,
        token_hint,
    );
    show_catalog(ui, profile);
    field(
        ui,
        "Model name",
        &mut profile.name,
        "Select from the list or enter a model name",
    );
    if profile.provider != previous_provider
        || profile.url.trim() != previous_url.trim()
        || profile.name.trim() != previous_model.trim()
    {
        profile.context_limit = 0;
    }
    if provider != Some(Provider::LlamaCpp) {
        ui.horizontal(|ui| {
            ui.label("Context limit (input tokens)");
            ui.add(egui::DragValue::new(&mut profile.context_limit).range(0..=u32::MAX))
                .on_hover_text("Set to 0 to use the model's reported limit.");
        });
    }
}

fn catalog_result(ui: &mut egui::Ui, state: &Lookup<Vec<String>>, model_name: &mut String) {
    match state {
        Lookup::Idle => {}
        Lookup::Loading(_) => {
            ui.label("Loading models…");
        }
        Lookup::Complete(Ok(models)) if models.is_empty() => {
            ui.label("This server returned no models.");
        }
        Lookup::Complete(Ok(models)) => {
            egui::ComboBox::from_id_salt("available_models")
                .selected_text(if model_name.is_empty() {
                    "Choose a model"
                } else {
                    model_name.as_str()
                })
                .truncate()
                .show_ui(ui, |ui| {
                    for model in models {
                        if ui.selectable_label(model == model_name, model).clicked() {
                            model_name.clone_from(model);
                        }
                    }
                });
        }
        Lookup::Complete(Err(error)) => {
            ui.label(format!("Could not load models: {error:#}"));
        }
    }
}

pub(super) struct FallbackCheck {
    pub(super) index: usize,
    pub(super) result: Lookup<Option<FallbackMetadata>>,
}

pub(super) fn fallback_form(
    ui: &mut egui::Ui,
    draft: &mut FallbackSettings,
    check: &mut Option<FallbackCheck>,
    mut start_check: impl FnMut(&egui::Ui, &FallbackModel) -> Lookup<Option<FallbackMetadata>>,
) {
    if let Some(check) = check {
        check.result.poll();
    }
    ui.separator();
    ui.checkbox(&mut draft.enabled, "Try other free models automatically");
    if !draft.enabled {
        return;
    }
    ui.label("Pluto tries these servers in order and sends the current chat and tool results to the server it selects.");
    let mut remove = None;
    for (index, entry) in draft.models.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            ui.group(|ui| {
                let url = field(
                    ui,
                    "Fallback API base URL",
                    &mut entry.url,
                    "https://provider.example/v1",
                );
                let model = field(
                    ui,
                    "Model ID",
                    &mut entry.model,
                    "Copy the exact ID from the provider",
                );
                let token = field(
                    ui,
                    "Token environment variable",
                    &mut entry.token_env,
                    "Optional",
                );
                if url.changed() || model.changed() {
                    entry.free_open = false;
                    entry.verified_tools = false;
                    entry.context_limit = 0;
                }
                if (url.changed() || model.changed() || token.changed())
                    && check.as_ref().is_some_and(|check| check.index == index)
                {
                    *check = None;
                }
                if ui
                    .add_enabled(
                        !entry.model.trim().is_empty(),
                        egui::Button::new("Check model metadata"),
                    )
                    .clicked()
                {
                    *check = Some(FallbackCheck {
                        index,
                        result: start_check(ui, entry),
                    });
                }
                if let Some(check) = check.as_ref().filter(|check| check.index == index) {
                    match &check.result {
                        Lookup::Loading(_) => {
                            ui.label("Checking the provider catalog…");
                        }
                        Lookup::Complete(Ok(Some(metadata))) => {
                            let limit = metadata.context_window.map_or_else(
                                || "unknown".into(),
                                |limit| format!("{limit} tokens"),
                            );
                            let tools = match metadata.supports_tools {
                                Some(true) => "supported",
                                Some(false) => "unsupported",
                                None => "unknown",
                            };
                            ui.label(format!("Context · {limit}    Tool calls · {tools}"));
                            if metadata.paid {
                                ui.label("The provider lists a price for this model.");
                            }
                        }
                        Lookup::Complete(Ok(None)) => {
                            ui.label("The provider catalog has no metadata for this model ID.");
                        }
                        Lookup::Complete(Err(error)) => {
                            ui.label(format!("Could not check model metadata: {error:#}"));
                        }
                        Lookup::Idle => {}
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("Context limit");
                    ui.add(egui::DragValue::new(&mut entry.context_limit))
                        .on_hover_text("0 reads the limit from the provider catalog.");
                });
                ui.checkbox(
                    &mut entry.verified_tools,
                    "I verified tool support if the catalog omits it",
                );
                ui.checkbox(
                    &mut entry.free_open,
                    "I verified this model is free and openly licensed",
                );
                if ui.small_button("Remove fallback").clicked() {
                    remove = Some(index);
                }
            });
        });
    }
    if let Some(index) = remove {
        draft.models.remove(index);
        *check = None;
    }
    if draft.models.len() < MAX_FALLBACK_MODELS && ui.small_button("Add fallback model").clicked() {
        draft.models.push(Default::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_form_loads_catalog() -> Result<()> {
        let context = egui::Context::default();
        let runtime = tokio::runtime::Builder::new_current_thread().build()?;
        let mut catalog = ModelCatalog::default();
        let mut profile = ModelProfile {
            provider: Some(Provider::LlamaCpp),
            url: "http://127.0.0.1:8080".into(),
            ..Default::default()
        };
        let mut render = |events| {
            context.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        catalog.show(
                            ui,
                            &runtime,
                            ModelMode::LocalOnly,
                            &mut profile,
                            "Optional",
                            |_, _| async { Ok(vec!["a-model".to_owned()]) },
                        );
                    });
                },
            )
        };
        let output = render(Vec::new());
        let position = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Load models" => {
                    Some(text.pos + text.galley.size() / 2.0)
                }
                _ => None,
            })
            .ok_or_else(|| anyhow::anyhow!("Load models button is missing"))?;
        let click = |pressed| egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        render(vec![
            egui::Event::PointerMoved(position),
            click(true),
            click(false),
        ]);
        runtime.block_on(tokio::task::yield_now());
        let output = render(Vec::new());
        assert!(output.shapes.iter().any(|shape| matches!(
            &shape.shape, egui::Shape::Text(text) if text.galley.text() == "Choose a model"
        )));
        Ok(())
    }
}
