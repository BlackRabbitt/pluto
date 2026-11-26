use super::{
    Lookup, egui, field,
    mcp_settings::{McpAction, McpEditor},
    model_settings::{FallbackCheck, ModelCatalog, fallback_form},
    saved_approval_list,
    settings_tabs::SettingsPage,
    vm_client::query_vm_api,
};
use anyhow::{Context, Result, bail, ensure};
use pluto::{
    api::{Request, Response},
    tools::{ApprovalRule, Mcp, ModelMode, Skill, VmSettings},
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
    mcp: McpEditor,
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
            mcp: McpEditor::default(),
        }
    }

    pub(super) fn replace_settings(&mut self, settings: VmSettings) {
        let page = self.page;
        *self = Self::new(settings);
        self.page = page;
    }

    pub(super) fn tool_counts(&self) -> (usize, usize) {
        (self.saved.skills.len(), self.saved.mcp.len())
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
        approvals: &mut Lookup<Vec<ApprovalRule>>,
        supports_saved_approvals: bool,
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
            SettingsPage::Mcp => self.mcp_ui(ui, runtime, instance),
            SettingsPage::Approvals => {
                ui.checkbox(
                    &mut self.draft.auto_approve_tools,
                    "Auto-approve tool calls (at your own risk)",
                ).on_hover_text("After saving, new messages can run terminal commands and MCP tools without asking.");
                if supports_saved_approvals {
                    if matches!(approvals, Lookup::Idle) {
                        let instance = instance.to_owned();
                        approvals.start(runtime, ui.ctx(), async move {
                            match query_vm_api(instance, Request::SavedApprovals).await? {
                                Response::SavedApprovals { rules } => Ok(rules),
                                _ => bail!("Unexpected saved approvals response"),
                            }
                        });
                    }
                    match approvals {
                        Lookup::Complete(Ok(rules)) => {
                            if let Some(rule) = saved_approval_list(ui, rules.iter()) {
                                request = Some(Request::ForgetApproval { rule });
                            }
                        }
                        Lookup::Complete(Err(error)) => {
                            let retry = ui
                                .small_button("Retry loading saved approvals")
                                .on_hover_text(format!("{error:#}"))
                                .clicked();
                            if retry {
                                *approvals = Lookup::Idle;
                            }
                        }
                        Lookup::Loading(_) => {
                            ui.weak("Loading saved approvals…");
                        }
                        Lookup::Idle => {}
                    }
                }
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
                        match query_vm_api(catalog_instance, Request::ListModels { mode, profile }).await? {
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
                                    match query_vm_api(instance, Request::CheckFallbackModel { model }).await? {
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
        ui.label("Terminal working directory");
        ui.weak("/var/lib/pluto-workspace");
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

    fn mcp_ui(&mut self, ui: &mut egui::Ui, runtime: &Runtime, instance: &str) {
        match self.mcp.show(ui, &self.draft.mcp, true) {
            Some(McpAction::Test(name, server)) => {
                self.start_mcp_test(ui, runtime, instance, name, server)
            }
            Some(McpAction::Remove(name)) => {
                self.draft.mcp.remove(&name);
            }
            Some(McpAction::Save(name, server)) => {
                self.mcp.applied(&name, &server);
                self.draft.mcp.insert(name, server);
            }
            None => {}
        }
    }

    fn start_mcp_test(
        &mut self,
        ui: &egui::Ui,
        runtime: &Runtime,
        instance: &str,
        name: Option<String>,
        server: Mcp,
    ) {
        let instance = instance.to_owned();
        let target = server.clone();
        self.mcp
            .start_test(runtime, ui.ctx(), name, server, async move {
                match query_vm_api(instance, Request::TestMcp { server: target }).await? {
                    Response::McpTools { names } => Ok(names),
                    _ => bail!("The VM did not return an MCP tool list"),
                }
            });
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

#[cfg(test)]
mod tests {
    use super::*;
    use pluto::tools::Config;

    #[test]
    fn vm_mcp_settings_show_draft_and_saved_tests_with_tool_names() -> Result<()> {
        let context = egui::Context::default();
        let runtime = tokio::runtime::Builder::new_current_thread().build()?;
        let server = Mcp::Http {
            url: "http://127.0.0.1:54321/mcp".into(),
            token_env: String::new(),
        };
        let mut form = VmSettingsForm::new(Config::default().into());
        form.page = SettingsPage::Mcp;
        form.draft.mcp.insert("fixture".into(), server.clone());
        form.mcp.test = Some(super::super::mcp_settings::McpConnectionTest {
            name: Some("fixture".into()),
            server,
            result: Lookup::Complete(Ok(vec!["lookup".into()])),
        });
        let output = context.run(Default::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                form.show(ui, &runtime, "pluto-test", &mut Lookup::Idle, false)
                    .unwrap();
            });
        });
        for label in [
            "Test",
            "Test connection",
            "Connected · 1 tool found",
            "lookup",
        ] {
            assert!(
                output.shapes.iter().any(|shape| matches!(
                    &shape.shape, egui::Shape::Text(text) if text.galley.text() == label
                )),
                "Missing {label}"
            );
        }
        Ok(())
    }
}
