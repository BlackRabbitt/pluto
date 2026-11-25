use super::{
    Lookup, egui, field,
    model_settings::{FallbackCheck, ModelCatalog, fallback_form},
    settings_tabs::SettingsPage,
    vm_client::query_settings,
};
use anyhow::{Context, Result, bail, ensure};
use pluto::{
    api::{Request, Response},
    tools::{Mcp, ModelMode, Skill, VmSettings, valid_name},
};
use std::{fs::File, io::Read, path::PathBuf};
use tokio::runtime::Runtime;

pub(super) struct VmSettingsForm {
    saved: VmSettings,
    draft: VmSettings,
    page: SettingsPage,
    catalog: ModelCatalog,
    fallback_check: Option<FallbackCheck>,
    skill_path: String,
    import_from_mac: bool,
    server_name: String,
    server_http: bool,
    server_address: String,
    server_details: String,
}

impl VmSettingsForm {
    pub(super) fn new(settings: VmSettings) -> Self {
        Self {
            saved: settings.clone(),
            draft: settings,
            page: SettingsPage::Model,
            catalog: ModelCatalog::default(),
            fallback_check: None,
            skill_path: String::new(),
            import_from_mac: false,
            server_name: String::new(),
            server_http: false,
            server_address: String::new(),
            server_details: String::new(),
        }
    }

    pub(super) fn replace_settings(&mut self, settings: VmSettings) {
        let page = self.page;
        *self = Self::new(settings);
        self.page = page;
    }

    pub(super) fn add_skill(&mut self, name: String, skill: Skill) {
        self.draft.skills.insert(name, skill);
        self.skill_path.clear();
    }

    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        runtime: &Runtime,
        instance: &str,
    ) -> Result<Option<Request>> {
        let mut request = None;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(self.draft != self.saved, egui::Button::new("Save settings"))
                .clicked()
            {
                request = Some(Request::SaveVmSettings {
                    previous: Box::new(self.saved.clone()),
                    settings: Box::new(self.draft.clone()),
                });
            }
            if ui
                .button("Reload settings")
                .on_hover_text("Discard unsaved changes and reload from the VM")
                .clicked()
            {
                request = Some(Request::VmSettings);
            }
        });
        if self.draft != self.saved {
            ui.weak("Unsaved changes");
        }
        self.page.tabs(ui);
        match self.page {
            SettingsPage::Model => self.model_ui(ui, runtime, instance),
            SettingsPage::Skills => {
                if let Some(skill) = self.skills_ui(ui)? {
                    request = Some(skill);
                }
            }
            SettingsPage::Mcp => self.mcp_ui(ui),
            SettingsPage::Approvals => {
                ui.checkbox(
                    &mut self.draft.auto_approve_tools,
                    "Auto-approve tool calls in this VM",
                );
                ui.weak("When enabled, terminal and MCP calls can run without asking.");
            }
        }
        Ok(request)
    }

    fn model_ui(&mut self, ui: &mut egui::Ui, runtime: &Runtime, instance: &str) {
        ui.weak("Server addresses and token environment variables are resolved inside this VM.");
        ui.horizontal_wrapped(|ui| {
            for mode in [ModelMode::LocalOnly, ModelMode::OpenAiCompatible] {
                ui.radio_value(&mut self.draft.models.mode, mode, mode.label());
            }
        });
        for (mode, label) in [
            (ModelMode::LocalOnly, "Local model"),
            (ModelMode::OpenAiCompatible, "OpenAI-compatible model"),
        ] {
            egui::CollapsingHeader::new(label)
                .default_open(self.draft.models.mode == mode)
                .show(ui, |ui| {
                    let profile = match mode {
                        ModelMode::LocalOnly => &mut self.draft.models.local,
                        ModelMode::OpenAiCompatible => &mut self.draft.models.openai,
                    };
                    let catalog_instance = instance.to_owned();
                    self.catalog.show(ui, runtime, mode, profile, "Optional · set inside the VM", |_, profile| async move {
                        match query_settings(catalog_instance, Request::ListModels { mode, profile }).await? {
                            Response::Models { names } => Ok(names),
                            _ => bail!("The VM did not return a model catalog"),
                        }
                    });
                    if mode == ModelMode::OpenAiCompatible {
                        ui.label("Pluto uses the saved local model when the configured cloud options fail.");
                        if let Some(fallbacks) = &mut self.draft.model_fallbacks {
                            fallback_form(ui, fallbacks, &mut self.fallback_check, |ui, model| {
                                let mut result = Lookup::Idle;
                                let instance = instance.to_owned();
                                let model = model.clone();
                                result.start(runtime, ui.ctx(), async move {
                                    match query_settings(instance, Request::CheckFallbackModel { model }).await? {
                                        Response::FallbackModelMetadata { metadata } => Ok(metadata),
                                        _ => bail!("The VM did not return model metadata"),
                                    }
                                });
                                result
                            });
                        }
                    }
                });
        }
    }

    fn skills_ui(&mut self, ui: &mut egui::Ui) -> Result<Option<Request>> {
        let mut remove = None;
        for (name, skill) in &self.draft.skills {
            ui.push_id(name, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(name).on_hover_text(format!(
                        "{}\n{}",
                        skill.description,
                        skill.path.display()
                    ));
                    if ui.small_button("Remove").clicked() {
                        remove = Some(name.clone());
                    }
                });
            });
        }
        if let Some(name) = remove {
            self.draft.skills.remove(&name);
        }
        ui.horizontal_wrapped(|ui| {
            ui.radio_value(&mut self.import_from_mac, false, "Path in VM");
            ui.radio_value(&mut self.import_from_mac, true, "Import from Mac");
        });
        let (label, hint) = if self.import_from_mac {
            ("File on Mac", "/Users/you/skills/example/SKILL.md")
        } else {
            (
                "Skill path in VM",
                "/var/lib/pluto-workspace/example/SKILL.md",
            )
        };
        field(ui, label, &mut self.skill_path, hint);
        if self.import_from_mac {
            ui.weak("Imports SKILL.md. Put companion files and commands inside the VM.");
        }
        if ui
            .add_enabled(
                !self.skill_path.trim().is_empty(),
                egui::Button::new("Add skill"),
            )
            .clicked()
        {
            let path = self.skill_path.trim();
            return Ok(Some(if self.import_from_mac {
                Request::ImportSkill {
                    text: read_local_skill(path)?,
                }
            } else {
                Request::InspectSkill {
                    path: path.to_owned(),
                }
            }));
        }
        Ok(None)
    }

    fn mcp_ui(&mut self, ui: &mut egui::Ui) {
        let mut remove = None;
        for (name, server) in &self.draft.mcp {
            ui.push_id(name, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(name);
                    if ui.small_button("Edit").clicked() {
                        self.server_name = name.clone();
                        match server {
                            Mcp::Stdio { command, args } => {
                                self.server_http = false;
                                self.server_address = command.clone();
                                self.server_details = args.join("\n");
                            }
                            Mcp::Http { url, token_env } => {
                                self.server_http = true;
                                self.server_address = url.clone();
                                self.server_details = token_env.clone();
                            }
                        }
                    }
                    if ui.small_button("Remove").clicked() {
                        remove = Some(name.clone());
                    }
                });
            });
        }
        if let Some(name) = remove {
            self.draft.mcp.remove(&name);
        }
        if ui
            .horizontal_wrapped(|ui| {
                ui.radio_value(&mut self.server_http, false, "Local process (stdio)")
                    .changed()
                    | ui.radio_value(&mut self.server_http, true, "Remote HTTP")
                        .changed()
            })
            .inner
        {
            self.server_address.clear();
            self.server_details.clear();
        }
        field(ui, "Server name", &mut self.server_name, "example");
        if self.server_http {
            field(
                ui,
                "MCP endpoint",
                &mut self.server_address,
                "https://example.com/mcp",
            );
            field(
                ui,
                "Bearer token environment variable",
                &mut self.server_details,
                "Optional · set inside the VM",
            );
        } else {
            field(
                ui,
                "Command in VM",
                &mut self.server_address,
                "/usr/local/bin/example",
            );
            ui.label("Arguments (one per line)");
            ui.text_edit_multiline(&mut self.server_details);
        }
        let name = self.server_name.trim();
        let address = self.server_address.trim();
        let existing = self.draft.mcp.contains_key(name);
        if ui
            .add_enabled(
                valid_name(name) && !address.is_empty(),
                egui::Button::new(if existing {
                    "Update MCP server"
                } else {
                    "Add MCP server"
                }),
            )
            .clicked()
        {
            let server = if self.server_http {
                Mcp::Http {
                    url: address.into(),
                    token_env: self.server_details.trim().into(),
                }
            } else {
                Mcp::Stdio {
                    command: address.into(),
                    args: self.server_details.lines().map(str::to_owned).collect(),
                }
            };
            self.draft.mcp.insert(name.to_owned(), server);
            self.server_name.clear();
            self.server_address.clear();
            self.server_details.clear();
        }
    }
}

fn read_local_skill(path: &str) -> Result<String> {
    let mut path = PathBuf::from(path);
    if path.is_dir() {
        path.push("SKILL.md");
    }
    ensure!(
        path.file_name().is_some_and(|name| name == "SKILL.md"),
        "Select a skill folder or SKILL.md"
    );
    ensure!(path.is_file(), "Could not find SKILL.md on the Mac");
    let mut text = String::new();
    File::open(&path)
        .context("Could not open the skill on the Mac")?
        .take(32 * 1024 + 1)
        .read_to_string(&mut text)?;
    ensure!(
        text.len() <= 32 * 1024,
        "Import supports SKILL.md files up to 32 KiB. Use a VM path for larger skills."
    );
    Ok(text)
}
