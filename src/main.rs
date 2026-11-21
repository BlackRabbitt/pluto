mod agent;
mod history;
mod providers;
mod tools;

#[cfg(all(test, unix))]
#[path = "../tests/support/chat.rs"]
mod chat_test;

use agent::{ApprovalDecision, Event, TokenUsage, TurnSelection};
use anyhow::{Context, Result};
use eframe::egui::{self, Color32, Frame, Margin, RichText, Stroke, TextEdit};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use history::{ChatEntry, ChatStore, Messages};
use providers::{Connection, ModelCapabilities, Provider, ThinkingSetting};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};
use tokio::sync::oneshot;
use tools::{ApprovalRequest, ApprovalRule, Config, Mcp, McpToolId, ToolSet};

const APP_NAME: &str = "Pluto";

fn configure_fonts(context: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    // Updated monochrome Noto Emoji from Google Fonts. See the bundled OFL.
    let name = "Noto Emoji".to_owned();
    let fallback = egui::FontData::from_static(include_bytes!("../assets/fonts/NotoEmoji.ttf"))
        .tweak(egui::FontTweak {
            scale: 0.9,
            ..Default::default()
        });
    fonts.font_data.insert(name.clone(), fallback.into());
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push(name.clone());
    }
    context.set_fonts(fonts);
}

fn configure_theme(context: &egui::Context) {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let (mut visuals, canvas, surface, text, muted, accent, selection, border, warning) =
            match theme {
                egui::Theme::Light => (
                    egui::Visuals::light(),
                    0xEEF5EF,
                    0xFAFCFA,
                    0x21352A,
                    0x465E4D,
                    0x175D3B,
                    0xB8DFC6,
                    0xB4C9B8,
                    0x874617,
                ),
                egui::Theme::Dark => (
                    egui::Visuals::dark(),
                    0x14221B,
                    0x1C2D23,
                    0xE1ECE4,
                    0xA9C2B0,
                    0xA2DAB5,
                    0x355F47,
                    0x486454,
                    0xF2C98B,
                ),
            };
        let color = |rgb: u32| Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8);

        visuals.panel_fill = color(canvas);
        visuals.window_fill = color(surface);
        visuals.window_stroke = Stroke::new(1.0_f32, color(border));
        visuals.text_edit_bg_color = Some(color(surface));
        visuals.override_text_color = Some(color(text));
        visuals.weak_text_color = Some(color(muted));
        visuals.hyperlink_color = color(accent);
        visuals.warn_fg_color = color(warning);
        visuals.selection.bg_fill = color(selection);
        visuals.selection.stroke = Stroke::new(1.0_f32, color(text));
        visuals.text_cursor.stroke = Stroke::new(2.0_f32, color(accent));
        context.set_visuals_of(theme, visuals);
    }

    context.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(10.0, 10.0);
        style.spacing.interact_size.y = 28.0;
        for (text_style, font) in &mut style.text_styles {
            font.size = match text_style {
                egui::TextStyle::Small => 14.0,
                egui::TextStyle::Heading => 23.0,
                _ => 17.0,
            };
        }
    });
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 690.0])
            .with_min_inner_size([700.0, 530.0]),
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(|cc| {
            configure_fonts(&cc.egui_ctx);
            configure_theme(&cc.egui_ctx);
            Ok(Box::new(Desktop::new()?))
        }),
    )
}

fn chat_message(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash,
    width: f32,
    role: &str,
    content: &str,
    markdown_cache: &mut CommonMarkCache,
) -> egui::Response {
    let assistant = role == "assistant";
    let background = if assistant {
        if ui.visuals().dark_mode {
            Color32::from_rgb(33, 60, 48)
        } else {
            Color32::from_rgb(224, 240, 231)
        }
    } else {
        ui.visuals().window_fill
    };
    Frame::new()
        .fill(background)
        .stroke(ui.visuals().window_stroke)
        .corner_radius(9)
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            // Bound the inner width to the visible column before any Markdown expands it.
            let inner_width = (width - 28.0 - 2.0 * ui.visuals().window_stroke.width).max(1.0);
            ui.set_width(inner_width);
            ui.label(
                RichText::new(if role == "user" { "You" } else { APP_NAME })
                    .strong()
                    .color(ui.visuals().hyperlink_color),
            );
            if assistant {
                ui.style_mut().spacing.scroll = egui::style::ScrollStyle::solid();
                egui::ScrollArea::horizontal()
                    .id_salt(("reply", id_salt))
                    .max_width(inner_width)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.set_width(inner_width);
                        ui.style_mut().url_in_tooltip = true;
                        CommonMarkViewer::new().show(ui, markdown_cache, content);
                    });
            } else {
                ui.add(egui::Label::new(content).wrap().selectable(true));
            }
        })
        .response
}

#[cfg(test)]
mod chat_layout_tests {
    use super::*;

    #[test]
    fn wide_table_cells_keep_their_width_and_scroll() {
        let context = egui::Context::default();
        configure_fonts(&context);
        configure_theme(&context);
        let mut cache = CommonMarkCache::default();
        let cell = "Wide table content that needs more room than the chat column provides";
        let table = format!("| Name | Value |\n| --- | --- |\n| {cell} | Second column |");
        let mut render = |events| {
            context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(300.0, 420.0),
                    )),
                    events,
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            let width = ui.available_width();
                            let reply = chat_message(ui, 0, width, "assistant", &table, &mut cache);
                            assert!(reply.rect.width() <= width + 1.0);
                        });
                    });
                },
            )
        };
        let find_cell = |output: egui::FullOutput| {
            output
                .shapes
                .into_iter()
                .find_map(|shape| match shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == cell => {
                        Some((text, shape.clip_rect))
                    }
                    _ => None,
                })
                .expect("The table cell should be painted")
        };
        render(Vec::new());
        let (before, viewport) = find_cell(render(Vec::new()));
        assert_eq!(
            before.galley.rows.len(),
            1,
            "The cell should stay on one line"
        );
        assert!(before.galley.size().x > viewport.width());
        render(vec![
            egui::Event::PointerMoved(before.pos + egui::vec2(8.0, 8.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(-90.0, 0.0),
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        let (after, _) = find_cell(render(Vec::new()));
        assert!(after.pos.x < before.pos.x);
    }

    #[test]
    fn long_replies_fit_a_narrow_chat_column_and_render_newer_emoji() {
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            let context = egui::Context::default();
            configure_fonts(&context);
            configure_theme(&context);
            context.set_theme(theme);
            context.options_mut(|options| options.warn_on_id_clash = true);
            let mut cache = CommonMarkCache::default();
            let output = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(300.0, 720.0),
                    )),
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        let width = ui.available_width();
                        let paragraph = format!("{} 🫨", "Long replies should wrap. ".repeat(20));
                        let code = format!("```text\n{}\n```", "x".repeat(180));
                        let table = "| A | B | C | D |\n| --- | --- | --- | --- |\n| Wide content | Wide content | Wide content | Wide content |";
                        let reply = chat_message(ui, 0, width, "assistant", &paragraph, &mut cache);
                        assert!(reply.rect.height() > 100.0);
                        assert!(reply.rect.width() <= width + 1.0);
                        let reply = chat_message(ui, 1, width, "assistant", &code, &mut cache);
                        assert!(reply.rect.height() > 80.0);
                        assert!(reply.rect.width() <= width + 1.0);
                        let reply = chat_message(ui, 2, width, "assistant", table, &mut cache);
                        assert!(reply.rect.width() <= width + 1.0);
                        assert!(ui.fonts(|fonts| fonts.has_glyph(&egui::FontId::proportional(17.0), '🫨')));
                    });
                },
            );
            for shape in output.shapes {
                if let egui::Shape::Text(text) = shape.shape {
                    assert!(
                        !text.galley.text().contains("use of ScrollArea ID"),
                        "{}",
                        text.galley.text()
                    );
                }
            }
        }
    }
}

#[derive(Default)]
enum Lookup<T> {
    #[default]
    Idle,
    Loading(oneshot::Receiver<Result<T>>),
    Complete(Result<T>),
}

impl<T: Send + 'static> Lookup<T> {
    fn start(
        &mut self,
        runtime: &tokio::runtime::Runtime,
        context: &egui::Context,
        request: impl Future<Output = Result<T>> + Send + 'static,
    ) {
        let (mut sender, receiver) = oneshot::channel();
        let context = context.clone();
        runtime.spawn(async move {
            tokio::select! {
                _ = sender.closed() => {}
                result = request => {
                    let _ = sender.send(result);
                    context.request_repaint();
                }
            }
        });
        *self = Self::Loading(receiver);
    }

    fn poll(&mut self) {
        if let Self::Loading(receiver) = self {
            match receiver.try_recv() {
                Ok(result) => *self = Self::Complete(result),
                Err(oneshot::error::TryRecvError::Closed) => {
                    *self = Self::Complete(Err(anyhow::anyhow!("Model lookup ended unexpectedly")));
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
            }
        }
    }
}

struct PendingApproval {
    request: ApprovalRequest,
    reply: oneshot::Sender<ApprovalDecision>,
    allow_other_operands: bool,
    error: Option<String>,
}

struct McpConnectionTest {
    name: Option<String>,
    server: Mcp,
    result: Lookup<Vec<String>>,
}

struct PickerCatalog {
    tools: Vec<McpToolId>,
    warnings: Vec<String>,
}

struct Desktop {
    runtime: tokio::runtime::Runtime,
    events: mpsc::Receiver<Event>,
    sender: mpsc::Sender<Event>,
    config: Config,
    discovery_client: reqwest::Client,
    discovery_connection: Option<Connection>,
    discovery_model: String,
    models: Lookup<Vec<String>>,
    capabilities: Lookup<ModelCapabilities>,
    thinking_setting: Option<ThinkingSetting>,
    context_usage: Option<TokenUsage>,
    history: Messages,
    markdown_cache: CommonMarkCache,
    chat_store: Option<ChatStore>,
    saved_chats: Vec<ChatEntry>,
    archived_chats: Vec<ChatEntry>,
    active_chat: Option<PathBuf>,
    chat_dirty: bool,
    history_notice: Option<String>,
    prompt: String,
    selection: Option<TurnSelection>,
    picker_catalog: Lookup<PickerCatalog>,
    picker_index: usize,
    settings: bool,
    busy: bool,
    status: String,
    loaded_skills: BTreeMap<usize, BTreeSet<String>>,
    notice: String,
    connection_errors: Vec<String>,
    config_error: bool,
    approval: Option<PendingApproval>,
    workspace: String,
    skill_path: String,
    mcp_name: String,
    mcp_http: bool,
    mcp_address: String,
    mcp_details: String,
    mcp_notice: Option<Result<String>>,
    mcp_test: Option<McpConnectionTest>,
}

impl Desktop {
    fn new() -> anyhow::Result<Self> {
        let (sender, events) = mpsc::channel();
        let (config, notice) = match Config::load() {
            Ok(config) => (config, String::new()),
            Err(error) => (
                Config::default(),
                format!("{error:#}. Fix the file and relaunch before saving settings."),
            ),
        };
        let (chat_store, mut history_notice) = match Config::path() {
            Ok(path) => (Some(ChatStore::beside(&path)), None),
            Err(error) => (None, Some(format!("Chat history unavailable: {error:#}"))),
        };
        let mut saved_chats = Vec::new();
        let mut archived_chats = Vec::new();
        let mut restored = None;
        if let Some(store) = &chat_store {
            match store.entries() {
                Ok((entries, skipped)) => {
                    saved_chats = entries;
                    if skipped > 0 {
                        history_notice =
                            Some(format!("Could not read {skipped} saved chat file(s)."));
                    }
                    restored = saved_chats
                        .iter()
                        .find_map(|entry| match store.read(&entry.path) {
                            Ok(messages) => Some((entry.path.clone(), messages)),
                            Err(error) => {
                                history_notice = Some(format!("{error:#}"));
                                None
                            }
                        });
                }
                Err(error) => history_notice = Some(format!("{error:#}")),
            }
            match store.archived_entries() {
                Ok((entries, skipped)) => {
                    archived_chats = entries;
                    if skipped > 0 {
                        history_notice =
                            Some(format!("Could not read {skipped} archived chat file(s)."));
                    }
                }
                Err(error) => {
                    history_notice = Some(format!("Could not load archived chats: {error:#}"))
                }
            }
        }
        let (active_chat, history) = match restored {
            Some((path, messages)) => (Some(path), messages),
            None => (None, Vec::new()),
        };
        Ok(Self {
            runtime: tokio::runtime::Runtime::new()?,
            settings: config.model_name.trim().is_empty() || config.model_connection().is_err(),
            workspace: config.workspace.display().to_string(),
            config_error: !notice.is_empty(),
            config,
            discovery_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()?,
            discovery_connection: None,
            discovery_model: String::new(),
            models: Lookup::Idle,
            capabilities: Lookup::Idle,
            thinking_setting: None,
            context_usage: None,
            events,
            sender,
            notice,
            history,
            markdown_cache: CommonMarkCache::default(),
            chat_store,
            saved_chats,
            archived_chats,
            active_chat,
            chat_dirty: false,
            history_notice,
            prompt: String::new(),
            selection: None,
            picker_catalog: Lookup::Idle,
            picker_index: 0,
            busy: false,
            status: String::new(),
            loaded_skills: BTreeMap::new(),
            connection_errors: Vec::new(),
            approval: None,
            skill_path: String::new(),
            mcp_name: String::new(),
            mcp_http: true,
            mcp_address: String::new(),
            mcp_details: String::new(),
            mcp_notice: None,
            mcp_test: None,
        })
    }

    fn send(&mut self, context: &egui::Context) {
        if self.busy || self.prompt.trim().is_empty() || self.active_chat_is_archived() {
            return;
        }
        if self.config.model_connection().is_err() || self.config.model_name.trim().is_empty() {
            self.settings = true;
            self.notice = "Choose a provider, enter its server URL, and select a model.".into();
            return;
        }
        let selection = self.selection.take();
        self.status = match &selection {
            Some(TurnSelection::Skill(name)) => format!("Preparing to load skill · {name}…"),
            _ => "Discovering tools…".into(),
        };
        self.connection_errors.clear();
        self.busy = true;
        self.history
            .push(("user".into(), std::mem::take(&mut self.prompt)));
        self.chat_dirty = true;
        self.persist_chat();
        let config = self.config.clone();
        let history = self.history.clone();
        let thinking = self.thinking_setting.clone();
        let sender = self.sender.clone();
        let context = context.clone();
        self.runtime.spawn(async move {
            let emit = |event| {
                let _ = sender.send(event);
                context.request_repaint();
            };
            let result = agent::chat(config, history, thinking, selection, emit).await;
            emit(Event::Finished(result));
        });
    }

    fn picker_query(&self) -> Option<&str> {
        if self.busy || self.selection.is_some() {
            return None;
        }
        self.prompt
            .strip_prefix('/')
            .filter(|query| !query.contains(['/', '\n', '\r']))
    }

    fn picker_choices(&self, query: &str) -> Vec<TurnSelection> {
        let query = query.to_lowercase();
        let matches = |label: &str| {
            let label = label.to_lowercase();
            query.split_whitespace().all(|word| label.contains(word))
        };
        let mut choices: Vec<_> = self
            .config
            .skills
            .keys()
            .filter(|name| matches(name))
            .cloned()
            .map(TurnSelection::Skill)
            .collect();
        if let Lookup::Complete(Ok(catalog)) = &self.picker_catalog {
            choices.extend(
                catalog
                    .tools
                    .iter()
                    .cloned()
                    .map(TurnSelection::Mcp)
                    .filter(|selection| matches(&selection.label())),
            );
        }
        choices
    }

    fn picker_ui(
        &mut self,
        ui: &mut egui::Ui,
        input: egui::Id,
        previous: bool,
        next: bool,
        select: bool,
    ) {
        if matches!(self.picker_catalog, Lookup::Idle) && !self.config.mcp.is_empty() {
            let config = self.config.clone();
            self.picker_catalog
                .start(&self.runtime, ui.ctx(), async move {
                    let mut warnings = Vec::new();
                    let tools = ToolSet::connect(&config, |warning| warnings.push(warning))
                        .await
                        .mcp_tool_choices();
                    Ok(PickerCatalog { tools, warnings })
                });
        }
        let choices = self.picker_choices(self.picker_query().unwrap_or_default());
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
            .max_height(155.0)
            .show(ui, |ui| {
                let mut current_group = "";
                for (index, choice) in choices.iter().enumerate() {
                    let group = match choice {
                        TurnSelection::Skill(_) => "Skills",
                        TurnSelection::Mcp(_) => "MCP tools",
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
        if let Some(selection) = picked {
            self.selection = Some(selection);
            self.prompt.clear();
            self.picker_catalog = Lookup::Idle;
            ui.memory_mut(|memory| memory.request_focus(input));
        }
    }

    fn composer_ui(&mut self, ui: &mut egui::Ui) {
        if self.active_chat_is_archived() {
            ui.label(RichText::new("Archived chat · Restore it to continue").weak());
            return;
        }
        let input_id = ui.make_persistent_id("chat_composer");
        if let Some(selection) = &self.selection {
            let clear = ui
                .horizontal(|ui| {
                    ui.label(format!("For this message · {}", selection.label()));
                    ui.small_button("Clear selection").clicked()
                })
                .inner;
            if clear {
                self.selection = None;
                ui.memory_mut(|memory| memory.request_focus(input_id));
            }
        }
        let mut keys = (false, false, false, false);
        if self.picker_query().is_some() && ui.memory(|memory| memory.has_focus(input_id)) {
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
        let input = ui.add_enabled(
            !self.busy,
            TextEdit::multiline(&mut self.prompt)
                .id(input_id)
                .hint_text("Message Pluto or type / to pick a tool")
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
        if input.changed() {
            self.picker_index = 0;
        }
        if self.picker_query().is_some() {
            self.picker_ui(ui, input_id, previous, next, select);
            return;
        }
        self.picker_catalog = Lookup::Idle;
        let keyboard = input.has_focus()
            && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
        if ui
            .add_enabled(!self.busy, egui::Button::new("Send"))
            .clicked()
            || keyboard
        {
            self.send(ui.ctx());
        }
    }

    fn persist_chat(&mut self) -> bool {
        if !self.chat_dirty {
            return true;
        }
        let Some(store) = &self.chat_store else {
            return false;
        };
        match store.save(self.active_chat.as_deref(), &self.history) {
            Ok(entry) => {
                self.active_chat = Some(entry.path.clone());
                self.saved_chats.retain(|chat| chat.path != entry.path);
                self.saved_chats.insert(0, entry);
                self.chat_dirty = false;
                self.history_notice = None;
                true
            }
            Err(error) => {
                self.history_notice = Some(format!("Could not save chat: {error:#}"));
                false
            }
        }
    }

    fn open_chat(&mut self, path: Option<PathBuf>) {
        self.settings = false;
        if path == self.active_chat || !self.persist_chat() {
            return;
        }
        let history = if let (Some(store), Some(path)) = (&self.chat_store, &path) {
            match store.read(path) {
                Ok(messages) => messages,
                Err(error) => {
                    self.history_notice = Some(format!("{error:#}"));
                    return;
                }
            }
        } else {
            Vec::new()
        };
        self.active_chat = path;
        self.selection = None;
        self.history = history;
        self.loaded_skills.clear();
        self.context_usage = None;
        self.status.clear();
        self.connection_errors.clear();
        self.history_notice = None;
    }

    fn saved_chats_ui(&mut self, ui: &mut egui::Ui) {
        if self.saved_chats.is_empty() && self.archived_chats.is_empty() {
            return;
        }
        ui.separator();
        let max_height = (ui.available_height() - 42.0).max(0.0);
        let mut selected = None;
        let mut chat_list = |ui: &mut egui::Ui, chats: &[ChatEntry]| {
            for chat in chats {
                let active = !self.settings && self.active_chat.as_ref() == Some(&chat.path);
                if ui
                    .add(egui::Button::selectable(active, &chat.title).truncate())
                    .on_hover_text(&chat.title)
                    .clicked()
                {
                    selected = Some(chat.path.clone());
                }
            }
        };
        egui::ScrollArea::vertical()
            .max_height(max_height)
            .show(ui, |ui| {
                ui.add_enabled_ui(!self.busy, |ui| {
                    if !self.saved_chats.is_empty() {
                        ui.label(RichText::new("Recent chats").weak().small());
                        chat_list(ui, &self.saved_chats);
                    }
                    if !self.archived_chats.is_empty() {
                        egui::CollapsingHeader::new("Archived chats")
                            .show(ui, |ui| chat_list(ui, &self.archived_chats));
                    }
                });
            });
        if let Some(path) = selected {
            self.open_chat(Some(path));
        }
    }

    fn active_chat_is_archived(&self) -> bool {
        self.chat_store
            .as_ref()
            .zip(self.active_chat.as_deref())
            .is_some_and(|(store, path)| store.is_archived(path))
    }

    fn move_active_chat(&mut self) {
        if self.busy || !self.persist_chat() {
            return;
        }
        let Some((store, path)) = self.chat_store.as_ref().zip(self.active_chat.as_deref()) else {
            return;
        };
        let restoring = store.is_archived(path);
        let (source, destination) = if restoring {
            (&mut self.archived_chats, &mut self.saved_chats)
        } else {
            (&mut self.saved_chats, &mut self.archived_chats)
        };
        let Some(index) = source.iter().position(|entry| entry.path == path) else {
            return;
        };
        let moved = if restoring {
            store.restore(path)
        } else {
            store.archive(path)
        };
        match moved {
            Ok(new_path) => {
                let mut entry = source.remove(index);
                entry.path = new_path.clone();
                destination.insert(0, entry);
                self.active_chat = Some(new_path);
                self.picker_catalog = Lookup::Idle;
                self.history_notice = None;
            }
            Err(error) => self.history_notice = Some(format!("{error:#}")),
        }
    }

    fn save(&mut self) {
        if self.config_error {
            return;
        }
        let result = (|| -> anyhow::Result<()> {
            let mut saved = self.config.clone();
            let workspace = self.workspace.trim();
            if !workspace.is_empty() {
                let path = Path::new(workspace);
                saved.workspace = path.canonicalize().unwrap_or_else(|_| path.to_owned());
            }
            saved
                .save()
                .context("Could not write Pluto's settings file")?;
            self.config = saved;
            Ok(())
        })();
        self.notice = match result {
            Ok(()) => {
                self.mcp_notice = None;
                "Settings saved".into()
            }
            Err(error) => format!("Could not save: {error:#}"),
        };
    }

    fn update_approvals(&mut self, edit: impl FnOnce(&mut Config)) -> Result<()> {
        let mut saved = Config::load()?;
        edit(&mut saved);
        saved.save()?;
        self.config.terminal_approvals = saved.terminal_approvals;
        self.config.mcp_approvals = saved.mcp_approvals;
        Ok(())
    }

    fn saved_approvals_ui(&mut self, ui: &mut egui::Ui) {
        if self.config.terminal_approvals.is_empty() && self.config.mcp_approvals.is_empty() {
            return;
        }
        let mut forget = None;
        egui::CollapsingHeader::new("Saved approvals").show(ui, |ui| {
            let cli = self
                .config
                .terminal_approvals
                .iter()
                .cloned()
                .map(ApprovalRule::Terminal);
            let mcp = self
                .config
                .mcp_approvals
                .iter()
                .cloned()
                .map(ApprovalRule::Mcp);
            for rule in cli.chain(mcp) {
                ui.horizontal(|ui| {
                    if ui.small_button("Forget").clicked() {
                        forget = Some(rule.clone());
                    }
                    ui.add(egui::Label::new(RichText::new(rule.label()).monospace()).wrap());
                });
            }
        });
        if let Some(rule) = forget {
            self.notice = match self.update_approvals(|saved| saved.forget_approval(&rule)) {
                Ok(()) => "Approval removed".into(),
                Err(error) => format!("Could not remove approval: {error:#}"),
            };
        }
    }

    fn approval_ui(&mut self, context: &egui::Context) {
        let Some(pending) = &mut self.approval else {
            return;
        };
        let mut decision = None;
        let mut remember = None;
        egui::Modal::new(egui::Id::new("approval")).show(context, |ui| {
            ui.set_width(500.0);
            ui.heading("Approve tool call");
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(&pending.request.description).monospace())
                            .wrap()
                            .selectable(true),
                    );
                });
            let scope = pending.request.rule.as_ref().map(|rule| {
                let ApprovalRule::Terminal(exact) = rule else {
                    ui.label(RichText::new(
                        "Run always approves this tool on this server, including calls with different arguments."
                    ).small());
                    return rule.clone();
                };
                let similar = exact.allow_other_operands();
                if similar.is_some() {
                    ui.checkbox(
                        &mut pending.allow_other_operands,
                        "Allow different values after --",
                    );
                }
                let selected = match similar.filter(|_| pending.allow_other_operands) {
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
            if let Some(error) = &pending.error {
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
        if let Some(rule) = remember {
            match self.update_approvals(|saved| saved.remember_approval(rule)) {
                Ok(()) => decision = Some(ApprovalDecision::RunOnce),
                Err(error) => {
                    if let Some(pending) = &mut self.approval {
                        pending.error = Some(format!("Could not save approval: {error:#}"));
                    }
                }
            }
        }
        if let Some(decision) = decision
            && let Some(pending) = self.approval.take()
        {
            let _ = pending.reply.send(decision);
        }
    }

    fn sync_discovery(&mut self) {
        let connection = self.config.model_connection().ok();
        let model = self.config.model_name.trim();
        if self.discovery_connection != connection {
            self.discovery_connection = connection;
            self.models = Lookup::Idle;
            self.capabilities = Lookup::Idle;
            self.thinking_setting = None;
            self.context_usage = None;
        }
        if self.discovery_model != model {
            self.discovery_model = model.to_owned();
            self.capabilities = Lookup::Idle;
            self.thinking_setting = None;
            self.context_usage = None;
        }
    }

    fn model_settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.label("Provider");
        let previous_provider = self.config.model_provider;
        let provider_label = previous_provider.map_or("Choose a provider", Provider::label);
        egui::ComboBox::from_id_salt("model_provider")
            .selected_text(provider_label)
            .show_ui(ui, |ui| {
                for provider in Provider::ALL {
                    ui.selectable_value(
                        &mut self.config.model_provider,
                        Some(provider),
                        provider.label(),
                    );
                }
            });
        let Some(provider) = self.config.model_provider else {
            return;
        };
        if Some(provider) != previous_provider {
            self.config.model_name.clear();
        }
        let hint = if provider == Provider::OpenAiCompatible {
            "OpenAI-compatible base URL"
        } else {
            "Server address including scheme and port"
        };
        field(ui, "Server URL", &mut self.config.model_url, hint);
        field(
            ui,
            "Model token environment variable",
            &mut self.config.model_token_env,
            "Optional",
        );
        self.sync_discovery();
        if let Some(connection) = self.discovery_connection.clone()
            && ui
                .add_enabled(
                    !matches!(self.models, Lookup::Loading(_)),
                    egui::Button::new("Load models"),
                )
                .clicked()
        {
            let client = self.discovery_client.clone();
            self.models.start(&self.runtime, ui.ctx(), async move {
                connection.models(&client).await
            });
        }
        match &self.models {
            Lookup::Idle => {}
            Lookup::Loading(_) => {
                ui.label("Loading models…");
            }
            Lookup::Complete(Ok(models)) if models.is_empty() => {
                ui.label("This server returned no models.");
            }
            Lookup::Complete(Ok(models)) => {
                let model_name = &mut self.config.model_name;
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
        field(
            ui,
            "Model name",
            &mut self.config.model_name,
            "Select from the list or enter a model name",
        );
    }

    fn chat_heading(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Chat");
            let Some(connection) = self.discovery_connection.clone() else {
                return;
            };
            if connection.provider == Provider::OpenAiCompatible || self.discovery_model.is_empty()
            {
                return;
            }
            let thinking = match &self.capabilities {
                Lookup::Idle => None,
                Lookup::Loading(_) => {
                    ui.label(RichText::new("Checking thinking levels…").weak().small());
                    return;
                }
                Lookup::Complete(Ok(capabilities)) => Some(capabilities.thinking.as_ref()),
                Lookup::Complete(Err(error)) => Some(Err(error)),
            };
            let refresh = match thinking {
                None => true,
                Some(Ok(levels)) if !levels.is_empty() => {
                    let label = levels
                        .iter()
                        .find(|level| self.thinking_setting.as_ref() == Some(&level.setting))
                        .map_or("Server default", |level| level.label.as_str());
                    ui.label("Thinking");
                    ui.add_enabled_ui(!self.busy, |ui| {
                        egui::ComboBox::from_id_salt("thinking_level")
                            .selected_text(label)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.thinking_setting,
                                    None,
                                    "Server default",
                                );
                                for level in levels {
                                    ui.selectable_value(
                                        &mut self.thinking_setting,
                                        Some(level.setting.clone()),
                                        &level.label,
                                    );
                                }
                            });
                    });
                    false
                }
                Some(result) => {
                    let explanation = match result {
                        Ok(_) => "The server did not report thinking levels.".into(),
                        Err(error) => format!("Could not check thinking levels: {error:#}"),
                    };
                    ui.small_button("Refresh thinking levels")
                        .on_hover_text(explanation)
                        .clicked()
                }
            };
            if refresh {
                let client = self.discovery_client.clone();
                let model = self.discovery_model.clone();
                self.capabilities
                    .start(&self.runtime, ui.ctx(), async move {
                        connection.capabilities(&client, &model).await
                    });
            }
        });
        self.context_usage_ui(ui);
    }

    fn context_usage_ui(&self, ui: &mut egui::Ui) {
        let window = match &self.capabilities {
            Lookup::Complete(Ok(capabilities)) => capabilities.context_window,
            _ => None,
        };
        let Some(usage) = self.context_usage else {
            if let Some(window) = window {
                ui.label(RichText::new(format!("Context window · {window} tokens")).small());
            }
            return;
        };
        let used = usage.total();
        let details = format!(
            "Last model request\nInput: {} tokens\nOutput: {} tokens",
            usage.input, usage.output
        );
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Context").small().strong());
            if let Some(window) = window {
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
                ui.label(RichText::new(format!("{used} / {window} tokens")).small())
                    .on_hover_text(&details);
            } else {
                ui.label(RichText::new(format!("{used} tokens · last request")).small())
                    .on_hover_text(&details);
            }
        });
    }

    fn mcp_draft(&self) -> Mcp {
        let target = self.mcp_address.trim().to_owned();
        if self.mcp_http {
            Mcp::Http {
                url: target,
                token_env: self.mcp_details.trim().into(),
            }
        } else {
            Mcp::Stdio {
                command: target,
                args: self
                    .mcp_details
                    .lines()
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned)
                    .collect(),
            }
        }
    }

    fn add_mcp(&mut self, server: Mcp) -> Result<String> {
        let name = self.mcp_name.trim().to_owned();
        self.config.set_mcp_server(&name, Some(server))?;
        if let Some(test) = &mut self.mcp_test
            && test.name.is_none()
            && self.config.mcp.get(&name) == Some(&test.server)
        {
            test.name = Some(name.clone());
        }
        self.mcp_name.clear();
        self.mcp_address.clear();
        self.mcp_details.clear();
        Ok(format!("Added {name}. Click Save settings to keep it."))
    }

    fn start_mcp_test(&mut self, context: &egui::Context, name: Option<String>, server: Mcp) {
        let mut result = Lookup::Idle;
        let connection = server.clone();
        result.start(&self.runtime, context, async move {
            connection.test_connection().await
        });
        self.mcp_test = Some(McpConnectionTest {
            name,
            server,
            result,
        });
    }

    fn mcp_test_status(&self, ui: &mut egui::Ui, name: Option<&str>, server: &Mcp) {
        let Some(test) = &self.mcp_test else { return };
        if test.name.as_deref() != name || &test.server != server {
            return;
        }
        match &test.result {
            Lookup::Idle => {}
            Lookup::Loading(_) => {
                ui.label("Testing connection…");
            }
            Lookup::Complete(Ok(tools)) => {
                ui.label(format!("Connected · {} tools found", tools.len()));
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

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        let save = ui
            .add_enabled(!self.config_error, egui::Button::new("Save settings"))
            .clicked();
        if save {
            self.save();
        }
        if !self.notice.is_empty() {
            ui.label(&self.notice);
        }
        ui.separator();
        ui.heading("Model and workspace");
        self.model_settings_ui(ui);
        field(
            ui,
            "Terminal working directory",
            &mut self.workspace,
            "/path/to/workspace",
        );
        self.saved_approvals_ui(ui);
        ui.separator();
        ui.heading("Skills");
        let mut remove = None;
        for (name, skill) in &self.config.skills {
            ui.horizontal(|ui| {
                ui.label(RichText::new(name).strong())
                    .on_hover_text(skill.path.display().to_string());
                if ui.small_button("Remove").clicked() {
                    remove = Some(name.clone());
                }
            });
        }
        if let Some(name) = remove {
            self.config.skills.remove(&name);
        }
        field(
            ui,
            "Skill folder or SKILL.md",
            &mut self.skill_path,
            "/path/to/skills/notes",
        );
        if ui.button("Add skill").clicked() {
            self.notice = match self.config.add_skill(Path::new(self.skill_path.trim())) {
                Ok(name) => {
                    self.skill_path.clear();
                    format!("Added {name}. Save settings to keep it across restarts.")
                }
                Err(error) => format!("Could not add skill: {error:#}"),
            };
        }
        ui.separator();
        ui.heading("MCP servers");
        let mut remove = None;
        let mut test_request = None;
        let testing = self
            .mcp_test
            .as_ref()
            .is_some_and(|test| matches!(test.result, Lookup::Loading(_)));
        for (name, server) in &self.config.mcp {
            ui.horizontal(|ui| {
                let target = match server {
                    Mcp::Http { url, .. } => url,
                    Mcp::Stdio { command, .. } => command,
                };
                ui.label(RichText::new(name).strong()).on_hover_text(target);
                if ui
                    .add_enabled(!testing, egui::Button::new("Test"))
                    .clicked()
                {
                    test_request = Some((name.clone(), server.clone()));
                }
                if ui.small_button("Remove").clicked() {
                    remove = Some(name.clone());
                }
            });
            self.mcp_test_status(ui, Some(name), server);
        }
        if let Some((name, server)) = test_request {
            self.start_mcp_test(ui.ctx(), Some(name), server);
        }
        if let Some(name) = remove {
            self.mcp_notice = Some(
                self.config
                    .set_mcp_server(&name, None)
                    .map(|()| format!("Removed {name}. Click Save settings to keep the change.")),
            );
        }
        ui.horizontal(|ui| {
            if ui.selectable_label(self.mcp_http, "Remote HTTP").clicked() && !self.mcp_http {
                self.mcp_http = true;
                self.mcp_address.clear();
                self.mcp_details.clear();
            }
            if ui
                .selectable_label(!self.mcp_http, "Local process")
                .clicked()
                && self.mcp_http
            {
                self.mcp_http = false;
                self.mcp_address.clear();
                self.mcp_details.clear();
            }
        });
        field(ui, "Server name", &mut self.mcp_name, "notes");
        if self.mcp_http {
            field(
                ui,
                "MCP endpoint URL",
                &mut self.mcp_address,
                "https://example.com/mcp",
            );
            field(
                ui,
                "Bearer token environment variable",
                &mut self.mcp_details,
                "Optional",
            );
        } else {
            field(
                ui,
                "Launch command",
                &mut self.mcp_address,
                "An executable on PATH",
            );
            ui.label("Arguments (one per line)");
            ui.add(
                TextEdit::multiline(&mut self.mcp_details)
                    .desired_width(f32::INFINITY)
                    .desired_rows(3),
            );
        }
        let draft = self.mcp_draft();
        ui.horizontal(|ui| {
            if ui.button("Add MCP server").clicked() {
                self.mcp_notice = Some(self.add_mcp(draft.clone()));
            }
            if ui
                .add_enabled(!testing, egui::Button::new("Test connection"))
                .clicked()
            {
                self.start_mcp_test(ui.ctx(), None, draft.clone());
            }
        });
        self.mcp_test_status(ui, None, &draft);
        if let Some(notice) = &self.mcp_notice {
            match notice {
                Ok(message) => ui.label(message),
                Err(error) => {
                    ui.label(RichText::new(format!("{error:#}")).color(ui.visuals().warn_fg_color))
                }
            };
        }
    }
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str) {
    ui.label(label);
    ui.add(
        TextEdit::singleline(value)
            .hint_text(hint)
            .desired_width(f32::INFINITY),
    );
}

impl eframe::App for Desktop {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.models.poll();
        self.capabilities.poll();
        self.picker_catalog.poll();
        if let Some(test) = &mut self.mcp_test {
            test.result.poll();
        }
        self.sync_discovery();
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Status(status) => self.status = status,
                Event::SkillLoaded(name) => {
                    if let Some(index) = self.history.len().checked_sub(1) {
                        self.loaded_skills.entry(index).or_default().insert(name);
                    }
                }
                Event::Warning(warning) => self.connection_errors.push(warning),
                Event::Approve(request, reply) => {
                    if request
                        .rule
                        .as_ref()
                        .is_some_and(|rule| self.config.permits(rule))
                    {
                        let _ = reply.send(ApprovalDecision::AutoApproved);
                    } else {
                        self.approval = Some(PendingApproval {
                            request,
                            reply,
                            allow_other_operands: false,
                            error: None,
                        });
                    }
                }
                Event::Usage {
                    connection,
                    model,
                    tokens,
                } if self.discovery_connection.as_ref() == Some(&connection)
                    && self.discovery_model == model =>
                {
                    self.context_usage = tokens;
                }
                Event::Usage { .. } => {}
                Event::Finished(result) => {
                    self.busy = false;
                    self.status.clear();
                    match result {
                        Ok(answer) => {
                            self.history.push(("assistant".into(), answer));
                            self.chat_dirty = true;
                            self.persist_chat();
                        }
                        Err(error) => self.status = format!("{error:#}"),
                    }
                }
            }
        }
        egui::SidePanel::left("navigation")
            .exact_width(213.0)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(context.style().visuals.window_fill)
                    .inner_margin(Margin::same(19)),
            )
            .show(context, |ui| {
                ui.label(
                    RichText::new(APP_NAME)
                        .size(21.0)
                        .strong()
                        .color(ui.visuals().hyperlink_color),
                );
                ui.add_space(20.0);
                if ui.selectable_label(!self.settings, "Chat").clicked() {
                    self.settings = false;
                }
                if ui.selectable_label(self.settings, "Settings").clicked() {
                    self.settings = true;
                }
                if ui
                    .add_enabled(!self.busy, egui::Button::new("New chat"))
                    .clicked()
                {
                    self.open_chat(None);
                }
                if !self.settings && self.active_chat.is_some() {
                    let label = if self.active_chat_is_archived() {
                        "Restore chat"
                    } else {
                        "Archive chat"
                    };
                    if ui
                        .add_enabled(!self.busy, egui::Button::new(label))
                        .clicked()
                    {
                        self.move_active_chat();
                    }
                }
                self.saved_chats_ui(ui);
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    let counts = format!(
                        "{} skills  ·  {} MCP servers",
                        self.config.skills.len(),
                        self.config.mcp.len()
                    );
                    ui.label(RichText::new(counts).weak().small());
                });
            });
        egui::CentralPanel::default()
            .frame(Frame::central_panel(&context.style()).inner_margin(Margin::same(24)))
            .show(context, |ui| {
                if self.settings {
                    self.picker_catalog = Lookup::Idle;
                    egui::ScrollArea::vertical().show(ui, |ui| self.settings_ui(ui));
                    return;
                }
                self.chat_heading(ui);
                if let Some(notice) = &self.history_notice {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(notice).color(ui.visuals().warn_fg_color));
                    });
                    if self.chat_dirty && ui.small_button("Retry save").clicked() {
                        self.persist_chat();
                    }
                }
                let composer_space = if self.active_chat_is_archived() {
                    40.0
                } else if self.picker_query().is_some() {
                    245.0
                } else if self.selection.is_some() {
                    138.0
                } else {
                    100.0
                };
                let message_area = (ui.available_height() - composer_space).max(40.0);
                egui::ScrollArea::vertical()
                    .id_salt(&self.active_chat)
                    .stick_to_bottom(true)
                    .max_height(message_area)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for warning in &self.connection_errors {
                            ui.label(RichText::new(warning).color(ui.visuals().warn_fg_color));
                        }
                        if self.history.is_empty() {
                            ui.add_space(52.0);
                            ui.label(RichText::new("What would you like to do?").size(25.0));
                        }
                        let column_width = ui.available_width().min(ui.clip_rect().width());
                        for (index, (role, content)) in self.history.iter().enumerate() {
                            chat_message(
                                ui,
                                (&self.active_chat, index),
                                column_width,
                                role,
                                content,
                                &mut self.markdown_cache,
                            );
                            if let Some(skills) = self.loaded_skills.get(&index) {
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
                        if !self.status.is_empty() {
                            ui.horizontal(|ui| {
                                if self.busy && self.approval.is_none() {
                                    ui.spinner();
                                }
                                let text = RichText::new(&self.status).small();
                                ui.add(
                                    egui::Label::new(if self.busy {
                                        text.weak()
                                    } else {
                                        text.color(ui.visuals().warn_fg_color)
                                    })
                                    .wrap(),
                                );
                            });
                        }
                    });
                self.composer_ui(ui);
            });
        self.approval_ui(context);
    }
}
