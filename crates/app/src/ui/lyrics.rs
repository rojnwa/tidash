//! Lyrics panel: the current track's lyrics. Synced ones follow the playback, and a click on a
//! line seeks there.

use super::Ctx;
use super::widgets::{fade, focus_ring, heading};
use crate::i18n::tr;
use crate::model::{Action, Event, Load};
use crate::theme;
use egui::text::LayoutJob;
use egui::{Align, Layout, RichText, ScrollArea, Sense, WidgetInfo, WidgetType, vec2};
use std::time::Duration;

/// The line playing at `at`: the last that has started.
pub fn current_line(lines: &[tidal::LyricLine], at: Duration) -> Option<usize> {
    lines.iter().rposition(|l| l.at <= at)
}

/// Draws into `ui`, the side panel's; `at` is the playback position. Unsynced lyrics are
/// `body()`; the provider's credit `small()` `dim`.
pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx, at: Duration) {
    // The last current line, kept by the panel.
    let last_line = ui.id().with("lyrics-line");
    let p = ctx.palette;
    heading(ui, &p, &tr("Lyrics"));
    let model = ctx.model;
    let ask = |id| (Event::Lyrics(id, Load::Loading), Action::LoadLyrics(id));
    let failed = tr("Couldn't load lyrics");
    let Some(lyrics) = super::track_data(ui, ctx, &model.lyrics, &failed, ask) else {
        return;
    };
    let Some(lyrics) = lyrics else {
        ui.label(tr("No lyrics for this track"));
        return;
    };
    let align = if lyrics.rtl { Align::Max } else { Align::Min };
    ScrollArea::vertical()
        .id_salt("lyrics")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.with_layout(Layout::top_down(align), |ui| {
                if lyrics.lines.is_empty() {
                    ui.label(RichText::new(&lyrics.text).font(theme::body()));
                } else {
                    synced(ui, ctx, &lyrics.lines, at, last_line);
                }
                if let Some(provider) = &lyrics.provider {
                    ui.add_space(12.0);
                    let credit =
                        tr("Lyrics provided by {provider}").replace("{provider}", provider);
                    ui.label(RichText::new(credit).font(theme::small()).color(p.dim));
                }
            });
        });
}

/// A row per line in `lyric()`, wrapped and aligned as the layout, that seeks to it: the one
/// playing at `at` in `text`, the others `dim` (`text` hovered). The playing one is scrolled to
/// the centre when it changes (the last one is kept under `last_line`). A gap shows as "…".
fn synced(
    ui: &mut egui::Ui,
    ctx: &Ctx,
    lines: &[tidal::LyricLine],
    at: Duration,
    last_line: egui::Id,
) {
    let p = ctx.palette;
    let current = current_line(lines, at);
    let moved = ui.ctx().data_mut(|d| {
        let last = d.get_temp::<Option<usize>>(last_line);
        d.insert_temp(last_line, current);
        last != Some(current)
    });
    let (width, align) = (ui.available_width(), ui.layout().horizontal_align());
    for (i, line) in lines.iter().enumerate() {
        let text = if line.text.is_empty() {
            "…"
        } else {
            &line.text
        };
        let on = current == Some(i);
        let mut job = LayoutJob::simple(text.to_owned(), theme::lyric(), p.text, width);
        job.halign = align;
        let galley = ui.painter().layout_job(job);
        let (rect, row) = ui.allocate_exact_size(vec2(width, galley.size().y), Sense::click());
        let row = row.on_hover_cursor(egui::CursorIcon::PointingHand);
        row.widget_info(|| {
            WidgetInfo::selected(WidgetType::SelectableLabel, ui.is_enabled(), on, text)
        });
        if ui.is_rect_visible(rect) {
            let lit = if on {
                1.0
            } else {
                fade(ui, &row, row.hovered())
            };
            let ink = theme::blend(p.dim, p.text, lit);
            // `halign` lays the galley out from its anchor: the left edge, or the right one.
            let anchor = if align == Align::Max {
                rect.right_top()
            } else {
                rect.left_top()
            };
            ui.painter()
                .galley_with_override_text_color(anchor, galley, ink);
        }
        focus_ring(ui, &row, theme::R_ROW);
        if row.clicked() {
            ctx.send(Action::Seek(line.at));
        }
        if moved && on {
            // ponytail: follows every line change; pause after a manual scroll if users ask
            row.scroll_to_me(Some(Align::Center));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Model, Route};
    use crate::ui::testing::{at, click, pass, shown, track};

    fn line(secs: u64, text: &str) -> tidal::LyricLine {
        let at = Duration::from_secs(secs);
        let text = text.into();
        tidal::LyricLine { at, text }
    }

    fn view(ui: &mut egui::Ui, ctx: &mut Ctx<'_>, _: &Route) {
        show(ui, ctx, Duration::ZERO);
    }

    /// Track 1 playing, its lyrics in `load` (none asked for with `None`).
    fn playing(load: Option<Load<Option<tidal::Lyrics>>>) -> Model {
        let mut m = at(Route::Home);
        m.playback.track = Some(track(1));
        if let Some(load) = load {
            m.apply(Event::Lyrics(1, Load::Loading));
            m.apply(Event::Lyrics(1, load));
        }
        m
    }

    fn sent(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Action>) -> Vec<String> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .map(|a| format!("{a:?}"))
            .collect()
    }

    #[test]
    fn current_line_picks_the_last_started() {
        let lines = [line(5, "a"), line(10, "b"), line(20, "c")];
        let at = |secs| current_line(&lines, Duration::from_secs(secs));
        assert_eq!(at(0), None, "before the first line");
        assert_eq!(at(5), Some(0), "as the first starts");
        assert_eq!(at(15), Some(1), "between lines");
        assert_eq!(at(600), Some(2), "past the end");
        assert_eq!(current_line(&[], Duration::ZERO), None);
    }

    /// The first pass asks; the model then holds its `Loading`, so the next does not. A new
    /// current track is asked for in turn.
    #[test]
    fn lyrics_load_once_per_track() {
        let egui = crate::ui::testing::context();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut model = playing(None);
        let mut asked = Vec::new();
        for id in [1, 1, 2] {
            model.playback.track = Some(track(id));
            let (events, _) = pass(&egui, &tx, &model, view, vec![]);
            for ev in events {
                model.apply(ev);
            }
            asked.extend(sent(&mut rx));
        }
        assert_eq!(asked, ["LoadLyrics(1)", "LoadLyrics(2)"]);
        assert_eq!(model.lyrics, Some((2, Load::Loading)));
    }

    /// A click on a synced line seeks to it; the provider is credited below.
    #[test]
    fn clicking_a_line_seeks() {
        let lyrics = tidal::Lyrics {
            lines: vec![line(5, "One"), line(10, "Two")],
            text: "One\nTwo".into(),
            rtl: false,
            provider: Some("MUSIXMATCH".into()),
        };
        let model = playing(Some(Load::Ready(Some(lyrics))));
        let egui = crate::ui::testing::context();
        egui.enable_accesskit();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (_, out) = pass(&egui, &tx, &model, view, vec![]);
        let credit = "Lyrics provided by MUSIXMATCH".to_owned();
        assert!(shown(&out).contains(&credit), "{:?}", shown(&out));
        pass(&egui, &tx, &model, view, vec![click(&out, "Two")]);
        assert_eq!(sent(&mut rx), ["Seek(10s)"]);
    }

    /// Spec §5.8: the line playing is drawn in `text`, the others in `dim`.
    #[test]
    fn the_current_line_is_brighter() {
        fn third(ui: &mut egui::Ui, ctx: &mut Ctx<'_>, _: &Route) {
            show(ui, ctx, Duration::from_secs(25)); // in line 3
        }
        let lines = [(5, "One"), (10, "Two"), (20, "Three"), (30, "Four")];
        let lyrics = tidal::Lyrics {
            lines: lines.iter().map(|&(at, text)| line(at, text)).collect(),
            text: String::new(),
            rtl: false,
            provider: None,
        };
        let model = playing(Some(Load::Ready(Some(lyrics))));
        let egui = crate::ui::testing::context();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (_, out) = pass(&egui, &tx, &model, third, vec![]);
        let ink = |text: &str| {
            let shape = out.shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == text => Some(t),
                _ => None,
            });
            let t = shape.unwrap_or_else(|| panic!("no {text:?} drawn"));
            let c = t.override_text_color;
            c.unwrap_or(t.galley.job.sections[0].format.color)
        };
        let p = crate::theme::Palette::dark();
        assert_eq!(ink("Three"), p.text);
        assert_eq!(ink("Two"), p.dim);
    }

    /// None says so, and so does no current track (asking for nothing); failed lyrics say
    /// that, and Retry asks again.
    #[test]
    fn no_lyrics_says_so() {
        let egui = crate::ui::testing::context();
        egui.enable_accesskit();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (events, out) = pass(&egui, &tx, &at(Route::Home), view, vec![]);
        let idle = "Nothing is playing".to_owned();
        assert!(shown(&out).contains(&idle), "{:?}", shown(&out));
        assert!(events.is_empty() && sent(&mut rx).is_empty(), "{events:?}");
        let model = playing(Some(Load::Ready(None)));
        let (_, out) = pass(&egui, &tx, &model, view, vec![]);
        let none = "No lyrics for this track".to_owned();
        assert!(shown(&out).contains(&none), "{:?}", shown(&out));
        let model = playing(Some(Load::Failed));
        let (_, out) = pass(&egui, &tx, &model, view, vec![]);
        let failed = "Couldn't load lyrics".to_owned();
        assert!(shown(&out).contains(&failed), "{:?}", shown(&out));
        assert_eq!(
            sent(&mut rx),
            Vec::<String>::new(),
            "shown, not asked again"
        );
        let (events, _) = pass(&egui, &tx, &model, view, vec![click(&out, "Retry")]);
        assert!(
            matches!(events[..], [Event::Lyrics(1, Load::Loading)]),
            "{events:?}"
        );
        assert_eq!(sent(&mut rx), ["LoadLyrics(1)"]);
    }
}
