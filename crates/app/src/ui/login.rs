//! Login screen: paste-the-redirect-URL flow, and the way to Settings (the client to log in
//! with).

use super::Ctx;
use super::widgets::{pill_button, text_field};
use crate::i18n::tr;
use crate::model::{Action, Redirect, Route};
use crate::theme;

#[derive(Default)]
pub struct State {
    pasted: String,
}

pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx, state: &mut State) {
    super::padded(ui, |ui| form(ui, ctx, state));
}

/// A centred column (spec §5.8): the primary pill and the Settings link, then the paste step.
/// No top bar shows logged out: `ui::draw` pads the top, this the gap under it.
fn form(ui: &mut egui::Ui, ctx: &mut Ctx, state: &mut State) {
    ui.add_space(theme::MODULE_GAP);
    ui.vertical_centered(|ui| {
        ui.set_max_width(560.0);
        let login = tr("Log in with TIDAL");
        if pill_button(ui, &ctx.palette, None, &login, true).clicked() {
            ctx.send(Action::BeginLogin);
        }
        if ui.link(tr("Settings")).clicked() {
            ctx.go(Route::Settings);
        }
        let Some(url) = &ctx.model.login_url else {
            return;
        };
        ui.add_space(12.0);
        ui.label(tr(
            "After logging in, your browser shows a TIDAL page. Copy the full address from the address bar and paste it here.",
        ));
        ui.add_space(8.0);
        ui.label(tr("Login page"));
        let p = ctx.palette;
        // Read-only but selectable/copyable: edits go to a throwaway copy.
        let url = &mut url.as_str();
        text_field(ui, &p, egui::TextEdit::singleline(url).desired_width(f32::INFINITY));
        ui.add_space(8.0);
        let paste = egui::TextEdit::singleline(&mut state.pasted)
            .hint_text(tr("Paste the address here"))
            .desired_width(f32::INFINITY);
        text_field(ui, &p, paste);
        let ready = !state.pasted.trim().is_empty();
        let next = ui.add_enabled_ui(ready, |ui| pill_button(ui, &p, None, &tr("Continue"), false));
        if next.inner.clicked() {
            let pasted = Redirect(std::mem::take(&mut state.pasted));
            ctx.send(Action::FinishLogin(pasted));
        }
    });
}
