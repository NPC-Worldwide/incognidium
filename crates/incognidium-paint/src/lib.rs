use ab_glyph::{point, Font, FontVec, PxScale, ScaleFont};
use incognidium_css::CssColor;
use incognidium_layout::{BoxType, FlatBox};
use incognidium_style::{
    ComputedStyle, Display, FontFamily, FontStyle, FontWeight, SizeValue, StyleMap, TextDecoration,
    TextDecorationLine, TextOverflow, TextTransform, Visibility, WhiteSpace,
};
use std::collections::HashMap;
use std::sync::OnceLock;
use tiny_skia::{Color, FillRule, Paint, Path, PathBuilder, Pixmap, Rect, Transform};

/// Build a tiny-skia Transform from CSS transform values
fn build_transform(
    transforms: &[incognidium_style::Transform],
    origin_x: f32,
    origin_y: f32,
) -> Transform {
    // Build the transform that will be applied to paths
    // For CSS transforms, we need to:
    // 1. Translate so the transform origin is at (0,0)
    // 2. Apply all transforms
    // 3. Translate back

    // First, create the "around origin" part: translate to origin, then translate back
    // We do this by starting with "translate back from origin", then each transform
    // is applied, then "translate to origin"
    // But since transforms are applied right-to-left when using post_*:
    // post_translate(origin) * transforms * post_translate(-origin)

    let mut transforms_matrix = Transform::identity();

    for t in transforms {
        transforms_matrix = match t {
            incognidium_style::Transform::Translate(x, y) => {
                transforms_matrix.post_translate(*x, *y)
            }
            incognidium_style::Transform::TranslateX(x) => {
                transforms_matrix.post_translate(*x, 0.0)
            }
            incognidium_style::Transform::TranslateY(y) => {
                transforms_matrix.post_translate(0.0, *y)
            }
            incognidium_style::Transform::Scale(x, y) => transforms_matrix.post_scale(*x, *y),
            incognidium_style::Transform::ScaleX(x) => transforms_matrix.post_scale(*x, 1.0),
            incognidium_style::Transform::ScaleY(y) => transforms_matrix.post_scale(1.0, *y),
            incognidium_style::Transform::Rotate(deg) => {
                // tiny-skia's post_rotate expects degrees
                transforms_matrix.post_rotate(*deg)
            }
            incognidium_style::Transform::SkewX(deg) => {
                let rad = deg.to_radians();
                let skew = Transform::from_skew(rad.tan(), 0.0);
                transforms_matrix.post_concat(skew)
            }
            incognidium_style::Transform::SkewY(deg) => {
                let rad = deg.to_radians();
                let skew = Transform::from_skew(0.0, rad.tan());
                transforms_matrix.post_concat(skew)
            }
            _ => transforms_matrix,
        };
    }

    // Now combine: translate to origin, apply transforms, translate back
    // Using post_* means the order is reversed:
    // final = post_translate(origin) * transforms * post_translate(-origin)
    // Which means: first translate -origin, then transforms, then translate +origin
    // Wait, that's wrong. Let me think again...
    // post_B(post_A(M)) = M * A * B
    // So post_translate(origin).post_concat(transforms).post_translate(-origin) would be:
    // M * translate(-origin) * transforms * translate(origin)
    // That's: translate by -origin, then transforms, then translate by origin
    // But we want: translate by origin, then transforms (around origin), then translate by -origin

    // Actually the right order using post_* is:
    // Start with translate(-origin) to bring origin to (0,0)
    // Then apply transforms
    // Then translate(origin) to bring back
    // post_translate(origin) * transforms * post_translate(-origin)
    // = M * translate(-origin) * transforms * translate(origin)
    // Which transforms the point by: translate(-origin), then transforms, then translate(origin)
    // That's wrong! We want translate(origin) to bring origin to 0,0, then transforms, then translate(-origin)

    // Let me just build it step by step with explicit order:
    // Final = translate(-origin) * transforms * translate(origin) * M
    // But that's wrong too.

    // Correct CSS transform order:
    // 1. Translate element so origin is at (0,0): T(-origin_x, -origin_y)
    // 2. Apply transforms
    // 3. Translate back: T(origin_x, origin_y)
    // Combined: T(origin) * transforms * T(-origin)

    // Using pre_* methods:
    // pre_B(pre_A(M)) = B * A * M
    // So pre_translate(origin).pre_concat(transforms).pre_translate(-origin) would give:
    // translate(origin) * transforms * translate(-origin) * M
    // Which is what we want!

    // But we already built transforms_matrix using post_*, which means:
    // transforms_matrix = M * transforms (identity * transforms = transforms)
    // So now we need: translate(origin) * transforms_matrix * translate(-origin)
    // Using pre_* for the outer operations:

    Transform::identity()
        .pre_translate(origin_x, origin_y)
        .pre_concat(transforms_matrix)
        .pre_translate(-origin_x, -origin_y)
}

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

    // Sort flat boxes by z-index for proper stacking order
    // Higher z-index values paint later (on top)
    let mut sorted_boxes: Vec<&FlatBox> = flat_boxes.iter().collect();
    sorted_boxes.sort_by(|a, b| {
        let z_a = styles.get(&a.node_id).map(|s| s.z_index).unwrap_or(0);
        let z_b = styles.get(&b.node_id).map(|s| s.z_index).unwrap_or(0);
        z_a.cmp(&z_b)
    });

    for fbox in sorted_boxes {
        let style = styles.get(&fbox.node_id).cloned().unwrap_or_default();

        if style.display == Display::None
            || style.visibility != Visibility::Visible
            || style.opacity == 0.0
        {
            continue;
        }

        // Calculate transform for this element
        let transform = if style.transform.is_empty() {
            Transform::identity()
        } else {
            // Use transform-origin from style (values are 0-1 percentages)
            let origin_x = fbox.x + fbox.width * style.transform_origin.0;
            let origin_y = fbox.y + fbox.height * style.transform_origin.1;
            build_transform(&style.transform, origin_x, origin_y)
        };

        // Transform clip bounds if this element has a transform
        // The clip comes from ancestors' overflow:hidden and is in their coordinate space
        // If this element has a transform, the clip needs to be transformed too
        let transformed_clip = transform_clip_bounds(fbox.clip, transform);

        // Apply opacity by modulating background/border alpha
        // NOTE: Opacity affects backgrounds and borders, but NOT text content
        // Text should remain fully opaque - only the container's box is transparent
        let opacity = style.opacity;
        let mut effective_style = style.clone();
        if opacity < 1.0 {
            effective_style.background_color.a =
                (effective_style.background_color.a as f32 * opacity) as u8;
            effective_style.border_color.a =
                (effective_style.border_color.a as f32 * opacity) as f32 as u8;
            // DO NOT apply opacity to text color - text should remain fully opaque
            // effective_style.color.a = (effective_style.color.a as f32 * opacity) as u8;
        }
        let style = effective_style;

        // Compute effective draw bounds after clipping
        let (draw_x, draw_y, draw_w, draw_h) = if let Some((cx, cy, cw, ch)) = transformed_clip {
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

        // Draw box shadow (outer shadows behind background)
        if let Some(ref shadows) = style.box_shadow {
            for shadow in shadows.iter().rev() {
                if !shadow.inset {
                    draw_box_shadow(
                        &mut pixmap,
                        fbox.x,
                        fbox.y,
                        fbox.width,
                        fbox.height,
                        shadow,
                        transform,
                    );
                }
            }
        }

        // Build clip path if clip-path is set
        let clip_path = build_clip_path(draw_x, draw_y, draw_w, draw_h, &style.clip_path);

        // Draw background (clipped) - check for gradient first, then solid color
        // Note: transforms on gradients need special handling
        let bg_drawn = match &style.background_image {
            incognidium_style::BackgroundImage::LinearGradient(grad) => {
                if let Some(ref cp) = clip_path {
                    draw_linear_gradient_clipped(&mut pixmap, draw_x, draw_y, draw_w, draw_h, grad, cp, transform,
                        style.border_top_left_radius.clone(),
                        style.border_top_right_radius.clone(),
                        style.border_bottom_right_radius.clone(),
                        style.border_bottom_left_radius.clone(),
                    );
                } else {
                    draw_linear_gradient(&mut pixmap, draw_x, draw_y, draw_w, draw_h, grad,
                        style.border_top_left_radius.clone(),
                        style.border_top_right_radius.clone(),
                        style.border_bottom_right_radius.clone(),
                        style.border_bottom_left_radius.clone(),
                        transform);
                }
                true
            }
            incognidium_style::BackgroundImage::RadialGradient(grad) => {
                if let Some(ref cp) = clip_path {
                    draw_radial_gradient_clipped(&mut pixmap, draw_x, draw_y, draw_w, draw_h, grad, cp, transform,
                        style.border_top_left_radius.clone(),
                        style.border_top_right_radius.clone(),
                        style.border_bottom_right_radius.clone(),
                        style.border_bottom_left_radius.clone(),
                    );
                } else {
                    draw_radial_gradient(&mut pixmap, draw_x, draw_y, draw_w, draw_h, grad,
                        style.border_top_left_radius.clone(),
                        style.border_top_right_radius.clone(),
                        style.border_bottom_right_radius.clone(),
                        style.border_bottom_left_radius.clone(),
                        transform);
                }
                true
            }
            _ => {
                // Fall back to solid background color (with border-radius)
                if style.background_color.a > 0 {
                    if let Some(ref cp) = clip_path {
                        draw_solid_rect_clipped(
                            &mut pixmap, draw_x, draw_y, draw_w, draw_h,
                            style.background_color, cp, transform,
                        );
                    } else {
                        draw_rounded_rect_with_transform(
                            &mut pixmap,
                            draw_x,
                            draw_y,
                            draw_w,
                            draw_h,
                            style.background_color,
                            style.border_top_left_radius.clone(),
                            style.border_top_right_radius.clone(),
                            style.border_bottom_right_radius.clone(),
                            style.border_bottom_left_radius.clone(),
                            transform,
                        );
                    }
                    true
                } else {
                    false
                }
            }
        };

        // Draw inset box shadows (on top of background, before borders)
        if let Some(ref shadows) = style.box_shadow {
            for shadow in shadows.iter().rev() {
                if shadow.inset {
                    draw_box_shadow(
                        &mut pixmap,
                        fbox.x,
                        fbox.y,
                        fbox.width,
                        fbox.height,
                        shadow,
                        transform,
                    );
                }
            }
        }

        // Apply CSS filters if any are set
        // Filters are applied to the entire element including background, borders, and content
        if !style.filter.is_empty() && (draw_w > 0.0 && draw_h > 0.0) {
            apply_filters_to_region(&mut pixmap, draw_x, draw_y, draw_w, draw_h, &style.filter);
        }

        // Draw border (always draw borders on the box itself - clip only affects children)
        if style.border_top_width > 0.0
            || style.border_right_width > 0.0
            || style.border_bottom_width > 0.0
            || style.border_left_width > 0.0
        {
            draw_borders_with_transform(&mut pixmap, fbox, &style, transform);
        }

        // Draw outline (focus indicator)
        if transformed_clip.is_none()
            && style.outline_width > 0.0
            && style.outline_style != incognidium_style::OutlineStyle::None
        {
            draw_outline(&mut pixmap, fbox, &style, transform);
        }

        // Draw checkbox/radio buttons
        if fbox.box_type == BoxType::InlineBlock {
            if let Some(input_type) = fbox.input_type {
                match input_type {
                    incognidium_layout::InputType::Checkbox { checked } => {
                        draw_checkbox(
                            &mut pixmap,
                            fbox.x,
                            fbox.y,
                            fbox.width,
                            fbox.height,
                            &style,
                            checked,
                        );
                    }
                    incognidium_layout::InputType::Radio { checked } => {
                        draw_radio(
                            &mut pixmap,
                            fbox.x,
                            fbox.y,
                            fbox.width,
                            fbox.height,
                            &style,
                            checked,
                        );
                    }
                    _ => {}
                }
            }
        }

        // Draw image (with clip bounds)
        if fbox.box_type == BoxType::Image {
            if let Some(ref src) = fbox.image_src {
                if let Some(img) = images.get(src) {
                    draw_image_with_transform_and_clip(
                        &mut pixmap,
                        fbox.x,
                        fbox.y,
                        fbox.width,
                        fbox.height,
                        img,
                        transformed_clip,
                        transform,
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
                    draw_text_with_transform(
                        &mut pixmap,
                        fbox.x,
                        fbox.y,
                        fbox.width,
                        fbox.height,
                        &display_text,
                        &style,
                        transformed_clip,
                        transform,
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
                    draw_text_with_transform(
                        &mut pixmap,
                        fbox.x + padding_left,
                        fbox.y + padding_top,
                        fbox.width - padding_left - style.padding_right,
                        fbox.height - padding_top - style.padding_bottom,
                        &display_text,
                        &style,
                        transformed_clip,
                        transform,
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
    radius_tl: SizeValue,
    radius_tr: SizeValue,
    radius_br: SizeValue,
    radius_bl: SizeValue,
    transform: Transform,
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
        .map(|stop| GradientStop::new(stop.position.unwrap_or(0.0), css_to_skia_color(stop.color)))
        .collect();

    if stops.len() < 2 {
        // Need at least 2 stops for a gradient
        return;
    }

    // Helper to resolve SizeValue to pixels
    let resolve_radius = |sv: &SizeValue| -> f32 {
        match sv {
            SizeValue::Percent(p) => {
                width.min(height) * p / 100.0
            }
            SizeValue::Px(px) => *px,
            _ => 0.0,
        }
    };

    // Clamp radii to half the smaller dimension
    let max_radius = (width.min(height) / 2.0).max(0.0);
    let rtl = resolve_radius(&radius_tl).min(max_radius);
    let rtr = resolve_radius(&radius_tr).min(max_radius);
    let rbr = resolve_radius(&radius_br).min(max_radius);
    let rbl = resolve_radius(&radius_bl).min(max_radius);

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

    // Build path - rounded rectangle if any radius is present, otherwise simple rect
    let path = if rtl > 0.0 || rtr > 0.0 || rbr > 0.0 || rbl > 0.0 {
        build_rounded_rect_path(x, y, width, height, rtl, rtr, rbr, rbl)
    } else {
        let rect = match Rect::from_xywh(x, y, width.max(1.0), height.max(1.0)) {
            Some(r) => r,
            None => return,
        };
        PathBuilder::from_rect(rect)
    };
    pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
}

/// Draw a radial gradient background
fn draw_radial_gradient(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    gradient: &incognidium_style::RadialGradient,
    radius_tl: SizeValue,
    radius_tr: SizeValue,
    radius_br: SizeValue,
    radius_bl: SizeValue,
    transform: Transform,
) {
    use tiny_skia::{GradientStop, Point, RadialGradient as SkiaRadialGradient, SpreadMode};

    if width <= 0.0 || height <= 0.0 {
        return;
    }

    // Calculate center point based on position percentage
    let cx = x + width * (gradient.position.0 / 100.0);
    let cy = y + height * (gradient.position.1 / 100.0);

    // Calculate radius based on size keyword
    // For simplicity, use a radius that covers the box
    let radius_x = width / 2.0;
    let radius_y = height / 2.0;
    let radius = radius_x.max(radius_y);

    // Convert color stops
    let stops: Vec<GradientStop> = gradient
        .stops
        .iter()
        .map(|stop| GradientStop::new(stop.position.unwrap_or(0.0), css_to_skia_color(stop.color)))
        .collect();

    if stops.len() < 2 {
        return;
    }

    // Helper to resolve SizeValue to pixels
    let resolve_radius = |sv: &SizeValue| -> f32 {
        match sv {
            SizeValue::Percent(p) => width.min(height) * p / 100.0,
            SizeValue::Px(px) => *px,
            _ => 0.0,
        }
    };

    // Clamp radii to half the smaller dimension
    let max_radius = (width.min(height) / 2.0).max(0.0);
    let rtl = resolve_radius(&radius_tl).min(max_radius);
    let rtr = resolve_radius(&radius_tr).min(max_radius);
    let rbr = resolve_radius(&radius_br).min(max_radius);
    let rbl = resolve_radius(&radius_bl).min(max_radius);

    // Create the radial gradient
    // For circle, use same radius for both; for ellipse, scale appropriately
    let skia_grad = match SkiaRadialGradient::new(
        Point::from_xy(cx, cy), // start center
        Point::from_xy(cx, cy), // end center (same as start for radial)
        radius,
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

    // Build path - rounded rectangle if any radius is present, otherwise simple rect
    let path = if rtl > 0.0 || rtr > 0.0 || rbr > 0.0 || rbl > 0.0 {
        build_rounded_rect_path(x, y, width, height, rtl, rtr, rbr, rbl)
    } else {
        let rect = match Rect::from_xywh(x, y, width.max(1.0), height.max(1.0)) {
            Some(r) => r,
            None => return,
        };
        PathBuilder::from_rect(rect)
    };
    pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
}

fn draw_rect(pixmap: &mut Pixmap, x: f32, y: f32, width: f32, height: f32, color: CssColor) {
    draw_rounded_rect(pixmap, x, y, width, height, color, SizeValue::Px(0.0), SizeValue::Px(0.0), SizeValue::Px(0.0), SizeValue::Px(0.0));
}

fn draw_rect_with_transform(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    color: CssColor,
    transform: Transform,
) {
    draw_rounded_rect_with_transform(
        pixmap, x, y, width, height, color,
        SizeValue::Px(0.0), SizeValue::Px(0.0),
        SizeValue::Px(0.0), SizeValue::Px(0.0),
        transform,
    );
}

/// Draw a filled circle.
fn draw_circle(pixmap: &mut Pixmap, cx: f32, cy: f32, radius: f32, color: CssColor) {
    if radius <= 0.0 {
        return;
    }

    let mut pb = PathBuilder::new();
    pb.push_circle(cx, cy, radius);

    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(color.r, color.g, color.b, color.a));
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// Draw a text decoration line (underline, strikethrough, overline) with style.
fn draw_text_decoration_line(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    thickness: f32,
    color: CssColor,
    style: incognidium_style::TextDecorationStyle,
) {
    use incognidium_style::TextDecorationStyle;

    match style {
        TextDecorationStyle::Solid => {
            draw_rect(pixmap, x, y, width, thickness, color);
        }
        TextDecorationStyle::Double => {
            // Two parallel lines
            draw_rect(pixmap, x, y, width, thickness, color);
            draw_rect(pixmap, x, y + thickness * 2.0, width, thickness, color);
        }
        TextDecorationStyle::Dotted => {
            // Dotted line - draw circles
            let dot_spacing = thickness * 4.0;
            let dot_radius = thickness * 0.8;
            let num_dots = (width / dot_spacing).ceil() as i32;
            for i in 0..=num_dots {
                let cx = x + i as f32 * dot_spacing + dot_radius;
                let cy = y + thickness / 2.0;
                if cx + dot_radius <= x + width {
                    draw_circle(pixmap, cx, cy, dot_radius, color);
                }
            }
        }
        TextDecorationStyle::Dashed => {
            // Dashed line
            let dash_len = thickness * 4.0;
            let gap_len = thickness * 2.0;
            let total = dash_len + gap_len;
            let num_dashes = (width / total).ceil() as i32;
            for i in 0..=num_dashes {
                let dx = x + i as f32 * total;
                if dx < x + width {
                    let dl = dash_len.min(x + width - dx);
                    draw_rect(pixmap, dx, y, dl, thickness, color);
                }
            }
        }
        TextDecorationStyle::Wavy => {
            // Wavy line using bezier curves
            let wave_height = thickness * 2.0;
            let wave_width = thickness * 6.0;
            let num_waves = (width / wave_width).ceil() as i32;

            let mut pb = PathBuilder::new();
            let base_y = y + thickness / 2.0;

            pb.move_to(x, base_y);

            for i in 0..num_waves {
                let wx = x + i as f32 * wave_width;
                let next_x = wx + wave_width;
                if next_x > x + width {
                    break;
                }

                // Create a smooth wave using cubic bezier
                // Control points for a smooth sine-like curve
                let cp1x = wx + wave_width * 0.25;
                let cp1y = base_y - wave_height;
                let cp2x = wx + wave_width * 0.75;
                let cp2y = base_y - wave_height;
                let end_x = next_x;
                let end_y = base_y;

                pb.cubic_to(cp1x, cp1y, cp2x, cp2y, end_x, end_y);

                // Second half of wave (down)
                let next_next_x = next_x + wave_width;
                if next_next_x > x + width {
                    break;
                }

                let cp3x = next_x + wave_width * 0.25;
                let cp3y = base_y + wave_height;
                let cp4x = next_x + wave_width * 0.75;
                let cp4y = base_y + wave_height;
                let end2_x = next_next_x;
                let end2_y = base_y;

                pb.cubic_to(cp3x, cp3y, cp4x, cp4y, end2_x, end2_y);
            }

            // Stroke the path
            if let Some(path) = pb.finish() {
                let mut paint = Paint::default();
                paint.set_color(Color::from_rgba8(color.r, color.g, color.b, color.a));
                paint.anti_alias = true;
                // Use stroke instead of fill for the line
                // Since tiny-skia doesn't have stroke, we'll fill a thick path
                // For now, approximate with a filled rect along the path
                pixmap.stroke_path(
                    &path,
                    &paint,
                    &tiny_skia::Stroke {
                        width: thickness,
                        line_cap: tiny_skia::LineCap::Round,
                        line_join: tiny_skia::LineJoin::Round,
                        dash: None,
                        miter_limit: 4.0,
                    },
                    Transform::identity(),
                    None,
                );
            }
        }
    }
}

/// Draw a rectangle with optional rounded corners (border-radius).
fn draw_rounded_rect(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    color: CssColor,
    radius_tl: SizeValue, // top-left
    radius_tr: SizeValue, // top-right
    radius_br: SizeValue, // bottom-right
    radius_bl: SizeValue, // bottom-left
) {
    draw_rounded_rect_with_transform(
        pixmap,
        x,
        y,
        width,
        height,
        color,
        radius_tl,
        radius_tr,
        radius_br,
        radius_bl,
        Transform::identity(),
    )
}

/// Draw a rounded rectangle with transform support.
fn draw_rounded_rect_with_transform(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    color: CssColor,
    radius_tl: SizeValue,
    radius_tr: SizeValue,
    radius_br: SizeValue,
    radius_bl: SizeValue,
    transform: Transform,
) {
    if width <= 0.0 || height <= 0.0 {
        return;
    }

    // Helper to resolve SizeValue to pixels
    let resolve_radius = |sv: &SizeValue| -> f32 {
        match sv {
            SizeValue::Percent(p) => width.min(height) * p / 100.0,
            SizeValue::Px(px) => *px,
            _ => 0.0,
        }
    };

    // Clamp radii to half the smaller dimension
    let max_radius = (width.min(height) / 2.0).max(0.0);
    let rtl = resolve_radius(&radius_tl).min(max_radius);
    let rtr = resolve_radius(&radius_tr).min(max_radius);
    let rbr = resolve_radius(&radius_br).min(max_radius);
    let rbl = resolve_radius(&radius_bl).min(max_radius);

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
        pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
        return;
    }

    let path = build_rounded_rect_path(x, y, width, height, rtl, rtr, rbr, rbl);
    let mut paint = Paint::default();
    paint.set_color(css_to_skia_color(color));
    paint.anti_alias = true;
    pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
}

/// Build a rounded rectangle path with the given corner radii.
fn build_rounded_rect_path(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    rtl: f32,
    rtr: f32,
    rbr: f32,
    rbl: f32,
) -> Path {
    // kappa is the distance from the endpoint to the control point for a quarter circle
    // kappa = (4/3) * tan(pi/8) ≈ 0.5522847498
    let kappa = 0.5522847498;

    let mut pb = PathBuilder::new();

    // Top edge: start after top-left radius
    pb.move_to(x + rtl, y);
    // Top edge to top-right radius start
    pb.line_to(x + width - rtr, y);
    // Top-right corner curve
    if rtr > 0.0 {
        let k = rtr * kappa;
        pb.cubic_to(
            x + width - rtr + k,
            y, // control point 1
            x + width,
            y + rtr - k, // control point 2
            x + width,
            y + rtr, // end point
        );
    }
    // Right edge
    pb.line_to(x + width, y + height - rbr);
    // Bottom-right corner
    if rbr > 0.0 {
        let k = rbr * kappa;
        pb.cubic_to(
            x + width,
            y + height - rbr + k, // control point 1
            x + width - rbr + k,
            y + height, // control point 2
            x + width - rbr,
            y + height, // end point
        );
    }
    // Bottom edge
    pb.line_to(x + rbl, y + height);
    // Bottom-left corner
    if rbl > 0.0 {
        let k = rbl * kappa;
        pb.cubic_to(
            x + rbl - k,
            y + height, // control point 1
            x,
            y + height - rbl + k, // control point 2
            x,
            y + height - rbl, // end point
        );
    }
    // Left edge
    pb.line_to(x, y + rtl);
    // Top-left corner
    if rtl > 0.0 {
        let k = rtl * kappa;
        pb.cubic_to(
            x,
            y + rtl - k, // control point 1
            x + rtl - k,
            y, // control point 2
            x + rtl,
            y, // end point
        );
    }
    pb.close();

    pb.finish().expect("Failed to build rounded rect path")
}

fn draw_borders(pixmap: &mut Pixmap, fbox: &FlatBox, style: &ComputedStyle) {
    draw_borders_with_transform(pixmap, fbox, style, Transform::identity());
}

fn draw_borders_with_transform(
    pixmap: &mut Pixmap,
    fbox: &FlatBox,
    style: &ComputedStyle,
    transform: Transform,
) {
    use incognidium_style::BorderStyle;

    // Get per-side colors, falling back to border_color if not set
    let top_color = style.border_top_color.unwrap_or(style.border_color);
    let right_color = style.border_right_color.unwrap_or(style.border_color);
    let bottom_color = style.border_bottom_color.unwrap_or(style.border_color);
    let left_color = style.border_left_color.unwrap_or(style.border_color);

    // Draw each border side with its specific style and color
    draw_border_side(
        pixmap,
        fbox.x,
        fbox.y,
        fbox.width,
        style.border_top_width,
        top_color,
        style.border_top_style,
        BorderSide::Top,
        fbox,
        transform,
    );

    draw_border_side(
        pixmap,
        fbox.x,
        fbox.y + fbox.height - style.border_bottom_width,
        fbox.width,
        style.border_bottom_width,
        bottom_color,
        style.border_bottom_style,
        BorderSide::Bottom,
        fbox,
        transform,
    );

    draw_border_side(
        pixmap,
        fbox.x,
        fbox.y,
        style.border_left_width,
        fbox.height,
        left_color,
        style.border_left_style,
        BorderSide::Left,
        fbox,
        transform,
    );

    draw_border_side(
        pixmap,
        fbox.x + fbox.width - style.border_right_width,
        fbox.y,
        style.border_right_width,
        fbox.height,
        right_color,
        style.border_right_style,
        BorderSide::Right,
        fbox,
        transform,
    );
}

#[derive(Clone, Copy)]
enum BorderSide {
    Top,
    Bottom,
    Left,
    Right,
}

fn draw_border_side(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    length: f32,
    width: f32,
    color: CssColor,
    style: incognidium_style::BorderStyle,
    side: BorderSide,
    fbox: &FlatBox,
    transform: Transform,
) {
    if width <= 0.0 || length <= 0.0 {
        return;
    }

    use incognidium_style::BorderStyle;

    match style {
        BorderStyle::None | BorderStyle::Hidden => {}
        BorderStyle::Solid => {
            draw_rect_with_transform(pixmap, x, y, length, width, color, transform);
        }
        BorderStyle::Dashed => {
            draw_dashed_border(pixmap, x, y, length, width, color, side, transform);
        }
        BorderStyle::Dotted => {
            draw_dotted_border(pixmap, x, y, length, width, color, side, transform);
        }
        BorderStyle::Double => {
            draw_double_border(pixmap, x, y, length, width, color, side, fbox, transform);
        }
        // For groove, ridge, inset, outset - just draw solid for now
        _ => {
            draw_rect_with_transform(pixmap, x, y, length, width, color, transform);
        }
    }
}

fn draw_dashed_border(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    length: f32,
    width: f32,
    color: CssColor,
    side: BorderSide,
    transform: Transform,
) {
    let dash_length = width * 3.0;
    let gap_length = width * 2.0;
    let total_pattern = dash_length + gap_length;
    let num_dashes = (length / total_pattern).ceil() as i32;

    for i in 0..num_dashes {
        let offset = i as f32 * total_pattern;
        if offset >= length {
            break;
        }

        let current_dash_length = (dash_length).min(length - offset);

        match side {
            BorderSide::Top | BorderSide::Bottom => {
                draw_rect_with_transform(
                    pixmap,
                    x + offset,
                    y,
                    current_dash_length,
                    width,
                    color,
                    transform,
                );
            }
            BorderSide::Left | BorderSide::Right => {
                draw_rect_with_transform(
                    pixmap,
                    x,
                    y + offset,
                    width,
                    current_dash_length,
                    color,
                    transform,
                );
            }
        }
    }
}

fn draw_dotted_border(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    length: f32,
    width: f32,
    color: CssColor,
    side: BorderSide,
    transform: Transform,
) {
    let dot_spacing = width * 2.0;
    let num_dots = (length / dot_spacing).ceil() as i32;

    for i in 0..num_dots {
        let offset = i as f32 * dot_spacing + dot_spacing / 2.0;
        if offset >= length {
            break;
        }

        match side {
            BorderSide::Top | BorderSide::Bottom => {
                draw_rect_with_transform(
                    pixmap,
                    x + offset - width / 2.0,
                    y,
                    width,
                    width,
                    color,
                    transform,
                );
            }
            BorderSide::Left | BorderSide::Right => {
                draw_rect_with_transform(
                    pixmap,
                    x,
                    y + offset - width / 2.0,
                    width,
                    width,
                    color,
                    transform,
                );
            }
        }
    }
}

fn draw_double_border(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    length: f32,
    width: f32,
    color: CssColor,
    side: BorderSide,
    fbox: &FlatBox,
    transform: Transform,
) {
    if width < 3.0 {
        // Too thin for double, just draw solid
        draw_rect_with_transform(pixmap, x, y, length, width, color, transform);
        return;
    }

    let inner_width = width / 3.0;
    let gap_width = width / 3.0;

    match side {
        BorderSide::Top => {
            // Outer line
            draw_rect_with_transform(pixmap, x, y, length, inner_width, color, transform);
            // Inner line
            draw_rect_with_transform(
                pixmap,
                x,
                y + inner_width + gap_width,
                length,
                inner_width,
                color,
                transform,
            );
        }
        BorderSide::Bottom => {
            // Inner line
            draw_rect_with_transform(
                pixmap,
                x,
                y + gap_width,
                length,
                inner_width,
                color,
                transform,
            );
            // Outer line
            draw_rect_with_transform(
                pixmap,
                x,
                y + inner_width + gap_width * 2.0,
                length,
                inner_width,
                color,
                transform,
            );
        }
        BorderSide::Left => {
            // Outer line
            draw_rect_with_transform(pixmap, x, y, inner_width, length, color, transform);
            // Inner line
            draw_rect_with_transform(
                pixmap,
                x + inner_width + gap_width,
                y,
                inner_width,
                length,
                color,
                transform,
            );
        }
        BorderSide::Right => {
            // Inner line
            draw_rect_with_transform(
                pixmap,
                x + gap_width,
                y,
                inner_width,
                length,
                color,
                transform,
            );
            // Outer line
            draw_rect_with_transform(
                pixmap,
                x + inner_width + gap_width * 2.0,
                y,
                inner_width,
                length,
                color,
                transform,
            );
        }
    }
}

/// Draw an outline (focus indicator) around an element.
/// Outline is drawn outside the border with an optional offset.
fn draw_outline(pixmap: &mut Pixmap, fbox: &FlatBox, style: &ComputedStyle, transform: Transform) {
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
    draw_rect_with_transform(
        pixmap,
        outline_x,
        outline_y,
        outline_w,
        outline_width,
        oc,
        transform,
    );
    // Bottom
    draw_rect_with_transform(
        pixmap,
        outline_x,
        outline_y + outline_h - outline_width,
        outline_w,
        outline_width,
        oc,
        transform,
    );
    // Left (between top and bottom)
    draw_rect_with_transform(
        pixmap,
        outline_x,
        outline_y + outline_width,
        outline_width,
        outline_h - outline_width * 2.0,
        oc,
        transform,
    );
    // Right (between top and bottom)
    draw_rect_with_transform(
        pixmap,
        outline_x + outline_w - outline_width,
        outline_y + outline_width,
        outline_width,
        outline_h - outline_width * 2.0,
        oc,
        transform,
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
    let border_color = CssColor {
        r: 100,
        g: 100,
        b: 100,
        a: 255,
    };
    draw_rect(pixmap, x, y, size, size, border_color);

    // Draw white background
    let bg_color = CssColor {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };
    draw_rect(pixmap, x + 1.0, y + 1.0, size - 2.0, size - 2.0, bg_color);

    // Draw checkmark if checked
    if checked {
        let check_color = CssColor {
            r: 50,
            g: 50,
            b: 50,
            a: 255,
        };
        // Simple checkmark: filled square (larger for visibility)
        let margin = size * 0.25;
        draw_rect(
            pixmap,
            x + margin,
            y + margin,
            size - margin * 2.0,
            size - margin * 2.0,
            check_color,
        );
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
    let cx = x + margin + size / 2.0;
    let cy = y + margin + size / 2.0;
    let radius = size / 2.0;

    // Draw border circle
    let border_color = CssColor {
        r: 100,
        g: 100,
        b: 100,
        a: 255,
    };
    draw_circle(pixmap, cx, cy, radius, border_color);

    // Draw white background (slightly smaller circle)
    let bg_color = CssColor {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };
    draw_circle(pixmap, cx, cy, radius - 1.0, bg_color);

    // Draw dot if checked
    if checked {
        let dot_color = CssColor {
            r: 50,
            g: 50,
            b: 50,
            a: 255,
        };
        let dot_radius = size * 0.25;
        draw_circle(pixmap, cx, cy, dot_radius, dot_color);
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
    transform: Transform,
) {
    use incognidium_style::BoxShadow;

    let shadow_color = shadow.color;
    let blur_radius = shadow.blur_radius;
    let spread_radius = shadow.spread_radius;

    // Create the box rect
    let box_rect = match Rect::from_xywh(x, y, width, height) {
        Some(r) => r,
        None => return,
    };

    if shadow.inset {
        // INSET SHADOW: shadow appears inside the box, fading inward from edges
        // The shadow is strongest at the box edge and fades toward the center
        // offset moves the shadow center, blur controls the fade distance

        let spread = spread_radius;

        // Calculate shadow dimensions with spread applied
        let shadow_w = (width - spread * 2.0).max(0.0);
        let shadow_h = (height - spread * 2.0).max(0.0);
        let shadow_x = x + spread + shadow.offset_x;
        let shadow_y = y + spread + shadow.offset_y;

        if shadow_w <= 0.0 || shadow_h <= 0.0 {
            return;
        }

        let shadow_rect = match Rect::from_xywh(shadow_x, shadow_y, shadow_w, shadow_h) {
            Some(r) => r,
            None => return,
        };

        if blur_radius <= 0.0 {
            // Solid inset shadow: draw a solid border inside the box
            let mut paint = Paint::default();
            paint.set_color(css_to_skia_color(shadow_color));
            paint.anti_alias = true;

            // Draw the area between box_rect and shadow_rect (the shadow ring)
            let mut pb = PathBuilder::new();
            pb.push_rect(box_rect);
            pb.push_rect(shadow_rect);
            if let Some(path) = pb.finish() {
                pixmap.fill_path(&path, &paint, FillRule::EvenOdd, transform, None);
            }
        } else {
            // Blurred inset shadow: draw fading shadow from edge inward
            let steps = (blur_radius * 1.5).max(5.0).min(30.0) as i32;

            for i in 0..=steps {
                let factor = i as f32 / steps as f32;
                // Gaussian-like falloff
                let alpha = (shadow_color.a as f32 * (-factor * factor * 3.0).exp()).max(0.0).min(255.0) as u8;

                if alpha == 0 {
                    continue;
                }

                let mut paint = Paint::default();
                let color = CssColor {
                    r: shadow_color.r,
                    g: shadow_color.g,
                    b: shadow_color.b,
                    a: alpha,
                };
                paint.set_color(css_to_skia_color(color));
                paint.anti_alias = true;

                // Contract the rect inward to create the fade effect
                let inset = blur_radius * factor;
                let inset_rect = match Rect::from_xywh(
                    shadow_rect.x() + inset,
                    shadow_rect.y() + inset,
                    (shadow_rect.width() - inset * 2.0).max(0.0),
                    (shadow_rect.height() - inset * 2.0).max(0.0),
                ) {
                    Some(r) if r.width() > 0.0 && r.height() > 0.0 => r,
                    _ => continue,
                };

                // Draw the ring between box_rect and inset_rect
                let mut pb = PathBuilder::new();
                pb.push_rect(box_rect);
                pb.push_rect(inset_rect);
                if let Some(path) = pb.finish() {
                    pixmap.fill_path(&path, &paint, FillRule::EvenOdd, transform, None);
                }
            }
        }
    } else {
        // OUTER SHADOW: shadow appears outside the box
        // Calculate shadow position with offset
        let shadow_x = x + shadow.offset_x;
        let shadow_y = y + shadow.offset_y;

        // Calculate shadow size based on spread
        let shadow_width = width + spread_radius * 2.0;
        let shadow_height = height + spread_radius * 2.0;

        if shadow_width <= 0.0 || shadow_height <= 0.0 {
            return;
        }

        // Create shadow rect
        let rect = match Rect::from_xywh(
            shadow_x - spread_radius,
            shadow_y - spread_radius,
            shadow_width.max(1.0),
            shadow_height.max(1.0),
        ) {
            Some(r) => r,
            None => return,
        };

        // Check if this is a centered glow (no offset)
        let is_centered_glow = shadow.offset_x == 0.0 && shadow.offset_y == 0.0;

        if blur_radius <= 0.0 {
            // No blur: draw solid shadow rect
            let mut paint = Paint::default();
            paint.set_color(css_to_skia_color(shadow_color));
            paint.anti_alias = true;

            // Just draw the shadow rect directly - the box will be drawn on top
            let path = PathBuilder::from_rect(rect);
            pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
        } else {
            // With blur: draw expanding shadow layers with Gaussian-like alpha distribution
            // Use more steps for smoother blur
            let steps = (blur_radius * 3.0).max(12.0).min(60.0) as i32;

            for i in 0..=steps {
                let t = i as f32 / steps as f32; // 0.0 to 1.0
                // t=0 is outer edge (fully expanded), t=1 is inner (shadow source)
                let expand = blur_radius * (1.0 - t);

                // Smooth falloff from outer to inner
                // Use power curve for better visual falloff
                // t=0 (outer) should have visible alpha, t=1 (inner) should have full alpha
                let falloff = (1.0 - t).powf(2.0); // Quadratic falloff from outer
                let min_alpha_ratio = 0.4; // Minimum 40% of shadow alpha at outer edge
                let alpha_factor = min_alpha_ratio + (1.0 - min_alpha_ratio) * (1.0 - falloff);
                let alpha = (shadow_color.a as f32 * alpha_factor).max(0.0).min(255.0) as u8;

                if alpha < 2 {
                    continue;
                }

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

                if is_centered_glow {
                    // For centered glow: draw full expanded rect
                    let path = PathBuilder::from_rect(expanded_rect);
                    pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
                } else {
                    // For offset shadow: punch hole for box area
                    // Only punch hole if box_rect is inside expanded_rect
                    let box_inside = box_rect.x() >= expanded_rect.x()
                        && box_rect.y() >= expanded_rect.y()
                        && box_rect.x() + box_rect.width() <= expanded_rect.x() + expanded_rect.width()
                        && box_rect.y() + box_rect.height() <= expanded_rect.y() + expanded_rect.height();

                    if box_inside {
                        let mut pb = PathBuilder::new();
                        pb.push_rect(expanded_rect);
                        pb.push_rect(box_rect);
                        if let Some(path) = pb.finish() {
                            pixmap.fill_path(&path, &paint, FillRule::EvenOdd, transform, None);
                        }
                    } else {
                        // Box extends outside expanded rect, just draw full shadow
                        let path = PathBuilder::from_rect(expanded_rect);
                        pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
                    }
                }
            }
        }
    }
}

/// Draw an image scaled to fit the given box.
#[allow(dead_code)]
fn draw_image(pixmap: &mut Pixmap, x: f32, y: f32, box_w: f32, box_h: f32, img: &ImageData) {
    draw_image_with_transform(pixmap, x, y, box_w, box_h, img, Transform::identity());
}

/// Draw an image with transform support.
/// Uses backward mapping: for each destination pixel, find the source pixel
/// by applying the inverse transform.
fn draw_image_with_transform(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    box_w: f32,
    box_h: f32,
    img: &ImageData,
    transform: Transform,
) {
    if img.width == 0 || img.height == 0 || box_w <= 0.0 || box_h <= 0.0 {
        return;
    }

    let pm_w = pixmap.width();
    let pm_h = pixmap.height();
    let px_data = pixmap.data_mut();

    // Compute inverse transform to map destination pixels back to source
    let inverse = transform.invert().unwrap_or(Transform::identity());

    // Calculate the transformed bounds to determine which destination pixels to iterate
    // Transform the four corners of the destination box
    let corners = [
        transform_xy(&transform, x, y),
        transform_xy(&transform, x + box_w, y),
        transform_xy(&transform, x, y + box_h),
        transform_xy(&transform, x + box_w, y + box_h),
    ];

    let min_x = corners
        .iter()
        .map(|(px, _)| *px)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as u32;
    let max_x = corners
        .iter()
        .map(|(px, _)| *px)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(pm_w as f32) as u32;
    let min_y = corners
        .iter()
        .map(|(_, py)| *py)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as u32;
    let max_y = corners
        .iter()
        .map(|(_, py)| *py)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(pm_h as f32) as u32;

    let sx_ratio = img.width as f32 / box_w;
    let sy_ratio = img.height as f32 / box_h;
    let iw = img.width as i32;
    let ih = img.height as i32;

    for py in min_y..max_y {
        for px in min_x..max_x {
            // Map destination pixel back to source space using inverse transform
            let (src_x, src_y) = transform_xy(&inverse, px as f32, py as f32);

            // Check if the source position is within the original box
            if src_x < x || src_x >= x + box_w || src_y < y || src_y >= y + box_h {
                continue;
            }

            // Map to image coordinates
            let fx = (src_x - x + 0.5) * sx_ratio - 0.5;
            let fy = (src_y - y + 0.5) * sy_ratio - 0.5;

            // Bilinear sampling
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

            let c00 = sample(x0, y0);
            let c10 = sample(x1, y0);
            let c01 = sample(x0, y1);
            let c11 = sample(x1, y1);

            let bilinear = |c: [[u32; 4]; 4]| -> u8 {
                let v00 = c[0][0] as f32;
                let v10 = c[1][0] as f32;
                let v01 = c[2][0] as f32;
                let v11 = c[3][0] as f32;
                let val = v00 * (1.0 - tx) * (1.0 - ty)
                    + v10 * tx * (1.0 - ty)
                    + v01 * (1.0 - tx) * ty
                    + v11 * tx * ty;
                val.clamp(0.0, 255.0) as u8
            };

            let r = bilinear([c00, c10, c01, c11]);
            let g = bilinear([[c00[1]; 4], [c10[1]; 4], [c01[1]; 4], [c11[1]; 4]]);
            let b = bilinear([[c00[2]; 4], [c10[2]; 4], [c01[2]; 4], [c11[2]; 4]]);
            let a = bilinear([[c00[3]; 4], [c10[3]; 4], [c01[3]; 4], [c11[3]; 4]]);

            if a > 0 {
                let dst_idx = ((py * pm_w + px) * 4) as usize;
                if a == 255 {
                    px_data[dst_idx] = r;
                    px_data[dst_idx + 1] = g;
                    px_data[dst_idx + 2] = b;
                    px_data[dst_idx + 3] = 255;
                } else {
                    let inv_a = (255 - a) as u32;
                    px_data[dst_idx] =
                        ((r as u32 * a as u32 + px_data[dst_idx] as u32 * inv_a) / 255) as u8;
                    px_data[dst_idx + 1] =
                        ((g as u32 * a as u32 + px_data[dst_idx + 1] as u32 * inv_a) / 255) as u8;
                    px_data[dst_idx + 2] =
                        ((b as u32 * a as u32 + px_data[dst_idx + 2] as u32 * inv_a) / 255) as u8;
                    px_data[dst_idx + 3] = 255;
                }
            }
        }
    }
}

/// Helper to transform a point using a tiny-skia Transform
fn transform_xy(t: &Transform, x: f32, y: f32) -> (f32, f32) {
    // Transform matrix is:
    // | sx  ky  tx |
    // | kx  sy  ty |
    // | 0   0   1  |
    // where: x' = sx*x + kx*y + tx
    //        y' = ky*x + sy*y + ty
    let x_new = t.sx * x + t.kx * y + t.tx;
    let y_new = t.ky * x + t.sy * y + t.ty;
    (x_new, y_new)
}

/// Transform clip bounds (x, y, width, height) by applying the transform to all four corners
/// and returning the bounding box of the transformed corners.
fn transform_clip_bounds(
    clip: Option<(f32, f32, f32, f32)>,
    transform: Transform,
) -> Option<(f32, f32, f32, f32)> {
    if clip.is_none() || transform == Transform::identity() {
        return clip;
    }
    let (cx, cy, cw, ch) = clip.unwrap();
    // Transform all four corners
    let corners = [
        transform_xy(&transform, cx, cy),
        transform_xy(&transform, cx + cw, cy),
        transform_xy(&transform, cx, cy + ch),
        transform_xy(&transform, cx + cw, cy + ch),
    ];
    // Compute bounding box
    let min_x = corners
        .iter()
        .map(|(x, _)| *x)
        .fold(f32::INFINITY, f32::min);
    let max_x = corners
        .iter()
        .map(|(x, _)| *x)
        .fold(f32::NEG_INFINITY, f32::max);
    let min_y = corners
        .iter()
        .map(|(_, y)| *y)
        .fold(f32::INFINITY, f32::min);
    let max_y = corners
        .iter()
        .map(|(_, y)| *y)
        .fold(f32::NEG_INFINITY, f32::max);
    Some((min_x, min_y, max_x - min_x, max_y - min_y))
}

/// Apply CSS filters to a region of the pixmap.
/// This is a simplified implementation that processes pixels in-place.
fn apply_filters_to_region(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    filters: &[incognidium_style::Filter],
) {
    // Convert region to integer bounds
    let x0 = x.max(0.0) as u32;
    let y0 = y.max(0.0) as u32;
    let x1 = ((x + width).min(pixmap.width() as f32)) as u32;
    let y1 = ((y + height).min(pixmap.height() as f32)) as u32;

    if x0 >= x1 || y0 >= y1 {
        return;
    }

    let pm_width = pixmap.width();
    let pm_height = pixmap.height();
    let data = pixmap.data_mut();

    // Build filter parameters
    let mut brightness: f32 = 1.0;
    let mut contrast: f32 = 1.0;
    let mut grayscale: f32 = 0.0;
    let mut sepia: f32 = 0.0;
    let mut hue_rotate: f32 = 0.0;
    let mut invert: f32 = 0.0;
    let mut saturate: f32 = 1.0;
    let mut opacity: f32 = 1.0;

    for filter in filters {
        match filter {
            incognidium_style::Filter::Brightness(v) => brightness *= v,
            incognidium_style::Filter::Contrast(v) => contrast *= v,
            incognidium_style::Filter::Grayscale(v) => grayscale = grayscale.max(*v),
            incognidium_style::Filter::Sepia(v) => sepia = sepia.max(*v),
            incognidium_style::Filter::HueRotate(v) => hue_rotate += v,
            incognidium_style::Filter::Invert(v) => invert = invert.max(*v),
            incognidium_style::Filter::Saturate(v) => saturate *= v,
            incognidium_style::Filter::Opacity(v) => opacity *= v,
            // Blur and drop-shadow require more complex processing - skip for now
            _ => {}
        }
    }

    // Convert hue rotate to radians
    let hue_rad = hue_rotate.to_radians();

    // Apply filters to each pixel in the region
    for py in y0..y1 {
        for px in x0..x1 {
            let idx = ((py * pm_width + px) * 4) as usize;
            if idx + 3 >= data.len() {
                continue;
            }

            let r = data[idx] as f32 / 255.0;
            let g = data[idx + 1] as f32 / 255.0;
            let b = data[idx + 2] as f32 / 255.0;
            let a = data[idx + 3] as f32 / 255.0;

            // Skip fully transparent pixels
            if a == 0.0 {
                continue;
            }

            let (mut r, mut g, mut b) = (r, g, b);

            // Apply grayscale
            if grayscale > 0.0 {
                let gray = r * 0.299 + g * 0.587 + b * 0.114;
                r = r * (1.0 - grayscale) + gray * grayscale;
                g = g * (1.0 - grayscale) + gray * grayscale;
                b = b * (1.0 - grayscale) + gray * grayscale;
            }

            // Apply sepia
            if sepia > 0.0 {
                let sepia_r = (r * 0.393 + g * 0.769 + b * 0.189).min(1.0);
                let sepia_g = (r * 0.349 + g * 0.686 + b * 0.168).min(1.0);
                let sepia_b = (r * 0.272 + g * 0.534 + b * 0.131).min(1.0);
                r = r * (1.0 - sepia) + sepia_r * sepia;
                g = g * (1.0 - sepia) + sepia_g * sepia;
                b = b * (1.0 - sepia) + sepia_b * sepia;
            }

            // Apply hue rotation
            if hue_rad.abs() > 0.001 {
                let (h, s, v) = rgb_to_hsv(r, g, b);
                let h_new = (h + hue_rad / (2.0 * std::f32::consts::PI)) % 1.0;
                let (r_new, g_new, b_new) = hsv_to_rgb(h_new, s, v);
                r = r_new;
                g = g_new;
                b = b_new;
            }

            // Apply saturation
            if saturate != 1.0 {
                let gray = r * 0.299 + g * 0.587 + b * 0.114;
                r = (gray + (r - gray) * saturate).clamp(0.0, 1.0);
                g = (gray + (g - gray) * saturate).clamp(0.0, 1.0);
                b = (gray + (b - gray) * saturate).clamp(0.0, 1.0);
            }

            // Apply brightness
            if brightness != 1.0 {
                r = (r * brightness).clamp(0.0, 1.0);
                g = (g * brightness).clamp(0.0, 1.0);
                b = (b * brightness).clamp(0.0, 1.0);
            }

            // Apply contrast
            if contrast != 1.0 {
                let factor = contrast;
                r = ((r - 0.5) * factor + 0.5).clamp(0.0, 1.0);
                g = ((g - 0.5) * factor + 0.5).clamp(0.0, 1.0);
                b = ((b - 0.5) * factor + 0.5).clamp(0.0, 1.0);
            }

            // Apply invert
            if invert > 0.0 {
                r = (r * (1.0 - invert) + (1.0 - r) * invert).clamp(0.0, 1.0);
                g = (g * (1.0 - invert) + (1.0 - g) * invert).clamp(0.0, 1.0);
                b = (b * (1.0 - invert) + (1.0 - b) * invert).clamp(0.0, 1.0);
            }

            // Apply opacity
            let a_new = (a * opacity).clamp(0.0, 1.0);

            // Write back
            data[idx] = (r * 255.0) as u8;
            data[idx + 1] = (g * 255.0) as u8;
            data[idx + 2] = (b * 255.0) as u8;
            data[idx + 3] = (a_new * 255.0) as u8;
        }
    }
}

/// Convert RGB to HSV color space
fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let diff = max - min;

    let h = if diff == 0.0 {
        0.0
    } else if max == r {
        (60.0 * ((g - b) / diff) + 360.0) % 360.0
    } else if max == g {
        (60.0 * ((b - r) / diff) + 120.0)
    } else {
        (60.0 * ((r - g) / diff) + 240.0)
    };

    let s = if max == 0.0 { 0.0 } else { diff / max };
    let v = max;

    (h / 360.0, s, v) // Normalize hue to 0-1
}

/// Convert HSV to RGB color space
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let h_deg = h * 360.0;
    let c = v * s;
    let x = c * (1.0 - ((h_deg / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;

    let (r1, g1, b1) = if h_deg < 60.0 {
        (c, x, 0.0)
    } else if h_deg < 120.0 {
        (x, c, 0.0)
    } else if h_deg < 180.0 {
        (0.0, c, x)
    } else if h_deg < 240.0 {
        (0.0, x, c)
    } else if h_deg < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };

    (r1 + m, g1 + m, b1 + m)
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
    let mut ellipsis_to_render: Option<(f32, f32, f32)> = None; // (x, y, width)

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
        let nowrap = matches!(style.white_space, WhiteSpace::NoWrap | WhiteSpace::Pre);

        // Split line into words while preserving space counts (for text-align: justify)
        let (words, space_counts): (Vec<&str>, Vec<usize>) = if nowrap {
            (vec![line], vec![0])
        } else {
            // Split on spaces, but count how many spaces between each word
            // "word1   word2   word3".split(' ') -> ["word1", "", "", "word2", ...]
            let parts: Vec<&str> = line.split(' ').collect();
            let mut words = Vec::new();
            let mut space_counts = Vec::new();
            let mut space_buffer = 0;

            for part in parts {
                if part.is_empty() {
                    // Empty string means we hit a space delimiter
                    space_buffer += 1;
                } else {
                    // This is a word - the space buffer contains preceding spaces
                    // Minimum 1 space between words (the delimiter that preceded this word)
                    words.push(part);
                    space_counts.push(space_buffer.max(1));
                    space_buffer = 0;
                }
            }
            // Trailing spaces are ignored (no word after them)
            (words, space_counts)
        };
        let line_start_x = cursor_x;

        for (wi, word) in words.iter().enumerate() {
            let word_width: f32 = word
                .chars()
                .map(|c| scaled.h_advance(scaled.glyph_id(c)) + letter_spacing)
                .sum::<f32>()
                - if word.chars().count() > 0 {
                    letter_spacing
                } else {
                    0.0
                };

            // NOTE: We intentionally do NOT re-wrap text here. Layout phase already
            // determined line breaks by inserting \n characters. Paint should trust
            // layout's wrapping decisions and not re-wrap based on max_width.
            // max_width/max_height are only for overflow clipping, not soft wrapping.

            // Render each glyph
            let mut prev_glyph = None;
            let mut should_stop = false;
            for ch in word.chars() {
                let glyph_id = scaled.glyph_id(ch);

                // Kerning
                if let Some(prev) = prev_glyph {
                    cursor_x += scaled.kern(prev, glyph_id);
                }

                // Check if this glyph would exceed max_width (for overflow:hidden)
                let glyph_width = scaled.h_advance(glyph_id);
                // For text-overflow: ellipsis, calculate ellipsis width for later use
                let ellipsis_width: f32 = if style.text_overflow == TextOverflow::Ellipsis {
                    ['.', '.', '.']
                        .iter()
                        .map(|c| scaled.h_advance(scaled.glyph_id(*c)))
                        .sum()
                } else {
                    0.0
                };
                // Check if this glyph alone would overflow (don't add ellipsis_width here,
                // that would cause premature truncation of text that would otherwise fit)
                let would_overflow = cursor_x + glyph_width > x + max_width + 0.5;
                // Check if we should stop rendering due to overflow
                if max_width > 0.0 && would_overflow {
                    if style.text_overflow == TextOverflow::Ellipsis {
                        // Always render ellipsis at the overflow point
                        // This may overwrite the last 1-2 characters, which is acceptable
                        ellipsis_to_render = Some((cursor_x, cursor_y, ellipsis_width));
                        should_stop = true;
                        break;
                    }
                    // For normal text (not ellipsis), continue rendering and let
                    // pixel-level clipping handle overflow. Layout phase should have
                    // already wrapped text to fit, but small calculation differences
                    // between layout and paint shouldn't truncate visible text.
                }

                // Text shadow (render first, behind text)
                if let Some(shadow) = style.text_shadow {
                    let shadow_x = cursor_x + shadow.offset_x;
                    let shadow_y = cursor_y + ascent + shadow.offset_y;
                    let shadow_glyph =
                        glyph_id.with_scale_and_position(scale, point(shadow_x, shadow_y));
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
                                    blend_pixel(
                                        pixmap,
                                        px,
                                        py,
                                        shadow_color.r,
                                        shadow_color.g,
                                        shadow_color.b,
                                        alpha,
                                    );
                                }
                            }
                        });
                    }
                }

                // Use fractional positioning for smoother text (Chrome-style)
                let glyph =
                    glyph_id.with_scale_and_position(scale, point(cursor_x, cursor_y + ascent));
                if let Some(outlined) = font.outline_glyph(glyph) {
                    let bounds = outlined.px_bounds();
                    outlined.draw(|gx, gy, coverage| {
                        let px = gx as i32 + bounds.min.x as i32;
                        let py = gy as i32 + bounds.min.y as i32;
                        if px >= 0 && py >= 0 {
                            let px = px as u32;
                            let py = py as u32;
                            // Check pixmap bounds - overflow clipping is handled by clip_path
                            // Do NOT clip horizontally here, as text-overflow: ellipsis needs
                            // to see the text to determine where to truncate
                            let in_pixmap = px < pixmap.width() && py < pixmap.height();
                            // Only check vertical bounds (for max_height constraint)
                            let in_vertical_bounds = max_height <= 0.0
                                || py <= (y + max_height) as u32;
                            if in_pixmap && in_vertical_bounds {
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
                // Use the space count to add appropriate spacing (supports text-align: justify)
                let num_spaces = if nowrap { 1 } else { space_counts.get(wi).copied().unwrap_or(1) };
                cursor_x += space_width * num_spaces as f32;
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
            let decor_color = style.text_decoration_color.unwrap_or(color);
            let decor_style = style.text_decoration_style;

            if has_underline {
                let ul_y = y + ascent + 2.0;
                draw_text_decoration_line(
                    pixmap, decor_x, ul_y, decor_w, line_thickness, decor_color, decor_style,
                );
            }

            if has_line_through {
                // Middle of the text (approximate with x-height)
                let lt_y = y + ascent * 0.35;
                draw_text_decoration_line(
                    pixmap, decor_x, lt_y, decor_w, line_thickness, decor_color, decor_style,
                );
            }

            if has_overline {
                // Above the text
                let ol_y = y + ascent - font_size * 0.85;
                draw_text_decoration_line(
                    pixmap, decor_x, ol_y, decor_w, line_thickness, decor_color, decor_style,
                );
            }
        }
    }

    // Render ellipsis if text was truncated
    if let Some((ex, ey, _)) = ellipsis_to_render {
        let ellipsis_y = ey;
        let mut ellipsis_x = ex;
        let mut prev_egid = None;
        for ech in ['.', '.', '.'] {
            let egid = scaled.glyph_id(ech);
            if let Some(prev) = prev_egid {
                ellipsis_x += scaled.kern(prev, egid);
            }
            let eglyph =
                egid.with_scale_and_position(scale, point(ellipsis_x, ellipsis_y + ascent));
            if let Some(outlined) = font.outline_glyph(eglyph) {
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
            ellipsis_x += scaled.h_advance(egid);
            prev_egid = Some(egid);
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
        let nowrap = matches!(style.white_space, WhiteSpace::NoWrap | WhiteSpace::Pre);

        let words: Vec<&str> = if nowrap {
            vec![line]
        } else {
            line.split_whitespace().collect()
        };
        let line_start_x = cursor_x;

        for (wi, word) in words.iter().enumerate() {
            let word_width = word.len() as f32 * char_width;

            // NOTE: We intentionally do NOT re-wrap text here. Layout phase already
            // determined line breaks by inserting \n characters. Paint should trust
            // layout's wrapping decisions and not re-wrap based on max_width.
            // max_width/max_height are only for overflow clipping, not soft wrapping.

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
        let decor_color = style.text_decoration_color.unwrap_or(color);
        let decor_style = style.text_decoration_style;

        if has_underline {
            let underline_y = y + font_size;
            draw_text_decoration_line(
                pixmap, decor_x, underline_y, text_width.min(max_width),
                line_thickness, decor_color, decor_style,
            );
        }

        if has_line_through {
            let lt_y = y + font_size * 0.55;
            draw_text_decoration_line(
                pixmap, decor_x, lt_y, text_width.min(max_width),
                line_thickness, decor_color, decor_style,
            );
        }

        if has_overline {
            let overline_y = y + font_size * 0.15;
            draw_text_decoration_line(
                pixmap, decor_x, overline_y, text_width.min(max_width),
                line_thickness, decor_color, decor_style,
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
    draw_image_with_transform_and_clip(
        pixmap,
        x,
        y,
        box_w,
        box_h,
        img,
        clip,
        Transform::identity(),
    );
}

fn draw_image_with_transform_and_clip(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    box_w: f32,
    box_h: f32,
    img: &ImageData,
    clip: Option<(f32, f32, f32, f32)>,
    transform: Transform,
) {
    if img.width == 0 || img.height == 0 || box_w <= 0.0 || box_h <= 0.0 {
        return;
    }
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

    // Compute inverse transform to map destination pixels back to source
    let inverse = transform.invert().unwrap_or(Transform::identity());

    // Calculate transformed bounds
    let corners = [
        transform_xy(&transform, x, y),
        transform_xy(&transform, x + box_w, y),
        transform_xy(&transform, x, y + box_h),
        transform_xy(&transform, x + box_w, y + box_h),
    ];

    let min_x = corners
        .iter()
        .map(|(px, _)| *px)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(clip_x1 as f32)
        .max(0.0) as u32;
    let max_x = corners
        .iter()
        .map(|(px, _)| *px)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(clip_x2 as f32)
        .min(pm_w as f32) as u32;
    let min_y = corners
        .iter()
        .map(|(_, py)| *py)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(clip_y1 as f32)
        .max(0.0) as u32;
    let max_y = corners
        .iter()
        .map(|(_, py)| *py)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(clip_y2 as f32)
        .min(pm_h as f32) as u32;

    for py in min_y..max_y {
        for px in min_x..max_x {
            // Map destination pixel back to source space using inverse transform
            let (src_x, src_y) = transform_xy(&inverse, px as f32, py as f32);

            // Check if the source position is within the original box
            if src_x < x || src_x >= x + box_w || src_y < y || src_y >= y + box_h {
                continue;
            }

            // Map to image coordinates
            let fx = (src_x - x + 0.5) * sx_ratio - 0.5;
            let fy = (src_y - y + 0.5) * sy_ratio - 0.5;

            // Bilinear sampling
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
                let inv_a: u32 = 255 - sa;
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
    draw_text_with_transform(
        pixmap,
        x,
        y,
        max_width,
        max_height,
        text,
        style,
        clip,
        Transform::identity(),
    );
}

/// Draw text with transform and optional clipping.
/// Renders text to a temporary buffer, then applies the transform.
#[allow(clippy::too_many_arguments)]
fn draw_text_with_transform(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    max_width: f32,
    max_height: f32,
    text: &str,
    style: &ComputedStyle,
    clip: Option<(f32, f32, f32, f32)>,
    transform: Transform,
) {
    // If no transform (or identity), use direct rendering
    if transform == Transform::identity() {
        if let Some((cx, cy, cw, ch)) = clip {
            // When there's a clip (from overflow:hidden on parent), constrain rendering to clip
            // For text-overflow: ellipsis to work, we need to use the clip width as max_width
            // because the text may have natural width > container width with white-space: nowrap
            // But keep the original text position (x, y) - the clip is just for bounds checking
            let eff_w = (x + max_width).min(cx + cw) - x.max(cx);
            let eff_h = (y + max_height).min(cy + ch) - y.max(cy);
            if eff_w > 0.0 && eff_h > 0.0 {
                draw_text(pixmap, x, y, max_width, max_height, text, style);
            }
        } else {
            draw_text(pixmap, x, y, max_width, max_height, text, style);
        }
        return;
    }

    // Calculate the bounds of the text in screen space
    let pm_w = pixmap.width();
    let pm_h = pixmap.height();

    // Transform the four corners of the text area to find screen bounds
    let corners = [
        transform_xy(&transform, x, y),
        transform_xy(&transform, x + max_width, y),
        transform_xy(&transform, x, y + max_height),
        transform_xy(&transform, x + max_width, y + max_height),
    ];

    let mut min_x = corners
        .iter()
        .map(|(px, _)| *px)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as u32;
    let mut max_x = corners
        .iter()
        .map(|(px, _)| *px)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(pm_w as f32) as u32;
    let mut min_y = corners
        .iter()
        .map(|(_, py)| *py)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as u32;
    let mut max_y = corners
        .iter()
        .map(|(_, py)| *py)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(pm_h as f32) as u32;

    // Apply clip bounds if present
    if let Some((cx, cy, cw, ch)) = clip {
        let clip_x1 = cx.max(0.0) as u32;
        let clip_y1 = cy.max(0.0) as u32;
        let clip_x2 = (cx + cw).min(pm_w as f32) as u32;
        let clip_y2 = (cy + ch).min(pm_h as f32) as u32;
        min_x = min_x.max(clip_x1);
        min_y = min_y.max(clip_y1);
        max_x = max_x.min(clip_x2);
        max_y = max_y.min(clip_y2);
    }

    // Compute inverse transform
    let inverse = transform.invert().unwrap_or(Transform::identity());

    // Get clipping bounds
    let (clip_x1, clip_y1, clip_x2, clip_y2) = if let Some((cx, cy, cw, ch)) = clip {
        (
            cx.max(0.0) as u32,
            cy.max(0.0) as u32,
            (cx + cw).min(pm_w as f32) as u32,
            (cy + ch).min(pm_h as f32) as u32,
        )
    } else {
        (0, 0, pm_w, pm_h)
    };

    // For text, we use a different approach than images:
    // We need to render each pixel by finding what text would be there
    // This is done by transforming the destination pixel back to source,
    // then checking if that source position contains text

    let px_data = pixmap.data_mut();

    for py in min_y..max_y {
        for px in min_x..max_x {
            // Check clipping
            if px < clip_x1 || py < clip_y1 || px >= clip_x2 || py >= clip_y2 {
                continue;
            }

            // Map destination pixel back to source space
            let (src_x, src_y) = transform_xy(&inverse, px as f32, py as f32);

            // Check if within text bounds
            if src_x < x || src_x >= x + max_width || src_y < y || src_y >= y + max_height {
                continue;
            }

            // Sample the text at this source position
            // We render a small region around this point to the temp buffer
            let sample_x = (src_x - x).max(0.0);
            let sample_y = (src_y - y).max(0.0);

            // For performance, we'll render a small tile of text and sample from it
            // This is an approximation but works for simple cases
            // A full implementation would track glyph positions and transform each

            // For now, use a simpler approach: check if this pixel would have text
            // by looking at the glyph that would be rendered here
            let color = sample_text_at_position(x, y, sample_x, sample_y, text, style);

            if color.a > 0 {
                let dst_idx = ((py * pm_w + px) * 4) as usize;
                if color.a == 255 {
                    px_data[dst_idx] = color.r;
                    px_data[dst_idx + 1] = color.g;
                    px_data[dst_idx + 2] = color.b;
                    px_data[dst_idx + 3] = 255;
                } else {
                    let inv_a = (255 - color.a) as u32;
                    px_data[dst_idx] = ((color.r as u32 * color.a as u32
                        + px_data[dst_idx] as u32 * inv_a)
                        / 255) as u8;
                    px_data[dst_idx + 1] = ((color.g as u32 * color.a as u32
                        + px_data[dst_idx + 1] as u32 * inv_a)
                        / 255) as u8;
                    px_data[dst_idx + 2] = ((color.b as u32 * color.a as u32
                        + px_data[dst_idx + 2] as u32 * inv_a)
                        / 255) as u8;
                    px_data[dst_idx + 3] = 255;
                }
            }
        }
    }
}

/// Sample the text color at a specific position within the text area
fn sample_text_at_position(
    text_x: f32,
    text_y: f32,
    local_x: f32,
    local_y: f32,
    text: &str,
    style: &ComputedStyle,
) -> CssColor {
    use ab_glyph::{Font, PxScale, ScaleFont};

    let fonts = get_fonts();
    if fonts.is_none() {
        // Bitmap fallback - simplified
        return CssColor::TRANSPARENT;
    }
    let fonts = fonts.unwrap();

    let font_size = style.font_size;
    let bold = style.font_weight == FontWeight::Bold;
    let italic = style.font_style == FontStyle::Italic;
    let font = pick_font(&fonts, bold, italic);
    let scale = PxScale::from(font_size);
    let scaled = font.as_scaled(scale);
    let line_height = font_size * style.line_height;
    let color = style.color;

    let ascent = scaled.ascent();
    let space_width = scaled.h_advance(scaled.glyph_id(' ')) + style.word_spacing;
    let letter_spacing = style.letter_spacing;

    let mut cursor_x = text_x;
    let mut cursor_y = text_y;

    if text.starts_with(' ') {
        cursor_x += space_width;
    }

    // Split on newlines
    let lines: Vec<&str> = text.split('\n').collect();

    for (li, line) in lines.iter().enumerate() {
        if li > 0 {
            cursor_x = text_x;
            cursor_y += line_height;
        }

        if line.is_empty() {
            continue;
        }

        // Check if we're on this line
        if local_y >= cursor_y - text_y && local_y < cursor_y - text_y + line_height {
            let words: Vec<&str> = line.split_whitespace().collect();
            let line_start_x = cursor_x;

            for word in words.iter() {
                let word_width: f32 = word
                    .chars()
                    .map(|c| scaled.h_advance(scaled.glyph_id(c)) + letter_spacing)
                    .sum::<f32>()
                    - if word.chars().count() > 0 {
                        letter_spacing
                    } else {
                        0.0
                    };

                // Check each character in the word
                let mut char_x = cursor_x;
                let mut prev_glyph = None;

                for ch in word.chars() {
                    let glyph_id = scaled.glyph_id(ch);

                    if let Some(prev) = prev_glyph {
                        char_x += scaled.kern(prev, glyph_id);
                    }

                    let glyph_width = scaled.h_advance(glyph_id);

                    // Check if the local position is within this glyph
                    if local_x >= char_x - text_x && local_x < char_x - text_x + glyph_width {
                        // Check vertical position within glyph
                        let glyph_y = cursor_y + ascent;
                        if local_y >= glyph_y - text_y - font_size && local_y < glyph_y - text_y {
                            // This is within the glyph's bounding box
                            // Sample the actual glyph coverage
                            let glyph =
                                glyph_id.with_scale_and_position(scale, point(char_x, glyph_y));
                            if let Some(outlined) = font.outline_glyph(glyph) {
                                let bounds = outlined.px_bounds();
                                let rel_x = local_x - (bounds.min.x - text_x);
                                let rel_y = local_y - (bounds.min.y - text_y);

                                if rel_x >= 0.0 && rel_y >= 0.0 {
                                    let gx = rel_x as u32;
                                    let gy = rel_y as u32;

                                    // Check coverage at this pixel
                                    let mut coverage = 0.0f32;
                                    outlined.draw(|cx, cy, c| {
                                        if cx == gx && cy == gy {
                                            coverage = c;
                                        }
                                    });

                                    if coverage > 0.0 {
                                        let alpha = (coverage * color.a as f32) as u8;
                                        return CssColor {
                                            r: color.r,
                                            g: color.g,
                                            b: color.b,
                                            a: alpha,
                                        };
                                    }
                                }
                            }
                            return CssColor::TRANSPARENT;
                        }
                    }

                    char_x += glyph_width + letter_spacing;
                    prev_glyph = Some(glyph_id);
                }

                cursor_x = char_x;
                if !word.is_empty() {
                    cursor_x -= letter_spacing;
                }
                cursor_x += space_width;
            }
        }
    }

    CssColor::TRANSPARENT
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

/// Build a clip path from the CSS clip-path property
fn build_clip_path(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    clip: &Option<incognidium_style::ClipPath>,
) -> Option<Path> {
    match clip {
        None => None,
        Some(incognidium_style::ClipPath::None) => None,
        Some(incognidium_style::ClipPath::Circle(radius)) => {
            let cx = x + width / 2.0;
            let cy = y + height / 2.0;
            let r = radius.min(width.min(height) / 2.0);
            Some(build_circle_path(cx, cy, r))
        }
        Some(incognidium_style::ClipPath::Ellipse(rx, ry)) => {
            let cx = x + width / 2.0;
            let cy = y + height / 2.0;
            let rx = rx.min(width / 2.0);
            let ry = ry.min(height / 2.0);
            Some(build_ellipse_path(cx, cy, rx, ry))
        }
        Some(incognidium_style::ClipPath::Polygon(points)) => {
            Some(build_polygon_path(x, y, width, height, points))
        }
        Some(incognidium_style::ClipPath::Inset(t, r, b, l)) => {
            let inset_x = x + l;
            let inset_y = y + t;
            let inset_w = width - l - r;
            let inset_h = height - t - b;
            if inset_w > 0.0 && inset_h > 0.0 {
                let rect = Rect::from_xywh(inset_x, inset_y, inset_w, inset_h)?;
                Some(PathBuilder::from_rect(rect))
            } else {
                None
            }
        }
    }
}

/// Build a circle path
fn build_circle_path(cx: f32, cy: f32, r: f32) -> Path {
    let mut pb = PathBuilder::new();
    // Use kappa for cubic bezier approximation of circle
    let kappa = 0.5522847498 * r;

    pb.move_to(cx, cy - r);
    // Top-right quadrant
    pb.cubic_to(cx + kappa, cy - r, cx + r, cy - kappa, cx + r, cy);
    // Bottom-right quadrant
    pb.cubic_to(cx + r, cy + kappa, cx + kappa, cy + r, cx, cy + r);
    // Bottom-left quadrant
    pb.cubic_to(cx - kappa, cy + r, cx - r, cy + kappa, cx - r, cy);
    // Top-left quadrant
    pb.cubic_to(cx - r, cy - kappa, cx - kappa, cy - r, cx, cy - r);
    pb.close();
    pb.finish().unwrap_or_else(|| PathBuilder::new().finish().unwrap())
}

/// Build an ellipse path
fn build_ellipse_path(cx: f32, cy: f32, rx: f32, ry: f32) -> Path {
    let mut pb = PathBuilder::new();
    let kappa_x = 0.5522847498 * rx;
    let kappa_y = 0.5522847498 * ry;

    pb.move_to(cx, cy - ry);
    pb.cubic_to(cx + kappa_x, cy - ry, cx + rx, cy - kappa_y, cx + rx, cy);
    pb.cubic_to(cx + rx, cy + kappa_y, cx + kappa_x, cy + ry, cx, cy + ry);
    pb.cubic_to(cx - kappa_x, cy + ry, cx - rx, cy + kappa_y, cx - rx, cy);
    pb.cubic_to(cx - rx, cy - kappa_y, cx - kappa_x, cy - ry, cx, cy - ry);
    pb.close();
    pb.finish().unwrap_or_else(|| PathBuilder::new().finish().unwrap())
}

/// Build a polygon path from percentage coordinates
fn build_polygon_path(x: f32, y: f32, width: f32, height: f32, points: &[(f32, f32)]) -> Path {
    let mut pb = PathBuilder::new();
    if points.is_empty() {
        return pb.finish().unwrap_or_else(|| PathBuilder::new().finish().unwrap());
    }

    // First point
    let first_x = x + (points[0].0 / 100.0) * width;
    let first_y = y + (points[0].1 / 100.0) * height;
    pb.move_to(first_x, first_y);

    // Remaining points
    for point in &points[1..] {
        let px = x + (point.0 / 100.0) * width;
        let py = y + (point.1 / 100.0) * height;
        pb.line_to(px, py);
    }

    pb.close();
    pb.finish().unwrap_or_else(|| PathBuilder::new().finish().unwrap())
}

/// Draw a solid rect with clip path
fn draw_solid_rect_clipped(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    color: CssColor,
    clip_path: &Path,
    transform: Transform,
) {
    let mut paint = Paint::default();
    paint.set_color(css_to_skia_color(color));
    paint.anti_alias = true;
    pixmap.fill_path(clip_path, &paint, FillRule::Winding, transform, None);
}

/// Draw linear gradient with clip path
fn draw_linear_gradient_clipped(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    grad: &incognidium_style::LinearGradient,
    clip_path: &Path,
    transform: Transform,
    radius_tl: SizeValue,
    radius_tr: SizeValue,
    radius_br: SizeValue,
    radius_bl: SizeValue,
) {
    use incognidium_style::GradientDirection;
    use tiny_skia::{GradientStop, LinearGradient as SkiaLinearGradient, Point, SpreadMode};

    if width <= 0.0 || height <= 0.0 {
        return;
    }

    // Calculate gradient line based on direction (same as draw_linear_gradient)
    let (x1, y1, x2, y2) = match grad.direction {
        GradientDirection::ToTop => (x + width / 2.0, y + height, x + width / 2.0, y),
        GradientDirection::ToBottom => (x + width / 2.0, y, x + width / 2.0, y + height),
        GradientDirection::ToLeft => (x + width, y + height / 2.0, x, y + height / 2.0),
        GradientDirection::ToRight => (x, y + height / 2.0, x + width, y + height / 2.0),
        GradientDirection::ToTopLeft => (x + width, y + height, x, y),
        GradientDirection::ToTopRight => (x, y + height, x + width, y),
        GradientDirection::ToBottomLeft => (x + width, y, x, y + height),
        GradientDirection::ToBottomRight => (x, y, x + width, y + height),
        GradientDirection::Angle(deg) => {
            let rad = deg.to_radians();
            let cx = x + width / 2.0;
            let cy = y + height / 2.0;
            let half_diag = (width * width + height * height).sqrt() / 2.0;
            let dx = rad.sin() * half_diag;
            let dy = -rad.cos() * half_diag;
            (cx - dx, cy - dy, cx + dx, cy + dy)
        }
    };

    let stops: Vec<GradientStop> = grad
        .stops
        .iter()
        .map(|stop| GradientStop::new(stop.position.unwrap_or(0.0), css_to_skia_color(stop.color)))
        .collect();

    if stops.len() < 2 {
        return;
    }

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

    // Helper to resolve SizeValue to pixels
    let resolve_radius = |sv: &SizeValue| -> f32 {
        match sv {
            SizeValue::Percent(p) => width.min(height) * p / 100.0,
            SizeValue::Px(px) => *px,
            _ => 0.0,
        }
    };

    // Clamp radii to half the smaller dimension
    let max_radius = (width.min(height) / 2.0).max(0.0);
    let rtl = resolve_radius(&radius_tl).min(max_radius);
    let rtr = resolve_radius(&radius_tr).min(max_radius);
    let rbr = resolve_radius(&radius_br).min(max_radius);
    let rbl = resolve_radius(&radius_bl).min(max_radius);

    let mut paint = Paint::default();
    paint.shader = skia_grad;
    paint.anti_alias = true;

    // Use rounded rect path if border-radius is present
    let path = if rtl > 0.0 || rtr > 0.0 || rbr > 0.0 || rbl > 0.0 {
        build_rounded_rect_path(x, y, width, height, rtl, rtr, rbr, rbl)
    } else {
        // For clipped version without border-radius, we still need to respect the clip_path
        // For now, just use the clip_path (this maintains existing behavior)
        // TODO: Properly combine clip_path with the shape
        pixmap.fill_path(clip_path, &paint, FillRule::Winding, transform, None);
        return;
    };
    pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
}

/// Draw radial gradient with clip path
fn draw_radial_gradient_clipped(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    grad: &incognidium_style::RadialGradient,
    clip_path: &Path,
    transform: Transform,
    radius_tl: SizeValue,
    radius_tr: SizeValue,
    radius_br: SizeValue,
    radius_bl: SizeValue,
) {
    use tiny_skia::{GradientStop, Point, RadialGradient as SkiaRadialGradient, SpreadMode};

    let center = Point::from_xy(
        x + grad.position.0 / 100.0 * width,
        y + grad.position.1 / 100.0 * height,
    );
    let radius = width.min(height) / 2.0;

    let stops: Vec<GradientStop> = grad
        .stops
        .iter()
        .map(|stop| GradientStop::new(stop.position.unwrap_or(0.0), css_to_skia_color(stop.color)))
        .collect();

    if stops.len() < 2 {
        return;
    }

    let skia_grad = match SkiaRadialGradient::new(
        center,
        center,
        radius,
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        Some(g) => g,
        None => return,
    };

    // Helper to resolve SizeValue to pixels
    let resolve_radius = |sv: &SizeValue| -> f32 {
        match sv {
            SizeValue::Percent(p) => width.min(height) * p / 100.0,
            SizeValue::Px(px) => *px,
            _ => 0.0,
        }
    };

    // Clamp radii to half the smaller dimension
    let max_radius = (width.min(height) / 2.0).max(0.0);
    let rtl = resolve_radius(&radius_tl).min(max_radius);
    let rtr = resolve_radius(&radius_tr).min(max_radius);
    let rbr = resolve_radius(&radius_br).min(max_radius);
    let rbl = resolve_radius(&radius_bl).min(max_radius);

    let mut paint = Paint::default();
    paint.shader = skia_grad;
    paint.anti_alias = true;

    // Use rounded rect path if border-radius is present
    let path = if rtl > 0.0 || rtr > 0.0 || rbr > 0.0 || rbl > 0.0 {
        build_rounded_rect_path(x, y, width, height, rtl, rtr, rbr, rbl)
    } else {
        pixmap.fill_path(clip_path, &paint, FillRule::Winding, transform, None);
        return;
    };
    pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
}
