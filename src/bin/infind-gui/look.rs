//! How the window looks: the font, colors for light and dark desktops, and small drawn icons.

use eframe::egui::{self, vec2, Color32, FontData, FontDefinitions, FontFamily, FontId, Painter, Pos2, Rect, Shape, Stroke};
use std::sync::Arc;

const SEMIBOLD: &str = "semibold";

/// Inter for all text; egui's own fonts stay behind it for symbols and emoji.
pub fn install_fonts(ctx: &egui::Context) {
    let mut f = FontDefinitions::default();
    let regular = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
    let semibold = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");
    f.font_data.insert("inter".into(), Arc::new(FontData::from_static(regular)));
    f.font_data.insert("inter-semibold".into(), Arc::new(FontData::from_static(semibold)));
    let fallbacks: Vec<String> = f.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    f.families.entry(FontFamily::Proportional).or_default().insert(0, "inter".into());
    let mut bold = vec!["inter-semibold".to_string()];
    bold.extend(fallbacks);
    f.families.insert(FontFamily::Name(SEMIBOLD.into()), bold);
    ctx.set_fonts(f);
}

pub fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

/// The panel's colors. Neutrals follow the desktop's light or dark mode; the accent is the
/// amber of infind's icon.
#[derive(Clone, Copy)]
pub struct Palette {
    pub surface: Color32,
    pub text: Color32,
    pub secondary: Color32,
    pub tertiary: Color32,
    pub line: Color32,
    pub selected: Color32,
    pub hover: Color32,
    pub chip: Color32,
    pub mark: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    /// Text on an accent-filled button.
    pub on_accent: Color32,
    pub danger: Color32,
    pub shadow: Color32,
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            surface: Color32::from_rgb(0x1f, 0x1f, 0x22),
            text: Color32::from_rgb(0xf2, 0xf2, 0xf7),
            secondary: Color32::from_rgb(0xa1, 0xa1, 0xa6),
            tertiary: Color32::from_rgb(0x6e, 0x6e, 0x73),
            line: Color32::from_white_alpha(26),
            selected: Color32::from_rgba_unmultiplied(10, 132, 255, 82),
            hover: Color32::from_white_alpha(12),
            chip: Color32::from_white_alpha(23),
            mark: Color32::from_rgba_unmultiplied(251, 191, 36, 90),
            accent: Color32::from_rgb(0xfb, 0xbf, 0x24),
            accent_soft: Color32::from_rgba_unmultiplied(251, 191, 36, 36),
            on_accent: Color32::BLACK,
            danger: Color32::from_rgb(0xff, 0x45, 0x3a),
            shadow: Color32::from_black_alpha(110),
        }
    } else {
        Palette {
            surface: Color32::from_rgb(0xfb, 0xfb, 0xfb),
            text: Color32::from_rgb(0x1d, 0x1d, 0x1f),
            secondary: Color32::from_rgb(0x6e, 0x6e, 0x73),
            tertiary: Color32::from_rgb(0xa1, 0xa1, 0xa6),
            line: Color32::from_black_alpha(23),
            selected: Color32::from_rgba_unmultiplied(10, 132, 255, 51),
            hover: Color32::from_black_alpha(10),
            chip: Color32::from_black_alpha(15),
            mark: Color32::from_rgba_unmultiplied(251, 191, 36, 80),
            accent: Color32::from_rgb(0xd9, 0x77, 0x06),
            accent_soft: Color32::from_rgba_unmultiplied(217, 119, 6, 36),
            on_accent: Color32::WHITE,
            danger: Color32::from_rgb(0xff, 0x3b, 0x30),
            shadow: Color32::from_black_alpha(70),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Search,
    Mic,
    Stop,
    Folder,
    Doc,
    Pdf,
    Image,
    Audio,
    Video,
}

impl Icon {
    pub fn for_kind(kind: &str) -> Self {
        match kind {
            "pdf" => Icon::Pdf,
            "image" => Icon::Image,
            "audio" => Icon::Audio,
            "video" => Icon::Video,
            _ => Icon::Doc,
        }
    }
}

/// Draws `icon` into the square `rect`, on a 24-unit grid like an SVG icon.
pub fn paint(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width() / 24.0;
    let at = |x: f32, y: f32| -> Pos2 { rect.min + vec2(x, y) * s };
    let stroke = Stroke::new((1.7 * s).max(1.2), color);
    let line = |pts: &[(f32, f32)]| Shape::line(pts.iter().map(|&(x, y)| at(x, y)).collect(), stroke);
    let closed = |pts: &[(f32, f32)]| Shape::closed_line(pts.iter().map(|&(x, y)| at(x, y)).collect(), stroke);
    match icon {
        Icon::Search => {
            p.circle_stroke(at(10.0, 10.0), 6.5 * s, Stroke::new(2.4 * s, color));
            p.line_segment([at(15.0, 15.0), at(20.5, 20.5)], Stroke::new(2.6 * s, color));
        }
        Icon::Mic => {
            p.rect_stroke(Rect::from_min_max(at(9.0, 3.0), at(15.0, 14.5)), 3.0 * s, stroke, egui::StrokeKind::Middle);
            let arc: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let a = std::f32::consts::PI * i as f32 / 12.0;
                    at(12.0 + 6.0 * a.cos(), 11.0 + 6.0 * a.sin())
                })
                .collect();
            p.add(Shape::line(arc, stroke));
            p.line_segment([at(12.0, 17.0), at(12.0, 21.0)], stroke);
            p.line_segment([at(9.0, 21.0), at(15.0, 21.0)], stroke);
        }
        Icon::Stop => {
            p.rect_filled(Rect::from_min_max(at(7.0, 7.0), at(17.0, 17.0)), 2.0 * s, color);
        }
        Icon::Folder => {
            p.add(closed(&[(3.0, 6.0), (9.5, 6.0), (11.5, 8.5), (21.0, 8.5), (21.0, 19.0), (3.0, 19.0)]));
        }
        Icon::Doc | Icon::Pdf => {
            p.add(closed(&[(6.0, 3.0), (14.0, 3.0), (18.5, 7.5), (18.5, 21.0), (6.0, 21.0)]));
            p.add(line(&[(14.0, 3.0), (14.0, 7.5), (18.5, 7.5)]));
            if icon == Icon::Pdf {
                p.rect_filled(Rect::from_min_max(at(4.0, 13.0), at(16.0, 18.5)), 1.5 * s, color);
            } else {
                p.line_segment([at(9.0, 12.0), at(15.5, 12.0)], stroke);
                p.line_segment([at(9.0, 16.0), at(15.5, 16.0)], stroke);
            }
        }
        Icon::Image => {
            p.rect_stroke(Rect::from_min_max(at(3.0, 5.0), at(21.0, 19.0)), 2.5 * s, stroke, egui::StrokeKind::Middle);
            p.add(line(&[(3.5, 17.0), (9.0, 11.5), (13.5, 16.0), (16.5, 13.0), (20.5, 17.0)]));
            p.circle_filled(at(16.0, 9.0), 1.6 * s, color);
        }
        Icon::Audio => {
            for (x, h) in [(4.0, 4.0), (8.0, 10.0), (12.0, 16.0), (16.0, 8.0), (20.0, 4.0)] {
                p.line_segment([at(x, 12.0 - h / 2.0), at(x, 12.0 + h / 2.0)], Stroke::new(2.0 * s, color));
            }
        }
        Icon::Video => {
            p.rect_stroke(Rect::from_min_max(at(3.0, 6.0), at(15.5, 18.0)), 2.5 * s, stroke, egui::StrokeKind::Middle);
            p.add(closed(&[(15.5, 10.5), (21.0, 7.5), (21.0, 16.5), (15.5, 13.5)]));
        }
    }
}

/// The square of `size` centered in `rect`.
pub fn centered(rect: Rect, size: f32) -> Rect {
    Rect::from_center_size(rect.center(), vec2(size, size))
}

