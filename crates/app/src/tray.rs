//! The tray item (fastframe-tray): its menu, its drawn icon, and what its clicks ask for.

use crate::i18n::tr;
use crate::model::Action;
use fastframe_tray::{Config, Event, MenuItem};
use std::borrow::Cow;

const SHOW: &str = "show";
pub const PLAY_PAUSE: &str = "play-pause";
const NEXT: &str = "next";
const PREVIOUS: &str = "previous";
const QUIT: &str = "quit";

/// What the tray, the desktop's media controls, or a later launch (`instance`) ask of the app.
#[derive(Debug)]
pub enum TrayAction {
    /// Bring the window up, making one if there is none.
    Show,
    /// Bring the window up and open this link once logged in.
    Open(crate::link::Link),
    /// Make a window if there is none, else close it to the tray.
    Toggle,
    /// For the backend.
    Play(Action),
    Quit,
}

/// The item: Tidash's mark, and a menu that shows or hides the window, controls playback
/// and quits.
pub fn config() -> Config {
    Config {
        id: "tidash",
        title: "Tidash".into(),
        icon,
        template_icon: None,
        themed_icon: true,
        menu_on_click: false,
        menu: vec![
            MenuItem::action(SHOW, tr("Show or hide Tidash")),
            MenuItem::Separator,
            MenuItem::action(PLAY_PAUSE, play_pause_label(false)),
            MenuItem::action(NEXT, tr("Next")),
            MenuItem::action(PREVIOUS, tr("Previous")),
            MenuItem::Separator,
            MenuItem::action(QUIT, tr("Quit")),
        ],
    }
}

/// The Play/Pause entry's label while playing or not.
pub fn play_pause_label(playing: bool) -> Cow<'static, str> {
    if playing { tr("Pause") } else { tr("Play") }
}

/// A teal disc with a wave cut out below its middle, `size` pixels square, as RGBA.
// ponytail: drawn mark; the real icon comes with packaging (M5)
pub fn icon(size: usize) -> Vec<u8> {
    let half = size as f32 / 2.0;
    let r = half - 1.0;
    let mut rgba = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x as f32 + 0.5 - half, y as f32 + 0.5 - half);
            // Each edge blends over one pixel.
            let disc = (r - dx.hypot(dy) + 0.5).clamp(0.0, 1.0);
            let wave = r * (0.3 + 0.25 * (dx / r * std::f32::consts::PI * 1.5).cos());
            let cut = ((dy - wave).abs() - r * 0.14 + 0.5).clamp(0.0, 1.0);
            let alpha = (disc * cut * 255.0).round() as u8;
            rgba.extend_from_slice(&[0x1f, 0xb5, 0xc9, alpha]);
        }
    }
    rgba
}

pub fn action_for(e: &Event) -> TrayAction {
    match *e {
        Event::Show => TrayAction::Show,
        Event::Menu(PLAY_PAUSE) => TrayAction::Play(Action::TogglePlay),
        Event::Menu(NEXT) => TrayAction::Play(Action::Next),
        Event::Menu(PREVIOUS) => TrayAction::Play(Action::Previous),
        Event::Menu(QUIT) => TrayAction::Quit,
        // The menu's one other entry is Show or hide.
        Event::Toggle | Event::Menu(_) => TrayAction::Toggle,
    }
}

/// Whether closing the window keeps the app running in the tray: only with the setting on and
/// a tray the desktop shows (none on stock GNOME, where closing quits).
pub fn hides_to_tray(tray_shown: bool, close_to_tray: bool) -> bool {
    tray_shown && close_to_tray
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_events_map_to_actions() {
        let act = |e| format!("{:?}", action_for(&e));
        assert_eq!(act(Event::Show), "Show");
        assert_eq!(act(Event::Toggle), "Toggle");
        let ids = |config: Config| {
            let ids = config.menu.into_iter().filter_map(|item| match item {
                MenuItem::Action { id, .. } => Some(id),
                MenuItem::Separator => None,
            });
            ids.map(|id| act(Event::Menu(id))).collect::<Vec<_>>()
        };
        let want = [
            "Toggle",
            "Play(TogglePlay)",
            "Play(Next)",
            "Play(Previous)",
            "Quit",
        ];
        assert_eq!(ids(config()), want, "the menu, top to bottom");
    }

    #[test]
    fn hides_to_tray_needs_a_shown_tray_and_the_setting() {
        assert!(hides_to_tray(true, true));
        assert!(!hides_to_tray(false, true), "no tray host: closing quits");
        assert!(!hides_to_tray(true, false));
        assert!(!hides_to_tray(false, false));
    }

    #[test]
    fn icon_is_square_rgba() {
        for size in [16, 32, 64] {
            let px = icon(size);
            assert_eq!(px.len(), size * size * 4, "{size}");
            let alpha = |x: usize, y: usize| px[(y * size + x) * 4 + 3];
            assert_eq!(alpha(size / 2, size / 2), 255, "{size}: opaque centre");
            assert_eq!(alpha(0, 0), 0, "{size}: transparent corner");
            let cut = (3..size - 3).any(|y| alpha(size / 2, y) < 128);
            assert!(cut, "{size}: the wave cuts the disc");
        }
    }
}
