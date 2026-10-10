//! TIDAL's look (design system spec §4): the palette, egui's style from it, the type, sizes,
//! radii and durations, the quality tiers' colours, and the album-art tint.

use crate::model::OutputInfo;
use egui::epaint::Shadow;
use egui::style::{HandleShape, ScrollStyle};
use egui::{Color32, CornerRadius, FontId, Stroke, TextStyle, vec2};
use fastframe_fonts::Weight;
use std::collections::HashMap;
use std::sync::OnceLock;

/// The sixteen colours of `fastframe_theme::BASE_COLORS` on a dark or light base (spec §4.1).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Palette {
    pub dark: bool,
    pub window: Color32,
    pub panel: Color32,
    pub surface: Color32,
    pub surface_hover: Color32,
    pub surface_active: Color32,
    pub outline: Color32,
    pub text: Color32,
    pub secondary: Color32,
    pub dim: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub on_accent: Color32,
    pub danger: Color32,
    pub warning: Color32,
    pub overlay: Color32,
    pub shadow: Color32,
}

/// `0xRRGGBBAA` with its alpha not premultiplied, as palette files write `#RRGGBBAA`.
const fn hex(rgba: u32) -> Color32 {
    let [r, g, b, a] = rgba.to_be_bytes();
    Color32::from_rgba_unmultiplied_const(r, g, b, a)
}

impl Palette {
    /// TIDAL's dark mode, its only one.
    pub const fn dark() -> Self {
        Self {
            dark: true,
            window: hex(0x000000ff),
            panel: hex(0x1e1e1eff),
            surface: hex(0x1c1c1cff),
            surface_hover: hex(0xffffff1a),
            surface_active: hex(0xffffff26),
            outline: hex(0xffffff14),
            text: hex(0xffffffff),
            secondary: hex(0xffffff8c),
            dim: hex(0xffffff4d),
            accent: hex(0xffffffff),
            accent_hover: hex(0xe5e5e5ff),
            on_accent: hex(0x000000ff),
            danger: hex(0xff4242ff),
            warning: hex(0xff3269ff),
            overlay: hex(0x2d2d2dff),
            shadow: hex(0x0000004d),
        }
    }

    /// Ours: Dark's rules over white.
    pub const fn light() -> Self {
        Self {
            dark: false,
            window: hex(0xffffffff),
            panel: hex(0xf5f5f5ff),
            surface: hex(0xf0f0f0ff),
            surface_hover: hex(0x0000000f),
            surface_active: hex(0x0000001f),
            outline: hex(0x00000014),
            text: hex(0x000000ff),
            secondary: hex(0x0000008c),
            dim: hex(0x00000059),
            accent: hex(0x000000ff),
            accent_hover: hex(0x333333ff),
            on_accent: hex(0xffffffff),
            danger: hex(0xd92d2dff),
            warning: hex(0xd4145aff),
            overlay: hex(0xffffffff),
            shadow: hex(0x00000026),
        }
    }

    /// `t`'s colour (spec §4.2): not themed, one set per base; Low is `text`.
    pub fn tier(&self, t: Tier) -> Color32 {
        match (t, self.dark) {
            (Tier::Max, true) => hex(0xffd432ff),
            (Tier::Max, false) => hex(0x9c7a00ff),
            (Tier::Lossless, true) => hex(0x33ffeeff),
            (Tier::Lossless, false) => hex(0x00897bff),
            (Tier::Low, _) => self.text,
        }
    }

    /// `art` (a `dominant` colour) at HSL lightness 0.30 on a dark palette, 0.72 on a light
    /// one, its hue and saturation kept (spec §5.9). Opaque.
    pub fn tint(&self, art: Color32) -> Color32 {
        let to = if self.dark { 0.30 } else { 0.72 };
        let [r, g, b, _] = art.to_srgba_unmultiplied().map(|c| f32::from(c) / 255.0);
        let l = (r.max(g).max(b) + r.min(g).min(b)) / 2.0;
        // In HSL a channel is L + C·k(H), chroma C = S·(1 − |2L − 1|): scaling each
        // channel's distance from L by the ratio of the two (1 − |2L − 1|) keeps H and S.
        let room = |l: f32| 1.0 - (2.0 * l - 1.0).abs();
        let scale = if room(l) > 0.0 {
            room(to) / room(l)
        } else {
            0.0
        };
        let [r, g, b] = [r, g, b].map(|c| ((to + (c - l) * scale) * 255.0).round() as u8);
        Color32::from_rgb(r, g, b)
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::dark()
    }
}

/// Palette files set the colours by `BASE_COLORS`' names.
impl fastframe_theme::Palette for Palette {
    fn base(base: fastframe_theme::Base) -> Self {
        match base {
            fastframe_theme::Base::Dark => Self::dark(),
            fastframe_theme::Base::Light => Self::light(),
        }
    }

    fn set(&mut self, name: &str, color: Color32) -> bool {
        let slot = match name {
            "window" => &mut self.window,
            "panel" => &mut self.panel,
            "surface" => &mut self.surface,
            "surface_hover" => &mut self.surface_hover,
            "surface_active" => &mut self.surface_active,
            "outline" => &mut self.outline,
            "text" => &mut self.text,
            "secondary" => &mut self.secondary,
            "dim" => &mut self.dim,
            "accent" => &mut self.accent,
            "accent_hover" => &mut self.accent_hover,
            "on_accent" => &mut self.on_accent,
            "danger" => &mut self.danger,
            "warning" => &mut self.warning,
            "overlay" => &mut self.overlay,
            "shadow" => &mut self.shadow,
            _ => return false,
        };
        *slot = color;
        true
    }
}

/// The stream's quality, as TIDAL colours it (spec §4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Max,
    Lossless,
    Low,
}

impl Tier {
    /// FLAC above 16 bit or 48 kHz is Max, other FLAC Lossless, any other codec Low.
    pub fn of(o: &OutputInfo) -> Tier {
        if !o.codec.eq_ignore_ascii_case("flac") {
            Tier::Low
        } else if o.format.bits > 16 || o.format.rate > 48_000 {
            Tier::Max
        } else {
            Tier::Lossless
        }
    }
}

// Sizes in points (spec §4.4).
pub const SIDEBAR: f32 = 240.0;
pub const SIDEBAR_COLLAPSED: f32 = 78.0;
/// The sidebar's side margins, and the gap above each of its sections.
pub const SIDEBAR_PAD: f32 = 16.0;
pub const TOP_BAR: f32 = 56.0;
pub const PLAYER_BAR: f32 = 88.0;
pub const PLAYER_PAD: f32 = 18.0;
pub const SIDE_PANEL: f32 = 360.0;
pub const SIDE_PANEL_MIN: f32 = 280.0;
pub const SIDE_PANEL_MAX: f32 = 560.0;
pub const PAGE_PAD: f32 = 20.0;
/// Between a page's modules (shelves, header, list).
pub const MODULE_GAP: f32 = 40.0;
/// A track row, and its column header.
pub const ROW: f32 = 48.0;
pub const ROW_HEADER: f32 = 36.0;
pub const INDEX_COL: f32 = 42.0;
pub const COVER_COL: f32 = 42.0;
pub const ROW_COVER: f32 = 40.0;
/// The durations' column: "10:02:05" fits.
pub const TIME_COL: f32 = 64.0;
/// Between a row's columns, and inside its right edge.
pub const COL_GAP: f32 = 12.0;
/// A card's target width (`card_columns`).
pub const CARD: f32 = 175.0;
pub const CARD_GAP: f32 = 16.0;
pub const CARDS_MAX: usize = 8;
pub const HEADER_COVER: f32 = 240.0;
/// The page header's backdrop, and the artist hero, tall.
pub const BACKDROP: f32 = 450.0;
/// The tint behind the top of a page without a cover (spec §5.9).
pub const TINT_HEADER: f32 = 340.0;
pub const PLAYER_COVER: f32 = 52.0;
pub const SIDEBAR_COVER: f32 = 36.0;
pub const ICON_BUTTON: f32 = 32.0;
/// An icon button's glyph, a pill's, a notice's.
pub const GLYPH: f32 = 16.0;
/// Previous, Next and Play/Pause's hit box.
pub const TRANSPORT: f32 = 40.0;
/// Previous and Next's glyphs; Play/Pause's is larger (ruling 26).
pub const SKIP_GLYPH: f32 = 22.0;
pub const PLAY_GLYPH: f32 = 24.0;
/// A pill's height.
pub const PILL: f32 = 40.0;
pub const SEARCH_W: f32 = 280.0;
/// Settings' label column (ruling 30), and a text field there at its widest.
pub const SETTINGS_LABEL: f32 = 180.0;
pub const SETTINGS_FIELD: f32 = 280.0;
/// The search pill at its narrowest: its icon and a word. With less room it reaches over the
/// history pill, never the gear.
pub const SEARCH_MIN: f32 = 120.0;

// Corner radii; pills and discs are fully round.
pub const R_ART: f32 = 2.0;
pub const R_ROW: f32 = 5.0;
pub const R_BADGE: f32 = 4.0;
/// Fields and stock widgets.
pub const R_FIELD: f32 = 8.0;
/// Nav rows, menus, card hover.
pub const R_CARD: f32 = 12.0;
pub const R_DIALOG: f32 = 20.0;

/// The shade a hovered card's cover darkens to: black at 70%, on any palette (spec §5.6).
pub const SCRIM: Color32 = Color32::from_black_alpha(179);

// Durations in seconds (spec §4.5).
/// Hover colour; egui's `animation_time`, so the sidebar's slide too.
pub const HOVER: f32 = 0.15;
/// The card cover's overlay fade.
pub const OVERLAY: f32 = 0.3;
/// The player bar's tint, per channel.
pub const TINT_FADE: f32 = 0.45;
/// The cover size the tint is taken from, in pixels.
pub const TINT_PX: u32 = 16;

// Type (spec §4.3): Inter at `fastframe_fonts`' weights.
/// 12 Regular: times, badge, column headers, subtitles in the sidebar and player.
pub fn small() -> FontId {
    Weight::Regular.font_id(12.0)
}

/// 14 Regular: secondary lines, settings, card subtitles.
pub fn body() -> FontId {
    Weight::Regular.font_id(14.0)
}

/// 14 Medium: track, card and sidebar playlist titles.
pub fn body_strong() -> FontId {
    Weight::Medium.font_id(14.0)
}

/// 14 SemiBold: nav rows, the player's title, pill labels.
pub fn label() -> FontId {
    Weight::SemiBold.font_id(14.0)
}

/// 16 SemiBold: a synced lyrics line (spec §5.8).
pub fn lyric() -> FontId {
    Weight::SemiBold.font_id(16.0)
}

/// 19 Bold: shelf, side panel and settings section titles.
pub fn heading() -> FontId {
    Weight::Bold.font_id(19.0)
}

/// 32 Bold: a page header's title.
pub fn title() -> FontId {
    Weight::Bold.font_id(32.0)
}

/// How the desktop renders text, read once (`fastframe_text::detect`). Tests take the
/// platform's default instead of asking the desktop (D-Bus on Linux).
pub fn rendering() -> fastframe_text::TextRendering {
    static RENDERING: OnceLock<fastframe_text::TextRendering> = OnceLock::new();
    *RENDERING.get_or_init(|| {
        if cfg!(test) {
            fastframe_text::TextRendering::platform_default()
        } else {
            fastframe_text::detect()
        }
    })
}

/// `style` on both of egui's themes, then the text rendering again: new visuals reset it.
pub fn apply(ctx: &egui::Context, p: &Palette) {
    ctx.all_styles_mut(|s| {
        style(s, p);
        rendering().apply_to_visuals(&mut s.visuals);
    });
}

/// egui's style in `p`'s colours (spec §4.6), for stock widgets, menus, dialogs and text.
pub fn style(s: &mut egui::Style, p: &Palette) {
    let v = &mut s.visuals;
    *v = if p.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.panel_fill = p.window;
    v.window_fill = p.overlay;
    v.extreme_bg_color = p.surface;
    v.faint_bg_color = p.surface;
    v.code_bg_color = p.surface;
    v.override_text_color = Some(p.text);
    v.weak_text_color = Some(p.secondary);
    v.hyperlink_color = p.text;
    v.warn_fg_color = p.warning;
    v.error_fg_color = p.danger;
    // The stroke is egui's focus outline.
    v.selection.bg_fill = p.accent.gamma_multiply(0.25);
    v.selection.stroke = Stroke::new(2.0, p.accent);
    v.text_cursor.stroke.color = p.accent;
    v.window_stroke = Stroke::new(1.0, p.outline);
    v.window_corner_radius = CornerRadius::same(R_DIALOG as u8);
    v.menu_corner_radius = CornerRadius::same(R_CARD as u8);
    let shadow = Shadow {
        offset: [0, 4],
        blur: 20,
        spread: 0,
        color: p.shadow,
    };
    v.window_shadow = shadow;
    v.popup_shadow = shadow;
    v.slider_trailing_fill = true;
    v.handle_shape = HandleShape::Circle;
    // Hover and press fills over `surface`, opaque: Dark's are translucent, and on their own
    // would read darker than the idle fill. Glyphs (checks, arrows, the scroll handle) are
    // `secondary` at rest, `text` otherwise.
    let w = &mut v.widgets;
    let hovered = p.surface.blend(p.surface_hover);
    let active = p.surface.blend(p.surface_active);
    let states = [
        (&mut w.inactive, p.surface, p.secondary),
        (&mut w.hovered, hovered, p.text),
        (&mut w.active, active, p.text),
        (&mut w.open, active, p.text),
    ];
    for (state, fill, glyph) in states {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::NONE;
        state.fg_stroke.color = glyph;
        state.corner_radius = CornerRadius::same(R_FIELD as u8);
        state.expansion = 0.0;
    }
    w.noninteractive.corner_radius = CornerRadius::same(R_FIELD as u8);
    w.noninteractive.bg_stroke.color = p.outline; // separators

    s.text_styles = [
        (TextStyle::Small, small()),
        (TextStyle::Body, body()),
        (TextStyle::Button, body()),
        (TextStyle::Heading, heading()),
        (TextStyle::Monospace, FontId::monospace(13.0)),
    ]
    .into();
    let sp = &mut s.spacing;
    sp.item_spacing = vec2(8.0, 8.0);
    sp.button_padding = vec2(16.0, 8.0);
    sp.interact_size = vec2(40.0, 32.0);
    sp.menu_margin = egui::Margin::same(4);
    sp.slider_rail_height = 4.0;
    // Solid: floating bars fade in and out, repainting while idle. The handle takes the
    // widgets' glyph colour; solid bars ignore the opacities, and their track is
    // `extreme_bg_color`.
    sp.scroll = ScrollStyle {
        foreground_color: true,
        ..ScrollStyle::solid()
    };
    s.animation_time = HOVER;
}

/// `a` moved `t` of the way to `b`, per channel, alpha too.
pub fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    a.lerp_to_gamma(b, t)
}

/// The colour a cover reads as (spec §5.9, as Spotifast): pixels bucketed by 4-bit RGB, each
/// weighing `(1 + 6·saturation) · max(0.05, 1 − 1.4·|lightness − 0.5|)` so vivid mid-tones
/// beat borders and lettering; the heaviest bucket's weighted mean. `None` when every pixel
/// is transparent.
pub fn dominant(image: &egui::ColorImage) -> Option<Color32> {
    let mut buckets: HashMap<[u8; 3], (f32, [f32; 3])> = HashMap::new();
    for pixel in image.pixels.iter().filter(|p| p.a() > 0) {
        let [r, g, b, _] = pixel.to_srgba_unmultiplied();
        let (max, min) = (f32::from(r.max(g).max(b)), f32::from(r.min(g).min(b)));
        let saturation = if max > 0.0 { (max - min) / max } else { 0.0 };
        let lightness = (max + min) / 510.0;
        let weight = (1.0 + 6.0 * saturation) * (1.0 - 1.4 * (lightness - 0.5).abs()).max(0.05);
        let (total, sums) = buckets.entry([r >> 4, g >> 4, b >> 4]).or_default();
        *total += weight;
        for (sum, c) in sums.iter_mut().zip([r, g, b]) {
            *sum += weight * f32::from(c);
        }
    }
    let (total, sums) = buckets.into_values().max_by(|a, b| a.0.total_cmp(&b.0))?;
    let [r, g, b] = sums.map(|sum| (sum / total).round() as u8);
    Some(Color32::from_rgb(r, g, b))
}

/// How many cards a row `width` wide holds, and each one's width (spec §4.4): as many as
/// fit at `CARD` wide, 1 to `CARDS_MAX`, sharing the width less the gaps.
pub fn card_columns(width: f32) -> (usize, f32) {
    let width = width.max(0.0); // NaN too
    let n = ((width / CARD) as usize).clamp(1, CARDS_MAX);
    (n, (width - (n - 1) as f32 * CARD_GAP) / n as f32)
}

fastframe_icons::icons! {
    /// Every icon the interface draws: Lucide's shared set from fastframe-icons, and the app's
    /// own Lucide files in `assets/icons/` (licences in `LICENSE.txt` there). All are drawn in
    /// white, so the tint is the colour on screen.
    pub enum Icon {
        prefix: "tidash-icon-",
        directory: "../assets/icons/",
        ChevronLeft => lucide "chevron-left",
        ChevronRight => lucide "chevron-right",
        ChevronDown => lucide "chevron-down",
        Search => lucide "search",
        Settings => lucide "settings",
        Info => lucide "info",
        VolumeX => lucide "volume-x",
        Volume2 => lucide "volume-2",
        X => lucide "x",
        CircleAlert => lucide "circle-alert",
        User => lucide "user",
        Clock => lucide "clock",
        PanelLeft => lucide "panel-left",
        House => "house",
        Compass => "compass",
        ListMusic => "list-music",
        ListPlus => "list-plus",
        Disc3 => "disc-3",
        Music => "music",
        Radio => "radio",
        AudioLines => "audio-lines",
        Heart => "heart",
        HeartFilled => "heart-filled",
        PlayFilled => "play-filled",
        PauseFilled => "pause-filled",
        SkipBackFilled => "skip-back-filled",
        SkipForwardFilled => "skip-forward-filled",
        Shuffle => "shuffle",
        Repeat => "repeat",
        Repeat1 => "repeat-1",
        Volume1 => "volume-1",
        MicVocal => "mic-vocal",
        FolderPlus => "folder-plus",
        Folder => "folder",
        FolderOpen => "folder-open",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::ColorImage;
    use fastframe_theme::{BASE_COLORS, parse_palette};

    /// Each name sets its own colour; any other name fails the file. A file restating a
    /// built-in's colour gives the built-in.
    #[test]
    fn every_base_colour_is_settable() {
        let same = parse_palette(r##"{"colors":{"surface_hover":"#ffffff1a"}}"##);
        assert_eq!(same, Ok(Palette::dark()));
        let mut seen = vec![Palette::dark()];
        for name in BASE_COLORS {
            let file = format!(r##"{{"colors":{{"{name}":"#123456"}}}}"##);
            let p: Palette = parse_palette(&file).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!seen.contains(&p), "{name} sets nothing new");
            seen.push(p);
        }
        let chat = parse_palette::<Palette>(r##"{"colors":{"chat":"#123456"}}"##);
        assert!(chat.is_err());
    }

    #[test]
    fn presets_parse() {
        let themes: Vec<_> = fastframe_theme::presets::themes::<Palette>().collect();
        assert_eq!(themes.len(), 8);
        let nord = themes.iter().find(|t| t.filename == "Nord.json");
        let nord = nord.expect("Nord ships").palette;
        assert_eq!(nord.window, Color32::from_rgb(0x2e, 0x34, 0x40));
    }

    #[test]
    fn apply_maps_the_palette() {
        for p in [Palette::light(), Palette::dark()] {
            let mut s = egui::Style::default();
            style(&mut s, &p);
            let v = &s.visuals;
            assert_eq!(v.dark_mode, p.dark);
            assert_eq!(v.panel_fill, p.window);
            assert_eq!(v.window_fill, p.overlay);
            assert_eq!(v.override_text_color, Some(p.text));
            assert!(v.widgets.hovered.bg_fill.is_opaque(), "{p:?}");
            assert_eq!(s.text_styles[&egui::TextStyle::Heading].size, 19.0);
            assert!(!s.spacing.scroll.floating);
            assert_eq!(s.animation_time, HOVER);
        }
        let ctx = egui::Context::default();
        let p = Palette::light();
        apply(&ctx, &p);
        let v = &ctx.global_style().visuals;
        assert_eq!(v.panel_fill, p.window);
        let kept = rendering().color_transfer_function(false);
        assert_eq!(v.text_options.color_transfer_function, kept);
    }

    #[test]
    fn tiers() {
        let output = |codec: &str, bits, rate| OutputInfo {
            codec: codec.into(),
            format: player::Format {
                rate,
                bits,
                channels: 2,
            },
            exclusive: false,
            device_rate: rate,
        };
        assert_eq!(Tier::of(&output("AAC", 16, 44_100)), Tier::Low);
        assert_eq!(Tier::of(&output("FLAC", 16, 44_100)), Tier::Lossless);
        assert_eq!(Tier::of(&output("flac", 24, 96_000)), Tier::Max);
        assert_eq!(Tier::of(&output("FLAC", 16, 96_000)), Tier::Max);
        let (dark, light) = (Palette::dark(), Palette::light());
        assert_eq!(dark.tier(Tier::Max), Color32::from_rgb(0xff, 0xd4, 0x32));
        assert_eq!(
            light.tier(Tier::Lossless),
            Color32::from_rgb(0x00, 0x89, 0x7b)
        );
        assert_eq!(light.tier(Tier::Low), light.text);
    }

    /// HSL lightness, 0 to 1.
    fn lightness(c: Color32) -> f32 {
        let [r, g, b, _] = c.to_array();
        (f32::from(r.max(g).max(b)) + f32::from(r.min(g).min(b))) / 510.0
    }

    /// A grey ramp over four of `dominant`'s buckets, as a cover's background spreads: one
    /// even grey would outweigh a 4×4 patch (240 pixels at weight 1 against 16 at about 6).
    #[test]
    fn dominant_colour_prefers_the_vivid_patch() {
        let red = Color32::from_rgb(0xe0, 0x20, 0x20);
        let pixels = (0..16u8).flat_map(|y| {
            (0..16).map(move |x| {
                let patch = (6..10).contains(&x) && (6..10).contains(&y);
                if patch {
                    red
                } else {
                    Color32::from_gray(0x60 + 4 * y)
                }
            })
        });
        let image = ColorImage::new([16, 16], pixels.collect());
        let [r, g, b, _] = dominant(&image).expect("a colour").to_array();
        assert!(r > g + 100 && r > b + 100, "{r} {g} {b}");
        let clear = ColorImage::filled([16, 16], Color32::TRANSPARENT);
        assert_eq!(dominant(&clear), None);
    }

    #[test]
    fn tint_reaches_its_lightness() {
        let red = Color32::from_rgb(0xe0, 0x20, 0x20);
        for (p, want) in [(Palette::dark(), 0.30), (Palette::light(), 0.72)] {
            let tint = p.tint(red);
            assert!((lightness(tint) - want).abs() <= 0.02, "{tint:?}");
            let [r, g, b, _] = tint.to_array();
            assert!(r > g && g == b, "the hue stays: {tint:?}");
        }
        assert_eq!(
            blend(Color32::BLACK, Color32::WHITE, 0.5),
            Color32::from_gray(128)
        );
    }

    #[test]
    fn card_columns_clamp() {
        assert_eq!(card_columns(0.0), (1, 0.0));
        assert_eq!(card_columns(100.0), (1, 100.0));
        assert_eq!(card_columns(CARD * 3.0 + 2.0 * CARD_GAP), (3, CARD));
        assert_eq!(card_columns(5000.0).0, CARDS_MAX);
        assert_eq!(card_columns(-5.0), (1, 0.0));
        assert_eq!(card_columns(f32::NAN), (1, 0.0));
    }
}
