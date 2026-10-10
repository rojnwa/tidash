//! Login screen: paste-the-redirect-URL flow, and the way to Settings (the client to log in
//! with).

use super::Ctx;
use crate::i18n::tr;
use crate::model::{Action, Redirect, Route};

#[derive(Default)]
pub struct State {
    pasted: String,
}

pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx, state: &mut State) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.set_max_width(560.0);
        if ui.button(tr("Log in with TIDAL")).clicked() {
            ctx.send(Action::BeginLogin);
        }
        if ui.small_button(tr("Settings")).clicked() {
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
        // Read-only but selectable/copyable: edits go to a throwaway copy.
        ui.add(egui::TextEdit::singleline(&mut url.as_str()).desired_width(f32::INFINITY));
        ui.add_space(8.0);
        ui.add(
            egui::TextEdit::singleline(&mut state.pasted)
                .hint_text(tr("Paste the address here"))
                .desired_width(f32::INFINITY),
        );
        let ready = !state.pasted.trim().is_empty();
        if ui.add_enabled(ready, egui::Button::new(tr("Continue"))).clicked() {
            let pasted = Redirect(std::mem::take(&mut state.pasted));
            ctx.send(Action::FinishLogin(pasted));
        }
    });
}
