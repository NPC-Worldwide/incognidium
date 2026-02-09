use incognidium_css::CssColor;
use incognidium_layout::FlatBox;
use incognidium_style::{ComputedStyle, Display, FontWeight, StyleMap, TextDecoration};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

/// Paint the layout tree into a pixel buffer.
pub fn paint(
    flat_boxes: &[FlatBox],
    styles: &StyleMap,
    width: u32,
    height: u32,
) -> Pixmap {
    let mut pixmap = Pixmap::new(width, height).expect("failed to create pixmap");

    // Fill background white
    pixmap.fill(Color::WHITE);

    for fbox in flat_boxes {
        let style = styles
            .get(&fbox.node_id)
            .cloned()
            .unwrap_or_default();

        if style.display == Display::None {
            continue;
        }

        // Draw background
        if style.background_color.a > 0 {
            draw_rect(
                &mut pixmap,
                fbox.x,
                fbox.y,
                fbox.width,
                fbox.height,
                style.background_color,
            );
        }

        // Draw border
        if style.border_top_width > 0.0
            || style.border_right_width > 0.0
            || style.border_bottom_width > 0.0
            || style.border_left_width > 0.0
        {
            draw_borders(&mut pixmap, fbox, &style);
        }

        // Draw text
        if let Some(ref text) = fbox.text {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                draw_text(&mut pixmap, fbox.x, fbox.y, trimmed, &style);
            }
        }
    }

    pixmap
}

fn css_to_skia_color(c: CssColor) -> Color {
    Color::from_rgba8(c.r, c.g, c.b, c.a)
}

fn draw_rect(pixmap: &mut Pixmap, x: f32, y: f32, width: f32, height: f32, color: CssColor) {
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let rect = match Rect::from_xywh(x, y, width.max(1.0), height.max(1.0)) {
        Some(r) => r,
        None => return,
    };
    let mut paint = Paint::default();
    paint.set_color(css_to_skia_color(color));
    paint.anti_alias = true;

    let path = PathBuilder::from_rect(rect);
    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
}

fn draw_borders(pixmap: &mut Pixmap, fbox: &FlatBox, style: &ComputedStyle) {
    let bc = style.border_color;

    // Top border
    if style.border_top_width > 0.0 {
        draw_rect(
            pixmap,
            fbox.x,
            fbox.y,
            fbox.width,
            style.border_top_width,
            bc,
        );
    }
    // Bottom border
    if style.border_bottom_width > 0.0 {
        draw_rect(
            pixmap,
            fbox.x,
            fbox.y + fbox.height - style.border_bottom_width,
            fbox.width,
            style.border_bottom_width,
            bc,
        );
    }
    // Left border
    if style.border_left_width > 0.0 {
        draw_rect(
            pixmap,
            fbox.x,
            fbox.y,
            style.border_left_width,
            fbox.height,
            bc,
        );
    }
    // Right border
    if style.border_right_width > 0.0 {
        draw_rect(
            pixmap,
            fbox.x + fbox.width - style.border_right_width,
            fbox.y,
            style.border_right_width,
            fbox.height,
            bc,
        );
    }
}

/// Simple bitmap font text rendering.
/// This is a crude glyph renderer — just draws rectangles for each character.
/// Phase 2 will use proper font shaping (rustybuzz/parley).
fn draw_text(pixmap: &mut Pixmap, x: f32, y: f32, text: &str, style: &ComputedStyle) {
    let font_size = style.font_size;
    let char_width = font_size * 0.6;
    let line_height = font_size * style.line_height;
    let color = style.color;
    let bold = style.font_weight == FontWeight::Bold;

    let mut cursor_x = x;
    let mut cursor_y = y;

    for ch in text.chars() {
        if ch == '\n' {
            cursor_x = x;
            cursor_y += line_height;
            continue;
        }
        if ch == ' ' {
            cursor_x += char_width;
            continue;
        }

        // Draw the character as a simple bitmap pattern
        draw_bitmap_char(pixmap, cursor_x, cursor_y, ch, font_size, color, bold);
        cursor_x += char_width;
    }

    // Draw underline if needed
    if style.text_decoration == TextDecoration::Underline {
        let text_width = text.len() as f32 * char_width;
        let underline_y = y + font_size;
        draw_rect(pixmap, x, underline_y, text_width, 1.0, color);
    }
}

/// Draw a single character using simple pixel patterns.
/// This is a minimal bitmap font — just enough to be readable.
fn draw_bitmap_char(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    ch: char,
    font_size: f32,
    color: CssColor,
    bold: bool,
) {
    let scale = font_size / 16.0; // Base glyph is designed at 16px
    let w = if bold { 2.0 * scale } else { 1.5 * scale };

    // Get the glyph pattern (list of line segments)
    let segments = glyph_segments(ch);

    let mut paint = Paint::default();
    paint.set_color(css_to_skia_color(color));
    paint.anti_alias = true;

    for (x1, y1, x2, y2) in segments {
        let sx = x + x1 * scale;
        let sy = y + y1 * scale;
        let ex = x + x2 * scale;
        let ey = y + y2 * scale;

        // Draw a thick line as a thin rectangle
        if (sx - ex).abs() < 0.5 {
            // Vertical line
            let min_y = sy.min(ey);
            let max_y = sy.max(ey);
            draw_rect(pixmap, sx - w / 2.0, min_y, w, max_y - min_y, color);
        } else if (sy - ey).abs() < 0.5 {
            // Horizontal line
            let min_x = sx.min(ex);
            let max_x = sx.max(ex);
            draw_rect(pixmap, min_x, sy - w / 2.0, max_x - min_x, w, color);
        } else {
            // Diagonal — draw as series of small rects
            let steps = ((ex - sx).abs().max((ey - sy).abs()) / (w * 0.5)) as u32;
            let steps = steps.max(2);
            for i in 0..steps {
                let t = i as f32 / steps as f32;
                let px = sx + (ex - sx) * t;
                let py = sy + (ey - sy) * t;
                draw_rect(pixmap, px - w / 2.0, py - w / 2.0, w, w, color);
            }
        }
    }
}

/// Return line segments for rendering a character.
/// Format: (x1, y1, x2, y2) in a 10x16 grid.
fn glyph_segments(ch: char) -> Vec<(f32, f32, f32, f32)> {
    match ch {
        // Uppercase letters
        'A' | 'a' => vec![
            (1.0, 14.0, 5.0, 2.0),
            (5.0, 2.0, 9.0, 14.0),
            (3.0, 9.0, 7.0, 9.0),
        ],
        'B' | 'b' => vec![
            (2.0, 2.0, 2.0, 14.0),
            (2.0, 2.0, 7.0, 2.0),
            (7.0, 2.0, 8.0, 5.0),
            (8.0, 5.0, 7.0, 8.0),
            (2.0, 8.0, 7.0, 8.0),
            (7.0, 8.0, 8.0, 11.0),
            (8.0, 11.0, 7.0, 14.0),
            (2.0, 14.0, 7.0, 14.0),
        ],
        'C' | 'c' => vec![
            (8.0, 3.0, 5.0, 2.0),
            (5.0, 2.0, 2.0, 4.0),
            (2.0, 4.0, 2.0, 12.0),
            (2.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 8.0, 13.0),
        ],
        'D' | 'd' => vec![
            (2.0, 2.0, 2.0, 14.0),
            (2.0, 2.0, 6.0, 2.0),
            (6.0, 2.0, 8.0, 5.0),
            (8.0, 5.0, 8.0, 11.0),
            (8.0, 11.0, 6.0, 14.0),
            (2.0, 14.0, 6.0, 14.0),
        ],
        'E' | 'e' => vec![
            (2.0, 2.0, 2.0, 14.0),
            (2.0, 2.0, 8.0, 2.0),
            (2.0, 8.0, 7.0, 8.0),
            (2.0, 14.0, 8.0, 14.0),
        ],
        'F' | 'f' => vec![
            (2.0, 2.0, 2.0, 14.0),
            (2.0, 2.0, 8.0, 2.0),
            (2.0, 8.0, 7.0, 8.0),
        ],
        'G' | 'g' => vec![
            (8.0, 3.0, 5.0, 2.0),
            (5.0, 2.0, 2.0, 4.0),
            (2.0, 4.0, 2.0, 12.0),
            (2.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 8.0, 12.0),
            (8.0, 12.0, 8.0, 8.0),
            (5.0, 8.0, 8.0, 8.0),
        ],
        'H' | 'h' => vec![
            (2.0, 2.0, 2.0, 14.0),
            (8.0, 2.0, 8.0, 14.0),
            (2.0, 8.0, 8.0, 8.0),
        ],
        'I' | 'i' => vec![
            (3.0, 2.0, 7.0, 2.0),
            (5.0, 2.0, 5.0, 14.0),
            (3.0, 14.0, 7.0, 14.0),
        ],
        'J' | 'j' => vec![
            (4.0, 2.0, 8.0, 2.0),
            (7.0, 2.0, 7.0, 12.0),
            (7.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 3.0, 12.0),
        ],
        'K' | 'k' => vec![
            (2.0, 2.0, 2.0, 14.0),
            (8.0, 2.0, 2.0, 8.0),
            (2.0, 8.0, 8.0, 14.0),
        ],
        'L' | 'l' => vec![(2.0, 2.0, 2.0, 14.0), (2.0, 14.0, 8.0, 14.0)],
        'M' | 'm' => vec![
            (1.0, 14.0, 1.0, 2.0),
            (1.0, 2.0, 5.0, 8.0),
            (5.0, 8.0, 9.0, 2.0),
            (9.0, 2.0, 9.0, 14.0),
        ],
        'N' | 'n' => vec![
            (2.0, 14.0, 2.0, 2.0),
            (2.0, 2.0, 8.0, 14.0),
            (8.0, 14.0, 8.0, 2.0),
        ],
        'O' | 'o' => vec![
            (3.0, 2.0, 7.0, 2.0),
            (7.0, 2.0, 9.0, 4.0),
            (9.0, 4.0, 9.0, 12.0),
            (9.0, 12.0, 7.0, 14.0),
            (7.0, 14.0, 3.0, 14.0),
            (3.0, 14.0, 1.0, 12.0),
            (1.0, 12.0, 1.0, 4.0),
            (1.0, 4.0, 3.0, 2.0),
        ],
        'P' | 'p' => vec![
            (2.0, 2.0, 2.0, 14.0),
            (2.0, 2.0, 7.0, 2.0),
            (7.0, 2.0, 8.0, 5.0),
            (8.0, 5.0, 7.0, 8.0),
            (2.0, 8.0, 7.0, 8.0),
        ],
        'Q' | 'q' => vec![
            (3.0, 2.0, 7.0, 2.0),
            (7.0, 2.0, 9.0, 4.0),
            (9.0, 4.0, 9.0, 12.0),
            (9.0, 12.0, 7.0, 14.0),
            (7.0, 14.0, 3.0, 14.0),
            (3.0, 14.0, 1.0, 12.0),
            (1.0, 12.0, 1.0, 4.0),
            (1.0, 4.0, 3.0, 2.0),
            (6.0, 11.0, 9.0, 15.0),
        ],
        'R' | 'r' => vec![
            (2.0, 2.0, 2.0, 14.0),
            (2.0, 2.0, 7.0, 2.0),
            (7.0, 2.0, 8.0, 5.0),
            (8.0, 5.0, 7.0, 8.0),
            (2.0, 8.0, 7.0, 8.0),
            (5.0, 8.0, 8.0, 14.0),
        ],
        'S' | 's' => vec![
            (8.0, 3.0, 5.0, 2.0),
            (5.0, 2.0, 2.0, 4.0),
            (2.0, 4.0, 3.0, 7.0),
            (3.0, 7.0, 7.0, 9.0),
            (7.0, 9.0, 8.0, 12.0),
            (8.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 2.0, 13.0),
        ],
        'T' | 't' => vec![(1.0, 2.0, 9.0, 2.0), (5.0, 2.0, 5.0, 14.0)],
        'U' | 'u' => vec![
            (2.0, 2.0, 2.0, 12.0),
            (2.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 8.0, 12.0),
            (8.0, 12.0, 8.0, 2.0),
        ],
        'V' | 'v' => vec![(1.0, 2.0, 5.0, 14.0), (5.0, 14.0, 9.0, 2.0)],
        'W' | 'w' => vec![
            (0.0, 2.0, 2.0, 14.0),
            (2.0, 14.0, 5.0, 8.0),
            (5.0, 8.0, 8.0, 14.0),
            (8.0, 14.0, 10.0, 2.0),
        ],
        'X' | 'x' => vec![(2.0, 2.0, 8.0, 14.0), (8.0, 2.0, 2.0, 14.0)],
        'Y' | 'y' => vec![
            (1.0, 2.0, 5.0, 8.0),
            (9.0, 2.0, 5.0, 8.0),
            (5.0, 8.0, 5.0, 14.0),
        ],
        'Z' | 'z' => vec![
            (2.0, 2.0, 8.0, 2.0),
            (8.0, 2.0, 2.0, 14.0),
            (2.0, 14.0, 8.0, 14.0),
        ],
        // Numbers
        '0' => vec![
            (3.0, 2.0, 7.0, 2.0),
            (7.0, 2.0, 8.0, 4.0),
            (8.0, 4.0, 8.0, 12.0),
            (8.0, 12.0, 7.0, 14.0),
            (7.0, 14.0, 3.0, 14.0),
            (3.0, 14.0, 2.0, 12.0),
            (2.0, 12.0, 2.0, 4.0),
            (2.0, 4.0, 3.0, 2.0),
        ],
        '1' => vec![
            (3.0, 4.0, 5.0, 2.0),
            (5.0, 2.0, 5.0, 14.0),
            (3.0, 14.0, 7.0, 14.0),
        ],
        '2' => vec![
            (2.0, 4.0, 3.0, 2.0),
            (3.0, 2.0, 7.0, 2.0),
            (7.0, 2.0, 8.0, 4.0),
            (8.0, 4.0, 2.0, 14.0),
            (2.0, 14.0, 8.0, 14.0),
        ],
        '3' => vec![
            (2.0, 3.0, 3.0, 2.0),
            (3.0, 2.0, 7.0, 2.0),
            (7.0, 2.0, 8.0, 5.0),
            (8.0, 5.0, 5.0, 8.0),
            (5.0, 8.0, 8.0, 11.0),
            (8.0, 11.0, 7.0, 14.0),
            (7.0, 14.0, 3.0, 14.0),
            (3.0, 14.0, 2.0, 13.0),
        ],
        '4' => vec![
            (7.0, 2.0, 2.0, 9.0),
            (2.0, 9.0, 8.0, 9.0),
            (7.0, 2.0, 7.0, 14.0),
        ],
        '5' => vec![
            (8.0, 2.0, 2.0, 2.0),
            (2.0, 2.0, 2.0, 7.0),
            (2.0, 7.0, 7.0, 7.0),
            (7.0, 7.0, 8.0, 10.0),
            (8.0, 10.0, 7.0, 14.0),
            (7.0, 14.0, 3.0, 14.0),
            (3.0, 14.0, 2.0, 13.0),
        ],
        '6' => vec![
            (7.0, 2.0, 3.0, 2.0),
            (3.0, 2.0, 2.0, 4.0),
            (2.0, 4.0, 2.0, 12.0),
            (2.0, 12.0, 3.0, 14.0),
            (3.0, 14.0, 7.0, 14.0),
            (7.0, 14.0, 8.0, 12.0),
            (8.0, 12.0, 8.0, 9.0),
            (8.0, 9.0, 7.0, 7.0),
            (7.0, 7.0, 2.0, 7.0),
        ],
        '7' => vec![
            (2.0, 2.0, 8.0, 2.0),
            (8.0, 2.0, 4.0, 14.0),
        ],
        '8' => vec![
            (3.0, 2.0, 7.0, 2.0),
            (7.0, 2.0, 8.0, 4.0),
            (8.0, 4.0, 7.0, 7.0),
            (7.0, 7.0, 3.0, 7.0),
            (3.0, 7.0, 2.0, 4.0),
            (2.0, 4.0, 3.0, 2.0),
            (3.0, 7.0, 2.0, 10.0),
            (2.0, 10.0, 3.0, 14.0),
            (3.0, 14.0, 7.0, 14.0),
            (7.0, 14.0, 8.0, 10.0),
            (8.0, 10.0, 7.0, 7.0),
        ],
        '9' => vec![
            (8.0, 7.0, 7.0, 2.0),
            (7.0, 2.0, 3.0, 2.0),
            (3.0, 2.0, 2.0, 4.0),
            (2.0, 4.0, 2.0, 6.0),
            (2.0, 6.0, 3.0, 8.0),
            (3.0, 8.0, 8.0, 8.0),
            (8.0, 2.0, 8.0, 12.0),
            (8.0, 12.0, 5.0, 14.0),
        ],
        // Punctuation
        '.' => vec![(4.0, 13.0, 6.0, 13.0), (4.0, 13.0, 4.0, 14.0), (6.0, 13.0, 6.0, 14.0), (4.0, 14.0, 6.0, 14.0)],
        ',' => vec![(5.0, 12.0, 5.0, 14.0), (5.0, 14.0, 4.0, 15.0)],
        ':' => vec![
            (4.5, 5.0, 5.5, 5.0), (4.5, 5.0, 4.5, 6.0), (5.5, 5.0, 5.5, 6.0), (4.5, 6.0, 5.5, 6.0),
            (4.5, 12.0, 5.5, 12.0), (4.5, 12.0, 4.5, 13.0), (5.5, 12.0, 5.5, 13.0), (4.5, 13.0, 5.5, 13.0),
        ],
        ';' => vec![
            (4.5, 5.0, 5.5, 5.0), (4.5, 5.0, 4.5, 6.0), (5.5, 5.0, 5.5, 6.0),
            (5.0, 12.0, 5.0, 14.0), (5.0, 14.0, 4.0, 15.0),
        ],
        '!' => vec![
            (5.0, 2.0, 5.0, 10.0),
            (4.5, 12.0, 5.5, 12.0), (4.5, 12.0, 4.5, 13.0), (5.5, 12.0, 5.5, 13.0), (4.5, 13.0, 5.5, 13.0),
        ],
        '?' => vec![
            (2.0, 4.0, 3.0, 2.0),
            (3.0, 2.0, 7.0, 2.0),
            (7.0, 2.0, 8.0, 4.0),
            (8.0, 4.0, 5.0, 8.0),
            (5.0, 8.0, 5.0, 10.0),
            (4.5, 12.0, 5.5, 12.0), (4.5, 12.0, 4.5, 13.0), (5.5, 12.0, 5.5, 13.0),
        ],
        '-' => vec![(2.0, 8.0, 8.0, 8.0)],
        '_' => vec![(1.0, 14.0, 9.0, 14.0)],
        '+' => vec![(5.0, 4.0, 5.0, 12.0), (2.0, 8.0, 8.0, 8.0)],
        '=' => vec![(2.0, 6.0, 8.0, 6.0), (2.0, 10.0, 8.0, 10.0)],
        '/' => vec![(8.0, 2.0, 2.0, 14.0)],
        '\\' => vec![(2.0, 2.0, 8.0, 14.0)],
        '(' => vec![(6.0, 1.0, 4.0, 4.0), (4.0, 4.0, 4.0, 12.0), (4.0, 12.0, 6.0, 15.0)],
        ')' => vec![(4.0, 1.0, 6.0, 4.0), (6.0, 4.0, 6.0, 12.0), (6.0, 12.0, 4.0, 15.0)],
        '[' => vec![(3.0, 1.0, 7.0, 1.0), (3.0, 1.0, 3.0, 15.0), (3.0, 15.0, 7.0, 15.0)],
        ']' => vec![(3.0, 1.0, 7.0, 1.0), (7.0, 1.0, 7.0, 15.0), (3.0, 15.0, 7.0, 15.0)],
        '{' => vec![
            (6.0, 1.0, 5.0, 2.0), (5.0, 2.0, 5.0, 6.0), (5.0, 6.0, 3.0, 8.0),
            (3.0, 8.0, 5.0, 10.0), (5.0, 10.0, 5.0, 14.0), (5.0, 14.0, 6.0, 15.0),
        ],
        '}' => vec![
            (4.0, 1.0, 5.0, 2.0), (5.0, 2.0, 5.0, 6.0), (5.0, 6.0, 7.0, 8.0),
            (7.0, 8.0, 5.0, 10.0), (5.0, 10.0, 5.0, 14.0), (5.0, 14.0, 4.0, 15.0),
        ],
        '<' => vec![(8.0, 3.0, 2.0, 8.0), (2.0, 8.0, 8.0, 13.0)],
        '>' => vec![(2.0, 3.0, 8.0, 8.0), (8.0, 8.0, 2.0, 13.0)],
        '"' | '\u{201C}' | '\u{201D}' => vec![
            (3.0, 2.0, 3.0, 5.0),
            (7.0, 2.0, 7.0, 5.0),
        ],
        '\'' | '\u{2018}' | '\u{2019}' => vec![(5.0, 2.0, 5.0, 5.0)],
        '#' => vec![
            (3.0, 3.0, 3.0, 13.0),
            (7.0, 3.0, 7.0, 13.0),
            (1.0, 6.0, 9.0, 6.0),
            (1.0, 10.0, 9.0, 10.0),
        ],
        '@' => vec![
            (8.0, 4.0, 5.0, 2.0), (5.0, 2.0, 2.0, 4.0), (2.0, 4.0, 2.0, 12.0),
            (2.0, 12.0, 5.0, 14.0), (5.0, 14.0, 8.0, 12.0),
            (6.0, 6.0, 6.0, 10.0), (6.0, 10.0, 8.0, 10.0), (8.0, 4.0, 8.0, 10.0),
        ],
        '&' => vec![
            (6.0, 2.0, 4.0, 2.0), (4.0, 2.0, 3.0, 4.0), (3.0, 4.0, 4.0, 7.0),
            (4.0, 7.0, 2.0, 12.0), (2.0, 12.0, 4.0, 14.0), (4.0, 14.0, 6.0, 14.0),
            (6.0, 14.0, 8.0, 12.0), (4.0, 7.0, 8.0, 10.0),
        ],
        '*' => vec![
            (5.0, 3.0, 5.0, 11.0),
            (2.0, 5.0, 8.0, 9.0),
            (2.0, 9.0, 8.0, 5.0),
        ],
        '%' => vec![
            (2.0, 2.0, 4.0, 2.0), (2.0, 2.0, 2.0, 4.0), (4.0, 2.0, 4.0, 4.0), (2.0, 4.0, 4.0, 4.0),
            (8.0, 2.0, 2.0, 14.0),
            (6.0, 12.0, 8.0, 12.0), (6.0, 12.0, 6.0, 14.0), (8.0, 12.0, 8.0, 14.0), (6.0, 14.0, 8.0, 14.0),
        ],
        '$' => vec![
            (7.0, 3.0, 3.0, 3.0), (3.0, 3.0, 2.0, 5.0), (2.0, 5.0, 3.0, 7.0),
            (3.0, 7.0, 7.0, 9.0), (7.0, 9.0, 8.0, 11.0), (8.0, 11.0, 7.0, 13.0),
            (7.0, 13.0, 3.0, 13.0), (5.0, 1.0, 5.0, 15.0),
        ],
        '^' => vec![(2.0, 5.0, 5.0, 2.0), (5.0, 2.0, 8.0, 5.0)],
        '~' => vec![(1.0, 8.0, 3.0, 6.0), (3.0, 6.0, 5.0, 8.0), (5.0, 8.0, 7.0, 6.0), (7.0, 6.0, 9.0, 8.0)],
        '`' => vec![(4.0, 2.0, 6.0, 4.0)],
        '|' => vec![(5.0, 1.0, 5.0, 15.0)],
        _ => {
            // Unknown character: draw a small rectangle
            vec![
                (2.0, 2.0, 8.0, 2.0),
                (8.0, 2.0, 8.0, 14.0),
                (8.0, 14.0, 2.0, 14.0),
                (2.0, 14.0, 2.0, 2.0),
            ]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paint_empty() {
        let pixmap = Pixmap::new(100, 100).unwrap();
        assert_eq!(pixmap.width(), 100);
        assert_eq!(pixmap.height(), 100);
    }

    #[test]
    fn test_draw_rect_basic() {
        let mut pixmap = Pixmap::new(100, 100).unwrap();
        draw_rect(&mut pixmap, 10.0, 10.0, 50.0, 50.0, CssColor::from_rgb(255, 0, 0));
        // Check that some pixels in the rect area are red
        let data = pixmap.data();
        // Pixel at (20, 20) should be red (RGBA premultiplied)
        let idx = (20 * 100 + 20) * 4;
        assert!(data[idx as usize] > 200); // R
    }
}
