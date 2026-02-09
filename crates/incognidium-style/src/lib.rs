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

            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::NoWrap,
            justify_content: JustifyContent::FlexStart,
            align_items: AlignItems::Stretch,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: SizeValue::Auto,
            gap: 0.0,

            overflow: Overflow::Visible,
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
            let mut style = compute_style_for_element(el, stylesheet, parent_style);
            apply_ua_defaults(el, &mut style);
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
    element: &ElementData,
    stylesheet: &Stylesheet,
    parent_style: &ComputedStyle,
) -> ComputedStyle {
    let mut style = ComputedStyle::default();

    // Inherit inheritable properties from parent
    style.color = parent_style.color;
    style.font_size = parent_style.font_size;
    style.font_weight = parent_style.font_weight;
    style.font_style = parent_style.font_style;
    style.text_align = parent_style.text_align;
    style.line_height = parent_style.line_height;

    // Collect matching rules and sort by specificity
    let mut matched = matching_rules(stylesheet, element);
    matched.sort_by_key(|m| m.specificity);

    // Apply rules in specificity order (lowest first, so highest wins)
    for matched_rule in &matched {
        for decl in &matched_rule.rule.declarations {
            apply_declaration(&mut style, decl, parent_style.font_size);
        }
    }

    // Apply inline styles (highest specificity)
    if let Some(inline) = element.get_attr("style") {
        let decls = parse_inline_style(inline);
        for decl in &decls {
            apply_declaration(&mut style, decl, parent_style.font_size);
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
            if let CssValue::Color(c) = &decl.value {
                style.color = *c;
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
            style.margin_top = 21.44;
            style.margin_bottom = 21.44;
        }
        "h2" => {
            style.display = Display::Block;
            style.font_size = 24.0;
            style.font_weight = FontWeight::Bold;
            style.margin_top = 19.92;
            style.margin_bottom = 19.92;
        }
        "h3" => {
            style.display = Display::Block;
            style.font_size = 18.72;
            style.font_weight = FontWeight::Bold;
            style.margin_top = 18.72;
            style.margin_bottom = 18.72;
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
            style.margin_top = 16.0;
            style.margin_bottom = 16.0;
        }
        "div" | "section" | "article" | "main" | "header" | "footer" | "nav" | "aside" => {
            style.display = Display::Block;
        }
        "span" | "small" | "sub" | "sup" => {
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
            style.margin_top = 16.0;
            style.margin_bottom = 16.0;
            style.padding_left = 40.0;
        }
        "li" => {
            style.display = Display::Block;
        }
        "head" | "style" | "script" | "link" | "meta" | "title" => {
            style.display = Display::None;
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
        "br" | "hr" => {
            style.display = Display::Block;
        }
        _ => {}
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
