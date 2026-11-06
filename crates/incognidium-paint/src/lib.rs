use fontdue::Font as FontdueFont;
use incognidium_css::CssColor;
use incognidium_layout::{measure_text_width, BoxType, FlatBox};
use incognidium_style::{
    char_needs_emoji_fallback, ColumnRuleStyle, ComputedStyle, Display, Float, FontFamily,
    FontStyle, FontWeight, ImageRendering, LengthValue, Position, PrintColorAdjust, SizeValue,
    StyleMap, TextCombineUpright, TextDecoration, TextDecorationLine, TextEmphasisPosition,
    TextEmphasisStyle, TextOverflow, TextTransform, TextUnderlinePosition, Visibility, WhiteSpace,
    WritingMode,
};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use tiny_skia::{BlendMode, Color, FillRule, Paint, Path, PathBuilder, Pixmap, Rect, Transform};

// ── TTF Font Loading ──────────────────────────────────────────

struct LoadedFonts {
    regular: FontdueFont,
    bold: FontdueFont,
    italic: FontdueFont,
    bold_italic: FontdueFont,
    serif_regular: Option<FontdueFont>,
    serif_bold: Option<FontdueFont>,
    serif_italic: Option<FontdueFont>,
    serif_bold_italic: Option<FontdueFont>,
    mono_regular: Option<FontdueFont>,
    mono_bold: Option<FontdueFont>,
    mono_italic: Option<FontdueFont>,
    mono_bold_italic: Option<FontdueFont>,
}

static FONTS: OnceLock<Option<LoadedFonts>> = OnceLock::new();

fn load_script_fallback_fonts(bold: bool) -> Vec<FontdueFont> {
    let weight = if bold { "Bold" } else { "Regular" };
    let names = [
        "NotoSans",
        "NotoNaskhArabic",
        "NotoSansArabic",
        "NotoSansHebrew",
        "NotoSerifHebrew",
        "NotoSansDevanagari",
        "NotoSansBengali",
        "NotoSansTamil",
        "NotoSansThai",
        "NotoSansEthiopic",
        "NotoSansSymbols",
    ];
    let mut fonts = Vec::new();
    for name in names {
        let path = format!("/usr/share/fonts/truetype/noto/{}-{}.ttf", name, weight);
        if let Ok(data) = std::fs::read(&path) {
            if let Ok(font) = FontdueFont::from_bytes(data, fontdue::FontSettings::default()) {
                log::info!("Loaded script fallback font from {path}");
                fonts.push(font);
            }
        }
    }
    // Symbols2 only ships Regular; load it for both weights so geometric shapes
    // and other symbol characters do not render as placeholder boxes.
    if let Ok(data) = std::fs::read("/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf") {
        if let Ok(font) = FontdueFont::from_bytes(data, fontdue::FontSettings::default()) {
            log::info!("Loaded script fallback font from /usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf");
            fonts.push(font);
        }
    }
    fonts
}

fn load_emoji_font() -> Option<(FontdueFont, ttf_parser::Face<'static>)> {
    let paths = [
        "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
        "/usr/share/fonts/opentype/noto/NotoColorEmoji.ttf",
        "/usr/share/fonts/truetype/joypixels/JoyPixels.ttf",
        "/usr/share/fonts/truetype/emoji-one/EmojiOneColor.otf",
    ];
    paths.iter().find_map(|path| {
        let data = std::fs::read(path).ok()?;
        let font = FontdueFont::from_bytes(data.clone(), fontdue::FontSettings::default()).ok()?;
        let leaked: &'static [u8] = Box::leak(data.into_boxed_slice());
        let face = ttf_parser::Face::parse(leaked, 0).ok()?;
        Some((font, face))
    })
}

fn load_cjk_fonts() -> (Option<FontdueFont>, Option<FontdueFont>) {
    let paths = [
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSerifCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    ];
    let mut regular = None;
    for path in &paths {
        if let Ok(data) = std::fs::read(path) {
            if let Ok(font) = FontdueFont::from_bytes(data, fontdue::FontSettings::default()) {
                log::info!("Loaded CJK fallback font from {path}");
                regular = Some(font);
                break;
            }
        }
    }
    let bold = regular.as_ref().and_then(|_| {
        let bold_paths = [
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc",
            "/usr/share/fonts/opentype/noto/NotoSerifCJK-Bold.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Bold.ttc",
        ];
        for path in &bold_paths {
            if let Ok(data) = std::fs::read(path) {
                if let Ok(font) = FontdueFont::from_bytes(data, fontdue::FontSettings::default()) {
                    log::info!("Loaded CJK bold fallback font from {path}");
                    return Some(font);
                }
            }
        }
        None
    });
    (regular, bold)
}

fn try_load_family(
    dir: &str,
    reg: &str,
    bld: &str,
    ita: &str,
    bi: &str,
) -> Option<(FontdueFont, FontdueFont, FontdueFont, FontdueFont)> {
    let regular = FontdueFont::from_bytes(
        std::fs::read(format!("{dir}/{reg}")).ok()?,
        fontdue::FontSettings::default(),
    )
    .ok()?;
    let bold = FontdueFont::from_bytes(
        std::fs::read(format!("{dir}/{bld}")).ok()?,
        fontdue::FontSettings::default(),
    )
    .ok()?;
    let italic = FontdueFont::from_bytes(
        std::fs::read(format!("{dir}/{ita}")).ok()?,
        fontdue::FontSettings::default(),
    )
    .ok()?;
    let bold_italic = FontdueFont::from_bytes(
        std::fs::read(format!("{dir}/{bi}")).ok()?,
        fontdue::FontSettings::default(),
    )
    .ok()?;
    Some((regular, bold, italic, bold_italic))
}

fn load_fonts() -> Option<LoadedFonts> {
    let try_embedded = || -> Option<LoadedFonts> {
        let regular = FontdueFont::from_bytes(
            include_bytes!("../../../assets/fonts/Roboto-Regular.ttf").to_vec(),
            fontdue::FontSettings::default(),
        )
        .ok()?;
        let bold = FontdueFont::from_bytes(
            include_bytes!("../../../assets/fonts/Roboto-Bold.ttf").to_vec(),
            fontdue::FontSettings::default(),
        )
        .ok()?;
        let italic = FontdueFont::from_bytes(
            include_bytes!("../../../assets/fonts/Roboto-Italic.ttf").to_vec(),
            fontdue::FontSettings::default(),
        )
        .ok()?;
        let bold_italic = FontdueFont::from_bytes(
            include_bytes!("../../../assets/fonts/Roboto-BoldItalic.ttf").to_vec(),
            fontdue::FontSettings::default(),
        )
        .ok()?;
        Some(LoadedFonts {
            regular,
            bold,
            italic,
            bold_italic,
            serif_regular: None,
            serif_bold: None,
            serif_italic: None,
            serif_bold_italic: None,
            mono_regular: None,
            mono_bold: None,
            mono_italic: None,
            mono_bold_italic: None,
        })
    };

    // 1) Prefer installed system fonts so generic font-family stacks share
    // metrics with the host browser (Firefox/Chrome use the system defaults).
    // This fixes sites that specify web-safe stacks such as
    // `Verdana, Geneva, sans-serif` and expect the sans fallback to match the
    // OS default, not the bundled Roboto face.
    let sans_dirs = [
        // DejaVu is the default sans-serif on many Linux distributions and is
        // the face Firefox commonly falls back to for generic `sans-serif`, so
        // try it first. Liberation is a close second for systems without DejaVu.
        "/usr/share/fonts/truetype/dejavu",
        "/usr/share/fonts/truetype/liberation2",
        "/usr/share/fonts/truetype/liberation",
        "/usr/share/fonts/liberation-sans",
    ];
    let sans_families = [
        (
            "DejaVuSans.ttf",
            "DejaVuSans-Bold.ttf",
            "DejaVuSans-Oblique.ttf",
            "DejaVuSans-BoldOblique.ttf",
        ),
        (
            "LiberationSans-Regular.ttf",
            "LiberationSans-Bold.ttf",
            "LiberationSans-Italic.ttf",
            "LiberationSans-BoldItalic.ttf",
        ),
    ];

    let serif_dirs = [
        "/usr/share/fonts/truetype/liberation2",
        "/usr/share/fonts/truetype/liberation",
        "/usr/share/fonts/truetype/dejavu",
    ];
    let serif_families = [
        (
            "LiberationSerif-Regular.ttf",
            "LiberationSerif-Bold.ttf",
            "LiberationSerif-Italic.ttf",
            "LiberationSerif-BoldItalic.ttf",
        ),
        (
            "DejaVuSerif.ttf",
            "DejaVuSerif-Bold.ttf",
            "DejaVuSerif-Italic.ttf",
            "DejaVuSerif-BoldItalic.ttf",
        ),
    ];

    let mono_dirs = [
        "/usr/share/fonts/truetype/liberation2",
        "/usr/share/fonts/truetype/liberation",
        "/usr/share/fonts/truetype/dejavu",
    ];
    let mono_families = [
        (
            "LiberationMono-Regular.ttf",
            "LiberationMono-Bold.ttf",
            "LiberationMono-Italic.ttf",
            "LiberationMono-BoldItalic.ttf",
        ),
        (
            "DejaVuSansMono.ttf",
            "DejaVuSansMono-Bold.ttf",
            "DejaVuSansMono-Oblique.ttf",
            "DejaVuSansMono-BoldOblique.ttf",
        ),
    ];

    for dir in &sans_dirs {
        for (reg, bld, ita, bi) in &sans_families {
            let try_load = || -> Option<LoadedFonts> {
                let (regular, bold, italic, bold_italic) = try_load_family(dir, reg, bld, ita, bi)?;

                let mut serif_regular = None;
                let mut serif_bold = None;
                let mut serif_italic = None;
                let mut serif_bold_italic = None;
                for sdir in &serif_dirs {
                    for (sreg, sbld, sita, sbi) in &serif_families {
                        if let Some((sr, sb, si, sbi_f)) =
                            try_load_family(sdir, sreg, sbld, sita, sbi)
                        {
                            serif_regular = Some(sr);
                            serif_bold = Some(sb);
                            serif_italic = Some(si);
                            serif_bold_italic = Some(sbi_f);
                            break;
                        }
                    }
                    if serif_regular.is_some() {
                        break;
                    }
                }

                let mut mono_regular = None;
                let mut mono_bold = None;
                let mut mono_italic = None;
                let mut mono_bold_italic = None;
                for mdir in &mono_dirs {
                    for (mreg, mbld, mita, mbi) in &mono_families {
                        if let Some((mr, mb, mi, mbi_f)) =
                            try_load_family(mdir, mreg, mbld, mita, mbi)
                        {
                            mono_regular = Some(mr);
                            mono_bold = Some(mb);
                            mono_italic = Some(mi);
                            mono_bold_italic = Some(mbi_f);
                            break;
                        }
                    }
                    if mono_regular.is_some() {
                        break;
                    }
                }

                Some(LoadedFonts {
                    regular,
                    bold,
                    italic,
                    bold_italic,
                    serif_regular,
                    serif_bold,
                    serif_italic,
                    serif_bold_italic,
                    mono_regular,
                    mono_bold,
                    mono_italic,
                    mono_bold_italic,
                })
            };
            if let Some(fonts) = try_load() {
                log::info!("Loaded TTF fonts from {dir}");
                return Some(fonts);
            }
        }
    }

    // 2) Fall back to embedded Roboto when no system fonts are available.
    if let Some(fonts) = try_embedded() {
        return Some(fonts);
    }
    None
}

fn get_fonts() -> Option<&'static LoadedFonts> {
    FONTS.get_or_init(load_fonts).as_ref()
}

fn named_fonts() -> &'static HashMap<String, LoadedFonts> {
    static NAMED: OnceLock<HashMap<String, LoadedFonts>> = OnceLock::new();
    NAMED.get_or_init(load_named_fonts)
}

/// Load named system font families (e.g. Helvetica, Arial) so that text is
/// painted with the same face layout measured it with.
fn load_named_fonts() -> HashMap<String, LoadedFonts> {
    let mut map = HashMap::new();
    for name in [
        "Helvetica",
        "Arial",
        "Verdana",
        "Geneva",
        "Tahoma",
        "Trebuchet MS",
        "Times New Roman",
        "Georgia",
        "Courier New",
        "Courier",
        "Impact",
        "Roboto",
    ] {
        let Some(paths) = incognidium_style::system_font_paths(name) else {
            continue;
        };
        let regular_data = match std::fs::read(&paths.regular) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let regular =
            match FontdueFont::from_bytes(regular_data.clone(), fontdue::FontSettings::default()) {
                Ok(f) => f,
                Err(_) => continue,
            };
        let load_variant = |path: &str| {
            std::fs::read(path)
                .ok()
                .and_then(|d| FontdueFont::from_bytes(d, fontdue::FontSettings::default()).ok())
                .unwrap_or_else(|| {
                    FontdueFont::from_bytes(regular_data.clone(), fontdue::FontSettings::default())
                        .expect("regular face already parsed")
                })
        };
        let bold = load_variant(&paths.bold);
        let italic = load_variant(&paths.italic);
        let bold_italic = load_variant(&paths.bold_italic);
        map.insert(
            name.to_lowercase(),
            LoadedFonts {
                regular,
                bold,
                italic,
                bold_italic,
                serif_regular: None,
                serif_bold: None,
                serif_italic: None,
                serif_bold_italic: None,
                mono_regular: None,
                mono_bold: None,
                mono_italic: None,
                mono_bold_italic: None,
            },
        );
    }
    map
}

fn cjk_fonts() -> &'static (Option<FontdueFont>, Option<FontdueFont>) {
    static CJK: OnceLock<(Option<FontdueFont>, Option<FontdueFont>)> = OnceLock::new();
    CJK.get_or_init(load_cjk_fonts)
}

fn emoji_fonts() -> &'static Option<(FontdueFont, ttf_parser::Face<'static>)> {
    static EMOJI: OnceLock<Option<(FontdueFont, ttf_parser::Face<'static>)>> = OnceLock::new();
    EMOJI.get_or_init(load_emoji_font)
}

fn script_fallback_fonts(bold: bool) -> &'static Vec<FontdueFont> {
    static REGULAR: OnceLock<Vec<FontdueFont>> = OnceLock::new();
    static BOLD: OnceLock<Vec<FontdueFont>> = OnceLock::new();
    if bold {
        BOLD.get_or_init(|| load_script_fallback_fonts(true))
    } else {
        REGULAR.get_or_init(|| load_script_fallback_fonts(false))
    }
}

fn is_emoji_fallback_font(font: &FontdueFont) -> bool {
    emoji_fonts()
        .as_ref()
        .map_or(false, |(f, _)| std::ptr::eq(f, font))
}

fn pick_font(
    style: &ComputedStyle,
    bold: bool,
    italic: bool,
    family: FontFamily,
) -> &'static FontdueFont {
    let named = named_fonts();
    if let Some(named_fonts) = style
        .system_font_families
        .iter()
        .map(|name| name.to_lowercase())
        .find_map(|name| named.get(&name))
    {
        return match (bold, italic) {
            (true, true) => &named_fonts.bold_italic,
            (true, false) => &named_fonts.bold,
            (false, true) => &named_fonts.italic,
            (false, false) => &named_fonts.regular,
        };
    }
    let fonts = get_fonts().expect("fonts must be loaded before painting");
    match family {
        FontFamily::Serif => match (bold, italic) {
            (true, true) => fonts
                .serif_bold_italic
                .as_ref()
                .unwrap_or(&fonts.bold_italic),
            (true, false) => fonts.serif_bold.as_ref().unwrap_or(&fonts.bold),
            (false, true) => fonts.serif_italic.as_ref().unwrap_or(&fonts.italic),
            (false, false) => fonts.serif_regular.as_ref().unwrap_or(&fonts.regular),
        },
        FontFamily::Monospace => match (bold, italic) {
            (true, true) => fonts
                .mono_bold_italic
                .as_ref()
                .unwrap_or(&fonts.bold_italic),
            (true, false) => fonts.mono_bold.as_ref().unwrap_or(&fonts.bold),
            (false, true) => fonts.mono_italic.as_ref().unwrap_or(&fonts.italic),
            (false, false) => fonts.mono_regular.as_ref().unwrap_or(&fonts.regular),
        },
        _ => match (bold, italic) {
            (true, true) => &fonts.bold_italic,
            (true, false) => &fonts.bold,
            (false, true) => &fonts.italic,
            (false, false) => &fonts.regular,
        },
    }
}

/// Returns the loaded font due for a requested generic family so inline SVG
/// `currentColor` matches the surrounding text even when the page asks for a
/// system font.  If no face is loaded for that family, falls back to the
/// corresponding sans-serif face.
fn pick_family_font(style: &ComputedStyle, bold: bool, italic: bool) -> &'static FontdueFont {
    pick_font(style, bold, italic, style.font_family)
}

/// Fonts decoded from registered @font-face data, keyed by
/// (family, weight, italic) so each face is parsed once.
static WEBFONT_FONTS: OnceLock<
    std::sync::RwLock<std::collections::HashMap<(String, u16, bool), std::sync::Arc<FontdueFont>>>,
> = OnceLock::new();

/// Numeric weight for web font matching (`FontWeight` carries CSS keywords).
fn font_weight_number(weight: &FontWeight) -> u16 {
    match weight {
        FontWeight::Normal | FontWeight::Lighter => 400,
        FontWeight::Bold | FontWeight::Bolder => 700,
        FontWeight::Number(n) => (*n).clamp(1, 1000),
    }
}

/// Get (building and caching if needed) the registered web font for a
/// family/weight/style combination.
fn get_webfont_font(
    family: &str,
    weight: u16,
    italic: bool,
) -> Option<std::sync::Arc<FontdueFont>> {
    let cache =
        WEBFONT_FONTS.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()));
    let key = (family.to_lowercase(), weight, italic);
    if let Ok(map) = cache.read() {
        if let Some(f) = map.get(&key) {
            return Some(std::sync::Arc::clone(f));
        }
    }
    let data = incognidium_css::webfonts::lookup(family, weight, italic)?;
    let font = match FontdueFont::from_bytes(&data[..], fontdue::FontSettings::default()) {
        Ok(f) => std::sync::Arc::new(f),
        Err(_) => return None,
    };
    if let Ok(mut map) = cache.write() {
        map.insert(key, std::sync::Arc::clone(&font));
    }
    Some(font)
}

/// Returns true for characters that should be rendered with a CJK fallback font
/// when the primary Latin font does not cover them.
fn is_cjk_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x2E80..=0x9FFF
            | 0xAC00..=0xD7AF
            | 0x3000..=0x303F
            | 0x3040..=0x309F
            | 0x30A0..=0x30FF
            | 0x3100..=0x312F
            | 0x31F0..=0x31FF
            | 0x3200..=0x32FF
            | 0x3400..=0x4DBF
            | 0xF900..=0xFAFF
            | 0xFF01..=0xFF60
            | 0xFF65..=0xFF9F
            | 0xFFE0..=0xFFE6
            | 0x20000..=0x2A6DF
            | 0x2A700..=0x2B73F
            | 0x2B740..=0x2B81F
            | 0x2F800..=0x2FA1F
    )
}

fn pick_font_for_char(
    style: &ComputedStyle,
    ch: char,
    bold: bool,
    italic: bool,
    family: FontFamily,
) -> &'static FontdueFont {
    let primary = pick_font(style, bold, italic, family);
    if primary.has_glyph(ch) {
        return primary;
    }
    if is_cjk_char(ch) {
        let (cjk_regular, cjk_bold) = cjk_fonts();
        if bold {
            if let Some(ref f) = *cjk_bold {
                return f;
            }
        }
        if let Some(ref f) = *cjk_regular {
            return f;
        }
    }
    if char_needs_emoji_fallback(ch) {
        if let Some((ref f, _)) = emoji_fonts().as_ref() {
            if f.lookup_glyph_index(ch) != 0 {
                return f;
            }
        }
    }
    let fallbacks = script_fallback_fonts(bold);
    for f in fallbacks.iter() {
        if f.has_glyph(ch) {
            return f;
        }
    }
    // If a bold request has no bold fallback, fall back to the regular script
    // fallback so the glyph is readable instead of tofu.
    if bold {
        for f in script_fallback_fonts(false).iter() {
            if f.has_glyph(ch) {
                return f;
            }
        }
    }
    primary
}

fn font_due_ascent(font: &FontdueFont, px: f32) -> f32 {
    font.horizontal_line_metrics(px)
        .map(|m| m.ascent)
        .unwrap_or(px * 0.8)
}

fn font_due_space_width(
    font: &FontdueFont,
    px: f32,
    word_spacing: f32,
    letter_spacing: f32,
) -> f32 {
    // CSS word-spacing adds to the intrinsic space advance and may shrink the
    // gap down to zero, but the used inter-word gap must not become negative.
    // Keep this calculation identical to the layout engine's effective_space_width
    // so intrinsic widths, line breaking, and painting all agree.
    let advance = font.metrics(' ', px).advance_width;
    (advance + word_spacing + letter_spacing * 2.0).max(0.0)
}

/// Effective space advance for a computed style, using the primary font the
/// text renderer would pick. This is used for paint-time visual-gap adjustments,
/// so it only needs to match the renderer's own space width approximately.
fn space_width_for_style(style: &ComputedStyle) -> f32 {
    let bold = matches!(style.font_weight, FontWeight::Bold | FontWeight::Bolder)
        || matches!(
            style.font_weight,
            FontWeight::Number(n) if n >= 500
        );
    let italic = style.font_style == FontStyle::Italic;
    let font = pick_family_font(style, bold, italic);
    font_due_space_width(
        font,
        style.font_size,
        style.word_spacing,
        style.letter_spacing,
    )
}

/// Left edge of the first non-whitespace glyph for a style, relative to the
/// text origin. Negative values mean the glyph overhangs to the left, which
/// must be included when measuring the visual gap from a previous text box.
fn first_glyph_left_edge_for_style(style: &ComputedStyle, text: &str) -> f32 {
    let first_char = match text.chars().find(|c| !c.is_whitespace()) {
        Some(c) => c,
        None => return 0.0,
    };
    let bold = matches!(style.font_weight, FontWeight::Bold | FontWeight::Bolder)
        || matches!(
            style.font_weight,
            FontWeight::Number(n) if n >= 500
        );
    let italic = style.font_style == FontStyle::Italic;
    let web_font = style
        .web_font_family
        .as_deref()
        .and_then(|fam| get_webfont_font(fam, font_weight_number(&style.font_weight), italic));
    let font: &FontdueFont = if let Some(ref wf) = web_font {
        if wf.lookup_glyph_index(first_char) != 0 {
            wf
        } else {
            pick_font_for_char(style, first_char, bold, italic, style.font_family)
        }
    } else {
        pick_font_for_char(style, first_char, bold, italic, style.font_family)
    };
    font.metrics(first_char, style.font_size).bounds.xmin
}

/// Baseline y-coordinate for a style when the top of the first line box is at
/// `box_top`. Matches the calculation used by `draw_text_ttf` so first-letter
/// glyphs can be aligned to the same baseline as the surrounding text.
fn baseline_y_for_style(box_top: f32, style: &ComputedStyle) -> f32 {
    let bold = matches!(style.font_weight, FontWeight::Bold | FontWeight::Bolder)
        || matches!(style.font_weight, FontWeight::Number(n) if n >= 500);
    let italic = style.font_style == FontStyle::Italic;
    let web_font = style
        .web_font_family
        .as_deref()
        .and_then(|fam| get_webfont_font(fam, font_weight_number(&style.font_weight), italic));
    let font = match web_font.as_ref() {
        Some(wf) => wf,
        None => pick_font(style, bold, italic, style.font_family),
    };
    box_top + font_due_ascent(font, style.font_size)
}

fn font_due_advance(font: &FontdueFont, ch: char, px: f32) -> f32 {
    font.metrics(ch, px).advance_width
}

fn font_due_kern(font: &FontdueFont, prev: char, ch: char, px: f32) -> f32 {
    font.horizontal_kern(prev, ch, px).unwrap_or(0.0)
}

fn draw_glyph_due(
    pixmap: &mut Pixmap,
    font: &FontdueFont,
    ch: char,
    font_size: f32,
    x: f32,
    y: f32,
    r: u8,
    g: u8,
    b: u8,
    a: u8,
    clip_mask: Option<&ClipMask>,
) {
    let (metrics, bitmap) = font.rasterize(ch, font_size);
    // Snap glyph origin to nearest pixel for sharper unhinted text.
    let glyph_x = (x + metrics.bounds.xmin).round();
    // fontdue uses Y-up: ymin is the bottom edge relative to baseline.
    // Screen is Y-down, so the top of the glyph is at y - (ymin + height).
    let glyph_y = (y - metrics.bounds.ymin - metrics.height as f32).round();
    let w = pixmap.width();
    let h = pixmap.height();
    let data = pixmap.data_mut();
    for (i, &coverage) in bitmap.iter().enumerate() {
        if coverage == 0 {
            continue;
        }
        let gx = (i % metrics.width) as i32;
        let gy = (i / metrics.width) as i32;
        let px = glyph_x as i32 + gx;
        let py = glyph_y as i32 + gy;
        if px >= 0 && py >= 0 {
            let px = px as u32;
            let py = py as u32;
            if px < w && py < h {
                if let Some(mask) = clip_mask {
                    if !clip_mask_contains(mask, px as f32, py as f32) {
                        continue;
                    }
                }
                let idx = ((py * w + px) * 4) as usize;
                if idx + 3 < data.len() {
                    // Use the rasterizer's coverage as-is: measured against
                    // reference browser output on both light and dark
                    // backgrounds, the raw unhinted coverage matches, while
                    // remapping it left gray halos around stems.
                    let alpha = coverage as u32;
                    let sa = (alpha * a as u32) / 255;
                    if sa == 0 {
                        continue;
                    }
                    let inv = 255 - sa;
                    data[idx] = ((r as u32 * sa + data[idx] as u32 * inv) / 255) as u8;
                    data[idx + 1] = ((g as u32 * sa + data[idx + 1] as u32 * inv) / 255) as u8;
                    data[idx + 2] = ((b as u32 * sa + data[idx + 2] as u32 * inv) / 255) as u8;
                    data[idx + 3] = (sa + (data[idx + 3] as u32 * inv) / 255) as u8;
                }
            }
        }
    }
}

/// Draw a color-emoji glyph from a bitmap font (e.g. Noto Color Emoji's
/// CBDT/CBLC PNG strikes). fontdue cannot rasterize these glyphs, so we use
/// ttf-parser to extract the PNG and alpha-blend it onto the pixmap ourselves.
fn draw_emoji_glyph(
    pixmap: &mut Pixmap,
    face: &ttf_parser::Face<'static>,
    ch: char,
    font_size: f32,
    x: f32,
    baseline_y: f32,
    clip_mask: Option<&ClipMask>,
) {
    let glyph_id = match face.glyph_index(ch) {
        Some(id) => id,
        None => return,
    };
    let ppem = font_size.round().clamp(1.0, 4096.0) as u16;
    let raster = match face.glyph_raster_image(glyph_id, ppem) {
        Some(r) => r,
        None => return,
    };
    if !matches!(raster.format, ttf_parser::RasterImageFormat::PNG) {
        return;
    }
    let img = match image::load_from_memory(raster.data) {
        Ok(img) => img.to_rgba8(),
        Err(_) => return,
    };
    if img.width() == 0 || img.height() == 0 {
        return;
    }

    let scale = font_size / raster.pixels_per_em.max(1) as f32;
    let src_w = img.width() as f32 * scale;
    let src_h = img.height() as f32 * scale;
    let offset_x = raster.x as f32 * scale;
    let offset_y = raster.y as f32 * scale;
    let img_x = (x + offset_x).round();
    let img_y = (baseline_y + offset_y).round();
    let pm_w = pixmap.width();
    let pm_h = pixmap.height();

    let out_w = src_w.round().max(0.0) as u32;
    let out_h = src_h.round().max(0.0) as u32;
    for py in 0..out_h {
        for px in 0..out_w {
            let sx = (px as f32 / src_w.max(1.0) * img.width() as f32).min(img.width() as f32 - 1.0)
                as u32;
            let sy = (py as f32 / src_h.max(1.0) * img.height() as f32)
                .min(img.height() as f32 - 1.0) as u32;
            let pixel = img.get_pixel(sx, sy);
            let [r, g, b, a] = pixel.0;
            if a == 0 {
                continue;
            }
            let dx = img_x as u32 + px;
            let dy = img_y as u32 + py;
            if dx >= pm_w || dy >= pm_h {
                continue;
            }
            if let Some(mask) = clip_mask {
                if !clip_mask_contains(mask, dx as f32, dy as f32) {
                    continue;
                }
            }
            blend_pixel(pixmap, dx, dy, r, g, b, a);
        }
    }
}

/// Alpha-blend a single pixel onto the pixmap.
/// sRGB → linear conversion LUT (8-bit → f32 in 0..1 range).
fn srgb_to_linear_lut() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| {
        let mut t = [0.0f32; 256];
        for i in 0..256 {
            let c = i as f32 / 255.0;
            t[i] = if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            };
        }
        t
    })
}

/// linear → sRGB conversion LUT.
fn linear_to_srgb_lut() -> &'static [u8; 256] {
    static LUT: OnceLock<[u8; 256]> = OnceLock::new();
    LUT.get_or_init(|| {
        let mut t = [0u8; 256];
        for i in 0..256 {
            let c = i as f32 / 255.0;
            let s = if c <= 0.0031308 {
                c * 12.92
            } else {
                1.055 * c.powf(1.0 / 2.4) - 0.055
            };
            t[i] = (s * 255.0 + 0.5).min(255.0) as u8;
        }
        t
    })
}

/// Alpha-blend a source colour over the destination pixel using
/// **gamma-correct** sRGB compositing ( Porter-Duff `over` in linear
/// light ).  This prevents the “thin washed-out text” look that
/// linear blending produces on light backgrounds.
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
        return;
    }

    let alpha = a as f32 / 255.0;

    // Convert destination and source from sRGB to linear
    let s2l = srgb_to_linear_lut();
    let l2s = linear_to_srgb_lut();
    let dst_r = s2l[data[idx] as usize];
    let dst_g = s2l[data[idx + 1] as usize];
    let dst_b = s2l[data[idx + 2] as usize];

    let src_r = s2l[r as usize];
    let src_g = s2l[g as usize];
    let src_b = s2l[b as usize];

    // Porter-Duff `over` in linear space
    let one_a = 1.0 - alpha;
    let out_r = src_r * alpha + dst_r * one_a;
    let out_g = src_g * alpha + dst_g * one_a;
    let out_b = src_b * alpha + dst_b * one_a;

    // Convert back to sRGB
    data[idx] = l2s[(out_r * 255.0).min(255.0) as usize];
    data[idx + 1] = l2s[(out_g * 255.0).min(255.0) as usize];
    data[idx + 2] = l2s[(out_b * 255.0).min(255.0) as usize];

    // Alpha channel stays in linear (standard straight-alpha)
    let dst_a = data[idx + 3] as f32 / 255.0;
    let out_a = alpha + dst_a * (1.0 - alpha);
    data[idx + 3] = (out_a * 255.0).min(255.0) as u8;
}

/// How an SVG source wants its content fitted to the viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SvgPreserveAspectRatio {
    /// `preserveAspectRatio="none"`; content may be distorted.
    None,
    /// `meet` alignment (the SVG default); content is scaled uniformly to fit
    /// inside the viewport, leaving transparent space if the aspect ratios differ.
    Meet,
    /// `slice` alignment; content is scaled uniformly to cover the viewport,
    /// cropping if the aspect ratios differ.
    Slice,
}

/// Cached image data for painting.
#[derive(Clone)]
pub struct ImageData {
    pub pixels: Vec<u8>, // RGBA
    /// Raster width of the decoded pixel buffer. This may differ from the
    /// intrinsic size for vector images that are rasterized at a higher
    /// resolution to stay crisp when painted scaled up.
    pub width: u32,
    /// Raster height of the decoded pixel buffer.
    pub height: u32,
    /// CSS intrinsic width used for layout and object-fit calculations.
    /// For raster images this equals `width`; for SVGs it is the source's
    /// declared or viewBox-derived intrinsic width.
    pub intrinsic_width: u32,
    /// CSS intrinsic height used for layout and object-fit calculations.
    pub intrinsic_height: u32,
    /// For SVG images, the `preserveAspectRatio` value from the source. `None`
    /// means the image is a raster, or an SVG with no explicit instruction and
    /// the default `meet` behavior should be applied by the paint step.
    pub svg_preserve_aspect_ratio: Option<SvgPreserveAspectRatio>,
}

/// Estimate the width of text given a style
/// This is a simplified estimation - for accurate rendering we'd need the actual font metrics
fn estimate_text_width(text: &str, style: &ComputedStyle) -> f32 {
    if text.is_empty() {
        return 0.0;
    }

    // Use a rough approximation based on font size and character count
    // Average character width is roughly 0.6 * font_size for most fonts
    let char_count = text.chars().count() as f32;
    let avg_char_width = style.font_size * 0.6;

    // Adjust for font weight (bold or medium text is slightly wider)
    let is_heavy = matches!(
        style.font_weight,
        incognidium_style::FontWeight::Bold | incognidium_style::FontWeight::Bolder
    ) || matches!(style.font_weight, incognidium_style::FontWeight::Number(n) if n >= 500);
    let weight_factor = if is_heavy { 1.1 } else { 1.0 };

    // Base width
    let base_width = char_count * avg_char_width * weight_factor;

    // Add letter-spacing
    let letter_spacing = style.letter_spacing * char_count;

    base_width + letter_spacing
}

/// Build a tiny-skia `Transform` from a CSS transform list. The origin is the
/// pivot point for rotate/scale/skew; translate percentages resolve against the
/// element's box size.
fn build_transform(
    transforms: &[incognidium_style::Transform],
    origin_x: f32,
    origin_y: f32,
    width: f32,
    height: f32,
) -> Transform {
    let mut a = 1.0_f32;
    let mut b = 0.0_f32;
    let mut c = 0.0_f32;
    let mut d = 0.0_f32;
    let mut e = 1.0_f32;
    let mut f = 0.0_f32;

    fn post_concat(
        a: &mut f32,
        b: &mut f32,
        c: &mut f32,
        d: &mut f32,
        e: &mut f32,
        f: &mut f32,
        na: f32,
        nb: f32,
        nc: f32,
        nd: f32,
        ne: f32,
        nf: f32,
    ) {
        let oa = *a;
        let ob = *b;
        let oc = *c;
        let od = *d;
        let oe = *e;
        let of = *f;
        *a = na * oa + nb * od;
        *b = na * ob + nb * oe;
        *c = na * oc + nb * of + nc;
        *d = nd * oa + ne * od;
        *e = nd * ob + ne * oe;
        *f = nd * oc + ne * of + nf;
    }

    for t in transforms {
        match *t {
            incognidium_style::Transform::Translate(x, y) => post_concat(
                &mut a, &mut b, &mut c, &mut d, &mut e, &mut f, 1.0, 0.0, x, 0.0, 1.0, y,
            ),
            incognidium_style::Transform::TranslateX(x) => post_concat(
                &mut a, &mut b, &mut c, &mut d, &mut e, &mut f, 1.0, 0.0, x, 0.0, 1.0, 0.0,
            ),
            incognidium_style::Transform::TranslateY(y) => post_concat(
                &mut a, &mut b, &mut c, &mut d, &mut e, &mut f, 1.0, 0.0, 0.0, 0.0, 1.0, y,
            ),
            incognidium_style::Transform::TranslateXPercent(p) => {
                let x = width * p / 100.0;
                post_concat(
                    &mut a, &mut b, &mut c, &mut d, &mut e, &mut f, 1.0, 0.0, x, 0.0, 1.0, 0.0,
                )
            }
            incognidium_style::Transform::TranslateYPercent(p) => {
                let y = height * p / 100.0;
                post_concat(
                    &mut a, &mut b, &mut c, &mut d, &mut e, &mut f, 1.0, 0.0, 0.0, 0.0, 1.0, y,
                )
            }
            incognidium_style::Transform::Scale(sx, sy) => post_concat(
                &mut a, &mut b, &mut c, &mut d, &mut e, &mut f, sx, 0.0, 0.0, 0.0, sy, 0.0,
            ),
            incognidium_style::Transform::ScaleX(sx) => post_concat(
                &mut a, &mut b, &mut c, &mut d, &mut e, &mut f, sx, 0.0, 0.0, 0.0, 1.0, 0.0,
            ),
            incognidium_style::Transform::ScaleY(sy) => post_concat(
                &mut a, &mut b, &mut c, &mut d, &mut e, &mut f, 1.0, 0.0, 0.0, 0.0, sy, 0.0,
            ),
            incognidium_style::Transform::Rotate(deg) => {
                let rad = deg.to_radians();
                let cos = rad.cos();
                let sin = rad.sin();
                post_concat(
                    &mut a, &mut b, &mut c, &mut d, &mut e, &mut f, cos, -sin, 0.0, sin, cos, 0.0,
                )
            }
            incognidium_style::Transform::Skew(ax, ay) => {
                let tan_x = ax.to_radians().tan();
                let tan_y = ay.to_radians().tan();
                post_concat(
                    &mut a, &mut b, &mut c, &mut d, &mut e, &mut f, 1.0, tan_y, 0.0, tan_x, 1.0,
                    0.0,
                )
            }
            incognidium_style::Transform::SkewX(ax) => {
                let tan_x = ax.to_radians().tan();
                post_concat(
                    &mut a, &mut b, &mut c, &mut d, &mut e, &mut f, 1.0, 0.0, 0.0, tan_x, 1.0, 0.0,
                )
            }
            incognidium_style::Transform::SkewY(ay) => {
                let tan_y = ay.to_radians().tan();
                post_concat(
                    &mut a, &mut b, &mut c, &mut d, &mut e, &mut f, 1.0, tan_y, 0.0, 0.0, 1.0, 0.0,
                )
            }
        }
    }

    // CSS transform order: translate to origin, apply matrix, translate back.
    // tiny-skia's from_row takes (sx, ky, kx, sy, tx, ty).
    Transform::from_row(
        a,
        d,
        b,
        e,
        c + origin_x - a * origin_x - b * origin_y,
        f + origin_y - d * origin_x - e * origin_y,
    )
}

/// Transform an axis-aligned clip rectangle by a transform and return the
/// bounding box of the transformed corners.
fn transform_clip_bounds(
    clip: Option<(f32, f32, f32, f32)>,
    transform: Transform,
) -> Option<(f32, f32, f32, f32)> {
    if transform == Transform::identity() {
        return clip;
    }
    let (cx, cy, cw, ch) = clip?;
    let x0 = cx;
    let y0 = cy;
    let x1 = cx + cw;
    let y1 = cy + ch;
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for (px, py) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
        let mut pt = tiny_skia::Point::from_xy(px, py);
        transform.map_point(&mut pt);
        min_x = min_x.min(pt.x);
        min_y = min_y.min(pt.y);
        max_x = max_x.max(pt.x);
        max_y = max_y.max(pt.y);
    }
    Some((min_x, min_y, max_x - min_x, max_y - min_y))
}

/// Paint the layout tree into a pixel buffer.
pub fn paint(flat_boxes: &[FlatBox], styles: &StyleMap, width: u32, height: u32) -> Pixmap {
    paint_with_images(flat_boxes, styles, width, height, &HashMap::new())
}

/// Paint with image support, defaulting to a white canvas background.
pub fn paint_with_images(
    flat_boxes: &[FlatBox],
    styles: &StyleMap,
    width: u32,
    height: u32,
    images: &HashMap<String, ImageData>,
) -> Pixmap {
    paint_with_images_and_canvas(flat_boxes, styles, width, height, images, None)
}

/// Paint with image support and an explicit canvas background color.
///
/// The canvas background is the color that fills the output pixmap before any
/// layout boxes are painted. When `canvas_bg` is `None` the canvas is filled
/// white, preserving the previous default behavior.
pub fn paint_with_images_and_canvas(
    flat_boxes: &[FlatBox],
    styles: &StyleMap,
    width: u32,
    height: u32,
    images: &HashMap<String, ImageData>,
    canvas_bg: Option<CssColor>,
) -> Pixmap {
    let mut pixmap = Pixmap::new(width, height).expect("failed to create pixmap");

    // Fill background: explicit canvas color if provided, otherwise white.
    if let Some(bg) = canvas_bg {
        pixmap.fill(css_to_skia_color(bg));
    } else {
        pixmap.fill(Color::WHITE);
    }

    // Build a stacking-context tree and paint recursively.
    //
    // Each flat box knows the nearest stacking context it belongs to
    // (`stacking_context_root`) and the parent stacking context it is painted in
    // (`parent_stacking_context`). For a box that establishes its own context,
    // `stacking_context_root` is itself and `parent_stacking_context` is the
    // enclosing context. We group boxes by their `stacking_context_root` and then
    // attach each context under its parent. When painting a context we emit its
    // own boxes (root background + non-context descendants) in document order, then
    // recursively paint child contexts sorted by z-index. This keeps a parent
    // background behind its children while still letting sibling stacking
    // contexts with different z-index overlap correctly.
    use std::collections::HashMap;
    let mut groups: HashMap<Option<incognidium_dom::NodeId>, Vec<&FlatBox>> = HashMap::new();
    let mut first_index: HashMap<Option<incognidium_dom::NodeId>, usize> = HashMap::new();
    let mut root_parent: HashMap<Option<incognidium_dom::NodeId>, Option<incognidium_dom::NodeId>> =
        HashMap::new();
    for (i, fb) in flat_boxes.iter().enumerate() {
        let root = fb.stacking_context_root;
        groups.entry(root).or_default().push(fb);
        let entry = first_index.entry(root).or_insert(i);
        if i < *entry {
            *entry = i;
        }
        // The parent context of the root box itself is this box's parent_stacking_context.
        // Only the first flat box for a given root (the root box itself) should set this;
        // descendants that share the same node id (e.g. generated pseudo-element text)
        // must not overwrite it with their own parent context.
        if Some(fb.node_id) == root {
            root_parent
                .entry(root)
                .or_insert(fb.parent_stacking_context);
        }
    }

    // Within each stacking context, CSS paint order requires in-flow non-positioned
    // boxes to be painted before auto-z-index positioned descendants, and all such
    // positioned descendants must be painted in document (tree) order. The flat
    // boxes are collected in document order, so stable-partition each group so
    // that in-flow boxes come first, floats come next, and every positioned box
    // (relative, sticky, absolute, or fixed) with auto z-index comes last, with its
    // whole subtree kept as one contiguous unit. Capturing the subtree preserves the
    // internal order of a positioned container's background, in-flow children, and
    // nested positioned descendants, which keeps a relative box's background from
    // painting over its own text while still letting a later relative sibling paint
    // above an earlier absolute sibling.
    for group in groups.values_mut() {
        let mut non_float: Vec<&FlatBox> = Vec::with_capacity(group.len());
        let mut floats: Vec<&FlatBox> = Vec::with_capacity(group.len());
        let mut positioned: Vec<&FlatBox> = Vec::with_capacity(group.len());
        let mut capturing_positioned_depth: Option<u32> = None;
        let mut capturing_float_depth: Option<u32> = None;
        for fb in group.drain(..) {
            // Keep a float's entire subtree in the floats group. Otherwise the
            // float's background/border is painted after its in-flow children,
            // covering the text and other content inside the floated box.
            if let Some(threshold) = capturing_float_depth {
                if fb.depth > threshold {
                    floats.push(fb);
                    continue;
                }
                capturing_float_depth = None;
            }
            if let Some(threshold) = capturing_positioned_depth {
                if fb.depth > threshold {
                    positioned.push(fb);
                    continue;
                }
                capturing_positioned_depth = None;
            }
            let style = styles.get(&fb.node_id).cloned().unwrap_or_default();
            let is_auto_positioned = style.z_index.is_none() && style.position != Position::Static;
            if is_auto_positioned {
                capturing_positioned_depth = Some(fb.depth);
                positioned.push(fb);
            } else if style.float != Float::None {
                capturing_float_depth = Some(fb.depth);
                floats.push(fb);
            } else {
                non_float.push(fb);
            }
        }
        // CSS paint order within a stacking context: non-positioned in-flow block
        // boxes (backgrounds/borders) first, then non-positioned floats, then
        // auto-z-index positioned descendants in tree order.
        group.extend(non_float);
        group.extend(floats);
        group.extend(positioned);
    }

    // Attach every context under its parent context.
    let mut children: HashMap<
        Option<incognidium_dom::NodeId>,
        Vec<Option<incognidium_dom::NodeId>>,
    > = HashMap::new();
    for root in groups.keys().copied() {
        let parent = root_parent.get(&root).copied().unwrap_or(None);
        // The root viewport context has no parent; do not attach it to itself.
        if parent != root {
            children.entry(parent).or_default().push(root);
        }
    }

    fn collect_context<'a>(
        root: Option<incognidium_dom::NodeId>,
        groups: &mut HashMap<Option<incognidium_dom::NodeId>, Vec<&'a FlatBox>>,
        children: &HashMap<Option<incognidium_dom::NodeId>, Vec<Option<incognidium_dom::NodeId>>>,
        first_index: &HashMap<Option<incognidium_dom::NodeId>, usize>,
        styles: &StyleMap,
        out: &mut Vec<&'a FlatBox>,
    ) {
        // Build sorted list of child stacking contexts once
        let mut sorted_children: Vec<Option<incognidium_dom::NodeId>> = Vec::new();
        if let Some(child_roots) = children.get(&root) {
            sorted_children = child_roots.clone();
            sorted_children.sort_by(|a, b| {
                let z_a = a
                    .and_then(|id| styles.get(&id))
                    .map(|s| s.z_index.unwrap_or(0))
                    .unwrap_or(0);
                let z_b = b
                    .and_then(|id| styles.get(&id))
                    .map(|s| s.z_index.unwrap_or(0))
                    .unwrap_or(0);
                let idx_a = first_index.get(a).copied().unwrap_or(0);
                let idx_b = first_index.get(b).copied().unwrap_or(0);
                z_a.cmp(&z_b).then(idx_a.cmp(&idx_b))
            });
        }

        // Paint negative z-index children BEFORE this context's own boxes
        for child in &sorted_children {
            let z = child
                .and_then(|id| styles.get(&id))
                .map(|s| s.z_index.unwrap_or(0))
                .unwrap_or(0);
            if z < 0 {
                collect_context(*child, groups, children, first_index, styles, out);
            }
        }

        if let Some(group) = groups.remove(&root) {
            out.extend(group);
        }

        // Paint non-negative z-index children AFTER this context's own boxes
        for child in &sorted_children {
            let z = child
                .and_then(|id| styles.get(&id))
                .map(|s| s.z_index.unwrap_or(0))
                .unwrap_or(0);
            if z >= 0 {
                collect_context(*child, groups, children, first_index, styles, out);
            }
        }
    }

    let mut sorted_boxes: Vec<&FlatBox> = Vec::with_capacity(flat_boxes.len());
    collect_context(
        None,
        &mut groups,
        &children,
        &first_index,
        styles,
        &mut sorted_boxes,
    );
    // Track which elements have already emitted a ::backdrop rectangle so the
    // pseudo-element is painted exactly once before the first flat box of the
    // dialog/popover element.
    let mut drawn_backdrops: HashSet<incognidium_dom::NodeId> = HashSet::new();
    // Track the previous inline text box so paint can preserve the visual gap
    // between adjacent inline siblings when glyph bounding boxes overhang their
    // advances (common with uppercase text in fallback fonts).
    let mut last_text_visual_right: f32 = 0.0;
    let mut last_text_advance_end: f32 = 0.0;
    let mut last_text_y: f32 = f32::NEG_INFINITY;
    let mut last_text_trailing_space: bool = false;

    for fbox in sorted_boxes {
        let style = styles.get(&fbox.node_id).cloned().unwrap_or_default();

        if style.display == Display::None
            || style.visibility != Visibility::Visible
            || fbox.opacity == 0.0
        {
            continue;
        }

        // Draw the ::backdrop pseudo-element for modal elements. It covers the
        // whole viewport with the specified background color/opacity and sits
        // behind the element and its box-shadow.
        if let Some(bg) = style.backdrop_background_color {
            if drawn_backdrops.insert(fbox.node_id) {
                let mut backdrop_color = bg;
                if let Some(op) = style.backdrop_opacity {
                    let factor = op.max(0.0).min(1.0);
                    backdrop_color.a = (backdrop_color.a as f32 * factor) as u8;
                }
                draw_rect(
                    &mut pixmap,
                    0.0,
                    0.0,
                    width as f32,
                    height as f32,
                    backdrop_color,
                );
            }
        }

        // Calculate transform for this element. If the layout flattening step already
        // applied this transform because the box is a descendant of the element that
        // owns it, skip it in paint to avoid double-transforming shared pseudo-element
        // text fragments and other child flat boxes.
        let transform = if fbox.transform_applied || style.transform.is_empty() {
            Transform::identity()
        } else {
            // Use transform-origin from style (values are 0-1 percentages)
            let origin_x = fbox.x + fbox.width * style.transform_origin.0;
            let origin_y = fbox.y + fbox.height * style.transform_origin.1;
            build_transform(
                &style.transform,
                origin_x,
                origin_y,
                fbox.width,
                fbox.height,
            )
        };

        // Transform clip bounds if this element has a transform
        // The clip comes from ancestors' overflow:hidden and is in their coordinate space
        // If this element has a transform, the clip needs to be transformed too
        let transformed_clip = if fbox.transform_applied {
            fbox.clip
        } else {
            transform_clip_bounds(fbox.clip, transform)
        };

        // Apply the effective opacity inherited from ancestors. An opacity:0
        // ancestor hides its whole subtree; partial opacity modulates the box
        // and its text so hidden menus, loading placeholders, and fade
        // transitions render correctly.
        let opacity = fbox.opacity;
        let mut effective_style = style.clone();
        if opacity < 1.0 {
            effective_style.background_color.a =
                (effective_style.background_color.a as f32 * opacity) as u8;
            effective_style.border_color.a =
                (effective_style.border_color.a as f32 * opacity) as f32 as u8;
            effective_style.color.a = (effective_style.color.a as f32 * opacity) as u8;
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
        // Drawing element

        // Capture the backdrop for mix-blend-mode before we paint this element.
        // When we will blend, clear the same region to transparent so the pixmap
        // holds only the element's source group while we paint it, avoiding the
        // need to recover source color from an already-composited result.
        let mut mix_backdrop: Option<Vec<u8>> = None;
        let will_mix_blend = transform == Transform::identity()
            && !matches!(
                style.mix_blend_mode,
                incognidium_style::MixBlendMode::Normal
            )
            && style.isolation != incognidium_style::Isolation::Isolate;
        if will_mix_blend {
            let x0 = draw_x.max(0.0) as u32;
            let y0 = draw_y.max(0.0) as u32;
            let x1 = ((draw_x + draw_w).min(width as f32)).max(x0 as f32) as u32;
            let y1 = ((draw_y + draw_h).min(height as f32)).max(y0 as f32) as u32;
            if x0 < x1 && y0 < y1 {
                let pm_width = pixmap.width();
                let data = pixmap.data();
                let mut buf = Vec::with_capacity(((x1 - x0) * (y1 - y0) * 4) as usize);
                for py in y0..y1 {
                    let row_start = ((py * pm_width + x0) * 4) as usize;
                    let row_end = ((py * pm_width + x1) * 4) as usize;
                    buf.extend_from_slice(&data[row_start..row_end]);
                }
                mix_backdrop = Some(buf);
                // Clear the region to transparent so the following draw calls
                // build the isolated source group.
                clear_pixmap_region(&mut pixmap, x0, y0, x1 - x0, y1 - y0);
            }
        }

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
                        style.border_top_left_radius.clone(),
                        style.border_top_right_radius.clone(),
                        style.border_bottom_right_radius.clone(),
                        style.border_bottom_left_radius.clone(),
                    );
                }
            }
        }

        // Build clip path if clip-path is set (inherited from ancestors via layout)
        let css_clip_path = build_clip_path(draw_x, draw_y, draw_w, draw_h, &fbox.clip_path);
        let css_clip_mask = css_clip_path.as_ref().and_then(build_clip_mask);

        // Ancestor overflow:hidden clips are rectangular by default, but if the
        // ancestor also has a border-radius, descendants must be clipped to the
        // rounded padding box of that ancestor.
        let rounded_overflow_clip = if let (Some((cx, cy, cw, ch)), Some((rtl, rtr, rbr, rbl))) =
            (transformed_clip, fbox.clip_border_radius)
        {
            let max_radius = (cw.min(ch) / 2.0).max(0.0);
            let rtl = rtl.min(max_radius);
            let rtr = rtr.min(max_radius);
            let rbr = rbr.min(max_radius);
            let rbl = rbl.min(max_radius);
            Some(build_rounded_rect_path(cx, cy, cw, ch, rtl, rtr, rbr, rbl))
        } else {
            None
        };
        let rounded_overflow_mask = rounded_overflow_clip.as_ref().and_then(build_clip_mask);
        let clip_mask = combine_clip_masks(css_clip_mask, rounded_overflow_mask);

        // Apply backdrop-filter if set - captures what's behind the element and applies filter
        // This must be done before drawing the element's own background
        if !style.backdrop_filter.is_empty() && (draw_w > 0.0 && draw_h > 0.0) {
            apply_backdrop_filter(
                &mut pixmap,
                draw_x,
                draw_y,
                draw_w,
                draw_h,
                &style.backdrop_filter,
                css_clip_path.as_ref(),
            );
        }

        // Skip background and borders for empty cells with empty-cells: hide
        let skip_cell_rendering = fbox.hide_empty_cell;

        // Check print-color-adjust: economy skips backgrounds to save ink (like print media)
        let skip_background_for_economy = style.print_color_adjust == PrintColorAdjust::Economy;

        // Calculate background clip area based on background-clip property
        // This determines where the background extends to (border, padding, or content)
        let (bg_x, bg_y, bg_w, bg_h) = match style.background_clip {
            incognidium_style::BackgroundClip::BorderBox => (draw_x, draw_y, draw_w, draw_h),
            incognidium_style::BackgroundClip::PaddingBox => {
                // Background extends to padding edge (inside border)
                let border_left = style.border_left_width;
                let border_right = style.border_right_width;
                let border_top = style.border_top_width;
                let border_bottom = style.border_bottom_width;
                (
                    draw_x + border_left,
                    draw_y + border_top,
                    (draw_w - border_left - border_right).max(0.0),
                    (draw_h - border_top - border_bottom).max(0.0),
                )
            }
            incognidium_style::BackgroundClip::ContentBox => {
                // Background extends to content edge (inside padding)
                let border_left = style.border_left_width + style.padding_left;
                let border_right = style.border_right_width + style.padding_right;
                let border_top = style.border_top_width + style.padding_top;
                let border_bottom = style.border_bottom_width + style.padding_bottom;
                (
                    draw_x + border_left,
                    draw_y + border_top,
                    (draw_w - border_left - border_right).max(0.0),
                    (draw_h - border_top - border_bottom).max(0.0),
                )
            }
            incognidium_style::BackgroundClip::Text => {
                // For text clip, we handle it separately - background only shows through text
                // For now, fall back to padding-box behavior (text clipping is complex)
                let border_left = style.border_left_width;
                let border_right = style.border_right_width;
                let border_top = style.border_top_width;
                let border_bottom = style.border_bottom_width;
                (
                    draw_x + border_left,
                    draw_y + border_top,
                    (draw_w - border_left - border_right).max(0.0),
                    (draw_h - border_top - border_bottom).max(0.0),
                )
            }
        };

        // `background-origin` determines the positioning area for background
        // images, independent of `background-clip`. Positioning must use the
        // origin box so that repeated tiles start at the padding edge and
        // `background-position` percentages resolve against the right box.
        let (origin_x, origin_y, origin_w, origin_h) = match style.background_origin {
            incognidium_style::BackgroundOrigin::BorderBox => (draw_x, draw_y, draw_w, draw_h),
            incognidium_style::BackgroundOrigin::PaddingBox => {
                let border_left = style.border_left_width;
                let border_right = style.border_right_width;
                let border_top = style.border_top_width;
                let border_bottom = style.border_bottom_width;
                (
                    draw_x + border_left,
                    draw_y + border_top,
                    (draw_w - border_left - border_right).max(0.0),
                    (draw_h - border_top - border_bottom).max(0.0),
                )
            }
            incognidium_style::BackgroundOrigin::ContentBox => {
                let left = style.border_left_width + style.padding_left;
                let right = style.border_right_width + style.padding_right;
                let top = style.border_top_width + style.padding_top;
                let bottom = style.border_bottom_width + style.padding_bottom;
                (
                    draw_x + left,
                    draw_y + top,
                    (draw_w - left - right).max(0.0),
                    (draw_h - top - bottom).max(0.0),
                )
            }
        };

        // Draw background (clipped) - check for gradient first, then solid color
        // Note: transforms on gradients need special handling
        // Skip backgrounds if print-color-adjust: economy is set
        let mut bg_drawn = false;
        let mask_image_ref = style.mask_image.as_ref().and_then(|src| images.get(src));
        let mask_unavailable = style.mask_image.is_some() && mask_image_ref.is_none();
        // CSS mask-image hides the element when the referenced image fails to load,
        // so skip all background layers in that case. When the mask is available,
        // draw the layers normally and then clip them to the mask shape.
        let should_mask = mask_image_ref.is_some();
        // Draw the solid background color first so it sits beneath all
        // background-image layers, matching CSS background layer order.
        if !skip_cell_rendering
            && !skip_background_for_economy
            && !mask_unavailable
            && style.background_color.a > 0
        {
            if let Some(ref cp) = css_clip_path {
                draw_solid_rect_clipped(
                    &mut pixmap,
                    bg_x,
                    bg_y,
                    bg_w,
                    bg_h,
                    style.background_color,
                    cp,
                    transform,
                    clip_mask.as_ref(),
                );
            } else {
                draw_rounded_rect_with_transform(
                    &mut pixmap,
                    bg_x,
                    bg_y,
                    bg_w,
                    bg_h,
                    style.background_color,
                    style.border_top_left_radius.clone(),
                    style.border_top_right_radius.clone(),
                    style.border_bottom_right_radius.clone(),
                    style.border_bottom_left_radius.clone(),
                    transform,
                    clip_mask.as_ref(),
                );
            }
            bg_drawn = true;
        }
        if !skip_cell_rendering && !skip_background_for_economy && !mask_unavailable {
            // CSS lists background layers from the closest to the viewer (top)
            // to the farthest (bottom), so paint them in reverse order so the
            // bottom layer ends up underneath the layers above it.
            for img in style.background_image.iter().rev() {
                let drawn = match img {
                    incognidium_style::BackgroundImage::LinearGradient(grad) => {
                        if let Some(ref cp) = css_clip_path {
                            draw_linear_gradient_clipped(
                                &mut pixmap,
                                bg_x,
                                bg_y,
                                bg_w,
                                bg_h,
                                grad,
                                cp,
                                transform,
                                style.border_top_left_radius.clone(),
                                style.border_top_right_radius.clone(),
                                style.border_bottom_right_radius.clone(),
                                style.border_bottom_left_radius.clone(),
                                clip_mask.as_ref(),
                            );
                        } else {
                            draw_linear_gradient(
                                &mut pixmap,
                                bg_x,
                                bg_y,
                                bg_w,
                                bg_h,
                                grad,
                                style.border_top_left_radius.clone(),
                                style.border_top_right_radius.clone(),
                                style.border_bottom_right_radius.clone(),
                                style.border_bottom_left_radius.clone(),
                                transform,
                                clip_mask.as_ref(),
                            );
                        }
                        true
                    }
                    incognidium_style::BackgroundImage::RadialGradient(grad) => {
                        if let Some(ref cp) = css_clip_path {
                            draw_radial_gradient_clipped(
                                &mut pixmap,
                                bg_x,
                                bg_y,
                                bg_w,
                                bg_h,
                                grad,
                                cp,
                                transform,
                                style.border_top_left_radius.clone(),
                                style.border_top_right_radius.clone(),
                                style.border_bottom_right_radius.clone(),
                                style.border_bottom_left_radius.clone(),
                                clip_mask.as_ref(),
                            );
                        } else {
                            draw_radial_gradient(
                                &mut pixmap,
                                bg_x,
                                bg_y,
                                bg_w,
                                bg_h,
                                grad,
                                style.border_top_left_radius.clone(),
                                style.border_top_right_radius.clone(),
                                style.border_bottom_right_radius.clone(),
                                style.border_bottom_left_radius.clone(),
                                transform,
                                clip_mask.as_ref(),
                            );
                        }
                        true
                    }
                    incognidium_style::BackgroundImage::Url(ref src) => {
                        if let Some(img) = images.get(src) {
                            let img_w = img.intrinsic_width.max(1) as f32;
                            let img_h = img.intrinsic_height.max(1) as f32;
                            let img_aspect = img_w / img_h.max(0.0001);
                            let box_aspect = bg_w / bg_h.max(0.0001);

                            // Determine the rendered size of the background image
                            // according to CSS `background-size`.
                            let (rendered_w, rendered_h) = match style.background_size {
                                incognidium_style::BackgroundSize::Cover => {
                                    if img_aspect < box_aspect {
                                        (bg_w, bg_w / img_aspect)
                                    } else {
                                        (bg_h * img_aspect, bg_h)
                                    }
                                }
                                incognidium_style::BackgroundSize::Contain => {
                                    if img_aspect > box_aspect {
                                        (bg_w, bg_w / img_aspect)
                                    } else {
                                        (bg_h * img_aspect, bg_h)
                                    }
                                }
                                incognidium_style::BackgroundSize::Length(w, h) => {
                                    let rw = if w > 0.0 { w } else { img_w };
                                    let rh = if h > 0.0 {
                                        h
                                    } else {
                                        img_h * rw / img_w.max(0.0001)
                                    };
                                    (rw, rh)
                                }
                                _ => (img_w, img_h),
                            };

                            let (off_x, norm_x) = resolve_background_position(
                                style.background_position.0,
                                origin_w,
                                rendered_w,
                            );
                            let (off_y, norm_y) = resolve_background_position(
                                style.background_position.1,
                                origin_h,
                                rendered_h,
                            );

                            let repeat_x = matches!(
                                style.background_repeat,
                                incognidium_style::BackgroundRepeat::Repeat
                                    | incognidium_style::BackgroundRepeat::RepeatX
                            );
                            let repeat_y = matches!(
                                style.background_repeat,
                                incognidium_style::BackgroundRepeat::Repeat
                                    | incognidium_style::BackgroundRepeat::RepeatY
                            );

                            // Helper to draw one tile at the resolved box.
                            let mut draw_tile =
                                |x: f32,
                                 y: f32,
                                 w: f32,
                                 h: f32,
                                 fit: incognidium_style::ObjectFit,
                                 pos: (f32, f32)| {
                                    if css_clip_path.is_some() || fbox.clip_border_radius.is_some()
                                    {
                                        draw_image_with_transform_and_clip(
                                            &mut pixmap,
                                            x,
                                            y,
                                            w,
                                            h,
                                            img,
                                            fbox.clip,
                                            fbox.clip_border_radius,
                                            transform,
                                            fit,
                                            false,
                                            pos,
                                            style.image_rendering,
                                            style.border_top_left_radius.clone(),
                                            style.border_top_right_radius.clone(),
                                            style.border_bottom_right_radius.clone(),
                                            style.border_bottom_left_radius.clone(),
                                        );
                                    } else {
                                        draw_image_with_transform(
                                            &mut pixmap,
                                            x,
                                            y,
                                            w,
                                            h,
                                            img,
                                            transform,
                                            fit,
                                            false,
                                            pos,
                                            style.image_rendering,
                                            style.border_top_left_radius.clone(),
                                            style.border_top_right_radius.clone(),
                                            style.border_bottom_right_radius.clone(),
                                            style.border_bottom_left_radius.clone(),
                                            Some((bg_x, bg_y, bg_w, bg_h)),
                                        );
                                    }
                                };

                            if !repeat_x && !repeat_y {
                                // No-repeat: draw a single tile exactly as before.
                                let (object_fit, object_position, draw_x, draw_y, draw_w, draw_h) =
                                    match style.background_size {
                                        incognidium_style::BackgroundSize::Cover => (
                                            incognidium_style::ObjectFit::Cover,
                                            (norm_x, norm_y),
                                            origin_x,
                                            origin_y,
                                            origin_w,
                                            origin_h,
                                        ),
                                        incognidium_style::BackgroundSize::Contain => (
                                            incognidium_style::ObjectFit::Contain,
                                            (norm_x, norm_y),
                                            origin_x,
                                            origin_y,
                                            origin_w,
                                            origin_h,
                                        ),
                                        incognidium_style::BackgroundSize::Length(_, _) => (
                                            incognidium_style::ObjectFit::Fill,
                                            (0.0, 0.0),
                                            origin_x + off_x,
                                            origin_y + off_y,
                                            rendered_w.max(1.0),
                                            rendered_h.max(1.0),
                                        ),
                                        _ => (
                                            incognidium_style::ObjectFit::None,
                                            (norm_x, norm_y),
                                            bg_x,
                                            bg_y,
                                            bg_w,
                                            bg_h,
                                        ),
                                    };
                                draw_tile(
                                    draw_x,
                                    draw_y,
                                    draw_w,
                                    draw_h,
                                    object_fit,
                                    object_position,
                                );
                            } else {
                                // CSS `background-repeat: repeat` (and its axis
                                // variants) tiles the image across the background
                                // positioning area. The first image in the list is
                                // closest to the viewer, and its position sets the
                                // tiling origin.
                                let tile_w = rendered_w.max(1.0);
                                let tile_h = rendered_h.max(1.0);
                                let tile_origin_x = origin_x + off_x;
                                let tile_origin_y = origin_y + off_y;

                                let i_start =
                                    ((origin_x - tile_origin_x) / tile_w).floor() as i32 - 1;
                                let i_end =
                                    ((origin_x + origin_w - tile_origin_x) / tile_w).ceil() as i32;
                                let j_start =
                                    ((origin_y - tile_origin_y) / tile_h).floor() as i32 - 1;
                                let j_end =
                                    ((origin_y + origin_h - tile_origin_y) / tile_h).ceil() as i32;

                                let object_fit = match style.background_size {
                                    incognidium_style::BackgroundSize::Cover => {
                                        incognidium_style::ObjectFit::Cover
                                    }
                                    incognidium_style::BackgroundSize::Contain => {
                                        incognidium_style::ObjectFit::Contain
                                    }
                                    incognidium_style::BackgroundSize::Length(_, _) => {
                                        incognidium_style::ObjectFit::Fill
                                    }
                                    _ => incognidium_style::ObjectFit::None,
                                };

                                for i in i_start..=i_end {
                                    if !repeat_x && i != 0 {
                                        continue;
                                    }
                                    for j in j_start..=j_end {
                                        if !repeat_y && j != 0 {
                                            continue;
                                        }
                                        let tile_x = tile_origin_x + i as f32 * tile_w;
                                        let tile_y = tile_origin_y + j as f32 * tile_h;
                                        // Skip tiles that do not overlap the
                                        // background positioning area (where the
                                        // tile pattern must cover) and the
                                        // background painting area (where the
                                        // result is allowed to show). This keeps
                                        // partial edge tiles from being painted
                                        // outside the element.
                                        if tile_x + tile_w <= origin_x
                                            || tile_x >= origin_x + origin_w
                                            || tile_y + tile_h <= origin_y
                                            || tile_y >= origin_y + origin_h
                                        {
                                            continue;
                                        }
                                        if tile_x + tile_w <= bg_x
                                            || tile_x >= bg_x + bg_w
                                            || tile_y + tile_h <= bg_y
                                            || tile_y >= bg_y + bg_h
                                        {
                                            continue;
                                        }
                                        draw_tile(
                                            tile_x,
                                            tile_y,
                                            tile_w,
                                            tile_h,
                                            object_fit,
                                            (0.0, 0.0),
                                        );
                                    }
                                }
                            }
                            true
                        } else {
                            // Image not available: paint this layer's background
                            // color now, in layer order, so a background-color
                            // is still visible when the image fails to load
                            // (mirrors the solid fill the no-image path uses).
                            // The final fallback block below would otherwise be
                            // skipped because this layer reports itself as drawn.
                            if style.background_color.a > 0 {
                                if let Some(ref cp) = css_clip_path {
                                    draw_solid_rect_clipped(
                                        &mut pixmap,
                                        bg_x,
                                        bg_y,
                                        bg_w,
                                        bg_h,
                                        style.background_color,
                                        cp,
                                        transform,
                                        clip_mask.as_ref(),
                                    );
                                } else {
                                    draw_rounded_rect_with_transform(
                                        &mut pixmap,
                                        bg_x,
                                        bg_y,
                                        bg_w,
                                        bg_h,
                                        style.background_color,
                                        style.border_top_left_radius.clone(),
                                        style.border_top_right_radius.clone(),
                                        style.border_bottom_right_radius.clone(),
                                        style.border_bottom_left_radius.clone(),
                                        transform,
                                        clip_mask.as_ref(),
                                    );
                                }
                            }
                            style.background_color.a > 0
                        }
                    }
                    incognidium_style::BackgroundImage::None => false,
                };
                bg_drawn = bg_drawn || drawn;
            }
        }
        if should_mask {
            if let Some(mask_img) = mask_image_ref {
                apply_mask_image_to_region(
                    &mut pixmap,
                    bg_x,
                    bg_y,
                    bg_w,
                    bg_h,
                    mask_img,
                    style.mask_size,
                    style.mask_position,
                    style.mask_repeat,
                );
            }
        }
        // Apply background-blend-mode if set (only for gradients, since solid color is already final)
        if bg_drawn
            && !matches!(
                style.background_blend_mode,
                incognidium_style::BlendMode::Normal
            )
        {
            apply_background_blend_mode(
                &mut pixmap,
                bg_x,
                bg_y,
                bg_w,
                bg_h,
                style.background_blend_mode,
            );
        }

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
                        style.border_top_left_radius.clone(),
                        style.border_top_right_radius.clone(),
                        style.border_bottom_right_radius.clone(),
                        style.border_bottom_left_radius.clone(),
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
        // Skip borders for empty cells with empty-cells: hide
        if !skip_cell_rendering
            && (style.border_top_width > 0.0
                || style.border_right_width > 0.0
                || style.border_bottom_width > 0.0
                || style.border_left_width > 0.0)
        {
            draw_borders_with_transform(&mut pixmap, fbox, &style, transform, clip_mask.as_ref());
        }

        // Draw outline (focus indicator)
        if transformed_clip.is_none()
            && style.outline_width > 0.0
            && style.outline_style != incognidium_style::OutlineStyle::None
        {
            draw_outline(&mut pixmap, fbox, &style, transform);
        }

        // Draw column rules for multi-column layout
        if fbox.box_type == BoxType::Columns
            && fbox.column_count > 1
            && fbox.column_rule_width > 0.0
        {
            draw_column_rules(&mut pixmap, fbox, transform);
        }

        // Draw checkbox/radio buttons (unless appearance: none is set)
        if fbox.box_type == BoxType::InlineBlock {
            if let Some(input_type) = fbox.input_type {
                // Skip default rendering if appearance: none
                let appearance_none = style.appearance == incognidium_style::Appearance::None;
                match input_type {
                    incognidium_layout::InputType::Checkbox { checked } => {
                        if !appearance_none {
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
                    }
                    incognidium_layout::InputType::Radio { checked } => {
                        if !appearance_none {
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
                    }
                    _ => {}
                }
            }
        }

        // Draw the dropdown arrow on <select> controls unless the author has
        // suppressed the native appearance.
        if fbox.box_type == BoxType::Inline || fbox.box_type == BoxType::InlineBlock {
            if let Some(incognidium_layout::InputType::Select) = fbox.input_type {
                if style.appearance != incognidium_style::Appearance::None {
                    draw_select_arrow(&mut pixmap, fbox.x, fbox.y, fbox.width, fbox.height, &style);
                }
            }
        }

        // Draw image (with clip bounds) inside the content box, leaving any
        // border and padding area painted by the border/background passes.
        if fbox.box_type == BoxType::Image {
            if let Some(ref src) = fbox.image_src {
                if let Some(img) = images.get(src) {
                    let img_x = fbox.x + style.border_left_width + style.padding_left;
                    let img_y = fbox.y + style.border_top_width + style.padding_top;
                    let img_w = (fbox.width
                        - style.border_left_width
                        - style.border_right_width
                        - style.padding_left
                        - style.padding_right)
                        .max(0.0);
                    let img_h = (fbox.height
                        - style.border_top_width
                        - style.border_bottom_width
                        - style.padding_top
                        - style.padding_bottom)
                        .max(0.0);
                    draw_image_with_transform_and_clip(
                        &mut pixmap,
                        img_x,
                        img_y,
                        img_w,
                        img_h,
                        img,
                        transformed_clip,
                        fbox.clip_border_radius,
                        transform,
                        style.object_fit,
                        style.object_fit_specified,
                        style.object_position,
                        style.image_rendering,
                        style.border_top_left_radius.clone(),
                        style.border_top_right_radius.clone(),
                        style.border_bottom_right_radius.clone(),
                        style.border_bottom_left_radius.clone(),
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

                    // If this inline text box follows another inline text box on the
                    // same line, the previous glyph may overhang its advance and the
                    // current glyph may overhang to the left, visually collapsing the
                    // inter-word gap. Shift this text right so the visual gap stays
                    // at least one space-width wide. Only do this when layout already
                    // inserted a real inter-word gap; adjacent inline spans (e.g.
                    // per-character spans from JS) have a near-zero layout gap and
                    // must not be forced apart.
                    let mut text_x_offset = 0.0;
                    let layout_gap = fbox.x - last_text_advance_end;
                    let has_word_boundary = last_text_trailing_space || fbox.text_leading_space;
                    if (fbox.y - last_text_y).abs() < 4.0
                        && fbox.x > last_text_advance_end - 5.0
                        && has_word_boundary
                    {
                        let space_width = space_width_for_style(&style);
                        let first_left = first_glyph_left_edge_for_style(&style, &display_text);
                        let visual_gap = fbox.x + first_left - last_text_visual_right;
                        if layout_gap > 0.1
                            && space_width > 0.0
                            && visual_gap < space_width
                            && visual_gap > -50.0
                        {
                            // Layout inserted a real word gap; keep the visual ink
                            // gap at least that wide so glyphs do not collide.
                            text_x_offset = space_width - visual_gap;
                        } else if visual_gap < -0.5 && visual_gap > -50.0 {
                            // A word boundary exists but the nominal gap is zero or
                            // negative (e.g. strong negative word-spacing). Nudge
                            // this run right just enough to avoid overlapping the
                            // previous word's glyph ink, without forcing a gap.
                            text_x_offset = -visual_gap;
                        }
                    }

                    // Check if this text has marker styles (::marker pseudo-element)
                    // Marker styles override the regular text styles
                    let mut effective_style = style.clone();
                    let mut is_marker = false;
                    if let Some(marker_color) = fbox.marker_color {
                        effective_style.color = marker_color;
                        is_marker = true;
                    }
                    if let Some(marker_font_size) = fbox.marker_font_size {
                        effective_style.font_size = marker_font_size;
                        is_marker = true;
                    }
                    if let Some(marker_font_weight) = fbox.marker_font_weight {
                        effective_style.font_weight = marker_font_weight;
                        is_marker = true;
                    }
                    if let Some(ref marker_font_family) = fbox.marker_font_family {
                        effective_style.font_family = marker_font_family.clone();
                        is_marker = true;
                    }
                    // Check if this text has ::first-line styles
                    // If so, apply the first-line styles to the effective_style
                    if fbox.first_line_has_content && !display_text.is_empty() && !is_marker {
                        if let Some(color) = fbox.first_line_color {
                            effective_style.color = color;
                        }
                        if let Some(font_size) = fbox.first_line_font_size {
                            effective_style.font_size = font_size;
                        }
                        if let Some(font_weight) = fbox.first_line_font_weight {
                            effective_style.font_weight = font_weight;
                        }
                        if let Some(ref font_family) = fbox.first_line_font_family {
                            effective_style.font_family = font_family.clone();
                        }
                        if let Some(text_decoration) = fbox.first_line_text_decoration {
                            effective_style.text_decoration = text_decoration;
                            effective_style.text_decoration_line = match text_decoration {
                                TextDecoration::None => TextDecorationLine::None,
                                TextDecoration::Underline => TextDecorationLine::Underline,
                                TextDecoration::LineThrough => TextDecorationLine::LineThrough,
                            };
                        }
                        if let Some(letter_spacing) = fbox.first_line_letter_spacing {
                            effective_style.letter_spacing = letter_spacing;
                        }
                        if let Some(word_spacing) = fbox.first_line_word_spacing {
                            effective_style.word_spacing = word_spacing;
                        }
                        if let Some(text_transform) = fbox.first_line_text_transform {
                            effective_style.text_transform = text_transform;
                        }
                        // Note: first_line_background_color is handled separately
                    }

                    // Check if this text has ::first-letter styles
                    // If so, we need to render the first letter separately
                    if fbox.first_letter_len.is_some() && !display_text.is_empty() && !is_marker {
                        // Find the first letter (skipping any leading punctuation/spaces)
                        let first_char_idx = display_text
                            .char_indices()
                            .find(|(_, c)| c.is_alphabetic() || c.is_numeric())
                            .map(|(i, _)| i)
                            .unwrap_or(0);
                        let first_char_end = display_text
                            .char_indices()
                            .skip_while(|(i, _)| *i < first_char_idx)
                            .nth(fbox.first_letter_len.unwrap_or(1))
                            .map(|(i, _)| i)
                            .unwrap_or(display_text.len());

                        let first_letter = &display_text[first_char_idx..first_char_end];
                        let rest_text = &display_text[first_char_end..];

                        // Create style for first letter
                        let mut first_letter_style = effective_style.clone();
                        if let Some(color) = fbox.first_letter_color {
                            first_letter_style.color = color;
                        }
                        if let Some(font_size) = fbox.first_letter_font_size {
                            first_letter_style.font_size = font_size;
                        }
                        if let Some(font_weight) = fbox.first_letter_font_weight {
                            first_letter_style.font_weight = font_weight;
                        }
                        if let Some(ref font_family) = fbox.first_letter_font_family {
                            first_letter_style.font_family = font_family.clone();
                        }
                        if let Some(text_decoration) = fbox.first_letter_text_decoration {
                            first_letter_style.text_decoration = text_decoration;
                            first_letter_style.text_decoration_line = match text_decoration {
                                TextDecoration::None => TextDecorationLine::None,
                                TextDecoration::Underline => TextDecorationLine::Underline,
                                TextDecoration::LineThrough => TextDecorationLine::LineThrough,
                            };
                        }
                        if let Some(text_transform) = fbox.first_letter_text_transform {
                            first_letter_style.text_transform = text_transform;
                        }

                        // Apply the first-letter text transform (e.g. uppercase) to
                        // the sliced first character, since draw_text uses the text
                        // as-is and does not re-run apply_text_transform.
                        let first_letter = apply_text_transform(first_letter, &first_letter_style);

                        // Calculate first letter width using the actual font metrics so
                        // the first letter is not clipped when its real glyph is wider than
                        // the rough estimate.
                        let first_letter_width = measure_text_width(
                            &first_letter,
                            first_letter_style.font_size,
                            &first_letter_style,
                        );

                        // Draw background for first letter if specified
                        if let Some(bg_color) = fbox.first_letter_background_color {
                            if bg_color.a > 0 {
                                let margin_top =
                                    fbox.first_letter_margin.map(|m| m.0).unwrap_or(0.0);
                                let margin_right =
                                    fbox.first_letter_margin.map(|m| m.1).unwrap_or(0.0);
                                let margin_bottom =
                                    fbox.first_letter_margin.map(|m| m.2).unwrap_or(0.0);
                                let margin_left =
                                    fbox.first_letter_margin.map(|m| m.3).unwrap_or(0.0);
                                let padding_top =
                                    fbox.first_letter_padding.map(|p| p.0).unwrap_or(0.0);
                                let padding_right =
                                    fbox.first_letter_padding.map(|p| p.1).unwrap_or(0.0);
                                let padding_bottom =
                                    fbox.first_letter_padding.map(|p| p.2).unwrap_or(0.0);
                                let padding_left =
                                    fbox.first_letter_padding.map(|p| p.3).unwrap_or(0.0);

                                let bg_x = fbox.x + margin_left;
                                let bg_y = fbox.y + margin_top;
                                let bg_w = first_letter_width + padding_left + padding_right;
                                let bg_h =
                                    first_letter_style.font_size + padding_top + padding_bottom;

                                draw_rect(&mut pixmap, bg_x, bg_y, bg_w, bg_h, bg_color);
                            }
                        }

                        // Align the first-letter glyph's baseline with the
                        // surrounding text's baseline. The text box's y is the
                        // top of the line box; the parent text baseline sits at
                        // that top plus the parent font's ascent. A larger (or
                        // smaller) first-letter glyph must share that baseline
                        // so it does not hang below the line or float above it.
                        let parent_baseline_y = baseline_y_for_style(fbox.y, &effective_style);
                        let first_letter_baseline_y =
                            baseline_y_for_style(fbox.y, &first_letter_style);
                        let first_letter_y = fbox.y + (parent_baseline_y - first_letter_baseline_y);

                        // The parent text box's height was computed with the
                        // ordinary line height, so a large first-letter glyph
                        // would be clipped by the smaller max_height. Pass 0 to
                        // disable vertical clipping for the single first-letter
                        // glyph so it is always rendered.
                        let first_letter_visual_right = draw_text_with_transform(
                            &mut pixmap,
                            fbox.x,
                            first_letter_y,
                            first_letter_width + 1.0, // +1 to ensure it fits
                            0.0,
                            &first_letter,
                            &first_letter_style,
                            transformed_clip,
                            transform,
                            clip_mask.as_ref(),
                            None,
                            text_x_offset,
                        );

                        // Draw rest of text
                        let mut rest_visual_right = first_letter_visual_right;
                        if !rest_text.is_empty() {
                            rest_visual_right = draw_text_with_transform(
                                &mut pixmap,
                                fbox.x + first_letter_width,
                                fbox.y,
                                (fbox.width - first_letter_width).max(0.0),
                                fbox.height,
                                rest_text,
                                &effective_style,
                                transformed_clip,
                                transform,
                                clip_mask.as_ref(),
                                None,
                                text_x_offset,
                            );
                        }
                        last_text_visual_right = first_letter_visual_right.max(rest_visual_right);
                        last_text_advance_end = fbox.x + fbox.width;
                        last_text_y = fbox.y;
                        last_text_trailing_space = fbox.text_trailing_space;
                    } else {
                        let text_visual_right = draw_text_with_transform(
                            &mut pixmap,
                            fbox.x,
                            fbox.y,
                            fbox.width,
                            fbox.height,
                            &display_text,
                            &effective_style,
                            transformed_clip,
                            transform,
                            clip_mask.as_ref(),
                            Some(&fbox.text_align_offsets),
                            text_x_offset,
                        );
                        last_text_visual_right = text_visual_right;
                        last_text_advance_end = fbox.x + fbox.width;
                        last_text_y = fbox.y;
                        last_text_trailing_space = fbox.text_trailing_space;
                    }
                }
            }
        } else if fbox.box_type == BoxType::Inline {
            // Update only the trailing-whitespace state for inline containers.
            // Their text children already updated the visual/advance metrics, and
            // overwriting those with the container's content edge would discard
            // glyph overhang needed by the next sibling's gap-preservation logic.
            last_text_trailing_space = fbox.text_trailing_space;
        }

        // Draw text for InlineBlock form controls (textarea, input[type="text"])
        if fbox.box_type == BoxType::InlineBlock {
            if let Some(ref text) = fbox.text {
                if !text.is_empty() && text != " " {
                    // For textarea/input, draw text with padding offset
                    let padding_left = style.padding_left;
                    let padding_top = style.padding_top;
                    // Placeholder pseudo-element styles override the input's own
                    // color, font-size, font-weight and font-style.
                    let mut effective_style = style.clone();
                    if fbox.is_placeholder_text {
                        if let Some(c) = style.placeholder_color {
                            effective_style.color = c;
                        }
                        if let Some(fs) = style.placeholder_font_size {
                            effective_style.font_size = fs;
                        }
                        if let Some(fw) = style.placeholder_font_weight {
                            effective_style.font_weight = fw;
                        }
                        if let Some(fst) = style.placeholder_font_style {
                            effective_style.font_style = fst;
                        }
                    }
                    let display_text = apply_text_transform(text, &effective_style);
                    let text_width = measure_text_width(
                        &display_text,
                        effective_style.font_size,
                        &effective_style,
                    );
                    let text_max_width = fbox.width - padding_left - style.padding_right;
                    let x_offset = if display_text.contains('\n') {
                        0.0
                    } else {
                        match effective_style.text_align {
                            incognidium_style::TextAlign::Center => {
                                ((text_max_width - text_width) / 2.0).max(0.0)
                            }
                            incognidium_style::TextAlign::Right => {
                                (text_max_width - text_width).max(0.0)
                            }
                            _ => 0.0,
                        }
                    };
                    let ib_visual_right = draw_text_with_transform(
                        &mut pixmap,
                        fbox.x + padding_left,
                        fbox.y + padding_top,
                        text_max_width,
                        fbox.height - padding_top - style.padding_bottom,
                        &display_text,
                        &effective_style,
                        transformed_clip,
                        transform,
                        clip_mask.as_ref(),
                        None,
                        x_offset,
                    );
                    last_text_visual_right = ib_visual_right.max(fbox.x + fbox.width);
                    last_text_advance_end = fbox.x + fbox.width;
                    last_text_y = fbox.y;
                    last_text_trailing_space = fbox.text_trailing_space;
                }
            }
        }

        // Only block-level boxes and hard line breaks break the chain of adjacent
        // inline text siblings. Inline containers (e.g. <a>, <span>) and inline
        // blocks sit on the same line, so the next text box can still be shifted
        // relative to the previous text box's visual right edge.
        if !matches!(
            fbox.box_type,
            BoxType::Text | BoxType::Inline | BoxType::InlineBlock
        ) {
            last_text_y = f32::NEG_INFINITY;
            last_text_trailing_space = false;
        }

        // Draw resize handle if resize property is set (and not none).  Skip
        // when the box has overflow:hidden and is smaller than the handle so
        // the grip does not cover the box content.
        if style.resize != incognidium_style::Resize::None {
            draw_resize_handle(&mut pixmap, fbox.x, fbox.y, fbox.width, fbox.height, &style);
        }

        // Apply mix-blend-mode to blend the element with the background beneath it.
        // This must be done after all element content (background, borders, text) is drawn.
        if !matches!(
            style.mix_blend_mode,
            incognidium_style::MixBlendMode::Normal
        ) {
            // Skip if isolation: isolate is set.
            if style.isolation != incognidium_style::Isolation::Isolate {
                apply_mix_blend_mode(
                    &mut pixmap,
                    draw_x,
                    draw_y,
                    draw_w,
                    draw_h,
                    style.mix_blend_mode,
                    mix_backdrop.as_deref(),
                );
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
        TextTransform::FullWidth => {
            // Convert ASCII characters to their full-width variants
            // Full-width forms are at U+FF01-U+FF5E for ASCII !-~
            text.chars()
                .map(|c| {
                    if (0x21..=0x7E).contains(&(c as u32)) {
                        // Convert ASCII to full-width: U+FF01 - U+FF5E maps to 0x21 - 0x7E
                        char::from_u32(c as u32 + 0xFEE0).unwrap_or(c)
                    } else if c == ' ' {
                        // Space becomes full-width space (U+3000)
                        '\u{3000}'
                    } else {
                        c
                    }
                })
                .collect()
        }
        TextTransform::FullSizeKana => {
            // Convert small kana to full-size kana
            text.chars()
                .map(|c| {
                    match c {
                        // Small hiragana to full-size
                        'ぁ' => 'あ',
                        'ぃ' => 'い',
                        'ぅ' => 'う',
                        'ぇ' => 'え',
                        'ぉ' => 'お',
                        'っ' => 'つ',
                        'ゃ' => 'や',
                        'ゅ' => 'ゆ',
                        'ょ' => 'よ',
                        'ゎ' => 'わ',
                        // Small katakana to full-size
                        'ァ' => 'ア',
                        'ィ' => 'イ',
                        'ゥ' => 'ウ',
                        'ェ' => 'エ',
                        'ォ' => 'オ',
                        'ッ' => 'ツ',
                        'ャ' => 'ヤ',
                        'ュ' => 'ユ',
                        'ョ' => 'ヨ',
                        'ヮ' => 'ワ',
                        _ => c,
                    }
                })
                .collect()
        }
        TextTransform::MathAuto => {
            // CSS Text Level 4 math-auto: apply math variant styling
            // This would use font features to display math variables properly
            // For now, just pass through unchanged
            text.to_string()
        }
    }
}

fn css_to_skia_color(c: CssColor) -> Color {
    Color::from_rgba8(c.r, c.g, c.b, c.a)
}

/// Clear a rectangular region of `pixmap` to fully transparent black.
fn clear_pixmap_region(pixmap: &mut Pixmap, x: u32, y: u32, width: u32, height: u32) {
    let pm_width = pixmap.width();
    let pm_height = pixmap.height();
    let data = pixmap.data_mut();
    let x1 = (x + width).min(pm_width);
    let y1 = (y + height).min(pm_height);
    for py in y..y1 {
        for px in x..x1 {
            let idx = ((py * pm_width + px) * 4) as usize;
            if idx + 3 < data.len() {
                data[idx] = 0;
                data[idx + 1] = 0;
                data[idx + 2] = 0;
                data[idx + 3] = 0;
            }
        }
    }
}

/// Apply mix-blend-mode to blend the element's isolated source group with the
/// backdrop beneath it.
///
/// After the element was painted, `pixmap` contains the source group on a
/// transparent background (the region was cleared before drawing). `backdrop`
/// holds the pixels that were underneath the element before it was drawn.
fn apply_mix_blend_mode(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    blend_mode: incognidium_style::MixBlendMode,
    backdrop: Option<&[u8]>,
) {
    use incognidium_style::MixBlendMode;

    if matches!(blend_mode, MixBlendMode::Normal) {
        return;
    }

    let Some(backdrop) = backdrop else {
        return;
    };

    let x0 = x.max(0.0) as u32;
    let y0 = y.max(0.0) as u32;
    let x1 = ((x + width).min(pixmap.width() as f32)) as u32;
    let y1 = ((y + height).min(pixmap.height() as f32)) as u32;

    if x0 >= x1 || y0 >= y1 {
        return;
    }

    let pm_width = pixmap.width();
    let data = pixmap.data_mut();
    let backdrop_width = x1 - x0;

    for py in y0..y1 {
        for px in x0..x1 {
            let idx = ((py * pm_width + px) * 4) as usize;
            if idx + 3 >= data.len() {
                continue;
            }

            // Source group: the element's own content painted onto a transparent
            // canvas. Pixmap stores premultiplied RGBA, so unpremultiply before
            // applying the blend function and composite the result as straight
            // alpha, then premultiply again for storage.
            let src_a = data[idx + 3] as f32 / 255.0;
            let src_r = if src_a > 1e-4 {
                (data[idx] as f32 / 255.0) / src_a
            } else {
                0.0
            };
            let src_g = if src_a > 1e-4 {
                (data[idx + 1] as f32 / 255.0) / src_a
            } else {
                0.0
            };
            let src_b = if src_a > 1e-4 {
                (data[idx + 2] as f32 / 255.0) / src_a
            } else {
                0.0
            };

            let bx = px - x0;
            let by = py - y0;
            let bidx = ((by * backdrop_width + bx) * 4) as usize;
            if bidx + 3 >= backdrop.len() {
                continue;
            }
            let back_a = backdrop[bidx + 3] as f32 / 255.0;
            let back_r = if back_a > 1e-4 {
                (backdrop[bidx] as f32 / 255.0) / back_a
            } else {
                0.0
            };
            let back_g = if back_a > 1e-4 {
                (backdrop[bidx + 1] as f32 / 255.0) / back_a
            } else {
                0.0
            };
            let back_b = if back_a > 1e-4 {
                (backdrop[bidx + 2] as f32 / 255.0) / back_a
            } else {
                0.0
            };

            if src_a < 1e-4 {
                // No source contribution; restore the original backdrop pixel.
                data[idx] = backdrop[bidx];
                data[idx + 1] = backdrop[bidx + 1];
                data[idx + 2] = backdrop[bidx + 2];
                data[idx + 3] = backdrop[bidx + 3];
                continue;
            }

            // Apply the CSS compositing blend mode to source and backdrop colors.
            let (blend_r, blend_g, blend_b) = match blend_mode {
                MixBlendMode::Multiply => (src_r * back_r, src_g * back_g, src_b * back_b),
                MixBlendMode::Screen => (
                    1.0 - (1.0 - src_r) * (1.0 - back_r),
                    1.0 - (1.0 - src_g) * (1.0 - back_g),
                    1.0 - (1.0 - src_b) * (1.0 - back_b),
                ),
                MixBlendMode::Overlay => {
                    let overlay = |s: f32, b: f32| {
                        if b < 0.5 {
                            2.0 * s * b
                        } else {
                            1.0 - 2.0 * (1.0 - s) * (1.0 - b)
                        }
                    };
                    (
                        overlay(src_r, back_r).min(1.0),
                        overlay(src_g, back_g).min(1.0),
                        overlay(src_b, back_b).min(1.0),
                    )
                }
                MixBlendMode::Darken => (src_r.min(back_r), src_g.min(back_g), src_b.min(back_b)),
                MixBlendMode::Lighten => (src_r.max(back_r), src_g.max(back_g), src_b.max(back_b)),
                MixBlendMode::ColorDodge => {
                    let dodge = |s: f32, b: f32| {
                        if b >= 1.0 {
                            1.0
                        } else if s <= 0.0 {
                            0.0
                        } else {
                            (b / (1.0 - s)).min(1.0)
                        }
                    };
                    (
                        dodge(src_r, back_r),
                        dodge(src_g, back_g),
                        dodge(src_b, back_b),
                    )
                }
                MixBlendMode::ColorBurn => {
                    let burn = |s: f32, b: f32| {
                        if b <= 0.0 {
                            0.0
                        } else if s >= 1.0 {
                            1.0
                        } else {
                            1.0 - ((1.0 - b) / s).min(1.0)
                        }
                    };
                    (
                        burn(src_r, back_r),
                        burn(src_g, back_g),
                        burn(src_b, back_b),
                    )
                }
                MixBlendMode::HardLight => {
                    let hard_light = |s: f32, b: f32| {
                        if s < 0.5 {
                            2.0 * s * b
                        } else {
                            1.0 - 2.0 * (1.0 - s) * (1.0 - b)
                        }
                    };
                    (
                        hard_light(src_r, back_r).min(1.0),
                        hard_light(src_g, back_g).min(1.0),
                        hard_light(src_b, back_b).min(1.0),
                    )
                }
                MixBlendMode::SoftLight => {
                    let soft_light = |s: f32, b: f32| {
                        if b < 0.5 {
                            2.0 * b * s + b * b * (1.0 - 2.0 * s)
                        } else {
                            2.0 * s * (1.0 - b) + b.sqrt() * (2.0 * s - 1.0)
                        }
                    };
                    (
                        soft_light(src_r, back_r).min(1.0).max(0.0),
                        soft_light(src_g, back_g).min(1.0).max(0.0),
                        soft_light(src_b, back_b).min(1.0).max(0.0),
                    )
                }
                MixBlendMode::Difference => (
                    (src_r - back_r).abs(),
                    (src_g - back_g).abs(),
                    (src_b - back_b).abs(),
                ),
                MixBlendMode::Exclusion => (
                    (src_r + back_r - 2.0 * src_r * back_r).min(1.0),
                    (src_g + back_g - 2.0 * src_g * back_g).min(1.0),
                    (src_b + back_b - 2.0 * src_b * back_b).min(1.0),
                ),
                MixBlendMode::Hue => {
                    let (src_h, src_s, _) = rgb_to_hsl(src_r, src_g, src_b);
                    let (_, _, back_l) = rgb_to_hsl(back_r, back_g, back_b);
                    hsl_to_rgb(src_h, src_s, back_l)
                }
                MixBlendMode::Saturation => {
                    let (_, src_s, _) = rgb_to_hsl(src_r, src_g, src_b);
                    let (back_h, _, back_l) = rgb_to_hsl(back_r, back_g, back_b);
                    hsl_to_rgb(back_h, src_s, back_l)
                }
                MixBlendMode::Color => {
                    let (src_h, src_s, _) = rgb_to_hsl(src_r, src_g, src_b);
                    let (_, _, back_l) = rgb_to_hsl(back_r, back_g, back_b);
                    hsl_to_rgb(src_h, src_s, back_l)
                }
                MixBlendMode::Luminosity => {
                    let (_, _, src_l) = rgb_to_hsl(src_r, src_g, src_b);
                    let (back_h, back_s, _) = rgb_to_hsl(back_r, back_g, back_b);
                    hsl_to_rgb(back_h, back_s, src_l)
                }
                _ => (src_r, src_g, src_b),
            };

            // Composite the blended source group over the captured backdrop using
            // source-over, then premultiply the final color for Pixmap storage.
            let out_a = src_a + back_a - src_a * back_a;
            if out_a < 1e-4 {
                data[idx] = backdrop[bidx];
                data[idx + 1] = backdrop[bidx + 1];
                data[idx + 2] = backdrop[bidx + 2];
                data[idx + 3] = backdrop[bidx + 3];
                continue;
            }
            let out_r = (src_a * blend_r + back_a * (1.0 - src_a) * back_r) / out_a;
            let out_g = (src_a * blend_g + back_a * (1.0 - src_a) * back_g) / out_a;
            let out_b = (src_a * blend_b + back_a * (1.0 - src_a) * back_b) / out_a;

            data[idx] = (out_r * out_a * 255.0) as u8;
            data[idx + 1] = (out_g * out_a * 255.0) as u8;
            data[idx + 2] = (out_b * out_a * 255.0) as u8;
            data[idx + 3] = (out_a * 255.0) as u8;
        }
    }
}

/// Apply background-blend-mode to blend the drawn background with existing content
fn apply_background_blend_mode(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    blend_mode: incognidium_style::BlendMode,
) {
    use incognidium_style::BlendMode;

    if matches!(blend_mode, BlendMode::Normal) {
        return;
    }

    let x0 = x.max(0.0) as u32;
    let y0 = y.max(0.0) as u32;
    let x1 = ((x + width).min(pixmap.width() as f32)) as u32;
    let y1 = ((y + height).min(pixmap.height() as f32)) as u32;

    if x0 >= x1 || y0 >= y1 {
        return;
    }

    let pm_width = pixmap.width();
    let data = pixmap.data_mut();

    for py in y0..y1 {
        for px in x0..x1 {
            let idx = ((py * pm_width + px) * 4) as usize;
            if idx + 3 >= data.len() {
                continue;
            }

            // Source (what was just drawn - the background)
            let src_r = data[idx] as f32 / 255.0;
            let src_g = data[idx + 1] as f32 / 255.0;
            let src_b = data[idx + 2] as f32 / 255.0;
            let src_a = data[idx + 3] as f32 / 255.0;

            if src_a == 0.0 {
                continue;
            }

            // Premultiplied alpha blending is complex; for now use simple blending
            // on the RGB channels assuming the background was drawn
            let (r, g, b) = match blend_mode {
                BlendMode::Multiply => {
                    // Multiply: source * destination
                    // For simplicity, we just darken the color
                    (src_r * 0.8, src_g * 0.8, src_b * 0.8)
                }
                BlendMode::Screen => {
                    // Screen: 1 - (1-source)*(1-dest)
                    let r = 1.0 - (1.0 - src_r) * 0.5;
                    let g = 1.0 - (1.0 - src_g) * 0.5;
                    let b = 1.0 - (1.0 - src_b) * 0.5;
                    (r.min(1.0), g.min(1.0), b.min(1.0))
                }
                BlendMode::Overlay => {
                    // Overlay: multiply for dark, screen for light
                    let apply = |s: f32| {
                        if s < 0.5 {
                            2.0 * s * s
                        } else {
                            1.0 - 2.0 * (1.0 - s) * (1.0 - s)
                        }
                    };
                    (apply(src_r), apply(src_g), apply(src_b))
                }
                BlendMode::Darken => (src_r * 0.7, src_g * 0.7, src_b * 0.7),
                BlendMode::Lighten => {
                    let r = src_r * 1.3;
                    let g = src_g * 1.3;
                    let b = src_b * 1.3;
                    (r.min(1.0), g.min(1.0), b.min(1.0))
                }
                BlendMode::HardLight => {
                    let apply = |s: f32| {
                        if s < 0.5 {
                            2.0 * s
                        } else {
                            2.0 * (1.0 - s)
                        }
                    };
                    (
                        apply(src_r).min(1.0),
                        apply(src_g).min(1.0),
                        apply(src_b).min(1.0),
                    )
                }
                BlendMode::SoftLight => {
                    let apply = |s: f32| s - (s * s * (s - 1.0)).min(1.0);
                    (apply(src_r), apply(src_g), apply(src_b))
                }
                BlendMode::Difference => (src_r * 0.5, src_g * 0.5, src_b * 0.5),
                BlendMode::Exclusion => {
                    let r = src_r + 0.5 - 2.0 * src_r * 0.5;
                    let g = src_g + 0.5 - 2.0 * src_g * 0.5;
                    let b = src_b + 0.5 - 2.0 * src_b * 0.5;
                    (r.min(1.0), g.min(1.0), b.min(1.0))
                }
                _ => (src_r, src_g, src_b),
            };

            data[idx] = (r * 255.0) as u8;
            data[idx + 1] = (g * 255.0) as u8;
            data[idx + 2] = (b * 255.0) as u8;
            // Preserve alpha
        }
    }
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
    clip_mask: Option<&ClipMask>,
) {
    use incognidium_style::GradientDirection;
    use tiny_skia::{GradientStop, LinearGradient as SkiaLinearGradient, Point, SpreadMode};

    if width <= 0.0 || height <= 0.0 {
        return;
    }

    let spread_mode = if gradient.repeating {
        SpreadMode::Repeat
    } else {
        SpreadMode::Pad
    };

    // For repeating gradients the gradient line length is the repeat period in pixels;
    // for non-repeating gradients it spans the whole positioning area.
    let period = gradient.repeat_length.unwrap_or(-1.0);

    // Calculate gradient line based on direction
    let (x1, y1, x2, y2) = match gradient.direction {
        GradientDirection::ToTop => {
            let len = if period > 0.0 { period } else { height };
            let sx = x + width / 2.0;
            (sx, y + height, sx, y + height - len)
        }
        GradientDirection::ToBottom => {
            let len = if period > 0.0 { period } else { height };
            let sx = x + width / 2.0;
            (sx, y, sx, y + len)
        }
        GradientDirection::ToLeft => {
            let len = if period > 0.0 { period } else { width };
            let sy = y + height / 2.0;
            (x + width, sy, x + width - len, sy)
        }
        GradientDirection::ToRight => {
            let len = if period > 0.0 { period } else { width };
            let sy = y + height / 2.0;
            (x, sy, x + len, sy)
        }
        GradientDirection::ToTopLeft => {
            let full = (width * width + height * height).sqrt();
            let len = if period > 0.0 { period } else { full };
            let inv_sqrt2 = 1.0 / 2.0_f32.sqrt();
            (
                x + width,
                y + height,
                x + width - len * inv_sqrt2,
                y + height - len * inv_sqrt2,
            )
        }
        GradientDirection::ToTopRight => {
            let full = (width * width + height * height).sqrt();
            let len = if period > 0.0 { period } else { full };
            let inv_sqrt2 = 1.0 / 2.0_f32.sqrt();
            (
                x,
                y + height,
                x + len * inv_sqrt2,
                y + height - len * inv_sqrt2,
            )
        }
        GradientDirection::ToBottomLeft => {
            let full = (width * width + height * height).sqrt();
            let len = if period > 0.0 { period } else { full };
            let inv_sqrt2 = 1.0 / 2.0_f32.sqrt();
            (
                x + width,
                y,
                x + width - len * inv_sqrt2,
                y + len * inv_sqrt2,
            )
        }
        GradientDirection::ToBottomRight => {
            let full = (width * width + height * height).sqrt();
            let len = if period > 0.0 { period } else { full };
            let inv_sqrt2 = 1.0 / 2.0_f32.sqrt();
            (x, y, x + len * inv_sqrt2, y + len * inv_sqrt2)
        }
        GradientDirection::Angle(deg) => {
            let rad = deg.to_radians();
            let cx = x + width / 2.0;
            let cy = y + height / 2.0;
            let full = (width * width + height * height).sqrt();
            let len = if period > 0.0 { period } else { full };
            let half_len = len / 2.0;
            let dx = rad.sin() * half_len;
            let dy = -rad.cos() * half_len; // Negative because y increases downward
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

    // Create the gradient
    let skia_grad = match SkiaLinearGradient::new(
        Point::from_xy(x1, y1),
        Point::from_xy(x2, y2),
        stops,
        spread_mode,
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
    apply_clip_mask_to_region(pixmap, x, y, width, height, clip_mask);
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
    clip_mask: Option<&ClipMask>,
) {
    use tiny_skia::{GradientStop, Point, RadialGradient as SkiaRadialGradient, SpreadMode};

    if width <= 0.0 || height <= 0.0 {
        return;
    }

    // Calculate center point based on position percentage
    let cx = x + width * (gradient.position.0 / 100.0);
    let cy = y + height * (gradient.position.1 / 100.0);

    // Calculate radius based on size keyword
    // For repeating gradients the period is the repeat length; otherwise cover the box.
    let radius = if gradient.repeating {
        gradient
            .repeat_length
            .unwrap_or_else(|| width.max(height) / 2.0)
    } else {
        width.max(height) / 2.0
    };

    let spread_mode = if gradient.repeating {
        SpreadMode::Repeat
    } else {
        SpreadMode::Pad
    };

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
        spread_mode,
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
    apply_clip_mask_to_region(pixmap, x, y, width, height, clip_mask);
}

fn draw_rect(pixmap: &mut Pixmap, x: f32, y: f32, width: f32, height: f32, color: CssColor) {
    draw_rounded_rect(
        pixmap,
        x,
        y,
        width,
        height,
        color,
        SizeValue::Px(0.0),
        SizeValue::Px(0.0),
        SizeValue::Px(0.0),
        SizeValue::Px(0.0),
    );
}

fn draw_rect_with_transform(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    color: CssColor,
    transform: Transform,
    clip_mask: Option<&ClipMask>,
) {
    draw_rounded_rect_with_transform(
        pixmap,
        x,
        y,
        width,
        height,
        color,
        SizeValue::Px(0.0),
        SizeValue::Px(0.0),
        SizeValue::Px(0.0),
        SizeValue::Px(0.0),
        transform,
        clip_mask,
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

/// Draw a single solid text-decoration stroke without anti-aliasing so hairline
/// underlines (1 px) remain crisp and visible instead of fading into subpixels.
fn draw_solid_decoration_line(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    thickness: f32,
    color: CssColor,
) {
    if width <= 0.0 || thickness <= 0.0 {
        return;
    }
    let y = y.round();
    let h = thickness.max(1.0);
    let rect = match Rect::from_xywh(x, y, width.max(1.0), h) {
        Some(r) => r,
        None => return,
    };
    let mut paint = Paint::default();
    paint.set_color(Color::from_rgba8(color.r, color.g, color.b, color.a));
    paint.anti_alias = false;
    let path = PathBuilder::from_rect(rect);
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
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
            draw_solid_decoration_line(pixmap, x, y, width, thickness, color);
        }
        TextDecorationStyle::Double => {
            // Two parallel lines separated by a gap equal to the stroke thickness
            // so the double underline is visible instead of merging into one line.
            draw_solid_decoration_line(pixmap, x, y, width, thickness, color);
            draw_solid_decoration_line(pixmap, x, y + thickness * 2.0, width, thickness, color);
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

/// Draw one or more text-decoration segments as a single filled path so that
/// adjacent segments share crisp edges and do not reveal the background between
/// them. Solid and double lines are combined per-line; patterns fall back to
/// per-segment rendering.
fn draw_text_decoration_segments(
    pixmap: &mut Pixmap,
    y: f32,
    thickness: f32,
    color: CssColor,
    style: incognidium_style::TextDecorationStyle,
    segments: &[(f32, f32)],
) {
    use incognidium_style::TextDecorationStyle;

    if segments.is_empty() {
        return;
    }

    match style {
        TextDecorationStyle::Solid | TextDecorationStyle::Double => {
            // Draw each continuous run as a non-antialiased solid/double bar so
            // hairline underlines stay crisp and do not fade into the background.
            for (x, width) in segments {
                if *width > 0.0 {
                    draw_text_decoration_line(pixmap, *x, y, *width, thickness, color, style);
                }
            }
        }
        _ => {
            for (x, width) in segments {
                draw_text_decoration_line(pixmap, *x, y, *width, thickness, color, style);
            }
        }
    }
}

/// Draw underline with skip-ink support (skips over descenders like g, j, p, q, y).
/// Adjacent non-descender words are merged into a single continuous underline, and
/// the underline is extended across inter-word spaces so word boundaries do not
/// look like breaks.
fn draw_underline_with_skip_ink(
    pixmap: &mut Pixmap,
    underline_y: f32,
    thickness: f32,
    color: CssColor,
    decor_style: incognidium_style::TextDecorationStyle,
    word_positions: &[(f32, f32, f32, bool)], // (y, x, width, has_descenders)
    font_size: f32,
    space_width: f32,
    font: &FontdueFont,
) {
    // Group positions by line (y coordinate)
    let mut lines: Vec<(f32, Vec<(f32, f32, bool)>)> = Vec::new();

    for (word_y, word_x, word_w, has_descenders) in word_positions.iter() {
        let epsilon = 0.5; // Tolerance for floating point comparison
        let found = lines.iter().position(|(y, _)| (y - word_y).abs() < epsilon);
        match found {
            Some(idx) => lines[idx].1.push((*word_x, *word_w, *has_descenders)),
            None => lines.push((*word_y, vec![(*word_x, *word_w, *has_descenders)])),
        }
    }

    for (line_y, mut words) in lines {
        let line_ascent = font_due_ascent(font, font_size);
        let expected_ul_y = line_y + line_ascent + 2.0;

        if (expected_ul_y - underline_y).abs() > font_size {
            continue; // Skip lines not on this underline
        }

        words.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        // Build the span that each word occupies, including the following
        // inter-word space so the underline stays continuous across spaces.
        let mut spans: Vec<(f32, f32, bool)> = Vec::with_capacity(words.len());
        for (i, (word_x, word_w, has_descenders)) in words.iter().enumerate() {
            if *word_w <= 0.0 {
                continue;
            }
            let span_end = if i + 1 < words.len() {
                words[i + 1].0
            } else {
                word_x + word_w
            };
            spans.push((*word_x, span_end, *has_descenders));
        }

        let mut run_start: Option<f32> = None;
        let mut segments: Vec<(f32, f32)> = Vec::new();

        for (i, (span_start, span_end, has_descenders)) in spans.iter().enumerate() {
            let word_w = spans[i].1 - spans[i].0
                + if i + 1 < spans.len() {
                    space_width
                } else {
                    0.0
                };
            // The actual word width without the trailing space.
            let trailing_space = if i + 1 < spans.len() {
                space_width
            } else {
                0.0
            };
            let actual_word_w = word_w - trailing_space;

            if *has_descenders {
                // Close any open run before the descender word.
                if let Some(start) = run_start {
                    if start < *span_start {
                        segments.push((start, span_start - start));
                    }
                }

                // Skip roughly the middle 60% of the word, drawing small lead
                // and trail segments. The trail is extended to the end of the
                // span so the underline continues across the following space.
                let skip_portion = 0.6;
                let lead_width = actual_word_w * (1.0 - skip_portion) / 2.0;
                let skip_width = actual_word_w * skip_portion;

                if lead_width > 0.0 {
                    segments.push((*span_start, lead_width));
                }
                let trail_start = span_start + lead_width + skip_width;
                if *span_end > trail_start {
                    segments.push((trail_start, span_end - trail_start));
                }

                // Resume the underline after the descender word (and its space).
                run_start = Some(*span_end);
            } else {
                // Extend the current continuous run to include this word and the
                // following inter-word space.
                if run_start.is_none() {
                    run_start = Some(*span_start);
                }
            }
        }

        // Draw any underline run that reaches the last span.
        if let Some(start) = run_start {
            let last_end = spans.last().map(|s| s.1).unwrap_or(start);
            if last_end > start {
                segments.push((start, last_end - start));
            }
        }

        if !segments.is_empty() {
            draw_text_decoration_segments(
                pixmap,
                underline_y,
                thickness,
                color,
                decor_style,
                &segments,
            );
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
        None,
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
    clip_mask: Option<&ClipMask>,
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
        // fill_path with transform
        pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
        apply_clip_mask_to_region(pixmap, x, y, width, height, clip_mask);
        return;
    }

    let path = build_rounded_rect_path(x, y, width, height, rtl, rtr, rbr, rbl);
    let mut paint = Paint::default();
    paint.set_color(css_to_skia_color(color));
    paint.anti_alias = true;
    pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
    apply_clip_mask_to_region(pixmap, x, y, width, height, clip_mask);
}

// kappa is the distance from the endpoint to the control point for a quarter circle
// kappa = (4/3) * tan(pi/8) ≈ 0.5522847498
const ROUNDED_RECT_KAPPA: f32 = 0.5522847498;

/// Push a clockwise rounded rectangle subpath onto an existing PathBuilder.
fn push_rounded_rect_cw(
    pb: &mut PathBuilder,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    rtl: f32,
    rtr: f32,
    rbr: f32,
    rbl: f32,
) {
    // Top edge: start after top-left radius
    pb.move_to(x + rtl, y);
    // Top edge to top-right radius start
    pb.line_to(x + width - rtr, y);
    // Top-right corner curve
    if rtr > 0.0 {
        let k = rtr * ROUNDED_RECT_KAPPA;
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
        let k = rbr * ROUNDED_RECT_KAPPA;
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
        let k = rbl * ROUNDED_RECT_KAPPA;
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
        let k = rtl * ROUNDED_RECT_KAPPA;
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
    let mut pb = PathBuilder::new();
    push_rounded_rect_cw(&mut pb, x, y, width, height, rtl, rtr, rbr, rbl);
    pb.finish().expect("Failed to build rounded rect path")
}

fn draw_borders(pixmap: &mut Pixmap, fbox: &FlatBox, style: &ComputedStyle) {
    draw_borders_with_transform(pixmap, fbox, style, Transform::identity(), None);
}

fn draw_borders_with_transform(
    pixmap: &mut Pixmap,
    fbox: &FlatBox,
    style: &ComputedStyle,
    transform: Transform,
    clip_mask: Option<&ClipMask>,
) {
    use incognidium_style::BorderStyle;

    // Check if collapsed borders are set (for border-collapse tables)
    let (top_width, right_width, bottom_width, left_width) =
        if let Some(cb) = fbox.collapsed_borders {
            (cb.top, cb.right, cb.bottom, cb.left)
        } else {
            (
                style.border_top_width,
                style.border_right_width,
                style.border_bottom_width,
                style.border_left_width,
            )
        };

    // Get per-side colors, falling back to border_color if not set
    let top_color = style.border_top_color.unwrap_or(style.border_color);
    let right_color = style.border_right_color.unwrap_or(style.border_color);
    let bottom_color = style.border_bottom_color.unwrap_or(style.border_color);
    let left_color = style.border_left_color.unwrap_or(style.border_color);

    // Resolve border radii
    let resolve_radius = |sv: &incognidium_style::SizeValue| -> f32 {
        match sv {
            incognidium_style::SizeValue::Percent(p) => fbox.width.min(fbox.height) * p / 100.0,
            incognidium_style::SizeValue::Px(px) => *px,
            _ => 0.0,
        }
    };
    let max_radius = (fbox.width.min(fbox.height) / 2.0).max(0.0);
    let rtl = resolve_radius(&style.border_top_left_radius).min(max_radius);
    let rtr = resolve_radius(&style.border_top_right_radius).min(max_radius);
    let rbr = resolve_radius(&style.border_bottom_right_radius).min(max_radius);
    let rbl = resolve_radius(&style.border_bottom_left_radius).min(max_radius);
    let has_border_radius = rtl > 0.0 || rtr > 0.0 || rbr > 0.0 || rbl > 0.0;

    // If border-radius is present and borders are uniform (equal widths, solid style,
    // same visible color), draw a rounded border ring using outer/inner paths.
    // This handles common cases like circular loading spinners and rounded buttons.
    if has_border_radius
        && top_width == right_width
        && right_width == bottom_width
        && bottom_width == left_width
        && top_width > 0.0
        && style.border_top_style == BorderStyle::Solid
        && style.border_right_style == BorderStyle::Solid
        && style.border_bottom_style == BorderStyle::Solid
        && style.border_left_style == BorderStyle::Solid
    {
        // Find the dominant visible border color among non-transparent sides
        let sides = [
            (top_width, top_color),
            (right_width, right_color),
            (bottom_width, bottom_color),
            (left_width, left_color),
        ];
        let visible_sides: Vec<_> = sides.iter().filter(|(_, c)| c.a > 0).collect();
        if !visible_sides.is_empty() {
            // Use the first visible side's color for the ring
            let ring_color = visible_sides[0].1;
            // Build outer rounded rect path
            let outer_path = build_rounded_rect_path(
                fbox.x,
                fbox.y,
                fbox.width,
                fbox.height,
                rtl,
                rtr,
                rbr,
                rbl,
            );
            // Build inner rounded rect path (inset by border width)
            let inset = top_width;
            let inner_w = (fbox.width - inset * 2.0).max(0.0);
            let inner_h = (fbox.height - inset * 2.0).max(0.0);
            let inner_x = fbox.x + inset;
            let inner_y = fbox.y + inset;
            let inner_rtl = (rtl - inset).max(0.0);
            let inner_rtr = (rtr - inset).max(0.0);
            let inner_rbr = (rbr - inset).max(0.0);
            let inner_rbl = (rbl - inset).max(0.0);
            let inner_path = build_rounded_rect_path(
                inner_x, inner_y, inner_w, inner_h, inner_rtl, inner_rtr, inner_rbr, inner_rbl,
            );
            // Draw ring using even-odd fill rule
            let mut pb = PathBuilder::new();
            pb.push_path(&outer_path);
            pb.push_path(&inner_path);
            if let Some(path) = pb.finish() {
                let mut paint = Paint::default();
                paint.set_color(css_to_skia_color(ring_color));
                paint.anti_alias = true;
                pixmap.fill_path(&path, &paint, FillRule::EvenOdd, transform, None);
                apply_clip_mask_to_region(
                    pixmap,
                    fbox.x,
                    fbox.y,
                    fbox.width,
                    fbox.height,
                    clip_mask,
                );
            }
            return;
        }
    }

    // For collapsed borders, adjust positions so borders are drawn at the center
    // of where adjacent cells meet (borders are shared)
    let (top_y, left_x) = if fbox.collapsed_borders.is_some() {
        // For collapsed borders, the border is drawn at the edge
        (fbox.y, fbox.x)
    } else {
        // For separate borders, draw at the edge of the content box
        (fbox.y, fbox.x)
    };

    // Draw each border side with its specific style and color
    draw_border_side(
        pixmap,
        left_x,
        top_y,
        fbox.width,
        top_width,
        top_color,
        style.border_top_style,
        BorderSide::Top,
        fbox,
        transform,
        clip_mask,
    );

    draw_border_side(
        pixmap,
        left_x,
        top_y + fbox.height - bottom_width,
        fbox.width,
        bottom_width,
        bottom_color,
        style.border_bottom_style,
        BorderSide::Bottom,
        fbox,
        transform,
        clip_mask,
    );

    draw_border_side(
        pixmap,
        left_x,
        top_y,
        fbox.height,
        left_width,
        left_color,
        style.border_left_style,
        BorderSide::Left,
        fbox,
        transform,
        clip_mask,
    );

    draw_border_side(
        pixmap,
        left_x + fbox.width - right_width,
        top_y,
        fbox.height,
        right_width,
        right_color,
        style.border_right_style,
        BorderSide::Right,
        fbox,
        transform,
        clip_mask,
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
    clip_mask: Option<&ClipMask>,
) {
    if width <= 0.0 || length <= 0.0 {
        return;
    }

    use incognidium_style::BorderStyle;

    match style {
        BorderStyle::None | BorderStyle::Hidden => {}
        BorderStyle::Solid => {
            // Top/bottom borders extend horizontally; left/right borders extend
            // vertically, so swap the rect dimensions for the vertical sides.
            match side {
                BorderSide::Top | BorderSide::Bottom => {
                    draw_rect_with_transform(
                        pixmap, x, y, length, width, color, transform, clip_mask,
                    );
                }
                BorderSide::Left | BorderSide::Right => {
                    draw_rect_with_transform(
                        pixmap, x, y, width, length, color, transform, clip_mask,
                    );
                }
            }
        }
        BorderStyle::Dashed => {
            draw_dashed_border(
                pixmap, x, y, length, width, color, side, transform, clip_mask,
            );
        }
        BorderStyle::Dotted => {
            draw_dotted_border(
                pixmap, x, y, length, width, color, side, transform, clip_mask,
            );
        }
        BorderStyle::Double => {
            draw_double_border(
                pixmap, x, y, length, width, color, side, fbox, transform, clip_mask,
            );
        }
        // For groove, ridge, inset, outset - just draw solid for now
        _ => {
            draw_rect_with_transform(pixmap, x, y, length, width, color, transform, clip_mask);
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
    clip_mask: Option<&ClipMask>,
) {
    // Keep dashed borders readable on short sides. For long segments keep the
    // standard 3:2 dash-to-gap ratio based on border width. For short
    // segments scale the pattern so at least four dashes fit, with dashes taking
    // three-quarters of each cycle so the side does not look like a pair of
    // isolated corner nubs.
    let base_dash = width * 3.0;
    let base_gap = width * 2.0;
    let base_pattern = base_dash + base_gap;

    let (dash_length, gap_length, num_dashes) = if length >= base_pattern * 4.0 {
        (base_dash, base_gap, (length / base_pattern).ceil() as i32)
    } else if length > 0.0 {
        let n = 4.max((length / base_pattern).ceil() as i32);
        let pattern = length / n as f32;
        (pattern * 0.75, pattern * 0.25, n)
    } else {
        return;
    };

    let total_pattern = dash_length + gap_length;

    for i in 0..num_dashes {
        let offset = i as f32 * total_pattern;
        if offset >= length {
            break;
        }

        let current_dash_length = dash_length.min(length - offset);

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
                    clip_mask,
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
                    clip_mask,
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
    clip_mask: Option<&ClipMask>,
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
                    clip_mask,
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
                    clip_mask,
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
    clip_mask: Option<&ClipMask>,
) {
    if width < 3.0 {
        // Too thin for double, just draw solid
        draw_rect_with_transform(pixmap, x, y, length, width, color, transform, clip_mask);
        return;
    }

    let inner_width = width / 3.0;
    let gap_width = width / 3.0;

    match side {
        BorderSide::Top => {
            // Outer line
            draw_rect_with_transform(
                pixmap,
                x,
                y,
                length,
                inner_width,
                color,
                transform,
                clip_mask,
            );
            // Inner line
            draw_rect_with_transform(
                pixmap,
                x,
                y + inner_width + gap_width,
                length,
                inner_width,
                color,
                transform,
                clip_mask,
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
                clip_mask,
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
                clip_mask,
            );
        }
        BorderSide::Left => {
            // Outer line
            draw_rect_with_transform(
                pixmap,
                x,
                y,
                inner_width,
                length,
                color,
                transform,
                clip_mask,
            );
            // Inner line
            draw_rect_with_transform(
                pixmap,
                x + inner_width + gap_width,
                y,
                inner_width,
                length,
                color,
                transform,
                clip_mask,
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
                clip_mask,
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
                clip_mask,
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

    match style.outline_style {
        incognidium_style::OutlineStyle::None => {}
        incognidium_style::OutlineStyle::Solid => {
            // Draw solid outline as four rects
            draw_rect_with_transform(
                pixmap,
                outline_x,
                outline_y,
                outline_w,
                outline_width,
                oc,
                transform,
                None,
            );
            draw_rect_with_transform(
                pixmap,
                outline_x,
                outline_y + outline_h - outline_width,
                outline_w,
                outline_width,
                oc,
                transform,
                None,
            );
            draw_rect_with_transform(
                pixmap,
                outline_x,
                outline_y + outline_width,
                outline_width,
                outline_h - outline_width * 2.0,
                oc,
                transform,
                None,
            );
            draw_rect_with_transform(
                pixmap,
                outline_x + outline_w - outline_width,
                outline_y + outline_width,
                outline_width,
                outline_h - outline_width * 2.0,
                oc,
                transform,
                None,
            );
        }
        incognidium_style::OutlineStyle::Dashed => {
            // Draw dashed outline segments
            draw_dashed_outline_segment(
                pixmap,
                outline_x,
                outline_y,
                outline_w,
                outline_width,
                oc,
                true,
                transform,
            );
            draw_dashed_outline_segment(
                pixmap,
                outline_x,
                outline_y + outline_h - outline_width,
                outline_w,
                outline_width,
                oc,
                true,
                transform,
            );
            draw_dashed_outline_segment(
                pixmap,
                outline_x,
                outline_y + outline_width,
                outline_h - outline_width * 2.0,
                outline_width,
                oc,
                false,
                transform,
            );
            draw_dashed_outline_segment(
                pixmap,
                outline_x + outline_w - outline_width,
                outline_y + outline_width,
                outline_h - outline_width * 2.0,
                outline_width,
                oc,
                false,
                transform,
            );
        }
        incognidium_style::OutlineStyle::Dotted => {
            // Draw dotted outline
            draw_dotted_outline(
                pixmap,
                outline_x,
                outline_y,
                outline_w,
                outline_h,
                outline_width,
                oc,
                transform,
            );
        }
        incognidium_style::OutlineStyle::Double => {
            // Draw double outline (two thinner lines)
            let inner_width = outline_width / 3.0;
            let gap = outline_width / 3.0;
            // Outer line
            draw_rect_with_transform(
                pixmap,
                outline_x,
                outline_y,
                outline_w,
                inner_width,
                oc,
                transform,
                None,
            );
            draw_rect_with_transform(
                pixmap,
                outline_x,
                outline_y + outline_h - inner_width,
                outline_w,
                inner_width,
                oc,
                transform,
                None,
            );
            draw_rect_with_transform(
                pixmap,
                outline_x,
                outline_y + inner_width,
                inner_width,
                outline_h - inner_width * 2.0,
                oc,
                transform,
                None,
            );
            draw_rect_with_transform(
                pixmap,
                outline_x + outline_w - inner_width,
                outline_y + inner_width,
                inner_width,
                outline_h - inner_width * 2.0,
                oc,
                transform,
                None,
            );
            // Inner line
            let inner_offset = inner_width + gap;
            draw_rect_with_transform(
                pixmap,
                outline_x + inner_offset,
                outline_y + inner_offset,
                outline_w - inner_offset * 2.0,
                inner_width,
                oc,
                transform,
                None,
            );
            draw_rect_with_transform(
                pixmap,
                outline_x + inner_offset,
                outline_y + outline_h - inner_offset - inner_width,
                outline_w - inner_offset * 2.0,
                inner_width,
                oc,
                transform,
                None,
            );
            draw_rect_with_transform(
                pixmap,
                outline_x + inner_offset,
                outline_y + inner_offset,
                inner_width,
                outline_h - inner_offset * 2.0,
                oc,
                transform,
                None,
            );
            draw_rect_with_transform(
                pixmap,
                outline_x + outline_w - inner_offset - inner_width,
                outline_y + inner_offset,
                inner_width,
                outline_h - inner_offset * 2.0,
                oc,
                transform,
                None,
            );
        }
    }
}

/// Draw a dashed segment of outline
fn draw_dashed_outline_segment(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    length: f32,
    width: f32,
    color: CssColor,
    is_horizontal: bool,
    transform: Transform,
) {
    // Use a dash pattern that stays visible on short sides. For long
    // segments keep the standard 3:2 dash-to-gap ratio based on outline width.
    // For short segments scale the pattern so at least four dashes fit, with
    // dashes taking three-quarters of each cycle so the outline does not look
    // like a pair of isolated horizontal bars.
    let base_dash = width * 3.0;
    let base_gap = width * 2.0;
    let base_pattern = base_dash + base_gap;

    let (dash_length, gap_length, num_dashes) = if length >= base_pattern * 4.0 {
        (base_dash, base_gap, (length / base_pattern).ceil() as i32)
    } else if length > 0.0 {
        let n = 4.max((length / base_pattern).ceil() as i32);
        let pattern = length / n as f32;
        (pattern * 0.75, pattern * 0.25, n)
    } else {
        return;
    };

    let total_pattern = dash_length + gap_length;

    for i in 0..num_dashes {
        let offset = i as f32 * total_pattern;
        if offset >= length {
            break;
        }
        let current_dash_length = dash_length.min(length - offset);

        if is_horizontal {
            draw_rect_with_transform(
                pixmap,
                x + offset,
                y,
                current_dash_length,
                width,
                color,
                transform,
                None,
            );
        } else {
            draw_rect_with_transform(
                pixmap,
                x,
                y + offset,
                width,
                current_dash_length,
                color,
                transform,
                None,
            );
        }
    }
}

/// Draw a dotted outline
fn draw_dotted_outline(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    width: f32,
    color: CssColor,
    transform: Transform,
) {
    let dot_size = width;
    let gap = width * 2.0;
    let total = dot_size + gap;

    // Top edge
    let num_dots = (w / total).ceil() as i32;
    for i in 0..num_dots {
        let dx = i as f32 * total;
        if dx + dot_size <= w {
            draw_rect_with_transform(
                pixmap,
                x + dx,
                y,
                dot_size,
                dot_size,
                color,
                transform,
                None,
            );
        }
    }

    // Bottom edge
    for i in 0..num_dots {
        let dx = i as f32 * total;
        if dx + dot_size <= w {
            draw_rect_with_transform(
                pixmap,
                x + dx,
                y + h - dot_size,
                dot_size,
                dot_size,
                color,
                transform,
                None,
            );
        }
    }

    // Left edge (excluding corners already drawn)
    let num_dots_v = ((h - dot_size * 2.0) / total).ceil() as i32;
    for i in 0..num_dots_v {
        let dy = dot_size + i as f32 * total;
        if dy + dot_size <= h - dot_size {
            draw_rect_with_transform(
                pixmap,
                x,
                y + dy,
                dot_size,
                dot_size,
                color,
                transform,
                None,
            );
        }
    }

    // Right edge (excluding corners)
    for i in 0..num_dots_v {
        let dy = dot_size + i as f32 * total;
        if dy + dot_size <= h - dot_size {
            draw_rect_with_transform(
                pixmap,
                x + w - dot_size,
                y + dy,
                dot_size,
                dot_size,
                color,
                transform,
                None,
            );
        }
    }
}

/// Draw column rules (vertical lines between columns) for multi-column layout
fn draw_column_rules(pixmap: &mut Pixmap, fbox: &FlatBox, transform: Transform) {
    let num_columns = fbox.column_count;
    let column_width = fbox.column_width;
    let column_gap = fbox.column_gap;
    let rule_width = fbox.column_rule_width;
    let rule_style = fbox.column_rule_style;
    let rule_color = fbox.column_rule_color;

    // Only draw if there's more than one column and rule has width and style
    if num_columns <= 1 || rule_width <= 0.0 || rule_style == ColumnRuleStyle::None {
        return;
    }

    // Calculate the content area (where columns are)
    // For multi-column containers, content_x/content_y are set to the content area position
    let content_top = fbox.content_y;
    let content_height = if fbox.column_count > 0 && fbox.content_height > 0.0 {
        fbox.content_height
    } else {
        fbox.height
    };

    // Draw a rule between each pair of columns
    for i in 1..num_columns {
        // Position is at the gap between column i-1 and column i
        // Each column takes up column_width, and there's a gap after each column except the last
        let rule_x = fbox.content_x + i as f32 * column_width + (i as f32 - 0.5) * column_gap
            - rule_width / 2.0;

        // Draw the rule based on style
        match rule_style {
            ColumnRuleStyle::Dashed => {
                // Draw dashed line - approximate with segments
                let dash_length = 5.0;
                let gap_length = 3.0;
                let mut y = content_top;
                while y < content_top + content_height {
                    let segment_end = (y + dash_length).min(content_top + content_height);
                    draw_rect_with_transform(
                        pixmap,
                        rule_x,
                        y,
                        rule_width,
                        segment_end - y,
                        rule_color,
                        transform,
                        None,
                    );
                    y += dash_length + gap_length;
                }
            }
            ColumnRuleStyle::Dotted => {
                // Draw dotted line - small circles/rects
                let dot_size = rule_width.max(2.0);
                let dot_spacing = dot_size * 2.5;
                let mut y = content_top;
                while y < content_top + content_height {
                    draw_rect_with_transform(
                        pixmap,
                        rule_x - (dot_size - rule_width) / 2.0,
                        y,
                        dot_size,
                        dot_size,
                        rule_color,
                        transform,
                        None,
                    );
                    y += dot_spacing;
                }
            }
            _ => {
                // Solid line (or fallback for other styles like Double which we simplify)
                draw_rect_with_transform(
                    pixmap,
                    rule_x,
                    content_top,
                    rule_width,
                    content_height,
                    rule_color,
                    transform,
                    None,
                );
            }
        }
    }
}

/// Draw a checkbox input element
fn draw_checkbox(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    style: &ComputedStyle,
    checked: bool,
) {
    let margin = 1.0;
    let size = width.min(height) - margin * 2.0;
    let x = x + margin;
    let y = y + margin;

    // Check for dark color scheme
    let is_dark = matches!(style.color_scheme, incognidium_style::ColorScheme::Dark)
        || matches!(
            style.color_scheme,
            incognidium_style::ColorScheme::LightDark
        );

    // Face color for the unchecked control: light schemes get a white field,
    // dark schemes get a dark slate field so the control stays visible against
    // dark author backgrounds.
    let face_color = if is_dark {
        CssColor::from_rgb(45, 55, 72)
    } else {
        CssColor::from_rgb(255, 255, 255)
    };

    // Use accent-color for checked state if available; otherwise pick a default
    // that contrasts with the color scheme so the marker remains visible.
    let default_accent = if is_dark {
        CssColor::from_rgb(45, 55, 72)
    } else {
        CssColor::from_rgb(13, 110, 253)
    };
    let accent_color = style.accent_color.unwrap_or(default_accent);

    // The border uses the element's text color, which the author already chose
    // to contrast with the background, falling back to a scheme-appropriate
    // neutral so it does not disappear.
    let border_color = if style.color.a > 0 {
        style.color
    } else if is_dark {
        CssColor::from_rgb(200, 200, 200)
    } else {
        CssColor::from_rgb(100, 100, 100)
    };

    let checkbox_rect = match Rect::from_xywh(x, y, size, size) {
        Some(r) => r,
        None => return,
    };
    let checkbox_path = PathBuilder::from_rect(checkbox_rect);

    // Fill the control: checked uses the accent color, unchecked uses the
    // scheme face color so the box looks like a real control on any background.
    let fill_color = if checked { accent_color } else { face_color };
    let mut paint = Paint::default();
    paint.set_color(css_to_skia_color(fill_color));
    paint.anti_alias = true;
    pixmap.fill_path(
        &checkbox_path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );

    // Stroke the border on top so unchecked controls look like hollow outlines
    // and checked controls get a crisp edge.
    let border_width = if is_dark { 1.5 } else { 1.0 };
    let mut border_paint = Paint::default();
    border_paint.set_color(css_to_skia_color(border_color));
    border_paint.anti_alias = true;
    pixmap.stroke_path(
        &checkbox_path,
        &border_paint,
        &tiny_skia::Stroke {
            width: border_width,
            line_cap: tiny_skia::LineCap::Butt,
            line_join: tiny_skia::LineJoin::Miter,
            dash: None,
            miter_limit: 4.0,
        },
        Transform::identity(),
        None,
    );

    // Draw checkmark if checked. Use a contrasting color so the mark stays visible
    // against any accent color without hardcoding a specific foreground shade.
    if checked {
        let mut pb = PathBuilder::new();
        pb.move_to(x + size * 0.22, y + size * 0.53);
        pb.line_to(x + size * 0.42, y + size * 0.74);
        pb.line_to(x + size * 0.78, y + size * 0.30);
        if let Some(path) = pb.finish() {
            let mut paint = Paint::default();
            let luminance = 0.299 * accent_color.r as f32
                + 0.587 * accent_color.g as f32
                + 0.114 * accent_color.b as f32;
            let mark_color = if luminance > 140.0 {
                CssColor::from_rgb(0, 0, 0)
            } else {
                CssColor::from_rgb(255, 255, 255)
            };
            paint.set_color(Color::from_rgba8(
                mark_color.r,
                mark_color.g,
                mark_color.b,
                accent_color.a,
            ));
            paint.anti_alias = true;
            pixmap.stroke_path(
                &path,
                &paint,
                &tiny_skia::Stroke {
                    width: size * 0.16,
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

/// Draw a radio button input element
fn draw_radio(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    style: &ComputedStyle,
    checked: bool,
) {
    let margin = 1.0;
    let size = width.min(height) - margin * 2.0;
    let cx = x + margin + size / 2.0;
    let cy = y + margin + size / 2.0;
    let radius = size / 2.0;

    // Check for dark color scheme
    let is_dark = matches!(style.color_scheme, incognidium_style::ColorScheme::Dark)
        || matches!(
            style.color_scheme,
            incognidium_style::ColorScheme::LightDark
        );

    // Face color for the unchecked control: light schemes get a white field,
    // dark schemes get a dark slate field so the control stays visible against
    // dark author backgrounds.
    let face_color = if is_dark {
        CssColor::from_rgb(45, 55, 72)
    } else {
        CssColor::from_rgb(255, 255, 255)
    };

    // Use accent-color for checked state if available; otherwise pick a default
    // that contrasts with the color scheme so the dot remains visible.
    let default_accent = if is_dark {
        CssColor::from_rgb(45, 55, 72)
    } else {
        CssColor::from_rgb(13, 110, 253)
    };
    let accent_color = style.accent_color.unwrap_or(default_accent);

    // The border uses the element's text color, which the author already chose
    // to contrast with the background, falling back to a scheme-appropriate
    // neutral so it does not disappear.
    let border_color = if style.color.a > 0 {
        style.color
    } else if is_dark {
        CssColor::from_rgb(200, 200, 200)
    } else {
        CssColor::from_rgb(100, 100, 100)
    };

    let mut pb = PathBuilder::new();
    pb.push_circle(cx, cy, radius);
    let radio_path = match pb.finish() {
        Some(p) => p,
        None => return,
    };

    // Fill the circle: checked uses the accent color, unchecked uses the
    // scheme face color so the radio looks like a real control on any background.
    let fill_color = if checked { accent_color } else { face_color };
    let mut paint = Paint::default();
    paint.set_color(css_to_skia_color(fill_color));
    paint.anti_alias = true;
    pixmap.fill_path(
        &radio_path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );

    // Stroke the border on top so unchecked radios look like hollow outlines.
    let border_width = if is_dark { 1.5 } else { 1.0 };
    let mut border_paint = Paint::default();
    border_paint.set_color(css_to_skia_color(border_color));
    border_paint.anti_alias = true;
    pixmap.stroke_path(
        &radio_path,
        &border_paint,
        &tiny_skia::Stroke {
            width: border_width,
            line_cap: tiny_skia::LineCap::Butt,
            line_join: tiny_skia::LineJoin::Miter,
            dash: None,
            miter_limit: 4.0,
        },
        Transform::identity(),
        None,
    );

    // Draw the selection dot when checked, using a contrasting color so it
    // stays visible against any accent color.
    if checked {
        let luminance = 0.299 * accent_color.r as f32
            + 0.587 * accent_color.g as f32
            + 0.114 * accent_color.b as f32;
        let dot_color = if luminance > 140.0 {
            CssColor::from_rgb(0, 0, 0)
        } else {
            CssColor::from_rgb(255, 255, 255)
        };
        let dot_radius = size * 0.27;
        draw_circle(pixmap, cx, cy, dot_radius, dot_color);
    }
}

/// Draw a small downward arrow in the right padding of a `<select>` control so
/// users can tell it opens a dropdown list. The arrow is centered in the
/// element's right padding area and uses the element's text color.
fn draw_select_arrow(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    style: &ComputedStyle,
) {
    let border_right = style.border_right_width;
    let padding_right = style.padding_right_px(width);
    let avail = padding_right - 2.0;
    if avail < 5.0 {
        return;
    }
    let arrow_size = (style.font_size * 0.625).min(avail).max(6.0);
    let cx = x + width - border_right - padding_right / 2.0;
    let cy = y + height / 2.0;

    let color = if style.color.a > 0 {
        style.color
    } else {
        CssColor::from_rgb(80, 80, 80)
    };

    let mut pb = PathBuilder::new();
    pb.move_to(cx - arrow_size / 2.0, cy - arrow_size / 4.0);
    pb.line_to(cx + arrow_size / 2.0, cy - arrow_size / 4.0);
    pb.line_to(cx, cy + arrow_size * 0.55);
    pb.close();
    let path = match pb.finish() {
        Some(p) => p,
        None => return,
    };
    let mut paint = Paint::default();
    paint.set_color(css_to_skia_color(color));
    paint.anti_alias = true;
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

/// Draw resize handle indicator in the bottom-right corner
fn draw_resize_handle(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    style: &ComputedStyle,
) {
    use incognidium_style::Resize;

    // Only show handle for resize: both, horizontal, or vertical
    // For block/inline, we would show handles on different edges, but for now
    // we'll show the bottom-right corner handle for all resizable types
    let show_handle = matches!(
        style.resize,
        Resize::Both | Resize::Horizontal | Resize::Vertical | Resize::Block | Resize::Inline
    );

    if !show_handle {
        return;
    }

    // Draw a small triangle in the bottom-right corner
    let handle_size = 12.0;
    let handle_x = x + width - handle_size - 2.0;
    let handle_y = y + height - handle_size - 2.0;

    // Handle color - semi-transparent gray
    let handle_color = CssColor {
        r: 128,
        g: 128,
        b: 128,
        a: 200,
    };

    // Draw a small diagonal line pattern to indicate resize
    let path = {
        let mut pb = PathBuilder::new();
        // Draw three diagonal lines forming a resize grip pattern
        for i in 0..3 {
            let offset = i as f32 * 4.0;
            let start_x = handle_x + offset;
            let start_y = handle_y + handle_size;
            let end_x = handle_x + handle_size;
            let end_y = handle_y + offset;

            if start_x <= end_x && end_y <= start_y {
                pb.move_to(start_x, start_y);
                pb.line_to(end_x, end_y);
            }
        }
        pb.finish()
    };

    if let Some(path) = path {
        let mut paint = Paint::default();
        paint.set_color(css_to_skia_color(handle_color));
        paint.anti_alias = true;
        pixmap.stroke_path(
            &path,
            &paint,
            &tiny_skia::Stroke {
                width: 1.5,
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

/// Push a counter-clockwise rounded rectangle subpath onto an existing
/// PathBuilder. The geometric path is the same as the clockwise version, but
/// traced in reverse so it acts as a hole under the Winding fill rule.
fn push_rounded_rect_ccw(
    pb: &mut PathBuilder,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    rtl: f32,
    rtr: f32,
    rbr: f32,
    rbl: f32,
) {
    // Top edge from right to left, ending at the top-left radius start.
    pb.move_to(x + rtl, y);
    // Top-left corner (reverse of the CW arc)
    if rtl > 0.0 {
        let k = rtl * ROUNDED_RECT_KAPPA;
        pb.cubic_to(x + rtl - k, y, x, y + rtl - k, x, y + rtl);
    }
    // Left edge
    pb.line_to(x, y + height - rbl);
    // Bottom-left corner
    if rbl > 0.0 {
        let k = rbl * ROUNDED_RECT_KAPPA;
        pb.cubic_to(
            x,
            y + height - rbl + k,
            x + rbl - k,
            y + height,
            x + rbl,
            y + height,
        );
    }
    // Bottom edge
    pb.line_to(x + width - rbr, y + height);
    // Bottom-right corner
    if rbr > 0.0 {
        let k = rbr * ROUNDED_RECT_KAPPA;
        pb.cubic_to(
            x + width - rbr + k,
            y + height,
            x + width,
            y + height - rbr + k,
            x + width,
            y + height - rbr,
        );
    }
    // Right edge
    pb.line_to(x + width, y + rtr);
    // Top-right corner
    if rtr > 0.0 {
        let k = rtr * ROUNDED_RECT_KAPPA;
        pb.cubic_to(
            x + width,
            y + rtr - k,
            x + width - rtr + k,
            y,
            x + width - rtr,
            y,
        );
    }
    pb.close();
}

/// Resolve CSS border-radius SizeValues to absolute pixels and clamp them to
/// half the smaller box dimension so the rounded rect remains valid.
fn resolve_border_radii(
    width: f32,
    height: f32,
    rtl: &incognidium_style::SizeValue,
    rtr: &incognidium_style::SizeValue,
    rbr: &incognidium_style::SizeValue,
    rbl: &incognidium_style::SizeValue,
) -> (f32, f32, f32, f32) {
    let resolve = |sv: &incognidium_style::SizeValue| -> f32 {
        match sv {
            incognidium_style::SizeValue::Percent(p) => width.min(height) * p / 100.0,
            incognidium_style::SizeValue::Px(px) => *px,
            _ => 0.0,
        }
    };
    let max_radius = (width.min(height) / 2.0).max(0.0);
    (
        resolve(rtl).min(max_radius),
        resolve(rtr).min(max_radius),
        resolve(rbr).min(max_radius),
        resolve(rbl).min(max_radius),
    )
}

/// Draw a box shadow behind or inside an element, honoring border-radius so
/// rounded cards do not show sharp-corner shadow artifacts.
fn erfc_approx(x: f32) -> f32 {
    libm::erfcf(x)
}

fn draw_box_shadow(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    shadow: &incognidium_style::BoxShadow,
    transform: Transform,
    radius_tl: SizeValue,
    radius_tr: SizeValue,
    radius_br: SizeValue,
    radius_bl: SizeValue,
) {
    use incognidium_style::BoxShadow;

    let shadow_color = shadow.color;
    let blur_radius = shadow.blur_radius;
    let spread_radius = shadow.spread_radius;
    let offset_x = shadow.offset_x;
    let offset_y = shadow.offset_y;

    let (rtl, rtr, rbr, rbl) = resolve_border_radii(
        width, height, &radius_tl, &radius_tr, &radius_br, &radius_bl,
    );

    let box_rect = match Rect::from_xywh(x, y, width, height) {
        Some(r) => r,
        None => return,
    };

    // Build a clockwise rounded rectangle path, clamping radii to the box size.
    let rounded_path =
        |rx: f32, ry: f32, rw: f32, rh: f32, rt: (f32, f32, f32, f32)| -> Option<Path> {
            if rw <= 0.0 || rh <= 0.0 {
                return None;
            }
            let max_r = (rw.min(rh) / 2.0).max(0.0);
            let mut pb = PathBuilder::new();
            push_rounded_rect_cw(
                &mut pb,
                rx,
                ry,
                rw,
                rh,
                rt.0.min(max_r),
                rt.1.min(max_r),
                rt.2.min(max_r),
                rt.3.min(max_r),
            );
            pb.finish()
        };

    let box_path = rounded_path(x, y, width, height, (rtl, rtr, rbr, rbl));

    if shadow.inset {
        // INSET SHADOW: shadow appears inside the box, fading inward from edges.
        let spread = spread_radius;

        let shadow_w = (width - spread * 2.0).max(0.0);
        let shadow_h = (height - spread * 2.0).max(0.0);
        let shadow_x = x + spread + offset_x;
        let shadow_y = y + spread + offset_y;

        if shadow_w <= 0.0 || shadow_h <= 0.0 {
            return;
        }

        let srtl = (rtl - spread).max(0.0);
        let srtr = (rtr - spread).max(0.0);
        let srbr = (rbr - spread).max(0.0);
        let srbl = (rbl - spread).max(0.0);
        let shadow_path = rounded_path(
            shadow_x,
            shadow_y,
            shadow_w,
            shadow_h,
            (srtl, srtr, srbr, srbl),
        );

        if blur_radius <= 0.0 {
            // Solid inset shadow: ring between the border box and inner shadow shape.
            if let (Some(outer), Some(inner)) = (&box_path, &shadow_path) {
                let mut pb = PathBuilder::new();
                pb.push_path(outer);
                pb.push_path(inner);
                let mut paint = Paint::default();
                paint.set_color(css_to_skia_color(shadow_color));
                paint.anti_alias = true;
                if let Some(path) = pb.finish() {
                    pixmap.fill_path(&path, &paint, FillRule::EvenOdd, transform, None);
                }
            }
        } else {
            // Blurred inset shadow: fade from the shadow edge toward the center.
            let steps = (blur_radius * 1.5).max(5.0).min(30.0) as i32;
            for i in 0..=steps {
                let factor = i as f32 / steps as f32;
                let alpha =
                    (shadow_color.a as f32 * (-factor * factor * 3.0).exp() * (2.0 / steps as f32))
                        .max(0.0)
                        .min(255.0) as u8;
                if alpha == 0 {
                    continue;
                }

                let inset = blur_radius * factor;
                let inner_w = (shadow_w - inset * 2.0).max(0.0);
                let inner_h = (shadow_h - inset * 2.0).max(0.0);
                let inner_x = shadow_x + inset;
                let inner_y = shadow_y + inset;
                let irl = (srtl - inset).max(0.0);
                let irr = (srtr - inset).max(0.0);
                let ibr = (srbr - inset).max(0.0);
                let ibl = (srbl - inset).max(0.0);

                if let (Some(outer), Some(inner)) = (
                    &box_path,
                    rounded_path(inner_x, inner_y, inner_w, inner_h, (irl, irr, ibr, ibl)),
                ) {
                    let mut pb = PathBuilder::new();
                    pb.push_path(outer);
                    pb.push_path(&inner);
                    let mut paint = Paint::default();
                    let color = CssColor {
                        r: shadow_color.r,
                        g: shadow_color.g,
                        b: shadow_color.b,
                        a: alpha,
                    };
                    paint.set_color(css_to_skia_color(color));
                    paint.anti_alias = true;
                    if let Some(path) = pb.finish() {
                        pixmap.fill_path(&path, &paint, FillRule::EvenOdd, transform, None);
                    }
                }
            }
        }
    } else {
        // OUTER SHADOW: shadow appears outside the box.
        let shadow_x = x + offset_x - spread_radius;
        let shadow_y = y + offset_y - spread_radius;
        let shadow_w = width + spread_radius * 2.0;
        let shadow_h = height + spread_radius * 2.0;

        if shadow_w <= 0.0 || shadow_h <= 0.0 {
            return;
        }

        let max_shadow_r = (shadow_w.min(shadow_h) / 2.0).max(0.0);
        let srtl = (rtl + spread_radius).min(max_shadow_r);
        let srtr = (rtr + spread_radius).min(max_shadow_r);
        let srbr = (rbr + spread_radius).min(max_shadow_r);
        let srbl = (rbl + spread_radius).min(max_shadow_r);
        let shadow_path = rounded_path(
            shadow_x,
            shadow_y,
            shadow_w,
            shadow_h,
            (srtl, srtr, srbr, srbl),
        );

        // An outer box-shadow with a positive spread radius has a solid core
        // (the expanded shadow shape minus the border box) painted at the shadow's
        // full alpha. When there is no spread, the whole shadow is a Gaussian blur
        // of the offset box, so we draw only blur rings and avoid a dark solid
        // slab that can look like it sits on top of content.
        let has_spread = spread_radius > 0.0;
        if has_spread || blur_radius <= 0.0 {
            if let (Some(outer), Some(inner)) = (&shadow_path, &box_path) {
                let mut pb = PathBuilder::new();
                pb.push_path(outer);
                pb.push_path(inner);
                let mut paint = Paint::default();
                paint.set_color(css_to_skia_color(shadow_color));
                paint.anti_alias = true;
                if let Some(path) = pb.finish() {
                    pixmap.fill_path(&path, &paint, FillRule::EvenOdd, transform, None);
                }
            }
        }

        if blur_radius > 0.0 {
            // CSS blur radii describe the kernel size; the visible shadow extends
            // roughly twice the radius. Draw the blur as adjacent rings outside
            // the shadow edge so the opacity follows a Gaussian falloff. Only the
            // outer fringe is needed: the inner side is clipped by the element's
            // own border box, and drawing it on rounded cards produces dark
            // wedges in the corner cutouts.
            let blur_extent = blur_radius * 2.0;
            let signed_start = 0.0;
            let signed_end = blur_extent;
            let signed_range = signed_end - signed_start;
            let shadow_a = shadow_color.a as f32;
            let steps = (signed_range * 1.5).max(12.0).min(60.0) as i32;

            let shadow_falloff = |t: f32| {
                // The visible outer shadow is the convolution of the box shape
                // with a Gaussian kernel. With no spread, only the half of the
                // kernel that lies outside the box contributes, so opacity at
                // the box edge is roughly half the shadow color's alpha and
                // fades outward like the complementary error function.
                let d = 1.0 - t.clamp(0.0, 1.0);
                0.5 * erfc_approx(d * 2.0 * std::f32::consts::SQRT_2)
            };

            for i in 0..steps {
                let outer_t = i as f32 / steps as f32;
                let inner_t = (i + 1) as f32 / steps as f32;
                let outer_expand = signed_range * (1.0 - outer_t);
                let inner_expand = signed_range * (1.0 - inner_t);

                let mid_expand = (outer_expand + inner_expand) * 0.5;
                let mid_t = 1.0 - (mid_expand / blur_extent).clamp(0.0, 1.0);
                let alpha = (shadow_a * shadow_falloff(mid_t)).max(0.0).min(255.0) as u8;
                if alpha == 0 {
                    continue;
                }

                let outer_w = (shadow_w + outer_expand * 2.0).max(0.0);
                let outer_h = (shadow_h + outer_expand * 2.0).max(0.0);
                let outer_x = shadow_x - outer_expand;
                let outer_y = shadow_y - outer_expand;
                let inner_w = (shadow_w + inner_expand * 2.0).max(0.0);
                let inner_h = (shadow_h + inner_expand * 2.0).max(0.0);
                let inner_x = shadow_x - inner_expand;
                let inner_y = shadow_y - inner_expand;

                let max_outer_r = (outer_w.min(outer_h) / 2.0).max(0.0);
                let max_inner_r = (inner_w.min(inner_h) / 2.0).max(0.0);
                let orl = (srtl + outer_expand).max(0.0).min(max_outer_r);
                let ortr = (srtr + outer_expand).max(0.0).min(max_outer_r);
                let orbr = (srbr + outer_expand).max(0.0).min(max_outer_r);
                let orbl = (srbl + outer_expand).max(0.0).min(max_outer_r);
                let irl = (srtl + inner_expand).max(0.0).min(max_inner_r);
                let irr = (srtr + inner_expand).max(0.0).min(max_inner_r);
                let ibr = (srbr + inner_expand).max(0.0).min(max_inner_r);
                let ibl = (srbl + inner_expand).max(0.0).min(max_inner_r);

                if let (Some(outer), Some(inner)) = (
                    rounded_path(outer_x, outer_y, outer_w, outer_h, (orl, ortr, orbr, orbl)),
                    Some(rounded_path(
                        inner_x,
                        inner_y,
                        inner_w,
                        inner_h,
                        (irl, irr, ibr, ibl),
                    )),
                ) {
                    let mut pb = PathBuilder::new();
                    pb.push_path(&outer);
                    push_rounded_rect_ccw(
                        &mut pb, inner_x, inner_y, inner_w, inner_h, irl, irr, ibr, ibl,
                    );
                    // The counter-clockwise inner path leaves the border-box
                    // interior as a hole, so the shadow is only painted outside
                    // the element. This avoids the post-blur "clear the interior"
                    // workaround that turned transparent boxes black.
                    let mut band_paint = Paint::default();
                    let color = CssColor {
                        r: shadow_color.r,
                        g: shadow_color.g,
                        b: shadow_color.b,
                        a: alpha,
                    };
                    band_paint.set_color(css_to_skia_color(color));
                    band_paint.anti_alias = true;
                    if let Some(path) = pb.finish() {
                        pixmap.fill_path(&path, &band_paint, FillRule::Winding, transform, None);
                    }
                }
            }
        }
    }
}

/// Draw an image scaled to fit the given box.
#[allow(dead_code)]
fn draw_image(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    box_w: f32,
    box_h: f32,
    img: &ImageData,
    object_fit: incognidium_style::ObjectFit,
    object_position: (f32, f32),
    image_rendering: incognidium_style::ImageRendering,
) {
    draw_image_with_transform(
        pixmap,
        x,
        y,
        box_w,
        box_h,
        img,
        Transform::identity(),
        object_fit,
        false,
        object_position,
        image_rendering,
        incognidium_style::SizeValue::Px(0.0),
        incognidium_style::SizeValue::Px(0.0),
        incognidium_style::SizeValue::Px(0.0),
        incognidium_style::SizeValue::Px(0.0),
        None,
    );
}

/// Resolve a CSS `background-position` value to an absolute pixel offset and
/// the normalized object-position the existing image sampler expects.
///
/// The style layer stores percentages as `0.0..=1.0` and pixel lengths as
/// raw numbers. A value outside that range is treated as a pixel offset;
/// otherwise it is treated as a percentage of the leftover space.
fn resolve_background_position(value: f32, box_size: f32, rendered_size: f32) -> (f32, f32) {
    let offset = if value < 0.0 || value > 1.0 {
        value
    } else {
        value * (box_size - rendered_size)
    };
    let leftover = box_size - rendered_size;
    let normalized = if leftover.abs() > 0.001 {
        offset / leftover
    } else {
        0.5
    };
    (offset, normalized)
}

/// Decide which `object-fit` behavior to apply when the author did not set an
/// explicit value. SVG images default to their `preserveAspectRatio` attribute
/// (usually `meet`, equivalent to `contain`) instead of stretching like raster
/// images do with `object-fit: fill`.
fn effective_image_object_fit(
    object_fit: incognidium_style::ObjectFit,
    object_fit_specified: bool,
    preserve_aspect_ratio: Option<SvgPreserveAspectRatio>,
) -> incognidium_style::ObjectFit {
    if object_fit == incognidium_style::ObjectFit::Fill && !object_fit_specified {
        match preserve_aspect_ratio {
            Some(SvgPreserveAspectRatio::Meet) => incognidium_style::ObjectFit::Contain,
            Some(SvgPreserveAspectRatio::Slice) => incognidium_style::ObjectFit::Cover,
            _ => incognidium_style::ObjectFit::Fill,
        }
    } else {
        object_fit
    }
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
    object_fit: incognidium_style::ObjectFit,
    object_fit_specified: bool,
    object_position: (f32, f32),
    image_rendering: incognidium_style::ImageRendering,
    radius_tl: incognidium_style::SizeValue,
    radius_tr: incognidium_style::SizeValue,
    radius_br: incognidium_style::SizeValue,
    radius_bl: incognidium_style::SizeValue,
    scissor: Option<(f32, f32, f32, f32)>,
) {
    if img.width == 0 || img.height == 0 || box_w <= 0.0 || box_h <= 0.0 {
        return;
    }
    // Layout and object-fit are driven by the image's CSS intrinsic size,
    // while the raster buffer may have been decoded at a different resolution.
    let src_w = img.intrinsic_width.max(1) as f32;
    let src_h = img.intrinsic_height.max(1) as f32;

    let pm_w = pixmap.width();
    let pm_h = pixmap.height();
    let px_data = pixmap.data_mut();

    // Resolve border-radii for clipping (identity transforms only).
    let max_radius = (box_w.min(box_h) / 2.0).max(0.0);
    let resolve_radius = |sv: &incognidium_style::SizeValue| -> f32 {
        match sv {
            incognidium_style::SizeValue::Percent(p) => box_w.min(box_h) * p / 100.0,
            incognidium_style::SizeValue::Px(px) => *px,
            _ => 0.0,
        }
        .min(max_radius)
        .max(0.0)
    };
    let has_radius = transform == Transform::identity()
        && (radius_tl != incognidium_style::SizeValue::Px(0.0)
            || radius_tr != incognidium_style::SizeValue::Px(0.0)
            || radius_br != incognidium_style::SizeValue::Px(0.0)
            || radius_bl != incognidium_style::SizeValue::Px(0.0));
    let (rtl, rtr, rbr, rbl) = if has_radius {
        (
            resolve_radius(&radius_tl),
            resolve_radius(&radius_tr),
            resolve_radius(&radius_br),
            resolve_radius(&radius_bl),
        )
    } else {
        (0.0, 0.0, 0.0, 0.0)
    };
    let in_rounded_rect = |px: f32, py: f32| -> bool {
        if px < x || px > x + box_w || py < y || py > y + box_h {
            return false;
        }
        if px <= x + rtl && py <= y + rtl {
            let dx = px - (x + rtl);
            let dy = py - (y + rtl);
            return dx * dx + dy * dy <= rtl * rtl;
        }
        if px >= x + box_w - rtr && py <= y + rtr {
            let dx = px - (x + box_w - rtr);
            let dy = py - (y + rtr);
            return dx * dx + dy * dy <= rtr * rtr;
        }
        if px >= x + box_w - rbr && py >= y + box_h - rbr {
            let dx = px - (x + box_w - rbr);
            let dy = py - (y + box_h - rbr);
            return dx * dx + dy * dy <= rbr * rbr;
        }
        if px <= x + rbl && py >= y + box_h - rbl {
            let dx = px - (x + rbl);
            let dy = py - (y + box_h - rbl);
            return dx * dx + dy * dy <= rbl * rbl;
        }
        true
    };

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

    // Optional rectangular scissor (e.g. the CSS background painting area) so
    // a background tile with explicit size/position cannot spill outside the
    // element it decorates.
    let (scissor_min_x, scissor_min_y, scissor_max_x, scissor_max_y) =
        if let Some((sx, sy, sw, sh)) = scissor {
            (
                sx.floor().max(0.0) as u32,
                sy.floor().max(0.0) as u32,
                (sx + sw).ceil().min(pm_w as f32) as u32,
                (sy + sh).ceil().min(pm_h as f32) as u32,
            )
        } else {
            (0, 0, pm_w, pm_h)
        };
    let min_x = min_x.max(scissor_min_x);
    let max_x = max_x.min(scissor_max_x);
    let min_y = min_y.max(scissor_min_y);
    let max_y = max_y.min(scissor_max_y);

    // Calculate image scaling based on object-fit. SVG sources without an
    // explicit author `object-fit` use their own `preserveAspectRatio` behavior.
    let effective_fit = effective_image_object_fit(
        object_fit,
        object_fit_specified,
        img.svg_preserve_aspect_ratio,
    );
    let img_aspect = src_w / src_h;
    let box_aspect = box_w / box_h;

    let (scale_x, scale_y, offset_x, offset_y) = match effective_fit {
        incognidium_style::ObjectFit::Fill => {
            // Stretch to fill box
            (box_w / src_w, box_h / src_h, 0.0, 0.0)
        }
        incognidium_style::ObjectFit::Contain => {
            // Scale to fit within box, preserving aspect ratio
            let scale = if img_aspect > box_aspect {
                box_w / src_w
            } else {
                box_h / src_h
            };
            let scaled_w = src_w * scale;
            let scaled_h = src_h * scale;
            // Position based on object-position (0-1 percentages)
            let off_x = (box_w - scaled_w) * object_position.0;
            let off_y = (box_h - scaled_h) * object_position.1;
            (scale, scale, off_x, off_y)
        }
        incognidium_style::ObjectFit::Cover => {
            // Scale to cover entire box, preserving aspect ratio
            let scale = if img_aspect < box_aspect {
                box_w / src_w
            } else {
                box_h / src_h
            };
            let scaled_w = src_w * scale;
            let scaled_h = src_h * scale;
            // Position based on object-position (0-1 percentages)
            let off_x = (box_w - scaled_w) * object_position.0;
            let off_y = (box_h - scaled_h) * object_position.1;
            (scale, scale, off_x, off_y)
        }
        incognidium_style::ObjectFit::None => {
            // Show at original size (1:1 pixel mapping)
            let off_x = (box_w - src_w) * object_position.0;
            let off_y = (box_h - src_h) * object_position.1;
            (1.0, 1.0, off_x, off_y)
        }
        incognidium_style::ObjectFit::ScaleDown => {
            // Like contain, but never scale up
            let scale = if img_aspect > box_aspect {
                box_w / src_w
            } else {
                box_h / src_h
            };
            let scale = scale.min(1.0); // Never scale up
            let scaled_w = src_w * scale;
            let scaled_h = src_h * scale;
            let off_x = (box_w - scaled_w) * object_position.0;
            let off_y = (box_h - scaled_h) * object_position.1;
            (scale, scale, off_x, off_y)
        }
    };

    // Inverse ratios for mapping from destination to source
    let sx_ratio = 1.0 / scale_x;
    let sy_ratio = 1.0 / scale_y;
    // The raster buffer may be higher resolution than the intrinsic size (e.g.
    // SVGs decoded at 2x), so map from intrinsic source coordinates back to
    // raster pixel coordinates before sampling.
    let raster_scale_x = img.width as f32 / src_w;
    let raster_scale_y = img.height as f32 / src_h;
    let iw = img.width as i32;
    let ih = img.height as i32;

    // Portion of the destination box that the fitted image actually occupies.
    // For `contain`/`scale-down`/`none` the image may be smaller than the box;
    // pixels outside that region must stay transparent so the element's
    // background-color shows through the letterbox bars.
    let scaled_w = src_w * scale_x;
    let scaled_h = src_h * scale_y;
    let image_left = x + offset_x;
    let image_top = y + offset_y;
    let image_right = image_left + scaled_w;
    let image_bottom = image_top + scaled_h;

    // Small icons (especially SVG logos and sprite icons) are commonly
    // rasterized at 10-32 px. Bilinear blending for those icons -- when they are
    // drawn near 1:1 or just positioned at sub-pixel coordinates -- mixes their
    // edges with neighboring pixels, producing gray/dark artifacts and washing
    // out fine strokes. Switch to nearest-neighbor when both the source and
    // destination are small and the raster is not being significantly
    // downscaled, so crisp icon edges stay crisp. If the raster buffer is much
    // larger than the destination box (e.g. a supersampled tiny SVG or a 32 px
    // icon downscaled by CSS background-size), keep bilinear so the extra pixels
    // are averaged into a clean shape instead of discarded by nearest-neighbor.
    let effective_image_rendering = if image_rendering != incognidium_style::ImageRendering::Auto {
        image_rendering
    } else if (img.intrinsic_width <= 32 || img.intrinsic_height <= 32)
        && (box_w <= 32.0 || box_h <= 32.0)
        && box_w >= img.width as f32 * 0.8
        && box_h >= img.height as f32 * 0.8
    {
        incognidium_style::ImageRendering::CrispEdges
    } else {
        incognidium_style::ImageRendering::Auto
    };

    for py in min_y..max_y {
        for px in min_x..max_x {
            // Map destination pixel back to source space using inverse transform
            let (src_x, src_y) = transform_xy(&inverse, px as f32, py as f32);

            // Check if the source position is within the original box
            if src_x < x || src_x >= x + box_w || src_y < y || src_y >= y + box_h {
                continue;
            }

            // Apply border-radius clipping (only for identity transforms).
            if has_radius && !in_rounded_rect(px as f32, py as f32) {
                continue;
            }

            // Skip pixels that fall outside the fitted image area (e.g. the
            // transparent letterbox bars around a `contain`-fitted SVG). This
            // keeps the element's background-color visible behind the image.
            if src_x < image_left - 0.001
                || src_x >= image_right + 0.001
                || src_y < image_top - 0.001
                || src_y >= image_bottom + 0.001
            {
                continue;
            }

            // Map to intrinsic image coordinates (accounting for object-fit
            // offset), then scale to the raster buffer's pixel grid.
            let fx = ((src_x - x - offset_x + 0.5) * sx_ratio - 0.5) * raster_scale_x;
            let fy = ((src_y - y - offset_y + 0.5) * sy_ratio - 0.5) * raster_scale_y;

            // Sample based on image-rendering mode
            let (r, g, b, a) = match effective_image_rendering {
                ImageRendering::Pixelated | ImageRendering::CrispEdges => {
                    // Nearest neighbor sampling for pixelated/crisp-edges
                    let sx = (fx + 0.5).floor() as i32;
                    let sy = (fy + 0.5).floor() as i32;
                    let cx = sx.clamp(0, iw - 1) as u32;
                    let cy = sy.clamp(0, ih - 1) as u32;
                    let i = ((cy * img.width + cx) * 4) as usize;
                    if i + 3 < img.pixels.len() {
                        (
                            img.pixels[i],
                            img.pixels[i + 1],
                            img.pixels[i + 2],
                            img.pixels[i + 3],
                        )
                    } else {
                        (0, 0, 0, 0)
                    }
                }
                _ => {
                    // Bilinear sampling for Auto, Smooth, HighQuality
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
                    (r, g, b, a)
                }
            };

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

/// Apply CSS backdrop-filter to a region.
/// This captures what's already been drawn behind the element and applies filters to it.
fn apply_backdrop_filter(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    filters: &[incognidium_style::Filter],
    clip_path: Option<&Path>,
) {
    // Convert region to integer bounds
    let x0 = x.max(0.0) as u32;
    let y0 = y.max(0.0) as u32;
    let x1 = ((x + width).min(pixmap.width() as f32)) as u32;
    let y1 = ((y + height).min(pixmap.height() as f32)) as u32;

    if x0 >= x1 || y0 >= y1 {
        return;
    }

    // For now, only implement blur and brightness which are most common
    // Extract the region and apply filters
    let pm_width = pixmap.width();
    let pm_height = pixmap.height();

    // Build filter parameters (same as apply_filters_to_region)
    let mut blur_radius: f32 = 0.0;
    let mut brightness: f32 = 1.0;
    let mut contrast: f32 = 1.0;
    let mut grayscale: f32 = 0.0;
    let mut saturate: f32 = 1.0;
    let mut sepia: f32 = 0.0;

    for filter in filters {
        match filter {
            incognidium_style::Filter::Blur(v) => blur_radius = blur_radius.max(*v),
            incognidium_style::Filter::Brightness(v) => brightness *= v,
            incognidium_style::Filter::Contrast(v) => contrast *= v,
            incognidium_style::Filter::Grayscale(v) => grayscale = grayscale.max(*v),
            incognidium_style::Filter::Saturate(v) => saturate *= v,
            incognidium_style::Filter::Sepia(v) => sepia = sepia.max(*v),
            _ => {}
        }
    }

    // Simple blur implementation using box blur
    if blur_radius > 0.5 {
        apply_blur_to_region(pixmap, x0, y0, x1, y1, blur_radius, clip_path);
    }

    // Apply color filters
    if brightness != 1.0 || contrast != 1.0 || grayscale > 0.0 || saturate != 1.0 || sepia > 0.0 {
        apply_color_filters_to_region(
            pixmap,
            x0,
            y0,
            x1,
            y1,
            &[
                ("brightness", brightness),
                ("contrast", contrast),
                ("grayscale", grayscale),
                ("saturate", saturate),
                ("sepia", sepia),
            ],
            clip_path,
        );
    }
}

/// Simple box blur implementation for backdrop-filter
fn apply_blur_to_region(
    pixmap: &mut Pixmap,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    radius: f32,
    _clip_path: Option<&Path>,
) {
    let pm_width = pixmap.width();
    let pm_height = pixmap.height();
    let radius_i = radius.ceil() as i32;

    if radius_i <= 0 {
        return;
    }

    // Create a copy of the region for reading
    let mut temp_buffer: Vec<u8> = pixmap.data().to_vec();

    // Simple box blur - horizontal pass then vertical pass
    for py in y0..y1 {
        for px in x0..x1 {
            let mut r_sum: u32 = 0;
            let mut g_sum: u32 = 0;
            let mut b_sum: u32 = 0;
            let mut a_sum: u32 = 0;
            let mut count: u32 = 0;

            // Sample pixels in blur radius
            for dy in -radius_i..=radius_i {
                for dx in -radius_i..=radius_i {
                    let sx = (px as i32 + dx).clamp(0, pm_width as i32 - 1) as u32;
                    let sy = (py as i32 + dy).clamp(0, pm_height as i32 - 1) as u32;
                    let idx = ((sy * pm_width + sx) * 4) as usize;

                    if idx + 3 < temp_buffer.len() {
                        r_sum += temp_buffer[idx] as u32;
                        g_sum += temp_buffer[idx + 1] as u32;
                        b_sum += temp_buffer[idx + 2] as u32;
                        a_sum += temp_buffer[idx + 3] as u32;
                        count += 1;
                    }
                }
            }

            if count > 0 {
                let idx = ((py * pm_width + px) * 4) as usize;
                let data = pixmap.data_mut();
                data[idx] = (r_sum / count) as u8;
                data[idx + 1] = (g_sum / count) as u8;
                data[idx + 2] = (b_sum / count) as u8;
                data[idx + 3] = (a_sum / count) as u8;
            }
        }
    }
}

/// Apply color filters (brightness, contrast, etc.) to a region
fn apply_color_filters_to_region(
    pixmap: &mut Pixmap,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    filters: &[(&str, f32)],
    _clip_path: Option<&Path>,
) {
    let pm_width = pixmap.width();
    let data = pixmap.data_mut();

    // Parse filter values
    let mut brightness = 1.0;
    let mut contrast = 1.0;
    let mut grayscale = 0.0;
    let mut saturate = 1.0;
    let mut sepia = 0.0;

    for (name, value) in filters {
        match *name {
            "brightness" => brightness = *value,
            "contrast" => contrast = *value,
            "grayscale" => grayscale = *value,
            "saturate" => saturate = *value,
            "sepia" => sepia = *value,
            _ => {}
        }
    }

    for py in y0..y1 {
        for px in x0..x1 {
            let idx = ((py * pm_width + px) * 4) as usize;
            if idx + 3 >= data.len() {
                continue;
            }

            let mut r = data[idx] as f32 / 255.0;
            let mut g = data[idx + 1] as f32 / 255.0;
            let mut b = data[idx + 2] as f32 / 255.0;
            let a = data[idx + 3] as f32 / 255.0;

            if a == 0.0 {
                continue;
            }

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
                r = ((r - 0.5) * contrast + 0.5).clamp(0.0, 1.0);
                g = ((g - 0.5) * contrast + 0.5).clamp(0.0, 1.0);
                b = ((b - 0.5) * contrast + 0.5).clamp(0.0, 1.0);
            }

            data[idx] = (r * 255.0) as u8;
            data[idx + 1] = (g * 255.0) as u8;
            data[idx + 2] = (b * 255.0) as u8;
            // Preserve alpha
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
        60.0 * ((r - g) / diff) + 240.0
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

/// Convert RGB to HSL color space.
fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let diff = max - min;

    let l = (max + min) * 0.5;

    let s = if diff == 0.0 {
        0.0
    } else {
        diff / (1.0 - (2.0 * l - 1.0).abs())
    };

    let h = if diff == 0.0 {
        0.0
    } else if max == r {
        (60.0 * ((g - b) / diff) + 360.0) % 360.0
    } else if max == g {
        (60.0 * ((b - r) / diff) + 120.0)
    } else {
        60.0 * ((r - g) / diff) + 240.0
    };

    (h / 360.0, s, l)
}

/// Convert HSL to RGB color space.
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let h_deg = h * 360.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h_deg / 60.0) % 2.0 - 1.0).abs());
    let m = l - c * 0.5;

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
    clip_mask: Option<&ClipMask>,
    text_align_offsets: Option<&[f32]>,
) -> f32 {
    // Soft hyphens are invisible unless a line break occurs at one; layout
    // converts broken soft hyphens into real hyphens, so any remaining ones
    // should not be painted.
    let normalized;
    let text = if text.contains('\u{00ad}') {
        normalized = text.replace('\u{00ad}', "");
        &normalized
    } else {
        text
    };
    if let Some(fonts) = get_fonts() {
        draw_text_ttf(
            pixmap,
            x,
            y,
            max_width,
            max_height,
            text,
            style,
            fonts,
            clip_mask,
            text_align_offsets,
        )
    } else {
        draw_text_bitmap(
            pixmap,
            x,
            y,
            max_width,
            max_height,
            text,
            style,
            clip_mask,
            text_align_offsets,
        )
    }
}

/// Recolor an image by replacing every pixel's color with `tint` while
/// keeping its alpha. This renders CSS `mask-image` glyphs: the mask's alpha
/// selects where the element's background color shows through.
fn tint_image(img: &ImageData, tint: incognidium_style::CssColor) -> ImageData {
    let mut pixels = img.pixels.clone();
    for px in pixels.chunks_exact_mut(4) {
        px[0] = tint.r;
        px[1] = tint.g;
        px[2] = tint.b;
        px[3] = ((px[3] as u16 * tint.a as u16) / 255) as u8;
    }
    ImageData {
        pixels,
        width: img.width,
        height: img.height,
        intrinsic_width: img.intrinsic_width,
        intrinsic_height: img.intrinsic_height,
        svg_preserve_aspect_ratio: img.svg_preserve_aspect_ratio,
    }
}

/// TTF text rendering with anti-aliased glyphs via fontdue.
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
    clip_mask: Option<&ClipMask>,
    text_align_offsets: Option<&[f32]>,
) -> f32 {
    let base_font_size = style.font_size;
    let adjusted_font_size = if let Some(aspect_value) = style.font_size_adjust {
        let default_aspect = 0.5;
        base_font_size * (aspect_value / default_aspect)
    } else {
        base_font_size
    };

    let font_size = adjusted_font_size;
    // CSS numeric weights 500 and up are semantically heavier than normal and
    // should be rendered with the available bold face.
    let bold = matches!(style.font_weight, FontWeight::Bold | FontWeight::Bolder)
        || matches!(style.font_weight, FontWeight::Number(n) if n >= 500);
    let italic = style.font_style == FontStyle::Italic;
    // A registered @font-face matching the element's named family wins over
    // the built-in fallback fonts so glyphs land where measurement said they
    // would. Characters the web font does not cover (e.g. CJK) fall back to
    // the per-char selection below.
    let web_font = style
        .web_font_family
        .as_deref()
        .and_then(|fam| get_webfont_font(fam, font_weight_number(&style.font_weight), italic));
    let font: &FontdueFont = match web_font.as_ref() {
        Some(wf) => wf,
        None => pick_font(style, bold, italic, style.font_family),
    };
    let pick_char = |c: char| -> &FontdueFont {
        if let Some(ref wf) = web_font {
            if wf.lookup_glyph_index(c) != 0 {
                return wf;
            }
        }
        pick_font_for_char(style, c, bold, italic, style.font_family)
    };
    let line_height = style.effective_line_height();
    let color = style.color;

    let ascent = font_due_ascent(font, font_size);
    let space_width =
        font_due_space_width(font, font_size, style.word_spacing, style.letter_spacing);
    let letter_spacing = style.letter_spacing;

    let mut cursor_x = x;
    let mut cursor_y = y;
    let mut ellipsis_to_render: Option<(f32, f32, f32)> = None;

    if text.starts_with(' ') {
        cursor_x += space_width;
    }

    let lines: Vec<&str> = text.split('\n').collect();
    let mut rendered_end_x = cursor_x;
    let mut all_word_positions: Vec<(f32, f32, f32, bool)> = Vec::new();
    let mut line_offset: f32 = text_align_offsets
        .and_then(|o| o.get(0).copied())
        .unwrap_or(0.0);
    let mut overall_visual_right: f32 = x;

    for (li, line) in lines.iter().enumerate() {
        if li > 0 {
            line_offset = text_align_offsets
                .and_then(|o| o.get(li).copied())
                .unwrap_or(0.0);
            cursor_x = x + line_offset;
            cursor_y += line_height;
        } else {
            cursor_x += line_offset;
        }

        // Clip overflowing lines vertically for fixed-height form controls such
        // as textarea with a rows attribute. A zero max_height means no clip.
        if max_height > 0.0 && cursor_y + line_height > y + max_height + 0.5 {
            break;
        }

        let mut visual_right = cursor_x;
        if line.is_empty() {
            continue;
        }

        let nowrap = matches!(style.white_space, WhiteSpace::NoWrap | WhiteSpace::Pre);

        let (words, space_counts): (Vec<&str>, Vec<usize>) = if nowrap {
            (vec![line], vec![0])
        } else {
            let parts: Vec<&str> = line.split(' ').collect();
            let mut words = Vec::new();
            let mut space_counts = Vec::new();
            let mut space_buffer = 0;
            for part in parts {
                if part.is_empty() {
                    space_buffer += 1;
                } else {
                    words.push(part);
                    space_counts.push(space_buffer.max(1));
                    space_buffer = 0;
                }
            }
            (words, space_counts)
        };
        let line_start_x = cursor_x;

        // For single-line nowrap text with text-overflow: ellipsis, first measure
        // the whole line using the same advances the render loop will use. If the
        // line fits without an ellipsis, don't reserve ellipsis width while
        // rendering each glyph. This avoids turning "Search" into "Sear..." when
        // the container is sized exactly to the text width.
        let line_fits =
            if nowrap && style.text_overflow == incognidium_style::TextOverflow::Ellipsis {
                let mut measure_x = cursor_x;
                let mut prev = None;
                for word in words.iter() {
                    for ch in word.chars() {
                        let cfont = pick_char(ch);
                        if let Some(p) = prev {
                            measure_x += font_due_kern(cfont, p, ch, font_size);
                        }
                        measure_x += font_due_advance(cfont, ch, font_size) + letter_spacing;
                        prev = Some(ch);
                    }
                    if letter_spacing > 0.0 && !word.is_empty() {
                        measure_x -= letter_spacing;
                    }
                }
                measure_x - cursor_x <= max_width
            } else {
                false
            };

        for (wi, word) in words.iter().enumerate() {
            // text-combine-upright only takes effect in vertical writing modes;
            // in horizontal-tb it is treated as none.
            let should_combine = style.text_combine_upright != TextCombineUpright::None
                && style.writing_mode != WritingMode::HorizontalTb;
            let combine_digits_only = style.text_combine_upright == TextCombineUpright::Digits;

            let word_width: f32 = if should_combine && !combine_digits_only {
                font_due_advance(font, 'W', font_size)
            } else {
                word.chars()
                    .map(|c| {
                        let cfont = pick_char(c);
                        font_due_advance(cfont, c, font_size) + letter_spacing
                    })
                    .sum::<f32>()
                    - if word.chars().count() > 0 {
                        letter_spacing
                    } else {
                        0.0
                    }
            };

            let has_descenders = word
                .chars()
                .any(|c| matches!(c, 'g' | 'j' | 'p' | 'q' | 'y' | 'Q'));
            all_word_positions.push((cursor_y, cursor_x, word_width, has_descenders));

            let mut prev_char = None;
            let mut should_stop = false;

            let is_small_caps = style.font_variant == incognidium_style::FontVariant::SmallCaps;
            let small_caps_font_size = font_size * 0.75;

            let combine_scale_factor: f32 = if should_combine && !combine_digits_only {
                let char_width = font_due_advance(font, 'W', font_size);
                let total_width: f32 = word
                    .chars()
                    .map(|c| font_due_advance(font, c, font_size))
                    .sum();
                if total_width > 0.0 {
                    (char_width / total_width).min(1.0)
                } else {
                    1.0
                }
            } else if should_combine
                && combine_digits_only
                && word.chars().all(|c| c.is_ascii_digit())
            {
                let char_width = font_due_advance(font, '0', font_size);
                let total_width: f32 = word
                    .chars()
                    .map(|c| font_due_advance(font, c, font_size))
                    .sum();
                if total_width > 0.0 {
                    (char_width / total_width).min(1.0)
                } else {
                    1.0
                }
            } else {
                1.0
            };

            for ch in word.chars() {
                if ch == '\t' {
                    let tab_size = style.tab_size.max(1) as f32;
                    let tab_width = space_width * tab_size;
                    let current_offset = cursor_x - line_start_x;
                    let tab_stop = ((current_offset / tab_width).floor() + 1.0) * tab_width;
                    let tab_advance = tab_stop - current_offset;
                    cursor_x += tab_advance;
                    prev_char = None;
                    continue;
                }

                let (render_char, use_small_caps) = if is_small_caps && ch.is_lowercase() {
                    (ch.to_uppercase().next().unwrap_or(ch), true)
                } else {
                    (ch, false)
                };

                let glyph_font_size = if use_small_caps {
                    small_caps_font_size
                } else if combine_scale_factor != 1.0 {
                    font_size * combine_scale_factor
                } else {
                    font_size
                };

                let char_font = pick_char(render_char);
                let glyph_ascent = font_due_ascent(char_font, glyph_font_size);

                if let Some(prev) = prev_char {
                    cursor_x += font_due_kern(char_font, prev, render_char, glyph_font_size);
                }

                let glyph_metrics = char_font.metrics(render_char, glyph_font_size);
                let glyph_width = glyph_metrics.advance_width;
                let glyph_right_edge =
                    cursor_x + glyph_metrics.bounds.xmin + glyph_metrics.width as f32;
                visual_right = visual_right.max(glyph_right_edge);

                let ellipsis_width: f32 =
                    if style.text_overflow == TextOverflow::Ellipsis && !line_fits {
                        ['.', '.', '.']
                            .iter()
                            .map(|c| font_due_advance(font, *c, font_size))
                            .sum()
                    } else {
                        0.0
                    };

                // The ellipsis itself must remain inside the content box: stop
                // as soon as this glyph plus the trailing ellipsis would not
                // fit, so the `…` ends at the box edge instead of spilling
                // past it where a later sibling box covers it.
                let would_overflow = cursor_x + glyph_width + ellipsis_width > x + max_width + 2.0;
                if max_width > 0.0 && would_overflow {
                    if style.text_overflow == TextOverflow::Ellipsis {
                        ellipsis_to_render = Some((cursor_x, cursor_y, ellipsis_width));
                        should_stop = true;
                        break;
                    }
                }

                // Color-emoji fallback fonts store bitmap glyphs (PNG in CBDT)
                // that fontdue cannot rasterize as outlines. Extract the PNG
                // via ttf-parser, scale it to the requested font size, and
                // blend it directly onto the pixmap at the glyph origin.
                if is_emoji_fallback_font(char_font) {
                    if let Some((_, face)) = emoji_fonts().as_ref() {
                        let baseline_y = cursor_y + glyph_ascent;
                        draw_emoji_glyph(
                            pixmap,
                            face,
                            render_char,
                            glyph_font_size,
                            cursor_x,
                            baseline_y,
                            clip_mask,
                        );
                        visual_right = visual_right.max(cursor_x + glyph_width);
                        cursor_x += glyph_width + letter_spacing;
                        prev_char = Some(render_char);
                        continue;
                    }
                }

                // Text shadow
                if let Some(shadow) = style.text_shadow {
                    let shadow_color = shadow.color;
                    let blur_radius = shadow.blur_radius;
                    if blur_radius > 0.0 {
                        let steps = (blur_radius * 0.3).max(2.0).min(6.0) as i32;
                        let step_size = blur_radius / steps as f32;
                        let mut total_weight = 0.0f32;
                        for sx in -steps..=steps {
                            for sy in -steps..=steps {
                                let offset_x = sx as f32 * step_size;
                                let offset_y = sy as f32 * step_size;
                                let dist_sq = offset_x * offset_x + offset_y * offset_y;
                                let blur_dist = blur_radius * 0.5;
                                let falloff = (-dist_sq / (2.0 * blur_dist * blur_dist)).exp();
                                if falloff >= 0.01 {
                                    total_weight += falloff;
                                }
                            }
                        }
                        for sx in -steps..=steps {
                            for sy in -steps..=steps {
                                let offset_x = sx as f32 * step_size;
                                let offset_y = sy as f32 * step_size;
                                let dist_sq = offset_x * offset_x + offset_y * offset_y;
                                let blur_dist = blur_radius * 0.5;
                                let falloff = (-dist_sq / (2.0 * blur_dist * blur_dist)).exp();
                                if falloff < 0.01 {
                                    continue;
                                }
                                let normalized_alpha =
                                    shadow_color.a as f32 * falloff / total_weight.max(1.0);
                                let sample_x = cursor_x + shadow.offset_x + offset_x;
                                let sample_y = cursor_y + glyph_ascent + shadow.offset_y + offset_y;
                                draw_glyph_due(
                                    pixmap,
                                    char_font,
                                    render_char,
                                    glyph_font_size,
                                    sample_x,
                                    sample_y,
                                    shadow_color.r,
                                    shadow_color.g,
                                    shadow_color.b,
                                    normalized_alpha as u8,
                                    clip_mask,
                                );
                            }
                        }
                    } else {
                        let shadow_x = cursor_x + shadow.offset_x;
                        let shadow_y = cursor_y + glyph_ascent + shadow.offset_y;
                        draw_glyph_due(
                            pixmap,
                            char_font,
                            render_char,
                            glyph_font_size,
                            shadow_x,
                            shadow_y,
                            shadow_color.r,
                            shadow_color.g,
                            shadow_color.b,
                            shadow_color.a,
                            clip_mask,
                        );
                    }
                }

                // WebKit text stroke
                let stroke_width = style.webkit_text_stroke_width;
                if stroke_width > 0.0 {
                    let stroke_color =
                        style
                            .webkit_text_stroke_color
                            .unwrap_or(incognidium_style::CssColor {
                                r: 0,
                                g: 0,
                                b: 0,
                                a: 255,
                            });
                    let stroke_scale_factor = 1.0 + (stroke_width / font_size);
                    let stroke_font_size = font_size * stroke_scale_factor;
                    let scale_diff = font_size * (stroke_scale_factor - 1.0);
                    let center_offset_x = -scale_diff * 0.3;
                    let center_offset_y = -scale_diff * 0.5;
                    let stroke_x = cursor_x + center_offset_x;
                    let stroke_y = cursor_y + glyph_ascent + center_offset_y;
                    draw_glyph_due(
                        pixmap,
                        char_font,
                        render_char,
                        stroke_font_size,
                        stroke_x,
                        stroke_y,
                        stroke_color.r,
                        stroke_color.g,
                        stroke_color.b,
                        stroke_color.a,
                        clip_mask,
                    );
                }

                // Main glyph
                draw_glyph_due(
                    pixmap,
                    char_font,
                    render_char,
                    glyph_font_size,
                    cursor_x,
                    cursor_y + glyph_ascent,
                    color.r,
                    color.g,
                    color.b,
                    color.a,
                    clip_mask,
                );

                // Text emphasis marks
                if style.text_emphasis_style != TextEmphasisStyle::None
                    && !render_char.is_whitespace()
                {
                    let emphasis_char = match style.text_emphasis_style {
                        TextEmphasisStyle::Dot => '•',
                        TextEmphasisStyle::Circle => '○',
                        TextEmphasisStyle::DoubleCircle => '◎',
                        TextEmphasisStyle::Triangle => '▲',
                        TextEmphasisStyle::Sesame => '、',
                        TextEmphasisStyle::Filled => '●',
                        TextEmphasisStyle::Open => '○',
                        _ => '\0',
                    };
                    if emphasis_char != '\0' {
                        let emphasis_color = style.text_emphasis_color.unwrap_or(color);
                        // Render emphasis marks at about half the text size and
                        // center them above (over) or below (under) each glyph,
                        // matching browser typography rather than drawing them as
                        // full-size inline characters.
                        let emphasis_size = font_size * 0.55;
                        let emphasis_font = pick_char(emphasis_char);
                        let emphasis_metrics = emphasis_font.metrics(emphasis_char, emphasis_size);
                        let emphasis_width = emphasis_metrics.advance_width;
                        let emphasis_x = cursor_x + (glyph_width - emphasis_width) / 2.0;
                        let emphasis_half_height = emphasis_metrics.bounds.height * 0.5;
                        let emphasis_center_offset =
                            emphasis_metrics.bounds.ymin + emphasis_half_height;
                        // Place the mark just outside the line box: over marks sit
                        // above the ascenders, under marks sit below the baseline.
                        let target_center_y =
                            if style.text_emphasis_position == TextEmphasisPosition::Under {
                                cursor_y + glyph_ascent + emphasis_size * 0.75
                            } else {
                                cursor_y - emphasis_size * 0.75
                            };
                        let emphasis_y = target_center_y + emphasis_center_offset;
                        draw_glyph_due(
                            pixmap,
                            emphasis_font,
                            emphasis_char,
                            emphasis_size,
                            emphasis_x,
                            emphasis_y,
                            emphasis_color.r,
                            emphasis_color.g,
                            emphasis_color.b,
                            emphasis_color.a,
                            clip_mask,
                        );
                    }
                }

                cursor_x += glyph_width + letter_spacing;
                prev_char = Some(render_char);
            }

            if letter_spacing > 0.0 && !word.is_empty() {
                cursor_x -= letter_spacing;
            }
            rendered_end_x = cursor_x;

            if wi < words.len() - 1 {
                let num_spaces = if nowrap {
                    1
                } else {
                    space_counts.get(wi).copied().unwrap_or(1)
                };
                let space_total = space_width * num_spaces as f32;
                let nominal = cursor_x + space_total;
                // If the previous glyph's bounding box extends past its advance,
                // ensure the next glyph starts a full word-space away from that
                // visual right edge rather than from the nominal origin. This keeps
                // small space advances from visually collapsing when the fallback
                // font has tight or negative side bearings.
                let next_left_edge = words[wi + 1]
                    .chars()
                    .next()
                    .map(|nc| {
                        let nf = pick_char(nc);
                        nf.metrics(nc, font_size).bounds.xmin
                    })
                    .unwrap_or(0.0);
                let adjusted = visual_right + space_total - next_left_edge;
                cursor_x = nominal.max(adjusted);
            }
            overall_visual_right = overall_visual_right.max(visual_right);
        }
    }

    // Text decorations
    use incognidium_style::{TextDecoration, TextDecorationLine};

    // The line longhand is authoritative: the legacy `text_decoration` field is
    // a mirror set by the shorthand, and a later `text-decoration-line: none`
    // must be able to turn a link's underline off.
    let has_underline = style.text_decoration_line == TextDecorationLine::Underline;
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
            let line_thickness = match style.text_decoration_thickness {
                incognidium_style::TextDecorationThickness::Length(px) => px,
                incognidium_style::TextDecorationThickness::FromFont => {
                    1.0_f32.max(font_size * 0.05)
                }
                incognidium_style::TextDecorationThickness::Auto => 1.0_f32.max(font_size * 0.05),
            };
            let decor_color = style.text_decoration_color.unwrap_or(color);
            let decor_style = style.text_decoration_style;

            if has_underline {
                let underline_offset = style.text_underline_offset.unwrap_or(2.0);
                let ul_y = match style.text_underline_position {
                    incognidium_style::TextUnderlinePosition::Under => {
                        y + ascent + font_size * 0.25 + underline_offset
                    }
                    _ => y + ascent + underline_offset,
                };

                let skip_ink = style.text_decoration_skip_ink
                    == incognidium_style::TextDecorationSkipInk::Auto;
                if skip_ink {
                    draw_underline_with_skip_ink(
                        pixmap,
                        ul_y,
                        line_thickness,
                        decor_color,
                        decor_style,
                        &all_word_positions,
                        font_size,
                        space_width,
                        font,
                    );
                } else {
                    draw_text_decoration_line(
                        pixmap,
                        decor_x,
                        ul_y,
                        decor_w,
                        line_thickness,
                        decor_color,
                        decor_style,
                    );
                }
            }

            if has_line_through {
                // Strike through the center of the glyph area rather than near the
                // baseline, matching the visual middle of the line.
                let lt_y = y + ascent * 0.5;
                draw_text_decoration_line(
                    pixmap,
                    decor_x,
                    lt_y,
                    decor_w,
                    line_thickness,
                    decor_color,
                    decor_style,
                );
            }

            if has_overline {
                let ol_y = y + ascent - font_size * 0.85;
                draw_text_decoration_line(
                    pixmap,
                    decor_x,
                    ol_y,
                    decor_w,
                    line_thickness,
                    decor_color,
                    decor_style,
                );
            }
        }
    }

    // Render ellipsis
    if let Some((ex, ey, _)) = ellipsis_to_render {
        let ellipsis_y = ey;
        let mut ellipsis_x = ex;
        let mut prev_ech = None;
        for ech in ['.', '.', '.'] {
            if let Some(prev) = prev_ech {
                ellipsis_x += font_due_kern(font, prev, ech, font_size);
            }
            draw_glyph_due(
                pixmap,
                font,
                ech,
                font_size,
                ellipsis_x,
                ellipsis_y + ascent,
                color.r,
                color.g,
                color.b,
                color.a,
                clip_mask,
            );
            ellipsis_x += font_due_advance(font, ech, font_size);
            prev_ech = Some(ech);
        }
    }
    overall_visual_right
}

fn draw_text_bitmap(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    max_width: f32,
    max_height: f32,
    text: &str,
    style: &ComputedStyle,
    _clip_mask: Option<&ClipMask>,
    text_align_offsets: Option<&[f32]>,
) -> f32 {
    // TODO: apply clip_mask to bitmap fallback glyphs if needed.
    let font_size = style.font_size;
    let char_width = font_size * 0.6;
    let line_height = style.effective_line_height();
    let color = style.color;
    // CSS numeric weights 500 and up are semantically heavier than normal.
    let bold = matches!(style.font_weight, FontWeight::Bold | FontWeight::Bolder)
        || matches!(style.font_weight, FontWeight::Number(n) if n >= 500);

    let mut cursor_x = x;
    let mut cursor_y = y;
    let mut visual_right = x;

    if text.starts_with(' ') {
        cursor_x += char_width;
        visual_right = visual_right.max(cursor_x);
    }

    // Split on newlines first, then whitespace within each line
    let lines: Vec<&str> = text.split('\n').collect();
    let mut line_offset: f32 = text_align_offsets
        .and_then(|o| o.get(0).copied())
        .unwrap_or(0.0);

    for (li, line) in lines.iter().enumerate() {
        if li > 0 {
            // New line
            line_offset = text_align_offsets
                .and_then(|o| o.get(li).copied())
                .unwrap_or(0.0);
            cursor_x = x + line_offset;
            cursor_y += line_height;
        } else {
            cursor_x += line_offset;
        }

        // Clip overflowing lines vertically for fixed-height form controls such
        // as textarea with a rows attribute. A zero max_height means no clip.
        if max_height > 0.0 && cursor_y + line_height > y + max_height + 0.5 {
            break;
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
                // Handle tab characters specially
                if ch == '\t' {
                    let tab_size = style.tab_size.max(1) as f32;
                    let tab_width = char_width * tab_size;
                    let current_offset = cursor_x - line_start_x;
                    let tab_stop = ((current_offset / tab_width).floor() + 1.0) * tab_width;
                    let tab_advance = tab_stop - current_offset;
                    cursor_x += tab_advance;
                    visual_right = visual_right.max(cursor_x);
                    continue;
                }
                // Check if this character would exceed max_width (for overflow:hidden)
                if max_width > 0.0 && cursor_x + char_width > x + max_width + 0.5 {
                    break;
                }
                draw_bitmap_char(pixmap, cursor_x, cursor_y, ch, font_size, color, bold);
                cursor_x += char_width;
                visual_right = visual_right.max(cursor_x);
            }

            if wi < words.len() - 1 {
                cursor_x += char_width;
                visual_right = visual_right.max(cursor_x);
            }
        }
    }

    // Text decorations for bitmap fallback — the line longhand is
    // authoritative, as above.
    let has_underline = style.text_decoration_line == TextDecorationLine::Underline;
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
        // Calculate line thickness based on text-decoration-thickness
        let line_thickness = match style.text_decoration_thickness {
            incognidium_style::TextDecorationThickness::Length(px) => px,
            incognidium_style::TextDecorationThickness::FromFont => 1.0_f32.max(font_size * 0.05),
            incognidium_style::TextDecorationThickness::Auto => 1.0_f32.max(font_size * 0.05),
        };
        let decor_color = style.text_decoration_color.unwrap_or(color);
        let decor_style = style.text_decoration_style;

        if has_underline {
            // Apply text-underline-offset if specified, otherwise default to font_size
            let underline_offset = style.text_underline_offset.unwrap_or(0.0);
            let underline_y = y + font_size + underline_offset;
            draw_text_decoration_line(
                pixmap,
                decor_x,
                underline_y,
                text_width.min(max_width),
                line_thickness,
                decor_color,
                decor_style,
            );
        }

        if has_line_through {
            let lt_y = y + font_size * 0.55;
            draw_text_decoration_line(
                pixmap,
                decor_x,
                lt_y,
                text_width.min(max_width),
                line_thickness,
                decor_color,
                decor_style,
            );
        }

        if has_overline {
            let overline_y = y + font_size * 0.15;
            draw_text_decoration_line(
                pixmap,
                decor_x,
                overline_y,
                text_width.min(max_width),
                line_thickness,
                decor_color,
                decor_style,
            );
        }
    }
    visual_right
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
    object_fit: incognidium_style::ObjectFit,
    object_fit_specified: bool,
    object_position: (f32, f32),
    image_rendering: incognidium_style::ImageRendering,
    radius_tl: incognidium_style::SizeValue,
    radius_tr: incognidium_style::SizeValue,
    radius_br: incognidium_style::SizeValue,
    radius_bl: incognidium_style::SizeValue,
) {
    draw_image_with_transform_and_clip(
        pixmap,
        x,
        y,
        box_w,
        box_h,
        img,
        clip,
        None,
        Transform::identity(),
        object_fit,
        object_fit_specified,
        object_position,
        image_rendering,
        radius_tl,
        radius_tr,
        radius_br,
        radius_bl,
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
    clip_border_radius: Option<(f32, f32, f32, f32)>,
    transform: Transform,
    object_fit: incognidium_style::ObjectFit,
    object_fit_specified: bool,
    object_position: (f32, f32),
    image_rendering: incognidium_style::ImageRendering,
    radius_tl: incognidium_style::SizeValue,
    radius_tr: incognidium_style::SizeValue,
    radius_br: incognidium_style::SizeValue,
    radius_bl: incognidium_style::SizeValue,
) {
    if img.width == 0 || img.height == 0 || box_w <= 0.0 || box_h <= 0.0 {
        return;
    }
    let src_w = img.intrinsic_width.max(1) as f32;
    let src_h = img.intrinsic_height.max(1) as f32;
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

    // Resolve border-radii for clipping. We only apply the rounded clip when the
    // transform is identity because our point-in-rounded-rect test is in local
    // box coordinates.
    let max_radius = (box_w.min(box_h) / 2.0).max(0.0);
    let resolve_radius = |sv: &incognidium_style::SizeValue| -> f32 {
        match sv {
            incognidium_style::SizeValue::Percent(p) => box_w.min(box_h) * p / 100.0,
            incognidium_style::SizeValue::Px(px) => *px,
            _ => 0.0,
        }
        .min(max_radius)
        .max(0.0)
    };
    let has_radius = transform == Transform::identity()
        && (radius_tl != incognidium_style::SizeValue::Px(0.0)
            || radius_tr != incognidium_style::SizeValue::Px(0.0)
            || radius_br != incognidium_style::SizeValue::Px(0.0)
            || radius_bl != incognidium_style::SizeValue::Px(0.0));
    let (rtl, rtr, rbr, rbl) = if has_radius {
        (
            resolve_radius(&radius_tl),
            resolve_radius(&radius_tr),
            resolve_radius(&radius_br),
            resolve_radius(&radius_bl),
        )
    } else {
        (0.0, 0.0, 0.0, 0.0)
    };
    let in_rounded_rect = |px: f32, py: f32| -> bool {
        if px < x || px > x + box_w || py < y || py > y + box_h {
            return false;
        }
        // Top-left corner
        if px <= x + rtl && py <= y + rtl {
            let dx = px - (x + rtl);
            let dy = py - (y + rtl);
            return dx * dx + dy * dy <= rtl * rtl;
        }
        // Top-right corner
        if px >= x + box_w - rtr && py <= y + rtr {
            let dx = px - (x + box_w - rtr);
            let dy = py - (y + rtr);
            return dx * dx + dy * dy <= rtr * rtr;
        }
        // Bottom-right corner
        if px >= x + box_w - rbr && py >= y + box_h - rbr {
            let dx = px - (x + box_w - rbr);
            let dy = py - (y + box_h - rbr);
            return dx * dx + dy * dy <= rbr * rbr;
        }
        // Bottom-left corner
        if px <= x + rbl && py >= y + box_h - rbl {
            let dx = px - (x + rbl);
            let dy = py - (y + box_h - rbl);
            return dx * dx + dy * dy <= rbl * rbl;
        }
        true
    };

    // Ancestor overflow:hidden clips are rectangular by default. If the ancestor
    // also has a border-radius, descendants must be clipped to the rounded padding
    // box of that ancestor. The clip is in screen space, so the rounded test is
    // only reliable under an identity transform.
    let (clip_rtl, clip_rtr, clip_rbr, clip_rbl) = if transform == Transform::identity() {
        clip_border_radius.unwrap_or((0.0, 0.0, 0.0, 0.0))
    } else {
        (0.0, 0.0, 0.0, 0.0)
    };
    let has_clip_radius = clip_rtl > 0.0 || clip_rtr > 0.0 || clip_rbr > 0.0 || clip_rbl > 0.0;
    let clip_bounds = clip.unwrap_or((0.0, 0.0, pm_w as f32, pm_h as f32));
    let in_clip_rounded_rect = |px: f32, py: f32| -> bool {
        let cx = clip_bounds.0;
        let cy = clip_bounds.1;
        let cw = clip_bounds.2;
        let ch = clip_bounds.3;
        if px < cx || px > cx + cw || py < cy || py > cy + ch {
            return false;
        }
        // Top-left corner
        if px <= cx + clip_rtl && py <= cy + clip_rtl {
            let dx = px - (cx + clip_rtl);
            let dy = py - (cy + clip_rtl);
            return dx * dx + dy * dy <= clip_rtl * clip_rtl;
        }
        // Top-right corner
        if px >= cx + cw - clip_rtr && py <= cy + clip_rtr {
            let dx = px - (cx + cw - clip_rtr);
            let dy = py - (cy + clip_rtr);
            return dx * dx + dy * dy <= clip_rtr * clip_rtr;
        }
        // Bottom-right corner
        if px >= cx + cw - clip_rbr && py >= cy + ch - clip_rbr {
            let dx = px - (cx + cw - clip_rbr);
            let dy = py - (cy + ch - clip_rbr);
            return dx * dx + dy * dy <= clip_rbr * clip_rbr;
        }
        // Bottom-left corner
        if px <= cx + clip_rbl && py >= cy + ch - clip_rbl {
            let dx = px - (cx + clip_rbl);
            let dy = py - (cy + ch - clip_rbl);
            return dx * dx + dy * dy <= clip_rbl * clip_rbl;
        }
        true
    };

    // Calculate image scaling based on object-fit. SVG sources without an
    // explicit author `object-fit` use their own `preserveAspectRatio` behavior.
    let effective_fit = effective_image_object_fit(
        object_fit,
        object_fit_specified,
        img.svg_preserve_aspect_ratio,
    );
    let img_aspect = src_w / src_h;
    let box_aspect = box_w / box_h;

    let (scale_x, scale_y, offset_x, offset_y) = match effective_fit {
        incognidium_style::ObjectFit::Fill => {
            // Stretch to fill box
            (box_w / src_w, box_h / src_h, 0.0, 0.0)
        }
        incognidium_style::ObjectFit::Contain => {
            // Scale to fit within box, preserving aspect ratio
            let scale = if img_aspect > box_aspect {
                box_w / src_w
            } else {
                box_h / src_h
            };
            let scaled_w = src_w * scale;
            let scaled_h = src_h * scale;
            let off_x = (box_w - scaled_w) * object_position.0;
            let off_y = (box_h - scaled_h) * object_position.1;
            (scale, scale, off_x, off_y)
        }
        incognidium_style::ObjectFit::Cover => {
            // Scale to cover entire box, preserving aspect ratio
            let scale = if img_aspect < box_aspect {
                box_w / src_w
            } else {
                box_h / src_h
            };
            let scaled_w = src_w * scale;
            let scaled_h = src_h * scale;
            let off_x = (box_w - scaled_w) * object_position.0;
            let off_y = (box_h - scaled_h) * object_position.1;
            (scale, scale, off_x, off_y)
        }
        incognidium_style::ObjectFit::None => {
            // Show at original size
            let off_x = (box_w - src_w) * object_position.0;
            let off_y = (box_h - src_h) * object_position.1;
            (1.0, 1.0, off_x, off_y)
        }
        incognidium_style::ObjectFit::ScaleDown => {
            // Like contain, but never scale up
            let scale = if img_aspect > box_aspect {
                box_w / src_w
            } else {
                box_h / src_h
            };
            let scale = scale.min(1.0);
            let scaled_w = src_w * scale;
            let scaled_h = src_h * scale;
            let off_x = (box_w - scaled_w) * object_position.0;
            let off_y = (box_h - scaled_h) * object_position.1;
            (scale, scale, off_x, off_y)
        }
    };

    let sx_ratio = 1.0 / scale_x;
    let sy_ratio = 1.0 / scale_y;
    let raster_scale_x = img.width as f32 / src_w;
    let raster_scale_y = img.height as f32 / src_h;
    let iw = img.width as i32;
    let ih = img.height as i32;

    // Portion of the destination box that the fitted image actually occupies.
    // Pixels outside the fitted area must stay transparent so the element's
    // background-color shows through letterbox bars.
    let scaled_w = src_w * scale_x;
    let scaled_h = src_h * scale_y;
    let image_left = x + offset_x;
    let image_top = y + offset_y;
    let image_right = image_left + scaled_w;
    let image_bottom = image_top + scaled_h;
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

            // Apply border-radius clipping (only for identity transforms).
            if has_radius && !in_rounded_rect(px as f32, py as f32) {
                continue;
            }

            // Apply ancestor rounded overflow clip.
            if has_clip_radius && !in_clip_rounded_rect(px as f32, py as f32) {
                continue;
            }

            // Skip pixels that fall outside the fitted image area so the
            // element's background-color remains visible in letterbox bars.
            if src_x < image_left - 0.001
                || src_x >= image_right + 0.001
                || src_y < image_top - 0.001
                || src_y >= image_bottom + 0.001
            {
                continue;
            }

            // Map to intrinsic image coordinates (accounting for object-fit
            // offset), then scale to the raster buffer's pixel grid.
            let fx = ((src_x - x - offset_x + 0.5) * sx_ratio - 0.5) * raster_scale_x;
            let fy = ((src_y - y - offset_y + 0.5) * sy_ratio - 0.5) * raster_scale_y;
            // Sample based on image-rendering mode
            use incognidium_style::ImageRendering;
            let (sr, sg, sb, sa): (u32, u32, u32, u32) = match image_rendering {
                ImageRendering::Pixelated | ImageRendering::CrispEdges => {
                    // Nearest neighbor sampling for pixelated/crisp-edges
                    let sx = (fx + 0.5).floor() as i32;
                    let sy = (fy + 0.5).floor() as i32;
                    let cx = sx.clamp(0, iw - 1) as u32;
                    let cy = sy.clamp(0, ih - 1) as u32;
                    let i = ((cy * img.width + cx) * 4) as usize;
                    if i + 3 < img.pixels.len() {
                        (
                            img.pixels[i] as u32,
                            img.pixels[i + 1] as u32,
                            img.pixels[i + 2] as u32,
                            img.pixels[i + 3] as u32,
                        )
                    } else {
                        (0, 0, 0, 0)
                    }
                }
                _ => {
                    // Bilinear sampling for Auto, Smooth, HighQuality
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

                    (
                        mix(p00[0], p10[0], p01[0], p11[0]),
                        mix(p00[1], p10[1], p01[1], p11[1]),
                        mix(p00[2], p10[2], p01[2], p11[2]),
                        mix(p00[3], p10[3], p01[3], p11[3]),
                    )
                }
            };

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
        None,
        None,
        0.0,
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
    clip_mask: Option<&ClipMask>,
    text_align_offsets: Option<&[f32]>,
    x_offset: f32,
) -> f32 {
    // If no transform (or identity), use direct rendering. Pass the rounded
    // overflow clip mask (if any) so text inside a border-radius + overflow:hidden
    // ancestor is clipped to the ancestor's rounded padding box, not just its
    // rectangular bounds.
    let draw_x = x + x_offset;
    if transform == Transform::identity() {
        let visual_right = if let Some((cx, cy, cw, ch)) = clip {
            // When there's a clip (from overflow:hidden on parent), constrain rendering to clip
            // For text-overflow: ellipsis to work, we need to use the clip width as max_width
            // because the text may have natural width > container width with white-space: nowrap
            // But keep the original text position (x, y) - the clip is just for bounds checking
            let eff_w = (draw_x + max_width).min(cx + cw) - draw_x.max(cx);
            let eff_h = (y + max_height).min(cy + ch) - y.max(cy);
            if eff_w > 0.0 && eff_h > 0.0 {
                draw_text(
                    pixmap,
                    draw_x,
                    y,
                    eff_w,
                    eff_h,
                    text,
                    style,
                    clip_mask,
                    text_align_offsets,
                )
            } else {
                draw_x
            }
        } else {
            draw_text(
                pixmap,
                draw_x,
                y,
                max_width,
                max_height,
                text,
                style,
                clip_mask,
                text_align_offsets,
            )
        };
        return visual_right;
    }

    // Calculate the bounds of the text in screen space
    let pm_w = pixmap.width();
    let pm_h = pixmap.height();

    // Transform the four corners of the text area to find screen bounds
    let corners = [
        transform_xy(&transform, draw_x, y),
        transform_xy(&transform, draw_x + max_width, y),
        transform_xy(&transform, draw_x, y + max_height),
        transform_xy(&transform, draw_x + max_width, y + max_height),
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
            if let Some(mask) = clip_mask {
                if !clip_mask_contains(mask, px as f32, py as f32) {
                    continue;
                }
            }

            // Map destination pixel back to source space
            let (src_x, src_y) = transform_xy(&inverse, px as f32, py as f32);

            // Check if within text bounds
            if src_x < draw_x || src_x >= draw_x + max_width || src_y < y || src_y >= y + max_height
            {
                continue;
            }

            // Sample the text at this source position
            // We render a small region around this point to the temp buffer
            let sample_x = (src_x - draw_x).max(0.0);
            let sample_y = (src_y - y).max(0.0);

            // For performance, we'll render a small tile of text and sample from it
            // This is an approximation but works for simple cases
            // A full implementation would track glyph positions and transform each

            // For now, use a simpler approach: check if this pixel would have text
            // by looking at the glyph that would be rendered here
            let color = sample_text_at_position(
                draw_x,
                y,
                sample_x,
                sample_y,
                text,
                style,
                text_align_offsets,
            );

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
    draw_x + max_width
}

/// Sample the text color at a specific position within the text area
fn sample_text_at_position(
    text_x: f32,
    text_y: f32,
    local_x: f32,
    local_y: f32,
    text: &str,
    style: &ComputedStyle,
    text_align_offsets: Option<&[f32]>,
) -> CssColor {
    if get_fonts().is_none() {
        return CssColor::TRANSPARENT;
    }
    let font_size = style.font_size;
    // CSS numeric weights 500 and up are semantically heavier than normal.
    let bold = matches!(style.font_weight, FontWeight::Bold | FontWeight::Bolder)
        || matches!(style.font_weight, FontWeight::Number(n) if n >= 500);
    let italic = style.font_style == FontStyle::Italic;
    let font = pick_font(style, bold, italic, style.font_family);
    let line_height = style.effective_line_height();
    let color = style.color;

    let ascent = font_due_ascent(font, font_size);
    let space_width =
        font_due_space_width(font, font_size, style.word_spacing, style.letter_spacing);
    let letter_spacing = style.letter_spacing;

    // NBSP (U+00A0) is not collapsible in CSS but renders with a space
    // glyph; normalize it so fonts that lack the U+00A0 glyph do not paint
    // tofu for plain `&nbsp;` text.
    let nbsp_owned;
    let text = if text.contains('\u{00a0}') {
        nbsp_owned = text.replace('\u{00a0}', " ");
        &nbsp_owned
    } else {
        text
    };

    let mut cursor_x = text_x;
    let mut cursor_y = text_y;

    if text.starts_with(' ') {
        cursor_x += space_width;
    }

    let lines: Vec<&str> = text.split('\n').collect();
    let mut line_offset: f32 = text_align_offsets
        .and_then(|o| o.get(0).copied())
        .unwrap_or(0.0);

    for (li, line) in lines.iter().enumerate() {
        if li > 0 {
            line_offset = text_align_offsets
                .and_then(|o| o.get(li).copied())
                .unwrap_or(0.0);
            cursor_x = text_x + line_offset;
            cursor_y += line_height;
        } else {
            cursor_x += line_offset;
        }
        if line.is_empty() {
            continue;
        }
        if local_y >= cursor_y - text_y && local_y < cursor_y - text_y + line_height {
            let words: Vec<&str> = line.split_whitespace().collect();

            for word in words.iter() {
                let word_width: f32 = word
                    .chars()
                    .map(|c| {
                        let cfont = pick_font_for_char(style, c, bold, italic, style.font_family);
                        font_due_advance(cfont, c, font_size) + letter_spacing
                    })
                    .sum::<f32>()
                    - if word.chars().count() > 0 {
                        letter_spacing
                    } else {
                        0.0
                    };

                let mut char_x = cursor_x;
                let mut prev_char = None;

                for ch in word.chars() {
                    let char_font = pick_font_for_char(style, ch, bold, italic, style.font_family);
                    let char_ascent = font_due_ascent(char_font, font_size);
                    if let Some(prev) = prev_char {
                        char_x += font_due_kern(char_font, prev, ch, font_size);
                    }
                    let glyph_width = font_due_advance(char_font, ch, font_size);

                    if local_x >= char_x - text_x && local_x < char_x - text_x + glyph_width {
                        let glyph_y = cursor_y + char_ascent;
                        if local_y >= glyph_y - text_y - font_size && local_y < glyph_y - text_y {
                            let (metrics, bitmap) = char_font.rasterize(ch, font_size);
                            let bounds = metrics.bounds;
                            let rel_x = local_x - (char_x + bounds.xmin - text_x);
                            let rel_y = local_y - (glyph_y + bounds.ymin - text_y);
                            if rel_x >= 0.0 && rel_y >= 0.0 {
                                let gx = rel_x as u32;
                                let gy = rel_y as u32;
                                if gx < metrics.width as u32 && gy < metrics.height as u32 {
                                    let idx = (gy as usize) * metrics.width + (gx as usize);
                                    if idx < bitmap.len() {
                                        let coverage = bitmap[idx];
                                        if coverage > 0 {
                                            let alpha = coverage;
                                            return CssColor {
                                                r: color.r,
                                                g: color.g,
                                                b: color.b,
                                                a: alpha,
                                            };
                                        }
                                    }
                                }
                            }
                            return CssColor::TRANSPARENT;
                        }
                    }
                    char_x += glyph_width + letter_spacing;
                    prev_char = Some(ch);
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
    use incognidium_css::CssColor;
    use incognidium_dom::NodeId;
    use incognidium_layout::{BoxType, FlatBox};
    use incognidium_style::{ComputedStyle, MaskRepeat, MaskSize, Position};
    use std::collections::HashMap;

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

    #[test]
    fn test_relative_container_child_paints_on_top() {
        // A relatively positioned container with a static child image must not
        // have its background paint over the image. This regression test ensures
        // in-flow children of relative containers stay in document order.
        let parent_id: NodeId = 1;
        let child_id: NodeId = 2;
        let src = "red-avatar.png".to_string();

        let parent = FlatBox {
            node_id: parent_id,
            x: 0.0,
            y: 0.0,
            width: 32.0,
            height: 32.0,
            box_type: BoxType::Block,
            text: None,
            text_leading_space: false,
            text_trailing_space: false,
            image_src: None,
            link_href: None,
            clip: None,
            clip_path: None,
            clip_border_radius: None,
            float_text_indent: None,
            input_type: None,
            textarea_info: None,
            is_placeholder_text: false,
            marker_color: None,
            marker_font_size: None,
            marker_font_weight: None,
            marker_font_family: None,
            marker_background_color: None,
            is_list_marker: false,
            list_style_position: Default::default(),
            list_marker_is_ordered: false,
            first_letter_len: None,
            first_letter_color: None,
            first_letter_font_size: None,
            first_letter_font_weight: None,
            first_letter_font_family: None,
            first_letter_background_color: None,
            first_letter_text_decoration: None,
            first_letter_text_transform: None,
            first_letter_margin: None,
            first_letter_padding: None,
            first_letter_border_width: None,
            first_letter_border_color: None,
            first_line_has_content: false,
            first_line_color: None,
            first_line_font_size: None,
            first_line_font_weight: None,
            first_line_font_family: None,
            first_line_background_color: None,
            first_line_text_decoration: None,
            first_line_letter_spacing: None,
            first_line_word_spacing: None,
            first_line_text_transform: None,
            collapsed_borders: None,
            hide_empty_cell: false,
            column_count: 0,
            column_width: 0.0,
            column_gap: 0.0,
            column_rule_width: 0.0,
            column_rule_style: Default::default(),
            column_rule_color: CssColor::TRANSPARENT,
            content_x: 0.0,
            content_y: 0.0,
            content_height: 32.0,
            in_fixed_subtree: false,
            in_absolute_subtree: false,
            depth: 0,
            stacking_context_root: None,
            parent_stacking_context: None,
            transform_applied: false,
            opacity: 1.0,
            text_align_offsets: Vec::new(),
        };

        let child = FlatBox {
            node_id: child_id,
            x: 0.0,
            y: 0.0,
            width: 32.0,
            height: 32.0,
            box_type: BoxType::Image,
            text: None,
            text_leading_space: false,
            text_trailing_space: false,
            image_src: Some(src.clone()),
            link_href: None,
            clip: None,
            clip_path: None,
            clip_border_radius: None,
            float_text_indent: None,
            input_type: None,
            textarea_info: None,
            is_placeholder_text: false,
            marker_color: None,
            marker_font_size: None,
            marker_font_weight: None,
            marker_font_family: None,
            marker_background_color: None,
            is_list_marker: false,
            list_style_position: Default::default(),
            list_marker_is_ordered: false,
            first_letter_len: None,
            first_letter_color: None,
            first_letter_font_size: None,
            first_letter_font_weight: None,
            first_letter_font_family: None,
            first_letter_background_color: None,
            first_letter_text_decoration: None,
            first_letter_text_transform: None,
            first_letter_margin: None,
            first_letter_padding: None,
            first_letter_border_width: None,
            first_letter_border_color: None,
            first_line_has_content: false,
            first_line_color: None,
            first_line_font_size: None,
            first_line_font_weight: None,
            first_line_font_family: None,
            first_line_background_color: None,
            first_line_text_decoration: None,
            first_line_letter_spacing: None,
            first_line_word_spacing: None,
            first_line_text_transform: None,
            collapsed_borders: None,
            hide_empty_cell: false,
            column_count: 0,
            column_width: 0.0,
            column_gap: 0.0,
            column_rule_width: 0.0,
            column_rule_style: Default::default(),
            column_rule_color: CssColor::TRANSPARENT,
            content_x: 0.0,
            content_y: 0.0,
            content_height: 32.0,
            in_fixed_subtree: false,
            in_absolute_subtree: false,
            depth: 1,
            stacking_context_root: None,
            parent_stacking_context: None,
            transform_applied: false,
            opacity: 1.0,
            text_align_offsets: Vec::new(),
        };

        let mut styles = StyleMap::new();
        let mut parent_style = ComputedStyle::default();
        parent_style.position = Position::Relative;
        parent_style.background_color = CssColor::from_rgb(120, 118, 111);
        styles.insert(parent_id, parent_style);

        let mut child_style = ComputedStyle::default();
        child_style.position = Position::Static;
        styles.insert(child_id, child_style);

        let mut images: HashMap<String, ImageData> = HashMap::new();
        images.insert(
            src,
            ImageData {
                pixels: vec![
                    255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
                ],
                width: 2,
                height: 2,
                intrinsic_width: 2,
                intrinsic_height: 2,
                svg_preserve_aspect_ratio: None,
            },
        );

        let flat_boxes = vec![parent, child];
        let pixmap = paint_with_images(&flat_boxes, &styles, 32, 32, &images);
        let data = pixmap.data();
        let idx = (16 * 32 + 16) * 4;
        let r = data[idx as usize];
        let g = data[idx as usize + 1];
        let b = data[idx as usize + 2];
        assert!(
            r > 200 && g < 50 && b < 50,
            "Expected child image (red) on top of relative container background, got R={}, G={}, B={}",
            r, g, b
        );
    }

    #[test]
    fn paint_does_not_reapply_transform() {
        let mut style = ComputedStyle::default();
        style.display = incognidium_style::Display::Block;
        style.background_color = incognidium_css::CssColor::from_rgb(255, 0, 0);
        style.transform = vec![incognidium_style::Transform::TranslateX(100.0)];
        let mut styles = incognidium_style::StyleMap::new();
        styles.insert(1, style);

        // Flattening already applied the TranslateX(100) to reach x=50 and
        // marks the box accordingly; paint must not apply it a second time.
        let fbox = FlatBox {
            node_id: 1,
            x: 50.0,
            y: 50.0,
            width: 10.0,
            height: 10.0,
            box_type: BoxType::Block,
            transform_applied: true,
            ..FlatBox::default()
        };

        let pixmap = paint(&[fbox], &styles, 200, 200);
        assert_eq!(pixmap.width(), 200);
        assert_eq!(pixmap.height(), 200);

        fn pixel(pixmap: &Pixmap, x: u32, y: u32) -> (u8, u8, u8, u8) {
            let idx = ((y * pixmap.width() + x) * 4) as usize;
            let data = pixmap.data();
            (data[idx], data[idx + 1], data[idx + 2], data[idx + 3])
        }

        assert_eq!(pixel(&pixmap, 55, 55), (255, 0, 0, 255));
        assert_eq!(pixel(&pixmap, 155, 55), (255, 255, 255, 255));
    }

    #[test]
    fn test_mask_image_clips_background_to_icon_shape() {
        let mask_url = "bar-mask.svg".to_string();
        let mut mask_pixels = vec![0u8; 20 * 20 * 4];
        // Black horizontal bar in the middle of the 20x20 mask, everything else
        // transparent.
        for y in 8..12 {
            for x in 0..20 {
                let idx = ((y * 20 + x) * 4) as usize;
                mask_pixels[idx] = 0;
                mask_pixels[idx + 1] = 0;
                mask_pixels[idx + 2] = 0;
                mask_pixels[idx + 3] = 255;
            }
        }

        let mut images: HashMap<String, ImageData> = HashMap::new();
        images.insert(
            mask_url.clone(),
            ImageData {
                pixels: mask_pixels,
                width: 20,
                height: 20,
                intrinsic_width: 20,
                intrinsic_height: 20,
                svg_preserve_aspect_ratio: None,
            },
        );

        let mut style = ComputedStyle::default();
        style.background_color = incognidium_css::CssColor::from_rgb(0x33, 0x33, 0x33);
        style.mask_image = Some(mask_url);
        style.mask_size = MaskSize::Contain;
        style.mask_position = (0.5, 0.5);
        style.mask_repeat = MaskRepeat::NoRepeat;

        let mut styles = incognidium_style::StyleMap::new();
        styles.insert(1, style);

        let fbox = FlatBox {
            node_id: 1,
            x: 0.0,
            y: 0.0,
            width: 40.0,
            height: 40.0,
            box_type: BoxType::Block,
            ..FlatBox::default()
        };

        let pixmap = paint_with_images(&[fbox], &styles, 100, 100, &images);

        fn pixel(pixmap: &Pixmap, x: u32, y: u32) -> (u8, u8, u8, u8) {
            let idx = ((y * pixmap.width() + x) * 4) as usize;
            let data = pixmap.data();
            (data[idx], data[idx + 1], data[idx + 2], data[idx + 3])
        }

        // The 20x20 mask is centered in the 40x40 box, so the bar covers
        // y = 18..22 in screen coordinates. Pixels inside the bar keep the
        // dark background color; pixels outside are clipped to transparent.
        let inside = pixel(&pixmap, 20, 20);
        assert!(
            inside.3 > 200 && inside.0 < 60 && inside.1 < 60 && inside.2 < 60,
            "Expected dark bar pixel, got {:?}",
            inside
        );

        let above = pixel(&pixmap, 20, 5);
        let below = pixel(&pixmap, 20, 35);
        assert_eq!(
            above.3, 0,
            "Expected transparent pixel above the bar, got {:?}",
            above
        );
        assert_eq!(
            below.3, 0,
            "Expected transparent pixel below the bar, got {:?}",
            below
        );
    }
}

/// A rasterised 1-bit mask for a clip-path, used to filter per-pixel text output.
struct ClipMask {
    pixmap: Pixmap,
    offset_x: f32,
    offset_y: f32,
}

/// Rasterise a clip-path into a small alpha mask and return it along with its screen offset.
fn build_clip_mask(path: &Path) -> Option<ClipMask> {
    let bounds = path.bounds();
    let x0 = bounds.x().floor();
    let y0 = bounds.y().floor();
    let width = ((bounds.x() + bounds.width()).ceil() - x0).max(1.0) as u32;
    let height = ((bounds.y() + bounds.height()).ceil() - y0).max(1.0) as u32;
    let mut pixmap = Pixmap::new(width, height)?;
    pixmap.data_mut().fill(0);
    let mut paint = Paint::default();
    paint.set_color(Color::WHITE);
    paint.anti_alias = true;
    let transform = Transform::from_translate(-x0, -y0);
    pixmap.fill_path(path, &paint, FillRule::Winding, transform, None);
    Some(ClipMask {
        pixmap,
        offset_x: x0,
        offset_y: y0,
    })
}

/// Return true if the screen pixel (x, y) lies inside the clip mask.
fn clip_mask_contains(mask: &ClipMask, x: f32, y: f32) -> bool {
    let mx = x - mask.offset_x;
    let my = y - mask.offset_y;
    if mx < 0.0 || my < 0.0 {
        return false;
    }
    let mx = mx as u32;
    let my = my as u32;
    let w = mask.pixmap.width();
    let h = mask.pixmap.height();
    if mx >= w || my >= h {
        return false;
    }
    let idx = ((my * w + mx) * 4 + 3) as usize;
    mask.pixmap.data()[idx] > 0
}

/// Zero the alpha of any pixel in the region that lies outside the clip mask.
/// This is used for solid/gradient/border draws that do not accept a per-pixel
/// clip mask themselves.
fn apply_clip_mask_to_region(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    clip_mask: Option<&ClipMask>,
) {
    let mask = match clip_mask {
        Some(m) => m,
        None => return,
    };
    let mask_x0 = mask.offset_x;
    let mask_y0 = mask.offset_y;
    let mask_x1 = mask_x0 + mask.pixmap.width() as f32;
    let mask_y1 = mask_y0 + mask.pixmap.height() as f32;

    let pw = pixmap.width();
    let ph = pixmap.height();
    let data = pixmap.data_mut();
    let mask_data = mask.pixmap.data();
    let mw = mask.pixmap.width();

    let rx0 = x.max(0.0) as u32;
    let ry0 = y.max(0.0) as u32;
    let rx1 = (x + width).min(pw as f32) as u32;
    let ry1 = (y + height).min(ph as f32) as u32;
    if rx1 <= rx0 || ry1 <= ry0 {
        return;
    }
    let mut cleared = 0u32;

    for py in ry0..ry1 {
        for px in rx0..rx1 {
            let pidx = ((py * pw + px) * 4 + 3) as usize;
            let inside_mask = (px as f32) >= mask_x0
                && (px as f32) < mask_x1
                && (py as f32) >= mask_y0
                && (py as f32) < mask_y1;
            let mask_alpha = if inside_mask {
                let mx = (px as f32 - mask_x0) as u32;
                let my = (py as f32 - mask_y0) as u32;
                let midx = ((my * mw + mx) * 4 + 3) as usize;
                mask_data[midx]
            } else {
                0
            };
            if mask_alpha == 0 {
                let base = ((py * pw + px) * 4) as usize;
                data[base] = 0;
                data[base + 1] = 0;
                data[base + 2] = 0;
                data[base + 3] = 0;
                cleared += 1;
            } else if mask_alpha < 255 {
                // The mask is anti-aliased; modulate both the color and alpha
                // by the mask coverage so clipped edges blend smoothly instead
                // of keeping a fully saturated fringe with reduced alpha.
                let base = ((py * pw + px) * 4) as usize;
                let factor = mask_alpha as u32;
                data[base] = ((data[base] as u32 * factor) / 255) as u8;
                data[base + 1] = ((data[base + 1] as u32 * factor) / 255) as u8;
                data[base + 2] = ((data[base + 2] as u32 * factor) / 255) as u8;
                data[base + 3] = ((data[base + 3] as u32 * factor) / 255) as u8;
            }
        }
    }
}

/// Clip a rectangular region to the alpha channel of a CSS `mask-image`.
///
/// The background color and any background-image layers have already been
/// painted into the region; this pass zeros pixels that fall outside the
/// mask tile or where the mask source is transparent. Mask sizing, position,
/// and repeat are resolved the same way as background images.
fn apply_mask_image_to_region(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    mask_img: &ImageData,
    mask_size: incognidium_style::MaskSize,
    mask_position: (f32, f32),
    mask_repeat: incognidium_style::MaskRepeat,
) {
    let img_w = mask_img.intrinsic_width.max(1) as f32;
    let img_h = mask_img.intrinsic_height.max(1) as f32;
    let img_aspect = img_w / img_h.max(0.0001);
    let box_aspect = width / height.max(0.0001);
    let (rendered_w, rendered_h) = match mask_size {
        incognidium_style::MaskSize::Cover => {
            if img_aspect < box_aspect {
                (width, width / img_aspect)
            } else {
                (height * img_aspect, height)
            }
        }
        incognidium_style::MaskSize::Contain => {
            if img_aspect > box_aspect {
                (width, width / img_aspect)
            } else {
                (height * img_aspect, height)
            }
        }
        incognidium_style::MaskSize::Length(w, h) => {
            let rw = if w > 0.0 { w } else { img_w };
            let rh = if h > 0.0 {
                h
            } else {
                img_h * rw / img_w.max(0.0001)
            };
            (rw, rh)
        }
        incognidium_style::MaskSize::Auto => (img_w, img_h),
    };
    if rendered_w <= 0.0 || rendered_h <= 0.0 {
        return;
    }
    let draw_x = x + (width - rendered_w) * mask_position.0;
    let draw_y = y + (height - rendered_h) * mask_position.1;
    let src_w = mask_img.width.max(1) as f32;
    let src_h = mask_img.height.max(1) as f32;

    let pw = pixmap.width();
    let ph = pixmap.height();
    let data = pixmap.data_mut();
    let pixels = &mask_img.pixels;

    let rx0 = x.max(0.0) as u32;
    let ry0 = y.max(0.0) as u32;
    let rx1 = (x + width).min(pw as f32) as u32;
    let ry1 = (y + height).min(ph as f32) as u32;
    if rx1 <= rx0 || ry1 <= ry0 {
        return;
    }

    let inside_tile =
        |dx: f32, dy: f32| dx >= 0.0 && dy >= 0.0 && dx < rendered_w && dy < rendered_h;

    for py in ry0..ry1 {
        for px in rx0..rx1 {
            let dx = px as f32 - draw_x;
            let dy = py as f32 - draw_y;
            let (sx, sy) = match mask_repeat {
                incognidium_style::MaskRepeat::Repeat => {
                    let tx = dx.rem_euclid(rendered_w);
                    let ty = dy.rem_euclid(rendered_h);
                    (
                        (tx / rendered_w * src_w).clamp(0.0, src_w - 1.0),
                        (ty / rendered_h * src_h).clamp(0.0, src_h - 1.0),
                    )
                }
                _ => {
                    if !inside_tile(dx, dy) {
                        let pidx = ((py * pw + px) * 4 + 3) as usize;
                        data[pidx] = 0;
                        continue;
                    }
                    (
                        (dx / rendered_w * src_w).clamp(0.0, src_w - 1.0),
                        (dy / rendered_h * src_h).clamp(0.0, src_h - 1.0),
                    )
                }
            };
            let sx = sx as u32;
            let sy = sy as u32;
            let sidx = ((sy * mask_img.width + sx) * 4 + 3) as usize;
            let src_alpha = pixels[sidx];
            let pidx = ((py * pw + px) * 4 + 3) as usize;
            if src_alpha == 0 {
                data[pidx] = 0;
            } else if src_alpha < 255 {
                data[pidx] = ((data[pidx] as u16 * src_alpha as u16) / 255) as u8;
            }
        }
    }
}

/// Intersect two clip masks, returning a mask that is white only where both
/// input masks are white.
fn intersect_clip_masks(a: &ClipMask, b: &ClipMask) -> Option<ClipMask> {
    let x0 = a.offset_x.min(b.offset_x).floor();
    let y0 = a.offset_y.min(b.offset_y).floor();
    let x1 =
        ((a.offset_x + a.pixmap.width() as f32).max(b.offset_x + b.pixmap.width() as f32)).ceil();
    let y1 =
        ((a.offset_y + a.pixmap.height() as f32).max(b.offset_y + b.pixmap.height() as f32)).ceil();
    let width = (x1 - x0).max(1.0) as u32;
    let height = (y1 - y0).max(1.0) as u32;
    let mut pixmap = Pixmap::new(width, height)?;
    pixmap.data_mut().fill(0);
    let a_data = a.pixmap.data();
    let b_data = b.pixmap.data();
    let aw = a.pixmap.width();
    let ah = a.pixmap.height();
    let bw = b.pixmap.width();
    let bh = b.pixmap.height();
    let out_data = pixmap.data_mut();
    for y in 0..height {
        for x in 0..width {
            let sx = x0 + x as f32;
            let sy = y0 + y as f32;
            let ax = (sx - a.offset_x) as i32;
            let ay = (sy - a.offset_y) as i32;
            let bx = (sx - b.offset_x) as i32;
            let by = (sy - b.offset_y) as i32;
            let a_in = ax >= 0
                && ax < aw as i32
                && ay >= 0
                && ay < ah as i32
                && a_data[((ay as u32 * aw + ax as u32) * 4 + 3) as usize] > 0;
            let b_in = bx >= 0
                && bx < bw as i32
                && by >= 0
                && by < bh as i32
                && b_data[((by as u32 * bw + bx as u32) * 4 + 3) as usize] > 0;
            if a_in && b_in {
                let idx = ((y * width + x) * 4 + 3) as usize;
                out_data[idx] = 255;
            }
        }
    }
    Some(ClipMask {
        pixmap,
        offset_x: x0,
        offset_y: y0,
    })
}

/// Combine two optional clip masks, returning their intersection if both are
/// present.
fn combine_clip_masks(a: Option<ClipMask>, b: Option<ClipMask>) -> Option<ClipMask> {
    match (a, b) {
        (None, None) => None,
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (Some(a), Some(b)) => intersect_clip_masks(&a, &b),
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
            let resolve = |v: &LengthValue, base: f32| match v {
                LengthValue::Px(px) => *px,
                LengthValue::Percent(p) => base * (*p / 100.0),
            };
            let t = resolve(t, height);
            let r = resolve(r, width);
            let b = resolve(b, height);
            let l = resolve(l, width);
            let inset_x = x + l;
            let inset_y = y + t;
            let inset_w = width - l - r;
            let inset_h = height - t - b;
            if inset_w > 0.0 && inset_h > 0.0 {
                let rect = Rect::from_xywh(inset_x, inset_y, inset_w, inset_h)?;
                Some(PathBuilder::from_rect(rect))
            } else {
                // Fully clipped (e.g. inset(50%)) – return a zero-area path so nothing draws.
                let rect = Rect::from_xywh(inset_x, inset_y, inset_w.max(0.0), inset_h.max(0.0))?;
                Some(PathBuilder::from_rect(rect))
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
    pb.finish()
        .unwrap_or_else(|| PathBuilder::new().finish().unwrap())
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
    pb.finish()
        .unwrap_or_else(|| PathBuilder::new().finish().unwrap())
}

/// Build a polygon path from percentage coordinates
fn build_polygon_path(x: f32, y: f32, width: f32, height: f32, points: &[(f32, f32)]) -> Path {
    let mut pb = PathBuilder::new();
    if points.is_empty() {
        return pb
            .finish()
            .unwrap_or_else(|| PathBuilder::new().finish().unwrap());
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
    pb.finish()
        .unwrap_or_else(|| PathBuilder::new().finish().unwrap())
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
    clip_mask: Option<&ClipMask>,
) {
    let mut paint = Paint::default();
    paint.set_color(css_to_skia_color(color));
    paint.anti_alias = true;
    pixmap.fill_path(clip_path, &paint, FillRule::Winding, transform, None);
    apply_clip_mask_to_region(pixmap, x, y, width, height, clip_mask);
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
    clip_mask: Option<&ClipMask>,
) {
    use incognidium_style::GradientDirection;
    use tiny_skia::{GradientStop, LinearGradient as SkiaLinearGradient, Point, SpreadMode};

    if width <= 0.0 || height <= 0.0 {
        return;
    }

    let spread_mode = if grad.repeating {
        SpreadMode::Repeat
    } else {
        SpreadMode::Pad
    };
    let period = grad.repeat_length.unwrap_or(-1.0);

    // Calculate gradient line based on direction (same as draw_linear_gradient)
    let (x1, y1, x2, y2) = match grad.direction {
        GradientDirection::ToTop => {
            let len = if period > 0.0 { period } else { height };
            let sx = x + width / 2.0;
            (sx, y + height, sx, y + height - len)
        }
        GradientDirection::ToBottom => {
            let len = if period > 0.0 { period } else { height };
            let sx = x + width / 2.0;
            (sx, y, sx, y + len)
        }
        GradientDirection::ToLeft => {
            let len = if period > 0.0 { period } else { width };
            let sy = y + height / 2.0;
            (x + width, sy, x + width - len, sy)
        }
        GradientDirection::ToRight => {
            let len = if period > 0.0 { period } else { width };
            let sy = y + height / 2.0;
            (x, sy, x + len, sy)
        }
        GradientDirection::ToTopLeft => {
            let full = (width * width + height * height).sqrt();
            let len = if period > 0.0 { period } else { full };
            let inv_sqrt2 = 1.0 / 2.0_f32.sqrt();
            (
                x + width,
                y + height,
                x + width - len * inv_sqrt2,
                y + height - len * inv_sqrt2,
            )
        }
        GradientDirection::ToTopRight => {
            let full = (width * width + height * height).sqrt();
            let len = if period > 0.0 { period } else { full };
            let inv_sqrt2 = 1.0 / 2.0_f32.sqrt();
            (
                x,
                y + height,
                x + len * inv_sqrt2,
                y + height - len * inv_sqrt2,
            )
        }
        GradientDirection::ToBottomLeft => {
            let full = (width * width + height * height).sqrt();
            let len = if period > 0.0 { period } else { full };
            let inv_sqrt2 = 1.0 / 2.0_f32.sqrt();
            (
                x + width,
                y,
                x + width - len * inv_sqrt2,
                y + len * inv_sqrt2,
            )
        }
        GradientDirection::ToBottomRight => {
            let full = (width * width + height * height).sqrt();
            let len = if period > 0.0 { period } else { full };
            let inv_sqrt2 = 1.0 / 2.0_f32.sqrt();
            (x, y, x + len * inv_sqrt2, y + len * inv_sqrt2)
        }
        GradientDirection::Angle(deg) => {
            let rad = deg.to_radians();
            let cx = x + width / 2.0;
            let cy = y + height / 2.0;
            let full = (width * width + height * height).sqrt();
            let len = if period > 0.0 { period } else { full };
            let half_len = len / 2.0;
            let dx = rad.sin() * half_len;
            let dy = -rad.cos() * half_len;
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
        spread_mode,
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
        apply_clip_mask_to_region(pixmap, x, y, width, height, clip_mask);
        return;
    };
    pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
    apply_clip_mask_to_region(pixmap, x, y, width, height, clip_mask);
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
    clip_mask: Option<&ClipMask>,
) {
    use tiny_skia::{GradientStop, Point, RadialGradient as SkiaRadialGradient, SpreadMode};

    let center = Point::from_xy(
        x + grad.position.0 / 100.0 * width,
        y + grad.position.1 / 100.0 * height,
    );
    let radius = if grad.repeating {
        grad.repeat_length
            .unwrap_or_else(|| width.max(height) / 2.0)
    } else {
        width.min(height) / 2.0
    };

    let spread_mode = if grad.repeating {
        SpreadMode::Repeat
    } else {
        SpreadMode::Pad
    };

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
        spread_mode,
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
        apply_clip_mask_to_region(pixmap, x, y, width, height, clip_mask);
        return;
    };
    pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
    apply_clip_mask_to_region(pixmap, x, y, width, height, clip_mask);
}
