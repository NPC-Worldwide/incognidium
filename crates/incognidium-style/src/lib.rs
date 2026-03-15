use incognidium_css::{
    matching_rules, parse_inline_style, CssColor, CssValue, Declaration, Stylesheet,
};
use incognidium_dom::{Document, ElementData, NodeData, NodeId};
use std::collections::HashMap;

/// Computed style values for a single element.
#[derive(Debug, Clone)]
pub struct ComputedStyle {
    pub display: Display,
    pub position: Position,
    pub color: CssColor,
    pub background_color: CssColor,
    pub font_size: f32,
    pub font_weight: FontWeight,
    pub font_style: FontStyle,
    pub text_align: TextAlign,
    pub text_decoration: TextDecoration,
    pub line_height: f32,

    pub margin_top: f32,
    pub margin_right: f32,
    pub margin_bottom: f32,
    pub margin_left: f32,

    pub padding_top: f32,
    pub padding_right: f32,
    pub padding_bottom: f32,
    pub padding_left: f32,

    pub border_top_width: f32,
    pub border_right_width: f32,
    pub border_bottom_width: f32,
    pub border_left_width: f32,
    pub border_color: CssColor,

    pub width: SizeValue,
    pub height: SizeValue,
    pub min_width: SizeValue,
    pub max_width: SizeValue,
    pub min_height: SizeValue,
    pub max_height: SizeValue,

    // Flexbox
    pub flex_direction: FlexDirection,
    pub flex_wrap: FlexWrap,
    pub justify_content: JustifyContent,
    pub align_items: AlignItems,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub flex_basis: SizeValue,
    pub gap: f32,

    pub overflow: Overflow,
    pub visibility: Visibility,
    pub opacity: f32,
    pub text_transform: TextTransform,
    pub white_space: WhiteSpace,
}

impl Default for ComputedStyle {
    fn default() -> Self {
        ComputedStyle {
            display: Display::Block,
            position: Position::Static,
            color: CssColor::BLACK,
            background_color: CssColor::TRANSPARENT,
            font_size: 16.0,
            font_weight: FontWeight::Normal,
            font_style: FontStyle::Normal,
            text_align: TextAlign::Left,
            text_decoration: TextDecoration::None,
            line_height: 1.2,

            margin_top: 0.0,
            margin_right: 0.0,
            margin_bottom: 0.0,
            margin_left: 0.0,

            padding_top: 0.0,
            padding_right: 0.0,
            padding_bottom: 0.0,
            padding_left: 0.0,

            border_top_width: 0.0,
            border_right_width: 0.0,
            border_bottom_width: 0.0,
            border_left_width: 0.0,
            border_color: CssColor::BLACK,

            width: SizeValue::Auto,
            height: SizeValue::Auto,
            min_width: SizeValue::Auto,
            max_width: SizeValue::None,
            min_height: SizeValue::Auto,
            max_height: SizeValue::None,

            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::NoWrap,
            justify_content: JustifyContent::FlexStart,
            align_items: AlignItems::Stretch,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: SizeValue::Auto,
            gap: 0.0,

            overflow: Overflow::Visible,
            visibility: Visibility::Visible,
            opacity: 1.0,
            text_transform: TextTransform::None,
            white_space: WhiteSpace::Normal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Display {
    Block,
    Inline,
    Flex,
    InlineBlock,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Position {
    Static,
    Relative,
    Absolute,
    Fixed,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FontWeight {
    Normal,
    Bold,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FontStyle {
    Normal,
    Italic,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextDecoration {
    None,
    Underline,
    LineThrough,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FlexDirection {
    Row,
    RowReverse,
    Column,
    ColumnReverse,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FlexWrap {
    NoWrap,
    Wrap,
    WrapReverse,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JustifyContent {
    FlexStart,
    FlexEnd,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AlignItems {
    FlexStart,
    FlexEnd,
    Center,
    Stretch,
    Baseline,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Overflow {
    Visible,
    Hidden,
    Scroll,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Visibility {
    Visible,
    Hidden,
    Collapse,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextTransform {
    None,
    Uppercase,
    Lowercase,
    Capitalize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WhiteSpace {
    Normal,
    NoWrap,
    Pre,
    PreWrap,
    PreLine,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SizeValue {
    Px(f32),
    Percent(f32),
    Auto,
    None,
}

/// A map from NodeId to ComputedStyle.
pub type StyleMap = HashMap<NodeId, ComputedStyle>;

/// Resolve styles for the entire document.
pub fn resolve_styles(doc: &Document, stylesheet: &Stylesheet) -> StyleMap {
    let mut styles = HashMap::new();
    let root = doc.root();
    let default_style = ComputedStyle::default();
    resolve_node(doc, stylesheet, root, &default_style, &mut styles);
    styles
}

fn resolve_node(
    doc: &Document,
    stylesheet: &Stylesheet,
    node_id: NodeId,
    parent_style: &ComputedStyle,
    styles: &mut StyleMap,
) {
    let node = doc.node(node_id);
    let style = match &node.data {
        NodeData::Element(el) => {
            let style = compute_style_for_element(doc, node_id, el, stylesheet, parent_style);
            styles.insert(node_id, style.clone());
            style
        }
        NodeData::Text(_) => {
            // Text nodes inherit from parent
            let mut style = parent_style.clone();
            style.display = Display::Inline;
            styles.insert(node_id, style.clone());
            style
        }
        _ => {
            styles.insert(node_id, parent_style.clone());
            parent_style.clone()
        }
    };

    let children = doc.node(node_id).children.clone();
    for child_id in children {
        resolve_node(doc, stylesheet, child_id, &style, styles);
    }
}

/// Compute style for an element by matching CSS rules + inline styles.
fn compute_style_for_element(
    doc: &Document,
    node_id: NodeId,
    element: &ElementData,
    stylesheet: &Stylesheet,
    parent_style: &ComputedStyle,
) -> ComputedStyle {
    let mut style = ComputedStyle::default();

    // 1. Inherit inheritable properties from parent first
    style.color = parent_style.color;
    style.font_size = parent_style.font_size;
    style.font_weight = parent_style.font_weight;
    style.font_style = parent_style.font_style;
    style.text_align = parent_style.text_align;
    style.line_height = parent_style.line_height;
    style.visibility = parent_style.visibility;
    style.text_transform = parent_style.text_transform;
    style.white_space = parent_style.white_space;

    // 2. Apply UA defaults (display, margins, padding, and tag-specific colors like <a>)
    // These act as the UA stylesheet — lower priority than author CSS
    apply_ua_defaults(element, &mut style);

    // 4. Collect matching rules and sort by specificity
    let mut matched = matching_rules(stylesheet, element, doc, node_id);
    matched.sort_by_key(|m| m.specificity);

    // Apply rules in specificity order (lowest first, so highest wins)
    // First pass: normal declarations
    for matched_rule in &matched {
        for decl in &matched_rule.rule.declarations {
            if !decl.important {
                apply_declaration(&mut style, decl, parent_style.font_size);
            }
        }
    }
    // Second pass: !important declarations (override everything)
    for matched_rule in &matched {
        for decl in &matched_rule.rule.declarations {
            if decl.important {
                apply_declaration(&mut style, decl, parent_style.font_size);
            }
        }
    }

    // Hide <nav> elements — navigation menus aren't content and take up
    // enormous vertical space in a linear (non-flexbox) layout
    if element.tag_name == "nav" {
        style.display = Display::None;
    }
    // Hide elements with role="navigation" or common sidebar/menu classes
    if let Some(role) = element.get_attr("role") {
        if role == "navigation" || role == "banner" || role == "complementary" {
            style.display = Display::None;
        }
    }
    if let Some(class) = element.get_attr("class") {
        let cl = class.to_lowercase();
        if cl.contains("sidebar") || cl.contains("side-bar")
            || cl.contains("mw-panel") || cl.contains("vector-sidebar")
            || cl.contains("vector-menu") || cl.contains("vector-header")
            || cl.contains("site-header") || cl.contains("nav-bar")
            || cl.contains("navbar") || cl.contains("skip-link")
            || cl.contains("offcanvas") || cl.contains("off-canvas")
        {
            style.display = Display::None;
        }
    }
    if let Some(id) = element.get_attr("id") {
        let id_lower = id.to_lowercase();
        if id_lower == "mw-panel" || id_lower == "mw-navigation"
            || id_lower == "mw-head" || id_lower == "p-navigation"
            || id_lower == "p-search" || id_lower == "p-interaction"
            || id_lower.starts_with("vector-")
        {
            style.display = Display::None;
        }
    }

    // Apply HTML presentational attributes (width, height on img etc.)
    if let Some(w) = element.get_attr("width") {
        let w = w.trim();
        if w.ends_with('%') {
            if let Ok(p) = w.trim_end_matches('%').parse::<f32>() {
                style.width = SizeValue::Percent(p);
            }
        } else if let Ok(px) = w.trim_end_matches("px").parse::<f32>() {
            style.width = SizeValue::Px(px);
        }
    }
    if let Some(h) = element.get_attr("height") {
        let h = h.trim();
        if h.ends_with('%') {
            if let Ok(p) = h.trim_end_matches('%').parse::<f32>() {
                style.height = SizeValue::Percent(p);
            }
        } else if let Ok(px) = h.trim_end_matches("px").parse::<f32>() {
            style.height = SizeValue::Px(px);
        }
    }

    // cellpadding="0" on tables removes default padding from child td/th
    if element.tag_name == "td" || element.tag_name == "th" {
        if let Some(parent_id) = doc.node(node_id).parent {
            // Walk up to find ancestor <table>
            let mut table_id = Some(parent_id);
            while let Some(tid) = table_id {
                if let NodeData::Element(ref tel) = doc.node(tid).data {
                    if tel.tag_name == "table" {
                        if let Some(cp) = tel.get_attr("cellpadding") {
                            if let Ok(px) = cp.parse::<f32>() {
                                style.padding_top = px;
                                style.padding_right = px;
                                style.padding_bottom = px;
                                style.padding_left = px;
                            }
                        }
                        break;
                    }
                }
                table_id = doc.node(tid).parent;
            }
        }
    }

    // bgcolor attribute (used by HN tables, old-school HTML)
    if let Some(bg) = element.get_attr("bgcolor") {
        if let Some(c) = parse_html_color(bg) {
            style.background_color = c;
        }
    }

    // color attribute on <font> etc.
    if let Some(col) = element.get_attr("color") {
        if let Some(c) = parse_html_color(col) {
            style.color = c;
        }
    }

    // align attribute
    if let Some(align) = element.get_attr("align") {
        style.text_align = match align.to_ascii_lowercase().as_str() {
            "center" => TextAlign::Center,
            "right" => TextAlign::Right,
            "left" => TextAlign::Left,
            _ => style.text_align,
        };
    }

    // border attribute (e.g. <table border="1">)
    if let Some(border) = element.get_attr("border") {
        if let Ok(px) = border.parse::<f32>() {
            if px > 0.0 {
                style.border_top_width = px;
                style.border_right_width = px;
                style.border_bottom_width = px;
                style.border_left_width = px;
                if style.border_color.a == 0 {
                    style.border_color = CssColor::from_rgb(0, 0, 0);
                }
            }
        }
    }

    // colspan on <td>/<th> maps to flex-grow for proportional sizing
    if element.tag_name == "td" || element.tag_name == "th" {
        if let Some(cs) = element.get_attr("colspan") {
            if let Ok(n) = cs.parse::<f32>() {
                style.flex_grow = n;
            }
        }
    }

    // Apply inline styles (highest specificity)
    if let Some(inline) = element.get_attr("style") {
        let decls = parse_inline_style(inline);
        for decl in &decls {
            apply_declaration(&mut style, decl, parent_style.font_size);
        }
    }

    // Table cells with explicit width should not flex-grow
    if (element.tag_name == "td" || element.tag_name == "th")
        && !matches!(style.width, SizeValue::Auto | SizeValue::None)
    {
        style.flex_grow = 0.0;
    }

    // HTML hidden attribute overrides display
    if element.get_attr("hidden").is_some() {
        style.display = Display::None;
    }

    // input type="hidden"
    if element.tag_name == "input" {
        if let Some(t) = element.get_attr("type") {
            if t == "hidden" {
                style.display = Display::None;
            }
        }
    }

    // Show selected or first <option> in a <select>
    if element.tag_name == "option" {
        if let Some(parent_id) = doc.node(node_id).parent {
            let parent = doc.node(parent_id);
            if let NodeData::Element(ref pel) = parent.data {
                if pel.tag_name == "select" {
                    let is_selected = element.get_attr("selected").is_some();
                    let is_first = parent.children.iter()
                        .filter(|&&cid| matches!(&doc.node(cid).data, NodeData::Element(ref e) if e.tag_name == "option"))
                        .next()
                        .map(|&cid| cid == node_id)
                        .unwrap_or(false);
                    let any_selected = parent.children.iter().any(|&cid| {
                        matches!(&doc.node(cid).data, NodeData::Element(ref e) if e.tag_name == "option" && e.get_attr("selected").is_some())
                    });
                    if is_selected || (!any_selected && is_first) {
                        style.display = Display::Inline;
                    }
                }
            }
        }
    }

    // aria-hidden="true" elements (decorative/duplicate content)
    if element.get_attr("aria-hidden").map(|v| v == "true").unwrap_or(false) {
        style.display = Display::None;
    }

    // Hide only truly invisible accessibility patterns
    if let Some(class) = element.get_attr("class") {
        let classes: Vec<&str> = class.split_whitespace().collect();
        for c in &classes {
            match *c {
                "sr-only" | "visually-hidden" | "screen-reader-text" | "skip-link" => {
                    style.display = Display::None;
                    break;
                }
                _ => {}
            }
        }
    }

    style
}

fn apply_declaration(style: &mut ComputedStyle, decl: &Declaration, parent_font_size: f32) {
    match decl.property.as_str() {
        "display" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.display = match kw.as_str() {
                    "block" => Display::Block,
                    "inline" => Display::Inline,
                    "flex" => Display::Flex,
                    "inline-block" => Display::InlineBlock,
                    "none" => Display::None,
                    // Map unsupported display values to closest supported ones
                    "grid" => Display::Block,
                    "inline-flex" | "inline-grid" => Display::InlineBlock,
                    "list-item" => Display::Block,
                    "table" | "table-row-group" | "table-header-group"
                    | "table-footer-group" | "table-caption" => Display::Block,
                    "table-row" => Display::Block,
                    "table-cell" => Display::InlineBlock,
                    "contents" => Display::Block,
                    "flow-root" => Display::Block,
                    _ => style.display,
                };
            }
        }
        "position" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.position = match kw.as_str() {
                    "static" => Position::Static,
                    "relative" => Position::Relative,
                    "absolute" => Position::Absolute,
                    "fixed" => Position::Fixed,
                    _ => style.position,
                };
            }
        }
        "color" => {
            match &decl.value {
                CssValue::Color(c) => style.color = *c,
                CssValue::Inherit => {} // already inherited, no-op
                _ => {}
            }
        }
        "background-color" | "background" => {
            if let CssValue::Color(c) = &decl.value {
                style.background_color = *c;
            }
        }
        "font-size" => {
            if let Some(px) = decl.value.to_px(parent_font_size) {
                style.font_size = px;
            } else if let CssValue::Keyword(kw) = &decl.value {
                style.font_size = match kw.as_str() {
                    "xx-small" => 9.0,
                    "x-small" => 10.0,
                    "small" => 13.0,
                    "medium" => 16.0,
                    "large" => 18.0,
                    "x-large" => 24.0,
                    "xx-large" => 32.0,
                    _ => style.font_size,
                };
            }
        }
        "font-weight" => {
            style.font_weight = match &decl.value {
                CssValue::Keyword(kw) => match kw.as_str() {
                    "bold" => FontWeight::Bold,
                    "normal" => FontWeight::Normal,
                    _ => style.font_weight,
                },
                CssValue::Number(n) if *n >= 700.0 => FontWeight::Bold,
                CssValue::Number(_) => FontWeight::Normal,
                _ => style.font_weight,
            };
        }
        "font-style" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.font_style = match kw.as_str() {
                    "italic" | "oblique" => FontStyle::Italic,
                    "normal" => FontStyle::Normal,
                    _ => style.font_style,
                };
            }
        }
        "text-align" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.text_align = match kw.as_str() {
                    "left" => TextAlign::Left,
                    "center" => TextAlign::Center,
                    "right" => TextAlign::Right,
                    "justify" => TextAlign::Justify,
                    _ => style.text_align,
                };
            }
        }
        "text-decoration" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.text_decoration = match kw.as_str() {
                    "underline" => TextDecoration::Underline,
                    "line-through" => TextDecoration::LineThrough,
                    "none" => TextDecoration::None,
                    _ => style.text_decoration,
                };
            }
        }
        "line-height" => {
            if let CssValue::Number(n) = &decl.value {
                style.line_height = *n;
            } else if let Some(px) = decl.value.to_px(parent_font_size) {
                style.line_height = px / style.font_size;
            }
        }
        "margin" => apply_box_shorthand_margin(style, &decl.value, parent_font_size),
        "margin-top" => {
            if let Some(px) = decl.value.to_px(parent_font_size) {
                style.margin_top = px;
            }
        }
        "margin-right" => {
            if let Some(px) = decl.value.to_px(parent_font_size) {
                style.margin_right = px;
            }
        }
        "margin-bottom" => {
            if let Some(px) = decl.value.to_px(parent_font_size) {
                style.margin_bottom = px;
            }
        }
        "margin-left" => {
            if let Some(px) = decl.value.to_px(parent_font_size) {
                style.margin_left = px;
            }
        }
        "padding" => apply_box_shorthand_padding(style, &decl.value, parent_font_size),
        "padding-top" => {
            if let Some(px) = decl.value.to_px(parent_font_size) {
                style.padding_top = px;
            }
        }
        "padding-right" => {
            if let Some(px) = decl.value.to_px(parent_font_size) {
                style.padding_right = px;
            }
        }
        "padding-bottom" => {
            if let Some(px) = decl.value.to_px(parent_font_size) {
                style.padding_bottom = px;
            }
        }
        "padding-left" => {
            if let Some(px) = decl.value.to_px(parent_font_size) {
                style.padding_left = px;
            }
        }
        "border-width" => {
            if let Some(px) = decl.value.to_px(parent_font_size) {
                style.border_top_width = px;
                style.border_right_width = px;
                style.border_bottom_width = px;
                style.border_left_width = px;
            }
        }
        "border-color" => {
            if let CssValue::Color(c) = &decl.value {
                style.border_color = *c;
            }
        }
        "border" => {
            // Simplified border shorthand: just handle "Npx solid color"
            // We parse the first length as width
            if let Some(px) = decl.value.to_px(parent_font_size) {
                style.border_top_width = px;
                style.border_right_width = px;
                style.border_bottom_width = px;
                style.border_left_width = px;
            }
        }
        "width" => {
            style.width = to_size_value(&decl.value, parent_font_size);
        }
        "height" => {
            style.height = to_size_value(&decl.value, parent_font_size);
        }
        "min-width" => {
            style.min_width = to_size_value(&decl.value, parent_font_size);
        }
        "max-width" => {
            style.max_width = to_size_value(&decl.value, parent_font_size);
        }
        "flex-direction" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.flex_direction = match kw.as_str() {
                    "row" => FlexDirection::Row,
                    "row-reverse" => FlexDirection::RowReverse,
                    "column" => FlexDirection::Column,
                    "column-reverse" => FlexDirection::ColumnReverse,
                    _ => style.flex_direction,
                };
            }
        }
        "flex-wrap" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.flex_wrap = match kw.as_str() {
                    "nowrap" => FlexWrap::NoWrap,
                    "wrap" => FlexWrap::Wrap,
                    "wrap-reverse" => FlexWrap::WrapReverse,
                    _ => style.flex_wrap,
                };
            }
        }
        "justify-content" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.justify_content = match kw.as_str() {
                    "flex-start" | "start" => JustifyContent::FlexStart,
                    "flex-end" | "end" => JustifyContent::FlexEnd,
                    "center" => JustifyContent::Center,
                    "space-between" => JustifyContent::SpaceBetween,
                    "space-around" => JustifyContent::SpaceAround,
                    "space-evenly" => JustifyContent::SpaceEvenly,
                    _ => style.justify_content,
                };
            }
        }
        "align-items" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.align_items = match kw.as_str() {
                    "flex-start" | "start" => AlignItems::FlexStart,
                    "flex-end" | "end" => AlignItems::FlexEnd,
                    "center" => AlignItems::Center,
                    "stretch" => AlignItems::Stretch,
                    "baseline" => AlignItems::Baseline,
                    _ => style.align_items,
                };
            }
        }
        "flex-grow" => {
            if let CssValue::Number(n) = &decl.value {
                style.flex_grow = *n;
            }
        }
        "flex-shrink" => {
            if let CssValue::Number(n) = &decl.value {
                style.flex_shrink = *n;
            }
        }
        "flex-basis" => {
            style.flex_basis = to_size_value(&decl.value, parent_font_size);
        }
        "flex" => {
            match &decl.value {
                CssValue::List(vals) => {
                    // flex: <grow> <shrink> <basis>
                    if let Some(CssValue::Number(g)) = vals.get(0) {
                        style.flex_grow = *g;
                    }
                    if let Some(CssValue::Number(s)) = vals.get(1) {
                        style.flex_shrink = *s;
                    }
                    if let Some(basis) = vals.get(2) {
                        style.flex_basis = to_size_value(basis, parent_font_size);
                    }
                }
                CssValue::Number(n) => {
                    style.flex_grow = *n;
                    style.flex_shrink = 1.0;
                    style.flex_basis = SizeValue::Px(0.0);
                }
                CssValue::Keyword(kw) if kw == "none" => {
                    style.flex_grow = 0.0;
                    style.flex_shrink = 0.0;
                    style.flex_basis = SizeValue::Auto;
                }
                CssValue::Keyword(kw) if kw == "auto" => {
                    style.flex_grow = 1.0;
                    style.flex_shrink = 1.0;
                    style.flex_basis = SizeValue::Auto;
                }
                _ => {}
            }
        }
        "gap" => {
            if let Some(px) = decl.value.to_px(parent_font_size) {
                style.gap = px;
            }
        }
        "overflow" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.overflow = match kw.as_str() {
                    "visible" => Overflow::Visible,
                    "hidden" => Overflow::Hidden,
                    "scroll" => Overflow::Scroll,
                    "auto" => Overflow::Auto,
                    _ => style.overflow,
                };
            }
        }
        "visibility" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.visibility = match kw.as_str() {
                    "visible" => Visibility::Visible,
                    "hidden" => Visibility::Hidden,
                    "collapse" => Visibility::Collapse,
                    _ => style.visibility,
                };
            }
        }
        "opacity" => {
            if let Some(px) = decl.value.to_px(parent_font_size) {
                style.opacity = px.clamp(0.0, 1.0);
            }
        }
        "text-transform" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.text_transform = match kw.as_str() {
                    "uppercase" => TextTransform::Uppercase,
                    "lowercase" => TextTransform::Lowercase,
                    "capitalize" => TextTransform::Capitalize,
                    "none" => TextTransform::None,
                    _ => style.text_transform,
                };
            }
        }
        "white-space" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.white_space = match kw.as_str() {
                    "normal" => WhiteSpace::Normal,
                    "nowrap" => WhiteSpace::NoWrap,
                    "pre" => WhiteSpace::Pre,
                    "pre-wrap" => WhiteSpace::PreWrap,
                    "pre-line" => WhiteSpace::PreLine,
                    _ => style.white_space,
                };
            }
        }
        "min-height" => {
            style.min_height = to_size_value(&decl.value, parent_font_size);
        }
        "max-height" => {
            style.max_height = to_size_value(&decl.value, parent_font_size);
        }
        "border-top" | "border-right" | "border-bottom" | "border-left" => {
            // Parse shorthand: e.g. "1px solid #ccc"
            if let Some(px) = decl.value.to_px(parent_font_size) {
                match decl.property.as_str() {
                    "border-top" => style.border_top_width = px,
                    "border-right" => style.border_right_width = px,
                    "border-bottom" => style.border_bottom_width = px,
                    "border-left" => style.border_left_width = px,
                    _ => {}
                }
            }
        }
        "border-top-width" => {
            if let Some(px) = decl.value.to_px(parent_font_size) { style.border_top_width = px; }
        }
        "border-right-width" => {
            if let Some(px) = decl.value.to_px(parent_font_size) { style.border_right_width = px; }
        }
        "border-bottom-width" => {
            if let Some(px) = decl.value.to_px(parent_font_size) { style.border_bottom_width = px; }
        }
        "border-left-width" => {
            if let Some(px) = decl.value.to_px(parent_font_size) { style.border_left_width = px; }
        }
        "border-top-color" | "border-right-color" | "border-bottom-color" | "border-left-color" => {
            if let CssValue::Color(c) = &decl.value {
                style.border_color = *c;
            }
        }
        _ => {} // Unknown property, skip
    }
}

fn to_size_value(value: &CssValue, parent_font_size: f32) -> SizeValue {
    match value {
        CssValue::Auto => SizeValue::Auto,
        CssValue::None => SizeValue::None,
        CssValue::Percentage(p) => SizeValue::Percent(*p),
        _ => {
            if let Some(px) = value.to_px(parent_font_size) {
                SizeValue::Px(px)
            } else {
                SizeValue::Auto
            }
        }
    }
}

fn apply_box_shorthand_margin(style: &mut ComputedStyle, value: &CssValue, pfs: f32) {
    if let Some(px) = value.to_px(pfs) {
        style.margin_top = px;
        style.margin_right = px;
        style.margin_bottom = px;
        style.margin_left = px;
    }
}

fn apply_box_shorthand_padding(style: &mut ComputedStyle, value: &CssValue, pfs: f32) {
    if let Some(px) = value.to_px(pfs) {
        style.padding_top = px;
        style.padding_right = px;
        style.padding_bottom = px;
        style.padding_left = px;
    }
}

/// Apply user-agent default styles for HTML elements.
fn apply_ua_defaults(element: &ElementData, style: &mut ComputedStyle) {
    match element.tag_name.as_str() {
        "h1" => {
            style.display = Display::Block;
            style.font_size = 32.0;
            style.font_weight = FontWeight::Bold;
            style.margin_top = 12.0;
            style.margin_bottom = 8.0;
        }
        "h2" => {
            style.display = Display::Block;
            style.font_size = 24.0;
            style.font_weight = FontWeight::Bold;
            style.margin_top = 12.0;
            style.margin_bottom = 8.0;
        }
        "h3" => {
            style.display = Display::Block;
            style.font_size = 18.72;
            style.font_weight = FontWeight::Bold;
            style.margin_top = 10.0;
            style.margin_bottom = 6.0;
        }
        "h4" => {
            style.display = Display::Block;
            style.font_weight = FontWeight::Bold;
            style.margin_top = 21.28;
            style.margin_bottom = 21.28;
        }
        "h5" => {
            style.display = Display::Block;
            style.font_size = 13.28;
            style.font_weight = FontWeight::Bold;
            style.margin_top = 22.18;
            style.margin_bottom = 22.18;
        }
        "h6" => {
            style.display = Display::Block;
            style.font_size = 10.72;
            style.font_weight = FontWeight::Bold;
            style.margin_top = 24.97;
            style.margin_bottom = 24.97;
        }
        "p" => {
            style.display = Display::Block;
            style.margin_top = 8.0;
            style.margin_bottom = 8.0;
        }
        "div" | "section" | "article" | "main" | "aside" => {
            style.display = Display::Block;
        }
        "footer" | "header" => {
            style.display = Display::Block;
        }
        "span" | "small" => {
            style.display = Display::Inline;
        }
        "strong" | "b" => {
            style.display = Display::Inline;
            style.font_weight = FontWeight::Bold;
        }
        "em" | "i" => {
            style.display = Display::Inline;
            style.font_style = FontStyle::Italic;
        }
        "u" => {
            style.display = Display::Inline;
            style.text_decoration = TextDecoration::Underline;
        }
        "a" => {
            style.display = Display::Inline;
            style.color = CssColor::from_rgb(0, 0, 238);
            style.text_decoration = TextDecoration::Underline;
        }
        "code" => {
            style.display = Display::Inline;
            style.font_size = style.font_size * 0.875;
        }
        "pre" => {
            style.display = Display::Block;
            style.margin_top = 16.0;
            style.margin_bottom = 16.0;
        }
        "ul" | "ol" => {
            style.display = Display::Block;
            style.margin_top = 4.0;
            style.margin_bottom = 4.0;
            style.padding_left = 24.0;
        }
        "li" => {
            style.display = Display::Block;
        }
        "head" | "style" | "script" | "link" | "meta" | "title" | "template"
        | "svg" | "datalist" | "dialog" => {
            style.display = Display::None;
        }
        "noscript" => {
            // Show noscript content since our JS engine is limited
            style.display = Display::Block;
        }
        "nav" => {
            style.display = Display::Block;
        }
        "table" => {
            style.display = Display::Block;
            style.margin_top = 8.0;
            style.margin_bottom = 8.0;
        }
        "thead" | "tbody" | "tfoot" => {
            style.display = Display::Block;
        }
        "tr" => {
            // Table rows use flex to distribute column widths
            style.display = Display::Flex;
            style.flex_direction = FlexDirection::Row;
        }
        "td" | "th" => {
            style.display = Display::Block;
            style.flex_grow = 1.0;
            style.flex_shrink = 1.0;
            style.padding_left = 4.0;
            style.padding_right = 4.0;
            if element.tag_name == "th" {
                style.font_weight = FontWeight::Bold;
            }
        }
        "figure" => {
            style.display = Display::Block;
            style.margin_top = 4.0;
            style.margin_bottom = 4.0;
        }
        "figcaption" => {
            style.display = Display::Block;
            style.font_size = style.font_size * 0.875;
        }
        "dl" => {
            style.display = Display::Block;
            style.margin_top = 4.0;
            style.margin_bottom = 4.0;
        }
        "dt" => {
            style.display = Display::Block;
            style.font_weight = FontWeight::Bold;
        }
        "dd" => {
            style.display = Display::Block;
            style.margin_left = 40.0;
        }
        "blockquote" => {
            style.display = Display::Block;
            style.margin_top = 16.0;
            style.margin_bottom = 16.0;
            style.margin_left = 40.0;
            style.margin_right = 40.0;
        }
        "details" | "summary" => {
            style.display = Display::Block;
        }
        "form" | "fieldset" => {
            style.display = Display::Block;
        }
        "label" => {
            style.display = Display::Inline;
        }
        "sup" => {
            style.display = Display::Inline;
            style.font_size = style.font_size * 0.75;
        }
        "sub" => {
            style.display = Display::Inline;
            style.font_size = style.font_size * 0.75;
        }
        "abbr" | "cite" | "dfn" | "mark" | "time" | "var" | "kbd" | "samp" => {
            style.display = Display::Inline;
        }
        "center" => {
            style.display = Display::Block;
            style.text_align = TextAlign::Center;
        }
        "html" => {
            style.display = Display::Block;
        }
        "body" => {
            style.display = Display::Block;
            style.margin_top = 8.0;
            style.margin_right = 8.0;
            style.margin_bottom = 8.0;
            style.margin_left = 8.0;
        }
        "br" => {
            style.display = Display::Block;
        }
        "hr" => {
            style.display = Display::Block;
            style.height = SizeValue::Px(1.0);
            style.border_top_width = 1.0;
            style.border_color = CssColor::from_rgb(0xcc, 0xcc, 0xcc);
            style.margin_top = 8.0;
            style.margin_bottom = 8.0;
        }
        "img" => {
            style.display = Display::InlineBlock;
        }
        "canvas" => {
            style.display = Display::InlineBlock;
            style.width = SizeValue::Px(300.0);
            style.height = SizeValue::Px(150.0);
        }
        "button" => {
            style.display = Display::InlineBlock;
            style.padding_top = 4.0;
            style.padding_right = 12.0;
            style.padding_bottom = 4.0;
            style.padding_left = 12.0;
            style.border_top_width = 1.0;
            style.border_right_width = 1.0;
            style.border_bottom_width = 1.0;
            style.border_left_width = 1.0;
            style.border_color = CssColor::from_rgb(0x76, 0x76, 0x76);
            style.background_color = CssColor::from_rgb(0xef, 0xef, 0xef);
        }
        "input" | "textarea" => {
            style.display = Display::InlineBlock;
            style.padding_top = 2.0;
            style.padding_right = 4.0;
            style.padding_bottom = 2.0;
            style.padding_left = 4.0;
            style.border_top_width = 1.0;
            style.border_right_width = 1.0;
            style.border_bottom_width = 1.0;
            style.border_left_width = 1.0;
            style.border_color = CssColor::from_rgb(0x76, 0x76, 0x76);
            style.width = SizeValue::Px(200.0);
        }
        "select" => {
            style.display = Display::InlineBlock;
            style.padding_top = 2.0;
            style.padding_right = 20.0; // room for dropdown arrow
            style.padding_bottom = 2.0;
            style.padding_left = 4.0;
            style.border_top_width = 1.0;
            style.border_right_width = 1.0;
            style.border_bottom_width = 1.0;
            style.border_left_width = 1.0;
            style.border_color = CssColor::from_rgb(0x76, 0x76, 0x76);
            style.background_color = CssColor::from_rgb(0xf8, 0xf8, 0xf8);
        }
        "option" => {
            // Only show selected/first option, hide rest
            style.display = Display::None;
        }
        _ => {}
    }
}

/// Parse an HTML color attribute value like "#ff6600", "#f60", "red", "white" etc.
fn parse_html_color(s: &str) -> Option<CssColor> {
    let s = s.trim();
    if s.starts_with('#') {
        let hex = &s[1..];
        match hex.len() {
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(CssColor::from_rgb(r, g, b))
            }
            3 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
                Some(CssColor::from_rgb(r, g, b))
            }
            _ => None,
        }
    } else {
        // Named colors
        match s.to_ascii_lowercase().as_str() {
            "black" => Some(CssColor::from_rgb(0, 0, 0)),
            "white" => Some(CssColor::from_rgb(255, 255, 255)),
            "red" => Some(CssColor::from_rgb(255, 0, 0)),
            "green" => Some(CssColor::from_rgb(0, 128, 0)),
            "blue" => Some(CssColor::from_rgb(0, 0, 255)),
            "yellow" => Some(CssColor::from_rgb(255, 255, 0)),
            "orange" => Some(CssColor::from_rgb(255, 165, 0)),
            "purple" => Some(CssColor::from_rgb(128, 0, 128)),
            "gray" | "grey" => Some(CssColor::from_rgb(128, 128, 128)),
            "silver" => Some(CssColor::from_rgb(192, 192, 192)),
            "navy" => Some(CssColor::from_rgb(0, 0, 128)),
            "teal" => Some(CssColor::from_rgb(0, 128, 128)),
            "maroon" => Some(CssColor::from_rgb(128, 0, 0)),
            "olive" => Some(CssColor::from_rgb(128, 128, 0)),
            "lime" => Some(CssColor::from_rgb(0, 255, 0)),
            "aqua" | "cyan" => Some(CssColor::from_rgb(0, 255, 255)),
            "fuchsia" | "magenta" => Some(CssColor::from_rgb(255, 0, 255)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ua_defaults() {
        let el = ElementData::new("h1");
        let mut style = ComputedStyle::default();
        apply_ua_defaults(&el, &mut style);
        assert_eq!(style.font_size, 32.0);
        assert_eq!(style.font_weight, FontWeight::Bold);
        assert_eq!(style.display, Display::Block);
    }

    #[test]
    fn test_head_hidden() {
        let el = ElementData::new("head");
        let mut style = ComputedStyle::default();
        apply_ua_defaults(&el, &mut style);
        assert_eq!(style.display, Display::None);
    }

    #[test]
    fn test_resolve_styles() {
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut div_el = ElementData::new("div");
        div_el
            .attributes
            .insert("class".to_string(), "red".to_string());
        let div = doc.add_node(body, NodeData::Element(div_el));

        let stylesheet =
            incognidium_css::parse_css(".red { color: red; background-color: blue; }");
        let styles = resolve_styles(&doc, &stylesheet);

        let div_style = styles.get(&div).unwrap();
        assert_eq!(div_style.color, CssColor::from_rgb(255, 0, 0));
    }
}
