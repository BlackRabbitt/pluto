use super::{Lookup, field};
use anyhow::Result;
use eframe::egui::{self, RichText};
use pluto::tools::{Mcp, valid_name};
use std::{collections::BTreeMap, future::Future};
use tokio::runtime::Runtime;

pub(super) enum McpAction {
    Save(String, Mcp),
    Remove(String),
    Test(Option<String>, Mcp),
}

pub(super) struct McpEditor {
    name: String,
    address: String,
    details: String,
    http: bool,
    pub(super) test: Option<McpConnectionTest>,
}

impl Default for McpEditor {
    fn default() -> Self {
        Self {
            name: String::new(),
            address: String::new(),
            details: String::new(),
            http: true,
            test: None,
        }
    }
}

impl McpEditor {
    pub(super) fn applied(&mut self, name: &str, server: &Mcp) {
        if let Some(test) = &mut self.test
            && test.name.is_none()
            && &test.server == server
        {
            test.name = Some(name.to_owned());
        }
        self.name.clear();
        self.address.clear();
        self.details.clear();
    }

    pub(super) fn start_test(
        &mut self,
        runtime: &Runtime,
        context: &egui::Context,
        name: Option<String>,
        server: Mcp,
        test: impl Future<Output = Result<Vec<String>>> + Send + 'static,
    ) {
        self.test = Some(McpConnectionTest::start(
            runtime, context, name, server, test,
        ));
    }

    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        servers: &BTreeMap<String, Mcp>,
        vm: bool,
    ) -> Option<McpAction> {
        let mut action = None;
        if let Some(test) = &mut self.test {
            test.result.poll();
        }
        let testing = self.test.as_ref().is_some_and(McpConnectionTest::running);
        let mut remove = None;
        let mut test_request = None;
        for (name, server) in servers {
            ui.push_id(name, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let target = match server {
                        Mcp::Http { url, .. } => url,
                        Mcp::Stdio { command, .. } => command,
                    };
                    ui.label(RichText::new(name).strong()).on_hover_text(target);
                    if ui
                        .add_enabled(!testing, egui::Button::new("Test").small())
                        .clicked()
                    {
                        test_request = Some((name.clone(), server.clone()));
                    }
                    if ui.small_button("Edit").clicked() {
                        self.name = name.clone();
                        match server {
                            Mcp::Stdio { command, args } => {
                                self.http = false;
                                self.address = command.clone();
                                self.details = args.join("\n");
                            }
                            Mcp::Http { url, token_env } => {
                                self.http = true;
                                self.address = url.clone();
                                self.details = token_env.clone();
                            }
                        }
                    }
                    if ui.small_button("Remove").clicked() {
                        remove = Some(name.clone());
                    }
                });
                if let Some(test) = &self.test {
                    test.show_status(ui, Some(name), server);
                }
            });
        }
        if let Some((name, server)) = test_request {
            action = Some(McpAction::Test(Some(name), server));
        }
        if let Some(name) = remove {
            action = Some(McpAction::Remove(name));
        }
        if ui
            .horizontal_wrapped(|ui| {
                ui.radio_value(&mut self.http, false, "Local process (stdio)")
                    .changed()
                    | ui.radio_value(&mut self.http, true, "Remote HTTP")
                        .changed()
            })
            .inner
        {
            self.address.clear();
            self.details.clear();
        }
        field(ui, "Server name", &mut self.name, "example");
        if self.http {
            field(
                ui,
                "MCP endpoint URL",
                &mut self.address,
                "https://example.com/mcp",
            );
            field(
                ui,
                "Bearer token environment variable",
                &mut self.details,
                if vm {
                    "Optional · set inside the VM"
                } else {
                    "Optional"
                },
            );
        } else {
            field(
                ui,
                "Launch command",
                &mut self.address,
                if vm {
                    "Executable inside the VM"
                } else {
                    "An executable on PATH"
                },
            );
            ui.label("Arguments (one per line)");
            ui.add(
                egui::TextEdit::multiline(&mut self.details)
                    .desired_width(f32::INFINITY)
                    .desired_rows(3),
            );
        }
        let name = self.name.trim().to_owned();
        let address = self.address.trim();
        let valid_target = !address.is_empty();
        let existing = servers.contains_key(&name);
        let server = if self.http {
            Mcp::Http {
                url: address.into(),
                token_env: self.details.trim().into(),
            }
        } else {
            Mcp::Stdio {
                command: address.into(),
                args: self
                    .details
                    .lines()
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned)
                    .collect(),
            }
        };
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    valid_name(&name) && valid_target,
                    egui::Button::new(if existing {
                        "Update MCP server"
                    } else {
                        "Add MCP server"
                    }),
                )
                .clicked()
            {
                action = Some(McpAction::Save(name, server.clone()));
            }
            if ui
                .add_enabled(
                    valid_target && !self.test.as_ref().is_some_and(McpConnectionTest::running),
                    egui::Button::new("Test connection"),
                )
                .clicked()
            {
                action = Some(McpAction::Test(None, server.clone()));
            }
        });
        if let Some(test) = &self.test {
            test.show_status(ui, None, &server);
        }
        action
    }
}

pub(super) struct McpConnectionTest {
    pub(super) name: Option<String>,
    pub(super) server: Mcp,
    pub(super) result: Lookup<Vec<String>>,
}

impl McpConnectionTest {
    fn start(
        runtime: &tokio::runtime::Runtime,
        context: &egui::Context,
        name: Option<String>,
        server: Mcp,
        test: impl Future<Output = Result<Vec<String>>> + Send + 'static,
    ) -> Self {
        let mut result = Lookup::Idle;
        result.start(runtime, context, test);
        Self {
            name,
            server,
            result,
        }
    }

    fn running(&self) -> bool {
        matches!(self.result, Lookup::Loading(_))
    }

    fn show_status(&self, ui: &mut egui::Ui, name: Option<&str>, server: &Mcp) {
        if self.name.as_deref() != name || &self.server != server {
            return;
        }
        match &self.result {
            Lookup::Idle => {}
            Lookup::Loading(_) => {
                ui.label("Testing connection…");
            }
            Lookup::Complete(Ok(tools)) => {
                let unit = if tools.len() == 1 { "tool" } else { "tools" };
                ui.label(format!("Connected · {} {unit} found", tools.len()));
                if !tools.is_empty() {
                    let names = tools[..tools.len().min(6)].join(", ");
                    ui.label(RichText::new(names).weak());
                }
            }
            Lookup::Complete(Err(error)) => {
                ui.label(
                    RichText::new(format!("Connection failed: {error:#}"))
                        .color(ui.visuals().warn_fg_color),
                );
            }
        }
    }
}
