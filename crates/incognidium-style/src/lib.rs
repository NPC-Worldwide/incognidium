use incognidium_css::{
    matching_rules, parse_css, parse_inline_style, CssColor, CssValue, Declaration, LengthUnit,
    Stylesheet,
};
use incognidium_dom::{Document, ElementData, NodeData, NodeId};
use std::collections::HashMap;
use std::sync::OnceLock;

static UA_STYLESHEET: OnceLock<Stylesheet> = OnceLock::new();

fn ua_stylesheet() -> &'static Stylesheet {
    UA_STYLESHEET.get_or_init(|| parse_css(UA_CSS))
}

const UA_CSS: &str = r#"
html, body { display: block; margin: 0; }
head, style, script, link, meta, title, template, svg, datalist, dialog { display: none; }
noscript { display: none; }
h1 { display: block; font-size: 2em; font-weight: bold; margin-top: 0.67em; margin-bottom: 0.67em; }
h2 { display: block; font-size: 1.5em; font-weight: bold; margin-top: 0.83em; margin-bottom: 0.83em; }
h3 { display: block; font-size: 1.17em; font-weight: bold; margin-top: 1em; margin-bottom: 1em; }
h4 { display: block; font-weight: bold; margin-top: 1.33em; margin-bottom: 1.33em; }
h5 { display: block; font-size: 0.83em; font-weight: bold; margin-top: 1.67em; margin-bottom: 1.67em; }
h6 { display: block; font-size: 0.67em; font-weight: bold; margin-top: 2.33em; margin-bottom: 2.33em; }
p { display: block; margin-top: 1em; margin-bottom: 1em; }
div, article, section, main, header, footer, aside, details, summary, figure, figcaption { display: block; }
nav, address, hgroup, search { display: block; }
blockquote { display: block; margin-top: 1em; margin-bottom: 1em; margin-left: 40px; margin-right: 40px; }
pre { display: block; margin-top: 1em; margin-bottom: 1em; white-space: pre; }
ul, ol { display: block; margin-top: 0.25em; margin-bottom: 0.25em; padding-left: 24px; }
li { display: block; }
dl { display: block; margin-top: 1em; margin-bottom: 1em; }
dt { display: block; font-weight: bold; }
dd { display: block; margin-left: 40px; }
table { display: block; }
thead, tbody, tfoot { display: block; }
tr { display: flex; }
td, th { display: block; padding: 1px; }
th { font-weight: bold; }
caption { display: block; text-align: center; }
hr { display: block; margin-top: 0.5em; margin-bottom: 0.5em; border-top: 1px solid #cccccc; }
a { display: inline; color: #0645ad; text-decoration: underline; }
strong, b { display: inline; font-weight: bold; }
em, i { display: inline; font-style: italic; }
u, ins { display: inline; text-decoration: underline; }
s, strike, del { display: inline; text-decoration: line-through; }
small { display: inline; font-size: 0.875em; }
sub, sup { display: inline; font-size: 0.75em; }
code, kbd, samp, tt { display: inline; }
span { display: inline; }
br { display: block; }
img { display: inline; }
center { display: block; text-align: center; }
form { display: block; }
fieldset { display: block; margin: 0.5em 2px; padding: 0.5em; border: 1px solid #cccccc; }
legend { display: block; }
input, textarea { display: inline; padding: 2px 4px; border: 1px solid #767676; width: 200px; }
select { display: inline; padding: 2px 20px 2px 4px; border: 1px solid #767676; background-color: #f8f8f8; }
button { display: inline; padding: 2px 8px; border: 1px solid #767676; }
label { display: inline; }
canvas { display: inline; width: 300px; height: 150px; }
"#;

/// Computed style values for a single element.
#[derive(Debug, Clone)]
pub struct ComputedStyle {
    pub display: Display,
    pub position: Position,
    pub float: Float,
    pub color: CssColor,
    pub background_color: CssColor,
    pub font_size: f32,
    pub font_weight: FontWeight,
    pub font_style: FontStyle,
    pub text_align: TextAlign,
    pub text_indent: f32,
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

    // CSS Grid
    pub grid_template_columns: Vec<GridTrackSize>,
    pub grid_template_rows: Vec<GridTrackSize>,
    pub grid_template_areas: Vec<Vec<String>>, // Each row is a vec of area names
    pub grid_auto_flow: GridAutoFlow,
    pub column_gap: f32,
    pub row_gap: f32,
    pub grid_column_start: Option<i32>,
    pub grid_column_end: Option<i32>,
    pub grid_row_start: Option<i32>,
    pub grid_row_end: Option<i32>,
    pub grid_area: Option<String>,

    // Positioning
    pub top: SizeValue,
    pub right: SizeValue,
    pub bottom: SizeValue,
    pub left: SizeValue,

    pub overflow: Overflow,
    pub visibility: Visibility,
    pub opacity: f32,
    pub text_transform: TextTransform,
    pub white_space: WhiteSpace,
    pub box_sizing: BoxSizing,
    pub z_index: i32,
    pub order: i32,
    pub list_style_type: ListStyleType,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ListStyleType {
    Disc,
    Decimal,
    None,
}

impl Default for ComputedStyle {
    fn default() -> Self {
        ComputedStyle {
            display: Display::Block,
            position: Position::Static,
            float: Float::None,
            color: CssColor::BLACK,
            background_color: CssColor::TRANSPARENT,
            font_size: 16.0,
            font_weight: FontWeight::Normal,
            font_style: FontStyle::Normal,
            text_align: TextAlign::Left,
            text_indent: 0.0,
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

            grid_template_columns: Vec::new(),
            grid_template_rows: Vec::new(),
            grid_template_areas: Vec::new(),
            grid_auto_flow: GridAutoFlow::Row,
            column_gap: 0.0,
            row_gap: 0.0,
            grid_column_start: None,
            grid_column_end: None,
            grid_row_start: None,
            grid_row_end: None,
            grid_area: None,

            top: SizeValue::Auto,
            right: SizeValue::Auto,
            bottom: SizeValue::Auto,
            left: SizeValue::Auto,

            overflow: Overflow::Visible,
            visibility: Visibility::Visible,
            opacity: 1.0,
            text_transform: TextTransform::None,
            white_space: WhiteSpace::Normal,
            box_sizing: BoxSizing::ContentBox,
            z_index: 0,
            order: 0,
            list_style_type: ListStyleType::Disc,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Display {
    Block,
    Inline,
    Flex,
    Grid,
    InlineBlock,
    Contents,
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
pub enum BoxSizing {
    ContentBox,
    BorderBox,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Float {
    None,
    Left,
    Right,
}

/// A single track size in a CSS Grid template.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GridTrackSize {
    Px(f32),
    Percent(f32),
    Fr(f32),
    Auto,
    /// minmax(min, max) — stores (min_px, max_fr_or_px)
    MinMax(f32, f32),
}

/// Grid auto-flow direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GridAutoFlow {
    Row,
    Column,
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
pub fn resolve_styles(
    doc: &Document,
    stylesheet: &Stylesheet,
    viewport_width: f32,
    viewport_height: f32,
) -> StyleMap {
    let mut styles = HashMap::new();
    let root = doc.root();
    let default_style = ComputedStyle::default();
    resolve_node(
        doc,
        stylesheet,
        root,
        &default_style,
        &mut styles,
        viewport_width,
        viewport_height,
    );
    styles
}

fn resolve_node(
    doc: &Document,
    stylesheet: &Stylesheet,
    node_id: NodeId,
    parent_style: &ComputedStyle,
    styles: &mut StyleMap,
    viewport_width: f32,
    viewport_height: f32,
) {
    let node = doc.node(node_id);
    let style = match &node.data {
        NodeData::Element(el) => {
            let style = compute_style_for_element(
                doc,
                node_id,
                el,
                stylesheet,
                parent_style,
                viewport_width,
                viewport_height,
            );
            styles.insert(node_id, style.clone());
            style
        }
        NodeData::Text(_) => {
            // Text nodes inherit from parent
            let mut style = parent_style.clone();
            // Preserve display:none from parent (e.g. <style>, <script> text content)
            if style.display != Display::None {
                style.display = Display::Inline;
            }
            styles.insert(node_id, style.clone());
            style
        }
        _ => {
            styles.insert(node_id, parent_style.clone());
            parent_style.clone()
        }
    };

    // If this node is display:none, all descendants are also hidden — skip recursion
    if style.display == Display::None {
        fn hide_descendants(
            doc: &Document,
            node_id: NodeId,
            parent_style: &ComputedStyle,
            styles: &mut StyleMap,
        ) {
            let children = doc.node(node_id).children.clone();
            for child_id in children {
                let mut hidden = parent_style.clone();
                hidden.display = Display::None;
                styles.insert(child_id, hidden.clone());
                hide_descendants(doc, child_id, &hidden, styles);
            }
        }
        hide_descendants(doc, node_id, &style, styles);
        return;
    }

    // <details> without open: hide children except <summary>
    let is_closed_details = matches!(&node.data, NodeData::Element(el) if el.tag_name == "details" && el.get_attr("open").is_none());

    let children = doc.node(node_id).children.clone();
    for child_id in children {
        if is_closed_details {
            let child = doc.node(child_id);
            let is_summary =
                matches!(&child.data, NodeData::Element(el) if el.tag_name == "summary");
            if !is_summary {
                // Force hidden for non-summary children of closed <details>
                let mut hidden = style.clone();
                hidden.display = Display::None;
                styles.insert(child_id, hidden.clone());
                continue;
            }
        }
        resolve_node(
            doc,
            stylesheet,
            child_id,
            &style,
            styles,
            viewport_width,
            viewport_height,
        );
    }
}

/// Compute style for an element by matching CSS rules + inline styles.
fn compute_style_for_element(
    doc: &Document,
    node_id: NodeId,
    element: &ElementData,
    stylesheet: &Stylesheet,
    parent_style: &ComputedStyle,
    viewport_width: f32,
    viewport_height: f32,
) -> ComputedStyle {
    // 1. Inherit inheritable properties from parent first
    let mut style = ComputedStyle {
        color: parent_style.color,
        font_size: parent_style.font_size,
        font_weight: parent_style.font_weight,
        font_style: parent_style.font_style,
        text_align: parent_style.text_align,
        text_indent: parent_style.text_indent,
        line_height: parent_style.line_height,
        visibility: parent_style.visibility,
        text_transform: parent_style.text_transform,
        white_space: parent_style.white_space,
        list_style_type: parent_style.list_style_type,
        ..Default::default()
    };

    // 2. Apply UA stylesheet (lowest priority in cascade)
    let ua = ua_stylesheet();
    let ua_matched = matching_rules(ua, element, doc, node_id);
    for m in &ua_matched {
        for decl in &m.rule.declarations {
            apply_declaration(
                &mut style,
                decl,
                parent_style.font_size,
                viewport_width,
                viewport_height,
            );
        }
    }

    // 3. Apply page CSS rules (author origin, overrides UA)
    let mut matched = matching_rules(stylesheet, element, doc, node_id);
    matched.sort_by_key(|m| m.specificity);

    for matched_rule in &matched {
        for decl in &matched_rule.rule.declarations {
            if !decl.important {
                let resolved = resolve_var(&decl.value, &stylesheet.variables);
                let resolved_decl = Declaration {
                    property: decl.property.clone(),
                    value: resolved,
                    important: false,
                };
                apply_declaration(
                    &mut style,
                    &resolved_decl,
                    parent_style.font_size,
                    viewport_width,
                    viewport_height,
                );
            }
        }
    }
    for matched_rule in &matched {
        for decl in &matched_rule.rule.declarations {
            if decl.important {
                let resolved = resolve_var(&decl.value, &stylesheet.variables);
                let resolved_decl = Declaration {
                    property: decl.property.clone(),
                    value: resolved,
                    important: true,
                };
                apply_declaration(
                    &mut style,
                    &resolved_decl,
                    parent_style.font_size,
                    viewport_width,
                    viewport_height,
                );
            }
        }
    }

    // (Previous class/id blocklist for sidebar/offcanvas/vector-menu removed —
    // it was too broad and hid real content wrappers like .offcanvas-wrapper
    // on TheIntercept and similar WordPress themes. Let CSS drive visibility.)

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
            apply_declaration(
                &mut style,
                decl,
                parent_style.font_size,
                viewport_width,
                viewport_height,
            );
        }
    }

    // Table cells: last td in a row gets flex-grow to fill remaining space
    if element.tag_name == "td" || element.tag_name == "th" {
        if !matches!(style.width, SizeValue::Auto | SizeValue::None) {
            style.flex_grow = 0.0;
        } else if let Some(parent_id) = doc.node(node_id).parent {
            if let NodeData::Element(ref pel) = doc.node(parent_id).data {
                if pel.tag_name == "tr" {
                    let is_last = doc.node(parent_id).children.iter().rev()
                        .find(|&&sid| matches!(&doc.node(sid).data, NodeData::Element(e) if e.tag_name == "td" || e.tag_name == "th"))
                        .map(|&sid| sid == node_id)
                        .unwrap_or(false);
                    if is_last {
                        style.flex_grow = 1.0;
                    }
                }
            }
        }
    }

    // Elements with height:0 + overflow:hidden are effectively invisible
    if matches!(style.height, SizeValue::Px(h) if h == 0.0)
        && matches!(style.overflow, Overflow::Hidden)
    {
        style.display = Display::None;
    }

    // Elements with max-height:0 + overflow:hidden (common toggle pattern)
    if matches!(style.max_height, SizeValue::Px(h) if h == 0.0)
        && matches!(style.overflow, Overflow::Hidden)
    {
        style.display = Display::None;
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
                        .find(|&&cid| matches!(&doc.node(cid).data, NodeData::Element(ref e) if e.tag_name == "option"))
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
    if element
        .get_attr("aria-hidden")
        .map(|v| v == "true")
        .unwrap_or(false)
    {
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

/// Resolve CSS var() references in a value using the stylesheet's variable map.
fn resolve_var(value: &CssValue, variables: &HashMap<String, String>) -> CssValue {
    resolve_var_depth(value, variables, 0)
}

fn resolve_var_depth(
    value: &CssValue,
    variables: &HashMap<String, String>,
    depth: u32,
) -> CssValue {
    if depth > 8 {
        return value.clone();
    }
    match value {
        CssValue::Var(var_name, fallback) => {
            if let Some(resolved_str) = variables.get(var_name) {
                // Detect self-referential variables.
                let is_self_ref = resolved_str.contains(&format!("var({}", var_name));
                if !is_self_ref {
                    let decls = parse_inline_style(&format!("__x: {}", resolved_str));
                    if let Some(d) = decls.first() {
                        return resolve_var_depth(&d.value, variables, depth + 1);
                    }
                }
            }
            if let Some(fb) = fallback {
                return resolve_var_depth(fb.as_ref(), variables, depth + 1);
            }
            CssValue::Inherit
        }
        CssValue::List(items) => CssValue::List(
            items
                .iter()
                .map(|v| resolve_var_depth(v, variables, depth + 1))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn apply_declaration(
    style: &mut ComputedStyle,
    decl: &Declaration,
    parent_font_size: f32,
    viewport_width: f32,
    viewport_height: f32,
) {
    match decl.property.as_str() {
        "display" => {
            if matches!(&decl.value, CssValue::None) {
                style.display = Display::None;
                return;
            }
            if let CssValue::Keyword(kw) = &decl.value {
                style.display = match kw.as_str() {
                    "block" => Display::Block,
                    "inline" => Display::Inline,
                    "flex" => Display::Flex,
                    "inline-block" => Display::InlineBlock,
                    "none" => Display::None,
                    "grid" => Display::Grid,
                    "inline-flex" => Display::Flex,
                    "inline-grid" => Display::Grid,
                    "list-item" => Display::Block,
                    "table" | "table-row-group" | "table-header-group" | "table-footer-group"
                    | "table-caption" => Display::Block,
                    "table-row" => {
                        style.flex_direction = FlexDirection::Row;
                        Display::Flex
                    }
                    "table-cell" => Display::InlineBlock,
                    "contents" => Display::Contents,
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
        "top" => {
            style.top = to_size_value(
                &decl.value,
                parent_font_size,
                viewport_width,
                viewport_height,
            );
        }
        "right" => {
            style.right = to_size_value(
                &decl.value,
                parent_font_size,
                viewport_width,
                viewport_height,
            );
        }
        "bottom" => {
            style.bottom = to_size_value(
                &decl.value,
                parent_font_size,
                viewport_width,
                viewport_height,
            );
        }
        "left" => {
            style.left = to_size_value(
                &decl.value,
                parent_font_size,
                viewport_width,
                viewport_height,
            );
        }
        "inset" => {
            // inset shorthand: 1-4 values map to top/right/bottom/left.
            let values: Vec<CssValue> = match &decl.value {
                CssValue::List(vals) => vals.clone(),
                other => vec![other.clone()],
            };
            let (t, r, b, l) = match values.len() {
                1 => (&values[0], &values[0], &values[0], &values[0]),
                2 => (&values[0], &values[1], &values[0], &values[1]),
                3 => (&values[0], &values[1], &values[2], &values[1]),
                _ => (&values[0], &values[1], &values[2], &values[3]),
            };
            style.top = to_size_value(t, parent_font_size, viewport_width, viewport_height);
            style.right = to_size_value(r, parent_font_size, viewport_width, viewport_height);
            style.bottom = to_size_value(b, parent_font_size, viewport_width, viewport_height);
            style.left = to_size_value(l, parent_font_size, viewport_width, viewport_height);
        }
        "float" => match &decl.value {
            CssValue::None => style.float = Float::None,
            CssValue::Keyword(kw) => {
                style.float = match kw.as_str() {
                    "left" => Float::Left,
                    "right" => Float::Right,
                    "none" => Float::None,
                    _ => style.float,
                };
            }
            _ => {}
        },
        "clear" => {
            // clear property — skip for now but don't error
        }
        "color" => {
            match &decl.value {
                CssValue::Color(c) => style.color = *c,
                CssValue::Inherit => {} // already inherited, no-op
                _ => {}
            }
        }
        "background-color" => {
            if let CssValue::Color(c) = &decl.value {
                style.background_color = *c;
            }
        }
        "background" => {
            // background shorthand — extract color from any position
            match &decl.value {
                CssValue::Color(c) => style.background_color = *c,
                CssValue::Keyword(kw) if kw == "none" || kw == "transparent" => {
                    style.background_color = CssColor::TRANSPARENT;
                }
                CssValue::List(vals) => {
                    for v in vals {
                        if let CssValue::Color(c) = v {
                            style.background_color = *c;
                        }
                    }
                }
                _ => {}
            }
        }
        "font-size" => {
            if let CssValue::Inherit = &decl.value {
                style.font_size = parent_font_size;
            } else if let Some(px) =
                decl.value
                    .to_px(parent_font_size, viewport_width, viewport_height)
            {
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
                    "smaller" => (parent_font_size * 0.833).max(9.0),
                    "larger" => parent_font_size * 1.2,
                    _ => style.font_size,
                };
            }
        }
        "font-weight" => {
            style.font_weight = match &decl.value {
                CssValue::Keyword(kw) => match kw.as_str() {
                    "bold" | "bolder" => FontWeight::Bold,
                    "normal" | "lighter" => FontWeight::Normal,
                    _ => style.font_weight,
                },
                CssValue::Number(n) if *n >= 600.0 => FontWeight::Bold,
                CssValue::Number(_) => FontWeight::Normal,
                CssValue::Inherit => style.font_weight, // already inherited
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
        "text-decoration" | "text-decoration-line" => {
            let vals = match &decl.value {
                CssValue::List(v) => v.clone(),
                other => vec![other.clone()],
            };
            for v in &vals {
                match v {
                    CssValue::None => style.text_decoration = TextDecoration::None,
                    CssValue::Keyword(kw) => {
                        style.text_decoration = match kw.as_str() {
                            "underline" => TextDecoration::Underline,
                            "line-through" => TextDecoration::LineThrough,
                            "none" => TextDecoration::None,
                            _ => style.text_decoration,
                        };
                    }
                    _ => {}
                }
            }
        }
        "line-height" => {
            if let CssValue::Number(n) = &decl.value {
                style.line_height = *n;
            } else if let Some(px) =
                decl.value
                    .to_px(parent_font_size, viewport_width, viewport_height)
            {
                style.line_height = px / style.font_size;
            }
        }
        "text-indent" => {
            if let Some(px) = decl
                .value
                .to_px(parent_font_size, viewport_width, viewport_height)
            {
                style.text_indent = px;
            }
        }
        "margin" => apply_box_shorthand_margin(
            style,
            &decl.value,
            parent_font_size,
            viewport_width,
            viewport_height,
        ),
        "margin-top" => {
            if let Some(px) = decl
                .value
                .to_px(parent_font_size, viewport_width, viewport_height)
            {
                style.margin_top = px;
            }
        }
        "margin-right" => {
            if let Some(px) = decl
                .value
                .to_px(parent_font_size, viewport_width, viewport_height)
            {
                style.margin_right = px;
            }
        }
        "margin-bottom" => {
            if let Some(px) = decl
                .value
                .to_px(parent_font_size, viewport_width, viewport_height)
            {
                style.margin_bottom = px;
            }
        }
        "margin-left" => {
            if let Some(px) = decl
                .value
                .to_px(parent_font_size, viewport_width, viewport_height)
            {
                style.margin_left = px;
            }
        }
        "padding" => apply_box_shorthand_padding(
            style,
            &decl.value,
            parent_font_size,
            viewport_width,
            viewport_height,
        ),
        "padding-top" => {
            if let Some(px) = decl
                .value
                .to_px(parent_font_size, viewport_width, viewport_height)
            {
                style.padding_top = px;
            }
        }
        "padding-right" => {
            if let Some(px) = decl
                .value
                .to_px(parent_font_size, viewport_width, viewport_height)
            {
                style.padding_right = px;
            }
        }
        "padding-bottom" => {
            if let Some(px) = decl
                .value
                .to_px(parent_font_size, viewport_width, viewport_height)
            {
                style.padding_bottom = px;
            }
        }
        "padding-left" => {
            if let Some(px) = decl
                .value
                .to_px(parent_font_size, viewport_width, viewport_height)
            {
                style.padding_left = px;
            }
        }
        "border-width" => {
            if let Some(px) = decl
                .value
                .to_px(parent_font_size, viewport_width, viewport_height)
            {
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
            // border: <width> <style> <color>
            let vals = match &decl.value {
                CssValue::List(v) => v.clone(),
                other => vec![other.clone()],
            };
            for v in &vals {
                if let Some(px) = v.to_px(parent_font_size, viewport_width, viewport_height) {
                    style.border_top_width = px;
                    style.border_right_width = px;
                    style.border_bottom_width = px;
                    style.border_left_width = px;
                }
                if let CssValue::Color(c) = v {
                    style.border_color = *c;
                }
                if let CssValue::Keyword(kw) = v {
                    if kw == "none" {
                        style.border_top_width = 0.0;
                        style.border_right_width = 0.0;
                        style.border_bottom_width = 0.0;
                        style.border_left_width = 0.0;
                    }
                }
            }
            if style.border_top_width > 0.0 && style.border_color.a == 0 {
                style.border_color = CssColor::from_rgb(0, 0, 0);
            }
        }
        "width" => {
            style.width = to_size_value(
                &decl.value,
                parent_font_size,
                viewport_width,
                viewport_height,
            );
        }
        "height" => {
            style.height = to_size_value(
                &decl.value,
                parent_font_size,
                viewport_width,
                viewport_height,
            );
        }
        "min-width" => {
            style.min_width = to_size_value(
                &decl.value,
                parent_font_size,
                viewport_width,
                viewport_height,
            );
        }
        "max-width" => {
            style.max_width = to_size_value(
                &decl.value,
                parent_font_size,
                viewport_width,
                viewport_height,
            );
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
            style.flex_basis = to_size_value(
                &decl.value,
                parent_font_size,
                viewport_width,
                viewport_height,
            );
        }
        "flex" => {
            match &decl.value {
                CssValue::List(vals) => {
                    // flex: <grow> <shrink> <basis>
                    if let Some(CssValue::Number(g)) = vals.first() {
                        style.flex_grow = *g;
                    }
                    if let Some(CssValue::Number(s)) = vals.get(1) {
                        style.flex_shrink = *s;
                    }
                    if let Some(basis) = vals.get(2) {
                        style.flex_basis =
                            to_size_value(basis, parent_font_size, viewport_width, viewport_height);
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
        "gap" | "grid-gap" => {
            if let Some(px) = decl
                .value
                .to_px(parent_font_size, viewport_width, viewport_height)
            {
                style.gap = px;
                style.column_gap = px;
                style.row_gap = px;
            }
        }
        "column-gap" | "grid-column-gap" => {
            if let Some(px) = decl
                .value
                .to_px(parent_font_size, viewport_width, viewport_height)
            {
                style.gap = px; // flex compat
                style.column_gap = px;
            }
        }
        "row-gap" | "grid-row-gap" => {
            if let Some(px) = decl
                .value
                .to_px(parent_font_size, viewport_width, viewport_height)
            {
                style.row_gap = px;
            }
        }
        "overflow" | "overflow-x" | "overflow-y" => {
            // Accept single keyword or two-value shorthand (e.g. "hidden auto")
            let kws: Vec<String> = match &decl.value {
                CssValue::Keyword(k) => vec![k.to_lowercase()],
                CssValue::List(vals) => vals
                    .iter()
                    .filter_map(|v| {
                        if let CssValue::Keyword(k) = v {
                            Some(k.to_lowercase())
                        } else {
                            None
                        }
                    })
                    .collect(),
                _ => vec![],
            };
            if kws.iter().any(|k| k == "hidden") {
                style.overflow = Overflow::Hidden;
            } else if kws.iter().any(|k| k == "scroll") {
                style.overflow = Overflow::Scroll;
            } else if kws.iter().any(|k| k == "auto") {
                style.overflow = Overflow::Auto;
            } else if kws.iter().any(|k| k == "visible") {
                style.overflow = Overflow::Visible;
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
        "box-sizing" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.box_sizing = match kw.as_str() {
                    "border-box" => BoxSizing::BorderBox,
                    "content-box" => BoxSizing::ContentBox,
                    _ => style.box_sizing,
                };
            }
        }
        "opacity" => {
            if let Some(px) = decl
                .value
                .to_px(parent_font_size, viewport_width, viewport_height)
            {
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
        "list-style-type" | "list-style" => {
            let vals = match &decl.value {
                CssValue::List(v) => v.clone(),
                other => vec![other.clone()],
            };
            for v in &vals {
                match v {
                    CssValue::None => style.list_style_type = ListStyleType::None,
                    CssValue::Keyword(kw) => match kw.as_str() {
                        "none" => style.list_style_type = ListStyleType::None,
                        "disc" | "circle" | "square" => style.list_style_type = ListStyleType::Disc,
                        "decimal" => style.list_style_type = ListStyleType::Decimal,
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
        "min-height" => {
            style.min_height = to_size_value(
                &decl.value,
                parent_font_size,
                viewport_width,
                viewport_height,
            );
        }
        "max-height" => {
            style.max_height = to_size_value(
                &decl.value,
                parent_font_size,
                viewport_width,
                viewport_height,
            );
        }
        "border-top" | "border-right" | "border-bottom" | "border-left" => {
            let vals = match &decl.value {
                CssValue::List(v) => v.clone(),
                other => vec![other.clone()],
            };
            let mut width = 0.0f32;
            let mut color = style.border_color;
            let mut is_none = false;
            for v in &vals {
                if let Some(px) = v.to_px(parent_font_size, viewport_width, viewport_height) {
                    width = px;
                }
                if let CssValue::Color(c) = v {
                    color = *c;
                }
                if let CssValue::Keyword(kw) = v {
                    if kw == "none" {
                        is_none = true;
                    }
                }
            }
            if is_none {
                width = 0.0;
            }
            match decl.property.as_str() {
                "border-top" => style.border_top_width = width,
                "border-right" => style.border_right_width = width,
                "border-bottom" => style.border_bottom_width = width,
                "border-left" => style.border_left_width = width,
                _ => {}
            }
            if width > 0.0 {
                style.border_color = color;
            }
        }
        "border-top-width" => {
            if let Some(px) = decl
                .value
                .to_px(parent_font_size, viewport_width, viewport_height)
            {
                style.border_top_width = px;
            }
        }
        "border-right-width" => {
            if let Some(px) = decl
                .value
                .to_px(parent_font_size, viewport_width, viewport_height)
            {
                style.border_right_width = px;
            }
        }
        "border-bottom-width" => {
            if let Some(px) = decl
                .value
                .to_px(parent_font_size, viewport_width, viewport_height)
            {
                style.border_bottom_width = px;
            }
        }
        "border-left-width" => {
            if let Some(px) = decl
                .value
                .to_px(parent_font_size, viewport_width, viewport_height)
            {
                style.border_left_width = px;
            }
        }
        "border-top-color" | "border-right-color" | "border-bottom-color" | "border-left-color" => {
            if let CssValue::Color(c) = &decl.value {
                style.border_color = *c;
            }
        }
        "grid" | "grid-template" => {
            // grid: <rows> / <columns>  OR  grid: <columns-only>
            // The CSS parser encodes '/' as CssValue::Keyword("/")
            if let CssValue::List(vals) = &decl.value {
                // Find the '/' separator
                if let Some(slash_pos) = vals
                    .iter()
                    .position(|v| matches!(v, CssValue::Keyword(k) if k == "/"))
                {
                    let row_vals: Vec<CssValue> = vals[..slash_pos].to_vec();
                    let col_vals: Vec<CssValue> = vals[slash_pos + 1..].to_vec();
                    if !row_vals.is_empty() {
                        let row_value = if row_vals.len() == 1 {
                            row_vals.into_iter().next().unwrap()
                        } else {
                            CssValue::List(row_vals)
                        };
                        style.grid_template_rows = parse_grid_tracks(
                            &row_value,
                            parent_font_size,
                            viewport_width,
                            viewport_height,
                        );
                    }
                    if !col_vals.is_empty() {
                        let col_value = if col_vals.len() == 1 {
                            col_vals.into_iter().next().unwrap()
                        } else {
                            CssValue::List(col_vals)
                        };
                        style.grid_template_columns = parse_grid_tracks(
                            &col_value,
                            parent_font_size,
                            viewport_width,
                            viewport_height,
                        );
                    }
                } else {
                    // No slash — treat as grid-template-columns
                    style.grid_template_columns = parse_grid_tracks(
                        &decl.value,
                        parent_font_size,
                        viewport_width,
                        viewport_height,
                    );
                }
            } else {
                // Single value — treat as grid-template-columns
                style.grid_template_columns = parse_grid_tracks(
                    &decl.value,
                    parent_font_size,
                    viewport_width,
                    viewport_height,
                );
            }
        }
        "grid-template-columns" => {
            let tracks = parse_grid_tracks(
                &decl.value,
                parent_font_size,
                viewport_width,
                viewport_height,
            );
            style.grid_template_columns = tracks;
        }
        "grid-template-rows" => {
            style.grid_template_rows = parse_grid_tracks(
                &decl.value,
                parent_font_size,
                viewport_width,
                viewport_height,
            );
        }
        "grid-auto-flow" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.grid_auto_flow = match kw.as_str() {
                    "row" => GridAutoFlow::Row,
                    "column" => GridAutoFlow::Column,
                    _ => style.grid_auto_flow,
                };
            }
        }
        "grid-column" | "grid-column-start" | "grid-column-end" => {
            parse_grid_placement(
                &decl.property,
                &decl.value,
                &mut style.grid_column_start,
                &mut style.grid_column_end,
            );
        }
        "grid-row" | "grid-row-start" | "grid-row-end" => {
            parse_grid_placement(
                &decl.property,
                &decl.value,
                &mut style.grid_row_start,
                &mut style.grid_row_end,
            );
        }
        "grid-template-areas" => {
            // Parse grid-template-areas like: "header header" "sidebar main" "footer footer"
            match &decl.value {
                CssValue::List(vals) => {
                    let mut areas: Vec<Vec<String>> = Vec::new();
                    for v in vals {
                        if let CssValue::Keyword(s) = v {
                            // Split the string by whitespace and quotes
                            let row: Vec<String> = s
                                .split_whitespace()
                                .map(|s| s.trim_matches('"').trim_matches('\'').to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                            if !row.is_empty() {
                                areas.push(row);
                            }
                        }
                    }
                    style.grid_template_areas = areas;
                }
                CssValue::Keyword(s) => {
                    let row: Vec<String> = s
                        .split_whitespace()
                        .map(|s| s.trim_matches('"').trim_matches('\'').to_string())
                        .filter(|s| !s.is_empty() && s != ".")
                        .collect();
                    if !row.is_empty() {
                        style.grid_template_areas = vec![row];
                    }
                }
                _ => {}
            }
        }
        "grid-area" => {
            // grid-area: <area-name> or grid-area: <row-start> / <col-start> / <row-end> / <col-end>
            match &decl.value {
                CssValue::Keyword(kw) => {
                    // Single name = grid area reference
                    style.grid_area = Some(kw.clone());
                }
                CssValue::List(vals) => {
                    // Could be area name or slash-separated values
                    let parts: Vec<&CssValue> = vals
                        .iter()
                        .filter(|v| !matches!(v, CssValue::Keyword(k) if k == "/"))
                        .collect();
                    if parts.len() == 1 {
                        if let CssValue::Keyword(name) = parts[0] {
                            style.grid_area = Some(name.clone());
                        }
                    }
                    // slash-separated row-start / col-start / row-end / col-end handled by grid_placement
                }
                _ => {}
            }
        }
        "grid-auto-columns" | "grid-auto-rows" => {
            // Not yet supported
        }
        "border-style" => {
            if let CssValue::Keyword(kw) = &decl.value {
                if kw == "none" || kw == "hidden" {
                    style.border_top_width = 0.0;
                    style.border_right_width = 0.0;
                    style.border_bottom_width = 0.0;
                    style.border_left_width = 0.0;
                }
            }
        }
        "outline" | "outline-width" | "outline-style" | "outline-color" => {
            // Outlines don't affect layout — skip
        }
        "transform"
        | "transition"
        | "animation"
        | "animation-name"
        | "animation-duration"
        | "transform-origin"
        | "will-change"
        | "backface-visibility"
        | "perspective" => {
            // Visual effects we don't support — skip silently
        }
        "cursor" | "pointer-events" | "user-select" | "touch-action" | "scroll-behavior" => {
            // Interaction properties — skip
        }
        "z-index" => match &decl.value {
            CssValue::Number(v) => style.z_index = *v as i32,
            CssValue::Keyword(k) if k == "auto" => style.z_index = 0,
            _ => {}
        },
        "order" => {
            if let CssValue::Number(v) = &decl.value {
                style.order = *v as i32
            }
        }
        "content" => {
            // ::before/::after content — skip
        }
        _ => {} // Unknown property, skip
    }
}

/// Convert a CssValue into a Vec of GridTrackSize entries.
fn parse_grid_tracks(
    value: &CssValue,
    parent_font_size: f32,
    viewport_width: f32,
    viewport_height: f32,
) -> Vec<GridTrackSize> {
    match value {
        CssValue::List(vals) => {
            let mut tracks = Vec::new();
            let mut i = 0;
            while i < vals.len() {
                // Check for minmax(...) encoded as [Keyword("minmax"), min, max]
                if let CssValue::Keyword(kw) = &vals[i] {
                    if kw == "minmax" && i + 2 < vals.len() {
                        let min_px = vals[i + 1]
                            .to_px(parent_font_size, viewport_width, viewport_height)
                            .unwrap_or(0.0);
                        let max_val = css_value_to_track(
                            &vals[i + 2],
                            parent_font_size,
                            viewport_width,
                            viewport_height,
                        );
                        let max_fr = match max_val {
                            GridTrackSize::Fr(f) => f,
                            GridTrackSize::Px(p) => p,
                            _ => 1.0,
                        };
                        tracks.push(GridTrackSize::MinMax(min_px, max_fr));
                        i += 3;
                        continue;
                    }
                }
                // Check for nested List (from repeat() or minmax())
                if let CssValue::List(inner) = &vals[i] {
                    if inner.len() >= 3 {
                        if let CssValue::Keyword(kw) = &inner[0] {
                            if kw == "minmax" {
                                let min_px = inner[1]
                                    .to_px(parent_font_size, viewport_width, viewport_height)
                                    .unwrap_or(0.0);
                                let max_val = css_value_to_track(
                                    &inner[2],
                                    parent_font_size,
                                    viewport_width,
                                    viewport_height,
                                );
                                let max_fr = match max_val {
                                    GridTrackSize::Fr(f) => f,
                                    GridTrackSize::Px(p) => p,
                                    _ => 1.0,
                                };
                                tracks.push(GridTrackSize::MinMax(min_px, max_fr));
                                i += 1;
                                continue;
                            }
                        }
                    }
                    // Flat list from repeat() — recurse
                    for v in inner {
                        tracks.push(css_value_to_track(
                            v,
                            parent_font_size,
                            viewport_width,
                            viewport_height,
                        ));
                    }
                    i += 1;
                    continue;
                }
                tracks.push(css_value_to_track(
                    &vals[i],
                    parent_font_size,
                    viewport_width,
                    viewport_height,
                ));
                i += 1;
            }
            tracks
        }
        other => {
            let t = css_value_to_track(other, parent_font_size, viewport_width, viewport_height);
            if t == GridTrackSize::Auto && matches!(other, CssValue::None | CssValue::Keyword(_)) {
                Vec::new() // "none" or unknown keyword means no explicit tracks
            } else {
                vec![t]
            }
        }
    }
}

/// Convert a single CssValue to a GridTrackSize.
fn css_value_to_track(
    value: &CssValue,
    parent_font_size: f32,
    viewport_width: f32,
    viewport_height: f32,
) -> GridTrackSize {
    match value {
        CssValue::Length(v, LengthUnit::Fr) => GridTrackSize::Fr(*v),
        CssValue::Percentage(p) => GridTrackSize::Percent(*p),
        CssValue::Auto => GridTrackSize::Auto,
        CssValue::None => GridTrackSize::Auto,
        CssValue::Keyword(k) if k == "min-content" || k == "max-content" || k == "fit-content" => {
            GridTrackSize::Px(0.0)
        }
        other => {
            if let Some(px) = other.to_px(parent_font_size, viewport_width, viewport_height) {
                GridTrackSize::Px(px)
            } else {
                GridTrackSize::Auto
            }
        }
    }
}

/// Parse grid-column / grid-row placement values.
/// Formats: "2", "1 / 3", "1 / span 2", "span 2", "1 / -1"
fn parse_grid_placement(
    prop: &str,
    value: &CssValue,
    start: &mut Option<i32>,
    end: &mut Option<i32>,
) {
    let text = match value {
        CssValue::Number(n) => {
            if prop.ends_with("-start") {
                *start = Some(*n as i32);
            } else if prop.ends_with("-end") {
                *end = Some(*n as i32);
            } else {
                *start = Some(*n as i32);
            }
            return;
        }
        CssValue::Keyword(k) => k.clone(),
        CssValue::List(vals) => vals
            .iter()
            .map(|v| match v {
                CssValue::Number(n) => format!("{}", *n as i32),
                CssValue::Keyword(k) => k.clone(),
                CssValue::Length(n, _) => format!("{}", *n as i32),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join(" "),
        CssValue::Length(n, _) => {
            if prop.ends_with("-start") {
                *start = Some(*n as i32);
            } else if prop.ends_with("-end") {
                *end = Some(*n as i32);
            } else {
                *start = Some(*n as i32);
            }
            return;
        }
        _ => return,
    };
    if prop.ends_with("-start") {
        if let Ok(n) = text.trim().parse::<i32>() {
            *start = Some(n);
        }
        return;
    }
    if prop.ends_with("-end") {
        if let Ok(n) = text.trim().parse::<i32>() {
            *end = Some(n);
        }
        return;
    }
    // Shorthand: "start / end"
    let parts: Vec<&str> = text.split('/').map(|s| s.trim()).collect();
    if let Some(s) = parts.first() {
        if let Some(rest) = s.strip_prefix("span") {
            if let Ok(n) = rest.trim().parse::<i32>() {
                *start = Some(1);
                *end = Some(1 + n);
            }
        } else if let Ok(n) = s.parse::<i32>() {
            *start = Some(n);
        }
    }
    if let Some(e) = parts.get(1) {
        if let Some(rest) = e.strip_prefix("span") {
            if let Ok(n) = rest.trim().parse::<i32>() {
                if let Some(s) = *start {
                    *end = Some(s + n);
                }
            }
        } else if let Ok(n) = e.parse::<i32>() {
            *end = Some(n);
        }
    }
}

fn to_size_value(
    value: &CssValue,
    parent_font_size: f32,
    viewport_width: f32,
    viewport_height: f32,
) -> SizeValue {
    match value {
        CssValue::Auto => SizeValue::Auto,
        CssValue::None => SizeValue::None,
        CssValue::Percentage(p) => SizeValue::Percent(*p),
        _ => {
            if let Some(px) = value.to_px(parent_font_size, viewport_width, viewport_height) {
                SizeValue::Px(px)
            } else {
                SizeValue::Auto
            }
        }
    }
}

fn apply_box_shorthand_margin(
    style: &mut ComputedStyle,
    value: &CssValue,
    pfs: f32,
    viewport_width: f32,
    viewport_height: f32,
) {
    match value {
        CssValue::List(vals) => {
            let px: Vec<f32> = vals
                .iter()
                .filter_map(|v| v.to_px(pfs, viewport_width, viewport_height))
                .collect();
            match px.len() {
                4 => {
                    style.margin_top = px[0];
                    style.margin_right = px[1];
                    style.margin_bottom = px[2];
                    style.margin_left = px[3];
                }
                3 => {
                    style.margin_top = px[0];
                    style.margin_right = px[1];
                    style.margin_bottom = px[2];
                    style.margin_left = px[1];
                }
                2 => {
                    style.margin_top = px[0];
                    style.margin_right = px[1];
                    style.margin_bottom = px[0];
                    style.margin_left = px[1];
                }
                1 => {
                    style.margin_top = px[0];
                    style.margin_right = px[0];
                    style.margin_bottom = px[0];
                    style.margin_left = px[0];
                }
                _ => {}
            }
            // Handle auto in 2-value: margin: 0 auto
            if vals.len() >= 2 && matches!(vals[1], CssValue::Auto) {
                style.margin_left = 0.0;
                style.margin_right = 0.0;
                // Auto margins are handled in layout (centering)
            }
        }
        _ => {
            if let Some(px) = value.to_px(pfs, viewport_width, viewport_height) {
                style.margin_top = px;
                style.margin_right = px;
                style.margin_bottom = px;
                style.margin_left = px;
            }
        }
    }
}

fn apply_box_shorthand_padding(
    style: &mut ComputedStyle,
    value: &CssValue,
    pfs: f32,
    viewport_width: f32,
    viewport_height: f32,
) {
    match value {
        CssValue::List(vals) => {
            let px: Vec<f32> = vals
                .iter()
                .filter_map(|v| v.to_px(pfs, viewport_width, viewport_height))
                .collect();
            match px.len() {
                4 => {
                    style.padding_top = px[0];
                    style.padding_right = px[1];
                    style.padding_bottom = px[2];
                    style.padding_left = px[3];
                }
                3 => {
                    style.padding_top = px[0];
                    style.padding_right = px[1];
                    style.padding_bottom = px[2];
                    style.padding_left = px[1];
                }
                2 => {
                    style.padding_top = px[0];
                    style.padding_right = px[1];
                    style.padding_bottom = px[0];
                    style.padding_left = px[1];
                }
                1 => {
                    style.padding_top = px[0];
                    style.padding_right = px[0];
                    style.padding_bottom = px[0];
                    style.padding_left = px[0];
                }
                _ => {}
            }
        }
        _ => {
            if let Some(px) = value.to_px(pfs, viewport_width, viewport_height) {
                style.padding_top = px;
                style.padding_right = px;
                style.padding_bottom = px;
                style.padding_left = px;
            }
        }
    }
}

// Old apply_ua_defaults removed — UA styles now come from UA_CSS parsed

/// Parse an HTML color attribute value like "#ff6600", "#f60", "red", "white" etc.
fn parse_html_color(s: &str) -> Option<CssColor> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
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
    fn test_ua_stylesheet_parses() {
        let ua = ua_stylesheet();
        assert!(ua.rules.len() > 30, "UA stylesheet should have 30+ rules");
    }

    #[test]
    fn test_ua_h1_styles() {
        use incognidium_css::parse_css;
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let h1 = doc.add_node(body, NodeData::Element(ElementData::new("h1")));
        let empty = parse_css("");
        let styles = resolve_styles(&doc, &empty, 1024.0, 768.0);
        let s = styles.get(&h1).unwrap();
        assert_eq!(s.display, Display::Block);
        assert_eq!(s.font_weight, FontWeight::Bold);
        assert!(s.font_size > 30.0, "h1 should be ~32px");
    }

    #[test]
    fn test_ua_head_hidden() {
        use incognidium_css::parse_css;
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let head = doc.add_node(html, NodeData::Element(ElementData::new("head")));
        let empty = parse_css("");
        let styles = resolve_styles(&doc, &empty, 1024.0, 768.0);
        let s = styles.get(&head).unwrap();
        assert_eq!(s.display, Display::None);
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

        let stylesheet = incognidium_css::parse_css(".red { color: red; background-color: blue; }");
        let styles = resolve_styles(&doc, &stylesheet, 1024.0, 768.0);

        let div_style = styles.get(&div).unwrap();
        assert_eq!(div_style.color, CssColor::from_rgb(255, 0, 0));
    }
}
