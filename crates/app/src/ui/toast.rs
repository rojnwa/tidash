//! One short confirmation at a time for an action with no visible result (spec §3.10).

use crate::theme::{self, Palette, TOAST_FADE, TOAST_SHOW};
use egui::{Align2, Id, LayerId, Order, Rect, accesskit, pos2, vec2};
use std::time::Duration;

/// Where the toast showing keeps its text and the time it was shown.
fn id() -> Id {
    Id::new("toast")
}

/// Shows `text` from this pass on, replacing any toast showing.
pub fn show(ctx: &egui::Context, text: String) {
    let now = ctx.input(|i| i.time);
    ctx.data_mut(|d| d.insert_temp(id(), (text, now)));
}

/// The toast showing, if any: bottom-centre of `over`, `PAGE_PAD` above its bottom, on a
/// foreground layer that takes no input, and a polite live region for screen readers. It fades
/// in over `TOAST_FADE`, and out over the `TOAST_FADE` after `TOAST_SHOW`; between, it asks
/// for no repaint until its fade out.
pub fn paint(ctx: &egui::Context, p: &Palette, over: Rect) {
    let Some((text, at)) = ctx.data(|d| d.get_temp::<(String, f64)>(id())) else {
        return;
    };
    let age = (ctx.input(|i| i.time) - at) as f32;
    let end = TOAST_SHOW + TOAST_FADE;
    if age >= end {
        ctx.data_mut(|d| d.remove::<(String, f64)>(id()));
        return;
    }
    let opacity = (age.min(end - age) / TOAST_FADE).clamp(0.0, 1.0);
    if opacity < 1.0 {
        ctx.request_repaint();
    } else {
        // Rounding can leave a hair past `TOAST_SHOW` at full opacity; a negative panics.
        let left = (TOAST_SHOW - age).max(0.0);
        ctx.request_repaint_after(Duration::from_secs_f32(left));
    }
    let mut painter = ctx.layer_painter(LayerId::new(Order::Foreground, id()));
    painter.set_opacity(opacity);
    let pad = vec2(12.0, 8.0); // a menu item's
    let wrap = over.width() - 2.0 * (theme::PAGE_PAD + pad.x);
    let galley = painter.layout(text.clone(), theme::body(), p.text, wrap);
    let foot = pos2(over.center().x, over.bottom() - theme::PAGE_PAD);
    let rect = Align2::CENTER_BOTTOM.anchor_size(foot, galley.size() + 2.0 * pad);
    let shadow = ctx.global_style().visuals.popup_shadow;
    painter.add(shadow.as_shape(rect, theme::R_CARD));
    painter.rect_filled(rect, theme::R_CARD, p.overlay);
    painter.galley(rect.min + pad, galley, p.text);
    // A node per toast, so the same words shown again are announced again.
    ctx.accesskit_node_builder(id().with(at.to_bits()), |node| {
        node.set_role(accesskit::Role::Status);
        node.set_live(accesskit::Live::Polite);
        node.set_label(text);
        node.set_bounds(accesskit::Rect {
            x0: rect.min.x.into(),
            y0: rect.min.y.into(),
            x1: rect.max.x.into(),
            y1: rect.max.y.into(),
        });
    });
}

#[cfg(test)]
mod tests {
    use super::show;
    use crate::model::{Action, Event, PageData, Route};
    use crate::theme::{self, Palette, TOAST_FADE, TOAST_SHOW};
    use crate::ui::{UiState, draw, testing};
    use std::time::Duration;

    /// Spec §3.10: a toast is a polite live region from its first pass; it rests between its
    /// fades with no repaint every pass, and goes after `TOAST_SHOW + TOAST_FADE` with nothing
    /// left to repaint. A second replaces the first. Clicks go through it to the page.
    #[test]
    fn a_toast_comes_and_goes() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let route = Route::Collection(crate::model::CollectionTab::Tracks);
        let mut model = testing::at(route.clone());
        let data = PageData::Tracks(testing::tracks(30));
        model.apply(Event::Page { route, data });
        // One pass of `draw` at `time`, showing `toast` first if given, fed `events`.
        let mut pass = |time: f64, toast: Option<&str>, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                events,
                time: Some(time),
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                if let Some(text) = toast {
                    show(ui.ctx(), text.into());
                }
                draw(ui, &model, &tx, &mut state);
            });
            out.textures_delta.clear(); // nothing renders them
            out
        };
        let repaint =
            |out: &egui::FullOutput| out.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        let shown =
            |out: &egui::FullOutput, text: &str| testing::shown(out).iter().any(|s| s == text);

        let out = pass(0.0, Some("Link copied"), vec![]);
        let tree = out.platform_output.accesskit_update.as_ref();
        let nodes = &tree.expect("AccessKit is on").nodes;
        let toast = nodes.iter().find(|(_, n)| n.label() == Some("Link copied"));
        let toast = &toast.expect("the toast").1;
        assert_eq!(toast.role(), egui::accesskit::Role::Status);
        assert_eq!(toast.live(), Some(egui::accesskit::Live::Polite));

        pass(0.5, None, vec![]); // egui repaints once more after a pass that asked
        let out = pass(1.0, None, vec![]);
        assert!(shown(&out, "Link copied"), "still shown");
        let left = Duration::from_secs_f32(TOAST_SHOW - 1.0);
        let delay = repaint(&out);
        assert!(Duration::ZERO < delay && delay <= left, "{delay:?}");
        // The pill, opaque, `PAGE_PAD` above the content area's bottom (the player bar's top),
        // centred on the window, not on the content area beside the sidebar.
        let foot = 600.0 - theme::PLAYER_BAR - theme::PAGE_PAD;
        let pill = out.shapes.iter().any(|s| match &s.shape {
            egui::Shape::Rect(r) => {
                r.fill == Palette::dark().overlay
                    && r.rect.bottom() == foot
                    && (r.rect.center().x - 400.0).abs() <= 0.5
            }
            _ => false,
        });
        assert!(pill, "no pill painted");

        // A click where it sits plays the row under it.
        let at = testing::middle(&out, "Link copied");
        pass(1.02, None, vec![egui::Event::PointerMoved(at)]);
        for (i, pressed) in [true, false].into_iter().enumerate() {
            pass(
                1.04 + 0.02 * i as f64,
                None,
                vec![testing::primary(at, pressed)],
            );
        }
        let played = std::iter::from_fn(|| rx.try_recv().ok())
            .any(|a| matches!(a, Action::PlayTracks { .. }));
        assert!(played, "the click reaches the page");

        let gone = f64::from(TOAST_SHOW + TOAST_FADE) + 0.25;
        assert!(!shown(&pass(gone, None, vec![]), "Link copied"), "gone");
        let out = pass(gone + 0.1, None, vec![]);
        assert_eq!(repaint(&out), Duration::MAX, "repaints at rest");

        // One at a time: the second replaces the first, and shows its own while.
        pass(4.0, Some("Link copied"), vec![]);
        let out = pass(5.0, Some("Added to queue"), vec![]);
        assert!(shown(&out, "Added to queue") && !shown(&out, "Link copied"));
        let out = pass(4.0 + gone, None, vec![]);
        assert!(
            shown(&out, "Added to queue"),
            "the second keeps its own time"
        );
    }

    /// The backend's answer (`Event::Toast`), through `UiState::apply`, shows as a toast.
    #[test]
    fn a_toast_event_shows() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let model = testing::at(Route::Home);
        let said = "Added 1 track to Road trip";
        let mut out = ctx.run_ui(Default::default(), |ui| {
            state.apply(ui.ctx(), &Event::Toast(said.into()));
            draw(ui, &model, &tx, &mut state);
        });
        out.textures_delta.clear(); // nothing renders them
        assert!(testing::shown(&out).iter().any(|s| s == said));
    }
}
