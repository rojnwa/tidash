//! Queue panel: the play queue, its current track highlighted. Rows are dragged with egui's
//! bare `DragAndDrop` payload: its drop zones give no insertion index and need every row
//! built, and these rows are virtual. A playlist's rows drag the same way (`dragging`).

use super::tracks::{self, ROW_H};
use super::widgets::{context_menu, menu_button};
use super::{Ctx, rows};
use crate::i18n::tr;
use crate::model::Action;
use crate::theme::Icon;
use egui::style::ScrollAnimation;
use egui::{DragAndDrop, PointerButton, ScrollArea, Sense, vec2};
use std::any::Any;
use std::sync::Arc;

/// While dragging within `EDGE` of the list's top or bottom, it scrolls at `SCROLL` pt/s.
const EDGE: f32 = 48.0;
const SCROLL: f32 = 900.0;

/// The row being dragged: where it was, and its track, to find it again by.
pub struct QueueDrag {
    pub from: usize,
    pub track_id: u64,
}

/// Draws into `ui`, the side panel's: the track rows' painter with covers, no column header
/// and no album column.
pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx) {
    let model = ctx.model; // not borrowed from `ctx`, which the rows take
    let q = &model.queue;
    let tier = tracks::playing_colour(ctx);
    ui.horizontal(|ui| {
        super::widgets::heading(ui, &ctx.palette, &tr("Queue"));
        if !q.tracks.is_empty() {
            ui.weak(tracks::summary(&q.tracks));
        }
    });
    if q.tracks.is_empty() {
        ui.label(tr("The queue is empty"));
        return;
    }
    ScrollArea::vertical()
        .id_salt("queue")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let top = ui.cursor().top();
            rows::virtual_rows(ui, q.tracks.len(), ROW_H, |ui, i| {
                let t = &q.tracks[i];
                let size = vec2(ui.available_width(), ROW_H);
                let (_, row) = ui.allocate_exact_size(size, Sense::click_and_drag());
                let row = tracks::hand(row);
                let num = (i + 1).to_string();
                // By index: a track queued twice plays at one of them.
                let current = (q.current == Some(i)).then_some(tier);
                tracks::paint_row(ui, ctx, &row, num, t, true, false, current);
                if row.drag_started_by(PointerButton::Primary) {
                    let drag = QueueDrag {
                        from: i,
                        track_id: t.id,
                    };
                    DragAndDrop::set_payload(ui.ctx(), drag);
                }
                if t.stream_ready && (row.double_clicked() || tracks::activated(&row)) {
                    ctx.send(Action::PlayQueueIndex(i));
                }
                let p = ctx.palette;
                context_menu(&row, &p, |ui| {
                    let remove = menu_button(&p, Icon::X, tr("Remove"));
                    if ui.add(remove).clicked() {
                        ctx.send(Action::RemoveFromQueue(i));
                        ui.close();
                    }
                });
            });
            if let Some((drag, slot)) = dragging::<QueueDrag>(ui, q.tracks.len(), top)
                && let Some((from, to)) = drop_move(&q.tracks, &drag, slot)
            {
                ctx.send(Action::MoveInQueue { from, to });
            }
        });
}

/// While a row's payload `T` is dragged over the list of `len` rows `ROW_H` tall (scrolled so
/// its top is at `top`): a line where it would land, and scrolling near an edge, or past it.
/// On release over the visible list: the payload and the slot (`0..=len`) it lands in.
/// Released elsewhere (over the heading, a page's header above the list, or the player bar,
/// say), egui drops the payload and nothing moves.
pub fn dragging<T: Any + Send + Sync>(
    ui: &egui::Ui,
    len: usize,
    top: f32,
) -> Option<(Arc<T>, usize)> {
    DragAndDrop::payload::<T>(ui.ctx())?;
    let p = ui.input(|i| i.pointer.interact_pos())?;
    let view = ui.clip_rect();
    if !view.x_range().contains(p.x) {
        return None;
    }
    // Positive moves the content down, showing what is above.
    let dir = if p.y < view.top() + EDGE {
        1.0
    } else if p.y > view.bottom() - EDGE {
        -1.0
    } else {
        0.0
    };
    if dir != 0.0 {
        let step = dir * SCROLL * ui.input(|i| i.stable_dt);
        ui.scroll_with_delta_animation(vec2(0.0, step), ScrollAnimation::none());
        ui.ctx().request_repaint(); // the pointer may rest while the list scrolls
    }
    if !view.y_range().contains(p.y) {
        return None; // the slot under it is out of view
    }
    if p.y < top - ROW_H / 2.0 {
        return None; // above the list, over what the page shows there
    }
    let slot = drop_slot(p.y, top, ROW_H, len);
    // 2 px `accent`: `theme::style` makes the selection stroke that.
    let line = ui.visuals().selection.stroke;
    ui.painter()
        .hline(view.x_range(), top + slot as f32 * ROW_H, line);
    if !ui.input(|i| i.pointer.any_released()) {
        return None;
    }
    Some((DragAndDrop::take_payload::<T>(ui.ctx())?, slot))
}

/// The slot (`0..=len`) a row dropped at `pointer_y` goes to: the row boundary nearest it.
pub fn drop_slot(pointer_y: f32, list_top: f32, row_h: f32, len: usize) -> usize {
    // Above the list the float is negative, which `as` turns into 0.
    (((pointer_y - list_top) / row_h).round() as usize).min(len)
}

/// The `(from, to)` move a drop of `drag` at `slot` makes in `tracks`: `from` found again by
/// the track, as the queue may have changed mid-drag. None onto the row's own edges.
fn drop_move(tracks: &[tidal::Track], drag: &QueueDrag, slot: usize) -> Option<(usize, usize)> {
    let from = if tracks.get(drag.from).is_some_and(|t| t.id == drag.track_id) {
        drag.from // also right when the track is in the queue twice
    } else {
        tracks.iter().position(|t| t.id == drag.track_id)?
    };
    (slot != from && slot != from + 1).then_some((from, slot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Route;
    use crate::ui::{Side, UiState, draw, testing};
    use egui::{CursorIcon, PointerButton, Pos2, pos2};

    #[test]
    fn drop_slot_rounds_and_clamps() {
        let slot = |y| drop_slot(y, 100.0, 26.0, 5);
        assert_eq!(slot(100.0 + 12.0), 0, "upper half of row 0");
        assert_eq!(slot(100.0 + 14.0), 1, "lower half of row 0");
        assert_eq!(slot(100.0 + 3.0 * 26.0 + 1.0), 3, "top edge of row 3");
        assert_eq!(slot(40.0), 0, "above the list");
        assert_eq!(slot(100.0 + 4.6 * 26.0), 5, "lower half of the last row");
        assert_eq!(slot(1000.0), 5, "below the list");
    }

    #[test]
    fn drop_on_own_edges_is_ignored() {
        let list = testing::tracks(5);
        let drag = |from, id| QueueDrag { from, track_id: id };
        assert_eq!(drop_move(&list, &drag(2, 3), 2), None, "its top edge");
        assert_eq!(drop_move(&list, &drag(2, 3), 3), None, "its bottom edge");
        assert_eq!(drop_move(&list, &drag(2, 3), 0), Some((2, 0)));
        assert_eq!(drop_move(&list, &drag(2, 3), 5), Some((2, 5)));
        // The queue changed mid-drag: track 3 is at index 1 now.
        let moved = [list[0].clone(), list[2].clone(), list[1].clone()];
        assert_eq!(
            drop_move(&moved, &drag(2, 3), 2),
            None,
            "own edge, found again"
        );
        assert_eq!(drop_move(&moved, &drag(2, 3), 3), Some((1, 3)));
        assert_eq!(
            drop_move(&moved, &drag(2, 9), 0),
            None,
            "gone from the queue"
        );
    }

    /// Drag the first row onto the top edge of the fourth: `MoveInQueue { from: 0, to: 3 }`,
    /// the hand on the pressed row turning into egui's grab once it drags. Released off the list (beside the panel, or below it over the player bar), a drag moves
    /// nothing. A screen reader's click on a row plays it.
    #[test]
    fn dragging_a_row_moves_it() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState {
            side: Some(Side::Queue),
            ..Default::default()
        };
        let mut model = testing::at(Route::Home);
        model.queue.tracks = testing::tracks(5);
        model.queue.current = Some(0);
        let mut pass = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(1000.0, 600.0),
                )),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                draw(ui, &model, &tx, &mut state);
            });
            out.textures_delta.clear(); // nothing renders them
            out
        };
        let mut sent = || -> Vec<String> {
            std::iter::from_fn(|| rx.try_recv().ok())
                .map(|a| format!("{a:?}"))
                .collect()
        };
        let out = pass(vec![]);
        let label = |n: usize| format!("{n}. Track {n}, Artist 1, 3:20");
        // Just inside row `n`'s top edge, as a screen reader finds the row.
        let top_of = |n: usize| {
            let tree = out.platform_output.accesskit_update.as_ref();
            let (_, node) = tree
                .expect("AccessKit is on")
                .nodes
                .iter()
                .find(|(_, node)| node.label() == Some(label(n).as_str()))
                .unwrap_or_else(|| panic!("row {n} not shown"));
            let b = node.bounds().expect("a row has bounds");
            pos2(b.x0 as f32 + 100.0, b.y0 as f32 + 1.0)
        };
        let (row1, row4) = (top_of(1), top_of(4));
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let drag = |to: Pos2, off: f32| {
            vec![
                vec![egui::Event::PointerMoved(row1), button(row1, true)],
                vec![egui::Event::PointerMoved(row1 + egui::vec2(0.0, 10.0))],
                vec![egui::Event::PointerMoved(to + egui::vec2(off, 0.0))],
                vec![button(to + egui::vec2(off, 0.0), false)],
            ]
        };
        let cursors: Vec<_> = drag(row4, 0.0)
            .into_iter()
            .map(|input| pass(input).platform_output.cursor_icon)
            .collect();
        assert_eq!(sent(), ["MoveInQueue { from: 0, to: 3 }"]);
        let (hand, grab) = (CursorIcon::PointingHand, CursorIcon::Grabbing);
        assert_eq!(cursors[..3], [hand, grab, grab], "pressed, then dragged");
        for input in drag(row4, -500.0) {
            pass(input);
        }
        assert_eq!(sent(), Vec::<String>::new(), "released off the panel");
        for input in drag(pos2(row4.x, 595.0), 0.0) {
            pass(input);
        }
        assert_eq!(sent(), Vec::<String>::new(), "released over the player bar");
        let out = pass(vec![]);
        pass(vec![testing::click(&out, &label(2))]);
        assert_eq!(sent(), ["PlayQueueIndex(1)"]);
    }
}
