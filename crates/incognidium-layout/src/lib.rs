use incognidium_dom::{Document, NodeData, NodeId};
use std::collections::HashMap;

/// Float state passed from parent blocks to child blocks.
#[derive(Clone, Copy, Default)]
pub struct FloatState {
    pub left_width: f32,
    pub right_width: f32,
    pub remaining_height: f32,
}
use incognidium_style::{
    AlignItems, ContentVisibility, Display, FlexDirection, FlexWrap, Float, GridTrackSize, JustifyContent, JustifyItems, ListStylePosition, Overflow,
    Position, SizeValue, StyleMap, TextAlign, TextAlignLast, TextJustify, TextTransform, Visibility, WhiteSpaceCollapse, TextWrap,
    format_counter_value, CounterStyle,
};

/// Counter state for CSS counters
#[derive(Clone, Default)]
struct CounterState {
    /// Map of counter name to current value
    values: HashMap<String, i32>,
}

impl CounterState {
    fn get(&self, name: &str) -> i32 {
        *self.values.get(name).unwrap_or(&0)
    }

    fn set(&mut self, name: &str, value: i32) {
        self.values.insert(name.to_string(), value);
    }

    fn increment(&mut self, name: &str, delta: i32) {
        let current = self.get(name);
        self.set(name, current + delta);
    }
}

/// Resolve a Content value to text, using the provided counter state.
/// Returns None if the content should not generate a text box.
fn resolve_content_to_text(
    content: &incognidium_style::Content,
    counters: &CounterState,
    quotes: &incognidium_style::Quotes,
    quote_depth: usize,
) -> Option<String> {
    use incognidium_style::Content;

    match content {
        Content::Text(text) => Some(text.clone()),
        Content::OpenQuote => Some(quotes.open_quote(quote_depth)),
        Content::CloseQuote => Some(quotes.close_quote(quote_depth)),
        Content::NoOpenQuote | Content::NoCloseQuote => None,
        Content::Counter(name, style) => {
            let value = counters.get(name);
            Some(format_counter_value(value, style))
        }
        Content::Counters(name, _separator, style) => {
            // For counters(), we would need to track the full counter stack
            // For now, just use the current value (simplified)
            let value = counters.get(name);
            Some(format_counter_value(value, style))
        }
        Content::Parts(parts) => {
            let mut result = String::new();
            for part in parts {
                if let Some(text) = resolve_content_to_text(part, counters, quotes, quote_depth) {
                    result.push_str(&text);
                }
            }
            if result.is_empty() {
                None
            } else {
                Some(result)
            }
        }
        _ => None,
    }
}

/// Image dimensions: (width, height) keyed by image src.
pub type ImageSizes = HashMap<String, (u32, u32)>;

/// Input element types for special rendering
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputType {
    Text,
    Checkbox { checked: bool },
    Radio { checked: bool },
    Button,
    Submit,
    Hidden,
    Other,
}

/// Textarea element info for sizing
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextAreaInfo {
    pub rows: u32,
    pub cols: u32,
}

/// Calculate the intrinsic width of a layout box (shrink-to-fit content width).
/// For text boxes, returns the text width. For containers, returns the max child width.
fn calculate_intrinsic_width(lb: &LayoutBox) -> f32 {
    // For text boxes, use the content width directly (this is the natural text width
    // before any constraints are applied, especially important for nowrap text)
    if lb.box_type == BoxType::Text {
        if let Some(ref text) = lb.text {
            // content_width is set to natural width during text layout
            return lb.content_width.max(0.0);
        }
    }
    // For containers, use the max width of children
    let mut max_child_width: f32 = 0.0;
    for child in &lb.children {
        let child_intrinsic = calculate_intrinsic_width(child);
        max_child_width = max_child_width.max(child_intrinsic);
    }
    // If no children or all empty, use the box's own width
    if max_child_width > 0.0 {
        max_child_width
    } else {
        lb.content_width.min(lb.width)
    }
}

/// Evaluate a SizeValue (calc, min, max, clamp) to pixels using the containing block context
fn evaluate_size_value(
    value: &SizeValue,
    containing_width: f32,
    font_size: f32,
) -> Option<f32> {
    use incognidium_style::CalcExpression;
    use incognidium_style::CalcValue;

    fn evaluate_calc_value(val: &CalcValue, containing_width: f32, font_size: f32) -> f32 {
        match val {
            CalcValue::Px(v) => *v,
            CalcValue::Percent(p) => p / 100.0 * containing_width,
            CalcValue::Em(e) => e * font_size,
            CalcValue::Rem(r) => r * 16.0, // Default root font size
            CalcValue::Vw(v) => v * containing_width / 100.0, // Use containing_width as viewport proxy
            CalcValue::Vh(v) => v * containing_width / 100.0, // Use containing_width as viewport proxy
            // Container query units (treated similarly to viewport units for now)
            CalcValue::Cqw(v) => v * containing_width / 100.0,
            CalcValue::Cqh(v) => v * containing_width / 100.0, // Approximation
            CalcValue::Cqi(v) => v * containing_width / 100.0, // Inline size = width in horizontal writing
            CalcValue::Cqb(v) => v * containing_width / 100.0, // Block size approximation
            CalcValue::Cqmin(v) => v * containing_width.min(containing_width) / 100.0,
            CalcValue::Cqmax(v) => v * containing_width.max(containing_width) / 100.0,
        }
    }

    fn evaluate_calc_expr(
        expr: &CalcExpression,
        containing_width: f32,
        font_size: f32,
    ) -> f32 {
        match expr {
            CalcExpression::Value(v) => evaluate_calc_value(v, containing_width, font_size),
            CalcExpression::Add(a, b) => {
                evaluate_calc_expr(a, containing_width, font_size)
                    + evaluate_calc_expr(b, containing_width, font_size)
            }
            CalcExpression::Subtract(a, b) => {
                evaluate_calc_expr(a, containing_width, font_size)
                    - evaluate_calc_expr(b, containing_width, font_size)
            }
            CalcExpression::Multiply(a, f) => evaluate_calc_expr(a, containing_width, font_size) * f,
            CalcExpression::Divide(a, f) => {
                if *f == 0.0 {
                    0.0
                } else {
                    evaluate_calc_expr(a, containing_width, font_size) / f
                }
            }
        }
    }

    match value {
        SizeValue::Px(v) => Some(*v),
        SizeValue::Percent(p) => Some(p / 100.0 * containing_width),
        SizeValue::Calc(expr) => Some(evaluate_calc_expr(expr, containing_width, font_size)),
        SizeValue::Min(vals) => {
            let resolved: Vec<f32> = vals
                .iter()
                .map(|v| evaluate_calc_value(v, containing_width, font_size))
                .collect();
            resolved.into_iter().reduce(f32::min)
        }
        SizeValue::Max(vals) => {
            let resolved: Vec<f32> = vals
                .iter()
                .map(|v| evaluate_calc_value(v, containing_width, font_size))
                .collect();
            resolved.into_iter().reduce(f32::max)
        }
        SizeValue::Clamp { min, val, max } => {
            let min_px = evaluate_calc_value(min, containing_width, font_size);
            let val_px = evaluate_calc_value(val, containing_width, font_size);
            let max_px = evaluate_calc_value(max, containing_width, font_size);
            Some(val_px.clamp(min_px, max_px))
        }
        _ => None,
    }
}

/// Helper function to extract text content from a node recursively
fn extract_text_content(doc: &Document, node_id: NodeId) -> Option<String> {
    let node = doc.node(node_id);
    match &node.data {
        NodeData::Text(t) => Some(t.content.clone()),
        NodeData::Element(_) => {
            let mut result = String::new();
            for &child_id in &node.children {
                if let Some(text) = extract_text_content(doc, child_id) {
                    result.push_str(&text);
                }
            }
            if result.is_empty() {
                None
            } else {
                Some(result)
            }
        }
        _ => None,
    }
}

/// A positioned box in the layout tree.
#[derive(Debug, Clone)]
pub struct LayoutBox {
    pub node_id: NodeId,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub content_width: f32,
    pub content_height: f32,
    pub children: Vec<LayoutBox>,
    pub box_type: BoxType,
    /// For text nodes, the text content
    pub text: Option<String>,
    pub image_src: Option<String>,
    pub link_href: Option<String>,
    /// Float indent: (indent_px, num_indented_lines, is_left_float)
    /// Paint uses this to offset the first N lines of text.
    pub float_text_indent: Option<(f32, u32, bool)>,
    /// For input elements, the input type
    pub input_type: Option<InputType>,
    /// For textarea elements, the rows/cols info
    pub textarea_info: Option<TextAreaInfo>,
    /// Marker styles for list item markers (::marker pseudo-element)
    pub marker_color: Option<incognidium_style::CssColor>,
    pub marker_font_size: Option<f32>,
    pub marker_font_weight: Option<incognidium_style::FontWeight>,
    pub marker_font_family: Option<incognidium_style::FontFamily>,
    pub marker_background_color: Option<incognidium_style::CssColor>,
    pub marker_letter_spacing: Option<f32>,
    pub marker_word_spacing: Option<f32>,
    /// Whether this box is a list item marker
    pub is_list_marker: bool,
    /// List style position (inside/outside) for this marker
    pub list_style_position: incognidium_style::ListStylePosition,
    /// ::first-letter styles (for drop caps and initial letter styling)
    pub first_letter_len: Option<usize>, // Number of chars to treat as first letter
    pub first_letter_color: Option<incognidium_style::CssColor>,
    pub first_letter_font_size: Option<f32>,
    pub first_letter_font_weight: Option<incognidium_style::FontWeight>,
    pub first_letter_font_family: Option<incognidium_style::FontFamily>,
    pub first_letter_background_color: Option<incognidium_style::CssColor>,
    pub first_letter_text_decoration: Option<incognidium_style::TextDecoration>,
    pub first_letter_margin: Option<(f32, f32, f32, f32)>, // top, right, bottom, left
    pub first_letter_padding: Option<(f32, f32, f32, f32)>,
    pub first_letter_border_width: Option<f32>,
    pub first_letter_border_color: Option<incognidium_style::CssColor>,
    /// ::first-line styles (for styling the first line of text)
    pub first_line_has_content: bool, // Whether this text box is on the first line
    pub first_line_color: Option<incognidium_style::CssColor>,
    pub first_line_font_size: Option<f32>,
    pub first_line_font_weight: Option<incognidium_style::FontWeight>,
    pub first_line_font_family: Option<incognidium_style::FontFamily>,
    pub first_line_background_color: Option<incognidium_style::CssColor>,
    pub first_line_text_decoration: Option<incognidium_style::TextDecoration>,
    pub first_line_letter_spacing: Option<f32>,
    pub first_line_word_spacing: Option<f32>,
    pub first_line_text_transform: Option<incognidium_style::TextTransform>,
    /// For table cells: whether this cell is in a border-collapse table
    /// When true, borders are shared with adjacent cells
    pub collapsed_borders: Option<CollapsedBorders>,
    /// For table cells: if true, hide borders/background (empty-cells: hide)
    pub hide_empty_cell: bool,
    /// For multi-column layout: number of columns
    pub column_count: usize,
    /// For multi-column layout: width of each column
    pub column_width: f32,
    /// For multi-column layout: gap between columns
    pub column_gap: f32,
    /// For multi-column layout: rule (line) between columns
    pub column_rule_width: f32,
    pub column_rule_style: incognidium_style::ColumnRuleStyle,
    pub column_rule_color: incognidium_style::CssColor,
}

/// Border information for a cell in a collapsed-border table
#[derive(Debug, Clone, Copy)]
pub struct CollapsedBorders {
    /// The effective border widths after conflict resolution
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
    /// Whether this cell is at the table edge
    pub is_first_row: bool,
    pub is_last_row: bool,
    pub is_first_column: bool,
    pub is_last_column: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BoxType {
    Block,
    InlineBlock,
    Inline,
    Flex,
    Grid,
    Columns,      // For multi-column layout
    Table,
    TableRow,
    TableCell,
    TableSection, // For thead, tbody, tfoot
    TableCaption, // For <caption> elements
    Text,
    Image,
    LineBreak,    // For <br> elements
    Contents,
    None,
}

/// Build the layout tree and compute positions.
pub fn layout(
    doc: &Document,
    styles: &StyleMap,
    viewport_width: f32,
    viewport_height: f32,
) -> LayoutBox {
    let empty = ImageSizes::new();
    layout_with_images(doc, styles, viewport_width, viewport_height, &empty)
}

/// Build the layout tree with image size information.
pub fn layout_with_images(
    doc: &Document,
    styles: &StyleMap,
    viewport_width: f32,
    viewport_height: f32,
    image_sizes: &ImageSizes,
) -> LayoutBox {
    let root_id = doc.root();
    let mut counters = CounterState::default();
    let mut root_box = build_layout_tree(doc, styles, root_id, &mut counters);
    root_box.width = viewport_width;
    compute_layout(
        &mut root_box,
        styles,
        viewport_width,
        viewport_height,
        image_sizes,
    );
    root_box
}

fn build_layout_tree(
    doc: &Document,
    styles: &StyleMap,
    node_id: NodeId,
    counters: &mut CounterState,
) -> LayoutBox {
    let node = doc.node(node_id);
    let style = styles.get(&node_id);

    // Process counter-reset and counter-increment
    if let Some(s) = style {
        // Apply counter-reset first (Sets counters to initial values)
        for (name, value) in &s.counter_reset {
            counters.set(name, *value);
        }
        // Apply counter-increment
        for (name, delta) in &s.counter_increment {
            let new_val = counters.get(name) + delta;
            counters.increment(name, *delta);
        }
    }

    let display = style.map(|s| s.display).unwrap_or(Display::Block);

    // Skip display:none elements only; fixed-position elements are laid out
    // as normal blocks so their content still appears in the page flow.
    if display == Display::None {
        return LayoutBox {
            node_id,
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            content_width: 0.0,
            content_height: 0.0,
            children: Vec::new(),
            box_type: BoxType::None,
            text: None,
            image_src: None,
            link_href: None,
            float_text_indent: None,
            input_type: None,
            textarea_info: None,
            marker_color: None,
            marker_background_color: None,
            marker_letter_spacing: None,
            marker_word_spacing: None,
            marker_font_size: None,
            marker_font_weight: None,
            marker_font_family: None,
            is_list_marker: false,
            list_style_position: incognidium_style::ListStylePosition::Outside,
            first_letter_len: None,
            first_letter_color: None,
            first_letter_font_size: None,
            first_letter_font_weight: None,
            first_letter_font_family: None,
            first_letter_background_color: None,
            first_letter_text_decoration: None,
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
            column_rule_style: incognidium_style::ColumnRuleStyle::None,
            column_rule_color: incognidium_style::CssColor::TRANSPARENT,
        };
    }

    let (box_type, text, image_src, input_type, textarea_info) = match &node.data {
        NodeData::Text(t) => {
            // Preserve text content as-is; whitespace handling is done during layout
            // based on the CSS white-space property
            if t.content.is_empty() {
                (BoxType::None, None, None, None, None)
            } else {
                (BoxType::Text, Some(t.content.clone()), None, None, None)
            }
        }
        NodeData::Element(el) => {
            if el.tag_name == "br" {
                // Line break element - special box type
                (BoxType::LineBreak, None, None, None, None)
            } else if el.tag_name == "img" {
                let src = el.get_attr("src").map(|s| s.to_string());
                // Extract alt text for accessibility and text extraction
                let alt_text = el.get_attr("alt").map(|s| s.to_string());
                (BoxType::Image, alt_text, src, None, None)
            } else if el.tag_name == "canvas" {
                // Canvas elements render as Image boxes with a special src key
                let canvas_src = format!("__canvas__{}", node_id);
                (BoxType::Image, None, Some(canvas_src), None, None)
            } else if el.tag_name == "input" {
                // Detect input type and handle specially for checkboxes/radios
                let input_type_attr = el.get_attr("type").unwrap_or("text");
                let checked = el.get_attr("checked").is_some();
                let input_type = match input_type_attr {
                    "checkbox" => InputType::Checkbox { checked },
                    "radio" => InputType::Radio { checked },
                    "button" => InputType::Button,
                    "submit" => InputType::Submit,
                    "hidden" => InputType::Hidden,
                    _ => InputType::Text,
                };
                // Show value or placeholder text (for text inputs and buttons)
                let text = if matches!(
                    input_type,
                    InputType::Text | InputType::Button | InputType::Submit
                ) {
                    el.get_attr("value")
                        .or_else(|| el.get_attr("placeholder"))
                        .map(|s| s.to_string())
                } else {
                    None
                };
                (BoxType::InlineBlock, text, None, Some(input_type), None)
            } else if el.tag_name == "textarea" {
                // Textarea element - get rows/cols for sizing
                let rows = el
                    .get_attr("rows")
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(2);
                let cols = el
                    .get_attr("cols")
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(20);
                let textarea_info = TextAreaInfo { rows, cols };
                // Get the text content from children (the initial value)
                let mut text_content = String::new();
                for &child_id in &node.children {
                    if let Some(child_text) = extract_text_content(doc, child_id) {
                        text_content.push_str(&child_text);
                    }
                }
                let text = if text_content.is_empty() {
                    el.get_attr("placeholder").map(|s| s.to_string())
                } else {
                    Some(text_content)
                };
                (BoxType::InlineBlock, text, None, None, Some(textarea_info))
            } else {
                // Check for multi-column layout
                let has_columns = style.map(|s| {
                    s.column_count.is_some() || s.column_width.is_some()
                }).unwrap_or(false);

                if has_columns {
                    (BoxType::Columns, None, None, None, None)
                } else {
                    match display {
                        Display::Block => (BoxType::Block, None, None, None, None),
                        Display::InlineBlock => (BoxType::InlineBlock, None, None, None, None),
                        Display::Inline => (BoxType::Inline, None, None, None, None),
                        Display::Flex => (BoxType::Flex, None, None, None, None),
                        Display::Grid => (BoxType::Grid, None, None, None, None),
                        Display::Table => (BoxType::Table, None, None, None, None),
                        Display::TableRow => (BoxType::TableRow, None, None, None, None),
                        Display::TableCell => (BoxType::TableCell, None, None, None, None),
                        Display::TableHeaderGroup
                        | Display::TableRowGroup
                        | Display::TableFooterGroup => (BoxType::TableSection, None, None, None, None),
                        // Table columns and captions don't create boxes
                        Display::TableCaption => (BoxType::TableCaption, None, None, None, None),
                        // Table columns don't create boxes
                        Display::TableColumn | Display::TableColumnGroup => {
                            (BoxType::None, None, None, None, None)
                        }
                        Display::Contents => (BoxType::Contents, None, None, None, None),
                        Display::None => (BoxType::None, None, None, None, None),
                    }
                }
            }
        }
        _ => (BoxType::Block, None, None, None, None),
    };
    // Collect link_href from ancestor <a> elements
    let link_href = if let NodeData::Element(el) = &node.data {
        if el.tag_name == "a" {
            el.get_attr("href").map(|s| s.to_string())
        } else {
            None
        }
    } else {
        None
    };

    let mut children: Vec<LayoutBox> = Vec::new();
    // Textarea content is extracted as text, don't process children separately
    let is_textarea_element = matches!(
        &node.data,
        NodeData::Element(el) if el.tag_name == "textarea"
    );
    if !is_textarea_element {
        for &child_id in &node.children {
            let child_box = build_layout_tree(doc, styles, child_id, counters);
            if child_box.box_type == BoxType::None {
                continue;
            }
            if child_box.box_type == BoxType::Contents {
                // Flatten display:contents — splice its children directly into parent
                fn flatten_contents(into: &mut Vec<LayoutBox>, boxes: &[LayoutBox]) {
                    for c in boxes {
                        if c.box_type == BoxType::None {
                            continue;
                        }
                        if c.box_type == BoxType::Contents {
                            flatten_contents(into, &c.children);
                        } else {
                            into.push(c.clone());
                        }
                    }
                }
                flatten_contents(&mut children, &child_box.children);
            } else {
                children.push(child_box);
            }
        }
    }

    // Add list bullet/number markers for <li> elements (respect list-style-type)
    // Also handle list-style-image for custom image markers
    if let NodeData::Element(ref el) = node.data {
        let has_list_style_image = style
            .and_then(|s| s.list_style_image.as_ref())
            .is_some();

        if el.tag_name == "li"
            && (has_list_style_image
                || styles.get(&node_id).map(|s| s.list_style_type)
                    != Some(incognidium_style::ListStyleType::None))
        {
            // Get the list-style-position for this list item
            let list_style_position = style
                .map(|s| s.list_style_position)
                .unwrap_or(ListStylePosition::Outside);

            // Check if list-style-image is set - use image marker if so
            if let Some(image_url) = style.and_then(|s| s.list_style_image.clone()) {
                // Create an image marker box
                children.insert(
                    0,
                    LayoutBox {
                        node_id,
                        x: 0.0,
                        y: 0.0,
                        width: 16.0, // Default marker image size
                        height: 16.0,
                        content_width: 16.0,
                        content_height: 16.0,
                        children: Vec::new(),
                        box_type: BoxType::Image,
                        text: None,
                        image_src: Some(image_url),
                        link_href: None,
                        float_text_indent: None,
                        input_type: None,
                        textarea_info: None,
                        // No marker styles for image markers
                        marker_color: None,
                        marker_background_color: None,
                        marker_letter_spacing: None,
                        marker_word_spacing: None,
                        marker_font_size: None,
                        marker_font_weight: None,
                        marker_font_family: None,
                        // This is a list marker
                        is_list_marker: true,
                        list_style_position,
                        // ::first-letter styles (not applicable for markers)
                        first_letter_len: None,
                        first_letter_color: None,
                        first_letter_font_size: None,
                        first_letter_font_weight: None,
                        first_letter_font_family: None,
                        first_letter_background_color: None,
                        first_letter_text_decoration: None,
                        first_letter_margin: None,
                        first_letter_padding: None,
                        first_letter_border_width: None,
                        first_letter_border_color: None,
                        // ::first-line styles (not applicable for markers)
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
                        column_rule_style: incognidium_style::ColumnRuleStyle::None,
                        column_rule_color: incognidium_style::CssColor::TRANSPARENT,
                    },
                );
            } else {
                // Text-based marker (existing implementation)
                let marker_type = styles
                    .get(&node_id)
                    .map(|s| s.list_style_type)
                    .unwrap_or(incognidium_style::ListStyleType::Disc);
                let marker = if let Some(parent_id) = node.parent {
                    let parent_node = doc.node(parent_id);
                    let _is_ordered = matches!(marker_type, incognidium_style::ListStyleType::Decimal)
                        || matches!(marker_type, incognidium_style::ListStyleType::DecimalLeadingZero)
                        || matches!(marker_type, incognidium_style::ListStyleType::LowerAlpha)
                        || matches!(marker_type, incognidium_style::ListStyleType::UpperAlpha)
                        || matches!(marker_type, incognidium_style::ListStyleType::LowerRoman)
                        || matches!(marker_type, incognidium_style::ListStyleType::UpperRoman)
                        || matches!(marker_type, incognidium_style::ListStyleType::LowerGreek)
                        || matches!(marker_type, incognidium_style::ListStyleType::UpperGreek)
                        || matches!(marker_type, incognidium_style::ListStyleType::Armenian)
                        || matches!(marker_type, incognidium_style::ListStyleType::Georgian)
                        || matches!(marker_type, incognidium_style::ListStyleType::Hebrew)
                        || matches!(marker_type, incognidium_style::ListStyleType::Hiragana)
                        || matches!(marker_type, incognidium_style::ListStyleType::Katakana)
                        || matches!(marker_type, incognidium_style::ListStyleType::HiraganaIroha)
                        || matches!(marker_type, incognidium_style::ListStyleType::KatakanaIroha)
                        || matches!(marker_type, incognidium_style::ListStyleType::LowerLatin)
                        || matches!(marker_type, incognidium_style::ListStyleType::UpperLatin)
                        || matches!(&parent_node.data, NodeData::Element(ref pel) if pel.tag_name == "ol");
                    let idx = parent_node.children.iter()
                        .filter(|&&cid| {
                            matches!(&doc.node(cid).data, NodeData::Element(ref e) if e.tag_name == "li")
                        })
                        .position(|&cid| cid == node_id)
                        .unwrap_or(0);
                    let num = idx + 1;
                    match marker_type {
                        incognidium_style::ListStyleType::Decimal => format!("{}. ", num),
                        incognidium_style::ListStyleType::DecimalLeadingZero => {
                            format!("{:02}. ", num)
                        }
                        incognidium_style::ListStyleType::LowerAlpha => {
                            format!("{}. ", number_to_alpha(num, false))
                        }
                        incognidium_style::ListStyleType::UpperAlpha => {
                            format!("{}. ", number_to_alpha(num, true))
                        }
                        incognidium_style::ListStyleType::LowerRoman => {
                            format!("{}. ", number_to_roman(num))
                        }
                        incognidium_style::ListStyleType::UpperRoman => {
                            format!("{}. ", number_to_roman(num).to_uppercase())
                        }
                        incognidium_style::ListStyleType::LowerGreek => {
                            format!("{}. ", number_to_greek(num, false))
                        }
                        incognidium_style::ListStyleType::UpperGreek => {
                            format!("{}. ", number_to_greek(num, true))
                        }
                        incognidium_style::ListStyleType::Armenian => {
                            format!("{}. ", number_to_armenian(num))
                        }
                        incognidium_style::ListStyleType::Georgian => {
                            format!("{}. ", number_to_georgian(num))
                        }
                        incognidium_style::ListStyleType::Hebrew => {
                            format!("{} ", number_to_hebrew(num))
                        }
                        incognidium_style::ListStyleType::Hiragana => {
                            format!("{} ", number_to_hiragana(num))
                        }
                        incognidium_style::ListStyleType::Katakana => {
                            format!("{} ", number_to_katakana(num))
                        }
                        incognidium_style::ListStyleType::HiraganaIroha => {
                            format!("{} ", number_to_hiragana_iroha(num))
                        }
                        incognidium_style::ListStyleType::KatakanaIroha => {
                            format!("{} ", number_to_katakana_iroha(num))
                        }
                        incognidium_style::ListStyleType::LowerLatin => {
                            format!("{} ", number_to_alpha(num, false))
                        }
                        incognidium_style::ListStyleType::UpperLatin => {
                            format!("{} ", number_to_alpha(num, true))
                        }
                        incognidium_style::ListStyleType::Circle => "\u{25e6} ".to_string(), // ◦
                        incognidium_style::ListStyleType::Square => "\u{25a0} ".to_string(), // ■
                        _ => "\u{2022} ".to_string(),                                        // • (disc)
                    }
                } else {
                    "\u{2022} ".to_string()
                };
                children.insert(
                    0,
                    LayoutBox {
                        node_id,
                        x: 0.0,
                        y: 0.0,
                        width: 0.0,
                        height: 0.0,
                        content_width: 0.0,
                        content_height: 0.0,
                        children: Vec::new(),
                        box_type: BoxType::Text,
                        text: Some(marker),
                        image_src: None,
                        link_href: None,
                        float_text_indent: None,
                        input_type: None,
                        textarea_info: None,
                        // Apply ::marker pseudo-element styles from parent li element
                        marker_color: style.and_then(|s| s.marker_color),
                        marker_font_size: style.and_then(|s| s.marker_font_size),
                        marker_font_weight: style.and_then(|s| s.marker_font_weight),
                        marker_font_family: style.and_then(|s| s.marker_font_family.clone()),
                        marker_background_color: style.and_then(|s| s.marker_background_color),
                        marker_letter_spacing: style.and_then(|s| s.marker_letter_spacing),
                        marker_word_spacing: style.and_then(|s| s.marker_word_spacing),
                        // This is a list marker
                        is_list_marker: true,
                        list_style_position,
                        // ::first-letter styles (not applicable for markers)
                        first_letter_len: None,
                        first_letter_color: None,
                        first_letter_font_size: None,
                        first_letter_font_weight: None,
                        first_letter_font_family: None,
                        first_letter_background_color: None,
                        first_letter_text_decoration: None,
                        first_letter_margin: None,
                        first_letter_padding: None,
                        first_letter_border_width: None,
                        first_letter_border_color: None,
                        // ::first-line styles (not applicable for markers)
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
                        column_rule_style: incognidium_style::ColumnRuleStyle::None,
                        column_rule_color: incognidium_style::CssColor::TRANSPARENT,
                    },
                );
            }
        }
    }

    // Add ::before pseudo-element content if present
    if let Some(s) = style {
        // Apply counter-increment for ::before BEFORE resolving content
        for (name, delta) in &s.before_counter_increment {
            counters.increment(name, *delta);
        }
        if let Some(text) = resolve_content_to_text(&s.before_content, counters, &s.quotes, 0) {
            children.insert(
                0,
                LayoutBox {
                    node_id,
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                    content_width: 0.0,
                    content_height: 0.0,
                    children: Vec::new(),
                    box_type: BoxType::Text,
                    text: Some(text),
                    image_src: None,
                    link_href: None,
                    float_text_indent: None,
                    input_type: None,
                    textarea_info: None,
                    marker_color: None,
                    marker_background_color: None,
                    marker_letter_spacing: None,
                    marker_word_spacing: None,
                    marker_font_size: None,
                    marker_font_weight: None,
                    marker_font_family: None,
                    is_list_marker: false,
                    list_style_position: ListStylePosition::Outside,
                    // ::first-letter styles (not applicable for ::before)
                    first_letter_len: None,
                    first_letter_color: None,
                    first_letter_font_size: None,
                    first_letter_font_weight: None,
                    first_letter_font_family: None,
                    first_letter_background_color: None,
                    first_letter_text_decoration: None,
                    first_letter_margin: None,
                    first_letter_padding: None,
                    first_letter_border_width: None,
                    first_letter_border_color: None,
                    // ::first-line styles (not applicable for ::before)
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
                    column_rule_style: incognidium_style::ColumnRuleStyle::None,
                    column_rule_color: incognidium_style::CssColor::TRANSPARENT,
                },
            );
        }
    }

    // Add ::after pseudo-element content if present
    if let Some(s) = style {
        // Apply counter-increment for ::after BEFORE resolving content
        for (name, delta) in &s.after_counter_increment {
            counters.increment(name, *delta);
        }
        if let Some(text) = resolve_content_to_text(&s.after_content, counters, &s.quotes, 0) {
            children.push(
                LayoutBox {
                    node_id,
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                    content_width: 0.0,
                    content_height: 0.0,
                    children: Vec::new(),
                    box_type: BoxType::Text,
                    text: Some(text),
                    image_src: None,
                    link_href: None,
                    float_text_indent: None,
                    input_type: None,
                    textarea_info: None,
                    marker_color: None,
                    marker_background_color: None,
                    marker_letter_spacing: None,
                    marker_word_spacing: None,
                    marker_font_size: None,
                    marker_font_weight: None,
                    marker_font_family: None,
                    is_list_marker: false,
                    list_style_position: ListStylePosition::Outside,
                    // ::first-letter styles (not applicable for ::after)
                    first_letter_len: None,
                    first_letter_color: None,
                    first_letter_font_size: None,
                    first_letter_font_weight: None,
                    first_letter_font_family: None,
                    first_letter_background_color: None,
                    first_letter_text_decoration: None,
                    first_letter_margin: None,
                    first_letter_padding: None,
                    first_letter_border_width: None,
                    first_letter_border_color: None,
                    // ::first-line styles (not applicable for ::after)
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
                    column_rule_style: incognidium_style::ColumnRuleStyle::None,
                    column_rule_color: incognidium_style::CssColor::TRANSPARENT,
                },
            );
        }
    }

    // Check if element has visual styling even if empty (background, borders, explicit size)
    let has_visual_style = style
        .map(|s| {
            s.background_color.a > 0
                || s.border_top_width > 0.0
                || s.border_bottom_width > 0.0
                || s.border_left_width > 0.0
                || s.border_right_width > 0.0
                || matches!(s.width, SizeValue::Px(_))
                || matches!(s.height, SizeValue::Px(_))
        })
        .unwrap_or(false);

    // Collapse empty containers: block/flex/inline with no meaningful content
    // This prevents empty wrapper divs from taking up space when all their content is hidden
    let has_meaningful_content = if has_visual_style {
        true
    } else if text
        .as_deref()
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false)
    {
        true
    } else if children.is_empty() && image_src.is_none() {
        false
    } else {
        // Check if children have meaningful visible content
        children.iter().any(|c| {
            match c.box_type {
                BoxType::Text => c
                    .text
                    .as_deref()
                    .map(|t| !t.trim().is_empty())
                    .unwrap_or(false),
                BoxType::None => false,
                BoxType::Image => {
                    // Image is only meaningful if it has a src (actual content)
                    // It'll still be 0-sized if we don't have the image data
                    c.image_src.is_some()
                }
                _ => true,
            }
        }) || image_src.is_some()
    };

    let effective_box_type = if (box_type == BoxType::Block
        || box_type == BoxType::InlineBlock
        || box_type == BoxType::Flex
        || box_type == BoxType::Grid
        || box_type == BoxType::Inline
        || box_type == BoxType::Contents)
        && !has_meaningful_content
    {
        BoxType::None
    } else {
        box_type
    };

    LayoutBox {
        node_id,
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
        content_width: 0.0,
        content_height: 0.0,
        children,
        box_type: effective_box_type,
        text,
        image_src,
        link_href,
        float_text_indent: None,
        input_type,
        textarea_info,
        marker_color: None,
        marker_font_size: None,
        marker_font_weight: None,
        marker_font_family: None,
        marker_background_color: None,
        marker_letter_spacing: None,
        marker_word_spacing: None,
        is_list_marker: false,
        list_style_position: ListStylePosition::Outside,
        // ::first-letter styles (populated from element's computed style)
        first_letter_len: if style.map(|s| s.first_letter_color.is_some()
            || s.first_letter_font_size.is_some()
            || s.first_letter_font_weight.is_some()).unwrap_or(false) {
            Some(1) // Default to 1 character for now
        } else {
            None
        },
        first_letter_color: style.and_then(|s| s.first_letter_color),
        first_letter_font_size: style.and_then(|s| s.first_letter_font_size),
        first_letter_font_weight: style.and_then(|s| s.first_letter_font_weight),
        first_letter_font_family: style.and_then(|s| s.first_letter_font_family.clone()),
        first_letter_background_color: style.and_then(|s| s.first_letter_background_color),
        first_letter_text_decoration: style.and_then(|s| s.first_letter_text_decoration),
        first_letter_margin: style.and_then(|s| s.first_letter_margin),
        first_letter_padding: style.and_then(|s| s.first_letter_padding),
        first_letter_border_width: style.and_then(|s| s.first_letter_border_width),
        first_letter_border_color: style.and_then(|s| s.first_letter_border_color),
        // ::first-line styles (populated from element's computed style)
        first_line_has_content: false, // Will be set during layout when we determine if this is first line
        first_line_color: style.and_then(|s| s.first_line_color),
        first_line_font_size: style.and_then(|s| s.first_line_font_size),
        first_line_font_weight: style.and_then(|s| s.first_line_font_weight),
        first_line_font_family: style.and_then(|s| s.first_line_font_family.clone()),
        first_line_background_color: style.and_then(|s| s.first_line_background_color),
        first_line_text_decoration: style.and_then(|s| s.first_line_text_decoration),
        first_line_letter_spacing: style.and_then(|s| s.first_line_letter_spacing),
        first_line_word_spacing: style.and_then(|s| s.first_line_word_spacing),
        first_line_text_transform: style.and_then(|s| s.first_line_text_transform),
        collapsed_borders: None,
        hide_empty_cell: false,
        column_count: 0,
        column_width: 0.0,
        column_gap: 0.0,
        column_rule_width: 0.0,
        column_rule_style: incognidium_style::ColumnRuleStyle::None,
        column_rule_color: incognidium_style::CssColor::TRANSPARENT,
    }
}

fn compute_layout(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    _containing_height: f32,
    image_sizes: &ImageSizes,
) {
    compute_layout_with_floats(
        layout_box,
        styles,
        containing_width,
        _containing_height,
        image_sizes,
        FloatState::default(),
    );
}

/// Layout an absolutely or fixed positioned element.
/// These elements are removed from normal flow and positioned relative to their containing block.
fn layout_absolute(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    containing_height: f32,
    image_sizes: &ImageSizes,
) {
    let cs = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    // Compute layout with container width
    let (abs_width, is_auto_width) = match cs.width {
        SizeValue::Px(w) => (w, false),
        SizeValue::Percent(p) => (containing_width * p / 100.0, false),
        _ => {
            // auto width: use container width as available space
            // We'll shrink-to-fit after layout
            (containing_width, true)
        }
    };

    // Layout as a block with the available width
    layout_block(
        layout_box,
        styles,
        abs_width,
        containing_height,
        image_sizes,
        FloatState::default(),
    );

    // For auto width, shrink-to-fit the content
    if is_auto_width {
        let intrinsic_width = calculate_intrinsic_width(layout_box);
        if intrinsic_width > 0.0 && intrinsic_width < layout_box.width {
            layout_box.width = intrinsic_width;
            layout_box.content_width = intrinsic_width - cs.padding_left - cs.padding_right
                - cs.border_left_width - cs.border_right_width;
        }
    }

    // Apply top/left/right/bottom positioning
    let content_w = containing_width - cs.padding_left - cs.padding_right
        - cs.border_left_width - cs.border_right_width;
    layout_box.x = match cs.left {
        SizeValue::Px(v) => v + cs.margin_left,
        SizeValue::Percent(p) => content_w * p / 100.0 + cs.margin_left,
        _ => match cs.right {
            SizeValue::Px(v) => (content_w - layout_box.width - v - cs.margin_right).max(0.0),
            SizeValue::Percent(p) => {
                (content_w - layout_box.width - content_w * p / 100.0).max(0.0)
            }
            _ => cs.margin_left,
        },
    };
    layout_box.y = match cs.top {
        SizeValue::Px(v) => v + cs.margin_top,
        SizeValue::Percent(p) => containing_height * p / 100.0 + cs.margin_top,
        _ => match cs.bottom {
            SizeValue::Px(v) => (containing_height - layout_box.height - v - cs.margin_bottom).max(0.0),
            SizeValue::Percent(p) => {
                (containing_height - layout_box.height - containing_height * p / 100.0).max(0.0)
            }
            _ => cs.margin_top,
        },
    };
}

fn compute_layout_with_floats(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    _containing_height: f32,
    image_sizes: &ImageSizes,
    parent_floats: FloatState,
) {
    // Check if this element is absolutely positioned
    // Absolutely positioned elements need special handling regardless of their box_type
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    // Handle content-visibility: hidden - skip rendering children but keep layout
    // This is like display: none for content, but the element still takes up space
    if style.content_visibility == ContentVisibility::Hidden {
        // Clear children so they don't get laid out or rendered
        layout_box.children.clear();
        // Set box dimensions based on style, but with no content
        layout_block(layout_box, styles, containing_width, _containing_height, image_sizes, parent_floats);
        return;
    }

    if style.position == Position::Absolute || style.position == Position::Fixed {
        layout_absolute(layout_box, styles, containing_width, _containing_height, image_sizes);
        return;
    }

    match layout_box.box_type {
        BoxType::Block => {
            layout_block(
                layout_box,
                styles,
                containing_width,
                _containing_height,
                image_sizes,
                parent_floats,
            );
        }
        BoxType::InlineBlock => {
            layout_inline_block(layout_box, styles, containing_width, image_sizes);
        }
        BoxType::Inline => {
            layout_inline(layout_box, styles, containing_width, image_sizes);
        }
        BoxType::Flex => {
            layout_flex(layout_box, styles, containing_width, image_sizes);
        }
        BoxType::Grid => {
            layout_grid(layout_box, styles, containing_width, image_sizes);
        }
        BoxType::Columns => {
            layout_columns(layout_box, styles, containing_width, image_sizes, parent_floats);
        }
        BoxType::Table => {
            layout_table(
                layout_box,
                styles,
                containing_width,
                image_sizes,
                parent_floats,
            );
        }
        BoxType::TableRow => {
            layout_table_row(layout_box, styles, containing_width, image_sizes);
        }
        BoxType::TableCell => {
            layout_table_cell(
                layout_box,
                styles,
                containing_width,
                image_sizes,
                parent_floats,
            );
        }
        BoxType::TableSection => {
            layout_table_section(
                layout_box,
                styles,
                containing_width,
                image_sizes,
                parent_floats,
            );
        }
        BoxType::TableCaption => {
            // Table captions are laid out as block-level elements
            layout_block(layout_box, styles, containing_width, 0.0, image_sizes, parent_floats);
        }
        BoxType::Text => {
            layout_text(layout_box, styles, containing_width);
        }
        BoxType::Image => {
            layout_image(layout_box, styles, containing_width, image_sizes);
        }
        BoxType::LineBreak => {
            // Line break elements have 0 size but participate in inline layout
            layout_box.width = 0.0;
            layout_box.height = 0.0;
            layout_box.content_width = 0.0;
            layout_box.content_height = 0.0;
        }
        BoxType::Contents => {}
        BoxType::None => {}
    }
}

fn layout_block(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    containing_height: f32,
    image_sizes: &ImageSizes,
    parent_floats: FloatState,
) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    // Calculate width
    let margin_left = style.margin_left;
    let margin_right = style.margin_right;
    let padding_left = style.padding_left;
    let padding_right = style.padding_right;
    let border_left = style.border_left_width;
    let border_right = style.border_right_width;

    let is_border_box = style.box_sizing == incognidium_style::BoxSizing::BorderBox;
    let mut content_width = match style.width {
        SizeValue::Px(w) => {
            if is_border_box {
                (w - padding_left - padding_right - border_left - border_right).max(0.0)
            } else {
                w
            }
        }
        SizeValue::Percent(p) => {
            let total = containing_width * p / 100.0;
            if is_border_box {
                (total - padding_left - padding_right - border_left - border_right).max(0.0)
            } else {
                total
            }
        }
        SizeValue::Auto | SizeValue::None => (containing_width
            - margin_left
            - margin_right
            - padding_left
            - padding_right
            - border_left
            - border_right)
            .max(0.0),
        // CSS Math Functions - evaluate with containing block context
        SizeValue::Calc(ref expr) => {
            evaluate_size_value(&SizeValue::Calc(expr.clone()), containing_width, style.font_size)
                .unwrap_or(containing_width)
        }
        SizeValue::Min(ref vals) => {
            evaluate_size_value(&SizeValue::Min(vals.clone()), containing_width, style.font_size)
                .unwrap_or(containing_width)
        }
        SizeValue::Max(ref vals) => {
            evaluate_size_value(&SizeValue::Max(vals.clone()), containing_width, style.font_size)
                .unwrap_or(containing_width)
        }
        SizeValue::Clamp {
            ref min,
            ref val,
            ref max,
        } => {
            evaluate_size_value(
                &SizeValue::Clamp {
                    min: min.clone(),
                    val: val.clone(),
                    max: max.clone(),
                },
                containing_width,
                style.font_size,
            )
            .unwrap_or(containing_width)
        }
        // CSS Intrinsic Sizing - treat as auto for now (content-based sizing requires multi-pass)
        SizeValue::MinContent | SizeValue::MaxContent | SizeValue::FitContent => {
            // For now, use available width; proper implementation would measure content
            (containing_width
                - margin_left
                - margin_right
                - padding_left
                - padding_right
                - border_left
                - border_right)
                .max(0.0)
        }
    };

    // Track if min/max-width constraints were applied (affects border-box calculation)
    let mut constrained_by_min = false;
    let mut constrained_by_max = false;

    // Apply max-width constraint
    match style.max_width {
        SizeValue::Px(mw) if content_width > mw => {
            content_width = mw;
            constrained_by_max = true;
        }
        SizeValue::Percent(p) => {
            let mw = containing_width * p / 100.0;
            if content_width > mw {
                content_width = mw;
                constrained_by_max = true;
            }
        }
        // CSS Math Functions in max-width
        SizeValue::Calc(_)
        | SizeValue::Min(_)
        | SizeValue::Max(_)
        | SizeValue::Clamp { .. } => {
            if let Some(mw) =
                evaluate_size_value(&style.max_width, containing_width, style.font_size)
            {
                if content_width > mw {
                    content_width = mw;
                    constrained_by_max = true;
                }
            }
        }
        _ => {}
    }

    // Apply min-width constraint
    match style.min_width {
        SizeValue::Px(mw) if content_width < mw => {
            content_width = mw;
            constrained_by_min = true;
        }
        SizeValue::Percent(p) => {
            let mw = containing_width * p / 100.0;
            if content_width < mw {
                content_width = mw;
                constrained_by_min = true;
            }
        }
        // CSS Math Functions in min-width
        SizeValue::Calc(_)
        | SizeValue::Min(_)
        | SizeValue::Max(_)
        | SizeValue::Clamp { .. } => {
            if let Some(mw) =
                evaluate_size_value(&style.min_width, containing_width, style.font_size)
            {
                if content_width < mw {
                    content_width = mw;
                    constrained_by_min = true;
                }
            }
        }
        _ => {}
    }

    layout_box.content_width = content_width.max(0.0);
    // For border-box, the total width should be exactly the specified width (if given),
    // not content_width + padding + border (which would be incorrect if min/max-width was applied)
    // However, if min/max-width constrained the content, we must use the constrained value
    layout_box.width = if is_border_box && matches!(style.width, SizeValue::Px(_) | SizeValue::Percent(_)) {
        if constrained_by_min || constrained_by_max {
            // When constrained by min/max-width, use content_width + padding + border
            content_width + padding_left + padding_right + border_left + border_right
        } else {
            match style.width {
                SizeValue::Px(w) => w,
                SizeValue::Percent(p) => containing_width * p / 100.0,
                _ => content_width + padding_left + padding_right + border_left + border_right,
            }
        }
    } else {
        content_width + padding_left + padding_right + border_left + border_right
    };

    // Calculate explicit height early so it can be passed to children
    // This allows percentage heights on children to work when parent has explicit height
    let explicit_height = match style.height {
        SizeValue::Px(h) => Some(h),
        _ => None,
    };

    // Layout children
    let child_containing_width = layout_box.content_width;
    let child_containing_height = explicit_height.unwrap_or(0.0);

    let mut cursor_y: f32 = style.padding_top + style.border_top_width;
    let content_x = padding_left + border_left;
    // Track previous child's margin-bottom for margin collapse
    let mut prev_margin_bottom: f32 = 0.0;

    let mut float_right_width: f32 = parent_floats.right_width;
    let mut float_left_width: f32 = parent_floats.left_width;
    let mut float_bottom: f32 = if parent_floats.remaining_height > 0.0 {
        style.padding_top + style.border_top_width + parent_floats.remaining_height
    } else {
        0.0
    };

    // Collect indices of absolutely positioned children
    // All absolute/fixed positioned elements are removed from normal flow
    let abs_indices: Vec<usize> = layout_box
        .children
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            let cs = styles.get(&c.node_id).cloned().unwrap_or_default();
            cs.position == Position::Absolute
                || cs.position == Position::Fixed
        })
        .map(|(i, _)| i)
        .collect();

    // Separate inline and block children
    let mut i = 0;
    let mut first_inline_run = true;
    while i < layout_box.children.len() {
        // Skip absolutely positioned children from normal flow
        if abs_indices.contains(&i) {
            i += 1;
            continue;
        }

        let child = &layout_box.children[i];

        if is_inline_level_styled(child.box_type, styles, child.node_id) {
            // Inline/text/image: lay out horizontally on a line
            // Reduce available width if floats are active
            if cursor_y >= float_bottom {
                float_right_width = 0.0;
                float_left_width = 0.0;
            }
            let mut inline_available =
                (child_containing_width - float_right_width - float_left_width).max(0.0);
            let mut inline_x_start = content_x + float_left_width;

            let line_start = i;
            // CSS line-height from parent style - minimum height for each line
            let css_line_height = style.font_size * style.line_height;
            // Apply line-height-step if specified (rounds up to nearest multiple)
            let css_line_height = if let Some(step) = style.line_height_step {
                if step > 0.0 {
                    // Round up to nearest multiple of step
                    (css_line_height / step).ceil() * step
                } else {
                    css_line_height
                }
            } else {
                css_line_height
            };
            let mut line_height: f32 = css_line_height;

            while i < layout_box.children.len() {
                let c = &layout_box.children[i];
                if !is_inline_level_styled(c.box_type, styles, c.node_id) {
                    break;
                }
                compute_layout(
                    &mut layout_box.children[i],
                    styles,
                    inline_available,
                    child_containing_height,
                    image_sizes,
                );
                i += 1;
            }

            // First pass: identify and mark line break elements (br tags)
            for j in line_start..i {
                if layout_box.children[j].box_type == BoxType::LineBreak {
                    layout_box.children[j].width = 0.0;
                    layout_box.children[j].height = 0.0;
                }
            }

            // Skip inline runs that consist only of whitespace text nodes
            // (whitespace between block elements should not take up space)
            let all_whitespace =
                (line_start..i).all(|j| layout_box.children[j].text.as_deref() == Some(" "));
            if all_whitespace {
                continue;
            }

            // Compute inter-element gaps to prevent text concatenation
            let gaps = compute_inline_gaps(&layout_box.children, line_start, i, styles);

            // Position inline children on a line with word-wrap
            let mut line_x = if first_inline_run {
                first_inline_run = false;
                inline_x_start + style.text_indent
            } else {
                inline_x_start
            };

            let mut line_begin = line_start;
            for j in line_start..i {
                let gap = gaps[j - line_start];
                line_x += gap;

                // Get child style for margins
                let child_style = styles
                    .get(&layout_box.children[j].node_id)
                    .cloned()
                    .unwrap_or_default();
                let margin_left = child_style.margin_left;
                let margin_right = child_style.margin_right;

                let child_width = layout_box.children[j].width;
                let child_height = layout_box.children[j].height;

                // Check if this is a line break element (br tag)
                let is_line_break = layout_box.children[j].box_type == BoxType::LineBreak;

                // Check if child has nowrap (should not break line even if too wide)
                let child_has_nowrap = matches!(
                    child_style.white_space,
                    incognidium_style::WhiteSpace::NoWrap | incognidium_style::WhiteSpace::Pre
                );

                // Line breaking with float-aware width (include margins in width calculation)
                // Also force line break for br elements
                // Do NOT break if child has nowrap (let it overflow)
                let would_break = (line_x + margin_left + child_width + margin_right
                    > inline_x_start + inline_available + 0.5
                    && line_x > inline_x_start
                    && !child_has_nowrap)
                    || is_line_break;
                if would_break {
                    apply_text_align(
                        &mut layout_box.children,
                        line_begin,
                        j,
                        line_x - inline_x_start,
                        inline_available,
                        &style,
                        false, // Not the last line
                    );
                    cursor_y += line_height;
                    line_x = inline_x_start;
                    line_height = 0.0;
                    line_begin = j;
                    if cursor_y >= float_bottom {
                        float_right_width = 0.0;
                        float_left_width = 0.0;
                        inline_available = child_containing_width;
                        inline_x_start = content_x;
                        line_x = inline_x_start;
                    }
                }
                // Position child with margin-left offset
                // Special handling for list markers with list-style-position: outside
                let is_outside_marker = layout_box.children[j].is_list_marker
                    && layout_box.children[j].list_style_position == ListStylePosition::Outside;

                if is_outside_marker {
                    // Position outside marker in the left padding area
                    // The marker is positioned before the content starts
                    let marker_width = layout_box.children[j].width + 5.0;
                    layout_box.children[j].x = (content_x - marker_width).max(0.0);
                } else {
                    layout_box.children[j].x = line_x + margin_left;
                }
                layout_box.children[j].y = cursor_y;
                line_x += margin_left + child_width + margin_right;
                // Line height is the max of CSS line-height and tallest element on the line
                line_height = line_height.max(child_height);
            }

            // Apply text-align to the last line
            apply_text_align(
                &mut layout_box.children,
                line_begin,
                i,
                line_x - inline_x_start,
                inline_available,
                &style,
                true, // This is the last line
            );

            // Apply vertical-align to inline elements on this line
            // First pass: collect line metrics
            let mut max_ascent: f32 = 0.0;
            let mut max_descent: f32 = 0.0;

            for j in line_begin..i {
                let child_style = styles
                    .get(&layout_box.children[j].node_id)
                    .cloned()
                    .unwrap_or_default();
                let child_height = layout_box.children[j].height;

                // Estimate ascent/descent based on font metrics
                let ascent = child_style.font_size * 0.75;
                let descent = child_height - ascent;

                max_ascent = max_ascent.max(ascent);
                max_descent = max_descent.max(descent);
            }

            // If no text content on this line, use CSS line-height as baseline
            let has_text_content = (line_begin..i).any(|j| {
                layout_box.children[j].box_type == BoxType::Text
                    && layout_box.children[j].text.is_some()
            });
            if !has_text_content {
                // Use CSS line-height for baseline when no text
                max_ascent = css_line_height * 0.75;
                max_descent = css_line_height - max_ascent;
            }

            let baseline_y = max_ascent;

            // Second pass: apply vertical-align
            for j in line_begin..i {
                let child_style = styles
                    .get(&layout_box.children[j].node_id)
                    .cloned()
                    .unwrap_or_default();

                // Skip elements without explicit vertical-align (baseline is default)
                let child_height = layout_box.children[j].height;
                let debug_text = layout_box.children[j].text.clone().unwrap_or_default();
                let box_type = layout_box.children[j].box_type;
                let vertical_offset = match child_style.vertical_align {
                    incognidium_style::VerticalAlign::Top => {
                        // Align element top to line top (no offset needed)
                        0.0
                    }
                    incognidium_style::VerticalAlign::Bottom => {
                        // Align element bottom to line bottom
                        line_height - child_height
                    }
                    incognidium_style::VerticalAlign::Middle => {
                        // Center element vertically in the line
                        (line_height - child_height) / 2.0
                    }
                    incognidium_style::VerticalAlign::TextTop => {
                        // Align element top to text content top (ascender)
                        // The text content top is at baseline_y - max_ascent
                        // We want element top to be at that position
                        // Current position is line top (cursor_y)
                        // Offset = (baseline_y - max_ascent) - 0 = baseline_y - max_ascent = 0
                        // Actually, text-top means align with the font's ascender
                        let text_top = baseline_y - max_ascent;
                        text_top
                    }
                    incognidium_style::VerticalAlign::TextBottom => {
                        // Align element bottom to text content bottom (descender)
                        // Text bottom is at baseline_y + max_descent
                        // We want element bottom to be at that position
                        // Offset = (baseline_y + max_descent) - child_height
                        let text_bottom = baseline_y + max_descent;
                        text_bottom - child_height
                    }
                    incognidium_style::VerticalAlign::Super => {
                        // Raise above baseline (for inline text elements)
                        -(child_style.font_size * 0.4)
                    }
                    incognidium_style::VerticalAlign::Sub => {
                        // Lower below baseline (for inline text elements)
                        child_style.font_size * 0.25
                    }
                    _ => {
                        // Baseline - align element's baseline with line baseline
                        let is_text = layout_box.children[j].box_type == BoxType::Text;
                        let has_text_content = layout_box.children[j].text.is_some();
                        let is_inline_block =
                            layout_box.children[j].box_type == BoxType::InlineBlock;

                        if is_text {
                            // For pure text elements, the baseline is at ~75% of font size
                            // from the top of the text content area.
                            // The text's natural height is font_size, so the offset is:
                            // baseline_y - text_ascent = baseline_y - (font_size * 0.75)
                            // This positions the text so its baseline aligns with the line baseline
                            let text_ascent = child_style.font_size * 0.75;
                            baseline_y - text_ascent
                        } else if is_inline_block && has_text_content {
                            // For inline-block with text, align so the text baseline
                            // matches the line baseline. Estimate text baseline as
                            // 75% of font size from the top of the content area.
                            let text_baseline_in_element = child_style.font_size * 0.75;
                            // Position element so its text baseline aligns with line baseline
                            baseline_y - text_baseline_in_element
                        } else {
                            // For images and other replaced elements, align bottom to baseline
                            baseline_y - child_height
                        }
                    }
                };

                if vertical_offset != 0.0 {
                    layout_box.children[j].y += vertical_offset;
                }
            }

            cursor_y += line_height;
            first_inline_run = true; // Reset for next inline run after completing this one
        } else {
            // Block child
            let cm = styles.get(&child.node_id).cloned().unwrap_or_default();

            // Handle clear property - move past floats before laying out
            if cm.clear != incognidium_style::Clear::None && cursor_y < float_bottom {
                match cm.clear {
                    incognidium_style::Clear::Left if float_left_width > 0.0 => {
                        cursor_y = float_bottom;
                        float_left_width = 0.0;
                    }
                    incognidium_style::Clear::Right if float_right_width > 0.0 => {
                        cursor_y = float_bottom;
                        float_right_width = 0.0;
                    }
                    incognidium_style::Clear::Both => {
                        cursor_y = float_bottom;
                        float_left_width = 0.0;
                        float_right_width = 0.0;
                    }
                    _ => {}
                }
            }

            // Clear floats if cursor is past float bottom
            if cursor_y >= float_bottom {
                float_right_width = 0.0;
                float_left_width = 0.0;
            }

            // Handle floated elements
            if cm.float != Float::None {
                let float_content_width = match cm.width {
                    SizeValue::Px(w) => w,
                    SizeValue::Percent(p) => {
                        child_containing_width * p / 100.0 - cm.margin_left - cm.margin_right
                    }
                    _ => {
                        // Auto width: shrink-wrap to content (intrinsic width).
                        // For floats, we need to calculate the minimum width needed
                        // to contain the content without unnecessary wrapping.

                        // First compute layout at generous width to get text measurements
                        compute_layout(
                            &mut layout_box.children[i],
                            styles,
                            child_containing_width - cm.margin_left - cm.margin_right,
                            child_containing_height,
                            image_sizes,
                        );

                        // Then calculate intrinsic width from the laid out content
                        let child_ref = &layout_box.children[i];
                        let intrinsic = calculate_intrinsic_width(child_ref);
                        // Add padding and border to get total width
                        intrinsic + cm.padding_left + cm.padding_right
                            + cm.border_left_width + cm.border_right_width
                    }
                };
                compute_layout(
                    &mut layout_box.children[i],
                    styles,
                    float_content_width,
                    child_containing_height,
                    image_sizes,
                );
                if cm.float == Float::Right {
                    layout_box.children[i].x = content_x + child_containing_width
                        - layout_box.children[i].width
                        - cm.margin_right;
                    layout_box.children[i].y = cursor_y + cm.margin_top;
                    float_right_width =
                        layout_box.children[i].width + cm.margin_left + cm.margin_right;
                } else {
                    layout_box.children[i].x = content_x + float_left_width + cm.margin_left;
                    layout_box.children[i].y = cursor_y + cm.margin_top;
                    float_left_width +=
                        layout_box.children[i].width + cm.margin_left + cm.margin_right;
                }
                float_bottom =
                    (cursor_y + layout_box.children[i].height + cm.margin_top + cm.margin_bottom)
                        .max(float_bottom);
                i += 1;
                continue;
            }

            // Block beside a float: give it full width so text below
            // the float can use the full column. Pass float info so
            // layout_block can set up float state for inline children.
            let beside_float = cursor_y < float_bottom;
            let effective_width = child_containing_width;
            let effective_x = content_x;

            if beside_float {
                let pf = FloatState {
                    left_width: float_left_width,
                    right_width: float_right_width,
                    remaining_height: (float_bottom - cursor_y - cm.margin_top).max(0.0),
                };
                compute_layout_with_floats(
                    &mut layout_box.children[i],
                    styles,
                    effective_width,
                    child_containing_height,
                    image_sizes,
                    pf,
                );
            } else {
                compute_layout(
                    &mut layout_box.children[i],
                    styles,
                    effective_width,
                    child_containing_height,
                    image_sizes,
                );
            }
            // Skip zero-height/empty blocks from contributing margins.
            // A block with 0 content height and no visible background is an
            // empty collapsed container that shouldn't push siblings down.
            let child_has_visual = layout_box.children[i].height > 0.0
                && (layout_box.children[i].content_height > 0.0
                    || cm.background_color.a > 0
                    || cm.border_top_width > 0.0
                    || cm.border_bottom_width > 0.0);
            if child_has_visual {
                // Only center with auto margins if BOTH margin-left AND margin-right are auto
                let child_w = layout_box.children[i].width;
                let extra = (effective_width - child_w).max(0.0);
                let has_auto_margins = cm.margin_left_auto && cm.margin_right_auto;
                let x_offset = if has_auto_margins && child_w < effective_width && extra > 1.0 {
                    // Center the block: distribute extra space equally
                    extra / 2.0
                } else {
                    // Normal left-aligned block
                    cm.margin_left
                };
                // Margin collapse: use max of previous margin-bottom and current margin-top
                let collapsed_margin_top = cm.margin_top.max(prev_margin_bottom);
                layout_box.children[i].x = effective_x + x_offset;
                layout_box.children[i].y = cursor_y + collapsed_margin_top - prev_margin_bottom;
                cursor_y += collapsed_margin_top + layout_box.children[i].height;
                prev_margin_bottom = cm.margin_bottom;
            }
            i += 1;
        }
    }

    // Calculate height — must encompass floated children (block formatting context)
    // Add the last child's margin-bottom to auto_height
    let mut auto_height =
        cursor_y + prev_margin_bottom - style.padding_top - style.border_top_width;

    // SAFETY CAP: Prevent extreme height bloat from buggy layouts
    // Max reasonable content height (100k px covers most long articles)
    const MAX_AUTO_HEIGHT: f32 = 100_000.0;

    // Floats and absolutely positioned children can extend below the last block child;
    // the parent must contain them (creates a BFC for overflow:hidden or when it has floats)
    let mut auto_content_bottom = auto_height + style.padding_top + style.border_top_width;
    for child in &layout_box.children {
        let cs = styles.get(&child.node_id).cloned().unwrap_or_default();
        if cs.float != Float::None {
            let child_bottom = child.y + child.height + cs.margin_bottom;
            if child_bottom > auto_content_bottom {
                let extend_by = child_bottom - auto_content_bottom;
                // Safety check: don't extend beyond reasonable limits
                if auto_height + extend_by < MAX_AUTO_HEIGHT {
                    auto_height += extend_by;
                    auto_content_bottom += extend_by;
                } else {
                    // Cap at maximum
                    auto_height = MAX_AUTO_HEIGHT.min(auto_height + extend_by);
                    break;
                }
            }
        }
    }

    // Apply safety cap to auto_height
    auto_height = auto_height.min(MAX_AUTO_HEIGHT);
    let content_height = match style.height {
        SizeValue::Px(h) => h,
        SizeValue::Percent(p) => {
            if containing_height > 0.0 {
                containing_height * p / 100.0
            } else {
                auto_height
            }
        }
        _ => {
            // Check for aspect-ratio when height is auto
            if let Some(ref ar) = style.aspect_ratio {
                // Use the computed width to calculate height from aspect ratio
                // height = width / (aspect_ratio)
                let ratio = ar.width / ar.height.max(0.001);
                if ratio > 0.0 {
                    layout_box.content_width / ratio
                } else {
                    auto_height
                }
            } else {
                auto_height
            }
        }
    };
    // Apply min-height / max-height
    let content_height = match style.min_height {
        SizeValue::Px(mh) if content_height < mh => mh,
        _ => content_height,
    };
    let content_height = match style.max_height {
        SizeValue::Px(mh) if content_height > mh => mh,
        _ => content_height,
    };

    layout_box.content_height = content_height.max(0.0);
    layout_box.height = content_height
        + style.padding_top
        + style.padding_bottom
        + style.border_top_width
        + style.border_bottom_width;

    // Position absolutely/fixed positioned children
    let container_w = layout_box.width;
    let container_h = layout_box.height;
    for &idx in &abs_indices {
        let child = &mut layout_box.children[idx];
        let cs = styles.get(&child.node_id).cloned().unwrap_or_default();

        // Compute their layout with container width
        let (abs_width, is_auto_width) = match cs.width {
            SizeValue::Px(w) => (w, false),
            SizeValue::Percent(p) => (container_w * p / 100.0, false),
            _ => {
                // auto width: use container width as available space
                // We'll shrink-to-fit after layout
                (container_w, true)
            }
        };
        compute_layout(child, styles, abs_width, container_h, image_sizes);

        // For auto width, shrink-to-fit the content
        if is_auto_width {
            // Calculate the intrinsic width from the laid out content
            let intrinsic_width = calculate_intrinsic_width(child);
            // Apply the intrinsic width
            if intrinsic_width > 0.0 && intrinsic_width < child.width {
                child.width = intrinsic_width;
                child.content_width = intrinsic_width - cs.padding_left - cs.padding_right
                    - cs.border_left_width - cs.border_right_width;
            }
        }

        // Apply top/left/right/bottom
        // Use content width for positioning calculations (excluding padding/border)
        let content_w = container_w - cs.padding_left - cs.padding_right - cs.border_left_width - cs.border_right_width;
        child.x = match cs.left {
            SizeValue::Px(v) => v + cs.margin_left,
            SizeValue::Percent(p) => content_w * p / 100.0 + cs.margin_left,
            _ => match cs.right {
                SizeValue::Px(v) => (content_w - child.width - v - cs.margin_right).max(0.0),
                SizeValue::Percent(p) => {
                    (content_w - child.width - content_w * p / 100.0).max(0.0)
                }
                _ => cs.margin_left,
            },
        };
        child.y = match cs.top {
            SizeValue::Px(v) => v + cs.margin_top,
            SizeValue::Percent(p) => container_h * p / 100.0 + cs.margin_top,
            _ => match cs.bottom {
                SizeValue::Px(v) => (container_h - child.height - v - cs.margin_bottom).max(0.0),
                SizeValue::Percent(p) => {
                    (container_h - child.height - container_h * p / 100.0).max(0.0)
                }
                _ => cs.margin_top,
            },
        };
    }

    // Apply relative positioning offsets to positioned children
    // Relative positioning: offset from normal position without removing from flow
    for child in &mut layout_box.children {
        let cs = styles.get(&child.node_id).cloned().unwrap_or_default();
        if cs.position == Position::Relative {
            // Apply left/right offset (prefer left)
            let offset_x = match cs.left {
                SizeValue::Px(v) => v,
                SizeValue::Percent(p) => container_w * p / 100.0,
                _ => match cs.right {
                    SizeValue::Px(v) => -v,
                    SizeValue::Percent(p) => -(container_w * p / 100.0),
                    _ => 0.0,
                },
            };
            // Apply top/bottom offset (prefer top)
            let offset_y = match cs.top {
                SizeValue::Px(v) => v,
                SizeValue::Percent(p) => container_h * p / 100.0,
                _ => match cs.bottom {
                    SizeValue::Px(v) => -v,
                    SizeValue::Percent(p) => -(container_h * p / 100.0),
                    _ => 0.0,
                },
            };
            child.x += offset_x;
            child.y += offset_y;
        }
    }
}

/// Layout an inline-block element: establishes a block formatting context but
/// shrinks to fit its content width instead of expanding to the containing width.
fn layout_inline_block(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    image_sizes: &ImageSizes,
) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    let margin_left = style.margin_left;
    let margin_right = style.margin_right;
    let padding_left = style.padding_left;
    let padding_right = style.padding_right;
    let border_left = style.border_left_width;
    let border_right = style.border_right_width;
    let padding_top = style.padding_top;
    let padding_bottom = style.padding_bottom;
    let border_top = style.border_top_width;
    let border_bottom = style.border_bottom_width;

    let is_border_box = style.box_sizing == incognidium_style::BoxSizing::BorderBox;

    // Special handling for textarea: use rows/cols for sizing, unless field-sizing: content is set
    let is_textarea = layout_box.textarea_info.is_some();
    let textarea_cols = layout_box.textarea_info.map(|t| t.cols).unwrap_or(0);
    let textarea_rows = layout_box.textarea_info.map(|t| t.rows).unwrap_or(0);
    // field-sizing: content makes the field size to its content
    let field_sizing_content = style.field_sizing == incognidium_style::FieldSizing::Content;

    // Check if width is explicitly set
    let explicit_width = match style.width {
        SizeValue::Px(w) => Some(if is_border_box {
            (w - padding_left - padding_right - border_left - border_right).max(0.0)
        } else {
            w
        }),
        SizeValue::Percent(p) => {
            let total = containing_width * p / 100.0;
            Some(if is_border_box {
                (total - padding_left - padding_right - border_left - border_right).max(0.0)
            } else {
                total
            })
        }
        SizeValue::Auto | SizeValue::None => None,
        // CSS Math Functions - treat as auto for now
        _ => None,
    };

    if let Some(content_width) = explicit_width {
        // Explicit width: behave like a block with that width
        let mut content_width = content_width;

        // Apply max-width
        match style.max_width {
            SizeValue::Px(mw) if content_width > mw => content_width = mw,
            SizeValue::Percent(p) => {
                let mw = containing_width * p / 100.0;
                if content_width > mw {
                    content_width = mw;
                }
            }
            _ => {}
        }
        // Apply min-width
        match style.min_width {
            SizeValue::Px(mw) if content_width < mw => content_width = mw,
            SizeValue::Percent(p) => {
                let mw = containing_width * p / 100.0;
                if content_width < mw {
                    content_width = mw;
                }
            }
            _ => {}
        }

        layout_box.content_width = content_width.max(0.0);
        // For border-box, total width should be exactly the specified width
        layout_box.width = if is_border_box {
            match style.width {
                SizeValue::Px(w) => w,
                SizeValue::Percent(p) => containing_width * p / 100.0,
                _ => content_width + padding_left + padding_right + border_left + border_right,
            }
        } else {
            content_width + padding_left + padding_right + border_left + border_right
        };

        // Layout children as a block formatting context
        let child_containing = layout_box.content_width;
        let mut cursor_y: f32 = padding_top + border_top;
        let content_x = padding_left + border_left;

        // SAFETY CAP: Track total height to prevent runaway layout
        const MAX_HEIGHT: f32 = 100_000.0;

        for child in &mut layout_box.children {
            compute_layout(child, styles, child_containing, 0.0, image_sizes);
            let cm = styles.get(&child.node_id).cloned().unwrap_or_default();
            if child.height > 0.0 {
                child.x = content_x + cm.margin_left;
                child.y = cursor_y + cm.margin_top;
                cursor_y += cm.margin_top + child.height + cm.margin_bottom;
                // Safety check: stop if we're exceeding reasonable height
                if cursor_y > MAX_HEIGHT {
                    break;
                }
            }
        }

        let auto_height = if is_textarea && textarea_rows > 0 && !field_sizing_content {
            // Calculate height based on rows attribute (unless field-sizing: content)
            let line_height = style.font_size * style.line_height;
            (textarea_rows as f32 * line_height).min(MAX_HEIGHT)
        } else if is_textarea && field_sizing_content {
            // field-sizing: content - size to actual content height
            (cursor_y - padding_top - border_top).min(MAX_HEIGHT)
        } else if layout_box.input_type.is_some() && !is_textarea {
            // Input elements: use font size for reasonable single-line height
            let line_height = style.font_size * style.line_height;
            line_height.min(MAX_HEIGHT)
        } else {
            (cursor_y - padding_top - border_top).min(MAX_HEIGHT)
        };
        let content_height = match style.height {
            SizeValue::Px(h) => h.min(MAX_HEIGHT),
            _ => auto_height,
        };
        let content_height = match style.min_height {
            SizeValue::Px(mh) if content_height < mh => mh,
            _ => content_height,
        };
        let content_height = match style.max_height {
            SizeValue::Px(mh) if content_height > mh => mh,
            _ => content_height,
        };

        layout_box.content_height = content_height.max(0.0);
        layout_box.height =
            content_height + padding_top + padding_bottom + border_top + border_bottom;
    } else {
        // Auto width: shrink-to-fit
        // Layout children with the max available width first to get their natural sizes
        let max_available = containing_width
            - margin_left
            - margin_right
            - padding_left
            - padding_right
            - border_left
            - border_right;

        let content_x = padding_left + border_left;
        let mut cursor_y: f32 = padding_top + border_top;
        let mut max_child_width: f32 = 0.0;

        // SAFETY CAP for auto-width inline-block
        const MAX_HEIGHT: f32 = 100_000.0;

        for child in &mut layout_box.children {
            compute_layout(child, styles, max_available.max(0.0), 0.0, image_sizes);
            let cm = styles.get(&child.node_id).cloned().unwrap_or_default();
            if child.height > 0.0 {
                child.x = content_x + cm.margin_left;
                child.y = cursor_y + cm.margin_top;
                cursor_y += cm.margin_top + child.height + cm.margin_bottom;
                // Safety check
                if cursor_y > MAX_HEIGHT {
                    break;
                }
            }
            max_child_width = max_child_width.max(child.width + cm.margin_left + cm.margin_right);
        }

        // Shrink to fit: use the widest child, clamped by available space
        // For textarea, calculate width based on cols attribute
        // For checkbox/radio, use a square size based on font size
        let is_checkbox_radio = matches!(
            layout_box.input_type,
            Some(InputType::Checkbox { .. }) | Some(InputType::Radio { .. })
        );
        let mut content_width = if is_textarea && textarea_cols > 0 && !field_sizing_content {
            // Estimate character width based on cols attribute (unless field-sizing: content)
            let char_width = style.font_size * 0.6; // Approximate char width
            (textarea_cols as f32 * char_width).min(max_available.max(0.0))
        } else if is_textarea && field_sizing_content {
            // field-sizing: content - size to actual content width
            max_child_width.min(max_available.max(0.0))
        } else if is_checkbox_radio {
            // Checkbox/radio: use line height as intrinsic size (square aspect ratio)
            let line_height = style.font_size * style.line_height;
            line_height.min(max_available.max(0.0))
        } else {
            max_child_width.min(max_available.max(0.0))
        };

        // Apply max-width
        match style.max_width {
            SizeValue::Px(mw) if content_width > mw => {
                content_width = mw;
            }
            SizeValue::Percent(p) => {
                let mw = containing_width * p / 100.0;
                if content_width > mw {
                    content_width = mw;
                }
            }
            _ => {}
        }
        // Apply min-width
        match style.min_width {
            SizeValue::Px(mw) if content_width < mw => {
                content_width = mw;
            }
            SizeValue::Percent(p) => {
                let mw = containing_width * p / 100.0;
                if content_width < mw {
                    content_width = mw;
                }
            }
            _ => {}
        }

        layout_box.content_width = content_width.max(0.0);
        layout_box.width =
            content_width + padding_left + padding_right + border_left + border_right;

        let auto_height = if is_textarea && textarea_rows > 0 && !field_sizing_content {
            // Calculate height based on rows attribute (unless field-sizing: content)
            let line_height = style.font_size * style.line_height;
            (textarea_rows as f32 * line_height).min(MAX_HEIGHT)
        } else if is_textarea && field_sizing_content {
            // field-sizing: content - size to actual content height
            (cursor_y - padding_top - border_top).min(MAX_HEIGHT)
        } else if layout_box.input_type.is_some() && !is_textarea {
            // Input elements: use font size for reasonable single-line height
            let line_height = style.font_size * style.line_height;
            line_height.min(MAX_HEIGHT)
        } else {
            (cursor_y - padding_top - border_top).min(MAX_HEIGHT)
        };
        let content_height = match style.height {
            SizeValue::Px(h) => h.min(MAX_HEIGHT),
            _ => auto_height,
        };
        let content_height = match style.min_height {
            SizeValue::Px(mh) if content_height < mh => mh,
            _ => content_height,
        };
        let content_height = match style.max_height {
            SizeValue::Px(mh) if content_height > mh => mh,
            _ => content_height,
        };

        layout_box.content_height = content_height.max(0.0);
        layout_box.height =
            content_height + padding_top + padding_bottom + border_top + border_bottom;
    }
}

/// Check if a box type participates in inline flow.
#[allow(dead_code)]
fn is_inline_level(box_type: BoxType) -> bool {
    matches!(
        box_type,
        BoxType::Text | BoxType::Inline | BoxType::InlineBlock | BoxType::LineBreak
    )
}

fn is_inline_level_styled(box_type: BoxType, styles: &StyleMap, node_id: NodeId) -> bool {
    if matches!(
        box_type,
        BoxType::Text | BoxType::Inline | BoxType::InlineBlock | BoxType::LineBreak
    ) {
        return true;
    }
    if box_type == BoxType::Image {
        let display = styles
            .get(&node_id)
            .map(|s| s.display)
            .unwrap_or(Display::InlineBlock);
        return display != Display::Block;
    }
    false
}

/// Compute inter-element gap to prevent text concatenation like "wordword".
/// Returns a Vec of gap values to add before each child.
fn compute_inline_gaps(
    children: &[LayoutBox],
    start: usize,
    end: usize,
    styles: &StyleMap,
) -> Vec<f32> {
    // Use parent font size to compute accurate space width
    let parent_font_size = children
        .get(start)
        .and_then(|c| styles.get(&c.node_id))
        .map(|s| s.font_size)
        .unwrap_or(16.0);
    let default_style = incognidium_style::ComputedStyle::default();
    let space_width = measure_text_width(" ", parent_font_size, &default_style);
    let count = end - start;
    let mut gaps = vec![0.0f32; count];
    for j in 1..count {
        let prev = &children[start + j - 1];
        let curr = &children[start + j];
        if prev.width > 0.0 && curr.width > 0.0 {
            let prev_is_space = prev.text.as_deref() == Some(" ");
            let curr_is_space = curr.text.as_deref() == Some(" ");
            let prev_ends_space = prev
                .text
                .as_deref()
                .map(|t| t.ends_with(' '))
                .unwrap_or(false);
            let curr_starts_space = curr
                .text
                .as_deref()
                .map(|t| t.starts_with(' '))
                .unwrap_or(false);

            if !prev_is_space && !curr_is_space && !prev_ends_space && !curr_starts_space {
                let prev_has_content = prev.text.is_some() || prev.box_type == BoxType::Inline;
                let curr_has_content = curr.text.is_some() || curr.box_type == BoxType::Inline;
                if prev_has_content && curr_has_content {
                    gaps[j] = space_width;
                }
            }
        }
    }
    gaps
}

/// Shift inline children on a line for text-align: center or right.
/// For the last line of justified text, uses text-align-last if specified.
fn apply_text_align(
    children: &mut [LayoutBox],
    start: usize,
    end: usize,
    used_width: f32,
    container_width: f32,
    style: &incognidium_style::ComputedStyle,
    is_last_line: bool,
) {
    let remaining = container_width - used_width;
    if remaining <= 1.0 {
        return;
    }

    // Determine effective alignment
    let align = if is_last_line && style.text_align == TextAlign::Justify {
        // For last line of justified text, use text-align-last
        match style.text_align_last {
            TextAlignLast::Auto => TextAlign::Left, // Default to left for auto
            TextAlignLast::Left | TextAlignLast::Start => TextAlign::Left,
            TextAlignLast::Right | TextAlignLast::End => TextAlign::Right,
            TextAlignLast::Center => TextAlign::Center,
            TextAlignLast::Justify => TextAlign::Justify, // Will be handled elsewhere
        }
    } else {
        style.text_align
    };

    let shift = match align {
        TextAlign::Center => remaining / 2.0,
        TextAlign::Right => remaining,
        // Note: Justify requires word-level spacing adjustment which needs to be
        // handled at text layout time, not here. For now, treat justify as left.
        TextAlign::Left | TextAlign::Justify => return,
    };
    for child in &mut children[start..end] {
        child.x += shift;
    }
}

/// Layout an inline element (e.g. <a>, <span>): shrink-to-fit width.
fn layout_inline(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    image_sizes: &ImageSizes,
) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    let padding_left = style.padding_left;
    let padding_right = style.padding_right;
    let padding_top = style.padding_top;
    let padding_bottom = style.padding_bottom;
    let border_left = style.border_left_width;
    let border_right = style.border_right_width;
    let border_top = style.border_top_width;
    let border_bottom = style.border_bottom_width;
    let margin_left = style.margin_left;
    let margin_right = style.margin_right;

    // Layout all children first to get their natural sizes
    for child in &mut layout_box.children {
        compute_layout(child, styles, containing_width.max(0.0), 0.0, image_sizes);
    }

    // Compute inter-element gaps for inline children
    let num_children = layout_box.children.len();
    let gaps = compute_inline_gaps(&layout_box.children, 0, num_children, styles);

    // Position children inline (horizontal flow), wrapping when needed
    let mut line_x: f32 = margin_left;
    let mut line_height: f32 = 0.0;
    let mut total_height: f32 = 0.0;
    let mut max_line_width: f32 = 0.0;

    for (idx, child) in layout_box.children.iter_mut().enumerate() {
        line_x += gaps[idx];

        // Check if this is a line break (br element)
        let is_line_break = child.box_type == BoxType::LineBreak;

        if is_line_break {
            // Line break: end current line and start new one
            max_line_width = max_line_width.max(line_x);
            total_height += line_height;
            line_x = margin_left;
            line_height = 0.0;
            // Position the line break box at the start of the new line (invisible)
            child.x = line_x + padding_left + border_left;
            child.y = total_height + padding_top + border_top;
            child.width = 0.0;
            child.height = 0.0;
            continue;
        }

        // Wrap if needed (0.5px tolerance for f32 rounding)
        if line_x + child.width > containing_width + 0.5 && line_x > margin_left {
            max_line_width = max_line_width.max(line_x);
            total_height += line_height;
            line_x = margin_left;
            line_height = 0.0;
        }
        child.x = line_x + padding_left + border_left;
        child.y = total_height + padding_top + border_top;
        line_x += child.width;
        line_height = line_height.max(child.height);
    }
    total_height += line_height;
    line_x += margin_right; // Add right margin to total width
    max_line_width = max_line_width.max(line_x);

    layout_box.content_width = max_line_width;
    layout_box.content_height = total_height;
    layout_box.width = max_line_width + padding_left + padding_right + border_left + border_right;
    layout_box.height = total_height + padding_top + padding_bottom + border_top + border_bottom;
}

fn layout_flex(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    image_sizes: &ImageSizes,
) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    let padding_left = style.padding_left;
    let padding_right = style.padding_right;
    let padding_top = style.padding_top;
    let padding_bottom = style.padding_bottom;
    let border_left = style.border_left_width;
    let border_right = style.border_right_width;
    let border_top = style.border_top_width;
    let border_bottom = style.border_bottom_width;

    let is_border_box = style.box_sizing == incognidium_style::BoxSizing::BorderBox;
    let content_width = match style.width {
        SizeValue::Px(w) => {
            if is_border_box {
                (w - padding_left - padding_right - border_left - border_right).max(0.0)
            } else {
                w
            }
        }
        SizeValue::Percent(p) => {
            let total = containing_width * p / 100.0;
            if is_border_box {
                (total - padding_left - padding_right - border_left - border_right).max(0.0)
            } else {
                total
            }
        }
        SizeValue::Auto | SizeValue::None => {
            containing_width
                - style.margin_left
                - style.margin_right
                - padding_left
                - padding_right
                - border_left
                - border_right
        }
        // CSS Math Functions - treat as auto for now
        _ => {
            containing_width
                - style.margin_left
                - style.margin_right
                - padding_left
                - padding_right
                - border_left
                - border_right
        }
    };

    layout_box.content_width = content_width.max(0.0);
    layout_box.width = content_width + padding_left + padding_right + border_left + border_right;

    let is_row = matches!(
        style.flex_direction,
        FlexDirection::Row | FlexDirection::RowReverse
    );

    let wrapping = style.flex_wrap != FlexWrap::NoWrap;

    // Container main-axis size for wrapping decisions
    let container_main = if is_row {
        content_width
    } else {
        match style.height {
            SizeValue::Px(h) => h,
            _ => f32::MAX, // column with auto height: no wrapping constraint
        }
    };

    // Compute the explicit container cross-axis size if any (for column wrapping)
    let container_cross_explicit = if is_row {
        match style.height {
            SizeValue::Px(h) => Some(h),
            _ => None,
        }
    } else {
        Some(content_width)
    };
    let _ = container_cross_explicit; // used implicitly through content_width for columns

    // Blockify inline flex children (CSS spec: flex items are blockified)
    for child in &mut layout_box.children {
        if child.box_type == BoxType::Inline {
            child.box_type = BoxType::Block;
        }
    }

    // Remove absolutely/fixed positioned children from flex flow
    let abs_child_ids: Vec<NodeId> = layout_box
        .children
        .iter()
        .filter(|c| {
            let cs = styles.get(&c.node_id).cloned().unwrap_or_default();
            cs.position == Position::Absolute
                || cs.position == Position::Fixed
        })
        .map(|c| c.node_id)
        .collect();

    // Sort children by CSS order property (stable sort preserves source order for same value)
    layout_box
        .children
        .sort_by_key(|child| styles.get(&child.node_id).map(|s| s.order).unwrap_or(0));

    // First pass: compute natural sizes of non-absolute children
    let num_children = layout_box
        .children
        .iter()
        .filter(|c| !abs_child_ids.contains(&c.node_id))
        .count();
    for child in &mut layout_box.children {
        if abs_child_ids.contains(&child.node_id) {
            continue;
        }
        let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
        let basis = match child_style.flex_basis {
            SizeValue::Px(v) => v,
            SizeValue::Percent(p) => {
                if is_row {
                    content_width * p / 100.0
                } else {
                    0.0
                }
            }
            _ => {
                // Auto: use width/height or content size
                if is_row {
                    match child_style.width {
                        SizeValue::Px(w) => w,
                        SizeValue::Percent(p) => content_width * p / 100.0,
                        _ => 0.0, // Will be determined by content
                    }
                } else {
                    match child_style.height {
                        SizeValue::Px(h) => h,
                        _ => 0.0,
                    }
                }
            }
        };

        if is_row {
            let initial_width = if basis > 0.0 {
                basis
            } else {
                // When flex-basis is auto, let content determine natural size
                // by using a large containing width. Flex-grow/shrink will
                // distribute space in the second pass.
                content_width.max(10000.0)
            };
            compute_layout(child, styles, initial_width, 0.0, image_sizes);
        } else {
            compute_layout(child, styles, content_width, 0.0, image_sizes);
        }
    }

    // Filter out whitespace-only text nodes from flex children (like grid does)
    let mut flex_children: Vec<usize> = Vec::new();
    for (i, child) in layout_box.children.iter().enumerate() {
        if abs_child_ids.contains(&child.node_id) {
            continue;
        }
        if child.box_type == BoxType::Text {
            if let Some(ref text) = child.text {
                if text.trim().is_empty() {
                    continue;
                }
            }
        }
        flex_children.push(i);
    }

    // Group children into flex lines
    // Each line is a range of indices into flex_children
    let mut lines: Vec<(usize, usize)> = Vec::new();
    let num_flex_children = flex_children.len();
    if wrapping && num_flex_children > 0 {
        let mut line_start = 0;
        let mut line_main_used = 0.0_f32;
        for idx in 0..num_flex_children {
            let i = flex_children[idx];
            let child = &layout_box.children[i];
            let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
            let child_main = if is_row {
                child.width + child_style.margin_left + child_style.margin_right
            } else {
                child.height + child_style.margin_top + child_style.margin_bottom
            };
            let gap_before = if idx > line_start { style.gap } else { 0.0 };

            if idx > line_start && line_main_used + gap_before + child_main > container_main + 0.5 {
                // This item overflows; start a new line
                lines.push((line_start, idx));
                line_start = idx;
                line_main_used = child_main;
            } else {
                line_main_used += gap_before + child_main;
            }
        }
        lines.push((line_start, num_flex_children));
    } else {
        // NoWrap: everything on one line
        if num_flex_children > 0 {
            lines.push((0, num_flex_children));
        }
    }

    // For WrapReverse, reverse the order of lines (but not the items within them)
    if style.flex_wrap == FlexWrap::WrapReverse {
        lines.reverse();
    }

    // Second pass: for each line, distribute space (flex-grow/shrink) and position items
    let content_x = padding_left + border_left;
    let content_y = padding_top + border_top;
    let mut cross_cursor: f32 = 0.0; // accumulated cross-axis offset for stacking lines

    // We need per-line cross sizes to do alignment later
    let mut line_cross_sizes: Vec<f32> = Vec::with_capacity(lines.len());

    for &(line_start, line_end) in &lines {
        let line_count = line_end - line_start;
        if line_count == 0 {
            line_cross_sizes.push(0.0);
            continue;
        }

        // Map line indices to actual child indices
        let line_child_indices: Vec<usize> = (line_start..line_end)
            .map(|idx| flex_children[idx])
            .collect();

        // Compute total main size and total flex-grow for this line
        let line_main_size: f32 = line_child_indices
            .iter()
            .map(|i| {
                let c = &layout_box.children[*i];
                if is_row {
                    c.width
                } else {
                    c.height
                }
            })
            .sum();

        let line_gap_total = style.gap * (line_count.saturating_sub(1) as f32);

        let line_available = if is_row {
            content_width
        } else {
            match style.height {
                SizeValue::Px(h) => h,
                _ => match style.min_height {
                    SizeValue::Px(mh) => mh,
                    _ => line_main_size, // auto height = no free space
                },
            }
        } - line_gap_total;

        let line_free = (line_available - line_main_size).max(0.0);

        let line_total_grow: f32 = line_child_indices
            .iter()
            .map(|i| {
                styles
                    .get(&layout_box.children[*i].node_id)
                    .map(|s| s.flex_grow)
                    .unwrap_or(0.0)
            })
            .sum();

        // Distribute flex-grow within this line
        if line_total_grow > 0.0 && line_free > 0.0 {
            for &i in &line_child_indices {
                let grow = styles
                    .get(&layout_box.children[i].node_id)
                    .map(|s| s.flex_grow)
                    .unwrap_or(0.0);
                if grow > 0.0 {
                    let extra = line_free * (grow / line_total_grow);
                    if is_row {
                        layout_box.children[i].width += extra;
                        layout_box.children[i].content_width += extra;
                        // Re-layout children with new width, but preserve the computed width
                        let new_width = layout_box.children[i].width;
                        let new_content_width = layout_box.children[i].content_width;
                        let cw = layout_box.children[i].content_width;
                        compute_layout(&mut layout_box.children[i], styles, cw, 0.0, image_sizes);
                        // Restore the flex-determined width (re-layout may have changed it)
                        layout_box.children[i].width = new_width;
                        layout_box.children[i].content_width = new_content_width;
                    } else {
                        layout_box.children[i].height += extra;
                        layout_box.children[i].content_height += extra;
                    }
                }
            }
        }

        // Handle flex-shrink when items overflow the line (only for NoWrap or when line has one item)
        if !wrapping || line_count == 1 {
            let line_main_after_grow: f32 = line_child_indices
                .iter()
                .map(|i| {
                    let c = &layout_box.children[*i];
                    if is_row {
                        c.width
                    } else {
                        c.height
                    }
                })
                .sum();
            let overflow = line_main_after_grow + line_gap_total
                - (if is_row {
                    content_width
                } else {
                    match style.height {
                        SizeValue::Px(h) => h,
                        _ => line_main_after_grow, // auto = no overflow
                    }
                });
            if overflow > 0.0 {
                let line_total_shrink: f32 = line_child_indices
                    .iter()
                    .map(|i| {
                        styles
                            .get(&layout_box.children[*i].node_id)
                            .map(|s| s.flex_shrink)
                            .unwrap_or(1.0)
                    })
                    .sum();
                if line_total_shrink > 0.0 {
                    for &i in &line_child_indices {
                        let shrink = styles
                            .get(&layout_box.children[i].node_id)
                            .map(|s| s.flex_shrink)
                            .unwrap_or(1.0);
                        if shrink > 0.0 {
                            let reduction = overflow * (shrink / line_total_shrink);
                            if is_row {
                                layout_box.children[i].width =
                                    (layout_box.children[i].width - reduction).max(0.0);
                                layout_box.children[i].content_width =
                                    (layout_box.children[i].content_width - reduction).max(0.0);
                                let cw = layout_box.children[i].content_width;
                                compute_layout(
                                    &mut layout_box.children[i],
                                    styles,
                                    cw,
                                    0.0,
                                    image_sizes,
                                );
                            } else {
                                layout_box.children[i].height =
                                    (layout_box.children[i].height - reduction).max(0.0);
                                layout_box.children[i].content_height =
                                    (layout_box.children[i].content_height - reduction).max(0.0);
                            }
                        }
                    }
                }
            }
        }

        // Position items on this line
        let final_line_main: f32 = line_child_indices
            .iter()
            .map(|i| {
                let c = &layout_box.children[*i];
                if is_row {
                    c.width
                } else {
                    c.height
                }
            })
            .sum();
        let line_remaining = line_available - final_line_main;

        let (mut main_cursor, gap_between) = match style.justify_content {
            JustifyContent::FlexStart => (0.0_f32, style.gap),
            JustifyContent::FlexEnd => (line_remaining.max(0.0), style.gap),
            JustifyContent::Center => (line_remaining.max(0.0) / 2.0, style.gap),
            JustifyContent::SpaceBetween => {
                let n = line_count as f32;
                if n > 1.0 {
                    (0.0, line_remaining.max(0.0) / (n - 1.0))
                } else {
                    (0.0, 0.0)
                }
            }
            JustifyContent::SpaceAround => {
                let n = line_count as f32;
                let space = line_remaining.max(0.0) / n;
                (space / 2.0, space)
            }
            JustifyContent::SpaceEvenly => {
                let n = line_count as f32;
                let space = line_remaining.max(0.0) / (n + 1.0);
                (space, space)
            }
        };

        let mut line_max_cross: f32 = 0.0;
        for (idx, &i) in line_child_indices.iter().enumerate() {
            let child_style = styles
                .get(&layout_box.children[i].node_id)
                .cloned()
                .unwrap_or_default();
            if is_row {
                layout_box.children[i].x = content_x + main_cursor + child_style.margin_left;
                layout_box.children[i].y = content_y + cross_cursor + child_style.margin_top;
                main_cursor += layout_box.children[i].width
                    + child_style.margin_left
                    + child_style.margin_right;
                if idx < line_count - 1 {
                    main_cursor += gap_between;
                }
                line_max_cross = line_max_cross.max(
                    layout_box.children[i].height
                        + child_style.margin_top
                        + child_style.margin_bottom,
                );
            } else {
                layout_box.children[i].x = content_x + cross_cursor + child_style.margin_left;
                layout_box.children[i].y = content_y + main_cursor + child_style.margin_top;
                main_cursor += layout_box.children[i].height
                    + child_style.margin_top
                    + child_style.margin_bottom;
                if idx < line_count - 1 {
                    main_cursor += gap_between;
                }
                line_max_cross = line_max_cross.max(
                    layout_box.children[i].width
                        + child_style.margin_left
                        + child_style.margin_right,
                );
            }
        }

        line_cross_sizes.push(line_max_cross);
        cross_cursor += line_max_cross;
    }

    // Calculate total cross-axis size from all lines (including gaps between lines)
    let num_lines = lines.len();
    let cross_gap = if is_row {
        style.row_gap
    } else {
        style.column_gap
    };
    let total_cross: f32 = line_cross_sizes.iter().sum::<f32>()
        + if num_lines > 1 {
            cross_gap * (num_lines.saturating_sub(1) as f32)
        } else {
            0.0
        };

    // Calculate height
    let content_height = match style.height {
        SizeValue::Px(h) => h,
        _ => {
            if is_row {
                total_cross
            } else {
                // For column direction, main axis is vertical
                // Use the longest line's main cursor
                // We need to recompute: take the max main size across all lines
                let mut max_main: f32 = 0.0;
                for &(line_start, line_end) in &lines {
                    let line_main: f32 = (line_start..line_end)
                        .map(|i| {
                            let cs = styles
                                .get(&layout_box.children[i].node_id)
                                .cloned()
                                .unwrap_or_default();
                            layout_box.children[i].height + cs.margin_top + cs.margin_bottom
                        })
                        .sum();
                    let line_gap = style.gap * ((line_end - line_start).saturating_sub(1) as f32);
                    max_main = max_main.max(line_main + line_gap);
                }
                max_main
            }
        }
    };

    // Apply min-height for flex containers (e.g. min-height: 100vh)
    let content_height = match style.min_height {
        SizeValue::Px(mh) if content_height < mh => mh,
        _ => content_height,
    };

    // SAFETY CAP: Prevent extreme flex container heights
    let content_height = content_height.min(100_000.0);

    layout_box.content_height = content_height.max(0.0);
    layout_box.height = content_height + padding_top + padding_bottom + border_top + border_bottom;

    // For wrapping column flex, adjust container width to fit all lines
    if !is_row && wrapping && lines.len() > 1 {
        let total_line_cross: f32 = line_cross_sizes.iter().sum();
        if total_line_cross > content_width {
            layout_box.content_width = total_line_cross;
            layout_box.width =
                total_line_cross + padding_left + padding_right + border_left + border_right;
        }
    }

    // For row flex, update content_width to actual children usage
    // (needed for shrink-to-fit when this flex is inside another flex).
    if is_row && lines.len() == 1 {
        let (ls, le) = lines[0];
        let actual_main: f32 = (ls..le)
            .filter(|&i| !abs_child_ids.contains(&layout_box.children[i].node_id))
            .map(|i| {
                let cs = styles
                    .get(&layout_box.children[i].node_id)
                    .cloned()
                    .unwrap_or_default();
                layout_box.children[i].width + cs.margin_left + cs.margin_right
            })
            .sum::<f32>()
            + style.gap * (le - ls).saturating_sub(1) as f32;
        if actual_main < layout_box.content_width {
            layout_box.content_width = actual_main;
        }
    }

    // Calculate align-content distribution
    // align-content controls how flex lines are distributed in the cross axis
    let cross_gap = if is_row {
        style.row_gap
    } else {
        style.column_gap
    };

    // Calculate total cross size used by lines
    let total_lines_cross: f32 = line_cross_sizes.iter().sum::<f32>()
        + if lines.len() > 1 {
            cross_gap * (lines.len().saturating_sub(1) as f32)
        } else {
            0.0
        };

    // Calculate available cross-axis space for align-content
    let available_cross = if is_row {
        content_height
    } else {
        content_width
    };
    let extra_cross = (available_cross - total_lines_cross).max(0.0);

    // Calculate initial cross_offset based on align-content
    let (initial_cross_offset, line_gap_adjustment) = if lines.len() <= 1 {
        (0.0, cross_gap) // Single line, no align-content effect
    } else {
        use incognidium_style::AlignContent;
        match style.place_content.0 {
            AlignContent::FlexEnd => (extra_cross, cross_gap),
            AlignContent::Center => (extra_cross / 2.0, cross_gap),
            AlignContent::SpaceBetween => {
                if lines.len() > 1 {
                    let gap = extra_cross / (lines.len() - 1) as f32;
                    (0.0, cross_gap + gap)
                } else {
                    (0.0, cross_gap)
                }
            }
            AlignContent::SpaceAround => {
                let gap = extra_cross / lines.len() as f32;
                (gap / 2.0, cross_gap + gap)
            }
            AlignContent::SpaceEvenly => {
                let gap = extra_cross / (lines.len() + 1) as f32;
                (gap, cross_gap + gap)
            }
            AlignContent::Stretch => {
                // Stretch lines to fill container - handled below
                (0.0, cross_gap)
            }
            _ => (0.0, cross_gap), // FlexStart (default)
        }
    };

    // Cross-axis alignment within each line
    let mut cross_offset: f32 = initial_cross_offset;

    // For single-line flex containers with explicit cross size,
    // use the container's cross size for alignment (minus padding/border)
    let container_cross_for_alignment = if lines.len() == 1 {
        container_cross_explicit.map(|h| {
            if is_row {
                h - padding_top - padding_bottom
            } else {
                h - padding_left - padding_right
            }
        })
    } else {
        None
    };

    for (line_idx, &(line_start, line_end)) in lines.iter().enumerate() {
        // Use container's cross size if available and larger than content
        let line_cross = container_cross_for_alignment
            .unwrap_or(line_cross_sizes[line_idx])
            .max(line_cross_sizes[line_idx]);
        for i in line_start..line_end {
            let child_style = styles
                .get(&layout_box.children[i].node_id)
                .cloned()
                .unwrap_or_default();
            if is_row {
                match style.align_items {
                    AlignItems::Center => {
                        layout_box.children[i].y = content_y
                            + cross_offset
                            + (line_cross - layout_box.children[i].height) / 2.0;
                    }
                    AlignItems::FlexEnd => {
                        layout_box.children[i].y = content_y + cross_offset + line_cross
                            - layout_box.children[i].height
                            - child_style.margin_bottom;
                    }
                    AlignItems::Stretch => {
                        layout_box.children[i].height =
                            line_cross - child_style.margin_top - child_style.margin_bottom;
                    }
                    _ => {} // FlexStart and Baseline keep default position
                }
            } else {
                match style.align_items {
                    AlignItems::Center => {
                        layout_box.children[i].x = content_x
                            + cross_offset
                            + (line_cross - layout_box.children[i].width) / 2.0;
                    }
                    AlignItems::FlexEnd => {
                        layout_box.children[i].x = content_x + cross_offset + line_cross
                            - layout_box.children[i].width
                            - child_style.margin_right;
                    }
                    AlignItems::Stretch => {
                        layout_box.children[i].width =
                            line_cross - child_style.margin_left - child_style.margin_right;
                    }
                    _ => {}
                }
            }
        }
        cross_offset += line_cross;
        // Add gap between flex lines (except after the last line)
        // Use line_gap_adjustment which incorporates align-content spacing
        if line_idx + 1 < lines.len() {
            cross_offset += line_gap_adjustment;
        }
    }
}

fn layout_grid(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    image_sizes: &ImageSizes,
) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    let padding_left = style.padding_left;
    let padding_right = style.padding_right;
    let padding_top = style.padding_top;
    let padding_bottom = style.padding_bottom;
    let border_left = style.border_left_width;
    let border_right = style.border_right_width;
    let border_top = style.border_top_width;
    let border_bottom = style.border_bottom_width;

    // Resolve container content width
    let content_width = match style.width {
        SizeValue::Px(w) => {
            if style.box_sizing == incognidium_style::BoxSizing::BorderBox {
                (w - padding_left - padding_right - border_left - border_right).max(0.0)
            } else {
                w
            }
        }
        SizeValue::Percent(p) => {
            let total = containing_width * p / 100.0;
            if style.box_sizing == incognidium_style::BoxSizing::BorderBox {
                (total - padding_left - padding_right - border_left - border_right).max(0.0)
            } else {
                total
            }
        }
        SizeValue::Auto | SizeValue::None => {
            containing_width
                - style.margin_left
                - style.margin_right
                - padding_left
                - padding_right
                - border_left
                - border_right
        }
        // CSS Math Functions - treat as auto for now
        _ => {
            containing_width
                - style.margin_left
                - style.margin_right
                - padding_left
                - padding_right
                - border_left
                - border_right
        }
    };
    let content_width = content_width.max(0.0);

    let num_children = layout_box.children.len();
    if num_children == 0 {
        layout_box.content_width = content_width;
        layout_box.width =
            content_width + padding_left + padding_right + border_left + border_right;
        layout_box.content_height = 0.0;
        layout_box.height = padding_top + padding_bottom + border_top + border_bottom;
        return;
    }

    let col_gap = style.column_gap;
    let row_gap = style.row_gap;

    // grid-template-areas can override the number of columns
    // Each row in grid-template-areas defines column positions
    let num_cols_from_areas = style.grid_template_areas.iter().map(|row| row.len()).max();
    let num_cols = if style.grid_template_columns.is_empty() {
        num_cols_from_areas.unwrap_or(1)
    } else {
        style.grid_template_columns.len()
    };

    let col_widths = if style.grid_template_columns.is_empty() {
        vec![content_width]
    } else {
        resolve_track_sizes(&style.grid_template_columns, content_width, col_gap)
    };

    // Get auto-column size for implicit columns
    let auto_col_size = style
        .grid_auto_columns
        .first()
        .map(|t| match t {
            incognidium_style::GridTrackSize::Px(px) => *px,
            incognidium_style::GridTrackSize::Percent(p) => content_width * p / 100.0,
            _ => 100.0, // Default fallback
        })
        .unwrap_or(100.0); // Default if not specified

    // Helper to get column width (explicit or implicit)
    let get_col_width = |c: usize| -> f32 {
        if c < col_widths.len() {
            col_widths[c]
        } else {
            auto_col_size
        }
    };

    // Resolve explicit row heights
    let explicit_row_tracks = &style.grid_template_rows;
    let content_x = padding_left + border_left;
    let content_y = padding_top + border_top;

    // Grid placement: resolve each child's (col_start, col_end, row_start, row_end).
    // CSS grid lines are 1-indexed. Negative values count from the end.
    // Children without explicit placement get auto-placed into the next free cell.
    struct CellPlacement {
        col_start: usize, // 0-indexed column
        col_end: usize,   // exclusive
        row_start: usize, // 0-indexed row
        row_end: usize,   // exclusive
    }

    // Occupancy grid: tracks which cells are taken. Grows dynamically.
    let mut max_row: usize = 0;
    let mut occupied: Vec<Vec<bool>> = Vec::new(); // occupied[row][col]

    fn ensure_rows(occupied: &mut Vec<Vec<bool>>, num_rows: usize, num_cols: usize) {
        while occupied.len() < num_rows {
            occupied.push(vec![false; num_cols]);
        }
    }

    fn mark_occupied(occupied: &mut Vec<Vec<bool>>, p: &CellPlacement, num_cols: usize) {
        ensure_rows(occupied, p.row_end, num_cols);
        for row in occupied.iter_mut().take(p.row_end).skip(p.row_start) {
            for cell in row
                .iter_mut()
                .take(p.col_end.min(num_cols))
                .skip(p.col_start)
            {
                *cell = true;
            }
        }
    }

    fn find_next_free_row(
        occupied: &mut Vec<Vec<bool>>,
        col_span: usize,
        row_span: usize,
        num_cols: usize,
        auto_row: &mut usize,
        auto_col: &mut usize,
    ) -> (usize, usize) {
        // Safety limit to prevent infinite loops
        let mut iterations = 0;
        const MAX_ITERATIONS: usize = 10_000;
        loop {
            iterations += 1;
            if iterations > MAX_ITERATIONS {
                // Return a safe fallback position
                return (0, *auto_row);
            }
            ensure_rows(occupied, *auto_row + row_span, num_cols);
            if *auto_col + col_span <= num_cols {
                let fits = (0..row_span)
                    .all(|dr| (0..col_span).all(|dc| !occupied[*auto_row + dr][*auto_col + dc]));
                if fits {
                    let result = (*auto_col, *auto_row);
                    *auto_col += col_span;
                    if *auto_col >= num_cols {
                        *auto_col = 0;
                        *auto_row += 1;
                    }
                    return result;
                }
            }
            *auto_col += 1;
            if *auto_col >= num_cols {
                *auto_col = 0;
                *auto_row += 1;
            }
        }
    }

    fn find_next_free_column(
        occupied: &mut Vec<Vec<bool>>,
        col_span: usize,
        row_span: usize,
        num_cols: usize,
        num_explicit_rows: usize,
        auto_row: &mut usize,
        auto_col: &mut usize,
    ) -> (usize, usize) {
        // Column-based auto-flow: fill columns first
        // Safety limit to prevent infinite loops
        let mut iterations = 0;
        const MAX_ITERATIONS: usize = 10_000;
        loop {
            iterations += 1;
            if iterations > MAX_ITERATIONS {
                return (*auto_col, *auto_row);
            }
            ensure_rows(occupied, *auto_row + row_span, num_cols);

            // Determine the row limit for the current column
            // If we're in an explicit column, limit to explicit rows
            // If we're in an implicit column, allow unlimited rows
            let row_limit = if *auto_col + col_span <= num_cols && num_explicit_rows > 0 {
                num_explicit_rows
            } else {
                usize::MAX // No limit for implicit columns
            };

            // Check if we've exceeded the row limit for this column
            if *auto_row >= row_limit || *auto_row + row_span > row_limit {
                // Move to next column
                *auto_row = 0;
                *auto_col += col_span;
                continue;
            }

            // Check if current position fits
            // For explicit columns (within num_cols), check if occupied
            // For implicit columns (beyond num_cols), always place there
            let fits = if *auto_col + col_span <= num_cols {
                // Within explicit grid - check if occupied
                *auto_row + row_span <= occupied.len()
                    && (0..row_span)
                        .all(|dr| (0..col_span).all(|dc| !occupied[*auto_row + dr][*auto_col + dc]))
            } else {
                // Implicit column - always fits
                true
            };

            if fits {
                let result = (*auto_col, *auto_row);
                *auto_row += row_span;
                return result;
            }

            // Try next row
            *auto_row += 1;
        }
    }

    // Resolve line number: CSS uses 1-indexed, negative counts from end
    let resolve_line = |line: i32, max_line: usize| -> usize {
        if line > 0 {
            (line as usize).saturating_sub(1) // 1-indexed to 0-indexed
        } else if line < 0 {
            let total = max_line + 1; // number of grid lines = tracks + 1
            total.saturating_sub((-line) as usize)
        } else {
            0
        }
    };

    let mut placements: Vec<CellPlacement> = Vec::with_capacity(num_children);
    let mut auto_row: usize = 0;
    let mut auto_col: usize = 0;

    // Build area lookup from grid-template-areas
    // area_name -> (row_start, col_start, row_end, col_end) in 0-indexed grid coordinates
    let area_lookup: std::collections::HashMap<String, (usize, usize, usize, usize)> =
        if !style.grid_template_areas.is_empty() {
            let mut map = std::collections::HashMap::new();
            for (row_idx, row) in style.grid_template_areas.iter().enumerate() {
                for (col_idx, area_name) in row.iter().enumerate() {
                    if area_name == "." {
                        continue;
                    }
                    let entry = map.entry(area_name.clone()).or_insert((
                        row_idx,
                        col_idx,
                        row_idx + 1,
                        col_idx + 1,
                    ));
                    // Expand to cover all cells this area name occupies
                    entry.0 = entry.0.min(row_idx);
                    entry.1 = entry.1.min(col_idx);
                    entry.2 = entry.2.max(row_idx + 1);
                    entry.3 = entry.3.max(col_idx + 1);
                }
            }
            map
        } else {
            std::collections::HashMap::new()
        };

    for child in layout_box.children.iter() {
        // Skip whitespace-only text nodes - they shouldn't be grid items
        if child.box_type == BoxType::Text {
            if let Some(ref text) = child.text {
                if text.trim().is_empty() {
                    continue;
                }
            }
        }
        let cs = styles.get(&child.node_id).cloned().unwrap_or_default();

        // Check grid-area first (named area)
        if let Some(ref area_name) = cs.grid_area {
            if let Some(&(r0, c0, r1, c1)) = area_lookup.get(area_name.as_str()) {
                let p = CellPlacement {
                    col_start: c0,
                    col_end: c1,
                    row_start: r0,
                    row_end: r1,
                };
                mark_occupied(&mut occupied, &p, num_cols);
                max_row = max_row.max(p.row_end);
                placements.push(p);
                continue;
            }
        }

        let has_col = cs.grid_column_start.is_some() || cs.grid_column_end.is_some();
        let has_row = cs.grid_row_start.is_some() || cs.grid_row_end.is_some();

        let (col_start, col_end, row_start, row_end) = if has_col || has_row {
            let c0 = cs
                .grid_column_start
                .map(|v| resolve_line(v, num_cols))
                .unwrap_or(0);
            let c1 = cs
                .grid_column_end
                .map(|v| resolve_line(v, num_cols))
                .unwrap_or_else(|| (c0 + 1).min(num_cols));
            let r0 = cs
                .grid_row_start
                .map(|v| resolve_line(v, 100))
                .unwrap_or(auto_row);
            let r1 = cs
                .grid_row_end
                .map(|v| resolve_line(v, 100))
                .unwrap_or(r0 + 1);
            (
                c0.min(num_cols.saturating_sub(1)),
                c1.min(num_cols),
                r0,
                r1.max(r0 + 1),
            )
        } else {
            // Auto-placement based on grid-auto-flow
            let is_column_flow = matches!(
                style.grid_auto_flow,
                incognidium_style::GridAutoFlow::Column
            );
            let num_explicit_rows = style.grid_template_rows.len();
            let (c, r) = if is_column_flow {
                find_next_free_column(
                    &mut occupied,
                    1,
                    1,
                    num_cols,
                    num_explicit_rows,
                    &mut auto_row,
                    &mut auto_col,
                )
            } else {
                find_next_free_row(&mut occupied, 1, 1, num_cols, &mut auto_row, &mut auto_col)
            };
            (c, c + 1, r, r + 1)
        };

        let p = CellPlacement {
            col_start,
            col_end,
            row_start,
            row_end,
        };
        mark_occupied(&mut occupied, &p, num_cols);
        max_row = max_row.max(p.row_end);
        placements.push(p);
    }

    let num_rows = max_row.max(1);

    // Get auto-row size for implicit rows
    let auto_row_size = style
        .grid_auto_rows
        .first()
        .map(|t| match t {
            incognidium_style::GridTrackSize::Px(px) => *px,
            incognidium_style::GridTrackSize::Percent(p) => content_width * p / 100.0,
            _ => 0.0,
        })
        .unwrap_or(0.0);

    // First pass: compute natural heights per row
    let mut row_heights = vec![0.0_f32; num_rows];
    let mut placement_iter = placements.iter();
    for child in layout_box.children.iter_mut() {
        // Skip whitespace-only text nodes (must match first pass)
        if child.box_type == BoxType::Text {
            if let Some(ref text) = child.text {
                if text.trim().is_empty() {
                    continue;
                }
            }
        }
        let p = match placement_iter.next() {
            Some(p) => p,
            None => break, // Should not happen if counts match
        };
        // Cell width spans multiple columns
        let cell_width: f32 = (p.col_start..p.col_end)
            .map(|c| get_col_width(c))
            .sum::<f32>()
            + (p.col_end - p.col_start).saturating_sub(1) as f32 * col_gap;

        compute_layout(child, styles, cell_width, 0.0, image_sizes);

        let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
        let child_h = child.height + child_style.margin_top + child_style.margin_bottom;
        // Distribute height across spanned rows (attribute to first row for simplicity)
        let row_span = p.row_end - p.row_start;
        let per_row_h = child_h / row_span as f32;
        for row_height in row_heights
            .iter_mut()
            .take(p.row_end.min(num_rows))
            .skip(p.row_start)
        {
            *row_height = row_height.max(per_row_h);
        }
    }

    // Apply auto-row size to implicit rows (rows beyond explicit grid)
    let explicit_row_count = style.grid_template_rows.len();
    for (r, row_height) in row_heights.iter_mut().enumerate() {
        if r >= explicit_row_count && auto_row_size > 0.0 {
            // For implicit rows, use auto-row size if larger than content
            *row_height = row_height.max(auto_row_size);
        }
    }

    // Override with explicit row track sizes
    for (r, rh) in row_heights.iter_mut().enumerate() {
        if r < explicit_row_tracks.len() {
            match explicit_row_tracks[r] {
                GridTrackSize::Px(px) => *rh = px,
                GridTrackSize::Percent(p) => *rh = content_width * p / 100.0,
                GridTrackSize::Auto => {}
                GridTrackSize::Fr(_) => {}
                GridTrackSize::MinMax(min, _) => {
                    if *rh < min {
                        *rh = min;
                    }
                }
            }
        }
    }

    // Second pass: position each child
    let mut placement_iter2 = placements.iter();
    for child in layout_box.children.iter_mut() {
        // Skip whitespace-only text nodes (must match first pass)
        if child.box_type == BoxType::Text {
            if let Some(ref text) = child.text {
                if text.trim().is_empty() {
                    continue;
                }
            }
        }
        let p = match placement_iter2.next() {
            Some(p) => p,
            None => break,
        };

        let cell_x: f32 =
            (0..p.col_start).map(|c| get_col_width(c)).sum::<f32>() + p.col_start as f32 * col_gap;
        let cell_y: f32 =
            (0..p.row_start).map(|r| row_heights[r]).sum::<f32>() + p.row_start as f32 * row_gap;
        let cell_width: f32 = (p.col_start..p.col_end)
            .map(|c| get_col_width(c))
            .sum::<f32>()
            + (p.col_end - p.col_start).saturating_sub(1) as f32 * col_gap;
        let cell_height: f32 = (p.row_start..p.row_end)
            .map(|r| row_heights[r])
            .sum::<f32>()
            + (p.row_end - p.row_start).saturating_sub(1) as f32 * row_gap;

        let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();

        // Calculate item position within cell based on place-items (align-items, justify-items)
        let align = style.place_items.0;
        let justify = style.place_items.1;

        // Apply justify-items (horizontal alignment within cell)
        let item_width = child.width - child_style.margin_left - child_style.margin_right;
        let x_offset = match justify {
            JustifyItems::Center => (cell_width - item_width) / 2.0,
            JustifyItems::FlexEnd => cell_width - item_width - child_style.margin_right,
            JustifyItems::Stretch => {
                // Stretch to fill cell width
                let new_width = cell_width - child_style.margin_left - child_style.margin_right;
                if new_width > child.width {
                    child.width = new_width;
                    child.content_width = child.width
                        - child_style.padding_left
                        - child_style.padding_right
                        - child_style.border_left_width
                        - child_style.border_right_width;
                }
                0.0
            }
            _ => 0.0, // FlexStart/Auto default to start
        };

        // Apply align-items (vertical alignment within cell)
        let item_height = child.height - child_style.margin_top - child_style.margin_bottom;
        let y_offset = match align {
            AlignItems::Center => (cell_height - item_height) / 2.0,
            AlignItems::FlexEnd => cell_height - item_height - child_style.margin_bottom,
            AlignItems::Stretch => {
                // Stretch to fill cell height
                let new_height = cell_height - child_style.margin_top - child_style.margin_bottom;
                if new_height > child.height {
                    child.height = new_height;
                }
                0.0
            }
            _ => 0.0, // FlexStart/Baseline default to start
        };

        child.x = content_x + cell_x + child_style.margin_left + x_offset;
        child.y = content_y + cell_y + child_style.margin_top + y_offset;

        // Ensure width fills cell for stretch
        if justify == JustifyItems::Stretch && child.width < cell_width {
            child.width = cell_width - child_style.margin_left - child_style.margin_right;
            child.content_width = child.width
                - child_style.padding_left
                - child_style.padding_right
                - child_style.border_left_width
                - child_style.border_right_width;
        }
    }

    // Compute total height
    let total_row_height: f32 = row_heights.iter().sum();
    let total_gap_height = row_gap * (num_rows.saturating_sub(1)) as f32;
    let content_height = total_row_height + total_gap_height;

    // Apply explicit height if set
    let content_height = match style.height {
        SizeValue::Px(h) => h,
        _ => content_height,
    };
    let content_height = match style.min_height {
        SizeValue::Px(mh) if content_height < mh => mh,
        _ => content_height,
    };

    // SAFETY CAP: Prevent extreme grid container heights
    let content_height = content_height.min(100_000.0);

    layout_box.content_width = content_width;
    layout_box.width = content_width + padding_left + padding_right + border_left + border_right;
    layout_box.content_height = content_height.max(0.0);
    layout_box.height = content_height + padding_top + padding_bottom + border_top + border_bottom;
}

/// Layout multi-column content
fn layout_columns(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    image_sizes: &ImageSizes,
    parent_floats: FloatState,
) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    let padding_left = style.padding_left;
    let padding_right = style.padding_right;
    let padding_top = style.padding_top;
    let padding_bottom = style.padding_bottom;
    let border_left = style.border_left_width;
    let border_right = style.border_right_width;
    let border_top = style.border_top_width;
    let border_bottom = style.border_bottom_width;

    // Calculate content width
    let is_border_box = style.box_sizing == incognidium_style::BoxSizing::BorderBox;
    let content_width = match style.width {
        SizeValue::Px(w) => {
            if is_border_box {
                (w - padding_left - padding_right - border_left - border_right).max(0.0)
            } else {
                w
            }
        }
        SizeValue::Percent(p) => {
            let total = containing_width * p / 100.0;
            if is_border_box {
                (total - padding_left - padding_right - border_left - border_right).max(0.0)
            } else {
                total
            }
        }
        SizeValue::Auto | SizeValue::None => (containing_width
            - style.margin_left
            - style.margin_right
            - padding_left
            - padding_right
            - border_left
            - border_right)
        .max(0.0),
        _ => containing_width,
    };

    // Determine number of columns
    let column_gap = style.column_gap.max(0.0);
    let num_columns: usize = if let Some(count) = style.column_count {
        if count > 0 {
            count as usize
        } else {
            1
        }
    } else if let Some(width) = style.column_width {
        // Calculate columns based on column-width
        let available = (content_width + column_gap) as usize;
        let col_w = (width + column_gap) as usize;
        if col_w > 0 {
            (available / col_w).max(1)
        } else {
            1
        }
    } else {
        1
    };

    // Calculate column width
    let total_gap = column_gap * (num_columns.saturating_sub(1) as f32);
    let column_width = ((content_width - total_gap) / num_columns as f32).max(0.0);

    // First pass: layout all children as if in one column
    let content_x = padding_left + border_left;
    let content_y = padding_top + border_top;

    // Temporarily layout children to get their natural heights
    let mut cursor_y: f32 = 0.0;
    let mut prev_margin_bottom: f32 = 0.0;

    for child in layout_box.children.iter_mut() {
        // Apply margin collapse between block children
        let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
        let margin_top = child_style.margin_top;
        let margin_bottom = child_style.margin_bottom;

        let vertical_margin = if prev_margin_bottom > 0.0 {
            margin_top.max(prev_margin_bottom) - margin_top.min(prev_margin_bottom)
        } else {
            margin_top
        };

        cursor_y += vertical_margin;
        prev_margin_bottom = margin_bottom;

        // Layout child
        compute_layout(child, styles, column_width, 0.0, image_sizes);

        // Position child temporarily
        child.x = content_x;
        child.y = content_y + cursor_y;

        cursor_y += child.height;
    }

    // Calculate total content height
    let total_content_height = cursor_y;

    // Calculate column height (balance content across columns)
    let column_height = if num_columns > 0 {
        (total_content_height / num_columns as f32).ceil()
    } else {
        total_content_height
    };

    // Second pass: distribute children into columns
    let mut current_column: usize = 0;
    let mut current_column_height: f32 = 0.0;
    let mut column_start_y: f32 = content_y;

    for child in layout_box.children.iter_mut() {
        let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
        let margin_top = child_style.margin_top;
        let child_total_height = child.height + margin_top;

        // Check if we need to move to next column
        if current_column_height + child_total_height > column_height
            && current_column + 1 < num_columns
            && current_column_height > 0.0
        {
            current_column += 1;
            current_column_height = 0.0;
            column_start_y = content_y;
        }

        // Position in column
        child.x = content_x + current_column as f32 * (column_width + column_gap);
        child.y = column_start_y + current_column_height + margin_top;

        current_column_height += child_total_height;
    }

    // Calculate final container height
    let final_height = if style.height == SizeValue::Auto {
        column_height
    } else {
        match style.height {
            SizeValue::Px(h) => h,
            _ => column_height,
        }
    };

    layout_box.content_width = content_width;
    layout_box.width = content_width + padding_left + padding_right + border_left + border_right;
    layout_box.content_height = final_height;
    layout_box.height = final_height + padding_top + padding_bottom + border_top + border_bottom;

    // Store column info for column-rule rendering
    layout_box.column_count = num_columns;
    layout_box.column_width = column_width;
    layout_box.column_gap = column_gap;
    layout_box.column_rule_width = style.column_rule_width;
    layout_box.column_rule_style = style.column_rule_style;
    layout_box.column_rule_color = style.column_rule_color;
}

/// Resolve grid track sizes to actual pixel widths given the available space.
fn resolve_track_sizes(tracks: &[GridTrackSize], available: f32, gap: f32) -> Vec<f32> {
    let n = tracks.len();
    if n == 0 {
        return vec![available];
    }

    let total_gap = gap * (n.saturating_sub(1)) as f32;
    let space = (available - total_gap).max(0.0);

    // First pass: resolve fixed sizes and collect fr totals
    let mut widths = vec![0.0_f32; n];
    let mut total_fr = 0.0_f32;
    let mut fixed_used = 0.0_f32;

    for (i, track) in tracks.iter().enumerate() {
        match track {
            GridTrackSize::Px(px) => {
                widths[i] = *px;
                fixed_used += *px;
            }
            GridTrackSize::Percent(p) => {
                let w = space * *p / 100.0;
                widths[i] = w;
                fixed_used += w;
            }
            GridTrackSize::Fr(fr) => {
                total_fr += *fr;
            }
            GridTrackSize::Auto => {
                // Auto tracks get treated like 1fr if there are no fr tracks,
                // otherwise they get a minimum share
                total_fr += 1.0;
            }
            GridTrackSize::MinMax(min, max_fr) => {
                widths[i] = *min;
                fixed_used += *min;
                total_fr += *max_fr;
            }
        }
    }

    // Second pass: distribute remaining space among fr tracks
    let fr_space = (space - fixed_used).max(0.0);
    if total_fr > 0.0 {
        for (i, track) in tracks.iter().enumerate() {
            match track {
                GridTrackSize::Fr(fr) => {
                    widths[i] = fr_space * (*fr / total_fr);
                }
                GridTrackSize::Auto => {
                    widths[i] = fr_space * (1.0 / total_fr);
                }
                GridTrackSize::MinMax(min, max_fr) => {
                    let extra = fr_space * (*max_fr / total_fr);
                    widths[i] = (*min).max(extra);
                }
                _ => {}
            }
        }
    }

    widths
}

/// Expand tab characters to spaces based on tab-size
fn expand_tabs(text: &str, tab_size: i32) -> String {
    if tab_size <= 0 {
        return text.replace('\t', " ");
    }
    let tab_size = tab_size as usize;
    let mut result = String::with_capacity(text.len());
    let mut col = 0;
    for ch in text.chars() {
        if ch == '\t' {
            let spaces = tab_size - (col % tab_size);
            for _ in 0..spaces {
                result.push(' ');
            }
            col += spaces;
        } else {
            result.push(ch);
            col += 1;
        }
    }
    result
}

/// Process soft hyphens (&shy; or U+00AD) based on hyphens property
/// Returns the processed text with soft hyphens either removed (hyphens: none)
/// or kept (hyphens: manual/auto) for breaking
fn process_soft_hyphens(text: &str, hyphens: &incognidium_style::Hyphens) -> String {
    use incognidium_style::Hyphens;

    match hyphens {
        Hyphens::None => {
            // Remove all soft hyphens
            text.replace('\u{00AD}', "")
        }
        Hyphens::Manual | Hyphens::Auto => {
            // Keep soft hyphens - they indicate valid break points
            // In manual mode, we only break at explicit hyphens
            // In auto mode, browser may also break at other points
            // For now, we keep the text as-is with soft hyphens preserved
            text.to_string()
        }
    }
}

fn layout_text(layout_box: &mut LayoutBox, styles: &StyleMap, containing_width: f32) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();
    let text = layout_box.text.clone().unwrap_or_default();

    // Expand tabs to spaces based on tab-size property
    let text = expand_tabs(&text, style.tab_size);

    // Process soft hyphens based on hyphens property
    let text = process_soft_hyphens(&text, &style.hyphens);

    if text.is_empty() {
        layout_box.width = 0.0;
        layout_box.height = 0.0;
        return;
    }

    let line_height = style.font_size * style.line_height;
    let space_width = measure_text_width(" ", style.font_size, &style) + style.word_spacing;

    if text == " " {
        layout_box.content_width = 0.0;
        layout_box.content_height = 0.0;
        layout_box.width = 0.0;
        layout_box.height = 0.0;
        return;
    }

    // Determine text wrapping behavior from text-wrap property (CSS Text Level 4)
    // text-wrap: nowrap overrides normal wrapping
    let text_wrap_nowrap = matches!(style.text_wrap, TextWrap::NoWrap);

    // Check if breaking is allowed based on CSS properties
    // white-space property or text-wrap: nowrap can prevent wrapping
    let nowrap = matches!(
        style.white_space,
        incognidium_style::WhiteSpace::NoWrap | incognidium_style::WhiteSpace::Pre
    ) || text_wrap_nowrap
        || containing_width <= 0.0;

    // Determine white-space collapsing behavior from white-space-collapse property
    // This is the CSS Text Level 4 way to control whitespace handling
    let collapse_spaces = matches!(style.white_space_collapse, WhiteSpaceCollapse::Collapse);
    let preserve_spaces = matches!(style.white_space_collapse, WhiteSpaceCollapse::Preserve);
    let preserve_breaks_only = matches!(style.white_space_collapse, WhiteSpaceCollapse::PreserveBreaks);
    let break_spaces = matches!(style.white_space_collapse, WhiteSpaceCollapse::BreakSpaces);

    // Check if newlines should be preserved (legacy white-space property)
    let preserve_newlines_legacy = matches!(
        style.white_space,
        incognidium_style::WhiteSpace::Pre
            | incognidium_style::WhiteSpace::PreWrap
            | incognidium_style::WhiteSpace::PreLine
    );

    // Combine legacy and new property behavior
    let preserve_newlines = preserve_newlines_legacy || preserve_spaces || preserve_breaks_only || break_spaces;

    // Check if this is pre-wrap (preserves newlines AND wraps words)
    let is_pre_wrap_legacy =
        matches!(style.white_space, incognidium_style::WhiteSpace::PreWrap);
    // CSS Text Level 4: white-space-collapse: preserve with text-wrap: wrap behaves like pre-wrap
    let is_pre_wrap = is_pre_wrap_legacy || (preserve_spaces && !text_wrap_nowrap);
    // break-spaces also behaves like pre-wrap for layout purposes
    let is_break_spaces = break_spaces && !text_wrap_nowrap;

    // Handle pre-wrap specially: split by lines first, then wrap each line
    if is_pre_wrap || is_break_spaces {
        layout_text_pre_wrap(
            layout_box,
            &text,
            containing_width,
            line_height,
            space_width,
            &style,
        );
        return;
    }

    // Process text based on white-space-collapse setting
    // For nowrap, treat the entire text as a single word (preserve internal whitespace)
    // But split on newlines if they should be preserved
    // Note: We use Vec<String> to handle cases where we need owned strings
    let words: Vec<String> = if nowrap {
        if preserve_newlines {
            // Split on newlines but keep each line as a word
            text.split('\n').map(|s| s.to_string()).collect()
        } else {
            vec![text.clone()]
        }
    } else if preserve_spaces {
        // white-space-collapse: preserve - split on newlines only
        text.split('\n').map(|s| s.to_string()).collect()
    } else if preserve_breaks_only {
        // white-space-collapse: preserve-breaks - collapse spaces, keep newlines
        // First normalize spaces on each line, then collect non-empty lines
        text.split('\n')
            .filter_map(|line| {
                let normalized = line.split_whitespace().collect::<Vec<_>>().join(" ");
                if normalized.is_empty() {
                    None
                } else {
                    Some(normalized)
                }
            })
            .collect()
    } else {
        // Default: collapse all whitespace
        text.split_whitespace().map(|s| s.to_string()).collect()
    };

    if words.is_empty() {
        layout_box.width = 0.0;
        layout_box.height = 0.0;
        layout_box.content_width = 0.0;
        layout_box.content_height = 0.0;
        return;
    }

    let mut lines = 1u32;
    let mut current_line_width: f32 = 0.0;
    let mut max_line_width: f32 = 0.0;
    let mut broken_text_parts: Vec<String> = Vec::new();

    // For text-align: justify, track line info: (space_indices, word_count, line_width)
    // space_indices stores the indices in broken_text_parts where spaces occur
    let mut line_info: Vec<(Vec<usize>, usize, f32)> = Vec::new();
    let mut current_line_space_indices: Vec<usize> = Vec::new();
    let mut current_line_word_count: usize = 0;
    let mut current_line_start_idx: usize = 0;

    // Check if breaking is allowed based on CSS properties
    let can_break_word = matches!(
        style.word_break,
        incognidium_style::WordBreak::BreakAll | incognidium_style::WordBreak::BreakWord
    ) || matches!(
        style.overflow_wrap,
        incognidium_style::OverflowWrap::BreakWord | incognidium_style::OverflowWrap::Anywhere
    );

    for (i, word) in words.iter().enumerate() {
        let word_width = measure_text_width(word, style.font_size, &style);
        let needed = if i == 0 {
            word_width
        } else {
            space_width + word_width
        };

        // First, check if this word is wider than the container and needs breaking
        if !nowrap && word_width > containing_width + 0.5 && can_break_word {
            // Word is too long for container, break it into pieces
            let mut remaining: &str = word;
            let mut first_piece = true;
            while !remaining.is_empty() {
                let mut fit_len = 0usize;
                let mut piece_width = 0.0f32;
                let start_width = if first_piece && i > 0 {
                    current_line_width + space_width
                } else {
                    current_line_width
                };

                for (idx, ch) in remaining.char_indices() {
                    let ch_width =
                        measure_text_width(
                            &remaining[..idx + ch.len_utf8()],
                            style.font_size,
                            &style,
                        ) - measure_text_width(&remaining[..idx], style.font_size, &style);
                    if start_width + piece_width + ch_width > containing_width + 0.5
                        && piece_width > 0.0
                    {
                        break;
                    }
                    fit_len = idx + ch.len_utf8();
                    piece_width += ch_width;
                }

                if fit_len == 0 {
                    fit_len = remaining.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                    piece_width =
                        measure_text_width(&remaining[..fit_len], style.font_size, &style);
                }

                let piece = &remaining[..fit_len];

                if first_piece {
                    first_piece = false;
                    if i > 0 {
                        broken_text_parts.push(" ".to_string());
                        broken_text_parts.push(piece.to_string());
                        current_line_width += space_width + piece_width;
                    } else {
                        broken_text_parts.push(piece.to_string());
                        current_line_width += piece_width;
                    }
                } else {
                    broken_text_parts.push("\n".to_string());
                    broken_text_parts.push(piece.to_string());
                    max_line_width = max_line_width.max(current_line_width);
                    lines += 1;
                    current_line_width = piece_width;
                }

                remaining = &remaining[fit_len..];
            }
            continue;
        }

        // Check if we need to wrap to next line
        if !nowrap
            && current_line_width + needed > containing_width + 0.5
            && current_line_width > 0.0
        {
            // Normal wrap - record line info for justify
            if style.text_align == TextAlign::Justify {
                line_info.push((current_line_space_indices.clone(), current_line_word_count, current_line_width));
            }
            current_line_space_indices.clear();
            // Normal wrap
            broken_text_parts.push("\n".to_string());
            if i > 0 {
                broken_text_parts.push(word.to_string());
            }
            max_line_width = max_line_width.max(current_line_width);
            lines += 1;
            current_line_width = word_width;
            current_line_word_count = 1;
            current_line_start_idx = broken_text_parts.len();
        } else {
            // Check if we should add a separator before this word
            if i > 0 {
                if preserve_newlines {
                    // For pre/pre-wrap/pre-line: insert newline between lines
                    broken_text_parts.push("\n".to_string());
                    lines += 1;
                } else {
                    // Normal text: add space between words
                    // Track this space index for justify
                    if style.text_align == TextAlign::Justify {
                        current_line_space_indices.push(broken_text_parts.len());
                    }
                    broken_text_parts.push(" ".to_string());
                }
            }
            broken_text_parts.push(word.to_string());
            current_line_width += needed;
            current_line_word_count += 1;
        }
    }

    // Handle text-align: justify by adding extra spaces
    // text-justify controls the justification method:
    // - auto: default behavior (inter-word for most scripts)
    // - none: disable justification
    // - inter-word: expand spaces between words
    // - inter-character: expand between characters (for CJK)
    let should_justify = style.text_align == TextAlign::Justify
        && !matches!(style.text_justify, incognidium_style::TextJustify::None)
        && !line_info.is_empty();

    if should_justify {
        // Add last line (don't justify the last line)
        line_info.push((current_line_space_indices, current_line_word_count, current_line_width));

        // Determine justification method
        let inter_character = matches!(style.text_justify, TextJustify::InterCharacter);

        // Process each line (except the last) to add extra spaces
        for line_idx in 0..line_info.len() - 1 {
            let (space_indices, word_count, line_width) = &line_info[line_idx];
            if *word_count <= 1 && !inter_character {
                continue; // Can't justify single word with inter-word
            }
            let extra_space = containing_width - line_width;
            if extra_space <= 0.0 {
                continue; // Line is full or overflowed
            }

            if inter_character {
                // inter-character justification: add extra letter-spacing
                // This is used for CJK text where word boundaries aren't clear
                // For now, we add the extra space as trailing letter-spacing
                // A full implementation would distribute space between every character
                let total_chars: usize = broken_text_parts
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !space_indices.contains(i))
                    .map(|(_, part)| part.chars().count())
                    .sum();
                if total_chars > 1 {
                    let extra_per_char = extra_space / (total_chars - 1) as f32;
                    // Store the extra letter spacing for this line
                    // We can't easily modify existing text, so we add trailing spaces
                    // to simulate the effect
                    let num_trailing_spaces = (extra_space / space_width).round() as usize;
                    if let Some(last_idx) = space_indices.last() {
                        if *last_idx < broken_text_parts.len() {
                            broken_text_parts[*last_idx] = " ".repeat(1 + num_trailing_spaces);
                        }
                    }
                }
            } else {
                // inter-word justification (default): expand spaces between words
                let gaps = space_indices.len();
                if gaps == 0 {
                    continue; // No spaces to expand
                }
                let extra_per_gap = extra_space / gaps as f32;
                // Calculate how many spaces needed to fill the gap
                let num_extra_spaces = ((extra_per_gap / space_width).round() as usize).max(1);

                // Add extra spaces at each space position
                for &space_idx in space_indices {
                    if space_idx < broken_text_parts.len()
                        && broken_text_parts[space_idx] == " "
                    {
                        // Replace single space with multiple spaces
                        broken_text_parts[space_idx] = " ".repeat(1 + num_extra_spaces);
                    }
                }
            }
        }
    }

    // Handle text-wrap: balance - try to balance line lengths for better typography
    // This is a simplified implementation that redistributes words to minimize
    // the variance in line lengths
    if matches!(style.text_wrap, TextWrap::Balance) && lines > 1 {
        // For balance, we'd need to recompute the layout with a different algorithm
        // A simple approach: if the last line is significantly shorter, try to
        // redistribute words from the previous line
        // This is a placeholder for the full balancing algorithm
        // Full implementation would require re-laying out all words with a
        // dynamic programming approach to minimize raggedness
    }

    // Handle line-clamp: truncate text if it exceeds the specified number of lines
    let final_text = if let Some(max_lines) = style.line_clamp {
        if lines > max_lines as u32 {
            // Find where to truncate - we need to find the position after max_lines newlines
            let mut line_count = 0u32;
            let mut truncate_idx = 0usize;
            for (idx, part) in broken_text_parts.iter().enumerate() {
                if part == "\n" {
                    line_count += 1;
                    if line_count >= max_lines as u32 {
                        truncate_idx = idx;
                        break;
                    }
                }
            }
            // Truncate and add ellipsis
            let truncated: Vec<String> = broken_text_parts[..truncate_idx].to_vec();
            let mut result = truncated.join("");
            // Add ellipsis, removing any trailing partial word if necessary
            result.push_str("...");
            result
        } else {
            broken_text_parts.join("")
        }
    } else {
        broken_text_parts.join("")
    };
    layout_box.text = Some(final_text);

    // Calculate final dimensions
    let natural_width = max_line_width.max(current_line_width);
    // The content width is the natural text width (for measurement purposes)
    layout_box.content_width = natural_width;
    // Apply line-clamp to height if specified
    let clamped_lines = style.line_clamp.map(|max| (lines as i32).min(max)).unwrap_or(lines as i32);
    layout_box.content_height = clamped_lines as f32 * line_height;
    // The box width should always be constrained to containing_width
    // even with nowrap - text-overflow: ellipsis depends on this
    layout_box.width = natural_width.min(containing_width);
    layout_box.height = clamped_lines as f32 * line_height;
}

/// Layout text with white-space: pre-wrap behavior.
/// Preserves explicit newlines from source text, but also wraps long lines.
fn layout_text_pre_wrap(
    layout_box: &mut LayoutBox,
    text: &str,
    containing_width: f32,
    line_height: f32,
    space_width: f32,
    style: &incognidium_style::ComputedStyle,
) {
    // Split text by explicit newlines first - each segment is a "source line"
    let source_lines: Vec<&str> = text.split('\n').collect();

    let mut total_lines = 0u32;
    let mut max_line_width: f32 = 0.0;
    let mut all_parts: Vec<String> = Vec::new();

    // Check if breaking is allowed based on CSS properties
    let can_break_word = matches!(
        style.word_break,
        incognidium_style::WordBreak::BreakAll | incognidium_style::WordBreak::BreakWord
    ) || matches!(
        style.overflow_wrap,
        incognidium_style::OverflowWrap::BreakWord | incognidium_style::OverflowWrap::Anywhere
    );

    for (line_idx, source_line) in source_lines.iter().enumerate() {
        // For pre-wrap, preserve leading whitespace (indentation) on each line
        // Check if this line has leading spaces
        let leading_spaces: String = source_line
            .chars()
            .take_while(|c| c.is_whitespace() && *c != '\n')
            .collect();

        // Split this source line into words (removes all whitespace)
        let words: Vec<&str> = source_line.split_whitespace().collect();

        if words.is_empty() {
            // Empty line - just add a newline (except for first line)
            if line_idx > 0 || !all_parts.is_empty() {
                all_parts.push("\n".to_string());
            }
            total_lines += 1;
            continue;
        }

        let mut current_line_width: f32 = 0.0;
        let mut first_word = true;

        // Add leading spaces before the first word to preserve indentation
        if !leading_spaces.is_empty() {
            all_parts.push(leading_spaces.clone());
            current_line_width += measure_text_width(&leading_spaces, style.font_size, style);
        }

        for (i, word) in words.iter().enumerate() {
            let word_width = measure_text_width(word, style.font_size, style);
            let space_w = if first_word { 0.0 } else { space_width };
            let needed = space_w + word_width;

            // Check if word needs to be broken (too long for container)
            if word_width > containing_width + 0.5 && can_break_word {
                // Need to break this word
                if !first_word {
                    all_parts.push(" ".to_string());
                    current_line_width += space_width;
                }

                let mut remaining = *word;
                let mut first_piece = true;

                while !remaining.is_empty() {
                    let mut fit_len = 0usize;
                    let mut piece_width = 0.0f32;

                    for (idx, ch) in remaining.char_indices() {
                        let ch_width = measure_text_width(
                            &remaining[..idx + ch.len_utf8()],
                            style.font_size,
                            style,
                        ) - measure_text_width(&remaining[..idx], style.font_size, style);
                        if current_line_width + piece_width + ch_width > containing_width + 0.5
                            && piece_width > 0.0
                        {
                            break;
                        }
                        fit_len = idx + ch.len_utf8();
                        piece_width += ch_width;
                    }

                    if fit_len == 0 {
                        fit_len = remaining.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                        piece_width =
                            measure_text_width(&remaining[..fit_len], style.font_size, style);
                    }

                    let piece = &remaining[..fit_len];

                    if !first_piece {
                        all_parts.push("\n".to_string());
                        max_line_width = max_line_width.max(current_line_width);
                        total_lines += 1;
                        current_line_width = 0.0;
                    }
                    first_piece = false;

                    all_parts.push(piece.to_string());
                    current_line_width += piece_width;
                    remaining = &remaining[fit_len..];
                }

                first_word = false;
                continue;
            }

            // Check if this word fits on current line
            if !first_word
                && current_line_width + needed > containing_width + 0.5
                && current_line_width > 0.0
            {
                // Word doesn't fit, wrap to next line
                all_parts.push("\n".to_string());
                max_line_width = max_line_width.max(current_line_width);
                total_lines += 1;
                current_line_width = 0.0;

                // Add the word to new line
                all_parts.push(word.to_string());
                current_line_width = word_width;
            } else {
                // Word fits (or is first word on line)
                if !first_word {
                    all_parts.push(" ".to_string());
                }
                all_parts.push(word.to_string());
                current_line_width += needed;
            }
            first_word = false;
        }

        max_line_width = max_line_width.max(current_line_width);
        total_lines += 1;

        // Add explicit newline between source lines (but not after last line)
        if line_idx < source_lines.len() - 1 {
            all_parts.push("\n".to_string());
        }
    }

    // Handle line-clamp for pre-wrap text
    let (final_text, clamped_lines) = if let Some(max_lines) = style.line_clamp {
        if total_lines > max_lines as u32 {
            // Find where to truncate
            let mut line_count = 0u32;
            let mut truncate_idx = 0usize;
            for (idx, part) in all_parts.iter().enumerate() {
                if part == "\n" {
                    line_count += 1;
                    if line_count >= max_lines as u32 {
                        truncate_idx = idx;
                        break;
                    }
                }
            }
            // Truncate and add ellipsis
            let truncated: Vec<String> = all_parts[..truncate_idx].to_vec();
            let mut result = truncated.join("");
            result.push_str("...");
            (result, max_lines as u32)
        } else {
            (all_parts.join(""), total_lines)
        }
    } else {
        (all_parts.join(""), total_lines)
    };
    layout_box.text = Some(final_text);

    // Calculate dimensions
    layout_box.content_width = max_line_width.min(containing_width);
    layout_box.content_height = clamped_lines as f32 * line_height;
    layout_box.width = layout_box.content_width;
    layout_box.height = layout_box.content_height;
}

/// Measure the rendered width of `text` at `font_size` using the same
/// font ab_glyph will paint with. Falls back to a rough approximation if
/// no TTF is installed.
pub fn measure_text_width(
    text: &str,
    font_size: f32,
    style: &incognidium_style::ComputedStyle,
) -> f32 {
    use ab_glyph::{Font, PxScale, ScaleFont};
    let char_count = text.chars().count() as f32;
    let letter_spacing = style.letter_spacing;

    if let Some(font) = get_layout_font(
        style.font_weight == incognidium_style::FontWeight::Bold,
        style.font_style == incognidium_style::FontStyle::Italic,
    ) {
        let scale = PxScale::from(font_size);
        let scaled = font.as_scaled(scale);
        let mut w = 0.0_f32;
        let mut prev = None;
        for ch in text.chars() {
            let gid = scaled.glyph_id(ch);
            if let Some(p) = prev {
                w += scaled.kern(p, gid);
            }
            w += scaled.h_advance(gid);
            // Add letter-spacing after each character
            w += letter_spacing;
            prev = Some(gid);
        }
        // Remove extra letter-spacing from the last character
        if char_count > 0.0 {
            w -= letter_spacing;
        }
        w
    } else {
        // No TTF: approximate with proportional-font average
        char_count * font_size * 0.52 + (char_count - 1.0).max(0.0) * letter_spacing
    }
}

static LAYOUT_FONTS: std::sync::OnceLock<Option<LayoutFonts>> = std::sync::OnceLock::new();

struct LayoutFonts {
    regular: ab_glyph::FontVec,
    bold: ab_glyph::FontVec,
    italic: ab_glyph::FontVec,
    bold_italic: ab_glyph::FontVec,
}

fn load_layout_fonts() -> Option<LayoutFonts> {
    use ab_glyph::FontVec;
    let dirs = [
        "/usr/share/fonts/truetype/liberation2",
        "/usr/share/fonts/truetype/liberation",
        "/usr/share/fonts/liberation-sans",
        "/usr/share/fonts/truetype/dejavu",
    ];
    let families: &[(&str, &str, &str, &str)] = &[
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
    for dir in &dirs {
        for (r, b, i, bi) in families {
            let rr = std::fs::read(format!("{dir}/{r}")).ok()?;
            let br = std::fs::read(format!("{dir}/{b}")).ok()?;
            let ir = std::fs::read(format!("{dir}/{i}")).ok()?;
            let bir = std::fs::read(format!("{dir}/{bi}")).ok()?;
            if let (Ok(rf), Ok(bf), Ok(ifv), Ok(bif)) = (
                FontVec::try_from_vec(rr),
                FontVec::try_from_vec(br),
                FontVec::try_from_vec(ir),
                FontVec::try_from_vec(bir),
            ) {
                return Some(LayoutFonts {
                    regular: rf,
                    bold: bf,
                    italic: ifv,
                    bold_italic: bif,
                });
            }
        }
    }
    None
}

fn get_layout_font(bold: bool, italic: bool) -> Option<&'static ab_glyph::FontVec> {
    let fonts = LAYOUT_FONTS.get_or_init(load_layout_fonts).as_ref()?;
    Some(match (bold, italic) {
        (true, true) => &fonts.bold_italic,
        (true, false) => &fonts.bold,
        (false, true) => &fonts.italic,
        (false, false) => &fonts.regular,
    })
}

fn layout_image(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    image_sizes: &ImageSizes,
) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    // Try to get actual image dimensions from the cache
    let actual_dims = layout_box
        .image_src
        .as_ref()
        .and_then(|src| image_sizes.get(src));

    let explicit_w = !matches!(style.width, SizeValue::Auto | SizeValue::None);
    let explicit_h = !matches!(style.height, SizeValue::Auto | SizeValue::None);

    // If no actual image AND no explicit dimensions, collapse to 0
    if actual_dims.is_none()
        && !explicit_w
        && !explicit_h
        && !layout_box
            .image_src
            .as_deref()
            .unwrap_or("")
            .starts_with("__canvas__")
    {
        layout_box.width = 0.0;
        layout_box.height = 0.0;
        layout_box.content_width = 0.0;
        layout_box.content_height = 0.0;
        return;
    }

    let w = match style.width {
        SizeValue::Px(w) => w,
        SizeValue::Percent(p) => containing_width * p / 100.0,
        _ => actual_dims.map(|(w, _)| *w as f32).unwrap_or(0.0),
    };
    let h = match style.height {
        SizeValue::Px(h) => h,
        SizeValue::Percent(p) => containing_width * p / 100.0,
        _ => {
            if let Some((iw, ih)) = actual_dims {
                if explicit_w && *iw > 0 {
                    w * (*ih as f32) / (*iw as f32)
                } else {
                    *ih as f32
                }
            } else {
                0.0
            }
        }
    };

    layout_box.width = w;
    layout_box.height = h;
    layout_box.content_width = w;
    layout_box.content_height = h;
}

/// Flatten the layout tree into a list of positioned boxes for painting.
/// Boxes are sorted by z-index (stable sort preserves document order within same z-index).
pub fn flatten_layout(
    layout_box: &LayoutBox,
    offset_x: f32,
    offset_y: f32,
    styles: &StyleMap,
) -> Vec<FlatBox> {
    let mut boxes = flatten_with_clip(layout_box, offset_x, offset_y, None, styles);
    boxes.sort_by_key(|fb| styles.get(&fb.node_id).map(|s| s.z_index).unwrap_or(0));
    boxes
}

fn flatten_with_clip(
    layout_box: &LayoutBox,
    offset_x: f32,
    offset_y: f32,
    parent_clip: Option<(f32, f32, f32, f32)>,
    styles: &StyleMap,
) -> Vec<FlatBox> {
    let mut result = Vec::new();
    let abs_x = offset_x + layout_box.x;
    let abs_y = offset_y + layout_box.y;

    // Determine clip rect: if this box has overflow:hidden, clip children to its bounds
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();
    let has_hidden_overflow = matches!(style.overflow, Overflow::Hidden | Overflow::Scroll)
        || matches!(style.overflow, Overflow::Auto);
    let own_bounds = (abs_x, abs_y, layout_box.width, layout_box.height);

    // The effective clip is the intersection of parent clip and own bounds (if overflow:hidden)
    let clip = if has_hidden_overflow {
        match parent_clip {
            Some((px, py, pw, ph)) => {
                // Intersect parent clip with own bounds
                let x1 = px.max(own_bounds.0);
                let y1 = py.max(own_bounds.1);
                let x2 = (px + pw).min(own_bounds.0 + own_bounds.2);
                let y2 = (py + ph).min(own_bounds.1 + own_bounds.3);
                if x2 > x1 && y2 > y1 {
                    Some((x1, y1, x2 - x1, y2 - y1))
                } else {
                    Some((0.0, 0.0, 0.0, 0.0)) // Empty clip = nothing visible
                }
            }
            None => Some(own_bounds),
        }
    } else {
        parent_clip
    };

    // Also clip to visibility:hidden elements' own bounds being 0
    // (they're skipped entirely in paint, but their clip shouldn't propagate)

    // Skip boxes entirely outside their clip rect
    if let Some((cx, cy, cw, ch)) = clip {
        if cw <= 0.0 || ch <= 0.0 {
            return result;
        }
        // Check if this box is entirely outside the clip
        if abs_x + layout_box.width <= cx
            || abs_y + layout_box.height <= cy
            || abs_x >= cx + cw
            || abs_y >= cy + ch
        {
            return result;
        }
    }

    // Skip zero-size text boxes or whitespace-only text nodes
    let is_empty_text = layout_box.box_type == BoxType::Text
        && (layout_box
            .text
            .as_deref()
            .map(|t| t.trim().is_empty())
            .unwrap_or(true)
            || (layout_box.width <= 0.01 && layout_box.height <= 0.01));

    if is_empty_text {
        // Don't add to result, but still process children (there shouldn't be any for text)
    } else if layout_box.box_type != BoxType::None {
        result.push(FlatBox {
            node_id: layout_box.node_id,
            x: abs_x,
            y: abs_y,
            width: layout_box.width,
            height: layout_box.height,
            box_type: layout_box.box_type,
            text: layout_box.text.clone(),
            image_src: layout_box.image_src.clone(),
            link_href: layout_box.link_href.clone(),
            clip,
            float_text_indent: layout_box.float_text_indent,
            input_type: layout_box.input_type,
            textarea_info: layout_box.textarea_info,
            marker_color: layout_box.marker_color,
            marker_font_size: layout_box.marker_font_size,
            marker_font_weight: layout_box.marker_font_weight,
            marker_font_family: layout_box.marker_font_family.clone(),
            marker_background_color: layout_box.marker_background_color,
            marker_letter_spacing: layout_box.marker_letter_spacing,
            marker_word_spacing: layout_box.marker_word_spacing,
            is_list_marker: layout_box.is_list_marker,
            list_style_position: layout_box.list_style_position,
            // ::first-letter fields
            first_letter_len: layout_box.first_letter_len,
            first_letter_color: layout_box.first_letter_color,
            first_letter_font_size: layout_box.first_letter_font_size,
            first_letter_font_weight: layout_box.first_letter_font_weight,
            first_letter_font_family: layout_box.first_letter_font_family.clone(),
            first_letter_background_color: layout_box.first_letter_background_color,
            first_letter_text_decoration: layout_box.first_letter_text_decoration,
            first_letter_margin: layout_box.first_letter_margin,
            first_letter_padding: layout_box.first_letter_padding,
            first_letter_border_width: layout_box.first_letter_border_width,
            first_letter_border_color: layout_box.first_letter_border_color,
            // ::first-line fields
            first_line_has_content: layout_box.first_line_has_content,
            first_line_color: layout_box.first_line_color,
            first_line_font_size: layout_box.first_line_font_size,
            first_line_font_weight: layout_box.first_line_font_weight,
            first_line_font_family: layout_box.first_line_font_family.clone(),
            first_line_background_color: layout_box.first_line_background_color,
            first_line_text_decoration: layout_box.first_line_text_decoration,
            first_line_letter_spacing: layout_box.first_line_letter_spacing,
            first_line_word_spacing: layout_box.first_line_word_spacing,
            first_line_text_transform: layout_box.first_line_text_transform,
            collapsed_borders: layout_box.collapsed_borders,
            hide_empty_cell: layout_box.hide_empty_cell,
            column_count: layout_box.column_count,
            column_width: layout_box.column_width,
            column_gap: layout_box.column_gap,
            column_rule_width: layout_box.column_rule_width,
            column_rule_style: layout_box.column_rule_style,
            column_rule_color: layout_box.column_rule_color,
            // For multi-column containers, content position is inside padding/border
            content_x: if layout_box.column_count > 0 {
                abs_x + (layout_box.width - layout_box.content_width) / 2.0
            } else {
                abs_x
            },
            content_y: if layout_box.column_count > 0 {
                abs_y + (layout_box.height - layout_box.content_height) / 2.0
            } else {
                abs_y
            },
            content_height: layout_box.content_height,
        });
    }

    // Propagate parent link_href to children
    let parent_href = layout_box.link_href.clone();
    for child in &layout_box.children {
        let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
        let child_offset = if child_style.position == Position::Fixed {
            // Fixed positioned children are relative to the viewport
            (0.0, 0.0)
        } else if child_style.position == Position::Absolute {
            // Absolute positioned children have their positions set
            // relative to the nearest positioned ancestor (containing block).
            // The containing block is the nearest positioned ancestor,
            // and child's layout_box.x/y are set relative to that.
            // We use the parent's ABSOLUTE position (abs_x, abs_y) since this node
            // IS the containing block and the child's x/y are relative to it.
            (abs_x, abs_y)
        } else {
            (abs_x, abs_y)
        };
        let mut child_boxes =
            flatten_with_clip(child, child_offset.0, child_offset.1, clip, styles);
        if let Some(ref href) = parent_href {
            for fb in &mut child_boxes {
                if fb.link_href.is_none() {
                    fb.link_href = Some(href.clone());
                }
            }
        }
        // Propagate ::first-letter styles from parent to text children
        // The first-letter styles are on the element, but apply to its first text child
        if layout_box.first_letter_len.is_some() {
            for fb in &mut child_boxes {
                if fb.box_type == BoxType::Text && fb.first_letter_len.is_none() {
                    // Only apply to first text child that doesn't already have first-letter
                    fb.first_letter_len = layout_box.first_letter_len;
                    fb.first_letter_color = layout_box.first_letter_color;
                    fb.first_letter_font_size = layout_box.first_letter_font_size;
                    fb.first_letter_font_weight = layout_box.first_letter_font_weight;
                    fb.first_letter_font_family = layout_box.first_letter_font_family.clone();
                    fb.first_letter_background_color = layout_box.first_letter_background_color;
                    fb.first_letter_text_decoration = layout_box.first_letter_text_decoration;
                    fb.first_letter_margin = layout_box.first_letter_margin;
                    fb.first_letter_padding = layout_box.first_letter_padding;
                    fb.first_letter_border_width = layout_box.first_letter_border_width;
                    fb.first_letter_border_color = layout_box.first_letter_border_color;
                    // Only apply to first text child
                    break;
                }
            }
        }
        // Propagate ::first-line styles from parent to text children on the first line
        // The first-line styles are on the element, but apply to text on its first line
        if layout_box.first_line_color.is_some()
            || layout_box.first_line_font_size.is_some()
            || layout_box.first_line_font_weight.is_some()
        {
            // Find the first text child and mark it as first line
            // This is a simplified approach - true first-line detection
            // requires knowing the actual line breaks during text layout
            let mut first_line_applied = false;
            for fb in &mut child_boxes {
                if fb.box_type == BoxType::Text && !first_line_applied {
                    fb.first_line_has_content = true;
                    fb.first_line_color = layout_box.first_line_color;
                    fb.first_line_font_size = layout_box.first_line_font_size;
                    fb.first_line_font_weight = layout_box.first_line_font_weight;
                    fb.first_line_font_family = layout_box.first_line_font_family.clone();
                    fb.first_line_background_color = layout_box.first_line_background_color;
                    fb.first_line_text_decoration = layout_box.first_line_text_decoration;
                    fb.first_line_letter_spacing = layout_box.first_line_letter_spacing;
                    fb.first_line_word_spacing = layout_box.first_line_word_spacing;
                    fb.first_line_text_transform = layout_box.first_line_text_transform;
                    first_line_applied = true;
                }
            }
        }
        result.extend(child_boxes);
    }

    result
}

#[derive(Debug, Clone)]
pub struct FlatBox {
    pub node_id: NodeId,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub box_type: BoxType,
    pub text: Option<String>,
    pub image_src: Option<String>,
    pub link_href: Option<String>,
    /// Clipping rectangle from nearest ancestor with overflow:hidden.
    /// (x, y, width, height) in absolute coordinates. None = no clipping.
    pub clip: Option<(f32, f32, f32, f32)>,
    /// Float text indent: (indent_px, num_lines, is_left)
    pub float_text_indent: Option<(f32, u32, bool)>,
    /// Input type for form controls
    pub input_type: Option<InputType>,
    /// Textarea rows/cols info
    pub textarea_info: Option<TextAreaInfo>,
    /// Marker styles for list item markers (::marker pseudo-element)
    pub marker_color: Option<incognidium_style::CssColor>,
    pub marker_font_size: Option<f32>,
    pub marker_font_weight: Option<incognidium_style::FontWeight>,
    pub marker_font_family: Option<incognidium_style::FontFamily>,
    pub marker_background_color: Option<incognidium_style::CssColor>,
    pub marker_letter_spacing: Option<f32>,
    pub marker_word_spacing: Option<f32>,
    /// Whether this box is a list item marker
    pub is_list_marker: bool,
    /// List style position (inside/outside) for this marker
    pub list_style_position: incognidium_style::ListStylePosition,
    /// ::first-letter styles (for drop caps and initial letter styling)
    pub first_letter_len: Option<usize>, // Number of chars to treat as first letter
    pub first_letter_color: Option<incognidium_style::CssColor>,
    pub first_letter_font_size: Option<f32>,
    pub first_letter_font_weight: Option<incognidium_style::FontWeight>,
    pub first_letter_font_family: Option<incognidium_style::FontFamily>,
    pub first_letter_background_color: Option<incognidium_style::CssColor>,
    pub first_letter_text_decoration: Option<incognidium_style::TextDecoration>,
    pub first_letter_margin: Option<(f32, f32, f32, f32)>, // top, right, bottom, left
    pub first_letter_padding: Option<(f32, f32, f32, f32)>,
    pub first_letter_border_width: Option<f32>,
    pub first_letter_border_color: Option<incognidium_style::CssColor>,
    /// ::first-line styles (for styling the first line of text)
    pub first_line_has_content: bool, // Whether this text box is on the first line
    pub first_line_color: Option<incognidium_style::CssColor>,
    pub first_line_font_size: Option<f32>,
    pub first_line_font_weight: Option<incognidium_style::FontWeight>,
    pub first_line_font_family: Option<incognidium_style::FontFamily>,
    pub first_line_background_color: Option<incognidium_style::CssColor>,
    pub first_line_text_decoration: Option<incognidium_style::TextDecoration>,
    pub first_line_letter_spacing: Option<f32>,
    pub first_line_word_spacing: Option<f32>,
    pub first_line_text_transform: Option<incognidium_style::TextTransform>,
    /// For table cells in border-collapse mode: resolved border widths
    pub collapsed_borders: Option<CollapsedBorders>,
    /// For table cells: if true, hide borders/background (empty-cells: hide)
    pub hide_empty_cell: bool,
    /// For multi-column layout: number of columns
    pub column_count: usize,
    /// For multi-column layout: width of each column
    pub column_width: f32,
    /// For multi-column layout: gap between columns
    pub column_gap: f32,
    /// For multi-column layout: rule (line) between columns
    pub column_rule_width: f32,
    pub column_rule_style: incognidium_style::ColumnRuleStyle,
    pub column_rule_color: incognidium_style::CssColor,
    /// For multi-column layout: absolute position of content start (for rule positioning)
    pub content_x: f32,
    pub content_y: f32,
    /// For multi-column layout: content height
    pub content_height: f32,
}

/// Convert a number to alphabetic representation (a, b, c, ... aa, ab, etc.)
fn number_to_alpha(mut n: usize, uppercase: bool) -> String {
    if n == 0 {
        return if uppercase {
            "A".to_string()
        } else {
            "a".to_string()
        };
    }
    let mut result = String::new();
    while n > 0 {
        n -= 1;
        let ch = if uppercase {
            (b'A' + (n % 26) as u8) as char
        } else {
            (b'a' + (n % 26) as u8) as char
        };
        result.insert(0, ch);
        n /= 26;
    }
    result
}

/// Convert a number to roman numeral representation
fn number_to_roman(mut n: usize) -> String {
    if n == 0 {
        return "".to_string();
    }
    let values = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut result = String::new();
    for (value, symbol) in values.iter() {
        while n >= *value {
            result.push_str(symbol);
            n -= value;
        }
    }
    result
}

/// Convert a number to Greek letter representation (α, β, γ, ...)
fn number_to_greek(mut n: usize, uppercase: bool) -> String {
    if n == 0 {
        return String::new();
    }
    // Greek letters: αβγδεζηθικλμνξοπρστυφχψω
    let greek_lower = [
        'α', 'β', 'γ', 'δ', 'ε', 'ζ', 'η', 'θ', 'ι', 'κ', 'λ', 'μ',
        'ν', 'ξ', 'ο', 'π', 'ρ', 'σ', 'τ', 'υ', 'φ', 'χ', 'ψ', 'ω'
    ];
    let greek_upper = [
        'Α', 'Β', 'Γ', 'Δ', 'Ε', 'Ζ', 'Η', 'Θ', 'Ι', 'Κ', 'Λ', 'Μ',
        'Ν', 'Ξ', 'Ο', 'Π', 'Ρ', 'Σ', 'Τ', 'Υ', 'Φ', 'Χ', 'Ψ', 'Ω'
    ];

    let letters = if uppercase { &greek_upper } else { &greek_lower };
    let base = letters.len();

    if n <= base {
        letters.get(n - 1).map(|c| c.to_string()).unwrap_or_default()
    } else {
        // For numbers beyond the alphabet, combine letters (simplified)
        let mut result = String::new();
        while n > 0 {
            let idx = ((n - 1) % base) as usize;
            if let Some(c) = letters.get(idx) {
                result.insert(0, *c);
            }
            n = (n - 1) / base;
        }
        result
    }
}

/// Convert a number to Armenian numeral representation
fn number_to_armenian(mut n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    // Armenian numerals (simplified using Armenian letters)
    // Full Armenian numeral system is complex; this uses a letter-based approach
    let armenian = [
        (9000, "Ք"), (8000, "Փ"), (7000, "Ւ"), (6000, "Ց"), (5000, "Ր"),
        (4000, "Տ"), (3000, "Վ"), (2000, "Ս"), (1000, "Ռ"),
        (900, "Ջ"), (800, "Պ"), (700, "Չ"), (600, "Ո"), (500, "Շ"),
        (400, "Ն"), (300, "Յ"), (200, "Մ"), (100, "Ճ"),
        (90, "Ղ"), (80, "Ձ"), (70, "Հ"), (60, "Կ"), (50, "Ծ"),
        (40, "Խ"), (30, "Լ"), (20, "Ի"), (10, "Ժ"),
        (9, "Թ"), (8, "Ը"), (7, "Է"), (6, "Զ"), (5, "Ե"),
        (4, "Դ"), (3, "Գ"), (2, "Բ"), (1, "Ա"),
    ];
    let mut result = String::new();
    for (value, symbol) in armenian.iter() {
        while n >= *value {
            result.push_str(symbol);
            n -= value;
        }
    }
    result
}

/// Convert a number to Georgian numeral representation
fn number_to_georgian(mut n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    // Georgian (Georgian alphabet letters used as numerals)
    // Simplified representation
    let georgian = [
        (10000, "ჯ"), (9000, "ჴ"), (8000, ""), (7000, ""), (6000, ""),
        (5000, "ჰ"), (4000, "ჳ"), (3000, "ჲ"), (2000, "ჱ"), (1000, "ჺ"),
        (900, "ჵ"), (800, ""), (700, ""), (600, ""), (500, "ჭ"),
        (400, ""), (300, ""), (200, ""), (100, "რ"),
        (90, ""), (80, ""), (70, ""), (60, ""), (50, "ნ"),
        (40, ""), (30, ""), (20, ""), (10, "ი"),
        (9, "შ"), (8, "ყ"), (7, "ღ"), (6, "ქ"), (5, "ფ"),
        (4, "ჳ"), (3, "ბ"), (2, "გ"), (1, "ა"),
    ];
    let mut result = String::new();
    for (value, symbol) in georgian.iter() {
        if !symbol.is_empty() {
            while n >= *value {
                result.push_str(symbol);
                n -= value;
            }
        }
    }
    result
}

fn number_to_hebrew(mut n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    // Hebrew numerals using Hebrew letters
    // Hebrew uses letters as numerals, with special final forms for thousands
    let hebrew = [
        (400, "ת"), (300, "ש"), (200, "ר"), (100, "ק"),
        (90, "צ"), (80, "פ"), (70, "ע"), (60, "ס"), (50, "נ"),
        (40, "מ"), (30, "ל"), (20, "כ"), (10, "י"),
        (9, "ט"), (8, "ח"), (7, "ז"), (6, "ו"), (5, "ה"),
        (4, "ד"), (3, "ג"), (2, "ב"), (1, "א"),
    ];
    let mut result = String::new();
    for (value, symbol) in hebrew.iter() {
        while n >= *value {
            result.push_str(symbol);
            n -= value;
        }
    }
    result
}

fn number_to_hiragana(mut n: usize) -> String {
    if n == 0 || n > 48 {
        return format!("{}", n);
    }
    // Hiragana a, i, u, e, o, ka, ki, ku, ke, ko... pattern
    let hiragana = [
        "あ", "い", "う", "え", "お",
        "か", "き", "く", "け", "こ",
        "さ", "し", "す", "せ", "そ",
        "た", "ち", "つ", "て", "と",
        "な", "に", "ぬ", "ね", "の",
        "は", "ひ", "ふ", "へ", "ほ",
        "ま", "み", "む", "め", "も",
        "や", "ゆ", "よ",
        "ら", "り", "る", "れ", "ろ",
        "わ", "ゐ", "ゑ", "を", "ん",
    ];
    hiragana.get(n - 1).unwrap_or(&"").to_string()
}

fn number_to_katakana(mut n: usize) -> String {
    if n == 0 || n > 48 {
        return format!("{}", n);
    }
    // Katakana equivalent pattern
    let katakana = [
        "ア", "イ", "ウ", "エ", "オ",
        "カ", "キ", "ク", "ケ", "コ",
        "サ", "シ", "ス", "セ", "ソ",
        "タ", "チ", "ツ", "テ", "ト",
        "ナ", "ニ", "ヌ", "ネ", "ノ",
        "ハ", "ヒ", "フ", "ヘ", "ホ",
        "マ", "ミ", "ム", "メ", "モ",
        "ヤ", "ユ", "ヨ",
        "ラ", "リ", "ル", "レ", "ロ",
        "ワ", "ヰ", "ヱ", "ヲ", "ン",
    ];
    katakana.get(n - 1).unwrap_or(&"").to_string()
}

fn number_to_hiragana_iroha(mut n: usize) -> String {
    if n == 0 || n > 47 {
        return format!("{}", n);
    }
    // Iroha sequence - traditional Japanese ordering
    let iroha = [
        "い", "ろ", "は", "に", "ほ", "へ", "と",
        "ち", "り", "ぬ", "る", "を", "わ", "か",
        "よ", "た", "れ", "そ", "つ", "ね", "な",
        "ら", "む", "う", "の", "お", "く", "き",
        "ま", "け", "ふ", "こ", "え", "て", "あ",
        "さ", "き", "ゆ", "め", "み", "し", "ゑ",
        "ひ", "も", "せ", "す",
    ];
    iroha.get(n - 1).unwrap_or(&"").to_string()
}

fn number_to_katakana_iroha(mut n: usize) -> String {
    if n == 0 || n > 47 {
        return format!("{}", n);
    }
    // Katakana Iroha sequence
    let iroha = [
        "イ", "ロ", "ハ", "ニ", "ホ", "ヘ", "ト",
        "チ", "リ", "ヌ", "ル", "ヲ", "ワ", "カ",
        "ヨ", "タ", "レ", "ソ", "ツ", "ネ", "ナ",
        "ラ", "ム", "ウ", "ノ", "オ", "ク", "キ",
        "マ", "ケ", "フ", "コ", "エ", "テ", "ア",
        "サ", "キ", "ユ", "メ", "ミ", "シ", "ヱ",
        "ヒ", "モ", "セ", "ス",
    ];
    iroha.get(n - 1).unwrap_or(&"").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use incognidium_dom::{Document, ElementData, NodeData, TextData};

    #[test]
    fn test_basic_layout() {
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let p = doc.add_node(body, NodeData::Element(ElementData::new("p")));
        let _text = doc.add_node(
            p,
            NodeData::Text(TextData {
                content: "Hello, world!".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css("");
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 800.0, 600.0);
        let root = layout(&doc, &styles, 800.0, 600.0);

        assert!(root.width > 0.0);
        assert!(root.height > 0.0);

        let flat = flatten_layout(&root, 0.0, 0.0, &styles);
        assert!(!flat.is_empty());
    }
}

// Table layout functions

/// Resolve border conflict for collapsed table borders.
/// Returns the winning border width based on CSS border conflict resolution rules.
fn resolve_border_conflict(width1: f32, width2: f32) -> f32 {
    // CSS border conflict resolution: wider border wins
    // If equal, the order of preference is: double, solid, dashed, dotted, none
    // For simplicity, we just use the maximum width
    width1.max(width2)
}

fn layout_table(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    image_sizes: &ImageSizes,
    _parent_floats: FloatState,
) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    // Check if border-collapse is active
    let is_collapsed = style.border_collapse == incognidium_style::BorderCollapse::Collapse;

    // Calculate width
    let margin_left = style.margin_left;
    let margin_right = style.margin_right;
    let padding_left = style.padding_left;
    let padding_right = style.padding_right;
    let border_left = style.border_left_width;
    let border_right = style.border_right_width;

    let is_border_box = style.box_sizing == incognidium_style::BoxSizing::BorderBox;
    let content_width = match style.width {
        SizeValue::Px(w) => {
            if is_border_box {
                (w - padding_left - padding_right - border_left - border_right).max(0.0)
            } else {
                w
            }
        }
        SizeValue::Percent(p) => {
            let total = containing_width * p / 100.0;
            if is_border_box {
                (total - padding_left - padding_right - border_left - border_right).max(0.0)
            } else {
                total
            }
        }
        SizeValue::Auto | SizeValue::None => {
            (containing_width - margin_left - margin_right).max(0.0)
        }
        // CSS Math Functions - treat as auto for now
        _ => (containing_width - margin_left - margin_right).max(0.0),
    };

    layout_box.width = content_width + padding_left + padding_right + border_left + border_right;
    layout_box.content_width = content_width;

    // Handle caption-side: find caption element and position it
    let caption_at_bottom = style.caption_side == incognidium_style::CaptionSide::Bottom;

    // Separate captions from other table children (rows/sections)
    let mut caption_indices: Vec<usize> = Vec::new();
    let mut row_indices: Vec<usize> = Vec::new();
    for (i, child) in layout_box.children.iter().enumerate() {
        if child.box_type == BoxType::TableCaption {
            caption_indices.push(i);
        } else {
            row_indices.push(i);
        }
    }

    // Layout captions first (we'll reposition them based on caption-side)
    let mut caption_height = 0.0f32;
    let padding_top = style.padding_top;
    let border_top = style.border_top_width;
    for &idx in &caption_indices {
        compute_layout_with_floats(
            &mut layout_box.children[idx],
            styles,
            content_width,
            0.0,
            image_sizes,
            FloatState::default(),
        );
        // Position caption at top initially (will adjust if caption-side: bottom)
        layout_box.children[idx].x = padding_left + border_left;
        layout_box.children[idx].y = padding_top + border_top;
        caption_height = layout_box.children[idx].height;
    }

    // Layout children (rows or sections)
    let mut y_offset = padding_left + border_left + if caption_at_bottom { 0.0 } else { caption_height };
    let (border_h, border_v) = if is_collapsed {
        (0.0, 0.0) // No spacing in collapsed mode
    } else {
        style.border_spacing
    };

    // Collect border widths for all cells to resolve conflicts
    let mut cell_borders: Vec<Vec<(f32, f32, f32, f32)>> = Vec::new(); // (top, right, bottom, left) for each cell

    // First pass: collect all cell borders
    if is_collapsed {
        for (_row_idx, row) in layout_box.children.iter().enumerate() {
            let mut row_borders: Vec<(f32, f32, f32, f32)> = Vec::new();
            for cell in &row.children {
                if let Some(cell_style) = styles.get(&cell.node_id) {
                    row_borders.push((
                        cell_style.border_top_width,
                        cell_style.border_right_width,
                        cell_style.border_bottom_width,
                        cell_style.border_left_width,
                    ));
                } else {
                    row_borders.push((0.0, 0.0, 0.0, 0.0));
                }
            }
            cell_borders.push(row_borders);
        }
    }

    // Second pass: layout rows and calculate collapsed borders
    let num_rows = layout_box.children.len();
    for (row_idx, child) in layout_box.children.iter_mut().enumerate() {
        compute_layout_with_floats(
            child,
            styles,
            content_width,
            0.0,
            image_sizes,
            FloatState::default(),
        );
        child.x = padding_left + border_left + border_h;
        child.y = y_offset + border_v;

        // If border-collapse, resolve borders for cells in this row
        if is_collapsed {
            let is_first_row = row_idx == 0;
            let is_last_row = row_idx == num_rows - 1;
            let num_cells = child.children.len();

            for (cell_idx, cell) in child.children.iter_mut().enumerate() {
                let is_first_col = cell_idx == 0;
                let is_last_col = cell_idx == num_cells - 1;

                let cell_style = styles.get(&cell.node_id).cloned().unwrap_or_default();

                // Get this cell's borders
                let top = cell_style.border_top_width;
                let right = cell_style.border_right_width;
                let bottom = cell_style.border_bottom_width;
                let left = cell_style.border_left_width;

                // Resolve conflicts with adjacent cells
                // Top border: conflict with cell above (or table top border)
                let resolved_top = if is_first_row {
                    top.max(style.border_top_width) // Conflict with table border
                } else if let Some(prev_row) = cell_borders.get(row_idx - 1) {
                    if let Some(prev_cell) = prev_row.get(cell_idx) {
                        resolve_border_conflict(top, prev_cell.2) // Conflict with cell above's bottom border
                    } else {
                        top
                    }
                } else {
                    top
                };

                // Left border: conflict with cell to the left
                let resolved_left = if is_first_col {
                    left.max(style.border_left_width) // Conflict with table border
                } else if let Some(row_borders) = cell_borders.get(row_idx) {
                    if let Some(left_cell) = row_borders.get(cell_idx - 1) {
                        resolve_border_conflict(left, left_cell.1) // Conflict with left cell's right border
                    } else {
                        left
                    }
                } else {
                    left
                };

                // Store resolved borders in the cell
                cell.collapsed_borders = Some(CollapsedBorders {
                    top: resolved_top,
                    right: right, // Will be resolved when we process the next cell
                    bottom: bottom, // Will be resolved when we process the next row
                    left: resolved_left,
                    is_first_row,
                    is_last_row,
                    is_first_column: is_first_col,
                    is_last_column: is_last_col,
                });
            }
        }

        y_offset += child.height + border_v;
    }

    // If caption-side: bottom, reposition captions after table rows
    if caption_at_bottom {
        let table_content_height = y_offset - padding_left - border_left - (if caption_at_bottom { 0.0 } else { caption_height });
        for &idx in &caption_indices {
            layout_box.children[idx].y = padding_top + border_top + table_content_height;
        }
    }

    let content_height = y_offset - padding_left - border_left + border_v
        + if caption_at_bottom { caption_height } else { 0.0 };
    layout_box.content_height = content_height.max(0.0);
    layout_box.height = content_height + padding_left + padding_right + border_left + border_right;
}

fn layout_table_section(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    image_sizes: &ImageSizes,
    _parent_floats: FloatState,
) {
    // Table sections (thead, tbody, tfoot) just lay out their children (rows)
    let _style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    let mut y_offset = 0.0;
    // SAFETY CAP for table sections
    const MAX_HEIGHT: f32 = 100_000.0;

    for child in &mut layout_box.children {
        compute_layout_with_floats(
            child,
            styles,
            containing_width,
            0.0,
            image_sizes,
            FloatState::default(),
        );
        child.x = 0.0;
        child.y = y_offset;
        y_offset += child.height;
        if y_offset > MAX_HEIGHT {
            break;
        }
    }

    let final_height = y_offset.min(MAX_HEIGHT);
    layout_box.width = containing_width;
    layout_box.height = final_height;
    layout_box.content_width = containing_width;
    layout_box.content_height = final_height;
}

fn layout_table_row(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    image_sizes: &ImageSizes,
) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    // Handle visibility: collapse - collapsed rows take zero space but maintain column structure
    if style.visibility == Visibility::Collapse {
        layout_box.width = containing_width;
        layout_box.height = 0.0;
        layout_box.content_width = containing_width;
        layout_box.content_height = 0.0;
        // Still layout children (for column alignment) but they'll be hidden
        let num_children = layout_box.children.len().max(1);
        for child in &mut layout_box.children {
            compute_layout_with_floats(
                child,
                styles,
                containing_width / num_children as f32,
                0.0,
                image_sizes,
                FloatState::default(),
            );
        }
        return;
    }

    let num_cells = layout_box.children.len().max(1);

    // Check if we're in border-collapse mode by looking at parent table
    // In collapsed mode, cells are adjacent without spacing
    let is_collapsed = layout_box.children.iter().any(|child| {
        child
            .collapsed_borders
            .map(|cb| cb.top >= 0.0) // Just check if collapsed_borders is set
            .unwrap_or(false)
    });

    // Get border spacing from parent (use default if not in table context)
    // In border-collapse mode, spacing is 0
    let cell_width = containing_width / num_cells as f32;
    let (border_h, border_v) = if is_collapsed {
        (0.0, 0.0)
    } else {
        style.border_spacing
    };

    let mut max_cell_height = 0.0f32;
    let mut x_offset = border_h;

    // First pass: layout all cells to get their natural heights
    for child in &mut layout_box.children {
        // In border-collapse mode, cells include their borders in the width
        let available_width = if is_collapsed {
            cell_width
        } else {
            cell_width - border_h * 2.0
        };
        compute_layout_with_floats(
            child,
            styles,
            available_width,
            0.0,
            image_sizes,
            FloatState::default(),
        );
        max_cell_height = max_cell_height.max(child.height);
    }

    // Second pass: set positions and stretch cells to row height
    for child in &mut layout_box.children {
        child.x = x_offset;
        child.y = border_v;
        // Stretch cell to match tallest cell in row (for equal-height cells)
        if child.height < max_cell_height {
            child.height = max_cell_height;
            child.content_height = max_cell_height - child.y - border_v;
        }
        x_offset += child.width + border_h * 2.0;
    }

    layout_box.width = containing_width;
    layout_box.height = max_cell_height + border_v * 2.0;
    layout_box.content_width = containing_width;
    layout_box.content_height = max_cell_height;
}

fn layout_table_cell(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    image_sizes: &ImageSizes,
    _parent_floats: FloatState,
) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    let padding_left = style.padding_left;
    let padding_right = style.padding_right;
    let padding_top = style.padding_top;
    let padding_bottom = style.padding_bottom;

    // Use collapsed borders if set, otherwise use style borders
    let (border_top, border_right, border_bottom, border_left) = if let Some(cb) = layout_box.collapsed_borders {
        (cb.top, cb.right, cb.bottom, cb.left)
    } else {
        (
            style.border_top_width,
            style.border_right_width,
            style.border_bottom_width,
            style.border_left_width,
        )
    };

    let content_width =
        containing_width - padding_left - padding_right - border_left - border_right;

    // Layout children as a block
    let mut y_offset = padding_top + border_top;
    for child in &mut layout_box.children {
        compute_layout_with_floats(
            child,
            styles,
            content_width,
            0.0,
            image_sizes,
            FloatState::default(),
        );
        child.x = padding_left + border_left;
        child.y = y_offset;
        y_offset += child.height;
    }

    let content_height = y_offset - padding_top - border_top;
    layout_box.content_width = content_width.max(0.0);
    layout_box.content_height = content_height.max(0.0);
    layout_box.width = containing_width;
    layout_box.height = content_height + padding_top + padding_bottom + border_top + border_bottom;

    // Check for empty-cells: hide
    // An empty cell has no meaningful content (no text, no children with content)
    let is_empty = layout_box.children.is_empty() ||
        layout_box.children.iter().all(|c| {
            match c.box_type {
                BoxType::Text => c.text.as_ref().map(|t| t.trim().is_empty()).unwrap_or(true),
                BoxType::None => true,
                _ => false,
            }
        });

    if is_empty && style.empty_cells == incognidium_style::EmptyCells::Hide {
        layout_box.hide_empty_cell = true;
    }
}
