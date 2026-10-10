//! Track list, virtualized inside the caller's page scroll; shared by every view with tracks.

use super::{Ctx, library, queue_panel, rows};
use crate::i18n::{tr, tr_n};
use crate::model::{Action, PlaylistEdit, Route};
use egui::{Align2, DragAndDrop, FontId, PointerButton, Sense, WidgetInfo, WidgetType, vec2};

pub const ROW_H: f32 = 26.0;

#[derive(Clone, Copy)]
enum Row {
    Disc(u32),
    Track(usize),
}

/// The user's own playlist `uuid`, whose rows can be removed and dragged to reorder: each
/// row's position in it, and the `etag` of the listing those came from (ruling R11).
pub struct PlaylistRows<'a> {
    pub uuid: &'a str,
    pub etag: &'a str,
    pub positions: &'a [u32],
}

/// The playlist row being dragged, and the ETag of the listing it is a row of: one reloaded
/// mid-drag has other rows.
struct PlaylistDrag {
    from: usize,
    etag: String,
}

/// The `(from, to)` playlist positions a drop of row `from_row` at `slot` (`0..=len`) moves:
/// `to` is the position of the row dropped before, or past the last. None onto the row's own
/// edges.
fn move_for(positions: &[u32], from_row: usize, slot: usize) -> Option<(u32, u32)> {
    if slot == from_row || slot == from_row + 1 {
        return None;
    }
    let to = match positions.get(slot) {
        Some(&to) => to,
        None => positions.last()? + 1,
    };
    Some((*positions.get(from_row)?, to))
}

/// `m:ss`, or `h:mm:ss` from an hour.
pub fn duration(secs: u32) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// "{n} tracks · {duration}" of `tracks`.
pub fn summary(tracks: &[tidal::Track]) -> String {
    let n = tr_n("{n} track", "{n} tracks", tracks.len() as u32);
    format!(
        "{n} · {}",
        duration(tracks.iter().map(|t| t.duration).sum())
    )
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
/// tells apart two lists on one page. With `edit` (and no `discs`), rows can be removed from
/// the playlist and dragged to reorder it.
pub fn show(
    ui: &mut egui::Ui,
    ctx: &mut Ctx,
    id: &str,
    tracks: &[tidal::Track],
    discs: bool,
    edit: Option<&PlaylistRows>,
) {
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
        let top = ui.cursor().top();
        rows::virtual_rows(ui, n, ROW_H, |ui, r| {
            let row = if multi { rows[r] } else { Row::Track(r) };
            let sense = match row {
                Row::Disc(_) => Sense::hover(),
                Row::Track(_) if edit.is_some() => Sense::click_and_drag(),
                Row::Track(_) => Sense::click(),
            };
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), sense);
            match row {
                Row::Disc(n) => {
                    let label = format!("{} {n}", tr("Disc"));
                    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &label));
                    ui.painter_at(rect).text(
                        rect.left_center(),
                        Align2::LEFT_CENTER,
                        label,
                        FontId::proportional(14.0),
                        ui.visuals().strong_text_color(),
                    );
                }
                Row::Track(i) => {
                    let t = &tracks[i];
                    let num = if discs {
                        t.track_number.to_string()
                    } else {
                        (i + 1).to_string()
                    };
                    paint_row(ui, &resp, num, t, !discs, false);
                    if t.stream_ready && (resp.double_clicked() || activated(&resp)) {
                        ctx.send(Action::PlayTracks {
                            tracks: tracks.to_vec(),
                            start: i,
                        });
                    }
                    if let Some(e) = edit
                        && resp.drag_started_by(PointerButton::Primary)
                    {
                        let etag = e.etag.to_owned();
                        DragAndDrop::set_payload(ui.ctx(), PlaylistDrag { from: i, etag });
                    }
                    menu(&resp, ctx, t, edit.map(|e| (e, i)));
                }
            }
        });
        if let Some(e) = edit
            && let Some((drag, slot)) = queue_panel::dragging::<PlaylistDrag>(ui, n, top)
            && drag.etag == e.etag
            && let Some((from, to)) = move_for(e.positions, drag.from, slot)
        {
            ctx.send(Action::EditPlaylist(PlaylistEdit::Move {
                uuid: e.uuid.to_owned(),
                etag: e.etag.to_owned(),
                from,
                to,
            }));
        }
    });
}

/// Enter, Space or a screen reader "click" on `row`, without the mouse.
pub fn activated(row: &egui::Response) -> bool {
    row.clicked() && !row.clicked_by(PointerButton::Primary)
}

/// Paints track `t`'s row (`row` its response): `num`, title, artists, the album with `album`,
/// and duration, each column clipped. Unavailable tracks are dimmed, `current` highlighted.
pub fn paint_row(
    ui: &egui::Ui,
    row: &egui::Response,
    num: String,
    t: &tidal::Track,
    album: bool,
    current: bool,
) {
    if row.gained_focus() {
        row.scroll_to_me(None); // Tab onto the row built just out of view
    }
    let (rect, v) = (row.rect, ui.visuals());
    if current {
        ui.painter_at(rect)
            .rect_filled(rect, 3.0, v.selection.bg_fill);
    } else if row.hovered() || row.has_focus() {
        ui.painter_at(rect)
            .rect_filled(rect, 3.0, v.widgets.hovered.bg_fill);
    }
    let color = if !t.stream_ready {
        v.weak_text_color()
    } else if current {
        v.selection.stroke.color
    } else {
        v.text_color()
    };
    row.widget_info(|| {
        let label = format!(
            "{num}. {}, {}, {}",
            t.title,
            artists(t),
            duration(t.duration)
        );
        WidgetInfo::labeled(WidgetType::Button, t.stream_ready, label)
    });
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
        let font = FontId::proportional(14.0);
        let at = egui::pos2(x, rect.center().y);
        ui.painter_at(clip).text(at, align, text, font, color);
    };
    let (c_art, c_alb) = (w * 0.5, w * 0.75);
    cell(8.0, 44.0, num, Align2::LEFT_CENTER);
    cell(44.0, c_art, t.title.clone(), Align2::LEFT_CENTER);
    if album {
        cell(c_art, c_alb, artists(t), Align2::LEFT_CENTER);
        cell(c_alb, w - 56.0, t.album.title.clone(), Align2::LEFT_CENTER);
    } else {
        cell(c_art, w - 56.0, artists(t), Align2::LEFT_CENTER);
    }
    cell(w - 56.0, w, duration(t.duration), Align2::RIGHT_CENTER);
}

/// A track row's context menu; with `edit`, the playlist's rows and this one's index in them,
/// it can remove the row. `ui.close()` also closes it when an entry is picked from the
/// keyboard or a screen reader, which no pointer click does.
fn menu(
    row: &egui::Response,
    ctx: &mut Ctx,
    t: &tidal::Track,
    edit: Option<(&PlaylistRows, usize)>,
) {
    row.context_menu(|ui| {
        if ui.button(tr("Play next")).clicked() {
            ctx.send(Action::PlayNext(vec![t.clone()]));
            ui.close();
        }
        if ui.button(tr("Add to queue")).clicked() {
            ctx.send(Action::AddToQueue(vec![t.clone()]));
            ui.close();
        }
        library::add_to_playlist(ui, ctx, std::slice::from_ref(t));
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
        let radio = egui::Button::new(tr("Track radio"));
        if ui.add_enabled(t.mixes.track.is_some(), radio).clicked()
            && let Some(mix) = &t.mixes.track
        {
            ctx.go(Route::Mix(mix.clone()));
            ui.close();
        }
        let fav = tidal::Favorite::Track(t.id);
        let label = ctx.favorite_label(&fav);
        if ctx.favorite_button(ui, fav, &label).clicked() {
            ui.close();
        }
        if ui.button(tr("Copy link")).clicked() {
            ui.ctx()
                .copy_text(format!("https://tidal.com/browse/track/{}", t.id));
            ui.close();
        }
        if let Some((e, i)) = edit
            && let Some(&position) = e.positions.get(i)
        {
            ui.separator();
            if ui.button(tr("Remove from this playlist")).clicked() {
                ctx.send(Action::EditPlaylist(PlaylistEdit::Remove {
                    uuid: e.uuid.to_owned(),
                    etag: e.etag.to_owned(),
                    positions: vec![position],
                }));
                ui.close();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use crate::model::{Action, Event, Model, Route};
    use crate::ui::testing::{self, click, middle};
    use crate::ui::{library, page};
    use egui::{Key, Pos2};
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
            ..Default::default()
        }
    }

    /// One pass of a page holding `list`, with the library's dialog, fed `input`: the events it
    /// gave, and its output.
    fn pass(
        ctx: &egui::Context,
        tx: &UnboundedSender<Action>,
        model: &Model,
        list: &[tidal::Track],
        input: Vec<egui::Event>,
    ) -> (Vec<Event>, egui::FullOutput) {
        testing::pass_with(ctx, tx, model, input, |ui, c| {
            page(ui, &Route::Search, |ui| {
                super::show(ui, c, "t", list, false, None)
            });
            library::dialog(ui.ctx(), c);
        })
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
            pass(&ctx, &tx, &Model::default(), &list, input);
        }
        assert!(matches!(
            rx.try_recv(),
            Ok(Action::PlayTracks { start: 0, .. })
        ));
    }

    /// Right-clicks the first row of `list`: the output with its menu open.
    fn open_menu(
        ctx: &egui::Context,
        tx: &UnboundedSender<Action>,
        model: &Model,
        list: &[tidal::Track],
    ) -> egui::FullOutput {
        let at = Pos2::new(100.0, 13.0); // the first row
        let right = |pressed| testing::right(at, pressed);
        pass(ctx, tx, model, list, vec![]);
        let press = vec![egui::Event::PointerMoved(at), right(true)];
        pass(ctx, tx, model, list, press);
        pass(ctx, tx, model, list, vec![right(false)]);
        settle(ctx, tx, model, list) // the menu lays itself out
    }

    /// A few passes without input, for menus and dialogs to lay themselves out: the last output.
    fn settle(
        ctx: &egui::Context,
        tx: &UnboundedSender<Action>,
        model: &Model,
        list: &[tidal::Track],
    ) -> egui::FullOutput {
        pass(ctx, tx, model, list, vec![]);
        pass(ctx, tx, model, list, vec![]).1
    }

    /// Right-click a row, then pick "Go to album" (as a screen reader would): its album opens.
    #[test]
    fn context_menu_go_to_album_navigates() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (model, list) = (Model::default(), vec![track()]);
        let out = open_menu(&ctx, &tx, &model, &list);
        click(&out, "Add to favorites"); // panics unless the menu offers it too
        let go = vec![click(&out, "Go to album")];
        let (events, _) = pass(&ctx, &tx, &model, &list, go);
        assert!(
            matches!(events[..], [Event::Navigate(Route::Album(2))]),
            "{events:?}"
        );
    }

    /// "Play next" and "Add to queue" in a row's menu send that track to the queue.
    #[test]
    fn track_menu_play_next_sends_action() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (model, list) = (Model::default(), vec![track()]);
        let mut pick = |entry| {
            let out = open_menu(&ctx, &tx, &model, &list);
            pass(&ctx, &tx, &model, &list, vec![click(&out, entry)]);
            rx.try_recv().expect("an action")
        };
        let ids = |t: &[tidal::Track]| t.iter().map(|t| t.id).collect::<Vec<_>>();
        assert!(matches!(pick("Play next"), Action::PlayNext(t) if ids(&t) == [1]));
        assert!(matches!(pick("Add to queue"), Action::AddToQueue(t) if ids(&t) == [1]));
    }

    /// "Track radio" opens the track's mix.
    #[test]
    fn track_radio_opens_the_mix() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut t = track();
        t.mixes.track = Some("m".into());
        let (model, list) = (Model::default(), vec![t]);
        let out = open_menu(&ctx, &tx, &model, &list);
        let radio = vec![click(&out, "Track radio")];
        let (events, _) = pass(&ctx, &tx, &model, &list, radio);
        assert!(
            matches!(&events[..], [Event::Navigate(Route::Mix(id))] if id == "m"),
            "{events:?}"
        );
    }

    /// Without a mix, "Track radio" is shown but disabled, and a click does nothing.
    #[test]
    fn no_mix_disables_track_radio() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (model, list) = (Model::default(), vec![track()]);
        let out = open_menu(&ctx, &tx, &model, &list);
        let tree = out.platform_output.accesskit_update.as_ref();
        let nodes = &tree.expect("AccessKit is on").nodes;
        let radio = nodes.iter().find(|(_, n)| n.label() == Some("Track radio"));
        assert!(radio.expect("Track radio shown").1.is_disabled());
        let radio = vec![click(&out, "Track radio")];
        let (events, _) = pass(&ctx, &tx, &model, &list, radio);
        assert!(events.is_empty(), "{events:?}");
    }

    /// Logged in, with two own playlists, newest updated first (as the backend sorts them).
    fn with_playlists() -> Model {
        let mut model = testing::at(Route::Search);
        let p = |uuid: &str, title: &str| tidal::Playlist {
            uuid: uuid.into(),
            title: title.into(),
            ..testing::playlist()
        };
        let own = vec![p("p-new", "Newer"), p("p-old", "Older")];
        model.apply(Event::OwnPlaylists(own));
        model
    }

    /// Opens the first row's menu and its "Add to playlist" submenu: the output showing it.
    fn add_menu(
        ctx: &egui::Context,
        tx: &UnboundedSender<Action>,
        model: &Model,
        list: &[tidal::Track],
    ) -> egui::FullOutput {
        let out = open_menu(ctx, tx, model, list);
        pass(ctx, tx, model, list, vec![click(&out, "Add to playlist")]);
        settle(ctx, tx, model, list)
    }

    /// "Add to playlist" lists "New playlist…" and then the own playlists, newest updated
    /// first; picking one adds the track to it.
    #[test]
    fn add_to_playlist_lists_own_playlists_and_sends_add() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (model, list) = (with_playlists(), vec![track()]);
        let out = add_menu(&ctx, &tx, &model, &list);
        let y = |label| middle(&out, label).y;
        assert!(y("New playlist…") < y("Newer") && y("Newer") < y("Older"));
        pass(&ctx, &tx, &model, &list, vec![click(&out, "Older")]);
        let sent: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok())
            .map(|a| format!("{a:?}"))
            .collect();
        let add = r#"EditPlaylist(Add { uuid: "p-old", tracks: [1] })"#;
        assert_eq!(sent, [add]);
        let out = settle(&ctx, &tx, &model, &list);
        assert!(!testing::shown(&out).contains(&"Newer".into()), "closed");
    }

    /// "New playlist…" asks for a title; the playlist made starts with the track.
    #[test]
    fn new_playlist_from_a_track_sends_create_with_it() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (model, list) = (with_playlists(), vec![track()]);
        let out = add_menu(&ctx, &tx, &model, &list);
        pass(&ctx, &tx, &model, &list, vec![click(&out, "New playlist…")]);
        settle(&ctx, &tx, &model, &list); // the dialog opens, its title focused
        assert!(rx.try_recv().is_err(), "asked first");
        let text = egui::Event::Text("Road trip".into());
        pass(&ctx, &tx, &model, &list, vec![text]);
        pass(&ctx, &tx, &model, &list, vec![testing::key(Key::Enter)]);
        let create = concat!(
            r#"EditPlaylist(Create { title: "Road trip", description: "", "#,
            "folder: None, tracks: [1] })"
        );
        assert_eq!(format!("{:?}", rx.try_recv().expect("sent")), create);
    }

    /// A drop on a row's own edges moves nothing; elsewhere, the dragged row's position goes
    /// before the position of the row the drop lands on, or after the last.
    #[test]
    fn drag_reorder_sends_positions() {
        let positions = [0, 2, 3];
        let at = |from, slot| super::move_for(&positions, from, slot);
        assert_eq!(at(1, 1), None, "its top edge");
        assert_eq!(at(1, 2), None, "its bottom edge");
        assert_eq!(at(0, 2), Some((0, 3)));
        assert_eq!(at(2, 0), Some((3, 0)));
        assert_eq!(at(1, 0), Some((2, 0)));
        assert_eq!(at(0, 3), Some((0, 4)), "past the last");
    }

    #[test]
    fn duration_is_m_ss_or_h_mm_ss() {
        assert_eq!(super::duration(0), "0:00");
        assert_eq!(super::duration(65), "1:05");
        assert_eq!(super::duration(3600), "1:00:00");
        assert_eq!(super::duration(36_125), "10:02:05");
    }
}
