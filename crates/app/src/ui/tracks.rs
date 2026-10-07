//! Virtualized track list shared by the search and album views.

use crate::i18n::tr;
use crate::model::Action;
use egui::{Align2, FontId, Sense, vec2};
use tokio::sync::mpsc::UnboundedSender;

const ROW_H: f32 = 26.0;

enum Row {
    Disc(u32),
    Track(usize),
}

pub fn duration(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

pub fn artists(t: &tidal::Track) -> String {
    t.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")
}

/// Double-clicking an available row sends `PlayTracks` for the whole list. With `discs`,
/// multi-disc lists get a header row per disc; otherwise the album title is a column.
pub fn show(ui: &mut egui::Ui, id: &str, tracks: &[tidal::Track], discs: bool, tx: &UnboundedSender<Action>) {
    let multi = discs && tracks.iter().any(|t| t.volume_number > 1);
    let mut rows = Vec::with_capacity(tracks.len() + 4);
    let mut disc = 0;
    for (i, t) in tracks.iter().enumerate() {
        if multi && t.volume_number != disc {
            disc = t.volume_number;
            rows.push(Row::Disc(disc));
        }
        rows.push(Row::Track(i));
    }
    egui::ScrollArea::vertical().id_salt(id).auto_shrink([false, false]).show_rows(
        ui,
        ROW_H,
        rows.len(),
        |ui, range| {
            for row in &rows[range] {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                let p = ui.painter_at(rect);
                let font = FontId::proportional(14.0);
                match *row {
                    Row::Disc(n) => {
                        p.text(
                            rect.left_center(),
                            Align2::LEFT_CENTER,
                            format!("{} {n}", tr("Disc")),
                            font,
                            ui.visuals().strong_text_color(),
                        );
                    }
                    Row::Track(i) => {
                        let t = &tracks[i];
                        let ready = t.stream_ready;
                        if resp.hovered() {
                            p.rect_filled(rect, 3.0, ui.visuals().widgets.hovered.bg_fill);
                        }
                        let color = if ready { ui.visuals().text_color() } else { ui.visuals().weak_text_color() };
                        let w = rect.width();
                        // Text in [x0, x1) from the left edge, clipped so columns never overlap.
                        let cell = |x0: f32, x1: f32, text: String, align: Align2| {
                            let clip = egui::Rect::from_x_y_ranges(
                                (rect.left() + x0)..=(rect.left() + x1 - 6.0),
                                rect.y_range(),
                            );
                            let (x, a) = if align == Align2::RIGHT_CENTER { (clip.right(), align) } else { (clip.left(), align) };
                            ui.painter_at(clip).text(egui::pos2(x, rect.center().y), a, text, font.clone(), color);
                        };
                        let num = if discs { t.track_number.to_string() } else { (i + 1).to_string() };
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
                        if ready && resp.double_clicked() {
                            let _ = tx.send(Action::PlayTracks { tracks: tracks.to_vec(), start: i });
                        }
                    }
                }
            }
        },
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn duration_is_m_ss() {
        assert_eq!(super::duration(0), "0:00");
        assert_eq!(super::duration(65), "1:05");
        assert_eq!(super::duration(3600), "60:00");
    }
}
