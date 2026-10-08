//! Rows built only where they show, inside the caller's scroll: a list of 10 000 tracks costs
//! what a screenful does, and shares the page's one scroll with what is above and below it.

use egui::{Rangef, UiBuilder, vec2};
use std::ops::Range;

/// Calls `f(ui, i)` for the rows of `count` that meet the clip rect, plus one either side, so
/// the keyboard can move the focus past the edge; spacers stand in for the others. Each row
/// must be `row_h` tall. A row's ids depend on its index, not on what else is built, so a
/// focused row keeps its focus while the list scrolls (as long as it stays built).
pub fn virtual_rows(
    ui: &mut egui::Ui,
    count: usize,
    row_h: f32,
    mut f: impl FnMut(&mut egui::Ui, usize),
) {
    // One scope: whatever is built, the caller's later widgets keep their ids.
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let range = visible(ui.cursor().top(), ui.clip_rect().y_range(), row_h, count);
        let (width, id) = (ui.available_width(), ui.unique_id());
        ui.allocate_space(vec2(width, range.start as f32 * row_h));
        for i in range.clone() {
            ui.scope_builder(UiBuilder::new().id(id.with(i)), |ui| f(ui, i));
        }
        ui.allocate_space(vec2(width, (count - range.end) as f32 * row_h));
    });
}

/// The rows of `count`, `row_h` tall from `top`, that meet `clip`, and one either side.
fn visible(top: f32, clip: Rangef, row_h: f32, count: usize) -> Range<usize> {
    let row = |y: f32| ((y - top) / row_h).max(0.0);
    let first = (row(clip.min).floor() as usize)
        .min(count)
        .saturating_sub(1);
    let last = (row(clip.max).ceil() as usize).saturating_add(1).min(count);
    first..last.max(first)
}

#[cfg(test)]
mod tests {
    use egui::{Pos2, Rect, ScrollArea, Sense, vec2};

    /// 10 000 rows of 26 pt in a 600 pt viewport scrolled to the middle of row 5000: 24 rows
    /// show (5000 to 5023), and one either side is built, each where it belongs.
    #[test]
    fn virtual_rows_only_build_visible_indices() {
        let ctx = egui::Context::default();
        let offset = 5000.0 * 26.0 + 13.0;
        let mut built = Vec::new();
        for _ in 0..2 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                built.clear();
                ScrollArea::vertical()
                    .vertical_scroll_offset(offset)
                    .show(ui, |ui| {
                        super::virtual_rows(ui, 10_000, 26.0, |ui, i| {
                            let size = vec2(ui.available_width(), 26.0);
                            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                            built.push((i, rect.top()));
                        });
                    });
            });
            out.textures_delta.clear(); // nothing renders them
        }
        assert!(!built.is_empty() && built.len() <= 26, "{built:?}");
        for &(i, top) in &built {
            assert!((4999..=5024).contains(&i), "{built:?}");
            // The viewport's top edge is y = 0, half way down row 5000.
            let want = (i as f32 - 5000.0) * 26.0 - 13.0;
            assert!((top - want).abs() < 0.5, "row {i} at {top}, not {want}");
        }
    }
}
