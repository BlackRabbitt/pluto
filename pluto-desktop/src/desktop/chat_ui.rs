use super::{
    ApprovalDecision, ApprovalRequest, ApprovalRule, CommonMarkCache, Lookup, chat_message,
};
use eframe::egui::{self, Frame, Margin, RichText, TextEdit};
use pluto::{
    agent::{TokenUsage, TurnSelection},
    history::ChatHistory,
    providers::{ThinkingLevel, ThinkingSetting},
    tools::McpToolId,
};
use std::collections::{BTreeMap, BTreeSet};

const CHAT_DRAFT_HEIGHT: f32 = 80.0;
const CHAT_PICKER_HEIGHT: f32 = 155.0;
const CHAT_SELECTION_ROWS: usize = 3;

pub(super) struct PickerCatalog {
    pub(super) skills: Vec<String>,
    pub(super) tools: Vec<McpToolId>,
    pub(super) warnings: Vec<String>,
}

#[derive(Clone)]
pub(super) enum PickerChoice {
    Skill(String),
    Mcp(McpToolId),
}

impl PickerChoice {
    fn label(&self) -> String {
        match self {
            Self::Skill(name) => name.clone(),
            Self::Mcp(tool) => format!("{} / {}", tool.server, tool.name),
        }
    }
}

#[derive(Default)]
pub(super) struct ChatComposer {
    pub(super) prompt: String,
    pub(super) selection: TurnSelection,
    pub(super) picker_catalog: Lookup<PickerCatalog>,
    picker_index: usize,
}

impl ChatComposer {
    pub(super) fn picker_query(&self, enabled: bool) -> Option<&str> {
        if !enabled {
            return None;
        }
        self.prompt
            .strip_prefix('/')
            .filter(|query| !query.contains(['/', '\n', '\r']))
    }

    pub(super) fn picker_choices<'a>(
        &'a self,
        skills: impl Iterator<Item = &'a String>,
        query: &str,
    ) -> Vec<PickerChoice> {
        let query = query.to_lowercase();
        let matches = |label: &str| {
            let label = label.to_lowercase();
            query.split_whitespace().all(|word| label.contains(word))
        };
        let remote_skills = match &self.picker_catalog {
            Lookup::Complete(Ok(catalog)) => catalog.skills.as_slice(),
            _ => &[],
        };
        let mut choices: Vec<_> = skills
            .chain(remote_skills.iter())
            .filter(|name| matches(name) && !self.selection.skills.contains(name))
            .cloned()
            .map(PickerChoice::Skill)
            .collect();
        if let Lookup::Complete(Ok(catalog)) = &self.picker_catalog {
            choices.extend(
                catalog
                    .tools
                    .iter()
                    .filter(|tool| !self.selection.mcp.contains(tool))
                    .cloned()
                    .map(PickerChoice::Mcp)
                    .filter(|selection| matches(&selection.label())),
            );
        }
        choices
    }

    fn picker_ui(
        &mut self,
        skills: &[String],
        ui: &mut egui::Ui,
        input: egui::Id,
        previous: bool,
        next: bool,
        select: bool,
    ) {
        let choices =
            self.picker_choices(skills.iter(), self.picker_query(true).unwrap_or_default());
        if previous {
            self.picker_index = self.picker_index.saturating_sub(1);
        } else if next {
            self.picker_index = self.picker_index.saturating_add(1);
        }
        self.picker_index = self.picker_index.min(choices.len().saturating_sub(1));
        let mut picked = if select {
            choices.get(self.picker_index).cloned()
        } else {
            None
        };
        egui::ScrollArea::vertical()
            .id_salt("composer_picker")
            .max_height(CHAT_PICKER_HEIGHT)
            .show(ui, |ui| {
                let mut current_group = "";
                for (index, choice) in choices.iter().enumerate() {
                    let group = match choice {
                        PickerChoice::Skill(_) => "Skills",
                        PickerChoice::Mcp(_) => "MCP tools",
                    };
                    if group != current_group {
                        ui.label(RichText::new(group).small().strong());
                        current_group = group;
                    }
                    let selected = index == self.picker_index;
                    let row = ui.selectable_label(selected, choice.label());
                    if row.clicked() {
                        picked = Some(choice.clone());
                    }
                    if selected && (previous || next) {
                        row.scroll_to_me(None);
                    }
                }
                match &self.picker_catalog {
                    Lookup::Loading(_) => {
                        ui.label("Looking up MCP tools…");
                    }
                    Lookup::Complete(Ok(catalog)) => {
                        for warning in &catalog.warnings {
                            ui.label(
                                RichText::new(warning)
                                    .small()
                                    .color(ui.visuals().warn_fg_color),
                            );
                        }
                    }
                    Lookup::Complete(Err(error)) => {
                        ui.label(
                            RichText::new(format!("{error:#}"))
                                .small()
                                .color(ui.visuals().warn_fg_color),
                        );
                    }
                    Lookup::Idle => {}
                }
                if choices.is_empty() && !matches!(self.picker_catalog, Lookup::Loading(_)) {
                    ui.label("No matching skills or MCP tools");
                }
            });
        if let Some(choice) = picked {
            match choice {
                PickerChoice::Skill(name) => self.selection.skills.push(name),
                PickerChoice::Mcp(tool) => self.selection.mcp.push(tool),
            }
            self.prompt.clear();
            self.picker_index = 0;
            ui.memory_mut(|memory| memory.request_focus(input));
        }
    }

    fn selection_height(&self, ui: &egui::Ui) -> f32 {
        let count = self.selection.skills.len() + self.selection.mcp.len();
        let rows = count.min(CHAT_SELECTION_ROWS);
        rows as f32 * (ui.spacing().interact_size.y + ui.spacing().item_spacing.y)
    }

    pub(super) fn panel(
        &mut self,
        ui: &mut egui::Ui,
        skills: &[String],
        enabled: bool,
        archived: bool,
        picker_enabled: bool,
    ) -> bool {
        let spacing = ui.spacing().item_spacing.y;
        let row = ui.spacing().interact_size.y;
        let height = if archived {
            row + spacing
        } else {
            let actions = if self.picker_query(enabled && picker_enabled).is_some() {
                CHAT_PICKER_HEIGHT
            } else {
                row
            };
            self.selection_height(ui) + CHAT_DRAFT_HEIGHT + actions + 2.0 * spacing
        };
        egui::TopBottomPanel::bottom("chat_composer_panel")
            .exact_height(height)
            .show_separator_line(false)
            .frame(Frame::new().inner_margin(Margin {
                top: spacing as i8,
                ..Margin::ZERO
            }))
            .show_inside(ui, |ui| {
                self.show(ui, skills, enabled, archived, picker_enabled)
            })
            .inner
    }

    fn show(
        &mut self,
        ui: &mut egui::Ui,
        skills: &[String],
        enabled: bool,
        archived: bool,
        picker_enabled: bool,
    ) -> bool {
        if archived {
            ui.label(RichText::new("Archived chat · Restore it to continue").weak());
            return false;
        }
        let input_id = ui.make_persistent_id("chat_composer");
        let selection_height = self.selection_height(ui);
        if selection_height > 0.0 {
            let removed = egui::ScrollArea::vertical()
                .id_salt("composer_selection")
                .max_height(selection_height - ui.spacing().item_spacing.y)
                .min_scrolled_height(0.0)
                .show(ui, |ui| {
                    let mut removed = false;
                    let mut keep_row = |label: &str| {
                        let remove = ui
                            .horizontal(|ui| {
                                let remove = ui
                                    .small_button("×")
                                    .on_hover_text(format!("Remove {label}"))
                                    .clicked();
                                ui.add(egui::Label::new(label).truncate());
                                remove
                            })
                            .inner;
                        removed |= remove;
                        !remove
                    };
                    self.selection.skills.retain(|name| keep_row(name));
                    self.selection
                        .mcp
                        .retain(|tool| keep_row(&format!("{} / {}", tool.server, tool.name)));
                    removed
                })
                .inner;
            if removed {
                ui.memory_mut(|memory| memory.request_focus(input_id));
            }
        }
        let mut keys = (false, false, false, false);
        if self.picker_query(enabled && picker_enabled).is_some()
            && ui.memory(|memory| memory.has_focus(input_id))
        {
            keys = ui.input_mut(|input| {
                (
                    input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                    input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                    input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                    input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                )
            });
        }
        let (previous, next, select, cancel) = keys;
        if cancel {
            self.prompt.clear();
        }
        let input = egui::ScrollArea::vertical()
            .id_salt("composer_draft")
            .max_height(CHAT_DRAFT_HEIGHT)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_enabled(
                    enabled,
                    TextEdit::multiline(&mut self.prompt)
                        .id(input_id)
                        .hint_text(if picker_enabled {
                            "Message Pluto or type / to pick a skill or tool"
                        } else {
                            "Message Pluto"
                        })
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                )
            })
            .inner;
        if input.changed() {
            self.picker_index = 0;
        }
        if self.picker_query(enabled && picker_enabled).is_some() {
            self.picker_ui(skills, ui, input_id, previous, next, select);
            return false;
        }
        if self.selection.skills.is_empty() && self.selection.mcp.is_empty() {
            self.picker_catalog = Lookup::Idle;
        }
        let keyboard = input.has_focus()
            && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
        ui.add_enabled(enabled, egui::Button::new("Send")).clicked() || (enabled && keyboard)
    }
}

pub(super) fn chat_transcript(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash,
    history: &ChatHistory,
    markdown: &mut CommonMarkCache,
    warnings: &[String],
    loaded_skills: &BTreeMap<usize, BTreeSet<String>>,
    show_empty: bool,
    status: impl FnOnce(&mut egui::Ui),
) {
    egui::ScrollArea::vertical()
        .id_salt(&id)
        .stick_to_bottom(true)
        .min_scrolled_height(0.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for warning in warnings {
                ui.label(RichText::new(warning).color(ui.visuals().warn_fg_color));
            }
            if show_empty && history.messages().is_empty() {
                ui.add_space(52.0);
                ui.label(RichText::new("What would you like to do?").size(25.0));
            }
            let column_width = ui.available_width().min(ui.clip_rect().width());
            for (index, (role, content)) in history.messages().iter().enumerate() {
                chat_message(
                    ui,
                    (&id, index),
                    column_width,
                    role,
                    content,
                    history.timestamp(index),
                    markdown,
                );
                if let Some(skills) = loaded_skills.get(&index) {
                    for name in skills {
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("Skill loaded · {name}"))
                                    .small()
                                    .color(ui.visuals().hyperlink_color),
                            )
                            .wrap(),
                        );
                    }
                }
            }
            status(ui);
        });
}

pub(super) fn chat_title(ui: &mut egui::Ui, id: Option<&str>) {
    ui.heading("Chat");
    if let Some(id) = id
        && ui
            .small_button("Copy chat ID")
            .on_hover_text("Use the chat ID to find its diagnostic events")
            .clicked()
    {
        ui.ctx().copy_text(id.to_owned());
    }
}

pub(super) fn thinking_picker(
    ui: &mut egui::Ui,
    levels: &[ThinkingLevel],
    setting: &mut Option<ThinkingSetting>,
    enabled: bool,
) {
    let label = levels
        .iter()
        .find(|level| setting.as_ref() == Some(&level.setting))
        .map_or("Server default", |level| level.label.as_str());
    ui.label("Thinking");
    ui.add_enabled_ui(enabled, |ui| {
        egui::ComboBox::from_id_salt("thinking_level")
            .selected_text(label)
            .show_ui(ui, |ui| {
                ui.selectable_value(setting, None, "Server default");
                for level in levels {
                    ui.selectable_value(setting, Some(level.setting.clone()), &level.label);
                }
            });
    });
}

pub(super) fn context_usage(
    ui: &mut egui::Ui,
    input_only: bool,
    window: Option<u64>,
    usage: Option<TokenUsage>,
) {
    let units = if input_only { "input tokens" } else { "tokens" };
    let Some(usage) = usage else {
        if let Some(window) = window {
            ui.label(RichText::new(format!("Context window · {window} {units}")).small());
        }
        return;
    };
    let details = format!(
        "Last model request\nInput: {} tokens\nOutput: {} tokens",
        usage.input, usage.output
    );
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Context").small().strong());
        if let Some(window) = window {
            let used = if input_only {
                usage.input
            } else {
                usage.total()
            };
            let fraction = used as f64 / window as f64;
            let fill = if fraction >= 0.9 {
                ui.visuals().warn_fg_color
            } else {
                ui.visuals().hyperlink_color
            };
            ui.add(
                egui::ProgressBar::new(fraction.min(1.0) as f32)
                    .desired_width(112.0)
                    .fill(fill),
            )
            .on_hover_text(&details);
            ui.label(RichText::new(format!("{used} / {window} {units}")).small())
                .on_hover_text(&details);
        } else {
            ui.label(RichText::new(format!("{} tokens · last request", usage.total())).small())
                .on_hover_text(format!("{details}\nContext limit unavailable"));
        }
    });
}

pub(super) fn chat_list<'a, Id: PartialEq>(
    ui: &mut egui::Ui,
    recent: impl ExactSizeIterator<Item = (&'a Id, &'a str)>,
    archived: impl ExactSizeIterator<Item = (&'a Id, &'a str)>,
    active: Option<&Id>,
    enabled: bool,
) -> Option<&'a Id> {
    if recent.len() == 0 && archived.len() == 0 {
        return None;
    }
    ui.separator();
    let max_height = (ui.available_height() - 42.0).max(0.0);
    let mut selected = None;
    let mut row = |ui: &mut egui::Ui, id, title| {
        if ui
            .add(egui::Button::selectable(active == Some(id), title).truncate())
            .on_hover_text(title)
            .clicked()
        {
            selected = Some(id);
        }
    };
    egui::ScrollArea::vertical()
        .max_height(max_height)
        .show(ui, |ui| {
            ui.add_enabled_ui(enabled, |ui| {
                if recent.len() != 0 {
                    ui.label(RichText::new("Recent chats").weak().small());
                    for (id, title) in recent {
                        row(ui, id, title);
                    }
                }
                if archived.len() != 0 {
                    egui::CollapsingHeader::new("Archived chats").show(ui, |ui| {
                        for (id, title) in archived {
                            row(ui, id, title);
                        }
                    });
                }
            });
        });
    selected
}

pub(super) fn tool_approval_dialog(
    context: &egui::Context,
    request: &ApprovalRequest,
    allow_other_operands: &mut bool,
    error: Option<&str>,
) -> (Option<ApprovalDecision>, Option<ApprovalRule>) {
    let mut decision = None;
    let mut remember = None;
    egui::Modal::new(egui::Id::new("approval")).show(context, |ui| {
            ui.set_width(500.0);
            ui.heading("Approve tool call");
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(&request.description).monospace())
                            .wrap()
                            .selectable(true),
                    );
                });
            let scope = request.rule.as_ref().map(|rule| {
                let ApprovalRule::Terminal(exact) = rule else {
                    ui.label(RichText::new(
                        "Run always approves this tool on this server, including calls with different arguments."
                    ).small());
                    return rule.clone();
                };
                let similar = exact.allow_other_operands();
                if similar.is_some() {
                    ui.checkbox(
                        allow_other_operands,
                        "Allow different values after --",
                    );
                }
                let selected = match similar.filter(|_| *allow_other_operands) {
                    Some(rule) => {
                        ui.label(RichText::new(rule.label()).monospace().small());
                        rule
                    }
                    None => {
                        ui.label(
                            RichText::new(
                                "Run always saves this exact command and working directory.",
                            )
                            .small(),
                        );
                        exact.clone()
                    }
                };
                ApprovalRule::Terminal(selected)
            });
            if let Some(error) = error {
                ui.label(RichText::new(error).color(ui.visuals().warn_fg_color));
            }
            ui.horizontal(|ui| {
                if ui.button("Decline").clicked() {
                    decision = Some(ApprovalDecision::Decline);
                }
                if ui.button("Run once").clicked() {
                    decision = Some(ApprovalDecision::RunOnce);
                }
                if let Some(rule) = scope
                    && ui.button("Run always").clicked()
                {
                    remember = Some(rule);
                }
            });
        });
    (decision, remember)
}

pub(super) fn saved_approval_list<'a>(
    ui: &mut egui::Ui,
    rules: impl Iterator<Item = &'a ApprovalRule>,
) -> Option<ApprovalRule> {
    let mut rules = rules.peekable();
    rules.peek()?;
    let mut forget = None;
    egui::CollapsingHeader::new("Saved approvals").show(ui, |ui| {
        for rule in rules {
            ui.horizontal(|ui| {
                if ui.small_button("Forget").clicked() {
                    forget = Some(rule.clone());
                }
                ui.add(egui::Label::new(RichText::new(rule.label()).monospace()).wrap());
            });
        }
    });
    forget
}

pub(super) fn chat_progress(ui: &mut egui::Ui, status: &str) {
    ui.horizontal_top(|ui| {
        ui.spinner();
        ui.add(egui::Label::new(RichText::new(status).small().weak()).wrap());
    });
}

pub(super) fn chat_warning(ui: &mut egui::Ui, status: &str) {
    ui.add(
        egui::Label::new(
            RichText::new(status)
                .small()
                .color(ui.visuals().warn_fg_color),
        )
        .wrap(),
    );
}

pub(super) fn model_retry_ui(ui: &mut egui::Ui, status: &str) -> Option<bool> {
    chat_warning(ui, status);
    ui.horizontal_wrapped(|ui| {
        let retry = ui.button("Retry model call").clicked();
        let end = ui.button("End turn").clicked();
        (retry || end).then_some(retry)
    })
    .inner
}
