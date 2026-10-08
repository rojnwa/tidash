//! Queue panel: the play queue, its current track highlighted. Rows are dragged with egui's
//! bare `DragAndDrop` payload: its drop zones give no insertion index and need every row
//! built, and these rows are virtual.

use super::{rows, tracks};
use crate::i18n::tr;
use crate::model::{Action, Model};
use egui::style::ScrollAnimation;
use egui::{DragAndDrop, PointerButton, ScrollArea, Sense, Stroke, vec2};
use tokio::sync::mpsc::UnboundedSender;

const ROW_H: f32 = 26.0;
/// While dragging within `EDGE` of the list's top or bottom, it scrolls at `SCROLL` pt/s.
const EDGE: f32 = 48.0;
const SCROLL: f32 = 900.0;

/// The row being dragged: where it was, and its track, to find it again by.
pub struct QueueDrag {
    pub from: usize,
    pub track_id: u64,
}

pub fn show(ui: &mut egui::Ui, model: &Model, tx: &UnboundedSender<Action>) {
    let q = &model.queue;
    let send = |a: Action| {
        let _ = tx.send(a);
    };
    egui::Panel::right("queue")
        .resizable(true)
        .default_size(320.0)
        .size_range(260.0..=560.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(tr("Queue"));
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
                        let num = (i + 1).to_string();
                        tracks::paint_row(ui, &row, num, t, false, q.current == Some(i));
                        if row.drag_started_by(PointerButton::Primary) {
                            let drag = QueueDrag {
                                from: i,
                                track_id: t.id,
                            };
                            DragAndDrop::set_payload(ui.ctx(), drag);
                        }
                        if t.stream_ready && (row.double_clicked() || tracks::activated(&row)) {
                            send(Action::PlayQueueIndex(i));
                        }
                        row.context_menu(|ui| {
                            if ui.button(tr("Remove")).clicked() {
                                send(Action::RemoveFromQueue(i));
                                ui.close();
                            }
                        });
                    });
                    if let Some((from, to)) = dragging(ui, &q.tracks, top) {
                        send(Action::MoveInQueue { from, to });
                    }
                });
        });
}

/// While a row is dragged over the list (scrolled so its top is at `top`): a line where it
/// would land, and scrolling near an edge. On release there: the move to make, if any.
/// Released elsewhere, egui drops the payload and nothing moves.
fn dragging(ui: &egui::Ui, tracks: &[tidal::Track], top: f32) -> Option<(usize, usize)> {
    DragAndDrop::payload::<QueueDrag>(ui.ctx())?;
    let p = ui.input(|i| i.pointer.interact_pos())?;
    let view = ui.clip_rect();
    if !view.x_range().contains(p.x) {
        return None;
    }
    let slot = drop_slot(p.y, top, ROW_H, tracks.len());
    // The accent's stroke, which shows on the current row's highlight too.
    let line = Stroke::new(2.0, ui.visuals().selection.stroke.color);
    ui.painter()
        .hline(view.x_range(), top + slot as f32 * ROW_H, line);
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
    if !ui.input(|i| i.pointer.any_released()) {
        return None;
    }
    let drag = DragAndDrop::take_payload::<QueueDrag>(ui.ctx())?;
    drop_move(tracks, &drag, slot)
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
    use crate::ui::{UiState, draw, testing};
    use egui::{PointerButton, Pos2, pos2};

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

    /// Drag the first row onto the top edge of the fourth: `MoveInQueue { from: 0, to: 3 }`.
    /// Released off the panel, a drag moves nothing. A screen reader's click on a row plays it.
    #[test]
    fn dragging_a_row_moves_it() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState {
            queue_open: true,
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
        for input in drag(row4, 0.0) {
            pass(input);
        }
        assert_eq!(sent(), ["MoveInQueue { from: 0, to: 3 }"]);
        for input in drag(row4, -500.0) {
            pass(input);
        }
        assert_eq!(sent(), Vec::<String>::new(), "released off the panel");
        let out = pass(vec![]);
        pass(vec![testing::click(&out, &label(2))]);
        assert_eq!(sent(), ["PlayQueueIndex(1)"]);
    }
}
