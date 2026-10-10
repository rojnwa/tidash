//! Credits panel: the current track's credits, a heading per role; credited artists link to
//! their pages.

use super::Ctx;
use super::widgets::heading;
use crate::i18n::tr;
use crate::model::{Action, Event, Load, Route};
use crate::theme;
use egui::{RichText, ScrollArea};

/// Draws into `ui`, the side panel's: each role in `small()` `secondary` over its names.
pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx) {
    let p = ctx.palette;
    heading(ui, &p, &tr("Credits"));
    let model = ctx.model;
    let ask = |id| (Event::Credits(id, Load::Loading), Action::LoadCredits(id));
    let failed = tr("Couldn't load credits");
    let Some(credits) = super::track_data(ui, ctx, &model.credits, &failed, ask) else {
        return;
    };
    if credits.is_empty() {
        ui.label(tr("No credits for this track"));
        return;
    }
    ScrollArea::vertical()
        .id_salt("credits")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for credit in credits {
                let role = RichText::new(&credit.role).font(theme::small());
                ui.label(role.color(p.secondary));
                for person in &credit.contributors {
                    if let Some(id) = person.id {
                        if ui.link(&person.name).clicked() {
                            ctx.go(Route::Artist(id));
                        }
                    } else {
                        ui.label(&person.name);
                    }
                }
                ui.add_space(8.0);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::testing::{at, click, clicks, pass, shown, track};

    fn view(ui: &mut egui::Ui, ctx: &mut Ctx<'_>, _: &Route) {
        show(ui, ctx);
    }

    /// A contributor with an artist id links to the artist; one without is plain text.
    #[test]
    fn credits_list_roles_and_link_artists() {
        let person = |name: &str, id| tidal::Contributor {
            name: name.into(),
            id,
        };
        let credits = vec![tidal::Credit {
            role: "Producer".into(),
            contributors: vec![person("Linked", Some(5)), person("Plain", None)],
        }];
        let mut model = at(Route::Home);
        model.playback.track = Some(track(1));
        model.apply(Event::Credits(1, Load::Loading));
        model.apply(Event::Credits(1, Load::Ready(credits)));
        let egui = crate::ui::testing::context();
        egui.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (_, out) = pass(&egui, &tx, &model, view, vec![]);
        let shown = shown(&out);
        for t in ["Producer", "Linked", "Plain"] {
            assert!(shown.iter().any(|s| s == t), "no {t:?} in {shown:?}");
        }
        assert!(clicks(&out, "Plain").next().is_none(), "not a link");
        let (events, _) = pass(&egui, &tx, &model, view, vec![click(&out, "Linked")]);
        assert!(
            matches!(events[..], [Event::Navigate(Route::Artist(5))]),
            "{events:?}"
        );
    }
}
