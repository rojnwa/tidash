//! Track list, virtualized inside the caller's page scroll; shared by every view with tracks.

use super::{Ctx, rows};
use crate::i18n::tr;
use crate::model::{Action, Route};
use egui::{Align2, FontId, PointerButton, Sense, WidgetInfo, WidgetType, vec2};

const ROW_H: f32 = 26.0;

#[derive(Clone, Copy)]
enum Row {
    Disc(u32),
    Track(usize),
}

pub fn duration(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

pub fn names(artists: &[tidal::Artist]) -> String {
    artists
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn artists(t: &tidal::Track) -> String {
    names(&t.artists)
}

/// Double-clicking an available row, or activating it from the keyboard or a screen reader,
/// sends `PlayTracks` for the whole list; right-clicking opens its menu. With `discs`,
/// multi-disc lists get a header row per disc; otherwise the album title is a column. `id`
/// tells apart two lists on one page.
pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx, id: &str, tracks: &[tidal::Track], discs: bool) {
    let multi = discs && tracks.iter().any(|t| t.volume_number > 1);
    // Only a multi-disc album (short) needs a row list, for its disc headers; in any other
    // list row i is track i, and nothing is built per frame.
    let mut rows = Vec::new();
    if multi {
        let mut disc = 0;
        for (i, t) in tracks.iter().enumerate() {
            if t.volume_number != disc {
                disc = t.volume_number;
                rows.push(Row::Disc(disc));
            }
            rows.push(Row::Track(i));
        }
    }
    let n = if multi { rows.len() } else { tracks.len() };
    ui.push_id(id, |ui| {
        rows::virtual_rows(ui, n, ROW_H, |ui, r| {
            let row = if multi { rows[r] } else { Row::Track(r) };
            let sense = match row {
                Row::Disc(_) => Sense::hover(),
                Row::Track(_) => Sense::click(),
            };
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), sense);
            let p = ui.painter_at(rect);
            let font = FontId::proportional(14.0);
            match row {
                Row::Disc(n) => {
                    let label = format!("{} {n}", tr("Disc"));
                    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &label));
                    p.text(
                        rect.left_center(),
                        Align2::LEFT_CENTER,
                        label,
                        font,
                        ui.visuals().strong_text_color(),
                    );
                }
                Row::Track(i) => {
                    let t = &tracks[i];
                    let ready = t.stream_ready;
                    if resp.gained_focus() {
                        resp.scroll_to_me(None); // Tab onto the row built just out of view
                    }
                    if resp.hovered() || resp.has_focus() {
                        p.rect_filled(rect, 3.0, ui.visuals().widgets.hovered.bg_fill);
                    }
                    let color = if ready {
                        ui.visuals().text_color()
                    } else {
                        ui.visuals().weak_text_color()
                    };
                    let w = rect.width();
                    // Text in [x0, x1) from the left edge, clipped so columns never overlap.
                    let cell = |x0: f32, x1: f32, text: String, align: Align2| {
                        let clip = egui::Rect::from_x_y_ranges(
                            (rect.left() + x0)..=(rect.left() + x1 - 6.0),
                            rect.y_range(),
                        );
                        let x = if align == Align2::RIGHT_CENTER {
                            clip.right()
                        } else {
                            clip.left()
                        };
                        ui.painter_at(clip).text(
                            egui::pos2(x, rect.center().y),
                            align,
                            text,
                            font.clone(),
                            color,
                        );
                    };
                    let num = if discs {
                        t.track_number.to_string()
                    } else {
                        (i + 1).to_string()
                    };
                    resp.widget_info(|| {
                        let label = format!(
                            "{num}. {}, {}, {}",
                            t.title,
                            artists(t),
                            duration(t.duration)
                        );
                        WidgetInfo::labeled(WidgetType::Button, ready, label)
                    });
                    let (c_art, c_alb) = (w * 0.5, w * 0.75);
                    cell(8.0, 44.0, num, Align2::LEFT_CENTER);
                    cell(44.0, c_art, t.title.clone(), Align2::LEFT_CENTER);
                    if discs {
                        cell(c_art, w - 56.0, artists(t), Align2::LEFT_CENTER);
                    } else {
                        cell(c_art, c_alb, artists(t), Align2::LEFT_CENTER);
                        cell(c_alb, w - 56.0, t.album.title.clone(), Align2::LEFT_CENTER);
                    }
                    cell(w - 56.0, w, duration(t.duration), Align2::RIGHT_CENTER);
                    // Enter, Space or a screen reader "click" without the mouse.
                    let activated = resp.clicked() && !resp.clicked_by(PointerButton::Primary);
                    if ready && (resp.double_clicked() || activated) {
                        ctx.send(Action::PlayTracks {
                            tracks: tracks.to_vec(),
                            start: i,
                        });
                    }
                    menu(&resp, ctx, t);
                }
            }
        });
    });
}

/// A track row's context menu. `ui.close()` also closes it when an entry is picked from the
/// keyboard or a screen reader, which no pointer click does.
fn menu(row: &egui::Response, ctx: &mut Ctx, t: &tidal::Track) {
    row.context_menu(|ui| {
        if ui.button(tr("Go to album")).clicked() {
            ctx.go(Route::Album(t.album.id));
            ui.close();
        }
        if let Some(artist) = t.artists.first()
            && ui.button(tr("Go to artist")).clicked()
        {
            ctx.go(Route::Artist(artist.id));
            ui.close();
        }
        if ui.button(tr("Copy link")).clicked() {
            ui.ctx()
                .copy_text(format!("https://tidal.com/browse/track/{}", t.id));
            ui.close();
        }
    });
}

#[cfg(test)]
mod tests {
    use crate::images::TextureCache;
    use crate::model::{Action, Event, Model, Route};
    use crate::ui::{Ctx, page};
    use egui::accesskit;
    use egui::{Key, PointerButton, Pos2, Rect, vec2};
    use tokio::sync::mpsc::UnboundedSender;

    fn track() -> tidal::Track {
        tidal::Track {
            id: 1,
            title: "t".into(),
            duration: 60,
            track_number: 1,
            volume_number: 1,
            artists: vec![],
            album: tidal::AlbumRef {
                id: 2,
                title: "a".into(),
                cover: None,
            },
            stream_ready: true,
        }
    }

    /// One pass of a page holding `list`, fed `input`: the events it gave, and its output.
    fn pass(
        ctx: &egui::Context,
        tx: &UnboundedSender<Action>,
        list: &[tidal::Track],
        input: Vec<egui::Event>,
    ) -> (Vec<Event>, egui::FullOutput) {
        let model = Model::default();
        let mut images = TextureCache::new(1 << 20);
        let mut events = Vec::new();
        let input = egui::RawInput {
            events: input,
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            let mut c = Ctx {
                model: &model,
                tx,
                images: &mut images,
                events: &mut events,
            };
            page(ui, &Route::Search, |ui| {
                super::show(ui, &mut c, "t", list, false)
            });
        });
        out.textures_delta.clear(); // nothing renders them
        (events, out)
    }

    /// Tab to the first row and press Enter: it plays, as a double click would.
    #[test]
    fn enter_on_a_focused_row_plays() {
        let ctx = egui::Context::default();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let list = vec![track()];
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        for input in [vec![], vec![key(Key::Tab)], vec![key(Key::Enter)]] {
            pass(&ctx, &tx, &list, input);
        }
        assert!(matches!(
            rx.try_recv(),
            Ok(Action::PlayTracks { start: 0, .. })
        ));
    }

    /// Right-click a row, then pick "Go to album" (as a screen reader would): its album opens.
    #[test]
    fn context_menu_go_to_album_navigates() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let list = vec![track()];
        let at = Pos2::new(100.0, 13.0); // the first row
        let right = |pressed| egui::Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Default::default(),
        };
        pass(&ctx, &tx, &list, vec![]);
        pass(
            &ctx,
            &tx,
            &list,
            vec![egui::Event::PointerMoved(at), right(true)],
        );
        pass(&ctx, &tx, &list, vec![right(false)]);
        pass(&ctx, &tx, &list, vec![]); // the menu lays itself out
        let (_, out) = pass(&ctx, &tx, &list, vec![]);
        let tree = out
            .platform_output
            .accesskit_update
            .expect("AccessKit is on");
        let item = tree
            .nodes
            .iter()
            .find(|(_, n)| n.label() == Some("Go to album"))
            .map(|(id, _)| *id)
            .expect("the menu shows Go to album");
        let click = egui::Event::AccessKitActionRequest(accesskit::ActionRequest {
            action: accesskit::Action::Click,
            target_tree: accesskit::TreeId::ROOT,
            target_node: item,
            data: None,
        });
        let (events, _) = pass(&ctx, &tx, &list, vec![click]);
        assert!(
            matches!(events[..], [Event::Navigate(Route::Album(2))]),
            "{events:?}"
        );
    }

    #[test]
    fn duration_is_m_ss() {
        assert_eq!(super::duration(0), "0:00");
        assert_eq!(super::duration(65), "1:05");
        assert_eq!(super::duration(3600), "60:00");
    }
}
