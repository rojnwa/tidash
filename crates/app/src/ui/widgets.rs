//! The controls stock egui cannot draw in TIDAL's style (spec §3.1). Each allocates, paints
//! from the palette, names itself to AccessKit, shows that name as a tooltip where it shows no
//! text, and draws the focus ring.

use crate::theme::{self, GLYPH, Icon, Palette};
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align, Color32, CursorIcon, FontId, Galley, Layout, Mesh, Pos2, Rect, Response, RichText,
    Sense, Stroke, StrokeKind, TextEdit, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType, pos2, vec2,
};
use std::sync::Arc;

/// A nav row's inset around its icon and text; the library's rows share it.
pub(super) const NAV_PAD: f32 = 8.0;
const NAV_ICON: f32 = 24.0;
/// A nav row's height.
pub(super) const NAV_ROW: f32 = NAV_ICON + 2.0 * NAV_PAD;

/// `icon` at `size` points square, centred on `center`, tinted `tint`.
pub(super) fn paint_icon(ui: &Ui, icon: Icon, center: Pos2, size: f32, tint: Color32) {
    let rect = Rect::from_center_size(center, Vec2::splat(size));
    icon.image(tint, size).paint_at(ui, rect);
}

/// The scale a disc or icon button draws at (spec §4.5): 1.1 hovered, 0.9 pressed, eased
/// over `HOVER`. At rest it stays 1 and asks for no repaint.
// ponytail: the glyph rasterizes again at each size the animation passes through (a 16 px
// SVG, about 9 frames); paint one texture scaled if that ever shows in a profile.
fn scale(ui: &Ui, r: &Response) -> f32 {
    let target = if r.is_pointer_button_down_on() {
        0.9
    } else if r.hovered() {
        1.1
    } else {
        1.0
    };
    let id = r.id.with("scale");
    ui.ctx().animate_value_with_time(id, target, theme::HOVER)
}

/// 0 to 1 as `on` turns on, over `HOVER`.
pub(super) fn fade(ui: &Ui, r: &Response, on: bool) -> f32 {
    ui.ctx().animate_bool_with_time(r.id, on, theme::HOVER)
}

/// `text` on one line in `font`, cut with "…" past `width`.
pub(super) fn line(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = TextWrapping::truncate_at_width(width);
    ui.painter().layout_job(job)
}

/// The keyboard focus, as a 2 px `accent` ring 2 outside `response` (egui's selection
/// stroke, which `theme::style` makes that). `radius` is the control's own; the ring's is 2
/// more. A control the focus reaches off screen scrolls into view.
pub fn focus_ring(ui: &Ui, response: &Response, radius: f32) {
    if response.has_focus() {
        let rect = response.rect.expand(2.0);
        let stroke = ui.visuals().selection.stroke;
        ui.painter()
            .rect_stroke(rect, radius + 2.0, stroke, StrokeKind::Outside);
    }
    if response.gained_focus() {
        response.scroll_to_me(None);
    }
}

/// A 32 px square button with a 16 px glyph: `secondary`, `text` when hovered. `toggle` is
/// `None` for a plain button; `Some(on)` makes it a toggle that screen readers hear as on or
/// off, and one that is on shows `text` on a `surface_active` disc.
pub fn icon_button(
    ui: &mut Ui,
    p: &Palette,
    icon: Icon,
    label: &str,
    toggle: Option<bool>,
) -> Response {
    let size = Vec2::splat(theme::ICON_BUTTON);
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    r.widget_info(|| {
        let enabled = ui.is_enabled();
        match toggle {
            Some(on) => WidgetInfo::selected(WidgetType::Button, enabled, on, label),
            None => WidgetInfo::labeled(WidgetType::Button, enabled, label),
        }
    });
    let active = toggle == Some(true);
    if ui.is_rect_visible(rect) {
        let s = scale(ui, &r);
        if active {
            let radius = rect.width() / 2.0 * s;
            ui.painter()
                .circle_filled(rect.center(), radius, p.surface_active);
        }
        let tint = theme::blend(p.secondary, p.text, fade(ui, &r, r.hovered() || active));
        paint_icon(ui, icon, rect.center(), GLYPH * s, tint);
    }
    focus_ring(ui, &r, rect.width() / 2.0);
    r.on_hover_text(label)
        .on_hover_cursor(CursorIcon::PointingHand)
}

/// A `fill` disc `diameter` across with `icon` in `glyph` at 0.55 of it. Hovered, an `accent`
/// disc turns `accent_hover`; any other takes `surface_hover` over its fill.
pub fn circle_button(
    ui: &mut Ui,
    p: &Palette,
    icon: Icon,
    label: &str,
    diameter: f32,
    fill: Color32,
    glyph: Color32,
) -> Response {
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::click());
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label));
    if ui.is_rect_visible(rect) {
        let s = scale(ui, &r);
        let hovered = if fill == p.accent {
            p.accent_hover
        } else {
            fill.blend(p.surface_hover)
        };
        let fill = theme::blend(fill, hovered, fade(ui, &r, r.hovered()));
        let painter = ui.painter();
        painter.circle_filled(rect.center(), diameter / 2.0 * s, fill);
        paint_icon(ui, icon, rect.center(), diameter * 0.55 * s, glyph);
    }
    focus_ring(ui, &r, diameter / 2.0);
    r.on_hover_text(label)
        .on_hover_cursor(CursorIcon::PointingHand)
}

/// A bare `glyph`-point `icon` in `text` on a `TRANSPORT` square, the player's transport
/// (rulings 8, 27): no disc, hovered or pressed either. Hover and press scale the glyph;
/// it is `text` at rest already, as TIDAL's are, so there is no brighter ink to turn.
pub fn glyph_button(ui: &mut Ui, p: &Palette, icon: Icon, label: &str, glyph: f32) -> Response {
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(theme::TRANSPORT), Sense::click());
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label));
    if ui.is_rect_visible(rect) {
        paint_icon(ui, icon, rect.center(), glyph * scale(ui, &r), p.text);
    }
    focus_ring(ui, &r, rect.width() / 2.0);
    r.on_hover_text(label)
        .on_hover_cursor(CursorIcon::PointingHand)
}

/// A 40 px pill with `label` (and `icon` before it), padded 20. Primary: `accent`
/// (`accent_hover` hovered) under `on_accent`; secondary: `surface` (`surface_hover` over it
/// hovered) under `text`.
pub fn pill_button(
    ui: &mut Ui,
    p: &Palette,
    icon: Option<Icon>,
    label: &str,
    primary: bool,
) -> Response {
    const PAD: f32 = 20.0;
    let (fill, hovered, ink) = if primary {
        (p.accent, p.accent_hover, p.on_accent)
    } else {
        (p.surface, p.surface.blend(p.surface_hover), p.text)
    };
    let text = ui
        .painter()
        .layout_no_wrap(label.to_owned(), theme::label(), ink);
    let lead = if icon.is_some() { GLYPH + 8.0 } else { 0.0 };
    let size = vec2(PAD + lead + text.size().x + PAD, theme::PILL);
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label));
    if ui.is_rect_visible(rect) {
        let fill = theme::blend(fill, hovered, fade(ui, &r, r.hovered()));
        ui.painter().rect_filled(rect, rect.height() / 2.0, fill);
        let mid = rect.center().y;
        if let Some(icon) = icon {
            paint_icon(
                ui,
                icon,
                pos2(rect.left() + PAD + GLYPH / 2.0, mid),
                GLYPH,
                ink,
            );
        }
        let at = pos2(rect.left() + PAD + lead, mid - text.size().y / 2.0);
        ui.painter().galley(at, text, ink);
    }
    focus_ring(ui, &r, rect.height() / 2.0);
    r.on_hover_cursor(CursorIcon::PointingHand)
}

/// A menu item (spec §3.7): a `GLYPH` `secondary` `icon`, then `label`, `MENU_ITEM` tall.
/// Screen readers hear `label` alone.
pub fn menu_button<'a>(
    p: &Palette,
    icon: Icon,
    label: impl Into<egui::WidgetText>,
) -> egui::Button<'a> {
    let atoms = (icon.image(p.secondary, GLYPH), label.into());
    egui::Button::new(atoms).min_size(vec2(0.0, theme::MENU_ITEM))
}

/// A `menu_button` ending in a `secondary` chevron, opening the submenu `add` beside it.
pub fn submenu(
    ui: &mut Ui,
    p: &Palette,
    icon: Icon,
    label: impl Into<egui::WidgetText>,
    add: impl FnOnce(&mut Ui),
) {
    let chevron = Icon::ChevronRight.image(p.secondary, GLYPH);
    let button = menu_button(p, icon, label).right_text(chevron);
    let sub = egui::containers::menu::SubMenuButton::from_button(button);
    sub.ui(ui, |ui| {
        menu_width(ui);
        add(ui);
    });
}

/// `add`'s menu, opened by right-clicking `resp`, in `theme::menu_style` (its submenus too).
pub fn context_menu(resp: &Response, p: &Palette, add: impl FnOnce(&mut Ui)) {
    let menu = egui::Popup::context_menu(resp).style(theme::menu_style(*p));
    menu.show(|ui| {
        menu_width(ui);
        add(ui);
    });
}

/// Makes the menu `ui` is the inside of at least `MENU_W` wide, its frame included.
pub(super) fn menu_width(ui: &mut Ui) {
    let frame = egui::Frame::menu(ui.style()).total_margin().sum().x;
    ui.set_min_width(theme::MENU_W - frame);
}

/// The rule between a menu's groups: 1 px `outline` (egui's separator stroke, which
/// `theme::style` makes that) with 5 above and below.
pub fn menu_separator(ui: &mut Ui) {
    ui.add(egui::Separator::default().spacing(11.0));
}

/// `text` in `heading()`: a side panel's, a Settings section's or a dialog's title.
pub fn heading(ui: &mut Ui, p: &Palette, text: &str) -> Response {
    ui.label(RichText::new(text).font(theme::heading()).color(p.text))
}

/// A sidebar row (spec §5.2): a 24 px icon and `label`, inset 8, radius `R_CARD`, the
/// width available. Idle `secondary`; hovered `text` on `surface_hover`; selected `text` on
/// `surface_active`. `collapsed`, only the icon shows, and `label` is its tooltip.
pub fn nav_row(
    ui: &mut Ui,
    p: &Palette,
    icon: Icon,
    label: &str,
    selected: bool,
    collapsed: bool,
) -> Response {
    let height = NAV_ROW;
    let size = vec2(ui.available_width().max(height), height);
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    r.widget_info(|| {
        WidgetInfo::selected(
            WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            label,
        )
    });
    if ui.is_rect_visible(rect) {
        let hover = fade(ui, &r, r.hovered());
        let fill = if selected {
            p.surface_active
        } else {
            p.surface_hover.gamma_multiply(hover)
        };
        ui.painter().rect_filled(rect, theme::R_CARD, fill);
        let ink = theme::blend(p.secondary, p.text, if selected { 1.0 } else { hover });
        let mid = rect.center().y;
        let icon_x = if collapsed {
            rect.center().x
        } else {
            rect.left() + NAV_PAD + NAV_ICON / 2.0
        };
        paint_icon(ui, icon, pos2(icon_x, mid), NAV_ICON, ink);
        if !collapsed {
            let x = rect.left() + 2.0 * NAV_PAD + NAV_ICON;
            let text = line(ui, label, theme::label(), ink, rect.right() - NAV_PAD - x);
            let at = pos2(x, mid - text.size().y / 2.0);
            ui.painter().galley(at, text, ink);
        }
    }
    focus_ring(ui, &r, theme::R_CARD);
    let r = r.on_hover_cursor(CursorIcon::PointingHand);
    if collapsed { r.on_hover_text(label) } else { r }
}

/// The search pill (spec §5.3): `width` by 40, a `TextEdit` with `id` after a search icon.
/// Idle: `surface`, the icon and `hint` in `dim`; focused it inverts, `text` under `window`.
/// The `TextEdit`'s response, over the whole pill.
pub fn search_field(
    ui: &mut Ui,
    p: &Palette,
    text: &mut String,
    id: egui::Id,
    hint: &str,
    width: f32,
) -> Response {
    let (rect, _) = ui.allocate_exact_size(vec2(width, theme::PILL), Sense::hover());
    let focused = ui.memory(|m| m.has_focus(id));
    let (fill, ink, faint) = if focused {
        (p.text, p.window, p.window)
    } else {
        (p.surface, p.text, p.dim)
    };
    ui.painter().rect_filled(rect, rect.height() / 2.0, fill);
    let mid = rect.center().y;
    paint_icon(
        ui,
        Icon::Search,
        pos2(rect.left() + 20.0, mid),
        GLYPH,
        faint,
    );
    let field = Rect::from_min_max(
        pos2(rect.left() + 36.0, rect.top()),
        pos2(rect.right() - 16.0, rect.bottom()),
    );
    let layout = Layout::left_to_right(Align::Center);
    let mut child = ui.new_child(UiBuilder::new().max_rect(field).layout(layout));
    let v = child.visuals_mut();
    v.text_cursor.stroke.color = ink;
    v.selection.bg_fill = ink.gamma_multiply(0.25);
    let edit = TextEdit::singleline(text)
        .id(id)
        .hint_text(RichText::new(hint).color(faint))
        .font(theme::body())
        .text_color(ink)
        .frame(egui::Frame::NONE)
        .min_size(field.size())
        .desired_width(f32::INFINITY)
        .vertical_align(Align::Center);
    let r = child.add(edit);
    ui.ctx()
        .accesskit_node_builder(r.id, |node| node.set_label(hint));
    if r.gained_focus() || r.lost_focus() {
        ui.ctx().request_repaint(); // drawn in the old colours this pass
    }
    let r = r.with_new_rect(rect);
    focus_ring(ui, &r, rect.height() / 2.0);
    r
}

/// The house text field (spec §3.6): `edit` on a `PILL`-tall, fully round `surface` pill
/// like the search's, its text inset 16 and centred, with the focus ring. Width, hint and the
/// rest stay the caller's.
pub fn text_field(ui: &mut Ui, p: &Palette, edit: TextEdit<'_>) -> Response {
    let radius = theme::PILL / 2.0;
    // A frame of our own skips egui's margin, fill and stroke: it brings them itself.
    let frame = egui::Frame::new()
        .fill(p.surface)
        .corner_radius(radius)
        .inner_margin(egui::Margin::symmetric(16, 0));
    let edit = edit
        .frame(frame)
        .min_size(vec2(0.0, theme::PILL))
        .vertical_align(Align::Center);
    let r = ui.add(edit);
    focus_ring(ui, &r, radius);
    r
}

const BADGE_PAD: Vec2 = vec2(12.0, 6.0);

/// A pill of `small()` `text` in `color`, on `color` at 10% with a 1 px stroke of it at 50%,
/// padded `BADGE_PAD` (spec §4.2: the format badge, tier colours), cut with "…" to the width
/// available. It takes the palette like the rest of the kit, but `color` is all it paints.
pub fn badge(ui: &mut Ui, _p: &Palette, text: &str, color: Color32) -> Response {
    let pad = BADGE_PAD;
    let width = ui.available_width() - 2.0 * pad.x;
    let galley = line(ui, text, theme::small(), color, width);
    let (rect, r) = ui.allocate_exact_size(galley.size() + 2.0 * pad, Sense::hover());
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Label, ui.is_enabled(), text));
    if ui.is_rect_visible(rect) {
        let radius = rect.height() / 2.0;
        let painter = ui.painter();
        painter.rect_filled(rect, radius, color.gamma_multiply(0.1));
        let stroke = Stroke::new(1.0, color.gamma_multiply(0.5));
        painter.rect_stroke(rect, radius, stroke, StrokeKind::Inside);
        painter.galley(rect.min + pad, galley, color);
    }
    r
}

/// The width a `badge` takes to show `text` whole.
pub fn badge_width(ui: &Ui, text: &str) -> f32 {
    let clear = Color32::TRANSPARENT; // measured, not painted
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), theme::small(), clear);
    galley.size().x + 2.0 * BADGE_PAD.x
}

/// `rect` filled from `top` at its top edge to `bottom` at its bottom one.
pub fn vertical_gradient(painter: &egui::Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    painter.add(mesh);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::testing::{self, key};
    use egui::{Id, Key};

    /// One pass of `draw` fed `events`: what it returned, and egui's output.
    fn run<R>(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        mut draw: impl FnMut(&mut egui::Ui) -> R,
    ) -> (R, egui::FullOutput) {
        let input = egui::RawInput {
            events,
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
            ..Default::default()
        };
        let mut drawn = None;
        let mut out = ctx.run_ui(input, |ui| drawn = Some(draw(ui)));
        out.textures_delta.clear(); // nothing renders them
        (drawn.expect("drawn"), out)
    }

    fn context() -> egui::Context {
        let ctx = testing::context();
        ctx.enable_accesskit();
        ctx
    }

    #[test]
    fn widgets_take_focus_and_activate_from_the_keyboard() {
        let ctx = context();
        let p = Palette::dark();
        let row = |ui: &mut egui::Ui| {
            ui.horizontal(|ui| {
                [
                    icon_button(ui, &p, Icon::Shuffle, "Shuffle", Some(false)),
                    pill_button(ui, &p, Some(Icon::PlayFilled), "Play", true),
                    nav_row(ui, &p, Icon::House, "Home", false, false),
                ]
            })
            .inner
        };
        run(&ctx, vec![], row);
        for i in 0..3 {
            let (shown, _) = run(&ctx, vec![key(Key::Tab)], row);
            assert_eq!(ctx.memory(|m| m.focused()), Some(shown[i].id), "Tab {i}");
            let (pressed, _) = run(&ctx, vec![key(Key::Enter)], row);
            assert!(pressed[i].clicked(), "Enter on {i}");
        }
    }

    /// The cursor with the pointer over the middle of what `draw` adds.
    fn hovered(ctx: &egui::Context, draw: impl Fn(&mut Ui) -> Response) -> CursorIcon {
        let (r, _) = run(ctx, vec![], &draw);
        let at = egui::Event::PointerMoved(r.rect.center());
        run(ctx, vec![at], &draw).1.platform_output.cursor_icon
    }

    /// Enabled clickables show the hand, stock buttons too; disabled ones and plain text the
    /// arrow.
    #[test]
    fn clickables_show_the_hand() {
        let ctx = context();
        let p = Palette::dark();
        theme::apply(&ctx, &p);
        let hand = CursorIcon::PointingHand;
        let shuffle = |ui: &mut Ui| icon_button(ui, &p, Icon::Shuffle, "Shuffle", None);
        assert_eq!(hovered(&ctx, shuffle), hand);
        let play = |ui: &mut Ui| pill_button(ui, &p, Some(Icon::PlayFilled), "Play", true);
        assert_eq!(hovered(&ctx, play), hand);
        let home = |ui: &mut Ui| nav_row(ui, &p, Icon::House, "Home", false, false);
        assert_eq!(hovered(&ctx, home), hand);
        assert_eq!(hovered(&ctx, |ui| ui.button("Retry")), hand); // via `interact_cursor`
        let off = |ui: &mut Ui| ui.add_enabled(false, egui::Button::new("Off"));
        assert_eq!(hovered(&ctx, off), CursorIcon::Default);
        let off = |ui: &mut Ui| ui.add_enabled_ui(false, play).inner;
        assert_eq!(hovered(&ctx, off), CursorIcon::Default);
        let text = |ui: &mut Ui| ui.label("Plain text");
        assert_eq!(hovered(&ctx, text), CursorIcon::Default); // no I-beam
        assert!(!ctx.global_style().interaction.selectable_labels);
    }

    #[test]
    fn icon_button_is_labelled() {
        let ctx = context();
        let p = Palette::dark();
        let shuffle = |ui: &mut egui::Ui| icon_button(ui, &p, Icon::Shuffle, "Shuffle", Some(true));
        let (_, out) = run(&ctx, vec![], shuffle);
        let tree = out.platform_output.accesskit_update.as_ref();
        let nodes = &tree.expect("AccessKit is on").nodes;
        let button = nodes.iter().find(|(_, n)| n.label() == Some("Shuffle"));
        let role = button.expect("a Shuffle node").1.role();
        assert_eq!(role, egui::accesskit::Role::Button);
        let (clicked, _) = run(&ctx, vec![testing::click(&out, "Shuffle")], shuffle);
        assert!(clicked.clicked());
    }

    /// A toggle tells screen readers whether it is on; a plain button is no toggle.
    #[test]
    fn icon_buttons_tell_toggles_apart() {
        use egui::accesskit::Toggled;
        let p = Palette::dark();
        let cases = [
            ("Shuffle", Some(true), Some(Toggled::True)),
            ("Shuffle", Some(false), Some(Toggled::False)),
            ("Back", None, None),
        ];
        for (label, toggle, want) in cases {
            let ctx = context();
            let (_, out) = run(&ctx, vec![], |ui| {
                icon_button(ui, &p, Icon::Shuffle, label, toggle)
            });
            let tree = out.platform_output.accesskit_update.as_ref();
            let nodes = &tree.expect("AccessKit is on").nodes;
            let node = nodes.iter().find(|(_, n)| n.label() == Some(label));
            let node = node
                .unwrap_or_else(|| panic!("no {label:?} node"))
                .1
                .clone();
            assert_eq!(node.toggled(), want, "{label} {toggle:?}");
        }
    }

    #[test]
    fn discs_and_badges_are_labelled() {
        let ctx = context();
        let p = Palette::dark();
        let (_, out) = run(&ctx, vec![], |ui| {
            let rect = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 100.0));
            vertical_gradient(ui.painter(), rect, p.window, p.surface);
            circle_button(
                ui,
                &p,
                Icon::Heart,
                "Add to favorites",
                40.0,
                p.surface,
                p.text,
            );
            badge(ui, &p, "MAX", p.tier(theme::Tier::Max));
        });
        let shown = testing::shown(&out);
        for label in ["Add to favorites", "MAX"] {
            assert!(
                shown.iter().any(|s| s == label),
                "no {label:?} in {shown:?}"
            );
        }
    }

    /// A badge cuts its text with "…" to the width it has, and `badge_width` is what it takes
    /// to show it whole.
    #[test]
    fn badges_cut_their_text_to_the_room() {
        let ctx = context();
        let p = Palette::dark();
        let text = "24/176.4 FLAC · Shared 48 kHz";
        let whole = run(&ctx, vec![], |ui| badge_width(ui, text)).0;
        for (room, cut) in [(whole - 20.0, true), (whole, false)] {
            let (r, out) = run(&ctx, vec![], |ui| {
                let size = vec2(room, theme::PILL);
                ui.allocate_ui(size, |ui| badge(ui, &p, text, p.text)).inner
            });
            assert!(r.rect.width() <= room, "{room}: {:?}", r.rect);
            let elided = out.shapes.iter().any(|s| match &s.shape {
                egui::Shape::Text(t) => t.galley.elided,
                _ => false,
            });
            assert_eq!(elided, cut, "{room}");
        }
    }

    /// Hovered, a transport glyph paints no disc (ruling 27).
    #[test]
    fn glyph_buttons_have_no_disc() {
        let ctx = context();
        let p = Palette::dark();
        let play =
            |ui: &mut egui::Ui| glyph_button(ui, &p, Icon::PlayFilled, "Play", theme::PLAY_GLYPH);
        let (_, out) = run(&ctx, vec![], play);
        let at = egui::Event::PointerMoved(testing::middle(&out, "Play"));
        let (r, out) = run(&ctx, vec![at], play);
        assert!(r.hovered());
        let disc = out.shapes.iter().any(|s| {
            let shape = &s.shape;
            matches!(shape, egui::Shape::Circle(_))
                || matches!(shape, egui::Shape::Rect(r) if r.fill.a() > 0 && r.brush.is_none())
        });
        assert!(!disc, "{:?}", out.shapes);
    }

    #[test]
    fn search_field_keeps_its_id() {
        let ctx = context();
        let p = Palette::dark();
        let id = Id::new("query");
        let mut text = String::new();
        let mut field = |events| {
            run(&ctx, events, |ui| {
                search_field(ui, &p, &mut text, id, "Search TIDAL", 280.0);
            })
            .1
        };
        // A pointer's click: a text field takes no focus from a screen reader's.
        let at = testing::middle(&field(vec![]), "Search TIDAL");
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        field(vec![egui::Event::PointerMoved(at), button(true)]);
        field(vec![button(false)]);
        field(vec![egui::Event::Text("abc".into())]);
        assert_eq!(text, "abc");
        assert!(ctx.memory(|m| m.has_focus(id)));
    }

    /// A text field is a pill tall, and a pointer's click on it lets you type.
    #[test]
    fn text_fields_are_a_pill_tall() {
        let ctx = context();
        let p = Palette::dark();
        let mut text = String::new();
        let mut field = |events| {
            run(&ctx, events, |ui| {
                text_field(ui, &p, TextEdit::singleline(&mut text).hint_text("Name"))
            })
            .0
        };
        let r = field(vec![]);
        assert_eq!(r.rect.height(), theme::PILL);
        let at = r.rect.center();
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        field(vec![egui::Event::PointerMoved(at), button(true)]);
        field(vec![button(false)]);
        field(vec![egui::Event::Text("abc".into())]);
        assert_eq!(text, "abc");
    }

    #[test]
    fn icons_load() {
        use egui::load::{BytesPoll, ImagePoll, SizeHint};
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        fastframe_icons::install::<Icon>(&ctx);
        for icon in Icon::ALL {
            let bytes = ctx.try_load_bytes(icon.uri());
            assert!(matches!(bytes, Ok(BytesPoll::Ready { .. })), "{icon:?}");
            let image = ctx.try_load_image(icon.uri(), SizeHint::default());
            assert!(matches!(image, Ok(ImagePoll::Ready { .. })), "{icon:?}");
        }
    }
}
