use super::egui;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum SettingsPage {
    #[default]
    Model,
    Skills,
    Mcp,
    Approvals,
}

impl SettingsPage {
    pub(super) fn tabs(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            for (page, label) in [
                (Self::Model, "Model"),
                (Self::Skills, "Skills"),
                (Self::Mcp, "MCP servers"),
                (Self::Approvals, "Tool approvals"),
            ] {
                ui.selectable_value(self, page, label);
            }
        });
        ui.separator();
    }
}
