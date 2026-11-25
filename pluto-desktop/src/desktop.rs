mod model_settings;
mod settings_tabs;
#[cfg(unix)]
mod vm_client;
#[cfg(unix)]
mod vm_settings;

use agent::{
    ApprovalDecision, Event, ModelPhase, TokenUsage, ToolPhase, ToolProgress, TurnSelection,
};
use anyhow::{Context, Result};
use eframe::egui::{self, Color32, Frame, Margin, RichText, Stroke, TextEdit};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use history::{ChatEntry, ChatHistory, ChatStore};
use model_fallback::{FallbackSettings, ModelCooldowns};
use model_settings::{FallbackCheck, ModelCatalog};
use pluto::{ASSISTANT_NAME, agent, diagnostics, history, model_fallback, providers, tools};
use providers::{Connection, ModelCapabilities, Provider, ThinkingSetting};
use settings_tabs::SettingsPage;
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::Duration,
};
use tokio::sync::oneshot;
use tools::{
    ApprovalRequest, ApprovalRule, Config, Mcp, McpToolId, ModelMode, ModelSettings, ToolSet,
};

const CHAT_DRAFT_HEIGHT: f32 = 80.0;
const CHAT_PICKER_HEIGHT: f32 = 155.0;
const CHAT_SELECTION_ROWS: usize = 3;

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

const DESKTOP_NAME: &str = "Pluto Desktop";

pub fn run(vm_instance: Option<String>, use_saved: bool) -> eframe::Result {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/pluto-icon.png"))
        .map_err(|error| eframe::Error::AppCreation(Box::new(error)))?;
    let icon = std::sync::Arc::new(icon);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 690.0])
            .with_min_inner_size([700.0, 530.0])
            .with_icon(icon.clone()),
        ..Default::default()
    };
    eframe::run_native(
        DESKTOP_NAME,
        options,
        Box::new(move |cc| {
            configure_fonts(&cc.egui_ctx);
            configure_theme(&cc.egui_ctx);
            #[cfg(unix)]
            {
                Ok(Box::new(vm_client::DesktopApp::new(
                    vm_instance,
                    use_saved,
                    &cc.egui_ctx,
                    load_logo(&cc.egui_ctx, &icon),
                )?))
            }
            #[cfg(not(unix))]
            {
                let _ = (vm_instance, use_saved);
                let mut desktop = Desktop::new()?;
                desktop.logo = Some(load_logo(&cc.egui_ctx, &icon));
                Ok(Box::new(desktop))
            }
        }),
    )
}

fn load_logo(context: &egui::Context, icon: &egui::IconData) -> egui::TextureHandle {
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [icon.width as usize, icon.height as usize],
        &icon.rgba,
    );
    context.load_texture("pluto-logo", image, egui::TextureOptions::LINEAR)
}

fn navigation_panel(context: &egui::Context) -> egui::SidePanel {
    egui::SidePanel::left("navigation")
        .exact_width(213.0)
        .resizable(false)
        .frame(
            Frame::new()
                .fill(context.style().visuals.window_fill)
                .inner_margin(Margin::same(19)),
        )
}

fn desktop_brand(ui: &mut egui::Ui, logo: Option<&egui::TextureHandle>) {
    ui.horizontal(|ui| {
        if let Some(logo) = logo {
            ui.image((logo.id(), egui::vec2(32.0, 32.0)));
        }
        ui.label(
            RichText::new(DESKTOP_NAME)
                .size(21.0)
                .strong()
                .color(ui.visuals().hyperlink_color),
        );
    });
}

fn chat_message(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash,
    width: f32,
    role: &str,
    content: &str,
    timestamp: Option<chrono::DateTime<chrono::Utc>>,
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
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(
                    RichText::new(if role == "user" {
                        "You"
                    } else {
                        ASSISTANT_NAME
                    })
                    .strong()
                    .color(ui.visuals().hyperlink_color),
                );
                if let Some(timestamp) = timestamp {
                    let local_time = timestamp.with_timezone(&chrono::Local);
                    ui.label(
                        RichText::new(local_time.format("%Y-%m-%d %H:%M").to_string())
                            .small()
                            .weak(),
                    );
                }
            });
            if assistant {
                // Markdown tables derive their IDs from the reply's UI scope.
                ui.push_id(("reply", id_salt), |ui| {
                    let (thinking, answer) = split_thinking(content);
                    if let Some(thinking) = thinking {
                        thinking_frame(ui, |ui| {
                            egui::CollapsingHeader::new(RichText::new("Model thinking").strong())
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::Label::new(RichText::new(thinking).small().weak())
                                            .wrap()
                                            .selectable(true),
                                    );
                                });
                        });
                    }
                    ui.style_mut().spacing.scroll = egui::style::ScrollStyle::solid();
                    egui::ScrollArea::horizontal()
                        .max_width(inner_width)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.set_width(inner_width);
                            ui.style_mut().url_in_tooltip = true;
                            CommonMarkViewer::new().show(ui, markdown_cache, answer);
                        });
                });
            } else {
                ui.add(egui::Label::new(content).wrap().selectable(true));
            }
        })
        .response
}

fn split_thinking(content: &str) -> (Option<&str>, &str) {
    let Some((thinking, answer)) = content
        .trim_start()
        .strip_prefix("<think>")
        .and_then(|text| text.split_once("</think>"))
    else {
        return (None, content);
    };
    let thinking = thinking.trim();
    (
        (!thinking.is_empty()).then_some(thinking),
        answer.trim_start_matches(['\r', '\n']),
    )
}

fn thinking_frame(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    Frame::new()
        .fill(ui.visuals().panel_fill)
        .stroke(ui.visuals().window_stroke)
        .corner_radius(8)
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(
                ui.available_width()
                    .min(ui.clip_rect().right() - ui.min_rect().left())
                    .max(1.0),
            );
            add_contents(ui);
        });
}

fn tool_progress_ui(ui: &mut egui::Ui, tool: &ToolProgress) {
    let status = match tool.phase {
        ToolPhase::AwaitingApproval => "Waiting for approval",
        ToolPhase::Running => "Waiting for result",
        ToolPhase::AutoApproved => "Waiting for result · Auto-approved",
        ToolPhase::GloballyAutoApproved => "Waiting for result · Auto-approved · Global setting",
    };
    let elapsed = tool.started.elapsed().as_secs();
    let details = match &tool.activity.action {
        Some(action) => format!("{action} · {status} · {elapsed}s"),
        None => format!("{status} · {elapsed}s"),
    };
    ui.horizontal_top(|ui| {
        if tool.phase != ToolPhase::AwaitingApproval {
            ui.spinner();
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add(egui::Label::new(RichText::new(&tool.activity.name).small().strong()).wrap());
            ui.add(egui::Label::new(RichText::new(details).small().weak()).wrap());
        });
    });
    ui.ctx().request_repaint_after(Duration::from_secs(1));
}

#[cfg(test)]
mod chat_layout_tests {
    use super::*;

    #[test]
    fn standalone_settings_tabs_keep_their_existing_controls() -> Result<()> {
        let context = egui::Context::default();
        let mut desktop = Desktop::with_config(Config::default(), String::new(), None, None)?;
        let mut render = |events| {
            context.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| desktop.settings_ui(ui));
                },
            )
        };
        let label = |output: &egui::FullOutput, expected: &str| {
            output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == expected => {
                    Some(text.pos + text.galley.size() / 2.0)
                }
                _ => None,
            })
        };
        assert!(label(&render(Vec::new()), "Terminal working directory").is_some());
        for (tab, control) in [
            ("Skills", "Add skill"),
            ("MCP servers", "Test connection"),
            (
                "Tool approvals",
                "Auto-approve tool calls (at your own risk)",
            ),
        ] {
            let position = label(&render(Vec::new()), tab).context("Missing settings tab")?;
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
            let output = render(Vec::new());
            assert!(label(&output, control).is_some(), "Missing {control}");
            assert!(label(&output, "Save settings").is_some());
            assert!(label(&output, "Terminal working directory").is_none());
        }
        Ok(())
    }

    #[test]
    fn compatible_context_meter_uses_input_tokens_and_the_selected_limit() -> Result<()> {
        let mut config = Config::default();
        config.models.mode = ModelMode::OpenAiCompatible;
        let context = egui::Context::default();
        let mut desktop = Desktop::with_config(config, String::new(), None, None)?;
        desktop.context_usage = Some(TokenUsage {
            input: 100,
            output: 20,
        });
        let shows = |desktop: &Desktop, label| {
            let output = context.run(egui::RawInput::default(), |context| {
                egui::CentralPanel::default().show(context, |ui| desktop.context_usage_ui(ui));
            });
            output.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == label)
            })
        };
        assert!(shows(&desktop, "120 tokens · last request"));
        desktop.capabilities = Lookup::Complete(Ok(ModelCapabilities {
            thinking: Ok(Vec::new()),
            context_window: Some(1_000),
        }));
        assert!(shows(&desktop, "100 / 1000 input tokens"));
        desktop.config.models.openai.context_limit = 2_000;
        assert!(shows(&desktop, "100 / 2000 input tokens"));
        Ok(())
    }

    #[test]
    fn long_chat_and_draft_leave_composer_visible() -> Result<()> {
        let context = egui::Context::default();
        configure_fonts(&context);
        configure_theme(&context);
        let mut desktop = Desktop::with_config(Config::default(), String::new(), None, None)?;
        desktop.settings = false;
        desktop
            .history
            .push("assistant", "Long reply.\n\n".repeat(80));
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(700.0, 530.0));
        let render = |desktop: &mut Desktop, events| {
            context.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |context| desktop.show_ui(context),
            )
        };
        let visible_text = |output: &egui::FullOutput, matches: fn(&str) -> bool, fully_visible| {
            output.shapes.iter().find_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    let bounds = text.galley.rect.translate(text.pos.to_vec2());
                    let clip = shape.clip_rect.intersect(screen);
                    let visible = matches(text.galley.text())
                        && if fully_visible {
                            clip.contains_rect(bounds)
                        } else {
                            clip.intersects(bounds)
                        };
                    visible.then_some(bounds.center())
                } else {
                    None
                }
            })
        };
        let initial = render(&mut desktop, Vec::new());
        let send_position = visible_text(&initial, |text| text == "Send", true)
            .context("Send must be visible before entering a draft")?;
        for draft in ["Draft line\n".repeat(80), "Draft line ".repeat(240)] {
            desktop.prompt = draft;
            let output = render(&mut desktop, Vec::new());
            assert_eq!(
                visible_text(&output, |text| text == "Send", true),
                Some(send_position)
            );
            assert!(visible_text(&output, |text| text.starts_with("Draft line"), false).is_some());
        }

        let button = |pressed| egui::Event::PointerButton {
            pos: send_position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        render(
            &mut desktop,
            vec![
                egui::Event::PointerMoved(send_position),
                button(true),
                button(false),
            ],
        );
        assert!(
            desktop.settings,
            "Clicking Send should open setup when no model is configured"
        );
        Ok(())
    }

    #[test]
    fn chat_controls_can_share_a_frame_without_widget_id_collisions() -> Result<()> {
        let context = egui::Context::default();
        configure_fonts(&context);
        configure_theme(&context);
        context.options_mut(|options| options.warn_on_id_clash = true);

        let mut config = Config::default();
        config.models.local.provider = Some(Provider::LlamaCpp);
        config.models.local.url = "http://127.0.0.1:1".into();
        config.models.local.name = "fixture".into();
        config
            .add_skill(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../pluto/tests/test-notes"))?;
        config
            .skills
            .insert("summary".into(), config.skills["test-notes"].clone());
        let mut desktop = Desktop::with_config(config, String::new(), None, None)?;
        desktop.sync_discovery();
        desktop.capabilities = Lookup::Complete(Ok(ModelCapabilities {
            thinking: Ok(vec![providers::ThinkingLevel {
                label: "On".into(),
                setting: ThinkingSetting::Toggle(true),
            }]),
            context_window: Some(8_192),
        }));
        desktop.context_usage = Some(TokenUsage {
            input: 100,
            output: 20,
        });
        for index in 0..3 {
            desktop.saved_chats.push(ChatEntry {
                path: PathBuf::from(format!("chat-{index}.json")),
                title: "Same chat title".into(),
            });
        }
        desktop.active_chat = Some(desktop.saved_chats[0].path.clone());
        desktop.archived_chats.push(ChatEntry {
            path: PathBuf::from("archive/chat.json"),
            title: "Same chat title".into(),
        });
        let markdown = "| Name | Value |\n| --- | --- |\n| Apples | 2 |\n\n```text\nExample\n```";
        desktop.history.push("user", "Show the results".into());
        desktop.history.push("assistant", markdown.into());
        desktop.history.push("assistant", markdown.into());
        desktop.prompt = "/".into();
        desktop.picker_catalog = Lookup::Complete(Ok(PickerCatalog {
            tools: ["fixture", "secondary"]
                .map(|server| McpToolId {
                    server: server.into(),
                    name: "lookup".into(),
                })
                .into(),
            warnings: Vec::new(),
        }));

        let render = |desktop: &mut Desktop, events| {
            let output = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1100.0, 1000.0),
                    )),
                    events,
                    ..Default::default()
                },
                |context| desktop.show_ui(context),
            );
            output
                .shapes
                .into_iter()
                .filter_map(|shape| match shape.shape {
                    egui::Shape::Text(text) => {
                        let label = text.galley.text();
                        assert!(
                            !(label.starts_with("🔥 ") && label.contains(" use of ")),
                            "{label}"
                        );
                        Some((
                            label.to_owned(),
                            text.pos + text.galley.rect.center().to_vec2(),
                        ))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let find_label = |labels: &[(String, egui::Pos2)], expected| {
            labels
                .iter()
                .find_map(|(label, position)| (label == expected).then_some(*position))
                .unwrap_or_else(|| panic!("Missing chat UI label {expected}"))
        };
        let click = |pos| {
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            vec![egui::Event::PointerMoved(pos), button(true), button(false)]
        };
        render(&mut desktop, Vec::new());
        let labels = render(&mut desktop, Vec::new());
        for expected in ["Thinking", "test-notes", "fixture / lookup"] {
            find_label(&labels, expected);
        }
        let timestamp = desktop
            .history
            .timestamp(0)
            .context("Missing message timestamp")?
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string();
        let timestamp_y = find_label(&labels, timestamp.as_str()).y;
        assert!(find_label(&labels, "You").y < timestamp_y);
        assert!(timestamp_y < find_label(&labels, "Show the results").y);
        assert_eq!(
            labels.iter().filter(|(label, _)| label == "Apples").count(),
            2
        );
        render(&mut desktop, click(find_label(&labels, "Archived chats")));
        let labels = render(&mut desktop, Vec::new());
        assert_eq!(
            labels
                .iter()
                .filter(|(label, _)| label == "Same chat title")
                .count(),
            4
        );
        render(&mut desktop, click(find_label(&labels, "Server default")));
        let labels = render(&mut desktop, Vec::new());
        render(&mut desktop, click(find_label(&labels, "On")));
        assert_eq!(
            desktop.thinking_setting,
            Some(ThinkingSetting::Toggle(true))
        );

        let labels = render(&mut desktop, Vec::new());
        render(&mut desktop, click(find_label(&labels, "test-notes")));
        assert_eq!(desktop.selection.skills, ["test-notes"]);
        render(&mut desktop, vec![egui::Event::Text("/".into())]);
        assert_eq!(desktop.picker_query(), Some(""));
        let labels = render(&mut desktop, Vec::new());
        assert_eq!(
            labels
                .iter()
                .filter(|(label, _)| label == "test-notes")
                .count(),
            1
        );
        render(&mut desktop, click(find_label(&labels, "summary")));
        assert_eq!(desktop.selection.skills, ["test-notes", "summary"]);
        let labels = render(&mut desktop, Vec::new());
        for expected in ["test-notes", "summary", "Send"] {
            find_label(&labels, expected);
        }
        render(&mut desktop, click(find_label(&labels, "×")));
        assert_eq!(desktop.selection.skills, ["summary"]);
        desktop.prompt = "/".into();
        let labels = render(&mut desktop, Vec::new());
        render(&mut desktop, click(find_label(&labels, "fixture / lookup")));
        assert_eq!(desktop.selection.mcp.len(), 1);
        render(&mut desktop, vec![egui::Event::Text("/".into())]);
        assert_eq!(desktop.picker_query(), Some(""));
        let labels = render(&mut desktop, Vec::new());
        render(
            &mut desktop,
            click(find_label(&labels, "secondary / lookup")),
        );
        assert_eq!(desktop.selection.mcp.len(), 2);
        assert!(desktop.picker_choices("lookup").is_empty());
        assert_eq!(desktop.selection.skills, ["summary"]);
        for _ in 0..3 {
            let labels = render(&mut desktop, Vec::new());
            render(&mut desktop, click(find_label(&labels, "×")));
        }
        assert!(desktop.selection.skills.is_empty());
        assert!(desktop.selection.mcp.is_empty());

        desktop.prompt.clear();
        desktop.busy = true;
        let phases = [
            (ModelPhase::Thinking, "Thinking…"),
            (ModelPhase::ReviewingToolResults, "Reviewing tool results…"),
            (
                ModelPhase::WritingPartialAnswer,
                "Writing a partial answer…",
            ),
        ];
        let phase_labels = phases.each_ref().map(|(_, label)| *label);
        for (phase, expected) in phases {
            desktop.status = ChatStatus::ModelRequest(phase);
            let labels = render(&mut desktop, Vec::new());
            let visible_phases = labels
                .iter()
                .map(|(label, _)| label.as_str())
                .filter(|label| phase_labels.contains(label))
                .collect::<Vec<_>>();
            assert_eq!(visible_phases, [expected]);
        }

        for action in ["Retry model call", "End turn"] {
            let (retry, mut decision) = oneshot::channel();
            desktop.status = ChatStatus::ModelRequestFailed("HTTP 503".into(), retry);
            let labels = render(&mut desktop, Vec::new());
            let message_count = desktop.history.messages().len();
            render(&mut desktop, click(find_label(&labels, action)));
            let expected = if action == "Retry model call" {
                Ok(())
            } else {
                Err(oneshot::error::TryRecvError::Closed)
            };
            assert_eq!(decision.try_recv(), expected);
            assert_eq!(desktop.history.messages().len(), message_count);
            assert!(desktop.busy);
            let labels = render(&mut desktop, Vec::new());
            assert!(!labels.iter().any(|(label, _)| label == "Retry model call"));
        }

        desktop.status = ChatStatus::Idle;
        desktop.history.push(
            "assistant",
            "<think>Check the fixture.</think>\n\n**Fixture verified.**".into(),
        );
        let labels = render(&mut desktop, Vec::new());
        let header = find_label(&labels, "Model thinking");
        assert!(header.y < find_label(&labels, "Fixture verified.").y);
        assert!(
            !labels
                .iter()
                .any(|(label, _)| label == "Check the fixture.")
        );
        render(&mut desktop, click(header));
        let labels = render(&mut desktop, Vec::new());
        assert!(
            find_label(&labels, "Check the fixture.").y
                < find_label(&labels, "Fixture verified.").y
        );

        desktop.tool_progress = Some(ToolProgress {
            activity: tools::ToolActivity {
                name: "fixture / lookup".into(),
                action: Some("Look up the fixture".into()),
            },
            phase: ToolPhase::GloballyAutoApproved,
            started: std::time::Instant::now() - Duration::from_secs(8),
        });
        let labels = render(&mut desktop, Vec::new());
        let name = find_label(&labels, "fixture / lookup");
        let (seconds, details) = labels.iter().find_map(|(label, position)| {
            let seconds = label.strip_prefix("Look up the fixture · Waiting for result · Auto-approved · Global setting · ")?
                .strip_suffix('s')?.parse::<u64>().ok()?;
            Some((seconds, position))
        }).context("Missing running tool details")?;
        assert!(seconds >= 8);
        assert!(name.y < details.y);
        desktop
            .tool_progress
            .as_mut()
            .expect("Tool is active")
            .phase = ToolPhase::AwaitingApproval;
        let (reply, _decision) = oneshot::channel();
        desktop.approval = Some(PendingApproval {
            request: ApprovalRequest {
                description: "Call fixture / lookup".into(),
                rule: None,
            },
            reply,
            allow_other_operands: false,
            error: None,
        });
        render(&mut desktop, Vec::new());
        let labels = render(&mut desktop, Vec::new());
        for expected in ["Approve tool call", "Run once", "Decline"] {
            find_label(&labels, expected);
        }
        Ok(())
    }

    #[test]
    fn wide_table_cells_keep_their_width_and_scroll() {
        let context = egui::Context::default();
        configure_fonts(&context);
        configure_theme(&context);
        context.options_mut(|options| options.warn_on_id_clash = true);
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
                            let reply =
                                chat_message(ui, 0, width, "assistant", &table, None, &mut cache);
                            assert!(reply.rect.width() <= width + 1.0);
                            chat_message(
                                ui,
                                1,
                                width,
                                "assistant",
                                "| Item | Count |\n| --- | --- |\n| Apples | 2 |",
                                None,
                                &mut cache,
                            );
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
        let output = render(Vec::new());
        for shape in &output.shapes {
            if let egui::Shape::Text(text) = &shape.shape {
                let label = text.galley.text();
                assert!(!label.contains("use of Grid ID"), "{label}");
            }
        }
        let (before, viewport) = find_cell(output);
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
                        let reply = chat_message(ui, 0, width, "assistant", &paragraph, None, &mut cache);
                        assert!(reply.rect.height() > 100.0);
                        assert!(reply.rect.width() <= width + 1.0);
                        let reply = chat_message(ui, 1, width, "assistant", &code, None, &mut cache);
                        assert!(reply.rect.height() > 80.0);
                        assert!(reply.rect.width() <= width + 1.0);
                        let reply = chat_message(ui, 2, width, "assistant", table, None, &mut cache);
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

#[derive(Clone)]
enum PickerChoice {
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

enum ChatStatus {
    Idle,
    Preparing(String),
    ModelRequest(ModelPhase),
    ModelRequestFailed(String, oneshot::Sender<()>),
    Error(String),
}

struct Desktop {
    logo: Option<egui::TextureHandle>,
    runtime: tokio::runtime::Runtime,
    events: mpsc::Receiver<Event>,
    sender: mpsc::Sender<Event>,
    diagnostics: Option<diagnostics::LogSender>,
    config: Config,
    discovery_client: reqwest::Client,
    discovery_connection: Option<Connection>,
    discovery_model: String,
    model_catalog: ModelCatalog,
    models_draft: ModelSettings,
    capabilities: Lookup<ModelCapabilities>,
    thinking_setting: Option<ThinkingSetting>,
    context_usage: Option<TokenUsage>,
    switched_model: Option<(Provider, Option<u64>)>,
    fallback_draft: FallbackSettings,
    fallback_check: Option<FallbackCheck>,
    model_cooldowns: Arc<ModelCooldowns>,
    history: ChatHistory,
    markdown_cache: CommonMarkCache,
    chat_store: Option<ChatStore>,
    saved_chats: Vec<ChatEntry>,
    archived_chats: Vec<ChatEntry>,
    active_chat: Option<PathBuf>,
    chat_dirty: bool,
    history_notice: Option<String>,
    prompt: String,
    selection: TurnSelection,
    picker_catalog: Lookup<PickerCatalog>,
    picker_index: usize,
    settings: bool,
    settings_page: SettingsPage,
    tool_settings_dirty: bool,
    #[cfg(unix)]
    vm_picker: vm_client::VmPicker,
    busy: bool,
    status: ChatStatus,
    tool_progress: Option<ToolProgress>,
    loaded_skills: BTreeMap<usize, BTreeSet<String>>,
    notice: String,
    connection_errors: Vec<String>,
    config_error: bool,
    auto_approve_tools_draft: bool,
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
        let (config, notice) = match Config::load() {
            Ok(config) => (config, String::new()),
            Err(error) => (
                Config::default(),
                format!("{error:#}. Fix the file and reload settings before saving."),
            ),
        };
        let (chat_store, history_notice, diagnostics) = match Config::path() {
            Ok(path) => (
                Some(ChatStore::beside(&path)),
                None,
                diagnostics::start(path.with_file_name("agent-debug.jsonl")).ok(),
            ),
            Err(error) => (
                None,
                Some(format!("Chat history unavailable: {error:#}")),
                None,
            ),
        };
        let mut desktop = Self::with_config(config, notice, chat_store, history_notice)?;
        desktop.diagnostics = diagnostics;
        Ok(desktop)
    }

    fn with_config(
        config: Config,
        notice: String,
        chat_store: Option<ChatStore>,
        mut history_notice: Option<String>,
    ) -> Result<Self> {
        let (sender, events) = mpsc::channel();
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
            None => (None, ChatHistory::default()),
        };
        Ok(Self {
            logo: None,
            #[cfg(unix)]
            vm_picker: vm_client::VmPicker::default(),
            runtime: tokio::runtime::Runtime::new()?,
            settings: config
                .models
                .profile(config.models.mode)
                .name
                .trim()
                .is_empty()
                || config.model_connection().is_err(),
            settings_page: SettingsPage::Model,
            tool_settings_dirty: false,
            workspace: config.workspace.display().to_string(),
            config_error: !notice.is_empty(),
            auto_approve_tools_draft: config.auto_approve_tools,
            fallback_draft: config.model_fallbacks.clone(),
            models_draft: config.models.clone(),
            fallback_check: None,
            config,
            discovery_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()?,
            discovery_connection: None,
            discovery_model: String::new(),
            model_catalog: ModelCatalog::default(),
            capabilities: Lookup::Idle,
            thinking_setting: None,
            context_usage: None,
            switched_model: None,
            model_cooldowns: Arc::default(),
            events,
            sender,
            diagnostics: None,
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
            selection: TurnSelection::default(),
            picker_catalog: Lookup::Idle,
            picker_index: 0,
            busy: false,
            status: ChatStatus::Idle,
            tool_progress: None,
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
        if self.config.model_connection().is_err()
            || self
                .config
                .models
                .profile(self.config.models.mode)
                .name
                .trim()
                .is_empty()
        {
            self.settings = true;
            self.settings_page = SettingsPage::Model;
            self.notice = "Choose a provider, enter its server URL, and select a model.".into();
            return;
        }
        let selection = std::mem::take(&mut self.selection);
        self.picker_catalog = Lookup::Idle;
        self.status = ChatStatus::Preparing(match selection.skills.as_slice() {
            [] => "Discovering tools…".into(),
            [name] => format!("Preparing to load skill · {name}…"),
            skills => format!("Preparing to load {} skills…", skills.len()),
        });
        self.tool_progress = None;
        self.connection_errors.clear();
        if self.switched_model.take().is_some() {
            self.context_usage = None;
        }
        self.busy = true;
        self.history.push("user", std::mem::take(&mut self.prompt));
        self.chat_dirty = true;
        self.persist_chat();
        let config = self.config.clone();
        let history = self.history.messages().to_vec();
        let thinking = self.thinking_setting.clone();
        let cooldowns = self.model_cooldowns.clone();
        let sender = self.sender.clone();
        let chat_id = self.active_chat.as_deref().and_then(ChatStore::chat_id);
        let trace = diagnostics::Trace::new(self.diagnostics.clone(), chat_id);
        let context = context.clone();
        self.runtime.spawn(async move {
            let emit = |event| {
                let _ = sender.send(event);
                context.request_repaint();
            };
            let result = agent::chat(
                config, history, thinking, selection, &cooldowns, &trace, emit,
            )
            .await;
            trace.record(
                "turn_finished",
                || serde_json::json!({"reply_returned": result.is_ok()}),
            );
            emit(Event::Finished(result));
        });
    }

    fn picker_query(&self) -> Option<&str> {
        if self.busy {
            return None;
        }
        self.prompt
            .strip_prefix('/')
            .filter(|query| !query.contains(['/', '\n', '\r']))
    }

    fn picker_choices(&self, query: &str) -> Vec<PickerChoice> {
        let query = query.to_lowercase();
        let matches = |label: &str| {
            let label = label.to_lowercase();
            query.split_whitespace().all(|word| label.contains(word))
        };
        let mut choices: Vec<_> = self
            .config
            .skills
            .keys()
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

    fn composer_ui(&mut self, ui: &mut egui::Ui) {
        if self.active_chat_is_archived() {
            ui.label(RichText::new("Archived chat · Restore it to continue").weak());
            return;
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
        let input = egui::ScrollArea::vertical()
            .id_salt("composer_draft")
            .max_height(CHAT_DRAFT_HEIGHT)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_enabled(
                    !self.busy,
                    TextEdit::multiline(&mut self.prompt)
                        .id(input_id)
                        .hint_text("Message Pluto or type / to pick a skill or tool")
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                )
            })
            .inner;
        if input.changed() {
            self.picker_index = 0;
        }
        if self.picker_query().is_some() {
            self.picker_ui(ui, input_id, previous, next, select);
            return;
        }
        if self.selection.skills.is_empty() && self.selection.mcp.is_empty() {
            self.picker_catalog = Lookup::Idle;
        }
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
            ChatHistory::default()
        };
        self.active_chat = path;
        self.selection = TurnSelection::default();
        self.picker_catalog = Lookup::Idle;
        self.history = history;
        self.loaded_skills.clear();
        self.context_usage = None;
        self.switched_model = None;
        self.status = ChatStatus::Idle;
        self.tool_progress = None;
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
            saved.auto_approve_tools = self.auto_approve_tools_draft;
            saved.models = self.models_draft.clone();
            if saved.models.mode == ModelMode::OpenAiCompatible {
                self.fallback_draft.validate()?;
                saved.models.connection(ModelMode::OpenAiCompatible)?;
                anyhow::ensure!(
                    !saved.models.openai.name.trim().is_empty(),
                    "Enter the OpenAI-compatible model ID"
                );
            }
            saved.model_fallbacks = self.fallback_draft.clone();
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
                self.tool_settings_dirty = false;
                self.workspace = self.config.workspace.display().to_string();
                "Settings saved".into()
            }
            Err(error) => format!("Could not save: {error:#}"),
        };
    }

    fn reload_settings(&mut self) {
        match Config::load() {
            Ok(config) => {
                self.models_draft = config.models.clone();
                self.fallback_draft = config.model_fallbacks.clone();
                self.auto_approve_tools_draft = config.auto_approve_tools;
                self.workspace = config.workspace.display().to_string();
                self.config = config;
                self.config_error = false;
                self.tool_settings_dirty = false;
                self.model_catalog = ModelCatalog::default();
                self.fallback_check = None;
                self.skill_path.clear();
                self.mcp_name.clear();
                self.mcp_address.clear();
                self.mcp_details.clear();
                self.mcp_notice = None;
                self.mcp_test = None;
                self.picker_catalog = Lookup::Idle;
                self.notice = "Settings reloaded".into();
            }
            Err(error) => self.notice = format!("Could not reload settings: {error:#}"),
        }
    }

    fn settings_dirty(&self) -> bool {
        self.tool_settings_dirty
            || self.models_draft != self.config.models
            || self.fallback_draft != self.config.model_fallbacks
            || self.auto_approve_tools_draft != self.config.auto_approve_tools
            || (!self.workspace.trim().is_empty()
                && Path::new(self.workspace.trim()) != self.config.workspace)
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
        let model = self
            .config
            .models
            .profile(self.config.models.mode)
            .name
            .trim();
        if self.discovery_connection != connection {
            self.discovery_connection = connection;
            self.capabilities = Lookup::Idle;
            self.thinking_setting = None;
            self.context_usage = None;
            self.switched_model = None;
        }
        if self.discovery_model != model {
            self.discovery_model = model.to_owned();
            self.capabilities = Lookup::Idle;
            self.thinking_setting = None;
            self.context_usage = None;
            self.switched_model = None;
        }
    }

    fn model_settings_ui(&mut self, ui: &mut egui::Ui, mode: ModelMode) {
        let profile = match mode {
            ModelMode::LocalOnly => &mut self.models_draft.local,
            ModelMode::OpenAiCompatible => &mut self.models_draft.openai,
        };
        let client = self.discovery_client.clone();
        self.model_catalog.show(
            ui,
            &self.runtime,
            mode,
            profile,
            "Optional",
            |connection, _| async move { connection.models(&client).await },
        );
    }

    fn fallback_settings_ui(&mut self, ui: &mut egui::Ui) {
        model_settings::fallback_form(
            ui,
            &mut self.fallback_draft,
            &mut self.fallback_check,
            |ui, entry| {
                let connection = match entry.connection() {
                    Ok(connection) => connection,
                    Err(error) => return Lookup::Complete(Err(error)),
                };
                let client = self.discovery_client.clone();
                let model = entry.model.trim().to_owned();
                let mut result = Lookup::Idle;
                result.start(&self.runtime, ui.ctx(), async move {
                    connection.fallback_metadata(&client, &model).await
                });
                result
            },
        );
    }

    fn chat_heading(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Chat");
            if let Some(chat_id) = self.active_chat.as_deref().and_then(ChatStore::chat_id)
                && ui
                    .small_button("Copy chat ID")
                    .on_hover_text("Use the chat ID to find its diagnostic events")
                    .clicked()
            {
                ui.ctx().copy_text(chat_id.to_owned());
            }
            ui.label(
                RichText::new(self.config.models.mode.label())
                    .small()
                    .weak(),
            );
            let Some(connection) = self.discovery_connection.clone() else {
                return;
            };
            if self.discovery_model.is_empty() {
                return;
            }
            if connection.provider == Provider::OpenAiCompatible {
                if self.config.models.openai.context_limit == 0
                    && matches!(self.capabilities, Lookup::Idle)
                {
                    self.start_capabilities_lookup(ui, connection);
                }
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
                self.start_capabilities_lookup(ui, connection);
            }
        });
        self.context_usage_ui(ui);
    }

    fn start_capabilities_lookup(&mut self, ui: &egui::Ui, connection: Connection) {
        let client = self.discovery_client.clone();
        let model = self.discovery_model.clone();
        self.capabilities
            .start(&self.runtime, ui.ctx(), async move {
                connection.capabilities(&client, &model).await
            });
    }

    fn context_usage_ui(&self, ui: &mut egui::Ui) {
        let reported_window = match &self.capabilities {
            Lookup::Complete(Ok(capabilities)) => capabilities.context_window,
            _ => None,
        };
        let provider = self
            .switched_model
            .map(|(provider, _)| provider)
            .or_else(|| {
                self.discovery_connection
                    .as_ref()
                    .map(|connection| connection.provider)
            });
        let input_only = provider != Some(Provider::LlamaCpp);
        let override_limit = self
            .config
            .models
            .profile(self.config.models.mode)
            .context_limit;
        let window = if let Some((_, context_window)) = self.switched_model {
            context_window
        } else if input_only && override_limit > 0 {
            Some(u64::from(override_limit))
        } else {
            reported_window
        };
        let units = if input_only { "input tokens" } else { "tokens" };
        let Some(usage) = self.context_usage else {
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
        #[cfg(unix)]
        self.vm_picker.show(ui, &self.runtime, self.busy, None);
        ui.heading("Standalone settings");
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!self.config_error, egui::Button::new("Save settings"))
                .clicked()
            {
                self.save();
            }
            if ui
                .button("Reload settings")
                .on_hover_text("Discard unsaved changes and reload from this computer")
                .clicked()
            {
                self.reload_settings();
            }
        });
        if self.settings_dirty() {
            ui.weak("Unsaved changes");
        }
        if !self.notice.is_empty() {
            ui.label(&self.notice);
        }
        self.settings_page.tabs(ui);
        match self.settings_page {
            SettingsPage::Model => self.standalone_model_ui(ui),
            SettingsPage::Skills => self.standalone_skills_ui(ui),
            SettingsPage::Mcp => self.standalone_mcp_ui(ui),
            SettingsPage::Approvals => self.standalone_approvals_ui(ui),
        }
    }

    fn standalone_model_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            for mode in [ModelMode::LocalOnly, ModelMode::OpenAiCompatible] {
                ui.radio_value(&mut self.models_draft.mode, mode, mode.label());
            }
        });
        egui::CollapsingHeader::new("Local model")
            .default_open(self.models_draft.mode == ModelMode::LocalOnly)
            .show(ui, |ui| self.model_settings_ui(ui, ModelMode::LocalOnly));
        egui::CollapsingHeader::new("OpenAI-compatible model")
            .default_open(self.models_draft.mode == ModelMode::OpenAiCompatible)
            .show(ui, |ui| {
                self.model_settings_ui(ui, ModelMode::OpenAiCompatible);
                ui.label(
                    "Pluto uses the saved local model when the configured cloud options fail.",
                );
                self.fallback_settings_ui(ui);
            });
        field(
            ui,
            "Terminal working directory",
            &mut self.workspace,
            "/path/to/workspace",
        );
    }

    fn standalone_approvals_ui(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(
            &mut self.auto_approve_tools_draft,
            "Auto-approve tool calls (at your own risk)",
        )
        .on_hover_text(
            "After saving, new messages can run terminal commands and MCP tools without asking.",
        );
        self.saved_approvals_ui(ui);
    }

    fn standalone_skills_ui(&mut self, ui: &mut egui::Ui) {
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
            self.tool_settings_dirty = true;
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
                    self.tool_settings_dirty = true;
                    format!("Added {name}. Save settings to keep it across restarts.")
                }
                Err(error) => format!("Could not add skill: {error:#}"),
            };
        }
    }

    fn standalone_mcp_ui(&mut self, ui: &mut egui::Ui) {
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
            let result = self.config.set_mcp_server(&name, None);
            self.tool_settings_dirty |= result.is_ok();
            self.mcp_notice = Some(
                result.map(|()| format!("Removed {name}. Click Save settings to keep the change.")),
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
                let result = self.add_mcp(draft.clone());
                self.tool_settings_dirty |= result.is_ok();
                self.mcp_notice = Some(result);
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

fn field(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str) -> egui::Response {
    ui.label(label);
    ui.add(
        TextEdit::singleline(value)
            .hint_text(hint)
            .desired_width(f32::INFINITY),
    )
}

impl eframe::App for Desktop {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.capabilities.poll();
        self.picker_catalog.poll();
        if let Some(test) = &mut self.mcp_test {
            test.result.poll();
        }
        self.sync_discovery();
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::ModelRequest(phase) => {
                    self.status = ChatStatus::ModelRequest(phase);
                    self.tool_progress = None;
                }
                Event::ModelRequestFailed(error, retry) => {
                    self.status = ChatStatus::ModelRequestFailed(error, retry);
                }
                Event::ModelSwitched {
                    model,
                    origin,
                    provider,
                    context_window,
                } => {
                    self.switched_model = Some((provider, context_window));
                    self.context_usage = None;
                    self.connection_errors.push(format!(
                        "Switched to {model} via {} at {origin}",
                        provider.label()
                    ));
                }
                Event::Tool(tool) => self.tool_progress = Some(tool),
                Event::SkillLoaded(name) => {
                    if let Some(index) = self.history.messages().len().checked_sub(1) {
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
                } if self.switched_model.is_some()
                    || (self.discovery_connection.as_ref() == Some(&connection)
                        && self.discovery_model == model) =>
                {
                    self.context_usage = tokens;
                }
                Event::Usage { .. } => {}
                Event::Finished(result) => {
                    self.busy = false;
                    self.status = ChatStatus::Idle;
                    self.tool_progress = None;
                    match result {
                        Ok(answer) => {
                            self.history.push("assistant", answer);
                            self.chat_dirty = true;
                            self.persist_chat();
                        }
                        Err(error) => self.status = ChatStatus::Error(format!("{error:#}")),
                    }
                }
            }
        }
        self.show_ui(context);
    }
}

impl Desktop {
    fn show_ui(&mut self, context: &egui::Context) {
        navigation_panel(context).show(context, |ui| {
            desktop_brand(ui, self.logo.as_ref());
            ui.weak("Standalone · Unsandboxed").on_hover_text(
                "Terminal commands and local MCP servers run with your account’s permissions.",
            );
            ui.add_space(20.0);
            if ui.selectable_label(!self.settings, "Chat").clicked() {
                self.settings = false;
            }
            if ui.selectable_label(self.settings, "Settings").clicked() && !self.settings {
                #[cfg(unix)]
                self.vm_picker.refresh();
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
                let spacing = ui.spacing().item_spacing.y;
                let row = ui.spacing().interact_size.y;
                let composer_height = if self.active_chat_is_archived() {
                    row + spacing
                } else {
                    let actions = if self.picker_query().is_some() {
                        CHAT_PICKER_HEIGHT
                    } else {
                        row
                    };
                    self.selection_height(ui) + CHAT_DRAFT_HEIGHT + actions + 2.0 * spacing
                };
                egui::TopBottomPanel::bottom("chat_composer_panel")
                    .exact_height(composer_height)
                    .show_separator_line(false)
                    .frame(Frame::new().inner_margin(Margin {
                        top: spacing as i8,
                        ..Margin::ZERO
                    }))
                    .show_inside(ui, |ui| self.composer_ui(ui));
                egui::ScrollArea::vertical()
                    .id_salt(&self.active_chat)
                    .stick_to_bottom(true)
                    .min_scrolled_height(0.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for warning in &self.connection_errors {
                            ui.label(RichText::new(warning).color(ui.visuals().warn_fg_color));
                        }
                        if self.history.messages().is_empty() {
                            ui.add_space(52.0);
                            ui.label(RichText::new("What would you like to do?").size(25.0));
                        }
                        let column_width = ui.available_width().min(ui.clip_rect().width());
                        for (index, (role, content)) in self.history.messages().iter().enumerate() {
                            chat_message(
                                ui,
                                (&self.active_chat, index),
                                column_width,
                                role,
                                content,
                                self.history.timestamp(index),
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
                        if let Some(tool) = &self.tool_progress {
                            tool_progress_ui(ui, tool);
                        } else {
                            match &self.status {
                                ChatStatus::Idle => {}
                                ChatStatus::Preparing(status) => {
                                    ui.horizontal_top(|ui| {
                                        ui.spinner();
                                        ui.add(
                                            egui::Label::new(RichText::new(status).small().weak())
                                                .wrap(),
                                        );
                                    });
                                }
                                ChatStatus::ModelRequest(phase) => {
                                    let label = match phase {
                                        ModelPhase::Thinking => "Thinking…",
                                        ModelPhase::ReviewingToolResults => {
                                            "Reviewing tool results…"
                                        }
                                        ModelPhase::WritingPartialAnswer => {
                                            "Writing a partial answer…"
                                        }
                                    };
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.add(
                                            egui::Label::new(RichText::new(label).small()).wrap(),
                                        );
                                    });
                                }
                                ChatStatus::ModelRequestFailed(status, _)
                                | ChatStatus::Error(status) => {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(status)
                                                .small()
                                                .color(ui.visuals().warn_fg_color),
                                        )
                                        .wrap(),
                                    );
                                    if matches!(self.status, ChatStatus::ModelRequestFailed(..)) {
                                        let (retry, end_turn) = ui
                                            .horizontal_wrapped(|ui| {
                                                (
                                                    ui.button("Retry model call").clicked(),
                                                    ui.button("End turn").clicked(),
                                                )
                                            })
                                            .inner;
                                        if retry || end_turn {
                                            let next_status = if retry {
                                                ChatStatus::Preparing(
                                                    "Retrying model request…".into(),
                                                )
                                            } else {
                                                ChatStatus::Idle
                                            };
                                            if let ChatStatus::ModelRequestFailed(_, decision) =
                                                std::mem::replace(&mut self.status, next_status)
                                                && retry
                                            {
                                                let _ = decision.send(());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    });
            });
        self.approval_ui(context);
    }
}
