use ab_glyph::{point, Font, FontVec, PxScale, ScaleFont};
use incognidium_css::CssColor;
use incognidium_layout::{BoxType, FlatBox};
use incognidium_style::{
    ComputedStyle, Display, FontFamily, FontStyle, FontWeight, StyleMap, TextDecoration, TextDecorationLine, TextTransform,
    Visibility, WhiteSpace,
};
use std::collections::HashMap;
use std::sync::OnceLock;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

// ── TTF Font Loading ──────────────────────────────────────────

struct LoadedFonts {
    regular: FontVec,
    bold: FontVec,
    italic: FontVec,
    bold_italic: FontVec,
}

static FONTS: OnceLock<Option<LoadedFonts>> = OnceLock::new();

fn load_fonts() -> Option<LoadedFonts> {
    let search_dirs = [
        "/usr/share/fonts/truetype/liberation2",
        "/usr/share/fonts/truetype/liberation",
        "/usr/share/fonts/liberation-sans",
        "/usr/share/fonts/truetype/dejavu",
    ];
    let families = [
        // (regular, bold, italic, bold-italic) filename patterns
        (
            "LiberationSans-Regular.ttf",
            "LiberationSans-Bold.ttf",
            "LiberationSans-Italic.ttf",
            "LiberationSans-BoldItalic.ttf",
        ),
        (
            "DejaVuSans.ttf",
            "DejaVuSans-Bold.ttf",
            "DejaVuSans-Oblique.ttf",
            "DejaVuSans-BoldOblique.ttf",
        ),
    ];

    for dir in &search_dirs {
        for (reg, bld, ita, bi) in &families {
            let try_load = || -> Option<LoadedFonts> {
                let regular =
                    FontVec::try_from_vec(std::fs::read(format!("{dir}/{reg}")).ok()?).ok()?;
                let bold =
                    FontVec::try_from_vec(std::fs::read(format!("{dir}/{bld}")).ok()?).ok()?;
                let italic =
                    FontVec::try_from_vec(std::fs::read(format!("{dir}/{ita}")).ok()?).ok()?;
                let bold_italic =
                    FontVec::try_from_vec(std::fs::read(format!("{dir}/{bi}")).ok()?).ok()?;
                Some(LoadedFonts {
                    regular,
                    bold,
                    italic,
                    bold_italic,
                })
            };
            if let Some(fonts) = try_load() {
                log::info!("Loaded TTF fonts from {dir}");
                return Some(fonts);
            }
        }
    }
    log::warn!("No TTF fonts found, falling back to bitmap font");
    None
}

fn get_fonts() -> Option<&'static LoadedFonts> {
    FONTS.get_or_init(load_fonts).as_ref()
}

fn pick_font(fonts: &LoadedFonts, bold: bool, italic: bool) -> &FontVec {
    match (bold, italic) {
        (true, true) => &fonts.bold_italic,
        (true, false) => &fonts.bold,
        (false, true) => &fonts.italic,
        (false, false) => &fonts.regular,
    }
}

/// Alpha-blend a single pixel onto the pixmap.
fn blend_pixel(pixmap: &mut Pixmap, px: u32, py: u32, r: u8, g: u8, b: u8, a: u8) {
    if a == 0 {
        return;
    }
    let w = pixmap.width();
    let idx = ((py * w + px) * 4) as usize;
    let data = pixmap.data_mut();
    if idx + 3 >= data.len() {
        return;
    }

    if a == 255 {
        data[idx] = r;
        data[idx + 1] = g;
        data[idx + 2] = b;
        data[idx + 3] = 255;
    } else {
        let sa = a as u32;
        let inv = 255 - sa;
        data[idx] = ((r as u32 * sa + data[idx] as u32 * inv) / 255) as u8;
        data[idx + 1] = ((g as u32 * sa + data[idx + 1] as u32 * inv) / 255) as u8;
        data[idx + 2] = ((b as u32 * sa + data[idx + 2] as u32 * inv) / 255) as u8;
        data[idx + 3] = ((sa + data[idx + 3] as u32 * inv / 255).min(255)) as u8;
    }
}

/// Cached image data for painting.
#[derive(Clone)]
pub struct ImageData {
    pub pixels: Vec<u8>, // RGBA
    pub width: u32,
    pub height: u32,
}

/// Paint the layout tree into a pixel buffer.
pub fn paint(flat_boxes: &[FlatBox], styles: &StyleMap, width: u32, height: u32) -> Pixmap {
    paint_with_images(flat_boxes, styles, width, height, &HashMap::new())
}

/// Paint with image support.
pub fn paint_with_images(
    flat_boxes: &[FlatBox],
    styles: &StyleMap,
    width: u32,
    height: u32,
    images: &HashMap<String, ImageData>,
) -> Pixmap {
    let mut pixmap = Pixmap::new(width, height).expect("failed to create pixmap");

    // Fill background white
    pixmap.fill(Color::WHITE);

    for fbox in flat_boxes {
        let style = styles.get(&fbox.node_id).cloned().unwrap_or_default();

        if style.display == Display::None
            || style.visibility != Visibility::Visible
            || style.opacity == 0.0
        {
            continue;
        }

        // Apply opacity by modulating background/border alpha
        let opacity = style.opacity;
        let mut effective_style = style.clone();
        if opacity < 1.0 {
            effective_style.background_color.a =
                (effective_style.background_color.a as f32 * opacity) as u8;
            effective_style.border_color.a =
                (effective_style.border_color.a as f32 * opacity) as u8;
            effective_style.color.a = (effective_style.color.a as f32 * opacity) as u8;
        }
        let style = effective_style;

        // Compute effective draw bounds after clipping
        let (draw_x, draw_y, draw_w, draw_h) = if let Some((cx, cy, cw, ch)) = fbox.clip {
            let x1 = fbox.x.max(cx);
            let y1 = fbox.y.max(cy);
            let x2 = (fbox.x + fbox.width).min(cx + cw);
            let y2 = (fbox.y + fbox.height).min(cy + ch);
            if x2 <= x1 || y2 <= y1 {
                continue; // Entirely clipped
            }
            (x1, y1, x2 - x1, y2 - y1)
        } else {
            (fbox.x, fbox.y, fbox.width, fbox.height)
        };

        // Draw box shadow (behind background)
        if let Some(ref shadow) = style.box_shadow {
            draw_box_shadow(&mut pixmap, fbox.x, fbox.y, fbox.width, fbox.height, shadow);
        }

        // Draw background (clipped) - check for gradient first, then solid color
        match &style.background_image {
            incognidium_style::BackgroundImage::LinearGradient(grad) => {
                draw_linear_gradient(
                    &mut pixmap,
                    draw_x,
                    draw_y,
                    draw_w,
                    draw_h,
                    grad,
                );
            }
            _ => {
                // Fall back to solid background color (with border-radius)
                if style.background_color.a > 0 {
                    draw_rounded_rect(
                        &mut pixmap,
                        draw_x,
                        draw_y,
                        draw_w,
                        draw_h,
                        style.background_color,
                        style.border_top_left_radius,
                        style.border_top_right_radius,
                        style.border_bottom_right_radius,
                        style.border_bottom_left_radius,
                    );
                }
            }
        }

        // Draw border (only if not clipped — borders on clipped boxes look wrong)
        if fbox.clip.is_none()
            && (style.border_top_width > 0.0
                || style.border_right_width > 0.0
                || style.border_bottom_width > 0.0
                || style.border_left_width > 0.0)
        {
            draw_borders(&mut pixmap, fbox, &style);
        }

        // Draw outline (focus indicator)
        if fbox.clip.is_none()
            && style.outline_width > 0.0
            && style.outline_style != incognidium_style::OutlineStyle::None
        {
            draw_outline(&mut pixmap, fbox, &style);
        }

        // Draw checkbox/radio buttons
        if fbox.box_type == BoxType::InlineBlock {
            if let Some(input_type) = fbox.input_type {
                match input_type {
                    incognidium_layout::InputType::Checkbox { checked } => {
                        draw_checkbox(&mut pixmap, fbox.x, fbox.y, fbox.width, fbox.height, &style, checked);
                    }
                    incognidium_layout::InputType::Radio { checked } => {
                        draw_radio(&mut pixmap, fbox.x, fbox.y, fbox.width, fbox.height, &style, checked);
                    }
                    _ => {}
                }
            }
        }

        // Draw image (with clip bounds)
        if fbox.box_type == BoxType::Image {
            if let Some(ref src) = fbox.image_src {
                if let Some(img) = images.get(src) {
                    draw_image_clipped(
                        &mut pixmap,
                        fbox.x,
                        fbox.y,
                        fbox.width,
                        fbox.height,
                        img,
                        fbox.clip,
                    );
                }
            }
        }

        // Draw text (with clip bounds)
        // Only draw text for Text boxes, not for Images (alt text should not render as content)
        if fbox.box_type == BoxType::Text {
            if let Some(ref text) = fbox.text {
                if !text.is_empty() && text != " " {
                    let display_text = apply_text_transform(text, &style);
                    draw_text_clipped(
                        &mut pixmap,
                        fbox.x,
                        fbox.y,
                        fbox.width,
                        fbox.height,
                        &display_text,
                        &style,
                        fbox.clip,
                    );
                }
            }
        }

        // Draw text for InlineBlock form controls (textarea, input[type="text"])
        if fbox.box_type == BoxType::InlineBlock {
            if let Some(ref text) = fbox.text {
                if !text.is_empty() && text != " " {
                    // For textarea/input, draw text with padding offset
                    let padding_left = style.padding_left;
                    let padding_top = style.padding_top;
                    let display_text = apply_text_transform(text, &style);
                    draw_text_clipped(
                        &mut pixmap,
                        fbox.x + padding_left,
                        fbox.y + padding_top,
                        fbox.width - padding_left - style.padding_right,
                        fbox.height - padding_top - style.padding_bottom,
                        &display_text,
                        &style,
                        fbox.clip,
                    );
                }
            }
        }
    }

    pixmap
}

fn apply_text_transform(text: &str, style: &ComputedStyle) -> String {
    match style.text_transform {
        TextTransform::Uppercase => text.to_uppercase(),
        TextTransform::Lowercase => text.to_lowercase(),
        TextTransform::Capitalize => {
            let mut result = String::with_capacity(text.len());
            let mut prev_space = true;
            for c in text.chars() {
                if prev_space && c.is_alphabetic() {
                    for uc in c.to_uppercase() {
                        result.push(uc);
                    }
                } else {
                    result.push(c);
                }
                prev_space = c.is_whitespace();
            }
            result
        }
        TextTransform::None => text.to_string(),
    }
}

fn css_to_skia_color(c: CssColor) -> Color {
    Color::from_rgba8(c.r, c.g, c.b, c.a)
}

/// Draw a linear gradient background
fn draw_linear_gradient(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    gradient: &incognidium_style::LinearGradient,
) {
    use incognidium_style::GradientDirection;
    use tiny_skia::{GradientStop, LinearGradient as SkiaLinearGradient, Point, SpreadMode};

    if width <= 0.0 || height <= 0.0 {
        return;
    }

    // Calculate gradient line based on direction
    let (x1, y1, x2, y2) = match gradient.direction {
        GradientDirection::ToTop => (x + width / 2.0, y + height, x + width / 2.0, y),
        GradientDirection::ToBottom => (x + width / 2.0, y, x + width / 2.0, y + height),
        GradientDirection::ToLeft => (x + width, y + height / 2.0, x, y + height / 2.0),
        GradientDirection::ToRight => (x, y + height / 2.0, x + width, y + height / 2.0),
        GradientDirection::ToTopLeft => (x + width, y + height, x, y),
        GradientDirection::ToTopRight => (x, y + height, x + width, y),
        GradientDirection::ToBottomLeft => (x + width, y, x, y + height),
        GradientDirection::ToBottomRight => (x, y, x + width, y + height),
        GradientDirection::Angle(deg) => {
            // Convert angle to radians and calculate endpoint
            let rad = deg.to_radians();
            let cx = x + width / 2.0;
            let cy = y + height / 2.0;
            // Start and end points on the gradient line through center
            let half_diag = (width * width + height * height).sqrt() / 2.0;
            let dx = rad.sin() * half_diag;
            let dy = -rad.cos() * half_diag; // Negative because y increases downward
            (cx - dx, cy - dy, cx + dx, cy + dy)
        }
    };

    // Convert color stops
    let stops: Vec<GradientStop> = gradient
        .stops
        .iter()
        .map(|stop| GradientStop::new(
            stop.position.unwrap_or(0.0),
            css_to_skia_color(stop.color)
        ))
        .collect();

    if stops.len() < 2 {
        // Need at least 2 stops for a gradient
        return;
    }

    let rect = match Rect::from_xywh(x, y, width.max(1.0), height.max(1.0)) {
        Some(r) => r,
        None => return,
    };

    // Create the gradient
    let skia_grad = match SkiaLinearGradient::new(
        Point::from_xy(x1, y1),
        Point::from_xy(x2, y2),
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        Some(g) => g,
        None => return,
    };

    let mut paint = Paint::default();
    paint.shader = skia_grad;
    paint.anti_alias = true;

    let path = PathBuilder::from_rect(rect);
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

fn draw_rect(pixmap: &mut Pixmap, x: f32, y: f32, width: f32, height: f32, color: CssColor) {
    draw_rounded_rect(pixmap, x, y, width, height, color, 0.0, 0.0, 0.0, 0.0);
}

/// Draw a rectangle with optional rounded corners (border-radius).
fn draw_rounded_rect(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    color: CssColor,
    radius_tl: f32, // top-left
    radius_tr: f32, // top-right
    radius_br: f32, // bottom-right
    radius_bl: f32, // bottom-left
) {
    if width <= 0.0 || height <= 0.0 {
        return;
    }

    // Clamp radii to half the smaller dimension
    let max_radius = (width.min(height) / 2.0).max(0.0);
    let rtl = radius_tl.min(max_radius);
    let rtr = radius_tr.min(max_radius);
    let rbr = radius_br.min(max_radius);
    let rbl = radius_bl.min(max_radius);

    // If no rounding, use simple rect
    if rtl <= 0.0 && rtr <= 0.0 && rbr <= 0.0 && rbl <= 0.0 {
        let rect = match Rect::from_xywh(x, y, width.max(1.0), height.max(1.0)) {
            Some(r) => r,
            None => return,
        };
        let mut paint = Paint::default();
        paint.set_color(css_to_skia_color(color));
        paint.anti_alias = true;
        let path = PathBuilder::from_rect(rect);
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        return;
    }

    // Build rounded rectangle path manually
    let mut pb = PathBuilder::new();

    // Top edge: start after top-left radius
    pb.move_to(x + rtl, y);
    // Top edge to top-right radius start
    pb.line_to(x + width - rtr, y);
    // Top-right corner curve
    if rtr > 0.0 {
        // Cubic bezier approximating a quarter circle
        let cx = x + width - rtr;
        let cy = y + rtr;
        pb.cubic_to(x + width, y, x + width, y + rtr, x + width, y + rtr);
    }
    // Right edge
    pb.line_to(x + width, y + height - rbr);
    // Bottom-right corner
    if rbr > 0.0 {
        pb.cubic_to(x + width, y + height, x + width - rbr, y + height, x + width - rbr, y + height);
    }
    // Bottom edge
    pb.line_to(x + rbl, y + height);
    // Bottom-left corner
    if rbl > 0.0 {
        pb.cubic_to(x, y + height, x, y + height - rbl, x, y + height - rbl);
    }
    // Left edge
    pb.line_to(x, y + rtl);
    // Top-left corner
    if rtl > 0.0 {
        pb.cubic_to(x, y, x + rtl, y, x + rtl, y);
    }
    pb.close();

    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color(css_to_skia_color(color));
        paint.anti_alias = true;
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
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

/// Draw an outline (focus indicator) around an element.
/// Outline is drawn outside the border with an optional offset.
fn draw_outline(pixmap: &mut Pixmap, fbox: &FlatBox, style: &ComputedStyle) {
    let outline_width = style.outline_width;
    let offset = style.outline_offset;
    let oc = style.outline_color;

    // Calculate outline rectangle (outside the element + offset)
    let outline_x = fbox.x - outline_width - offset;
    let outline_y = fbox.y - outline_width - offset;
    let outline_w = fbox.width + (outline_width + offset) * 2.0;
    let outline_h = fbox.height + (outline_width + offset) * 2.0;

    // For dashed/dotted, we'd need more complex rendering, but for now
    // draw solid outline as four rects (top, bottom, left, right)
    // Top
    draw_rect(pixmap, outline_x, outline_y, outline_w, outline_width, oc);
    // Bottom
    draw_rect(
        pixmap,
        outline_x,
        outline_y + outline_h - outline_width,
        outline_w,
        outline_width,
        oc,
    );
    // Left (between top and bottom)
    draw_rect(
        pixmap,
        outline_x,
        outline_y + outline_width,
        outline_width,
        outline_h - outline_width * 2.0,
        oc,
    );
    // Right (between top and bottom)
    draw_rect(
        pixmap,
        outline_x + outline_w - outline_width,
        outline_y + outline_width,
        outline_width,
        outline_h - outline_width * 2.0,
        oc,
    );
}

/// Draw a checkbox input element
fn draw_checkbox(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    _style: &ComputedStyle,
    checked: bool,
) {
    let margin = 1.0;
    let size = width.min(height) - margin * 2.0;
    let x = x + margin;
    let y = y + margin;

    // Draw border
    let border_color = CssColor { r: 100, g: 100, b: 100, a: 255 };
    draw_rect(pixmap, x, y, size, size, border_color);

    // Draw white background
    let bg_color = CssColor { r: 255, g: 255, b: 255, a: 255 };
    draw_rect(pixmap, x + 1.0, y + 1.0, size - 2.0, size - 2.0, bg_color);

    // Draw checkmark if checked
    if checked {
        let check_color = CssColor { r: 50, g: 50, b: 50, a: 255 };
        // Simple checkmark: filled square (larger for visibility)
        let margin = size * 0.25;
        draw_rect(pixmap, x + margin, y + margin, size - margin * 2.0, size - margin * 2.0, check_color);
    }
}

/// Draw a radio button input element
fn draw_radio(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    _style: &ComputedStyle,
    checked: bool,
) {
    let margin = 1.0;
    let size = width.min(height) - margin * 2.0;
    let x = x + margin;
    let y = y + margin;

    // Draw border (circle approximation using rectangles)
    let border_color = CssColor { r: 100, g: 100, b: 100, a: 255 };
    draw_rect(pixmap, x + 2.0, y, size - 4.0, size, border_color);
    draw_rect(pixmap, x, y + 2.0, size, size - 4.0, border_color);

    // Draw white background
    let bg_color = CssColor { r: 255, g: 255, b: 255, a: 255 };
    draw_rect(pixmap, x + 2.0, y + 1.0, size - 4.0, size - 2.0, bg_color);
    draw_rect(pixmap, x + 1.0, y + 2.0, size - 2.0, size - 4.0, bg_color);

    // Draw dot if checked
    if checked {
        let dot_color = CssColor { r: 50, g: 50, b: 50, a: 255 };
        // Draw a larger dot in the center
        let margin = size * 0.3;
        draw_rect(pixmap, x + margin, y + margin, size - margin * 2.0, size - margin * 2.0, dot_color);
    }
}

/// Draw a box shadow behind an element.
fn draw_box_shadow(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    shadow: &incognidium_style::BoxShadow,
) {
    use incognidium_style::BoxShadow;

    // Calculate shadow position with offset
    let shadow_x = x + shadow.offset_x;
    let shadow_y = y + shadow.offset_y;

    // Calculate shadow size based on spread
    let shadow_width = width + shadow.spread_radius * 2.0;
    let shadow_height = height + shadow.spread_radius * 2.0;

    if shadow_width <= 0.0 || shadow_height <= 0.0 {
        return;
    }

    // Create shadow rect
    let rect = match Rect::from_xywh(
        shadow_x - shadow.spread_radius,
        shadow_y - shadow.spread_radius,
        shadow_width.max(1.0),
        shadow_height.max(1.0),
    ) {
        Some(r) => r,
        None => return,
    };

    // Build shadow color with blur consideration
    let shadow_color = shadow.color;
    let blur_radius = shadow.blur_radius;

    if blur_radius <= 0.0 {
        // No blur: draw solid shadow rect
        let mut paint = Paint::default();
        paint.set_color(css_to_skia_color(shadow_color));
        paint.anti_alias = true;

        let path = PathBuilder::from_rect(rect);
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    } else {
        // With blur: simulate by drawing multiple rects with decreasing alpha
        // This is a simplified approach - proper Gaussian blur would require
        // a more complex implementation
        let steps = (blur_radius / 2.0).max(3.0).min(10.0) as i32;

        for i in (0..=steps).rev() {
            let factor = i as f32 / steps as f32;
            let expand = blur_radius * (1.0 - factor);
            let alpha = (shadow_color.a as f32 * factor * factor) as u8;

            let expanded_rect = match Rect::from_xywh(
                rect.x() - expand,
                rect.y() - expand,
                rect.width() + expand * 2.0,
                rect.height() + expand * 2.0,
            ) {
                Some(r) => r,
                None => continue,
            };

            let mut paint = Paint::default();
            let color = CssColor {
                r: shadow_color.r,
                g: shadow_color.g,
                b: shadow_color.b,
                a: alpha,
            };
            paint.set_color(css_to_skia_color(color));
            paint.anti_alias = true;

            let path = PathBuilder::from_rect(expanded_rect);
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
}

/// Draw an image scaled to fit the given box.
#[allow(dead_code)]
fn draw_image(pixmap: &mut Pixmap, x: f32, y: f32, box_w: f32, box_h: f32, img: &ImageData) {
    if img.width == 0 || img.height == 0 {
        return;
    }
    let dst_w = box_w as u32;
    let dst_h = box_h as u32;
    let pm_w = pixmap.width();
    let pm_h = pixmap.height();
    let px_data = pixmap.data_mut();

    for dy in 0..dst_h {
        for dx in 0..dst_w {
            let px = (x as u32) + dx;
            let py = (y as u32) + dy;
            if px >= pm_w || py >= pm_h {
                continue;
            }
            // Sample from source image (nearest-neighbor scaling)
            let sx = (dx as f32 / box_w * img.width as f32) as u32;
            let sy = (dy as f32 / box_h * img.height as f32) as u32;
            let sx = sx.min(img.width - 1);
            let sy = sy.min(img.height - 1);
            let src_idx = ((sy * img.width + sx) * 4) as usize;
            let dst_idx = ((py * pm_w + px) * 4) as usize;
            if src_idx + 3 < img.pixels.len() && dst_idx + 3 < px_data.len() {
                let sa = img.pixels[src_idx + 3] as u32;
                if sa == 255 {
                    px_data[dst_idx] = img.pixels[src_idx];
                    px_data[dst_idx + 1] = img.pixels[src_idx + 1];
                    px_data[dst_idx + 2] = img.pixels[src_idx + 2];
                    px_data[dst_idx + 3] = 255;
                } else if sa > 0 {
                    // Alpha blend
                    let inv_a = 255 - sa;
                    px_data[dst_idx] = ((img.pixels[src_idx] as u32 * sa
                        + px_data[dst_idx] as u32 * inv_a)
                        / 255) as u8;
                    px_data[dst_idx + 1] = ((img.pixels[src_idx + 1] as u32 * sa
                        + px_data[dst_idx + 1] as u32 * inv_a)
                        / 255) as u8;
                    px_data[dst_idx + 2] = ((img.pixels[src_idx + 2] as u32 * sa
                        + px_data[dst_idx + 2] as u32 * inv_a)
                        / 255) as u8;
                    px_data[dst_idx + 3] = 255;
                }
            }
        }
    }
}

/// Render text — TTF with anti-aliasing if fonts are available, bitmap fallback otherwise.
fn draw_text(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    max_width: f32,
    max_height: f32,
    text: &str,
    style: &ComputedStyle,
) {
    if let Some(fonts) = get_fonts() {
        draw_text_ttf(pixmap, x, y, max_width, max_height, text, style, fonts);
    } else {
        draw_text_bitmap(pixmap, x, y, max_width, max_height, text, style);
    }
}

/// TTF text rendering with anti-aliased glyphs.
#[allow(clippy::too_many_arguments)]
fn draw_text_ttf(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    max_width: f32,
    max_height: f32,
    text: &str,
    style: &ComputedStyle,
    fonts: &LoadedFonts,
) {
    let font_size = style.font_size;
    let bold = style.font_weight == FontWeight::Bold;
    let italic = style.font_style == FontStyle::Italic;
    let font = pick_font(fonts, bold, italic);
    let scale = PxScale::from(font_size);
    let scaled = font.as_scaled(scale);
    let line_height = font_size * style.line_height;
    let color = style.color;

    let ascent = scaled.ascent();
    let space_width = scaled.h_advance(scaled.glyph_id(' ')) + style.word_spacing;
    let letter_spacing = style.letter_spacing;

    let mut cursor_x = x;
    let mut cursor_y = y;

    if text.starts_with(' ') {
        cursor_x += space_width;
    }

    // Split on newlines first, then whitespace within each line
    let lines: Vec<&str> = text.split('\n').collect();
    let mut rendered_end_x = cursor_x;

    for (li, line) in lines.iter().enumerate() {
        if li > 0 {
            // New line - move to next line
            cursor_x = x;
            cursor_y += line_height;
        }

        // Skip empty lines (but still advance cursor_y)
        if line.is_empty() {
            continue;
        }

        // For nowrap, treat entire line as single word to prevent wrapping
        let nowrap = matches!(
            style.white_space,
            WhiteSpace::NoWrap | WhiteSpace::Pre
        );

        let words: Vec<&str> = if nowrap {
            vec![line]
        } else {
            line.split_whitespace().collect()
        };
        let line_start_x = cursor_x;

        for (wi, word) in words.iter().enumerate() {
            let word_width: f32 = word
                .chars()
                .map(|c| scaled.h_advance(scaled.glyph_id(c)) + letter_spacing)
                .sum::<f32>()
                - if word.chars().count() > 0 { letter_spacing } else { 0.0 };

            // Check for wrap (but not on first word of a line, and not when nowrap)
            if !nowrap && cursor_x > line_start_x && cursor_x + word_width > x + max_width + 0.5 {
                cursor_x = x;
                cursor_y += line_height;
            }

            if max_height > 0.0 && cursor_y + font_size > y + max_height + font_size * 0.5 {
                break;
            }

            // Render each glyph
            let mut prev_glyph = None;
            for ch in word.chars() {
                let glyph_id = scaled.glyph_id(ch);

                // Kerning
                if let Some(prev) = prev_glyph {
                    cursor_x += scaled.kern(prev, glyph_id);
                }

                // Check if this glyph would exceed max_width (for overflow:hidden)
                let glyph_width = scaled.h_advance(glyph_id);
                if max_width > 0.0 && cursor_x + glyph_width > x + max_width + 0.5 {
                    break; // Stop rendering this word
                }

                // Text shadow (render first, behind text)
                if let Some(shadow) = style.text_shadow {
                    let shadow_x = cursor_x + shadow.offset_x;
                    let shadow_y = cursor_y + ascent + shadow.offset_y;
                    let shadow_glyph = glyph_id.with_scale_and_position(scale, point(shadow_x, shadow_y));
                    if let Some(outlined) = font.outline_glyph(shadow_glyph) {
                        let bounds = outlined.px_bounds();
                        let shadow_color = shadow.color;
                        outlined.draw(|gx, gy, coverage| {
                            let px = gx as i32 + bounds.min.x as i32;
                            let py = gy as i32 + bounds.min.y as i32;
                            if px >= 0 && py >= 0 {
                                let px = px as u32;
                                let py = py as u32;
                                if px < pixmap.width() && py < pixmap.height() {
                                            let alpha = (coverage * shadow_color.a as f32) as u8;
                                            blend_pixel(pixmap, px, py, shadow_color.r, shadow_color.g, shadow_color.b, alpha);
                                }
                            }
                        });
                    }
                }

                // Use fractional positioning for smoother text (Chrome-style)
                let glyph = glyph_id.with_scale_and_position(scale, point(cursor_x, cursor_y + ascent));
                if let Some(outlined) = font.outline_glyph(glyph) {
                    let bounds = outlined.px_bounds();
                    outlined.draw(|gx, gy, coverage| {
                        let px = gx as i32 + bounds.min.x as i32;
                        let py = gy as i32 + bounds.min.y as i32;
                        if px >= 0 && py >= 0 {
                            let px = px as u32;
                            let py = py as u32;
                            if px < pixmap.width() && py < pixmap.height() {
                                let alpha = (coverage * color.a as f32) as u8;
                                blend_pixel(pixmap, px, py, color.r, color.g, color.b, alpha);
                            }
                        }
                    });
                }

                cursor_x += glyph_width + letter_spacing;
                prev_glyph = Some(glyph_id);
            }

            // Remove extra letter-spacing added after last char
            if letter_spacing > 0.0 && !word.is_empty() {
                cursor_x -= letter_spacing;
            }

            rendered_end_x = cursor_x;

            if wi < words.len() - 1 {
                cursor_x += space_width;
            }
        }
    }

    // Text decorations (underline, line-through, overline)
    use incognidium_style::{TextDecoration, TextDecorationLine};

    // Check both old text_decoration and new text_decoration_line
    let has_underline = style.text_decoration == TextDecoration::Underline
        || style.text_decoration_line == TextDecorationLine::Underline;
    let has_line_through = style.text_decoration_line == TextDecorationLine::LineThrough;
    let has_overline = style.text_decoration_line == TextDecorationLine::Overline;

    if has_underline || has_line_through || has_overline {
        let decor_x = if text.starts_with(' ') {
            x + space_width
        } else {
            x
        };
        let decor_w = (rendered_end_x - decor_x).min(max_width);
        if decor_w > 0.0 {
            let line_thickness = 1.0_f32.max(font_size * 0.05);

            if has_underline {
                let ul_y = y + ascent + 2.0;
                draw_rect(pixmap, decor_x, ul_y, decor_w, line_thickness, color);
            }

            if has_line_through {
                // Middle of the text (approximate with x-height)
                let lt_y = y + ascent * 0.35;
                draw_rect(pixmap, decor_x, lt_y, decor_w, line_thickness, color);
            }

            if has_overline {
                // Above the text
                let ol_y = y + ascent - font_size * 0.85;
                draw_rect(pixmap, decor_x, ol_y, decor_w, line_thickness, color);
            }
        }
    }
}

/// Bitmap fallback text rendering (monospace segments).
fn draw_text_bitmap(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    max_width: f32,
    max_height: f32,
    text: &str,
    style: &ComputedStyle,
) {
    let font_size = style.font_size;
    let char_width = font_size * 0.6;
    let line_height = font_size * style.line_height;
    let color = style.color;
    let bold = style.font_weight == FontWeight::Bold;

    let mut cursor_x = x;
    let mut cursor_y = y;

    if text.starts_with(' ') {
        cursor_x += char_width;
    }

    // Split on newlines first, then whitespace within each line
    let lines: Vec<&str> = text.split('\n').collect();

    for (li, line) in lines.iter().enumerate() {
        if li > 0 {
            // New line
            cursor_x = x;
            cursor_y += line_height;
        }

        // Skip empty lines
        if line.is_empty() {
            continue;
        }

        // For nowrap, treat entire line as single word to prevent wrapping
        let nowrap = matches!(
            style.white_space,
            WhiteSpace::NoWrap | WhiteSpace::Pre
        );

        let words: Vec<&str> = if nowrap {
            vec![line]
        } else {
            line.split_whitespace().collect()
        };
        let line_start_x = cursor_x;

        for (wi, word) in words.iter().enumerate() {
            let word_width = word.len() as f32 * char_width;

            let would_wrap = !nowrap && cursor_x > line_start_x && cursor_x + word_width > x + max_width + 0.5;
            if would_wrap {
                cursor_x = x;
                cursor_y += line_height;
            }

            if max_height > 0.0 && cursor_y + font_size > y + max_height + font_size * 0.5 {
                break;
            }

            for ch in word.chars() {
                // Check if this character would exceed max_width (for overflow:hidden)
                if max_width > 0.0 && cursor_x + char_width > x + max_width + 0.5 {
                    break;
                }
                draw_bitmap_char(pixmap, cursor_x, cursor_y, ch, font_size, color, bold);
                cursor_x += char_width;
            }

            if wi < words.len() - 1 {
                cursor_x += char_width;
            }
        }
    }

    // Text decorations for bitmap fallback
    let has_underline = style.text_decoration == TextDecoration::Underline
        || style.text_decoration_line == TextDecorationLine::Underline;
    let has_line_through = style.text_decoration_line == TextDecorationLine::LineThrough;
    let has_overline = style.text_decoration_line == TextDecorationLine::Overline;

    if has_underline || has_line_through || has_overline {
        let trimmed = text.trim();
        let total_chars = trimmed.chars().count();
        let text_width = total_chars as f32 * char_width;
        let decor_x = if text.starts_with(' ') {
            x + char_width
        } else {
            x
        };
        let line_thickness = 1.0_f32.max(font_size * 0.05);

        if has_underline {
            let underline_y = y + font_size;
            draw_rect(
                pixmap,
                decor_x,
                underline_y,
                text_width.min(max_width),
                line_thickness,
                color,
            );
        }

        if has_line_through {
            let lt_y = y + font_size * 0.55;
            draw_rect(
                pixmap,
                decor_x,
                lt_y,
                text_width.min(max_width),
                line_thickness,
                color,
            );
        }

        if has_overline {
            let overline_y = y + font_size * 0.15;
            draw_rect(
                pixmap,
                decor_x,
                overline_y,
                text_width.min(max_width),
                line_thickness,
                color,
            );
        }
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

/// Draw an image with optional clipping.
fn draw_image_clipped(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    box_w: f32,
    box_h: f32,
    img: &ImageData,
    clip: Option<(f32, f32, f32, f32)>,
) {
    if img.width == 0 || img.height == 0 || box_w <= 0.0 || box_h <= 0.0 {
        return;
    }
    let dst_w = box_w as u32;
    let dst_h = box_h as u32;
    let pm_w = pixmap.width();
    let pm_h = pixmap.height();

    let (clip_x1, clip_y1, clip_x2, clip_y2) = if let Some((cx, cy, cw, ch)) = clip {
        (
            cx.max(0.0) as u32,
            cy.max(0.0) as u32,
            (cx + cw).max(0.0) as u32,
            (cy + ch).max(0.0) as u32,
        )
    } else {
        (0, 0, pm_w, pm_h)
    };

    let sx_ratio = img.width as f32 / box_w;
    let sy_ratio = img.height as f32 / box_h;
    let iw = img.width as i32;
    let ih = img.height as i32;
    let px_data = pixmap.data_mut();

    for dy in 0..dst_h {
        for dx in 0..dst_w {
            let px = (x as u32) + dx;
            let py = (y as u32) + dy;
            if px >= pm_w
                || py >= pm_h
                || px < clip_x1
                || py < clip_y1
                || px >= clip_x2
                || py >= clip_y2
            {
                continue;
            }
            // Bilinear sample: map dst pixel center (dx+0.5, dy+0.5) to src.
            let fx = (dx as f32 + 0.5) * sx_ratio - 0.5;
            let fy = (dy as f32 + 0.5) * sy_ratio - 0.5;
            let x0 = fx.floor() as i32;
            let y0 = fy.floor() as i32;
            let tx = fx - x0 as f32;
            let ty = fy - y0 as f32;
            let x1 = x0 + 1;
            let y1 = y0 + 1;
            let sample = |sx: i32, sy: i32| -> [u32; 4] {
                let cx = sx.clamp(0, iw - 1) as u32;
                let cy = sy.clamp(0, ih - 1) as u32;
                let i = ((cy * img.width + cx) * 4) as usize;
                if i + 3 < img.pixels.len() {
                    [
                        img.pixels[i] as u32,
                        img.pixels[i + 1] as u32,
                        img.pixels[i + 2] as u32,
                        img.pixels[i + 3] as u32,
                    ]
                } else {
                    [0, 0, 0, 0]
                }
            };
            let p00 = sample(x0, y0);
            let p10 = sample(x1, y0);
            let p01 = sample(x0, y1);
            let p11 = sample(x1, y1);
            // Weights (fixed-point 0..256 for perf)
            let wx1 = (tx * 256.0) as u32;
            let wx0 = 256 - wx1;
            let wy1 = (ty * 256.0) as u32;
            let wy0 = 256 - wy1;
            let mix = |a: u32, b: u32, c: u32, d: u32| -> u32 {
                let top = a * wx0 + b * wx1;
                let bot = c * wx0 + d * wx1;
                (top * wy0 + bot * wy1) >> 16
            };
            let sr = mix(p00[0], p10[0], p01[0], p11[0]);
            let sg = mix(p00[1], p10[1], p01[1], p11[1]);
            let sb = mix(p00[2], p10[2], p01[2], p11[2]);
            let sa = mix(p00[3], p10[3], p01[3], p11[3]);
            let dst_idx = ((py * pm_w + px) * 4) as usize;
            if dst_idx + 3 >= px_data.len() {
                continue;
            }
            if sa >= 255 {
                px_data[dst_idx] = sr as u8;
                px_data[dst_idx + 1] = sg as u8;
                px_data[dst_idx + 2] = sb as u8;
                px_data[dst_idx + 3] = 255;
            } else if sa > 0 {
                let inv_a = 255 - sa;
                px_data[dst_idx] = ((sr * sa + px_data[dst_idx] as u32 * inv_a) / 255) as u8;
                px_data[dst_idx + 1] =
                    ((sg * sa + px_data[dst_idx + 1] as u32 * inv_a) / 255) as u8;
                px_data[dst_idx + 2] =
                    ((sb * sa + px_data[dst_idx + 2] as u32 * inv_a) / 255) as u8;
                px_data[dst_idx + 3] = 255;
            }
        }
    }
}

/// Draw text with optional clipping.
#[allow(clippy::too_many_arguments)]
fn draw_text_clipped(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    max_width: f32,
    max_height: f32,
    text: &str,
    style: &ComputedStyle,
    clip: Option<(f32, f32, f32, f32)>,
) {
    if let Some((cx, cy, cw, ch)) = clip {
        let eff_w = (x + max_width).min(cx + cw) - x;
        let eff_h = (y + max_height).min(cy + ch) - y;
        if eff_w > 0.0 && eff_h > 0.0 {
            draw_text(pixmap, x, y, eff_w, eff_h, text, style);
        }
    } else {
        draw_text(pixmap, x, y, max_width, max_height, text, style);
    }
}

/// Return line segments for rendering a character.
/// Format: (x1, y1, x2, y2) in a 10x16 grid.
fn glyph_segments(ch: char) -> Vec<(f32, f32, f32, f32)> {
    match ch {
        // Uppercase letters
        'A' => vec![
            (1.0, 14.0, 5.0, 2.0),
            (5.0, 2.0, 9.0, 14.0),
            (3.0, 9.0, 7.0, 9.0),
        ],
        'a' => vec![
            (9.0, 6.0, 9.0, 14.0),
            (9.0, 6.0, 5.0, 6.0),
            (5.0, 6.0, 1.0, 8.0),
            (1.0, 8.0, 1.0, 12.0),
            (1.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 9.0, 14.0),
        ],
        'B' => vec![
            (2.0, 2.0, 2.0, 14.0),
            (2.0, 2.0, 7.0, 2.0),
            (7.0, 2.0, 8.0, 5.0),
            (8.0, 5.0, 7.0, 8.0),
            (2.0, 8.0, 7.0, 8.0),
            (7.0, 8.0, 8.0, 11.0),
            (8.0, 11.0, 7.0, 14.0),
            (2.0, 14.0, 7.0, 14.0),
        ],
        'b' => vec![
            (2.0, 2.0, 2.0, 14.0),
            (2.0, 9.0, 5.0, 6.0),
            (5.0, 6.0, 8.0, 8.0),
            (8.0, 8.0, 8.0, 12.0),
            (8.0, 12.0, 5.0, 14.0),
            (2.0, 14.0, 5.0, 14.0),
        ],
        'C' => vec![
            (8.0, 3.0, 5.0, 2.0),
            (5.0, 2.0, 2.0, 4.0),
            (2.0, 4.0, 2.0, 12.0),
            (2.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 8.0, 13.0),
        ],
        'c' => vec![
            (8.0, 7.0, 5.0, 6.0),
            (5.0, 6.0, 2.0, 8.0),
            (2.0, 8.0, 2.0, 12.0),
            (2.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 8.0, 13.0),
        ],
        'D' => vec![
            (2.0, 2.0, 2.0, 14.0),
            (2.0, 2.0, 6.0, 2.0),
            (6.0, 2.0, 8.0, 5.0),
            (8.0, 5.0, 8.0, 11.0),
            (8.0, 11.0, 6.0, 14.0),
            (2.0, 14.0, 6.0, 14.0),
        ],
        'd' => vec![
            (8.0, 2.0, 8.0, 14.0),
            (8.0, 9.0, 5.0, 6.0),
            (5.0, 6.0, 2.0, 8.0),
            (2.0, 8.0, 2.0, 12.0),
            (2.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 8.0, 14.0),
        ],
        'E' => vec![
            (2.0, 2.0, 2.0, 14.0),
            (2.0, 2.0, 8.0, 2.0),
            (2.0, 8.0, 7.0, 8.0),
            (2.0, 14.0, 8.0, 14.0),
        ],
        'e' => vec![
            (2.0, 10.0, 8.0, 10.0),
            (8.0, 10.0, 8.0, 8.0),
            (8.0, 8.0, 5.0, 6.0),
            (5.0, 6.0, 2.0, 8.0),
            (2.0, 8.0, 2.0, 12.0),
            (2.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 8.0, 13.0),
        ],
        'F' => vec![
            (2.0, 2.0, 2.0, 14.0),
            (2.0, 2.0, 8.0, 2.0),
            (2.0, 8.0, 7.0, 8.0),
        ],
        'f' => vec![
            (7.0, 2.0, 6.0, 2.0),
            (6.0, 2.0, 5.0, 4.0),
            (5.0, 4.0, 5.0, 14.0),
            (3.0, 7.0, 7.0, 7.0),
        ],
        'G' => vec![
            (8.0, 3.0, 5.0, 2.0),
            (5.0, 2.0, 2.0, 4.0),
            (2.0, 4.0, 2.0, 12.0),
            (2.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 8.0, 12.0),
            (8.0, 12.0, 8.0, 8.0),
            (5.0, 8.0, 8.0, 8.0),
        ],
        'g' => vec![
            (2.0, 8.0, 2.0, 12.0),
            (2.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 8.0, 14.0),
            (8.0, 6.0, 8.0, 15.0),
            (8.0, 15.0, 5.0, 16.0),
            (5.0, 16.0, 2.0, 15.0),
            (8.0, 6.0, 5.0, 6.0),
            (5.0, 6.0, 2.0, 8.0),
        ],
        'H' => vec![
            (2.0, 2.0, 2.0, 14.0),
            (8.0, 2.0, 8.0, 14.0),
            (2.0, 8.0, 8.0, 8.0),
        ],
        'h' => vec![
            (2.0, 2.0, 2.0, 14.0),
            (2.0, 9.0, 5.0, 6.0),
            (5.0, 6.0, 8.0, 8.0),
            (8.0, 8.0, 8.0, 14.0),
        ],
        'I' => vec![
            (3.0, 2.0, 7.0, 2.0),
            (5.0, 2.0, 5.0, 14.0),
            (3.0, 14.0, 7.0, 14.0),
        ],
        'i' => vec![
            (5.0, 3.0, 5.0, 5.0),
            (5.0, 7.0, 5.0, 14.0),
            (3.0, 14.0, 7.0, 14.0),
        ],
        'J' => vec![
            (4.0, 2.0, 8.0, 2.0),
            (7.0, 2.0, 7.0, 12.0),
            (7.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 3.0, 12.0),
        ],
        'j' => vec![
            (6.0, 3.0, 6.0, 5.0),
            (6.0, 7.0, 6.0, 15.0),
            (6.0, 15.0, 4.0, 16.0),
            (4.0, 16.0, 2.0, 15.0),
        ],
        'K' => vec![
            (2.0, 2.0, 2.0, 14.0),
            (8.0, 2.0, 2.0, 8.0),
            (2.0, 8.0, 8.0, 14.0),
        ],
        'k' => vec![
            (2.0, 2.0, 2.0, 14.0),
            (8.0, 6.0, 2.0, 10.0),
            (2.0, 10.0, 8.0, 14.0),
        ],
        'L' => vec![(2.0, 2.0, 2.0, 14.0), (2.0, 14.0, 8.0, 14.0)],
        'l' => vec![
            (4.0, 2.0, 5.0, 2.0),
            (5.0, 2.0, 5.0, 14.0),
            (5.0, 14.0, 7.0, 14.0),
        ],
        'M' => vec![
            (1.0, 14.0, 1.0, 2.0),
            (1.0, 2.0, 5.0, 8.0),
            (5.0, 8.0, 9.0, 2.0),
            (9.0, 2.0, 9.0, 14.0),
        ],
        'm' => vec![
            (1.0, 14.0, 1.0, 6.0),
            (1.0, 7.0, 4.0, 6.0),
            (4.0, 6.0, 5.0, 7.0),
            (5.0, 7.0, 5.0, 14.0),
            (5.0, 7.0, 8.0, 6.0),
            (8.0, 6.0, 9.0, 7.0),
            (9.0, 7.0, 9.0, 14.0),
        ],
        'N' => vec![
            (2.0, 14.0, 2.0, 2.0),
            (2.0, 2.0, 8.0, 14.0),
            (8.0, 14.0, 8.0, 2.0),
        ],
        'n' => vec![
            (2.0, 14.0, 2.0, 6.0),
            (2.0, 7.0, 5.0, 6.0),
            (5.0, 6.0, 8.0, 8.0),
            (8.0, 8.0, 8.0, 14.0),
        ],
        'O' => vec![
            (3.0, 2.0, 7.0, 2.0),
            (7.0, 2.0, 9.0, 4.0),
            (9.0, 4.0, 9.0, 12.0),
            (9.0, 12.0, 7.0, 14.0),
            (7.0, 14.0, 3.0, 14.0),
            (3.0, 14.0, 1.0, 12.0),
            (1.0, 12.0, 1.0, 4.0),
            (1.0, 4.0, 3.0, 2.0),
        ],
        'o' => vec![
            (3.0, 6.0, 7.0, 6.0),
            (7.0, 6.0, 9.0, 8.0),
            (9.0, 8.0, 9.0, 12.0),
            (9.0, 12.0, 7.0, 14.0),
            (3.0, 14.0, 7.0, 14.0),
            (3.0, 14.0, 1.0, 12.0),
            (1.0, 12.0, 1.0, 8.0),
            (1.0, 8.0, 3.0, 6.0),
        ],
        'P' => vec![
            (2.0, 2.0, 2.0, 14.0),
            (2.0, 2.0, 7.0, 2.0),
            (7.0, 2.0, 8.0, 5.0),
            (8.0, 5.0, 7.0, 8.0),
            (2.0, 8.0, 7.0, 8.0),
        ],
        'p' => vec![
            (2.0, 6.0, 2.0, 16.0),
            (2.0, 9.0, 5.0, 6.0),
            (5.0, 6.0, 8.0, 8.0),
            (8.0, 8.0, 8.0, 12.0),
            (8.0, 12.0, 5.0, 14.0),
            (2.0, 14.0, 5.0, 14.0),
        ],
        'Q' => vec![
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
        'q' => vec![
            (8.0, 6.0, 8.0, 16.0),
            (8.0, 9.0, 5.0, 6.0),
            (5.0, 6.0, 2.0, 8.0),
            (2.0, 8.0, 2.0, 12.0),
            (2.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 8.0, 14.0),
        ],
        'R' => vec![
            (2.0, 2.0, 2.0, 14.0),
            (2.0, 2.0, 7.0, 2.0),
            (7.0, 2.0, 8.0, 5.0),
            (8.0, 5.0, 7.0, 8.0),
            (2.0, 8.0, 7.0, 8.0),
            (5.0, 8.0, 8.0, 14.0),
        ],
        'r' => vec![
            (2.0, 6.0, 2.0, 14.0),
            (2.0, 7.0, 5.0, 6.0),
            (5.0, 6.0, 8.0, 7.0),
        ],
        'S' => vec![
            (8.0, 3.0, 5.0, 2.0),
            (5.0, 2.0, 2.0, 4.0),
            (2.0, 4.0, 3.0, 7.0),
            (3.0, 7.0, 7.0, 9.0),
            (7.0, 9.0, 8.0, 12.0),
            (8.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 2.0, 13.0),
        ],
        's' => vec![
            (8.0, 7.0, 5.0, 6.0),
            (5.0, 6.0, 2.0, 8.0),
            (2.0, 8.0, 8.0, 12.0),
            (8.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 2.0, 13.0),
        ],
        'T' => vec![(1.0, 2.0, 9.0, 2.0), (5.0, 2.0, 5.0, 14.0)],
        't' => vec![
            (5.0, 2.0, 5.0, 12.0),
            (5.0, 12.0, 7.0, 14.0),
            (7.0, 14.0, 8.0, 14.0),
            (3.0, 7.0, 7.0, 7.0),
        ],
        'U' => vec![
            (2.0, 2.0, 2.0, 12.0),
            (2.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 8.0, 12.0),
            (8.0, 12.0, 8.0, 2.0),
        ],
        'u' => vec![
            (2.0, 6.0, 2.0, 12.0),
            (2.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 8.0, 14.0),
            (8.0, 6.0, 8.0, 14.0),
        ],
        'V' => vec![(1.0, 2.0, 5.0, 14.0), (5.0, 14.0, 9.0, 2.0)],
        'v' => vec![(2.0, 6.0, 5.0, 14.0), (5.0, 14.0, 8.0, 6.0)],
        'W' => vec![
            (0.0, 2.0, 2.0, 14.0),
            (2.0, 14.0, 5.0, 8.0),
            (5.0, 8.0, 8.0, 14.0),
            (8.0, 14.0, 10.0, 2.0),
        ],
        'w' => vec![
            (0.0, 6.0, 2.0, 14.0),
            (2.0, 14.0, 5.0, 9.0),
            (5.0, 9.0, 8.0, 14.0),
            (8.0, 14.0, 10.0, 6.0),
        ],
        'X' => vec![(2.0, 2.0, 8.0, 14.0), (8.0, 2.0, 2.0, 14.0)],
        'x' => vec![(2.0, 6.0, 8.0, 14.0), (8.0, 6.0, 2.0, 14.0)],
        'Y' => vec![
            (1.0, 2.0, 5.0, 8.0),
            (9.0, 2.0, 5.0, 8.0),
            (5.0, 8.0, 5.0, 14.0),
        ],
        'y' => vec![
            (2.0, 6.0, 5.0, 10.0),
            (8.0, 6.0, 5.0, 10.0),
            (5.0, 10.0, 3.0, 16.0),
        ],
        'Z' => vec![
            (2.0, 2.0, 8.0, 2.0),
            (8.0, 2.0, 2.0, 14.0),
            (2.0, 14.0, 8.0, 14.0),
        ],
        'z' => vec![
            (2.0, 6.0, 8.0, 6.0),
            (8.0, 6.0, 2.0, 14.0),
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
        '7' => vec![(2.0, 2.0, 8.0, 2.0), (8.0, 2.0, 4.0, 14.0)],
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
        '.' => vec![
            (4.0, 13.0, 6.0, 13.0),
            (4.0, 13.0, 4.0, 14.0),
            (6.0, 13.0, 6.0, 14.0),
            (4.0, 14.0, 6.0, 14.0),
        ],
        ',' => vec![(5.0, 12.0, 5.0, 14.0), (5.0, 14.0, 4.0, 15.0)],
        ':' => vec![
            (4.5, 5.0, 5.5, 5.0),
            (4.5, 5.0, 4.5, 6.0),
            (5.5, 5.0, 5.5, 6.0),
            (4.5, 6.0, 5.5, 6.0),
            (4.5, 12.0, 5.5, 12.0),
            (4.5, 12.0, 4.5, 13.0),
            (5.5, 12.0, 5.5, 13.0),
            (4.5, 13.0, 5.5, 13.0),
        ],
        ';' => vec![
            (4.5, 5.0, 5.5, 5.0),
            (4.5, 5.0, 4.5, 6.0),
            (5.5, 5.0, 5.5, 6.0),
            (5.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 4.0, 15.0),
        ],
        '!' => vec![
            (5.0, 2.0, 5.0, 10.0),
            (4.5, 12.0, 5.5, 12.0),
            (4.5, 12.0, 4.5, 13.0),
            (5.5, 12.0, 5.5, 13.0),
            (4.5, 13.0, 5.5, 13.0),
        ],
        '?' => vec![
            (2.0, 4.0, 3.0, 2.0),
            (3.0, 2.0, 7.0, 2.0),
            (7.0, 2.0, 8.0, 4.0),
            (8.0, 4.0, 5.0, 8.0),
            (5.0, 8.0, 5.0, 10.0),
            (4.5, 12.0, 5.5, 12.0),
            (4.5, 12.0, 4.5, 13.0),
            (5.5, 12.0, 5.5, 13.0),
        ],
        '-' => vec![(2.0, 8.0, 8.0, 8.0)],
        '_' => vec![(1.0, 14.0, 9.0, 14.0)],
        '+' => vec![(5.0, 4.0, 5.0, 12.0), (2.0, 8.0, 8.0, 8.0)],
        '=' => vec![(2.0, 6.0, 8.0, 6.0), (2.0, 10.0, 8.0, 10.0)],
        '/' => vec![(8.0, 2.0, 2.0, 14.0)],
        '\\' => vec![(2.0, 2.0, 8.0, 14.0)],
        '(' => vec![
            (6.0, 1.0, 4.0, 4.0),
            (4.0, 4.0, 4.0, 12.0),
            (4.0, 12.0, 6.0, 15.0),
        ],
        ')' => vec![
            (4.0, 1.0, 6.0, 4.0),
            (6.0, 4.0, 6.0, 12.0),
            (6.0, 12.0, 4.0, 15.0),
        ],
        '[' => vec![
            (3.0, 1.0, 7.0, 1.0),
            (3.0, 1.0, 3.0, 15.0),
            (3.0, 15.0, 7.0, 15.0),
        ],
        ']' => vec![
            (3.0, 1.0, 7.0, 1.0),
            (7.0, 1.0, 7.0, 15.0),
            (3.0, 15.0, 7.0, 15.0),
        ],
        '{' => vec![
            (6.0, 1.0, 5.0, 2.0),
            (5.0, 2.0, 5.0, 6.0),
            (5.0, 6.0, 3.0, 8.0),
            (3.0, 8.0, 5.0, 10.0),
            (5.0, 10.0, 5.0, 14.0),
            (5.0, 14.0, 6.0, 15.0),
        ],
        '}' => vec![
            (4.0, 1.0, 5.0, 2.0),
            (5.0, 2.0, 5.0, 6.0),
            (5.0, 6.0, 7.0, 8.0),
            (7.0, 8.0, 5.0, 10.0),
            (5.0, 10.0, 5.0, 14.0),
            (5.0, 14.0, 4.0, 15.0),
        ],
        '<' => vec![(8.0, 3.0, 2.0, 8.0), (2.0, 8.0, 8.0, 13.0)],
        '>' => vec![(2.0, 3.0, 8.0, 8.0), (8.0, 8.0, 2.0, 13.0)],
        '"' | '\u{201C}' | '\u{201D}' => vec![(3.0, 2.0, 3.0, 5.0), (7.0, 2.0, 7.0, 5.0)],
        '\'' | '\u{2018}' | '\u{2019}' => vec![(5.0, 2.0, 5.0, 5.0)],
        '#' => vec![
            (3.0, 3.0, 3.0, 13.0),
            (7.0, 3.0, 7.0, 13.0),
            (1.0, 6.0, 9.0, 6.0),
            (1.0, 10.0, 9.0, 10.0),
        ],
        '@' => vec![
            (8.0, 4.0, 5.0, 2.0),
            (5.0, 2.0, 2.0, 4.0),
            (2.0, 4.0, 2.0, 12.0),
            (2.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 8.0, 12.0),
            (6.0, 6.0, 6.0, 10.0),
            (6.0, 10.0, 8.0, 10.0),
            (8.0, 4.0, 8.0, 10.0),
        ],
        '&' => vec![
            (6.0, 2.0, 4.0, 2.0),
            (4.0, 2.0, 3.0, 4.0),
            (3.0, 4.0, 4.0, 7.0),
            (4.0, 7.0, 2.0, 12.0),
            (2.0, 12.0, 4.0, 14.0),
            (4.0, 14.0, 6.0, 14.0),
            (6.0, 14.0, 8.0, 12.0),
            (4.0, 7.0, 8.0, 10.0),
        ],
        '*' => vec![
            (5.0, 3.0, 5.0, 11.0),
            (2.0, 5.0, 8.0, 9.0),
            (2.0, 9.0, 8.0, 5.0),
        ],
        '%' => vec![
            (2.0, 2.0, 4.0, 2.0),
            (2.0, 2.0, 2.0, 4.0),
            (4.0, 2.0, 4.0, 4.0),
            (2.0, 4.0, 4.0, 4.0),
            (8.0, 2.0, 2.0, 14.0),
            (6.0, 12.0, 8.0, 12.0),
            (6.0, 12.0, 6.0, 14.0),
            (8.0, 12.0, 8.0, 14.0),
            (6.0, 14.0, 8.0, 14.0),
        ],
        '$' => vec![
            (7.0, 3.0, 3.0, 3.0),
            (3.0, 3.0, 2.0, 5.0),
            (2.0, 5.0, 3.0, 7.0),
            (3.0, 7.0, 7.0, 9.0),
            (7.0, 9.0, 8.0, 11.0),
            (8.0, 11.0, 7.0, 13.0),
            (7.0, 13.0, 3.0, 13.0),
            (5.0, 1.0, 5.0, 15.0),
        ],
        '^' => vec![(2.0, 5.0, 5.0, 2.0), (5.0, 2.0, 8.0, 5.0)],
        '~' => vec![
            (1.0, 8.0, 3.0, 6.0),
            (3.0, 6.0, 5.0, 8.0),
            (5.0, 8.0, 7.0, 6.0),
            (7.0, 6.0, 9.0, 8.0),
        ],
        '`' => vec![(4.0, 2.0, 6.0, 4.0)],
        '|' => vec![(5.0, 1.0, 5.0, 15.0)],
        // Common unicode chars
        '\u{2013}' | '\u{2014}' => vec![(1.0, 8.0, 9.0, 8.0)], // en-dash, em-dash
        '\u{2026}' => vec![
            (2.0, 13.0, 3.0, 14.0),
            (5.0, 13.0, 6.0, 14.0),
            (8.0, 13.0, 9.0, 14.0),
        ], // ellipsis
        '\u{00A0}' => vec![], // non-breaking space — render nothing, spacing is in layout
        '\u{00B7}' => vec![(4.5, 7.5, 5.5, 8.5)], // middle dot
        '\u{2022}' => vec![
            (3.0, 6.0, 7.0, 6.0),
            (3.0, 6.0, 3.0, 10.0),
            (7.0, 6.0, 7.0, 10.0),
            (3.0, 10.0, 7.0, 10.0),
        ], // bullet
        '\u{00E9}' => vec![
            (2.0, 10.0, 8.0, 10.0),
            (8.0, 10.0, 8.0, 8.0),
            (8.0, 8.0, 5.0, 6.0),
            (5.0, 6.0, 2.0, 8.0),
            (2.0, 8.0, 2.0, 12.0),
            (2.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 8.0, 13.0),
            (6.0, 3.0, 5.0, 5.0),
        ], // é
        '\u{00EA}' => vec![
            (2.0, 10.0, 8.0, 10.0),
            (8.0, 10.0, 8.0, 8.0),
            (8.0, 8.0, 5.0, 6.0),
            (5.0, 6.0, 2.0, 8.0),
            (2.0, 8.0, 2.0, 12.0),
            (2.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 8.0, 13.0),
            (4.0, 4.0, 5.0, 3.0),
            (5.0, 3.0, 6.0, 4.0),
        ], // ê
        '\u{00E8}' => vec![
            (2.0, 10.0, 8.0, 10.0),
            (8.0, 10.0, 8.0, 8.0),
            (8.0, 8.0, 5.0, 6.0),
            (5.0, 6.0, 2.0, 8.0),
            (2.0, 8.0, 2.0, 12.0),
            (2.0, 12.0, 5.0, 14.0),
            (5.0, 14.0, 8.0, 13.0),
            (4.0, 5.0, 5.0, 3.0),
        ], // è
        _ => {
            // Unknown character: render as empty space instead of ugly box
            vec![]
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
        draw_rect(
            &mut pixmap,
            10.0,
            10.0,
            50.0,
            50.0,
            CssColor::from_rgb(255, 0, 0),
        );
        // Check that some pixels in the rect area are red
        let data = pixmap.data();
        // Pixel at (20, 20) should be red (RGBA premultiplied)
        let idx = (20 * 100 + 20) * 4;
        assert!(data[idx as usize] > 200); // R
    }
}
