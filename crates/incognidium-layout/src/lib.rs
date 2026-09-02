use incognidium_dom::{Document, NodeData, NodeId};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

thread_local! {
    /// Viewport dimensions used as the containing block for fixed positioned boxes.
    static VIEWPORT_SIZE: Cell<(f32, f32)> = Cell::new((0.0, 0.0));
    /// Node id of the document root passed to layout_with_images. The root is the
    /// initial containing block, so percentage heights on its direct children (e.g.
    /// html { height: 100% }) must resolve against the viewport even though the
    /// root box itself has auto height in the style map.
    static ROOT_NODE_ID: Cell<Option<NodeId>> = Cell::new(None);
    /// Column widths for the table currently being laid out. Set by layout_table
    /// so that sibling rows share the same column widths in auto-width tables.
    static TABLE_COL_WIDTHS: RefCell<Vec<f32>> = RefCell::new(Vec::new());
}

/// Float state passed from parent blocks to child blocks.
#[derive(Clone, Copy, Default)]
pub struct FloatState {
    pub left_width: f32,
    pub right_width: f32,
    pub remaining_height: f32,
}
use incognidium_style::{
    format_counter_value, AlignItems, AlignSelf, ClipRect, ComputedStyle, ContentVisibility,
    CounterStyle, Display, FlexDirection, FlexWrap, Float, GridLine, GridTrackSize, JustifyContent,
    JustifyItems, JustifySelf, ListStylePosition, Overflow, Position, RepeatCount, SizeValue,
    StyleMap, TextAlign, TextAlignLast, TextJustify, TextTransform, TextWrap, Visibility,
    WhiteSpaceCollapse,
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

/// Resolve a Content value to text, using the provided counter state and the
/// originating element's attributes for `attr()` values. Returns `None` if the
/// content should not generate a text box.
fn resolve_content_to_text(
    content: &incognidium_style::Content,
    counters: &CounterState,
    quotes: &incognidium_style::Quotes,
    quote_depth: usize,
    doc: &Document,
    node_id: NodeId,
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
        Content::Attr(name, fallback) => {
            let node = doc.node(node_id);
            if let NodeData::Element(el) = &node.data {
                el.get_attr(name)
                    .map(|v| v.to_string())
                    .or_else(|| fallback.clone())
            } else {
                fallback.clone()
            }
        }
        Content::Parts(parts) => {
            let mut result = String::new();
            for part in parts {
                if let Some(text) =
                    resolve_content_to_text(part, counters, quotes, quote_depth, doc, node_id)
                {
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
/// For text boxes, returns the text width. For block containers, returns the
/// max child width. For row flex containers, returns the sum of child widths
/// plus gaps on a single hypothetical line (the CSS max-content main size),
/// regardless of `flex-wrap`; this is needed to correctly shrink-to-fit
/// navigation bars and wrapping flex menus.
/// Return the content-box height for a style when it is definite, taking
/// `box-sizing` into account. Percentage padding is resolved against the
/// supplied `containing_width`; when that is unavailable a zero width is used,
/// which is sufficient for the common case of aspect-ratio wrappers that have
/// zero or pixel padding.
fn definite_content_box_height(
    style: &ComputedStyle,
    containing_width: f32,
    containing_height: f32,
) -> Option<f32> {
    let pb_height = style.padding_top_px(containing_width)
        + style.padding_bottom_px(containing_width)
        + style.border_top_width
        + style.border_bottom_width;
    let total = match style.height {
        SizeValue::Px(h) => Some(h),
        SizeValue::Percent(p) if containing_height > 0.0 => Some(containing_height * p / 100.0),
        _ => None,
    }?;
    Some(
        if style.box_sizing == incognidium_style::BoxSizing::BorderBox {
            (total - pb_height).max(0.0)
        } else {
            total
        },
    )
}

fn calculate_intrinsic_width(lb: &LayoutBox, styles: &StyleMap) -> f32 {
    // For text boxes, use the content width directly (this is the natural text width
    // before any constraints are applied, especially important for nowrap text)
    if lb.box_type == BoxType::Text {
        if let Some(ref text) = lb.text {
            if !text.is_empty() {
                // content_width is set to natural width during text layout
                return lb.content_width.max(0.0);
            }
        }
    }

    // Images (including rasterized inline-SVG placeholders) should report their
    // explicit or intrinsic width, not the width they were inflated to during a
    // max-content measuring pass.
    if lb.box_type == BoxType::Image {
        let style = styles.get(&lb.node_id).cloned().unwrap_or_default();
        if let SizeValue::Px(w) = style.width {
            if w > 0.0 {
                return w;
            }
        }
        return lb.content_width.max(0.0).min(lb.width);
    }

    // Honor explicit widths for any box type. This stops empty flex items from
    // falling back to a measuring-pass width, and makes block containers with a
    // fixed width report that width as their intrinsic size.
    let style = styles.get(&lb.node_id).cloned().unwrap_or_default();
    if let SizeValue::Px(w) = style.width {
        if w > 0.0 {
            return w;
        }
    }

    // A box with a definite height and an explicit aspect-ratio has a fixed
    // inline size derived from the ratio, even when its width is auto. Honor
    // that size before falling back to measuring children, otherwise an
    // aspect-ratio wrapper around a huge intrinsic image is sized by its
    // content instead of by the ratio.
    if !matches!(style.width, SizeValue::Px(_) | SizeValue::Percent(_)) {
        if let Some(ref ar) = style.aspect_ratio {
            if let Some(content_height) =
                definite_content_box_height(&style, lb.content_width, lb.content_height)
            {
                let ratio = ar.width / ar.height.max(0.001);
                if ratio > 0.0 && content_height > 0.0 {
                    let ratio_width = content_height * ratio;
                    return ratio_width.max(0.0);
                }
            }
        }
    }

    // Flex and inline containers measure their content width differently than
    // blocks: a row flex line is as wide as the sum of its items plus gaps.
    if lb.box_type == BoxType::Flex || lb.box_type == BoxType::InlineFlex {
        let style = styles.get(&lb.node_id).cloned().unwrap_or_default();
        let is_row = matches!(
            style.flex_direction,
            FlexDirection::Row | FlexDirection::RowReverse
        );
        if is_row {
            let gap = if style.column_gap > 0.0 {
                style.column_gap
            } else {
                style.gap
            };
            let mut total: f32 = 0.0;
            let mut count: usize = 0;
            for child in &lb.children {
                if child.box_type == BoxType::None {
                    continue;
                }
                let cs = styles.get(&child.node_id).cloned().unwrap_or_default();
                if cs.position == Position::Absolute || cs.position == Position::Fixed {
                    continue;
                }
                let child_pb = cs.padding_left_px(0.0)
                    + cs.padding_right_px(0.0)
                    + cs.border_left_width
                    + cs.border_right_width;
                total += calculate_intrinsic_width(child, styles)
                    + child_pb
                    + cs.margin_left
                    + cs.margin_right;
                if count > 0 {
                    total += gap;
                }
                count += 1;
            }
            return total.max(0.0);
        }
        // Column flex falls back to the max child width below.
    }

    // A grid container with a definite explicit width already has a used width
    // from layout; report that instead of re-deriving a min-content width from
    // its items. Otherwise a parent flex item with width:auto (e.g. a branded
    // hero grid) collapses to the narrowest item and is laid out at a tiny width,
    // even though its `width: 100%` resolved against a real containing block.
    //
    // A percentage width is the exception: when the only resolution it ever got
    // was against the max-content measuring sentinel, the laid-out width is
    // cyclic garbage, not a definite size. Treat it like auto there and measure
    // the tracks' content instead, otherwise an auto-width flex item holding
    // `width: 100%` (e.g. an icon-only search button) inflates to the full
    // measuring width and starves its flex line.
    if lb.box_type == BoxType::Grid {
        let style = styles.get(&lb.node_id).cloned().unwrap_or_default();
        if !matches!(style.width, SizeValue::Auto | SizeValue::None) && lb.width > 0.0 {
            let resolved_against_measuring_pass = match style.width {
                SizeValue::Percent(p) => (lb.width - 10000.0 * p / 100.0).abs() <= 512.0,
                _ => false,
            };
            if !resolved_against_measuring_pass {
                return lb.width;
            }
        }
    }

    if lb.box_type == BoxType::Inline
        || lb.box_type == BoxType::InlineBlock
        || lb.box_type == BoxType::InlineFlex
    {
        let style = styles.get(&lb.node_id).cloned().unwrap_or_default();
        // Compute intrinsic width by summing children's intrinsic widths.
        // This avoids inflation from block-level children (e.g. flex containers
        // with width:auto) that filled a huge measuring-pass width.
        let mut total: f32 = 0.0;
        // Word spaces between inline-level siblings: a whitespace-only run or
        // leading whitespace of a text run survives line layout as an inter-
        // word gap, so count one space at each such boundary.
        let mut prev_inline_open = false;
        let mut pending_space = false;
        // The previous inline sibling ended in a collapsible space that line
        // layout turns into an inter-word gap.
        let mut prev_trailing_space = false;
        for child in &lb.children {
            if child.box_type == BoxType::None {
                continue;
            }
            let cs = styles.get(&child.node_id).cloned().unwrap_or_default();
            if cs.position == Position::Absolute || cs.position == Position::Fixed {
                continue;
            }
            if is_whitespace_boundary_box(child) {
                if prev_inline_open {
                    pending_space = true;
                }
                continue;
            }
            let child_intrinsic = calculate_intrinsic_width(child, styles);
            // Block-level children (flex, grid, block, table) inside an
            // inline-block establish independent formatting contexts. Their
            // total outer width (content + padding + border) contributes to
            // the inline-block's intrinsic width, because the inline-block
            // must be wide enough to hold the child's full box.
            let is_block_level = matches!(
                child.box_type,
                BoxType::Block
                    | BoxType::Flex
                    | BoxType::InlineFlex
                    | BoxType::Grid
                    | BoxType::Table
                    | BoxType::Columns
            );
            if is_block_level {
                let child_pb = cs.padding_left_px(0.0)
                    + cs.padding_right_px(0.0)
                    + cs.border_left_width
                    + cs.border_right_width;
                total += child_intrinsic + child_pb;
                prev_inline_open = false;
                pending_space = false;
                prev_trailing_space = false;
            } else {
                // Non-text inline-level children contribute their horizontal
                // margins too: an inline run must fit the space around each
                // box (e.g. a generated `::before` separator glyph carrying
                // side margins inside an otherwise empty span). Text runs are
                // part of their enclosing inline box, whose own margins are
                // counted at that level, so adding them here would double
                // count.
                let is_inline_level = matches!(
                    child.box_type,
                    BoxType::Inline | BoxType::InlineBlock | BoxType::Image | BoxType::LineBreak
                );
                if is_inline_level || child.box_type == BoxType::Text {
                    if child.box_type == BoxType::LineBreak {
                        // A forced break ends the line: following leading
                        // whitespace collapses at the new line's start.
                        prev_inline_open = false;
                        pending_space = false;
                        prev_trailing_space = false;
                    } else {
                        let starts_with_space = child.text_leading_space;
                        if prev_inline_open
                            && (pending_space || starts_with_space || prev_trailing_space)
                        {
                            total += word_space_width(&cs);
                        }
                        pending_space = false;
                        prev_trailing_space = child.text_trailing_space;
                        prev_inline_open = true;
                    }
                    if child.box_type == BoxType::Text || child.box_type == BoxType::LineBreak {
                        total += child_intrinsic + cs.margin_left + cs.margin_right;
                    } else {
                        // Boxes that render their own padding/border (inline
                        // elements, form controls, images) count their full
                        // outer width on the line.
                        total += inline_child_outer_intrinsic_width(child, styles)
                            + cs.margin_left
                            + cs.margin_right;
                    }
                } else {
                    total += child_intrinsic;
                    prev_inline_open = false;
                    pending_space = false;
                    prev_trailing_space = false;
                }
            }
        }
        if total > 0.0 {
            if lb.box_type == BoxType::InlineBlock || lb.box_type == BoxType::InlineFlex {
                // Add the inline-block's own padding and border so the
                // intrinsic size accounts for the full box, not just the
                // content box.
                let pb = style.padding_left_px(0.0)
                    + style.padding_right_px(0.0)
                    + style.border_left_width
                    + style.border_right_width;
                return total + pb;
            }
            return total;
        }
        // Input elements with placeholder/value text but no children:
        // use the text width as the intrinsic width.
        if lb.input_type.is_some() {
            if let Some(ref text) = lb.text {
                if !text.is_empty() {
                    let text_width = measure_text_width(text, style.font_size, &style);
                    if lb.box_type == BoxType::InlineBlock || lb.box_type == BoxType::InlineFlex {
                        let pb = style.padding_left_px(0.0)
                            + style.padding_right_px(0.0)
                            + style.border_left_width
                            + style.border_right_width;
                        return text_width + pb;
                    }
                    return text_width;
                }
            }
        }
        // Empty inline/inline-block fallback to the laid-out width.
        if lb.width > 0.0 {
            return lb.width;
        }
        return lb.content_width.min(lb.width);
    }

    // For containers, use the max width of children. Floated children and
    // inline-level content share the current line — line layout places inline
    // runs beside floats in either DOM order (a right float lands at the line's
    // right edge next to the run before it, later content wraps around a left
    // float) — so the max-content line accumulates both, while a block-level
    // child (or a forced break) ends the line. Include each child's padding and
    // border so that a floated wrapper around a padded button/link reports the
    // full width it really needs (e.g. a header meta bar with floated links).
    let mut max_child_width: f32 = 0.0;
    let mut line_width: f32 = 0.0;
    // Word spaces between inline-level siblings survive line layout as
    // inter-word gaps, so they count toward the max-content line width even
    // though no single fragment's measured width carries them.
    let mut prev_inline_open = false;
    let mut pending_space = false;
    // The previous inline sibling ended in a collapsible space that line
    // layout turns into an inter-word gap.
    let mut prev_trailing_space = false;
    for child in &lb.children {
        if child.box_type == BoxType::None {
            continue;
        }
        // Whitespace-only text runs carry the word space between inline
        // siblings; remember the boundary instead of letting them break a line
        // of consecutive floats or inflate the intrinsic width by themselves.
        if is_whitespace_boundary_box(child) {
            if prev_inline_open {
                pending_space = true;
            }
            continue;
        }
        let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
        if child_style.position == Position::Absolute || child_style.position == Position::Fixed {
            continue;
        }
        let own_width = match child_style.width {
            SizeValue::Px(w) if w > 0.0 && child.box_type == BoxType::Image => Some(w),
            _ => None,
        };
        let child_intrinsic = own_width.unwrap_or_else(|| calculate_intrinsic_width(child, styles));
        // Include the child's own padding/border in its contribution, otherwise
        // a padded block child (e.g. an <a> inside a floated <div>) is measured
        // without its horizontal padding and the parent shrinks too far.
        let child_total = child_intrinsic
            + child_style.padding_left_px(0.0)
            + child_style.padding_right_px(0.0)
            + child_style.border_left_width
            + child_style.border_right_width;
        let is_inline_level = matches!(
            child.box_type,
            BoxType::Inline
                | BoxType::InlineBlock
                | BoxType::Text
                | BoxType::Image
                | BoxType::LineBreak
        );
        if child_style.float != Float::None {
            line_width += child_total + child_style.margin_left + child_style.margin_right;
            max_child_width = max_child_width.max(line_width);
            pending_space = false;
            prev_inline_open = false;
            prev_trailing_space = false;
        } else if is_inline_level {
            if child.box_type == BoxType::LineBreak {
                // A forced break ends the line: following leading whitespace
                // collapses at the new line's start.
                prev_inline_open = false;
                pending_space = false;
                prev_trailing_space = false;
                line_width = 0.0;
                continue;
            }
            let starts_with_space = child.text_leading_space;
            if prev_inline_open && (pending_space || starts_with_space || prev_trailing_space) {
                line_width += word_space_width(&child_style);
            }
            pending_space = false;
            prev_trailing_space = child.text_trailing_space;
            prev_inline_open = true;
            line_width += child_total + child_style.margin_left + child_style.margin_right;
            max_child_width = max_child_width.max(line_width);
        } else {
            // A block-level child occupies its own line below the floats and
            // inline content before it.
            max_child_width = max_child_width.max(child_total);
            line_width = 0.0;
            pending_space = false;
            prev_inline_open = false;
            prev_trailing_space = false;
        }
    }
    // If no children or all empty, the box has no intrinsic content width.  This
    // prevents auto-width table cells containing only a zero-width spacer image
    // (common on nested-table comment rows) from inflating to the 10000px
    // measuring-pass width and starving the real content column.
    if max_child_width > 0.0 {
        max_child_width
    } else {
        0.0
    }
}

/// Evaluate a SizeValue (calc, min, max, clamp) to pixels using the containing block context.
/// Viewport units (`vw`/`vh`) resolve against the real viewport dimensions stored in
/// `VIEWPORT_SIZE`, not the containing block, so nested flex/grid items using
/// `calc(Nvw ± ...)` get the same result as a top-level block.
fn evaluate_size_value(value: &SizeValue, containing_width: f32, font_size: f32) -> Option<f32> {
    use incognidium_style::CalcExpression;
    use incognidium_style::CalcValue;

    let (viewport_width, viewport_height) = VIEWPORT_SIZE.with(|v| {
        let (w, h) = v.get();
        if w > 0.0 {
            (w, h)
        } else {
            // Tests and fallback paths that run before the viewport is set can
            // use the containing width as a reasonable viewport proxy.
            (containing_width, containing_width)
        }
    });

    fn evaluate_calc_value(
        val: &CalcValue,
        containing_width: f32,
        viewport_width: f32,
        viewport_height: f32,
        font_size: f32,
    ) -> f32 {
        match val {
            CalcValue::Px(v) => *v,
            CalcValue::Percent(p) => p / 100.0 * containing_width,
            CalcValue::Em(e) => e * font_size,
            CalcValue::Rem(r) => r * incognidium_css::root_font_size(),
            CalcValue::Vw(v) => v * viewport_width / 100.0,
            CalcValue::Vh(v) => v * viewport_height / 100.0,
            CalcValue::Cap(v) => v * font_size * 0.7,
            // Container query units are approximated by the containing block / viewport.
            CalcValue::Cqw(v) => v * containing_width / 100.0,
            CalcValue::Cqh(v) => v * viewport_height / 100.0,
            CalcValue::Cqi(v) => v * containing_width / 100.0, // Inline size = width in horizontal writing
            CalcValue::Cqb(v) => v * viewport_height / 100.0,  // Block size approximation
            CalcValue::Cqmin(v) => v * containing_width.min(viewport_height) / 100.0,
            CalcValue::Cqmax(v) => v * containing_width.max(viewport_height) / 100.0,
        }
    }

    fn evaluate_calc_expr(
        expr: &CalcExpression,
        containing_width: f32,
        viewport_width: f32,
        viewport_height: f32,
        font_size: f32,
    ) -> f32 {
        match expr {
            CalcExpression::Value(v) => evaluate_calc_value(
                v,
                containing_width,
                viewport_width,
                viewport_height,
                font_size,
            ),
            CalcExpression::Add(a, b) => {
                evaluate_calc_expr(
                    a,
                    containing_width,
                    viewport_width,
                    viewport_height,
                    font_size,
                ) + evaluate_calc_expr(
                    b,
                    containing_width,
                    viewport_width,
                    viewport_height,
                    font_size,
                )
            }
            CalcExpression::Subtract(a, b) => {
                evaluate_calc_expr(
                    a,
                    containing_width,
                    viewport_width,
                    viewport_height,
                    font_size,
                ) - evaluate_calc_expr(
                    b,
                    containing_width,
                    viewport_width,
                    viewport_height,
                    font_size,
                )
            }
            CalcExpression::Multiply(a, b) => {
                evaluate_calc_expr(
                    a,
                    containing_width,
                    viewport_width,
                    viewport_height,
                    font_size,
                ) * evaluate_calc_expr(
                    b,
                    containing_width,
                    viewport_width,
                    viewport_height,
                    font_size,
                )
            }
            CalcExpression::Divide(a, b) => {
                let denom = evaluate_calc_expr(
                    b,
                    containing_width,
                    viewport_width,
                    viewport_height,
                    font_size,
                );
                if denom == 0.0 {
                    0.0
                } else {
                    evaluate_calc_expr(
                        a,
                        containing_width,
                        viewport_width,
                        viewport_height,
                        font_size,
                    ) / denom
                }
            }
        }
    }

    match value {
        SizeValue::Px(v) => Some(*v),
        SizeValue::Percent(p) => Some(p / 100.0 * containing_width),
        SizeValue::Calc(expr) => Some(evaluate_calc_expr(
            expr,
            containing_width,
            viewport_width,
            viewport_height,
            font_size,
        )),
        SizeValue::Min(vals) => {
            let resolved: Vec<f32> = vals
                .iter()
                .map(|v| {
                    evaluate_calc_value(
                        v,
                        containing_width,
                        viewport_width,
                        viewport_height,
                        font_size,
                    )
                })
                .collect();
            resolved.into_iter().reduce(f32::min)
        }
        SizeValue::Max(vals) => {
            let resolved: Vec<f32> = vals
                .iter()
                .map(|v| {
                    evaluate_calc_value(
                        v,
                        containing_width,
                        viewport_width,
                        viewport_height,
                        font_size,
                    )
                })
                .collect();
            resolved.into_iter().reduce(f32::max)
        }
        SizeValue::Clamp { min, val, max } => {
            let min_px = evaluate_calc_value(
                min,
                containing_width,
                viewport_width,
                viewport_height,
                font_size,
            );
            let val_px = evaluate_calc_value(
                val,
                containing_width,
                viewport_width,
                viewport_height,
                font_size,
            );
            let max_px = evaluate_calc_value(
                max,
                containing_width,
                viewport_width,
                viewport_height,
                font_size,
            );
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
    /// For text nodes, the pristine text the node started with. `layout_text`
    /// rewrites `text` with the line-broken form, and measuring passes can run
    /// at narrow widths that split words into fragments; without this field
    /// those fragments would leak into later passes as if they were separate
    /// words. Kept in sync whenever a text box is split into fragments.
    pub source_text: Option<String>,
    /// True if the original text node started with whitespace. Used to decide
    /// whether an automatic inter-word gap should appear between this text box
    /// and a neighboring inline-level box.
    pub text_leading_space: bool,
    /// True if the original text node ended with whitespace.
    pub text_trailing_space: bool,
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
    /// When set, layout_block uses this content width instead of resolving
    /// width/max-width/min-width from styles. Used by flex layout to size
    /// items without making percentage max-width resolve against the item's
    /// own basis.
    pub forced_content_width: Option<f32>,
    /// When set, layout_block uses this content height instead of resolving
    /// height/max-height/min-height from styles. Used by grid layout so a
    /// stretched grid item passes a definite height to percentage-height
    /// children.
    pub forced_content_height: Option<f32>,
    /// When set, layout_block passes this value as the containing height to
    /// children instead of using its own resolved height. Used by flex layout so
    /// percentage-height children inside auto-height flex items resolve against
    /// the flex container's cross size.
    pub forced_containing_height_for_children: Option<f32>,
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
    /// When true, this inline text fragment must be laid out below any active
    /// float rather than beside it. Set when a text box is split at a float
    /// boundary so the remaining text uses the full containing width.
    pub force_below_float: bool,
    /// When true, this inline fragment must start on a fresh line at the
    /// container's left edge. Set on the remainder of a split text box: the
    /// split decided the remainder begins a new line, so placement must not
    /// squeeze it onto the same line as the preceding fragment.
    pub force_line_break_before: bool,
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
    InlineFlex,
    Grid,
    Columns, // For multi-column layout
    Table,
    TableRow,
    TableCell,
    TableSection, // For thead, tbody, tfoot
    TableCaption, // For <caption> elements
    Text,
    Image,
    LineBreak, // For <br> elements
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
    ROOT_NODE_ID.with(|r| r.set(Some(root_id)));
    let mut counters = CounterState::default();
    let mut visited: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
    visited.insert(root_id);
    let mut root_box = build_layout_tree(doc, styles, root_id, &mut counters, &mut visited);
    root_box.width = viewport_width;
    VIEWPORT_SIZE.with(|v| v.set((viewport_width, viewport_height)));
    // The root font size must come from the <html> element, not the document
    // root node (which is often a bare #text node with the default 16 px font).
    // Author CSS such as `html { font-size: 62.5%; }` sets the rem basis; using
    // the document root node would silently reset it to 16 px and break every
    // rem-based layout (e.g. a header nav wrapping incorrectly).
    let html_id = doc.document_element().unwrap_or(root_id);
    if let Some(html_style) = styles.get(&html_id) {
        incognidium_css::set_root_font_size(html_style.font_size);
    }
    compute_layout(
        &mut root_box,
        styles,
        viewport_width,
        viewport_height,
        image_sizes,
    );
    ROOT_NODE_ID.with(|r| r.set(None));
    root_box
}

/// Extract the first image URL from a `srcset` attribute.
///
/// A srcset is a comma-separated list of candidates, each of the form
/// `url descriptor`. Browsers pick the best candidate based on device
/// pixel ratio and viewport. For layout purposes we only need *some* URL
/// that the engine can fetch and measure; picking the first candidate keeps
/// images from collapsing to nothing when an author omits a legacy `src`.
pub fn first_srcset_url(srcset: &str) -> Option<String> {
    srcset
        .split(',')
        .next()
        .and_then(|part| {
            let part = part.trim();
            // The URL is everything before the first whitespace descriptor.
            part.split_whitespace().next().map(|s| s.to_string())
        })
        .filter(|s| !s.is_empty())
}

/// Anonymous table boxes get this sentinel node id: no DOM node backs them, so
/// every style lookup for them misses and default styles apply — exactly what
/// CSS requires for anonymous boxes. Nothing may dereference this id against
/// the DOM.
const ANONYMOUS_TABLE_NODE_ID: NodeId = usize::MAX;

/// Build a styleless anonymous box (CSS 2.1 §17.2.1). Anonymous boxes carry no
/// styles of their own; they exist purely to give table-internal boxes a
/// well-formed table structure to lay out in.
fn anonymous_table_box(box_type: BoxType, children: Vec<LayoutBox>) -> LayoutBox {
    LayoutBox {
        node_id: ANONYMOUS_TABLE_NODE_ID,
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
        content_width: 0.0,
        content_height: 0.0,
        children,
        box_type,
        text: None,
        source_text: None,
        text_leading_space: false,
        text_trailing_space: false,
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
        forced_content_width: None,
        forced_content_height: None,
        forced_containing_height_for_children: None,
        force_below_float: false,
        force_line_break_before: false,
    }
}

fn is_table_internal_box(box_type: BoxType) -> bool {
    matches!(
        box_type,
        BoxType::TableCell | BoxType::TableRow | BoxType::TableSection
    )
}

/// Convert a run of table-internal sibling boxes into a single anonymous table
/// box. Consecutive cells are grouped into anonymous rows first; real rows and
/// row groups already are rows.
fn flush_anonymous_table_run(out: &mut Vec<LayoutBox>, run: &mut Vec<LayoutBox>) {
    if run.is_empty() {
        return;
    }
    let mut rows: Vec<LayoutBox> = Vec::new();
    let mut cells: Vec<LayoutBox> = Vec::new();
    for item in run.drain(..) {
        if item.box_type == BoxType::TableCell {
            cells.push(item);
        } else {
            if !cells.is_empty() {
                rows.push(anonymous_table_box(
                    BoxType::TableRow,
                    std::mem::take(&mut cells),
                ));
            }
            rows.push(item);
        }
    }
    if !cells.is_empty() {
        rows.push(anonymous_table_box(BoxType::TableRow, cells));
    }
    out.push(anonymous_table_box(BoxType::Table, rows));
}

/// CSS 2.1 §17.2.1: table-internal boxes (cells, rows, row groups) whose parent
/// is not a table box must get anonymous table and table-row wrappers so that
/// `display: table-cell` children of an ordinary block lay out side by side
/// like real table cells instead of stacking as full-width blocks.
fn wrap_anonymous_tables(children: Vec<LayoutBox>) -> Vec<LayoutBox> {
    if !children.iter().any(|c| is_table_internal_box(c.box_type)) {
        return children;
    }
    let mut out: Vec<LayoutBox> = Vec::new();
    let mut run: Vec<LayoutBox> = Vec::new();
    for child in children {
        if is_table_internal_box(child.box_type) {
            run.push(child);
            continue;
        }
        // Whitespace-only inline content between table boxes is not rendered
        // (CSS 2.1 §17.6.1); dropping it keeps runs contiguous across the
        // newlines authors leave between cell elements.
        if matches!(child.box_type, BoxType::Text | BoxType::Inline)
            && !run.is_empty()
            && child
                .text
                .as_ref()
                .map(|t| is_collapsible_whitespace_only(t))
                .unwrap_or(false)
        {
            continue;
        }
        flush_anonymous_table_run(&mut out, &mut run);
        out.push(child);
    }
    flush_anonymous_table_run(&mut out, &mut run);
    out
}

fn build_layout_tree(
    doc: &Document,
    styles: &StyleMap,
    node_id: NodeId,
    counters: &mut CounterState,
    visited: &mut std::collections::HashSet<NodeId>,
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

    // These elements never produce visual boxes, even if an author rule tries to
    // override their display. This prevents inline <style>/<script> content from
    // being laid out as visible text when CMS themes add classes that
    // accidentally match display:flex/display:block author rules.
    if let NodeData::Element(el) = &node.data {
        match el.tag_name.as_str() {
            "head" | "style" | "script" | "link" | "meta" | "title" | "template" | "datalist"
            | "base" => {
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
                    source_text: None,
                    text_leading_space: false,
                    text_trailing_space: false,
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
                    forced_content_width: None,
                    forced_content_height: None,
                    forced_containing_height_for_children: None,
                    force_below_float: false,
                    force_line_break_before: false,
                };
            }
            _ => {}
        }
    }

    let mut display = style.map(|s| s.display).unwrap_or(Display::Block);
    // A floated element blockifies its display (CSS 2.1 §9.7): an inline box
    // that floats lays out as a block. Leaving it inline laid out its children
    // in an inline context that ignores their float property, so floated
    // children stacked vertically instead of lining up side by side.
    if display == Display::Inline && style.map(|s| s.float != Float::None).unwrap_or(false) {
        display = Display::Block;
    }

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
            source_text: None,
            text_leading_space: false,
            text_trailing_space: false,
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
            forced_content_width: None,
            forced_content_height: None,
            forced_containing_height_for_children: None,
            force_below_float: false,
            force_line_break_before: false,
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
                let src = el
                    .get_attr("src")
                    .map(|s| s.to_string())
                    .or_else(|| el.get_attr("srcset").and_then(first_srcset_url))
                    .or_else(|| {
                        // Some responsive images use `<picture>` with `<source>`
                        // candidates and an `<img>` that has no `src` of its own.
                        // Fall back to the first preceding `<source srcset>` so the
                        // picture does not collapse in no-JS renders.
                        doc.nodes[node_id]
                            .parent
                            .and_then(|parent_id| {
                                let parent = &doc.nodes[parent_id];
                                if let NodeData::Element(parent_el) = &parent.data {
                                    (parent_el.tag_name == "picture").then_some(parent)
                                } else {
                                    None
                                }
                            })
                            .and_then(|parent| {
                                for &sibling_id in &parent.children {
                                    if sibling_id == node_id {
                                        break;
                                    }
                                    let sibling = &doc.nodes[sibling_id];
                                    if let NodeData::Element(sib_el) = &sibling.data {
                                        if sib_el.tag_name == "source" {
                                            if let Some(url) =
                                                sib_el.get_attr("srcset").and_then(first_srcset_url)
                                            {
                                                return Some(url);
                                            }
                                        }
                                    }
                                }
                                None
                            })
                    });
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
                // Show value or placeholder text (for text inputs and buttons).
                // An empty value="" attribute does not suppress the placeholder,
                // so only a non-empty value counts.
                let text = if matches!(
                    input_type,
                    InputType::Text | InputType::Button | InputType::Submit
                ) {
                    el.get_attr("value")
                        .filter(|s| !s.is_empty())
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
                let has_columns = style
                    .map(|s| s.column_count.is_some() || s.column_width.is_some())
                    .unwrap_or(false);

                if has_columns {
                    (BoxType::Columns, None, None, None, None)
                } else {
                    match display {
                        Display::Block => (BoxType::Block, None, None, None, None),
                        Display::InlineBlock => (BoxType::InlineBlock, None, None, None, None),
                        Display::Inline => (BoxType::Inline, None, None, None, None),
                        Display::Flex => (BoxType::Flex, None, None, None, None),
                        Display::InlineFlex => (BoxType::InlineFlex, None, None, None, None),
                        Display::Grid => (BoxType::Grid, None, None, None, None),
                        Display::Table => (BoxType::Table, None, None, None, None),
                        Display::TableRow => (BoxType::TableRow, None, None, None, None),
                        Display::TableCell => (BoxType::TableCell, None, None, None, None),
                        Display::TableHeaderGroup
                        | Display::TableRowGroup
                        | Display::TableFooterGroup => {
                            (BoxType::TableSection, None, None, None, None)
                        }
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
        // Comments never render. Mapping them to Block would let an empty
        // comment block (kept alive by an inherited background) reserve space
        // in the flow and displace its real siblings.
        NodeData::Comment(_) => (BoxType::None, None, None, None, None),
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
            if !visited.insert(child_id) {
                continue;
            }
            let child_box = build_layout_tree(doc, styles, child_id, counters, visited);
            // In a newline-preserving context (`<pre>`, `white-space:
            // pre-wrap`, ...), a whitespace-only text node containing newlines
            // is a forced line break: the newlines between the inline spans of
            // a preformatted block live in their own text nodes, and dropping
            // them collapses every line onto one.
            if is_whitespace_only_text(&child_box)
                && child_box.text.as_deref().is_some_and(|t| t.contains('\n'))
                && styles.get(&child_box.node_id).is_some_and(|s| {
                    matches!(
                        s.white_space,
                        incognidium_style::WhiteSpace::Pre
                            | incognidium_style::WhiteSpace::PreWrap
                            | incognidium_style::WhiteSpace::PreLine
                    ) || matches!(
                        s.white_space_collapse,
                        WhiteSpaceCollapse::Preserve
                            | WhiteSpaceCollapse::PreserveBreaks
                            | WhiteSpaceCollapse::BreakSpaces
                    )
                })
            {
                let newlines = child_box
                    .text
                    .as_deref()
                    .map(|t| t.matches('\n').count())
                    .unwrap_or(0);
                let mut br = child_box.clone();
                br.box_type = BoxType::LineBreak;
                br.text = None;
                br.source_text = None;
                br.text_leading_space = false;
                br.text_trailing_space = false;
                br.children = Vec::new();
                br.x = 0.0;
                br.y = 0.0;
                br.width = 0.0;
                br.height = 0.0;
                br.content_width = 0.0;
                br.content_height = 0.0;
                for _ in 0..newlines {
                    children.push(br.clone());
                }
                continue;
            }
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

    // Table-internal children of a non-table box need anonymous table and
    // table-row wrappers (CSS 2.1 §17.2.1). Real table boxes already provide
    // the structure, so skip them.
    if !matches!(
        box_type,
        BoxType::Table
            | BoxType::TableRow
            | BoxType::TableCell
            | BoxType::TableSection
            | BoxType::None
    ) {
        children = wrap_anonymous_tables(children);
    }

    // Add list bullet/number markers for <li> elements (respect list-style-type)
    // Also handle list-style-image for custom image markers. Only block-level
    // list items generate markers: browsers render `display: list-item` boxes,
    // so author CSS that overrides an item's display (inline, inline-block,
    // flex, ...) removes the marker.
    if let NodeData::Element(ref el) = node.data {
        let has_list_style_image = style.and_then(|s| s.list_style_image.as_ref()).is_some();
        let li_display = style
            .map(|s| s.display)
            .unwrap_or(incognidium_style::Display::Block);

        if el.tag_name == "li"
            && li_display == incognidium_style::Display::Block
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
                        source_text: None,
                        text_leading_space: false,
                        text_trailing_space: false,
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
                        forced_content_width: None,
                        forced_content_height: None,
                        forced_containing_height_for_children: None,
                        force_below_float: false,
                        force_line_break_before: false,
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
                    let _is_ordered = matches!(
                        marker_type,
                        incognidium_style::ListStyleType::Decimal
                    ) || matches!(
                        marker_type,
                        incognidium_style::ListStyleType::DecimalLeadingZero
                    ) || matches!(
                        marker_type,
                        incognidium_style::ListStyleType::LowerAlpha
                    ) || matches!(
                        marker_type,
                        incognidium_style::ListStyleType::UpperAlpha
                    ) || matches!(
                        marker_type,
                        incognidium_style::ListStyleType::LowerRoman
                    ) || matches!(
                        marker_type,
                        incognidium_style::ListStyleType::UpperRoman
                    ) || matches!(
                        marker_type,
                        incognidium_style::ListStyleType::LowerGreek
                    ) || matches!(
                        marker_type,
                        incognidium_style::ListStyleType::UpperGreek
                    ) || matches!(
                        marker_type,
                        incognidium_style::ListStyleType::Armenian
                    ) || matches!(
                        marker_type,
                        incognidium_style::ListStyleType::Georgian
                    ) || matches!(
                        marker_type,
                        incognidium_style::ListStyleType::Hebrew
                    ) || matches!(
                        marker_type,
                        incognidium_style::ListStyleType::Hiragana
                    ) || matches!(
                        marker_type,
                        incognidium_style::ListStyleType::Katakana
                    ) || matches!(
                        marker_type,
                        incognidium_style::ListStyleType::HiraganaIroha
                    ) || matches!(
                        marker_type,
                        incognidium_style::ListStyleType::KatakanaIroha
                    ) || matches!(
                        marker_type,
                        incognidium_style::ListStyleType::LowerLatin
                    ) || matches!(
                        marker_type,
                        incognidium_style::ListStyleType::UpperLatin
                    ) || matches!(&parent_node.data, NodeData::Element(ref pel) if pel.tag_name == "ol");
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
                        _ => "\u{2022} ".to_string(), // • (disc)
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
                        text: Some(marker.clone()),
                        source_text: Some(marker.clone()),
                        text_leading_space: false,
                        text_trailing_space: true,
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
                        forced_content_width: None,
                        forced_content_height: None,
                        forced_containing_height_for_children: None,
                        force_below_float: false,
                        force_line_break_before: false,
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
        if matches!(s.before_visibility, incognidium_style::Visibility::Visible) {
            let text =
                resolve_content_to_text(&s.before_content, counters, &s.quotes, 0, doc, node_id);
            if let Some(fake_id) = s.before_node_id {
                let pseudo_display = styles
                    .get(&fake_id)
                    .map(|ps| ps.display)
                    .unwrap_or(Display::Block);
                let pseudo_box_type = match pseudo_display {
                    Display::Flex => BoxType::Flex,
                    Display::InlineFlex => BoxType::InlineFlex,
                    Display::Grid => BoxType::Grid,
                    Display::InlineBlock => BoxType::InlineBlock,
                    Display::Inline => BoxType::Inline,
                    _ => BoxType::Block,
                };
                let mut pseudo_children = Vec::new();
                if let Some(ref t) = text {
                    pseudo_children.push(LayoutBox {
                        node_id: fake_id,
                        x: 0.0,
                        y: 0.0,
                        width: 0.0,
                        height: 0.0,
                        content_width: 0.0,
                        content_height: 0.0,
                        children: Vec::new(),
                        box_type: BoxType::Text,
                        text: Some(t.clone()),
                        source_text: Some(t.clone()),
                        text_leading_space: t.starts_with(char::is_whitespace),
                        text_trailing_space: t.ends_with(char::is_whitespace),
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
                        forced_content_width: None,
                        forced_content_height: None,
                        forced_containing_height_for_children: None,
                        force_below_float: false,
                        force_line_break_before: false,
                    });
                }
                children.insert(
                    0,
                    LayoutBox {
                        node_id: fake_id,
                        x: 0.0,
                        y: 0.0,
                        width: 0.0,
                        height: 0.0,
                        content_width: 0.0,
                        content_height: 0.0,
                        children: pseudo_children,
                        box_type: pseudo_box_type,
                        text: None,
                        source_text: None,
                        text_leading_space: false,
                        text_trailing_space: false,
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
                        forced_content_width: None,
                        forced_content_height: None,
                        forced_containing_height_for_children: None,
                        force_below_float: false,
                        force_line_break_before: false,
                    },
                );
            } else if let Some(t) = text {
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
                        text: Some(t.clone()),
                        source_text: Some(t.clone()),
                        text_leading_space: t.starts_with(char::is_whitespace),
                        text_trailing_space: t.ends_with(char::is_whitespace),
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
                        forced_content_width: None,
                        forced_content_height: None,
                        forced_containing_height_for_children: None,
                        force_below_float: false,
                        force_line_break_before: false,
                    },
                );
            }
        }
    }

    // Add ::after pseudo-element content if present
    if let Some(s) = style {
        // Apply counter-increment for ::after BEFORE resolving content
        for (name, delta) in &s.after_counter_increment {
            counters.increment(name, *delta);
        }
        if matches!(s.after_visibility, incognidium_style::Visibility::Visible) {
            let text =
                resolve_content_to_text(&s.after_content, counters, &s.quotes, 0, doc, node_id);
            if let Some(fake_id) = s.after_node_id {
                let pseudo_display = styles
                    .get(&fake_id)
                    .map(|ps| ps.display)
                    .unwrap_or(Display::Block);
                let pseudo_box_type = match pseudo_display {
                    Display::Flex => BoxType::Flex,
                    Display::InlineFlex => BoxType::InlineFlex,
                    Display::Grid => BoxType::Grid,
                    Display::InlineBlock => BoxType::InlineBlock,
                    Display::Inline => BoxType::Inline,
                    _ => BoxType::Block,
                };
                let mut pseudo_children = Vec::new();
                if let Some(ref t) = text {
                    pseudo_children.push(LayoutBox {
                        node_id: fake_id,
                        x: 0.0,
                        y: 0.0,
                        width: 0.0,
                        height: 0.0,
                        content_width: 0.0,
                        content_height: 0.0,
                        children: Vec::new(),
                        box_type: BoxType::Text,
                        text: Some(t.clone()),
                        source_text: Some(t.clone()),
                        text_leading_space: t.starts_with(char::is_whitespace),
                        text_trailing_space: t.ends_with(char::is_whitespace),
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
                        forced_content_width: None,
                        forced_content_height: None,
                        forced_containing_height_for_children: None,
                        force_below_float: false,
                        force_line_break_before: false,
                    });
                }
                children.push(LayoutBox {
                    node_id: fake_id,
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                    content_width: 0.0,
                    content_height: 0.0,
                    children: pseudo_children,
                    box_type: pseudo_box_type,
                    text: None,
                    source_text: None,
                    text_leading_space: false,
                    text_trailing_space: false,
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
                    forced_content_width: None,
                    forced_content_height: None,
                    forced_containing_height_for_children: None,
                    force_below_float: false,
                    force_line_break_before: false,
                });
            } else if let Some(t) = text {
                children.push(LayoutBox {
                    node_id,
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                    content_width: 0.0,
                    content_height: 0.0,
                    children: Vec::new(),
                    box_type: BoxType::Text,
                    text: Some(t.clone()),
                    source_text: Some(t.clone()),
                    text_leading_space: t.starts_with(char::is_whitespace),
                    text_trailing_space: t.ends_with(char::is_whitespace),
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
                    forced_content_width: None,
                    forced_content_height: None,
                    forced_containing_height_for_children: None,
                    force_below_float: false,
                    force_line_break_before: false,
                });
            }
        }
    }

    // Whitespace-only text nodes are needed as inter-word gap markers when they sit
    // between other inline-level content, but should not create boxes on their own.
    let has_inline_sibling = children.iter().any(|c| {
        !is_whitespace_only_text(c) && is_inline_level_styled(c.box_type, styles, c.node_id)
    });
    if !has_inline_sibling {
        children.retain(|c| !(c.box_type == BoxType::Text && is_whitespace_only_text(c)));
    }

    // Check if element has visual styling even if empty (background, borders, explicit size)
    let has_visual_style = style
        .map(|s| {
            s.background_color.a > 0
                || !s.background_image.is_empty()
                || s.border_top_width > 0.0
                || s.border_bottom_width > 0.0
                || s.border_left_width > 0.0
                || s.border_right_width > 0.0
                || matches!(s.width, SizeValue::Px(_))
                || matches!(s.height, SizeValue::Px(_))
                || s.flex_grow > 0.0
                || !matches!(s.min_width, SizeValue::Auto | SizeValue::None)
                || !matches!(s.min_height, SizeValue::Auto | SizeValue::None)
        })
        .unwrap_or(false);

    // Collapse empty containers: block/flex/inline with no meaningful content
    // This prevents empty wrapper divs from taking up space when all their content is hidden
    let has_meaningful_content = if has_visual_style {
        true
    } else if text
        .as_deref()
        .map(|t| !is_collapsible_whitespace_only(t))
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
                    .map(|t| !is_collapsible_whitespace_only(t))
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

    // A floated child of an inline box is out of flow and lays out in the
    // nearest ancestor block container. The inline layout path has no float
    // placement and would stack the floated children vertically as if they
    // were blocks. Route the children through the float-aware block path.
    let box_type = if box_type == BoxType::Inline
        && children.iter().any(|c| {
            styles
                .get(&c.node_id)
                .map(|s| s.float != Float::None)
                .unwrap_or(false)
        }) {
        BoxType::Block
    } else {
        box_type
    };

    let effective_box_type = if (box_type == BoxType::Block
        || box_type == BoxType::InlineBlock
        || box_type == BoxType::Flex
        || box_type == BoxType::InlineFlex
        || box_type == BoxType::Grid
        || box_type == BoxType::Inline
        || box_type == BoxType::Contents)
        && !has_meaningful_content
    {
        BoxType::None
    } else {
        box_type
    };

    // An inline container inherits the leading/trailing whitespace of its
    // first/last in-flow text: layout_text strips that whitespace from the
    // text box it lands on, so the container must carry the flag for
    // inter-element gap logic (and intrinsic sizing) to see the space.
    let text_leading_space = text
        .as_ref()
        .map_or(false, |t| t.starts_with(char::is_whitespace))
        || (effective_box_type == BoxType::Inline
            && children.first().map_or(false, inline_content_leading_space));
    let text_trailing_space = text
        .as_ref()
        .map_or(false, |t| t.ends_with(char::is_whitespace))
        || (effective_box_type == BoxType::Inline
            && children.last().map_or(false, inline_content_trailing_space));
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
        text: text.clone(),
        source_text: text,
        text_leading_space,
        text_trailing_space,
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
        first_letter_len: if style
            .map(|s| {
                s.first_letter_color.is_some()
                    || s.first_letter_font_size.is_some()
                    || s.first_letter_font_weight.is_some()
            })
            .unwrap_or(false)
        {
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
        forced_content_width: None,
        forced_content_height: None,
        forced_containing_height_for_children: None,
        force_below_float: false,
        force_line_break_before: false,
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

/// Resolve a `left`/`right`/`top`/`bottom` size value to a pixel offset.
/// Percentages resolve against `content_size`; calc()/min()/max()/clamp() are
/// evaluated against `containing_size` and the element font size.
fn resolve_offset(
    value: &SizeValue,
    containing_size: f32,
    _content_size: f32,
    font_size: f32,
) -> Option<f32> {
    match value {
        SizeValue::Px(v) => Some(*v),
        // left/right/top/bottom percentages resolve against the containing block
        // size, not the element's own content size.
        SizeValue::Percent(p) => Some(containing_size * p / 100.0),
        SizeValue::Calc(_) | SizeValue::Min(_) | SizeValue::Max(_) | SizeValue::Clamp { .. } => {
            evaluate_size_value(value, containing_size, font_size)
        }
        _ => None,
    }
}

/// Layout an absolutely or fixed positioned element.
/// These elements are removed from normal flow and positioned relative to their containing block.
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

    // Fixed positioned boxes are positioned relative to the viewport, not the
    // layout parent. Use the viewport dimensions so percentage sizes and offsets
    // resolve against something sensible even when the normal-flow parent has no
    // height (e.g. a column flex container whose only children are fixed).
    let (containing_width, containing_height) = if cs.position == Position::Fixed {
        VIEWPORT_SIZE.with(|v| v.get())
    } else {
        (containing_width, containing_height)
    };

    let padding_left = cs.padding_left_px(containing_width);
    let padding_right = cs.padding_right_px(containing_width);
    let padding_top = cs.padding_top_px(containing_width);
    let padding_bottom = cs.padding_bottom_px(containing_width);
    let border_left = cs.border_left_width;
    let border_right = cs.border_right_width;
    let pb_width = padding_left + padding_right + border_left + border_right;
    let is_border_box = cs.box_sizing == incognidium_style::BoxSizing::BorderBox;

    // Determine whether the width property resolves to auto. Math functions are
    // definite when they can be evaluated; otherwise they fall back to auto.
    let is_auto_width = match cs.width {
        SizeValue::Auto | SizeValue::None => true,
        SizeValue::Calc(_) | SizeValue::Min(_) | SizeValue::Max(_) | SizeValue::Clamp { .. } => {
            evaluate_size_value(&cs.width, containing_width, cs.font_size).is_none()
        }
        _ => false,
    };

    // Helper: evaluate a length/percentage/math constraint against the
    // containing width. Returns NaN when the value should be ignored.
    let evaluate_constraint = |value: &SizeValue| -> f32 {
        match value {
            SizeValue::Px(v) => *v,
            SizeValue::Percent(p) => containing_width * p / 100.0,
            SizeValue::Calc(_)
            | SizeValue::Min(_)
            | SizeValue::Max(_)
            | SizeValue::Clamp { .. } => {
                evaluate_size_value(value, containing_width, cs.font_size).unwrap_or(f32::NAN)
            }
            _ => f32::NAN,
        }
    };

    // Helper: evaluate a length/percentage/math constraint against the
    // containing height. Used for min/max-height so that percentages resolve
    // against the block containing block, not the inline one.
    let evaluate_vertical_constraint = |value: &SizeValue| -> f32 {
        match value {
            SizeValue::Px(v) => *v,
            SizeValue::Percent(p) => containing_height * p / 100.0,
            SizeValue::Calc(_)
            | SizeValue::Min(_)
            | SizeValue::Max(_)
            | SizeValue::Clamp { .. } => {
                evaluate_size_value(value, containing_height, cs.font_size).unwrap_or(f32::NAN)
            }
            _ => f32::NAN,
        }
    };

    // First layout pass: resolve children against the original containing block
    // width. layout_block() derives the content width from style.width, so children
    // see the correct available space even when the box's own width is a
    // percentage or calc() expression. We dispatch to the correct layout function
    // based on box_type (an absolutely positioned flex container must be laid out
    // as flex, not as block).
    layout_absolute_pass(
        layout_box,
        styles,
        containing_width,
        containing_height,
        image_sizes,
    );

    // For definite widths, lock the final width to the used value implied by the
    // width property and max-width/min-width, honoring box-sizing. layout_block
    // may have computed a different total because it re-evaluates
    // percentages/calc against the passed containing width; overriding here
    // prevents double-application.
    if !is_auto_width {
        let mut target_width = match cs.width {
            SizeValue::Px(w) => w,
            SizeValue::Percent(p) => containing_width * p / 100.0,
            SizeValue::Calc(_)
            | SizeValue::Min(_)
            | SizeValue::Max(_)
            | SizeValue::Clamp { .. } => {
                evaluate_size_value(&cs.width, containing_width, cs.font_size)
                    .unwrap_or(containing_width)
            }
            _ => containing_width,
        };

        let max_w = evaluate_constraint(&cs.max_width);
        if !max_w.is_nan() && target_width > max_w {
            target_width = max_w;
        }
        let min_w = evaluate_constraint(&cs.min_width);
        if !min_w.is_nan() && target_width < min_w {
            target_width = min_w;
        }

        let (content_width, total_width) = if is_border_box {
            let total = target_width.max(0.0);
            let content = (total - pb_width).max(0.0);
            (content, total)
        } else {
            let content = target_width.max(0.0);
            let total = content + pb_width;
            (content, total)
        };

        layout_box.content_width = content_width;
        layout_box.width = total_width;
    }

    // For auto width, measure the intrinsic content width and, if it is smaller
    // than the available width, re-layout children with the shrink-to-fit width.
    // This is essential for absolutely positioned navigation bars whose children
    // are a row flex container.
    if is_auto_width {
        let intrinsic_content_width = calculate_intrinsic_width(layout_box, styles);
        if intrinsic_content_width > 0.0 && intrinsic_content_width < layout_box.content_width {
            let final_content_width = intrinsic_content_width;
            let final_total_width = final_content_width
                + cs.padding_left
                + cs.padding_right
                + cs.border_left_width
                + cs.border_right_width;
            layout_box.width = final_total_width;
            layout_box.content_width = final_content_width;

            // Second layout pass: children are laid out against the final width so
            // their positions reflect the real shrink-wrapped box.
            layout_absolute_pass(
                layout_box,
                styles,
                final_total_width,
                containing_height,
                image_sizes,
            );
        }
    }

    // Apply top/left/right/bottom positioning. Percentages resolve against the
    // containing block's content box; calc()/min()/max()/clamp() are evaluated.
    let content_w = containing_width
        - cs.padding_left
        - cs.padding_right
        - cs.border_left_width
        - cs.border_right_width;

    // When an absolutely positioned box has auto width and both horizontal insets
    // are specified, CSS stretches it to fill the space between those insets
    // (minus margins). Do the same for auto height with both vertical insets.
    // This fixes overlays that use `position:absolute; inset:0` but would otherwise
    // shrink-wrap to their content and sit beside the image.
    let left_resolved = resolve_offset(&cs.left, containing_width, content_w, cs.font_size);
    let right_resolved = resolve_offset(&cs.right, containing_width, content_w, cs.font_size);
    if is_auto_width && left_resolved.is_some() && right_resolved.is_some() {
        let mut total_width = (containing_width
            - left_resolved.unwrap()
            - right_resolved.unwrap()
            - cs.margin_left
            - cs.margin_right)
            .max(0.0);
        // Honor min-width / max-width on the stretched used size. Many image
        // cover hacks (e.g. article cards with large cover photos) use
        // `position:absolute; inset:-9999px; margin:auto; min/max-width:100%`.
        // Without this clamp the box is stretched to a huge width and its
        // negative offsets push it off-canvas.
        let max_w = evaluate_constraint(&cs.max_width);
        if !max_w.is_nan() && total_width > max_w {
            total_width = max_w;
        }
        let min_w = evaluate_constraint(&cs.min_width);
        if !min_w.is_nan() && total_width < min_w {
            total_width = min_w;
        }
        let content_width = (total_width
            - cs.padding_left
            - cs.padding_right
            - cs.border_left_width
            - cs.border_right_width)
            .max(0.0);
        if total_width != layout_box.width {
            layout_box.width = total_width;
            layout_box.content_width = content_width;
            // Re-layout children against the stretched content width so
            // percentage widths and text wrapping use the real available space.
            layout_absolute_pass(
                layout_box,
                styles,
                total_width,
                containing_height,
                image_sizes,
            );
        }
    }
    let top_resolved = resolve_offset(&cs.top, containing_height, containing_height, cs.font_size);
    let bottom_resolved = resolve_offset(
        &cs.bottom,
        containing_height,
        containing_height,
        cs.font_size,
    );
    let is_auto_height = matches!(cs.height, SizeValue::Auto | SizeValue::None);
    // Replaced elements (images, etc.) with auto height should preserve their
    // intrinsic aspect ratio, not be stretched to fill the vertical insets.
    let is_replaced = layout_box.box_type == BoxType::Image;
    if is_auto_height && top_resolved.is_some() && bottom_resolved.is_some() && !is_replaced {
        let mut total_height = (containing_height
            - top_resolved.unwrap()
            - bottom_resolved.unwrap()
            - cs.margin_top
            - cs.margin_bottom)
            .max(0.0);
        let max_h = evaluate_vertical_constraint(&cs.max_height);
        if !max_h.is_nan() && total_height > max_h {
            total_height = max_h;
        }
        let min_h = evaluate_vertical_constraint(&cs.min_height);
        if !min_h.is_nan() && total_height < min_h {
            total_height = min_h;
        }
        let content_height = (total_height
            - cs.padding_top
            - cs.padding_bottom
            - cs.border_top_width
            - cs.border_bottom_width)
            .max(0.0);
        if total_height != layout_box.height {
            // Re-layout children against the stretched height so percentage
            // heights use the real available space. Set the stretched height
            // before the pass so nested absolutely positioned children see
            // a definite containing-block height (otherwise they collapse to
            // the old zero height). Restore it afterwards because block layout
            // otherwise shrinks it to the children's natural height.
            layout_box.height = total_height;
            layout_box.content_height = content_height;
            layout_absolute_pass(
                layout_box,
                styles,
                layout_box.width,
                total_height,
                image_sizes,
            );
            layout_box.height = total_height;
            layout_box.content_height = content_height;
        }
    }

    // Helper to center an absolutely positioned box when both insets on an axis
    // are specified and at least one margin is `auto`. This is the common
    // `inset:-9999px; margin:auto;` object-fit cover pattern.
    let distribute_auto_margin = |inset_start: Option<f32>,
                                  inset_end: Option<f32>,
                                  margin_start_auto: bool,
                                  margin_end_auto: bool,
                                  margin_start: f32,
                                  margin_end: f32,
                                  box_size: f32,
                                  containing_size: f32|
     -> (f32, f32) {
        if let (Some(start), Some(end)) = (inset_start, inset_end) {
            let remaining = containing_size - start - end - box_size - margin_start - margin_end;
            if margin_start_auto && margin_end_auto {
                let half = remaining.max(0.0) / 2.0;
                (start + half, half)
            } else if margin_start_auto {
                let m = remaining.max(0.0);
                (start + m, m)
            } else if margin_end_auto {
                (start + margin_start, margin_start)
            } else {
                (start + margin_start, margin_start)
            }
        } else {
            (f32::NAN, margin_start)
        }
    };

    let (x, _margin_left_used) = distribute_auto_margin(
        left_resolved,
        right_resolved,
        cs.margin_left_auto,
        cs.margin_right_auto,
        cs.margin_left,
        cs.margin_right,
        layout_box.width,
        containing_width,
    );
    layout_box.x = if !x.is_nan() {
        x
    } else if let Some(v) = resolve_offset(&cs.left, containing_width, content_w, cs.font_size) {
        v + cs.margin_left
    } else if let Some(v) = resolve_offset(&cs.right, containing_width, content_w, cs.font_size) {
        (content_w - layout_box.width - v - cs.margin_right).max(0.0)
    } else {
        cs.margin_left
    };

    let (y, _margin_top_used) = distribute_auto_margin(
        top_resolved,
        bottom_resolved,
        cs.margin_top_auto,
        cs.margin_bottom_auto,
        cs.margin_top,
        cs.margin_bottom,
        layout_box.height,
        containing_height,
    );
    layout_box.y = if !y.is_nan() {
        y
    } else if let Some(v) =
        resolve_offset(&cs.top, containing_height, containing_height, cs.font_size)
    {
        v + cs.margin_top
    } else if let Some(v) = resolve_offset(
        &cs.bottom,
        containing_height,
        containing_height,
        cs.font_size,
    ) {
        (containing_height - layout_box.height - v - cs.margin_bottom).max(0.0)
    } else {
        cs.margin_top
    };
}

/// Run a single layout pass for an absolutely/fixed positioned box, dispatching to
/// the appropriate layout algorithm based on its box_type.
fn layout_absolute_pass(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    containing_height: f32,
    image_sizes: &ImageSizes,
) {
    match layout_box.box_type {
        BoxType::Block => {
            layout_block(
                layout_box,
                styles,
                containing_width,
                containing_height,
                image_sizes,
                FloatState::default(),
            );
        }
        BoxType::InlineBlock => {
            layout_inline_block(
                layout_box,
                styles,
                containing_width,
                containing_width,
                containing_height,
                image_sizes,
            );
        }
        BoxType::Inline => {
            layout_inline(
                layout_box,
                styles,
                containing_width,
                containing_height,
                image_sizes,
            );
        }
        BoxType::Flex | BoxType::InlineFlex => {
            layout_flex(
                layout_box,
                styles,
                containing_width,
                containing_height,
                image_sizes,
            );
        }
        BoxType::Grid => {
            layout_grid(
                layout_box,
                styles,
                containing_width,
                containing_height,
                image_sizes,
            );
        }
        BoxType::Columns => {
            layout_columns(
                layout_box,
                styles,
                containing_width,
                image_sizes,
                FloatState::default(),
            );
        }
        BoxType::Table => {
            layout_table(
                layout_box,
                styles,
                containing_width,
                image_sizes,
                FloatState::default(),
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
                FloatState::default(),
            );
        }
        BoxType::Text => {
            layout_text(layout_box, styles, containing_width);
        }
        BoxType::Image => {
            layout_image(
                layout_box,
                styles,
                containing_width,
                containing_height,
                image_sizes,
            );
        }
        BoxType::TableSection
        | BoxType::TableCaption
        | BoxType::LineBreak
        | BoxType::Contents
        | BoxType::None => {
            layout_block(
                layout_box,
                styles,
                containing_width,
                containing_height,
                image_sizes,
                FloatState::default(),
            );
        }
    }
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
        layout_block(
            layout_box,
            styles,
            containing_width,
            _containing_height,
            image_sizes,
            parent_floats,
        );
        return;
    }

    if style.position == Position::Absolute || style.position == Position::Fixed {
        layout_absolute(
            layout_box,
            styles,
            containing_width,
            _containing_height,
            image_sizes,
        );
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
            layout_inline_block(
                layout_box,
                styles,
                containing_width,
                containing_width,
                _containing_height,
                image_sizes,
            );
        }
        BoxType::Inline => {
            layout_inline(
                layout_box,
                styles,
                containing_width,
                _containing_height,
                image_sizes,
            );
        }
        BoxType::Flex => {
            layout_flex(
                layout_box,
                styles,
                containing_width,
                _containing_height,
                image_sizes,
            );
        }
        BoxType::InlineFlex => {
            layout_inline_flex(
                layout_box,
                styles,
                containing_width,
                _containing_height,
                image_sizes,
            );
        }
        BoxType::Grid => {
            layout_grid(
                layout_box,
                styles,
                containing_width,
                _containing_height,
                image_sizes,
            );
        }
        BoxType::Columns => {
            layout_columns(
                layout_box,
                styles,
                containing_width,
                image_sizes,
                parent_floats,
            );
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
            layout_block(
                layout_box,
                styles,
                containing_width,
                0.0,
                image_sizes,
                parent_floats,
            );
        }
        BoxType::Text => {
            layout_text(layout_box, styles, containing_width);
        }
        BoxType::Image => {
            layout_image(
                layout_box,
                styles,
                containing_width,
                _containing_height,
                image_sizes,
            );
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

/// Return the maximum bottom edge (relative to `offset_y`) of any floated
/// descendant that participates in the same block formatting context as
/// `layout_box`. Descendants that establish their own BFC (overflow not visible,
/// flex/grid/table, floats themselves, positioned boxes, or clearfix pseudo-
/// elements) are not recursed into, because they already enclose their own
/// floats. This lets a clearfixed ancestor such as `.promo` grow tall enough
/// to contain nested floated media inside a plain `display:block`
/// wrapper.
fn max_float_bottom_within_bfc(
    layout_box: &LayoutBox,
    styles: &StyleMap,
    offset_y: f32,
) -> (f32, bool) {
    let mut max_bottom = 0.0_f32;
    let mut found = false;

    for child in &layout_box.children {
        let cs = styles.get(&child.node_id).cloned().unwrap_or_default();
        if cs.position == Position::Absolute || cs.position == Position::Fixed {
            continue;
        }
        if child.box_type == BoxType::None || child.box_type == BoxType::Contents {
            continue;
        }
        if cs.float != Float::None {
            let bottom = offset_y + child.y + child.height + cs.margin_bottom;
            if bottom > max_bottom {
                max_bottom = bottom;
            }
            found = true;
            continue;
        }
        // A clearfix pseudo creates a BFC at this descendant, so its floats are
        // already enclosed by its own auto height.
        let after_is_whitespace_only = matches!(
            cs.after_content,
            incognidium_style::Content::Text(ref t) if t.trim().is_empty()
        );
        let has_clearfix_pseudo =
            after_is_whitespace_only && matches!(cs.after_visibility, Visibility::Visible);
        if cs.establishes_bfc() || has_clearfix_pseudo {
            continue;
        }
        let (child_max, child_found) =
            max_float_bottom_within_bfc(child, styles, offset_y + child.y);
        if child_found {
            if child_max > max_bottom {
                max_bottom = child_max;
            }
            found = true;
        }
    }

    (max_bottom, found)
}

/// A floated box found inside a laid-out block subtree, expressed in that
/// block's border-box coordinate space (margins included in the span).
struct EscapedFloat {
    left: bool,
    x0: f32,
    x1: f32,
    top: f32,
    bottom: f32,
}

/// Collect the floated boxes inside a laid-out block subtree, mirroring the
/// descent rules of `max_float_bottom_within_bfc`: floats inside
/// BFC-establishing or clearfixed descendants are enclosed by them and never
/// escape to intrude on surrounding content.
fn collect_floats_within(
    layout_box: &LayoutBox,
    styles: &StyleMap,
    offset_x: f32,
    offset_y: f32,
    out: &mut Vec<EscapedFloat>,
) {
    for child in &layout_box.children {
        let cs = styles.get(&child.node_id).cloned().unwrap_or_default();
        if cs.position == Position::Absolute || cs.position == Position::Fixed {
            continue;
        }
        if child.box_type == BoxType::None || child.box_type == BoxType::Contents {
            continue;
        }
        if cs.float != Float::None {
            let x0 = offset_x + child.x - cs.margin_left;
            let x1 = offset_x + child.x + child.width + cs.margin_right;
            out.push(EscapedFloat {
                left: cs.float == Float::Left,
                x0,
                x1,
                top: offset_y + child.y,
                bottom: offset_y + child.y + child.height + cs.margin_bottom,
            });
            continue;
        }
        // A clearfix pseudo creates a BFC at this descendant, so its floats are
        // already enclosed by its own auto height.
        let after_is_whitespace_only = matches!(
            cs.after_content,
            incognidium_style::Content::Text(ref t) if t.trim().is_empty()
        );
        let has_clearfix_pseudo =
            after_is_whitespace_only && matches!(cs.after_visibility, Visibility::Visible);
        if cs.establishes_bfc() || has_clearfix_pseudo {
            continue;
        }
        collect_floats_within(child, styles, offset_x + child.x, offset_y + child.y, out);
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
    let padding_left = style.padding_left_px(containing_width);
    let padding_right = style.padding_right_px(containing_width);
    let padding_top = style.padding_top_px(containing_width);
    let padding_bottom = style.padding_bottom_px(containing_width);
    let border_left = style.border_left_width;
    let border_right = style.border_right_width;

    let is_border_box = style.box_sizing == incognidium_style::BoxSizing::BorderBox;
    let pb_width = padding_left + padding_right + border_left + border_right;

    let mut content_width;
    let mut total_width;

    if let Some(forced) = layout_box.forced_content_width.take() {
        // Caller has already resolved width/max-width/min-width and wants the
        // content box to be exactly this wide. This is used by flex layout so a
        // flex item can be sized against the flex container while its children
        // are laid out against the item's own content width.
        content_width = forced.max(0.0);
        total_width = content_width + pb_width;
    } else {
        content_width = match style.width {
            SizeValue::Px(w) => {
                if is_border_box {
                    (w - pb_width).max(0.0)
                } else {
                    w
                }
            }
            SizeValue::Percent(p) => {
                let total = containing_width * p / 100.0;
                if is_border_box {
                    (total - pb_width).max(0.0)
                } else {
                    total
                }
            }
            SizeValue::Auto | SizeValue::None => {
                let mut width = (containing_width - margin_left - margin_right - pb_width).max(0.0);
                // If the box has a definite height and an aspect-ratio, derive the
                // content-box width from the ratio instead of measuring children.
                // This mirrors the intrinsic-width path used by flex layout.
                if !matches!(style.width, SizeValue::Px(_) | SizeValue::Percent(_)) {
                    if let Some(ref ar) = style.aspect_ratio {
                        if let Some(content_box_height) = definite_content_box_height(
                            &style,
                            containing_width,
                            layout_box.content_height,
                        ) {
                            let ratio = ar.width / ar.height.max(0.001);
                            if ratio > 0.0 && content_box_height > 0.0 {
                                width = (content_box_height * ratio).max(0.0);
                            }
                        }
                    }
                }
                width
            }
            // CSS Math Functions - evaluate with containing block context.
            // When box-sizing is border-box, the math expression sets the total
            // border-box width, so subtract padding/border to get the content width.
            SizeValue::Calc(ref expr) => {
                let total = evaluate_size_value(
                    &SizeValue::Calc(expr.clone()),
                    containing_width,
                    style.font_size,
                )
                .unwrap_or(containing_width);
                if is_border_box {
                    (total - pb_width).max(0.0)
                } else {
                    total
                }
            }
            SizeValue::Min(ref vals) => {
                let total = evaluate_size_value(
                    &SizeValue::Min(vals.clone()),
                    containing_width,
                    style.font_size,
                )
                .unwrap_or(containing_width);
                if is_border_box {
                    (total - pb_width).max(0.0)
                } else {
                    total
                }
            }
            SizeValue::Max(ref vals) => {
                let total = evaluate_size_value(
                    &SizeValue::Max(vals.clone()),
                    containing_width,
                    style.font_size,
                )
                .unwrap_or(containing_width);
                if is_border_box {
                    (total - pb_width).max(0.0)
                } else {
                    total
                }
            }
            SizeValue::Clamp {
                ref min,
                ref val,
                ref max,
            } => {
                let total = evaluate_size_value(
                    &SizeValue::Clamp {
                        min: min.clone(),
                        val: val.clone(),
                        max: max.clone(),
                    },
                    containing_width,
                    style.font_size,
                )
                .unwrap_or(containing_width);
                if is_border_box {
                    (total - pb_width).max(0.0)
                } else {
                    total
                }
            }
            // CSS Intrinsic Sizing - treat as auto for now (content-based sizing requires multi-pass)
            SizeValue::MinContent | SizeValue::MaxContent | SizeValue::FitContent => {
                // For now, use available width; proper implementation would measure content
                (containing_width - margin_left - margin_right - pb_width).max(0.0)
            }
        };

        // Start with the total width implied by the content width and padding/border.
        total_width = content_width + pb_width;

        // Apply max-width/min-width to the dimension they actually constrain.
        // For box-sizing: border-box the limits apply to the total border-box
        // width; for content-box they apply to the content width.
        match style.max_width {
            SizeValue::Px(mw) => {
                if is_border_box && total_width > mw {
                    total_width = mw;
                } else if !is_border_box && content_width > mw {
                    content_width = mw;
                }
            }
            SizeValue::Percent(p) => {
                let mw = containing_width * p / 100.0;
                if is_border_box && total_width > mw {
                    total_width = mw;
                } else if !is_border_box && content_width > mw {
                    content_width = mw;
                }
            }
            SizeValue::Calc(_)
            | SizeValue::Min(_)
            | SizeValue::Max(_)
            | SizeValue::Clamp { .. } => {
                if let Some(mw) =
                    evaluate_size_value(&style.max_width, containing_width, style.font_size)
                {
                    if is_border_box && total_width > mw {
                        total_width = mw;
                    } else if !is_border_box && content_width > mw {
                        content_width = mw;
                    }
                }
            }
            _ => {}
        }

        match style.min_width {
            SizeValue::Px(mw) => {
                if is_border_box && total_width < mw {
                    total_width = mw;
                } else if !is_border_box && content_width < mw {
                    content_width = mw;
                }
            }
            SizeValue::Percent(p) => {
                let mw = containing_width * p / 100.0;
                if is_border_box && total_width < mw {
                    total_width = mw;
                } else if !is_border_box && content_width < mw {
                    content_width = mw;
                }
            }
            SizeValue::Calc(_)
            | SizeValue::Min(_)
            | SizeValue::Max(_)
            | SizeValue::Clamp { .. } => {
                if let Some(mw) =
                    evaluate_size_value(&style.min_width, containing_width, style.font_size)
                {
                    if is_border_box && total_width < mw {
                        total_width = mw;
                    } else if !is_border_box && content_width < mw {
                        content_width = mw;
                    }
                }
            }
            _ => {}
        }

        // Synchronize content/total after min/max-width constraints.
        if is_border_box {
            content_width = (total_width - pb_width).max(0.0);
        } else {
            total_width = content_width + pb_width;
        }
    }

    layout_box.content_width = content_width.max(0.0);
    layout_box.width = total_width;

    // When a block's used content width is significantly narrower than the full
    // space available in its containing block, it has been sized by an explicit
    // width (or a max-width clamp) rather than expanding to fill the line.
    // In that case it is effectively a separate column beside any floats, and
    // its inline children should wrap within the full content box instead of
    // being further shortened by the float. This prevents text from being laid
    // out as a single unwrapped line when a wide right float leaves only a
    // tiny sliver of "available" inline width.
    let full_available_content =
        (containing_width - margin_left - margin_right - pb_width).max(0.0);
    let inline_ignores_floats = layout_box.content_width + 1.0 < full_available_content;
    // Calculate explicit height early so it can be passed to children
    // This allows percentage heights on children to work when parent has explicit height
    let is_root = ROOT_NODE_ID.with(|r| r.get() == Some(layout_box.node_id));
    let explicit_height = match style.height {
        SizeValue::Px(h) => Some(h),
        SizeValue::Percent(p) if containing_height > 0.0 => Some(containing_height * p / 100.0),
        SizeValue::Calc(_) | SizeValue::Min(_) | SizeValue::Max(_) | SizeValue::Clamp { .. } => {
            evaluate_size_value(&style.height, containing_height, style.font_size)
        }
        _ if is_root
            && containing_height > 0.0
            && matches!(style.height, SizeValue::Auto | SizeValue::None) =>
        {
            // The document root is the initial containing block. When it has no
            // explicit height of its own, still give its children the viewport
            // height so html/body { height: 100% } resolves correctly.
            Some(containing_height)
        }
        _ => None,
    };

    // A stretched grid item may have supplied a definite content height that
    // must be used for its own sizing and for resolving percentage heights on
    // its children, even when its CSS height is auto.
    let forced_height = layout_box.forced_content_height.take();
    let effective_height = forced_height.or(explicit_height);

    // Layout children
    let child_containing_width = layout_box.content_width;
    let child_containing_height = if let Some(h) = effective_height {
        h
    } else if let Some(ref ar) = style.aspect_ratio {
        // A block with an explicit aspect-ratio and auto height gets a definite
        // used height from the ratio. Pass that height to children so
        // percentage heights on replaced elements (e.g. object-fit cover
        // images) resolve against the ratio-derived box, not the intrinsic
        // content height.
        let ratio = ar.width / ar.height.max(0.001);
        if ratio > 0.0 && layout_box.content_width > 0.0 {
            let ratio_height = layout_box.content_width / ratio;
            // Clamp by explicit min/max-height constraints if they are available.
            let ratio_height = match style.max_height {
                SizeValue::Px(mh) if ratio_height > mh => mh,
                _ => ratio_height,
            };
            let ratio_height = match style.min_height {
                SizeValue::Px(mh) if ratio_height < mh => mh,
                _ => ratio_height,
            };
            ratio_height
        } else {
            0.0
        }
    } else {
        0.0
    };

    let mut cursor_y: f32 = padding_top + style.border_top_width;
    let content_x = padding_left + border_left;
    // Track previous child's margin-bottom for margin collapse
    let mut prev_margin_bottom: f32 = 0.0;

    // If this block is a definite-width column beside a float, do not let that
    // float intrude into its descendants. The children already live inside the
    // dedicated column width, so treating the float as an additional intrusion
    // would leave no inline space and force text into a single unwrapped line.
    let mut float_right_width: f32 = if inline_ignores_floats {
        0.0
    } else {
        parent_floats.right_width
    };
    let mut float_left_width: f32 = if inline_ignores_floats {
        0.0
    } else {
        parent_floats.left_width
    };
    let mut float_bottom: f32 = if parent_floats.remaining_height > 0.0 {
        padding_top + style.border_top_width + parent_floats.remaining_height
    } else {
        0.0
    };
    // Track the top of the earliest float that is still active in this block
    // formatting context. When a later inline run follows a cleared float, the
    // run can start back at this top and wrap around the whole stack of floats
    // instead of being forced to start below the most recently placed one.
    let mut active_float_top: f32 = cursor_y;

    // Collect indices of absolutely positioned children
    // All absolute/fixed positioned elements are removed from normal flow
    let abs_indices: Vec<usize> = layout_box
        .children
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            let cs = styles.get(&c.node_id).cloned().unwrap_or_default();
            cs.position == Position::Absolute || cs.position == Position::Fixed
        })
        .map(|(i, _)| i)
        .collect();

    // Separate inline and block children
    let mut i = 0;
    let mut first_inline_run = true;
    let mut pending_float_line_top: Option<f32> = None;
    // Track the last inline run so a following float can reduce the available
    // width and prevent text-align from shifting inline elements into the float.
    #[derive(Clone, Copy)]
    struct LastInlineRun {
        line_begin: usize,
        end: usize,
        used_width: f32,
        inline_available: f32,
        inline_x_start: f32,
        // Total width already stolen from this run's line by floats that
        // followed it; the run's boxes have been shifted over by this much.
        shift: f32,
        // The y where this run's line started; floats placed at this y are
        // beside the run, floats placed lower are below it.
        line_top: f32,
    }
    let mut last_inline_run: Option<LastInlineRun> = None;
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
                active_float_top = cursor_y;
            }
            let mut inline_available = if inline_ignores_floats {
                child_containing_width
            } else {
                (child_containing_width - float_right_width - float_left_width).max(0.0)
            };
            let mut inline_x_start = if inline_ignores_floats {
                content_x
            } else {
                content_x + float_left_width
            };

            let mut line_start = i;
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
                // Skip absolutely positioned children from inline flow; they are
                // laid out and positioned later by the absolute-positioning pass.
                if abs_indices.contains(&i) {
                    if i == line_start {
                        // Leading absolute child: drop it from the start of this run.
                        line_start = i + 1;
                        i += 1;
                        continue;
                    } else {
                        // Absolute child in the middle of a run ends the run here so
                        // it does not inflate the line box height/size.
                        break;
                    }
                }
                let c = &layout_box.children[i];
                if !is_inline_level_styled(c.box_type, styles, c.node_id) {
                    break;
                }
                // Inline-block and inline-flex boxes establish their own containing
                // block, so percentage widths/padding resolve against the parent
                // block's full content width. The reduced `inline_available` is only
                // the line fragment beside any floats; passing it as the containing
                // width would shrink percentage-width inline-blocks (e.g. a 65% wide
                // grid) to the float-free sliver. Use the full containing block for
                // percentage resolution, while still limiting auto-width shrink-to-fit
                // to the actual line fragment.
                let is_inline_blockish =
                    matches!(c.box_type, BoxType::InlineBlock | BoxType::InlineFlex);
                if is_inline_blockish {
                    layout_inline_block(
                        &mut layout_box.children[i],
                        styles,
                        child_containing_width,
                        inline_available,
                        child_containing_height,
                        image_sizes,
                    );
                } else {
                    compute_layout(
                        &mut layout_box.children[i],
                        styles,
                        inline_available,
                        child_containing_height,
                        image_sizes,
                    );
                }
                i += 1;
            }

            // If every child in this inline run was absolutely positioned, there
            // is no normal-flow line to advance the cursor.
            if line_start >= i {
                continue;
            }

            // First pass: identify and mark line break elements (br tags)
            for j in line_start..i {
                if layout_box.children[j].box_type == BoxType::LineBreak {
                    layout_box.children[j].width = 0.0;
                    layout_box.children[j].height = 0.0;
                }
            }

            // Skip inline runs that consist only of whitespace text nodes
            // (whitespace between block elements should not take up space).
            // Absolutely positioned children are removed from normal flow and are
            // laid out by a separate pass, so they should not prevent skipping.
            let all_whitespace = (line_start..i).all(|j| {
                abs_indices.contains(&j)
                    || (matches!(layout_box.children[j].box_type, BoxType::Text)
                        && layout_box.children[j]
                            .text
                            .as_deref()
                            .map(is_collapsible_whitespace_only)
                            .unwrap_or(false))
            });
            if all_whitespace {
                continue;
            }

            // Text that wraps around a float may start beside the float with a
            // reduced line-box width and then continue below the float using the
            // full container width. Split such text boxes so the inline run can
            // place the first fragment beside the float and the second fragment on
            // the now-full-width line(s) below it.
            let mut extra_children = 0usize;
            for j in (line_start..i).rev() {
                if layout_box.children[j].box_type != BoxType::Text {
                    continue;
                }
                if cursor_y + layout_box.children[j].height <= float_bottom + 0.5 {
                    continue;
                }
                let fragments = split_text_at_float_boundary(
                    &layout_box.children[j],
                    styles,
                    inline_available,
                    child_containing_width,
                    cursor_y,
                    float_bottom,
                );
                let fragment_count = fragments.len();
                if fragment_count > 1 {
                    let new_count = fragment_count - 1;
                    layout_box.children.remove(j);
                    for (offset, fragment) in fragments.into_iter().enumerate() {
                        layout_box.children.insert(j + offset, fragment);
                    }
                    extra_children += new_count;
                }
            }
            if extra_children > 0 {
                i += extra_children;
            }

            // A wrapping text child that straddles a line boundary must fill the
            // remaining width of the line it starts on and continue on the
            // following lines, whatever precedes it in the run (inline elements,
            // images, or earlier text). Simulate the line placement below and
            // split every text child that does not fit in the space left on its
            // starting line; without this the whole text node moves to the next
            // line and leaves a ragged gap after the preceding content.
            let mut sim_gaps = compute_inline_gaps(&layout_box.children, line_start, i, styles);
            let mut line_used: f32 = if first_inline_run {
                style.text_indent
            } else {
                0.0
            };
            // A nonzero text indent counts as line content for the break test,
            // mirroring `line_x > inline_x_start` in the placement loop.
            let mut sim_line_has_content = first_inline_run && style.text_indent > 0.0;
            // Line width available to the simulation. A fragment that continues
            // below the active float switches from the float-side sliver back to
            // the full containing width, like the placement loop does.
            let mut sim_available = inline_available;
            let floats_active = float_left_width > 0.0 || float_right_width > 0.0;
            // Simulated cursor y: like the placement loop, once the simulated
            // lines advance past the float bottom the remaining lines leave the
            // float-side sliver and use the full containing width. Without this
            // the simulation keeps splitting text at the narrow width for lines
            // that placement actually lays out at full width.
            let mut sim_cursor_y = cursor_y;
            let mut j = line_start;
            while j < i {
                let child = &layout_box.children[j];
                if child.box_type == BoxType::LineBreak {
                    line_used = 0.0;
                    sim_line_has_content = false;
                    sim_cursor_y += css_line_height;
                    if floats_active && sim_cursor_y >= float_bottom {
                        sim_available = child_containing_width;
                    }
                    j += 1;
                    continue;
                }
                if child.force_below_float && floats_active {
                    // This fragment starts on a fresh line below the float and
                    // uses the full containing width; it is never split here.
                    line_used = 0.0;
                    sim_line_has_content = false;
                    sim_available = child_containing_width;
                    sim_cursor_y = sim_cursor_y.max(float_bottom);
                    line_used += child.width;
                    if child.width > 0.0 {
                        sim_line_has_content = true;
                    }
                    j += 1;
                    continue;
                }
                let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
                let child_nowrap = matches!(
                    child_style.white_space,
                    incognidium_style::WhiteSpace::NoWrap | incognidium_style::WhiteSpace::Pre
                ) || matches!(child_style.text_wrap, TextWrap::NoWrap);
                if sim_line_has_content {
                    line_used += sim_gaps[j - line_start];
                }
                let child_total = child_style.margin_left + child.width + child_style.margin_right;
                // A multi-line text child placed after other inline content would
                // keep its first-line x for every internal line, indenting the
                // wrapped lines into the middle of the container. It must be split
                // so only its first line stays on this line and the remainder
                // starts fresh at the container's left edge.
                let child_is_multiline_text = child.box_type == BoxType::Text
                    && child
                        .text
                        .as_deref()
                        .map(|t| t.contains('\n'))
                        .unwrap_or(false);
                let straddles = sim_line_has_content
                    && !child_nowrap
                    && !is_whitespace_only_text(child)
                    && (line_used + child_total > sim_available + 0.5 || child_is_multiline_text);
                if straddles && child.box_type == BoxType::Text {
                    let first_line_width = (sim_available - line_used).max(0.0);
                    let fragments = split_text_at_first_line_width(
                        &layout_box.children[j],
                        styles,
                        first_line_width,
                        sim_available,
                    );
                    if fragments.len() > 1 {
                        let new_count = fragments.len() - 1;
                        layout_box.children.remove(j);
                        for (offset, fragment) in fragments.into_iter().enumerate() {
                            layout_box.children.insert(j + offset, fragment);
                        }
                        i += new_count;
                        // First fragment stays on this line, then the line breaks;
                        // the remainder starts the next line.
                        line_used = 0.0;
                        sim_line_has_content = false;
                        sim_cursor_y += css_line_height;
                        if floats_active && sim_cursor_y >= float_bottom {
                            sim_available = child_containing_width;
                        }
                        sim_gaps = compute_inline_gaps(&layout_box.children, line_start, i, styles);
                        j += 1;
                        continue;
                    }
                }
                if straddles {
                    // Non-text child (or unsplittable text): the whole child
                    // moves to the next line, as the placement loop does. Re-check
                    // it on the fresh line without advancing.
                    line_used = 0.0;
                    sim_line_has_content = false;
                    sim_cursor_y += css_line_height;
                    if floats_active && sim_cursor_y >= float_bottom {
                        sim_available = child_containing_width;
                    }
                    continue;
                }
                line_used += child_total;
                if child.width > 0.0 {
                    sim_line_has_content = true;
                }
                j += 1;
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
            let mut line_has_content = false;
            for j in line_start..i {
                // Text fragments produced by splitting around a float must start
                // on a fresh line below the float so they can use the full
                // containing width instead of the shortened float-side sliver.
                if layout_box.children[j].force_below_float
                    && (float_left_width > 0.0 || float_right_width > 0.0)
                {
                    // Finalize the line that precedes this fragment.
                    if line_begin < j {
                        apply_text_align(
                            &mut layout_box.children,
                            line_begin,
                            j,
                            line_x - inline_x_start,
                            inline_available,
                            &style,
                            false,
                        );
                        let line_top = cursor_y;
                        let line_bottom = apply_vertical_align(
                            &mut layout_box.children,
                            line_begin,
                            j,
                            line_top,
                            line_height,
                            css_line_height,
                            child_containing_width,
                            styles,
                        );
                        cursor_y += line_bottom.max(line_height);
                        line_height = css_line_height;
                    }
                    // Move below the active float and clear its intrusion.
                    cursor_y = cursor_y.max(float_bottom);
                    float_left_width = 0.0;
                    float_right_width = 0.0;
                    active_float_top = cursor_y;
                    inline_available = child_containing_width;
                    inline_x_start = content_x;
                    line_x = inline_x_start;
                    line_begin = j;
                    line_has_content = false;
                } else if layout_box.children[j].force_line_break_before {
                    // The remainder of a split text box starts a fresh line at
                    // the container's left edge: the split decided the preceding
                    // line was full, so placement must not squeeze this fragment
                    // onto the same line as the preceding fragment.
                    if line_begin < j {
                        apply_text_align(
                            &mut layout_box.children,
                            line_begin,
                            j,
                            line_x - inline_x_start,
                            inline_available,
                            &style,
                            false,
                        );
                        let line_top = cursor_y;
                        let line_bottom = apply_vertical_align(
                            &mut layout_box.children,
                            line_begin,
                            j,
                            line_top,
                            line_height,
                            css_line_height,
                            child_containing_width,
                            styles,
                        );
                        cursor_y += line_bottom.max(line_height);
                        line_height = css_line_height;
                    }
                    line_x = inline_x_start;
                    line_begin = j;
                    line_has_content = false;
                } else if cursor_y >= float_bottom
                    && (float_left_width > 0.0 || float_right_width > 0.0)
                {
                    // A previous inline child (or text fragment) advanced us past
                    // the active float; clear the float intrusion so this child can
                    // use the full line width. Fragments already placed on the
                    // current line must keep their positions: restarting the line
                    // at the left edge would paint the remaining children on top
                    // of them, so only a line with nothing placed yet restarts.
                    float_left_width = 0.0;
                    float_right_width = 0.0;
                    active_float_top = cursor_y;
                    inline_available = child_containing_width;
                    inline_x_start = content_x;
                    if line_begin == j {
                        line_x = inline_x_start;
                        line_begin = j;
                        line_has_content = false;
                    }
                }

                let gap = gaps[j - line_start];
                if line_has_content {
                    line_x += gap;
                }

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
                    let line_top = cursor_y;
                    let line_bottom = apply_vertical_align(
                        &mut layout_box.children,
                        line_begin,
                        j,
                        line_top,
                        line_height,
                        css_line_height,
                        child_containing_width,
                        styles,
                    );
                    cursor_y += line_bottom.max(line_height);
                    line_x = inline_x_start;
                    line_height = 0.0;
                    line_begin = j;
                    line_has_content = false;
                    if cursor_y >= float_bottom {
                        float_right_width = 0.0;
                        float_left_width = 0.0;
                        active_float_top = cursor_y;
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
                    // Position outside marker to the left of the content area.
                    // The marker is positioned outside the principal box; it may
                    // extend into the parent’s padding area (e.g. <ul> padding).
                    // Do not clamp to content_x so the marker can sit in that space.
                    let marker_width = layout_box.children[j].width + 5.0;
                    layout_box.children[j].x = content_x - marker_width;
                    layout_box.children[j].y = cursor_y;
                    // Outside markers must not consume content width or affect line
                    // breaking for the principal box.
                    line_height = line_height.max(child_height);
                    if child_width > 0.0 {
                        line_has_content = true;
                    }
                } else {
                    layout_box.children[j].x = line_x + margin_left;
                    layout_box.children[j].y = cursor_y;
                    line_x += margin_left + child_width + margin_right;
                    // Line height is the max of CSS line-height and tallest element on the line
                    line_height = line_height.max(child_height);
                    if child_width > 0.0 {
                        line_has_content = true;
                    }
                }
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

            // Remember the last inline run so a following float can shrink the
            // line box and re-align the inline elements.
            last_inline_run = Some(LastInlineRun {
                line_begin,
                end: i,
                used_width: line_x - inline_x_start,
                inline_available,
                inline_x_start,
                shift: 0.0,
                // cursor_y is still the top of this line at this point; it is
                // advanced past the line just below.
                line_top: cursor_y,
            });

            // Apply vertical-align to the last line and advance by its actual
            // line bottom, so mixed-height inline content (e.g. a padded
            // inline-block badge next to headline text) shares a common baseline.
            let line_top = cursor_y;
            let line_bottom = apply_vertical_align(
                &mut layout_box.children,
                line_begin,
                i,
                line_top,
                line_height,
                css_line_height,
                child_containing_width,
                styles,
            );
            let line_top_before_advance = cursor_y;
            cursor_y += line_bottom.max(line_height);
            pending_float_line_top = Some(line_top_before_advance);

            // Also expand the parent block height to cover this inline run if it
            // protrudes below the current block content bottom. This keeps p/figure
            // wrappers from reporting zero or clipped height when their only child
            // is a tall image.
            let run_bottom = cursor_y;
            let content_bottom = layout_box.content_height + padding_top + style.border_top_width;
            if run_bottom > content_bottom {
                layout_box.content_height = run_bottom - padding_top - style.border_top_width;
            }
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
                        if cm.float == Float::None {
                            active_float_top = cursor_y;
                        }
                    }
                    incognidium_style::Clear::Right if float_right_width > 0.0 => {
                        cursor_y = float_bottom;
                        float_right_width = 0.0;
                        if cm.float == Float::None {
                            active_float_top = cursor_y;
                        }
                    }
                    incognidium_style::Clear::Both => {
                        cursor_y = float_bottom;
                        float_left_width = 0.0;
                        float_right_width = 0.0;
                        if cm.float == Float::None {
                            active_float_top = cursor_y;
                        }
                    }
                    _ => {}
                }
            }

            // Clear floats if cursor is past float bottom. When the child is
            // itself a float that will be placed at the pending inline line
            // top, the active floats still bound that line, so keep their
            // accumulated widths for stacking beside them.
            let float_wraps_to_line_top = cm.float != Float::None
                && pending_float_line_top.map_or(false, |top| top < float_bottom);
            if cursor_y >= float_bottom && !float_wraps_to_line_top {
                float_right_width = 0.0;
                float_left_width = 0.0;
                if cm.float == Float::None || cm.clear == incognidium_style::Clear::None {
                    active_float_top = cursor_y;
                }
            }

            // Handle floated elements
            if cm.float != Float::None {
                let (float_content_width, is_auto_float_width) = match cm.width {
                    SizeValue::Px(w) => (w, false),
                    SizeValue::Percent(_) => {
                        // Percentage widths on floats resolve against the containing
                        // block. Pass the full containing width to compute_layout so
                        // the child's own width percentage is applied exactly once.
                        (child_containing_width, false)
                    }
                    SizeValue::Calc(_)
                    | SizeValue::Min(_)
                    | SizeValue::Max(_)
                    | SizeValue::Clamp { .. } => {
                        // CSS math functions (calc/min/max/clamp) resolve against the
                        // float's containing block, just like percentage widths. Pass the
                        // full containing width to compute_layout so percentages inside the
                        // expression are evaluated exactly once. layout_block will apply
                        // the expression and honor box-sizing from there.
                        (child_containing_width, false)
                    }
                    _ => {
                        // Auto width: shrink-wrap to content (intrinsic width).
                        // For floats, we need to calculate the minimum width needed
                        // to contain the content without unnecessary wrapping.

                        // First compute layout at generous width to get text measurements.
                        // Pass the full containing width (do not subtract margins here);
                        // layout_block will subtract the float's own margins once when
                        // computing its content width.
                        compute_layout(
                            &mut layout_box.children[i],
                            styles,
                            child_containing_width,
                            child_containing_height,
                            image_sizes,
                        );

                        // Then calculate intrinsic width from the laid out content.
                        // calculate_intrinsic_width now sums floated children
                        // horizontally and includes child padding/border, so a floated
                        // wrapper around several floated buttons gets the correct
                        // shrink-to-fit width.
                        let child_ref = &layout_box.children[i];
                        let intrinsic = calculate_intrinsic_width(child_ref, styles);
                        // Add padding and border to get total width
                        let total = intrinsic
                            + cm.padding_left_px(child_containing_width)
                            + cm.padding_right_px(child_containing_width)
                            + cm.border_left_width
                            + cm.border_right_width;
                        (total, true)
                    }
                };
                // For auto-width floats, float_content_width is the total box width
                // (padding/border) without the float's own margins. Add the margins
                // back before calling compute_layout so layout_block's content-width
                // subtraction leaves enough room for the children; otherwise a
                // right-margined float (e.g. a small login button) wraps and shrinks
                // below its content. For fixed/percentage widths, pass the value as-is.
                let final_layout_width = if is_auto_float_width {
                    float_content_width + cm.margin_left + cm.margin_right
                } else {
                    float_content_width
                };
                compute_layout(
                    &mut layout_box.children[i],
                    styles,
                    final_layout_width,
                    child_containing_height,
                    image_sizes,
                );

                // When a float follows an inline run, place it on the same line
                // as the inline content so inline items after it can wrap around it.
                let float_placed_after_inline = pending_float_line_top.is_some();
                let mut float_y = if let Some(top) = pending_float_line_top.take() {
                    top
                } else {
                    cursor_y
                };

                // Float wrapping: if the new float does not fit in the remaining
                // horizontal space beside earlier floats at the y where it will
                // be placed, drop it to the next line (clear both sides). This
                // keeps percentage-width grid columns from overflowing their row
                // when there are more than two per line.
                let child_total_width =
                    layout_box.children[i].width + cm.margin_left + cm.margin_right;
                let mut float_dropped_below_run = false;
                if float_y < float_bottom
                    && float_left_width + float_right_width + child_total_width
                        > child_containing_width + 0.5
                {
                    cursor_y = float_bottom;
                    float_left_width = 0.0;
                    float_right_width = 0.0;
                    float_y = cursor_y;
                    float_dropped_below_run = true;
                }

                // If a right float immediately follows an inline run, shrink the
                // line box used for text-align so inline elements don't overlap the
                // float. A float that dropped below the run no longer steals
                // width from the run's line.
                if cm.float == Float::Right {
                    let beside_run = last_inline_run
                        .map_or(false, |run| float_y <= run.line_top + 0.5)
                        && !float_dropped_below_run;
                    if let Some(last_run) = if beside_run {
                        last_inline_run.take()
                    } else {
                        last_inline_run = None;
                        None
                    } {
                        let float_total_width =
                            layout_box.children[i].width + cm.margin_left + cm.margin_right;
                        let new_container_width = last_run.inline_available - float_total_width;
                        // Only re-align if there is actually room for the float
                        // beside the run at all.
                        if new_container_width > 1.0 {
                            // Re-compute the effective text-align exactly as
                            // apply_text_align does, then shift the inline
                            // elements left by the amount that the float steals
                            // from the line box.  apply_text_align uses +=, so
                            // calling it a second time would double-shift.
                            let align = if style.text_align == TextAlign::Justify {
                                match style.text_align_last {
                                    TextAlignLast::Auto => TextAlign::Left,
                                    TextAlignLast::Left | TextAlignLast::Start => TextAlign::Left,
                                    TextAlignLast::Right | TextAlignLast::End => TextAlign::Right,
                                    TextAlignLast::Center => TextAlign::Center,
                                    TextAlignLast::Justify => TextAlign::Justify,
                                }
                            } else {
                                style.text_align
                            };
                            let adjustment = match align {
                                TextAlign::Right => float_total_width,
                                TextAlign::Center => float_total_width / 2.0,
                                _ => 0.0,
                            };
                            // The shift re-aligns the run within the
                            // float-narrowed line box, but never past the line's
                            // content edge: a run wider than the narrowed box
                            // sits flush against the float instead of spilling
                            // over it or hanging outside the container.
                            let run_min_x = (last_run.line_begin..last_run.end)
                                .filter(|&j| {
                                    !(layout_box.children[j].is_list_marker
                                        && layout_box.children[j].list_style_position
                                            == ListStylePosition::Outside)
                                })
                                .map(|j| layout_box.children[j].x)
                                .fold(f32::INFINITY, f32::min);
                            let adjustment = adjustment.min((run_min_x - content_x).max(0.0));
                            if adjustment > 0.0 {
                                for j in last_run.line_begin..last_run.end {
                                    let is_outside_marker = layout_box.children[j].is_list_marker
                                        && layout_box.children[j].list_style_position
                                            == ListStylePosition::Outside;
                                    if !is_outside_marker {
                                        layout_box.children[j].x -= adjustment;
                                    }
                                }
                            }
                        }
                    }
                    // Successive right floats stack leftward: each float's right
                    // edge sits against the floats already placed to its right
                    // (CSS 2.1 §9.5.1), mirrored on how left floats accumulate
                    // rightward.
                    layout_box.children[i].x = content_x + child_containing_width
                        - cm.margin_right
                        - layout_box.children[i].width
                        - float_right_width;
                    layout_box.children[i].y = float_y + cm.margin_top;
                    float_right_width +=
                        layout_box.children[i].width + cm.margin_left + cm.margin_right;
                } else {
                    // A left float claims the left edge of the line. When it
                    // follows an inline run, shift the already-placed run right
                    // by the float's total width so the float paints beside the
                    // text instead of on top of it. The run stays recorded so a
                    // chain of left floats keeps shifting it. When there is no
                    // room for both on the line, the float drops below the run
                    // (CSS 2.1 §9.5.1 rule 5).
                    let mut left_float_y = float_y;
                    if let Some(mut run) = last_inline_run {
                        if float_y <= run.line_top + 0.5 {
                            let float_total_width =
                                layout_box.children[i].width + cm.margin_left + cm.margin_right;
                            if run.used_width
                                <= run.inline_available - run.shift - float_total_width
                            {
                                for j in run.line_begin..run.end {
                                    let is_outside_marker = layout_box.children[j].is_list_marker
                                        && layout_box.children[j].list_style_position
                                            == ListStylePosition::Outside;
                                    if !is_outside_marker {
                                        layout_box.children[j].x += float_total_width;
                                    }
                                }
                                run.shift += float_total_width;
                                last_inline_run = Some(run);
                            } else {
                                // No room beside the run: place the float below it.
                                left_float_y = cursor_y;
                                last_inline_run = None;
                            }
                        } else {
                            // The float lands below the run's line; the run no
                            // longer needs to make room for anything.
                            last_inline_run = None;
                        }
                    } else {
                        last_inline_run = None;
                    }
                    float_y = left_float_y;
                    layout_box.children[i].x = content_x + float_left_width + cm.margin_left;
                    layout_box.children[i].y = left_float_y + cm.margin_top;
                    float_left_width +=
                        layout_box.children[i].width + cm.margin_left + cm.margin_right;
                }
                float_bottom =
                    (float_y + layout_box.children[i].height + cm.margin_top + cm.margin_bottom)
                        .max(float_bottom);
                // Remember the top of the earliest float in the active stack so
                // later inline runs can wrap around the whole stack.
                active_float_top = active_float_top.min(float_y);

                // If the next child is inline-level, keep cursor_y at the line top
                // so the inline run can start on the same line and wrap around the float.
                // If the next child is another float, also keep cursor_y at the line top
                // so multiple floats can be placed side-by-side on the same line.
                // However, if this float was placed AFTER an inline run, the next inline
                // run should start on a new line, not on the same line (which would
                // cause it to overlap the preceding inline content).
                // Collapsed whitespace between floats is not a real inline run, so
                // look past it when deciding where the next meaningful sibling belongs.
                let mut next_idx = i + 1;
                while next_idx < layout_box.children.len()
                    && (abs_indices.contains(&next_idx)
                        || is_whitespace_only_text(&layout_box.children[next_idx]))
                {
                    next_idx += 1;
                }
                let next_is_inline = next_idx < layout_box.children.len()
                    && is_inline_level_styled(
                        layout_box.children[next_idx].box_type,
                        styles,
                        layout_box.children[next_idx].node_id,
                    );
                let next_is_float = next_idx < layout_box.children.len()
                    && styles
                        .get(&layout_box.children[next_idx].node_id)
                        .map(|s| s.float != Float::None)
                        .unwrap_or(false);
                // Keep cursor_y at the float's line so the next element starts
                // beside the float and can wrap around it. For blocks this lets
                // inline children flow beside the float; for inline/float it lets
                // them share the same line. Advance past the float only when the
                // float was placed after an inline run and the next child is
                // inline (a fresh line keeps it from painting over the run the
                // float follows); a following float always stacks beside this
                // one on the same line, so it keeps the float's line top.
                if float_placed_after_inline && next_is_inline {
                    cursor_y = cursor_y.max(float_bottom);
                } else if next_is_float {
                    cursor_y = float_y;
                } else if next_is_inline {
                    // Start subsequent inline runs at the top of the earliest
                    // active float so they can wrap around a stack of cleared
                    // floats, not just the most recently placed one.
                    cursor_y = active_float_top;
                } else {
                    cursor_y = float_y;
                }
                i += 1;
                continue;
            }
            last_inline_run = None;
            pending_float_line_top = None; // Stale line top doesn't apply to regular blocks

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
            // Floats inside a child that does not establish a BFC stick out
            // below its border box and keep intruding on this block's
            // subsequent content, exactly as they intrude on the child's own
            // inline text. Merge every float whose bottom lands below the
            // child's box into this block's float state so a following float
            // that cannot fit beside them is placed below them, and later
            // inline runs wrap around them. Without this, a container whose
            // only in-flow content is floated (so it has zero height) hides
            // its floats from the flow entirely.
            {
                let child_box = &layout_box.children[i];
                let cs = styles.get(&child_box.node_id).cloned().unwrap_or_default();
                let after_is_whitespace_only = matches!(
                    cs.after_content,
                    incognidium_style::Content::Text(ref t) if t.trim().is_empty()
                );
                let has_clearfix_pseudo =
                    after_is_whitespace_only && matches!(cs.after_visibility, Visibility::Visible);
                if !cs.establishes_bfc() && !has_clearfix_pseudo {
                    let child_border_bottom = child_box.y + child_box.height;
                    let mut escapes = Vec::new();
                    collect_floats_within(child_box, styles, 0.0, 0.0, &mut escapes);
                    let right_edge = content_x + child_containing_width;
                    for f in escapes {
                        // Coordinates are relative to the child's border-box
                        // origin; shift them into this block's space.
                        let bottom = child_box.y + f.bottom;
                        if bottom <= child_border_bottom + 0.5 {
                            continue;
                        }
                        if bottom > float_bottom {
                            float_bottom = bottom;
                        }
                        let top = child_box.y + f.top;
                        if top < active_float_top {
                            active_float_top = top;
                        }
                        if f.left {
                            let w = (child_box.x + f.x1 - content_x).max(0.0);
                            if w > float_left_width {
                                float_left_width = w;
                            }
                        } else {
                            let w = (right_edge - (child_box.x + f.x0)).max(0.0);
                            if w > float_right_width {
                                float_right_width = w;
                            }
                        }
                    }
                }
            }
            // A block contributes to vertical layout if it has visible height,
            // padding, borders, background, or non-zero margins. Elements with
            // zero height but explicit margins (e.g. <hr> used as a spacer) must
            // still participate in margin collapsing and advance the cursor.
            let child_has_visual = layout_box.children[i].height > 0.0
                || cm.margin_top != 0.0
                || cm.margin_bottom != 0.0
                || cm.padding_top_px(effective_width) > 0.0
                || cm.padding_bottom_px(effective_width) > 0.0
                || cm.background_color.a > 0
                || cm.border_top_width > 0.0
                || cm.border_bottom_width > 0.0;
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
                // Margin collapse: when both margins are positive, keep the larger;
                // when either (or both) is negative, add them so the negative margin
                // pulls the box upward against a previous sibling.
                let collapsed_margin_top = if cm.margin_top >= 0.0 && prev_margin_bottom >= 0.0 {
                    cm.margin_top.max(prev_margin_bottom)
                } else {
                    cm.margin_top + prev_margin_bottom
                };
                layout_box.children[i].x = effective_x + x_offset;
                layout_box.children[i].y = cursor_y + collapsed_margin_top;
                cursor_y += collapsed_margin_top + layout_box.children[i].height;
                prev_margin_bottom = cm.margin_bottom;
            }
            i += 1;
        }
    }

    // Calculate height. For normal block boxes, auto height is determined by
    // in-flow block children (with margin collapse). Only containers that
    // establish a block formatting context (BFC) are required to enclose their
    // floated children; otherwise floats stick out below the box. Absolutely/
    // fixed positioned children are removed from normal flow and must NOT
    // inflate the parent's auto height, or hidden mega-menus make headers
    // thousands of pixels tall.
    let mut auto_height = cursor_y + prev_margin_bottom - padding_top - style.border_top_width;

    let establishes_bfc = style.establishes_bfc();
    // Framework clearfix hacks use a visible ::after pseudo-element with
    // display:table; clear:both. Our pseudo-elements are not modeled as real
    // block boxes, so the BFC they would establish is lost. As a pragmatic
    // heuristic, treat a block container with a visible ::after as needing
    // to enclose its direct floated children, so clearfixed rows don't
    // collapse around their floats.
    let after_is_whitespace_only = matches!(
        style.after_content,
        incognidium_style::Content::Text(ref t) if t.trim().is_empty()
    );
    let (max_float_bottom, has_nested_float) = max_float_bottom_within_bfc(layout_box, styles, 0.0);
    let has_clearfix_pseudo = after_is_whitespace_only
        && matches!(style.after_visibility, Visibility::Visible)
        && has_nested_float;
    let encloses_floats = establishes_bfc || has_clearfix_pseudo;
    if encloses_floats && max_float_bottom > auto_height {
        auto_height = max_float_bottom;
    }
    let mut content_height = if let Some(h) = effective_height {
        h
    } else {
        match style.height {
            SizeValue::Px(h) => h,
            SizeValue::Percent(p) => {
                if containing_height > 0.0 {
                    containing_height * p / 100.0
                } else {
                    auto_height
                }
            }
            _ => {
                // When height is auto (or a percentage that cannot be resolved),
                // honor an explicit aspect-ratio. If the box clips overflow,
                // the ratio sets the used height; extra in-flow content is
                // clipped. For visible overflow, where text sits below a cover
                // image and the ratio only sizes the image area, allow the box
                // to grow with its content.
                if let Some(ref ar) = style.aspect_ratio {
                    let ratio = ar.width / ar.height.max(0.001);
                    if ratio > 0.0 && layout_box.content_width > 0.0 {
                        let ratio_height = layout_box.content_width / ratio;
                        if style.overflow != Overflow::Visible {
                            ratio_height
                        } else {
                            ratio_height.max(auto_height)
                        }
                    } else {
                        auto_height
                    }
                } else {
                    auto_height
                }
            }
        }
    };
    // Absolutely/fixed positioned boxes with auto height and both vertical
    // insets stretch to fill the space between those insets. Doing this in
    // the block pass ensures nested absolute children see a definite
    // containing-block height instead of a collapsed zero height.
    let is_auto_height = matches!(style.height, SizeValue::Auto | SizeValue::None);
    if is_auto_height && (style.position == Position::Absolute || style.position == Position::Fixed)
    {
        if let (Some(top), Some(bottom)) = (
            resolve_offset(
                &style.top,
                containing_height,
                containing_height,
                style.font_size,
            ),
            resolve_offset(
                &style.bottom,
                containing_height,
                containing_height,
                style.font_size,
            ),
        ) {
            let mut stretched =
                (containing_height - top - bottom - style.margin_top - style.margin_bottom)
                    .max(0.0);
            if let Some(mh) =
                evaluate_size_value(&style.max_height, containing_height, style.font_size)
            {
                stretched = stretched.min(mh);
            }
            if let Some(mh) =
                evaluate_size_value(&style.min_height, containing_height, style.font_size)
            {
                stretched = stretched.max(mh);
            }
            let pb_height =
                padding_top + padding_bottom + style.border_top_width + style.border_bottom_width;
            content_height = (stretched - pb_height).max(0.0);
        }
    }
    // Apply min-height / max-height
    let content_height = if let Some(mh) =
        evaluate_size_value(&style.min_height, containing_height, style.font_size)
    {
        content_height.max(mh)
    } else {
        content_height
    };
    let content_height = if let Some(mh) =
        evaluate_size_value(&style.max_height, containing_height, style.font_size)
    {
        content_height.min(mh)
    } else {
        content_height
    };

    layout_box.content_height = content_height.max(0.0);
    layout_box.height = content_height
        + padding_top
        + padding_bottom
        + style.border_top_width
        + style.border_bottom_width;

    // Position absolutely/fixed positioned children. compute_layout dispatches to
    // layout_absolute, which sets the child's size, insets, and (x, y) relative to
    // its containing block. The parent must not re-derive x/y afterwards; doing so
    // would overwrite layout_absolute's auto-margin centering and min/max clamping.
    let container_w = layout_box.width;
    let container_h = layout_box.height;
    for &idx in &abs_indices {
        let child = &mut layout_box.children[idx];
        let cs = styles.get(&child.node_id).cloned().unwrap_or_default();

        // Compute their layout with container width. Pass the full containing-block
        // width so layout_absolute can resolve percentages and right/left insets
        // correctly; pre-resolving them here would square the percentage
        // (e.g. 70% of 70%) and break right-offset positioning.
        compute_layout(child, styles, container_w, container_h, image_sizes);
    }

    // Apply relative positioning offsets to positioned children
    // Relative positioning: offset from normal position without removing from flow
    for child in &mut layout_box.children {
        let cs = styles.get(&child.node_id).cloned().unwrap_or_default();
        if cs.position == Position::Relative {
            // Apply left/right offset (prefer left)
            let content_w = container_w
                - cs.padding_left
                - cs.padding_right
                - cs.border_left_width
                - cs.border_right_width;
            let offset_x = if let Some(v) =
                resolve_offset(&cs.left, container_w, content_w, cs.font_size)
            {
                v
            } else if let Some(v) = resolve_offset(&cs.right, container_w, content_w, cs.font_size)
            {
                -v
            } else {
                0.0
            };
            // Apply top/bottom offset (prefer top)
            let offset_y =
                if let Some(v) = resolve_offset(&cs.top, container_h, container_h, cs.font_size) {
                    v
                } else if let Some(v) =
                    resolve_offset(&cs.bottom, container_h, container_h, cs.font_size)
                {
                    -v
                } else {
                    0.0
                };
            // Clamp extreme relative offsets that would push the box entirely (or
            // mostly) off-canvas. Entrance animations often start with
            // `top: -100%` on a relatively positioned header; without a real
            // animation timeline, that initial value stays applied and shifts the
            // whole document origin upward. We allow small decorative offsets but
            // refuse to move a box farther than its own size off the top/left edge.
            let clamped_offset_x = if offset_x < -child.width && child.width > 0.0 {
                0.0
            } else {
                offset_x
            };
            let clamped_offset_y = if offset_y < -child.height && child.height > 0.0 {
                0.0
            } else {
                offset_y
            };
            child.x += clamped_offset_x;
            child.y += clamped_offset_y;
        }
    }
}

/// Returns true when every child participates in an inline formatting context.
fn children_are_inline_level(children: &[LayoutBox], styles: &StyleMap) -> bool {
    children
        .iter()
        .all(|c| is_inline_level_styled(c.box_type, styles, c.node_id))
}

/// Layout inline-level children of an inline-block in a wrapping line run.
/// Children are positioned horizontally, wrapping when the line would exceed
/// `content_width`.  Padding/border offsets are added so the run sits inside
/// the inline-block's content box.  Returns the actual width of the widest line
/// and the total content height.
fn layout_inline_children_run(
    children: &mut [LayoutBox],
    styles: &StyleMap,
    content_width: f32,
    padding_left: f32,
    padding_top: f32,
    border_left: f32,
    border_top: f32,
) -> (f32, f32) {
    let num_children = children.len();
    if num_children == 0 {
        return (0.0, 0.0);
    }
    let gaps = compute_inline_gaps(children, 0, num_children, styles);
    let mut line_x: f32 = 0.0;
    let mut line_height: f32 = 0.0;
    let mut total_height: f32 = 0.0;
    let mut max_line_width: f32 = 0.0;

    for (idx, child) in children.iter_mut().enumerate() {
        line_x += gaps[idx];
        let is_line_break = child.box_type == BoxType::LineBreak;
        if is_line_break {
            max_line_width = max_line_width.max(line_x);
            total_height += line_height;
            line_x = 0.0;
            line_height = 0.0;
            child.x = padding_left + border_left;
            child.y = total_height + padding_top + border_top;
            child.width = 0.0;
            child.height = 0.0;
            continue;
        }
        let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
        let nowrap = matches!(
            child_style.white_space,
            incognidium_style::WhiteSpace::NoWrap | incognidium_style::WhiteSpace::Pre
        );
        // Horizontal margins are part of an inline-level box's advance, the
        // same way the block-container inline run applies them.
        let child_margin_left = child_style.margin_left;
        let child_margin_right = child_style.margin_right;
        if line_x + child_margin_left + child.width + child_margin_right > content_width + 0.5
            && line_x > 0.0
            && !nowrap
        {
            max_line_width = max_line_width.max(line_x);
            total_height += line_height;
            line_x = 0.0;
            line_height = 0.0;
        }
        child.x = line_x + child_margin_left + padding_left + border_left;
        child.y = total_height + padding_top + border_top;
        line_x += child_margin_left + child.width + child_margin_right;
        line_height = line_height.max(child.height);
    }
    total_height += line_height;
    max_line_width = max_line_width.max(line_x);
    (max_line_width.max(0.0), total_height.max(0.0))
}

/// Measure the max-content width of inline-level children by laying them out
/// with a generous line width and summing their natural widths (plus gaps).
fn measure_inline_children_max_width(
    children: &mut [LayoutBox],
    styles: &StyleMap,
    image_sizes: &ImageSizes,
) -> f32 {
    for child in children.iter_mut() {
        compute_layout(child, styles, 10_000.0, 0.0, image_sizes);
    }
    let num_children = children.len();
    if num_children == 0 {
        return 0.0;
    }
    let gaps = compute_inline_gaps(children, 0, num_children, styles);
    let mut line_x: f32 = 0.0;
    for (idx, child) in children.iter().enumerate() {
        line_x += gaps[idx];
        // Margins are part of the box's inline advance (see
        // `layout_inline_children_run`).
        let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
        line_x += child_style.margin_left + child.width + child_style.margin_right;
    }
    line_x.max(0.0)
}

/// Layout an inline-block element: establishes a block formatting context but
/// shrinks to fit its content width instead of expanding to the containing width.
fn layout_inline_block(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    available_width: f32,
    containing_height: f32,
    image_sizes: &ImageSizes,
) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    let margin_left = style.margin_left;
    let margin_right = style.margin_right;
    let padding_left = style.padding_left_px(containing_width);
    let padding_right = style.padding_right_px(containing_width);
    let border_left = style.border_left_width;
    let border_right = style.border_right_width;
    let padding_top = style.padding_top_px(containing_width);
    let padding_bottom = style.padding_bottom_px(containing_width);
    let border_top = style.border_top_width;
    let border_bottom = style.border_bottom_width;

    let is_border_box = style.box_sizing == incognidium_style::BoxSizing::BorderBox;

    // Special handling for textarea: use rows/cols for sizing, unless field-sizing: content is set
    let is_textarea = layout_box.textarea_info.is_some();
    let textarea_cols = layout_box.textarea_info.map(|t| t.cols).unwrap_or(0);
    let textarea_rows = layout_box.textarea_info.map(|t| t.rows).unwrap_or(0);
    // field-sizing: content makes the field size to its content
    let field_sizing_content = style.field_sizing == incognidium_style::FieldSizing::Content;

    // Check if width is explicitly set. A parent flex container may have already
    // resolved and forced the item's content-box width; honor that value
    // instead of recomputing it from the element's own style.
    let (explicit_width, forced_by_parent) = if let Some(forced) =
        layout_box.forced_content_width.take()
    {
        (Some(forced.max(0.0)), true)
    } else {
        let w = match style.width {
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
            // CSS Math Functions (calc()/min()/max()/clamp()) resolve as a
            // definite width against the containing block; percentages inside
            // them resolve the same way as a bare percentage.
            SizeValue::Calc(_)
            | SizeValue::Min(_)
            | SizeValue::Max(_)
            | SizeValue::Clamp { .. } => {
                evaluate_size_value(&style.width, containing_width, style.font_size).map(|total| {
                    if is_border_box {
                        (total - padding_left - padding_right - border_left - border_right).max(0.0)
                    } else {
                        total
                    }
                })
            }
            _ => None,
        };
        (w, false)
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
        // When a parent flex container forced the width, the total main-axis size
        // is the content width plus padding and border. Do not fall back to the
        // element's own style.width for border-box items, or a flex item with an
        // explicit width (e.g. input[type=search]) will ignore the space it was
        // assigned by flex-grow and snap back to its own width.
        layout_box.width = if forced_by_parent {
            layout_box.content_width + padding_left + padding_right + border_left + border_right
        } else if is_border_box {
            match style.width {
                SizeValue::Px(w) => w,
                SizeValue::Percent(p) => containing_width * p / 100.0,
                _ => {
                    layout_box.content_width
                        + padding_left
                        + padding_right
                        + border_left
                        + border_right
                }
            }
        } else {
            layout_box.content_width + padding_left + padding_right + border_left + border_right
        };

        // Layout children. Inline-level children participate in an inline formatting
        // context inside the inline-block; block-level children are stacked.
        let child_containing = layout_box.content_width;
        let inline_children = children_are_inline_level(&layout_box.children, styles);
        // SAFETY CAP: Track total height to prevent runaway layout
        const MAX_HEIGHT: f32 = 100_000.0;
        let mut cursor_y: f32 = if inline_children {
            // Children must be measured against the explicit content width before
            // the inline run can position them. Without this, inline children of a
            // width:100% inline-block (e.g. a prominent call-to-action button)
            // keep their zero initial dimensions and disappear.
            for child in &mut layout_box.children {
                compute_layout(child, styles, child_containing, 0.0, image_sizes);
            }
            let (_used_width, run_height) = layout_inline_children_run(
                &mut layout_box.children,
                styles,
                child_containing,
                padding_left,
                padding_top,
                border_left,
                border_top,
            );
            // The inline-block's content width already accounts for the chosen
            // explicit width; the run may not fill it fully.
            padding_top + border_top + run_height
        } else {
            // Block-level children inside an explicit-width inline-block should
            // participate in a block formatting context, including floats. The
            // manual vertical stack above lost multi-column floated layouts
            // (e.g. percentage-width `float: left` grid items). Re-use the full
            // block layout pass, forcing the already-resolved content width so
            // `layout_block` does not recompute it.
            layout_box.forced_content_width = Some(child_containing);
            layout_block(
                layout_box,
                styles,
                containing_width,
                0.0,
                image_sizes,
                FloatState::default(),
            );
            layout_box.content_height + padding_top + style.border_top_width
        };

        // After `layout_block`, the inline-block's width, content height, and
        // child positions are already set. The remaining height logic below
        // applies inline-block-specific sizing (textarea rows, aspect-ratio,
        // min/max-height) and may override the block-derived content height.

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
        let content_height = match &style.height {
            // A percentage height resolves against the containing block's
            // height, and only when that height is definite (an auto-height
            // parent makes the declaration fall back to auto). Resolving it
            // against the containing width made `height: 100%` form controls
            // balloon to the page width.
            SizeValue::Percent(p) if containing_height > 0.0 => {
                Some((containing_height * p / 100.0).min(MAX_HEIGHT))
            }
            SizeValue::Percent(_) => None,
            SizeValue::Px(h) => Some(h.min(MAX_HEIGHT)),
            SizeValue::Auto | SizeValue::None => None,
            _ => evaluate_size_value(&style.height, containing_height, style.font_size)
                .map(|h| h.min(MAX_HEIGHT)),
        }
        .unwrap_or_else(|| {
            // When height is auto, honor an explicit aspect-ratio while still
            // allowing in-flow content to make the inline-block taller.
            if let Some(ref ar) = style.aspect_ratio {
                let ratio = ar.width / ar.height.max(0.001);
                if ratio > 0.0 && layout_box.content_width > 0.0 {
                    (layout_box.content_width / ratio).max(auto_height)
                } else {
                    auto_height
                }
            } else {
                auto_height
            }
        });
        let content_height = if let Some(mh) = match &style.min_height {
            SizeValue::Percent(p) => {
                if containing_height > 0.0 {
                    Some(containing_height * p / 100.0)
                } else {
                    None
                }
            }
            _ => evaluate_size_value(&style.min_height, containing_height, style.font_size),
        } {
            content_height.max(mh)
        } else {
            content_height
        };
        let content_height = if let Some(mh) = match &style.max_height {
            SizeValue::Percent(p) => {
                if containing_height > 0.0 {
                    Some(containing_height * p / 100.0)
                } else {
                    None
                }
            }
            _ => evaluate_size_value(&style.max_height, containing_height, style.font_size),
        } {
            content_height.min(mh)
        } else {
            content_height
        };

        layout_box.content_height = content_height.max(0.0);
        layout_box.height =
            content_height + padding_top + padding_bottom + border_top + border_bottom;
    } else {
        // Auto width: shrink-to-fit
        // If the containing width is real, use it as the available space; otherwise
        // (containing_width == 0 usually means an auto-width parent) let children
        // measure their natural widths and shrink to fit them.
        let has_explicit_container = available_width > 0.0;
        let max_available = if has_explicit_container {
            (available_width
                - margin_left
                - margin_right
                - padding_left
                - padding_right
                - border_left
                - border_right)
                .max(0.0)
        } else {
            // No explicit available width: give children a generous measuring width
            // so shrink-to-fit uses their natural widths.
            10000.0
        };
        let inline_children = children_are_inline_level(&layout_box.children, styles);

        // SAFETY CAP for auto-width inline-block
        const MAX_HEIGHT: f32 = 100_000.0;

        let (mut max_child_width, mut cursor_y) = if inline_children {
            // Inline-level children: shrink-to-fit should use the max-content line
            // width (sum of natural widths) so tag-list inline-blocks don't wrap
            // internally and balloon the containing block.
            let max_content_width =
                measure_inline_children_max_width(&mut layout_box.children, styles, image_sizes);
            let content_width_for_run = max_content_width.min(max_available);
            let (used_width, run_height) = layout_inline_children_run(
                &mut layout_box.children,
                styles,
                content_width_for_run,
                padding_left,
                padding_top,
                border_left,
                border_top,
            );
            // Prefer the actual line width when it is wider than the wrapped width,
            // so single-line inline-blocks size to their content rather than the
            // available line fragment.  This is a pragmatic shrink-to-fit adjustment;
            // overflow is bounded by the parent line-breaking logic.
            let max_line_width = used_width.max(max_content_width);
            (max_line_width, padding_top + border_top + run_height)
        } else {
            let content_x = padding_left + border_left;
            let mut cursor_y: f32 = padding_top + border_top;
            let mut max_child_width: f32 = 0.0;
            for child in &mut layout_box.children {
                compute_layout(child, styles, max_available, 0.0, image_sizes);
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
                // Use intrinsic width (not the expanded layout width) so that
                // auto-width block children shrink-to-fit instead of filling
                // the measuring-pass width.
                let child_intrinsic = calculate_intrinsic_width(child, styles);
                let child_intrinsic_full = child_intrinsic
                    + cm.padding_left_px(0.0)
                    + cm.padding_right_px(0.0)
                    + cm.border_left_width
                    + cm.border_right_width
                    + cm.margin_left
                    + cm.margin_right;
                max_child_width = max_child_width.max(child_intrinsic_full);
            }
            (max_child_width, cursor_y)
        };

        // Shrink to fit: use the widest child/line, clamped by available space
        // For textarea, calculate width based on cols attribute
        // For checkbox/radio, use a square size based on font size
        let is_checkbox_radio = matches!(
            layout_box.input_type,
            Some(InputType::Checkbox { .. }) | Some(InputType::Radio { .. })
        );
        let mut content_width = if is_textarea && textarea_cols > 0 && !field_sizing_content {
            // Estimate character width based on cols attribute (unless field-sizing: content)
            let char_width = style.font_size * 0.6; // Approximate char width
            (textarea_cols as f32 * char_width).min(max_available)
        } else if is_textarea && field_sizing_content {
            // field-sizing: content - size to actual content width
            max_child_width.min(max_available)
        } else if is_checkbox_radio {
            // Checkbox/radio: use line height as intrinsic size (square aspect ratio)
            let line_height = style.font_size * style.line_height;
            line_height.min(max_available)
        } else if layout_box.input_type.is_some() && layout_box.text.is_some() {
            // Input with placeholder/value text but no children: size to text width
            let text_width = if max_child_width > 0.0 {
                max_child_width
            } else if let Some(ref text) = layout_box.text {
                measure_text_width(text, style.font_size, &style)
            } else {
                0.0
            };
            text_width.min(max_available)
        } else {
            max_child_width.min(max_available)
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

        // If shrink-to-fit or max-width clamping narrowed the box below the width
        // used during the measuring/layout passes, re-layout the children so that
        // percentage-width replaced elements (e.g. images with width:100%) fill the
        // final content box instead of overflowing it.
        let relayout_threshold = if inline_children {
            max_child_width
        } else {
            max_available
        };
        if content_width > 0.0 && content_width < relayout_threshold - 0.5 {
            if inline_children {
                for child in &mut layout_box.children {
                    compute_layout(child, styles, content_width, 0.0, image_sizes);
                }
                let (_used_width, run_height) = layout_inline_children_run(
                    &mut layout_box.children,
                    styles,
                    content_width,
                    padding_left,
                    padding_top,
                    border_left,
                    border_top,
                );
                cursor_y = padding_top + border_top + run_height;
            } else {
                let content_x = padding_left + border_left;
                cursor_y = padding_top + border_top;
                for child in &mut layout_box.children {
                    compute_layout(child, styles, content_width, 0.0, image_sizes);
                    let cm = styles.get(&child.node_id).cloned().unwrap_or_default();
                    if child.height > 0.0 {
                        child.x = content_x + cm.margin_left;
                        child.y = cursor_y + cm.margin_top;
                        cursor_y += cm.margin_top + child.height + cm.margin_bottom;
                        if cursor_y > MAX_HEIGHT {
                            break;
                        }
                    }
                }
            }
        }

        layout_box.content_width = content_width.max(0.0);
        layout_box.width =
            layout_box.content_width + padding_left + padding_right + border_left + border_right;
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
        let content_height = match &style.height {
            // A percentage height resolves against the containing block's
            // height, and only when that height is definite (an auto-height
            // parent makes the declaration fall back to auto). Resolving it
            // against the containing width made `height: 100%` form controls
            // balloon to the page width.
            SizeValue::Percent(p) if containing_height > 0.0 => {
                Some((containing_height * p / 100.0).min(MAX_HEIGHT))
            }
            SizeValue::Percent(_) => None,
            SizeValue::Px(h) => Some(h.min(MAX_HEIGHT)),
            SizeValue::Auto | SizeValue::None => None,
            _ => evaluate_size_value(&style.height, containing_height, style.font_size)
                .map(|h| h.min(MAX_HEIGHT)),
        }
        .unwrap_or_else(|| {
            // When height is auto, honor an explicit aspect-ratio while still
            // allowing in-flow content to make the inline-block taller.
            if let Some(ref ar) = style.aspect_ratio {
                let ratio = ar.width / ar.height.max(0.001);
                if ratio > 0.0 && layout_box.content_width > 0.0 {
                    (layout_box.content_width / ratio).max(auto_height)
                } else {
                    auto_height
                }
            } else {
                auto_height
            }
        });
        let content_height = if let Some(mh) = match &style.min_height {
            SizeValue::Percent(p) => {
                if containing_height > 0.0 {
                    Some(containing_height * p / 100.0)
                } else {
                    None
                }
            }
            _ => evaluate_size_value(&style.min_height, containing_height, style.font_size),
        } {
            content_height.max(mh)
        } else {
            content_height
        };
        let content_height = if let Some(mh) = match &style.max_height {
            SizeValue::Percent(p) => {
                if containing_height > 0.0 {
                    Some(containing_height * p / 100.0)
                } else {
                    None
                }
            }
            _ => evaluate_size_value(&style.max_height, containing_height, style.font_size),
        } {
            content_height.min(mh)
        } else {
            content_height
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
        BoxType::Text
            | BoxType::Inline
            | BoxType::InlineBlock
            | BoxType::InlineFlex
            | BoxType::LineBreak
    )
}

fn is_inline_level_styled(box_type: BoxType, styles: &StyleMap, node_id: NodeId) -> bool {
    // Floats create block-level boxes regardless of their display property.
    // A floated inline-block element must be handled as a block-level float,
    // not as part of an inline run.
    if let Some(style) = styles.get(&node_id) {
        if style.float != Float::None {
            return false;
        }
    }
    if matches!(
        box_type,
        BoxType::Text
            | BoxType::Inline
            | BoxType::InlineBlock
            | BoxType::InlineFlex
            | BoxType::LineBreak
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

fn is_whitespace_only_text(lb: &LayoutBox) -> bool {
    lb.text
        .as_deref()
        .map(is_collapsible_whitespace_only)
        .unwrap_or(false)
}

/// White space that CSS collapsing can remove: space, tab, LF, CR, form feed.
/// NBSP (U+00A0) is deliberately excluded — `&nbsp;` is not collapsible in
/// CSS, so text made of it is real content that keeps its box open (legend
/// swatches, empty table cells, inline spacers).
fn is_css_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{000C}')
}

/// True when text consists only of collapsible white space (NBSP is content).
fn is_collapsible_whitespace_only(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_css_whitespace)
}

/// True when a text box is only a collapsible word space between inline
/// siblings. Line layout may consume the space out of the box's text (it
/// becomes an inter-word gap), leaving empty content behind with the
/// leading/trailing-space flags still set.
fn is_whitespace_boundary_box(child: &LayoutBox) -> bool {
    if child.box_type != BoxType::Text {
        return false;
    }
    match &child.text {
        Some(t) if is_collapsible_whitespace_only(t) => true,
        Some(t) if t.is_empty() => child.text_leading_space || child.text_trailing_space,
        _ => false,
    }
}

/// Whether an inline-level box's content begins with collapsible source
/// whitespace. `layout_text` folds such whitespace into the text box's
/// leading-space flag, and that flag lives on the innermost text box, so
/// promote it through inline containers here: a span that starts with a
/// space still separates from its predecessor when it sits mid-line.
fn inline_content_leading_space(lb: &LayoutBox) -> bool {
    match lb.box_type {
        BoxType::Text => lb.text_leading_space,
        _ => lb
            .children
            .first()
            .map_or(false, inline_content_leading_space),
    }
}

/// The trailing counterpart of `inline_content_leading_space`.
fn inline_content_trailing_space(lb: &LayoutBox) -> bool {
    match lb.box_type {
        BoxType::Text => lb.text_trailing_space,
        _ => lb
            .children
            .last()
            .map_or(false, inline_content_trailing_space),
    }
}

/// The advance width of one collapsible word space in the given style.
///
/// Line layout preserves a word space between inline-level siblings as an
/// inter-word gap, but that gap is not part of any single fragment's measured
/// width, so intrinsic (max-content) width sums must add it back explicitly —
/// otherwise shrink-to-fit containers (floats, inline-blocks) come out a few
/// pixels too narrow and their last word wraps.
fn word_space_width(style: &incognidium_style::ComputedStyle) -> f32 {
    if matches!(
        style.white_space,
        incognidium_style::WhiteSpace::Pre | incognidium_style::WhiteSpace::PreWrap
    ) {
        // Preserved whitespace is already part of the fragment widths.
        return 0.0;
    }
    measure_text_width(" ", style.font_size, style)
}

/// The outer (border-box) intrinsic width of an inline-level child box as seen
/// from an inline formatting context.
///
/// `calculate_intrinsic_width` returns content widths for some box shapes and
/// full border-box widths for others: content-sized inline-blocks add their
/// own padding and border, while explicitly sized boxes and plain inline boxes
/// report the content box and rely on the caller to add it. Inline children
/// have no such caller, so normalize here — otherwise a padded inline element
/// or a sized/`min-width` form control inside a shrink-wrapped inline context
/// (a float, an inline-block) under-reports its width and its last item wraps.
fn inline_child_outer_intrinsic_width(child: &LayoutBox, styles: &StyleMap) -> f32 {
    let cs = styles.get(&child.node_id).cloned().unwrap_or_default();
    let pb = cs.padding_left_px(0.0)
        + cs.padding_right_px(0.0)
        + cs.border_left_width
        + cs.border_right_width;
    // Content-sized inline-blocks and inline-flexes already include their own
    // padding/border in the value calculate_intrinsic_width returns.
    let includes_pb = matches!(child.box_type, BoxType::InlineBlock | BoxType::InlineFlex)
        && !matches!(cs.width, SizeValue::Px(_));
    let mut width = calculate_intrinsic_width(child, styles);
    if !includes_pb {
        width += pb;
    }
    // A form control's laid-out width never drops below its min-width, which
    // the text-based intrinsic above does not account for.
    if child.input_type.is_some() {
        if let Some(min_w) = evaluate_size_value(&cs.min_width, 0.0, cs.font_size) {
            width = width.max(min_w + pb);
        }
    }
    width
}

/// Split text into words at CSS-collapsible white space boundaries. NBSP is
/// not a break opportunity: it stays glued inside its word so wrapping cannot
/// split there. Measurement and painting render NBSP with the space advance.
fn split_css_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    for c in text.chars() {
        if is_css_whitespace(c) {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(c);
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// Recursively check whether a layout box (or any descendant) contains
/// non-empty text content.  Used to decide whether an inline-level box
/// should receive text-baseline alignment instead of bottom-to-baseline
/// alignment during vertical-align processing.
fn layout_box_has_text(lb: &LayoutBox) -> bool {
    if lb.text.as_ref().map(|t| !t.is_empty()).unwrap_or(false) {
        return true;
    }
    lb.children.iter().any(|c| layout_box_has_text(c))
}

/// Compute inter-element gap to prevent text concatenation like "wordword".
/// Returns a Vec of gap values to add before each child.
///
/// Each gap is measured with the word-space advance of the child it precedes,
/// the same measurement `calculate_intrinsic_width` uses when it counts word
/// spaces between inline siblings. Measuring every gap against one global
/// space width taken from the run's first child (with a default font) drifts
/// from that intrinsic sum whenever siblings carry their own font size, and
/// shrink-to-fit containers then come out too narrow for their own line.
fn compute_inline_gaps(
    children: &[LayoutBox],
    start: usize,
    end: usize,
    styles: &StyleMap,
) -> Vec<f32> {
    let count = end - start;
    let mut gaps = vec![0.0f32; count];
    for j in 1..count {
        let prev = &children[start + j - 1];
        let curr = &children[start + j];

        let curr_style = styles.get(&curr.node_id).cloned().unwrap_or_default();
        let prev_is_whitespace = is_whitespace_only_text(prev);
        let curr_is_whitespace = is_whitespace_only_text(curr);

        // Whitespace-only text nodes are ignored for sizing, but a sequence of
        // them between two inline-level boxes should still produce the same
        // single-space gap that a real inter-word space would.
        if curr_is_whitespace {
            continue;
        }

        if prev_is_whitespace {
            // Find the nearest non-whitespace sibling before this whitespace.
            let mut k = j.saturating_sub(1);
            while k > 0 && is_whitespace_only_text(&children[start + k]) {
                k -= 1;
            }
            let prev_content = &children[start + k];
            // If the whole preceding run is whitespace, this is leading whitespace
            // for the line and should not produce an inter-element gap.
            if k == 0 && is_whitespace_only_text(prev_content) {
                continue;
            }
            let prev_is_inline =
                is_inline_level_styled(prev_content.box_type, styles, prev_content.node_id);
            let curr_is_inline = is_inline_level_styled(curr.box_type, styles, curr.node_id);
            if prev_is_inline && curr_is_inline && curr.width > 0.0 {
                // A sequence of whitespace-only text nodes between two inline-level
                // boxes represents source whitespace; collapse it to a single-space
                // gap just like a real inter-word space.
                gaps[j] = word_space_width(&curr_style);
            }
            continue;
        }

        if prev.width > 0.0 && curr.width > 0.0 {
            let prev_is_inline = is_inline_level_styled(prev.box_type, styles, prev.node_id);
            let curr_is_inline = is_inline_level_styled(curr.box_type, styles, curr.node_id);
            if prev_is_inline
                && curr_is_inline
                && (prev.text_trailing_space || curr.text_leading_space)
            {
                // Source whitespace that was collapsed into the leading or trailing
                // edge of a neighboring text node still needs to produce a single
                // automatic inter-word gap. Adjacent boxes with no source whitespace
                // are separated only by their own margins.
                gaps[j] = word_space_width(&curr_style);
            }
        }
    }
    gaps
}

/// Apply vertical-align to a single inline line and return the actual line
/// bottom relative to the provided line top.
fn apply_vertical_align(
    children: &mut [LayoutBox],
    start: usize,
    end: usize,
    line_top: f32,
    line_height: f32,
    css_line_height: f32,
    child_containing_width: f32,
    styles: &StyleMap,
) -> f32 {
    if start >= end {
        return 0.0;
    }
    let mut max_ascent: f32 = 0.0;
    let mut max_descent: f32 = 0.0;

    for j in start..end {
        let child_style = styles
            .get(&children[j].node_id)
            .cloned()
            .unwrap_or_default();
        let child_height = children[j].height;
        let box_type = children[j].box_type;

        // Estimate ascent/descent based on font metrics and box type.
        // For inline boxes that contain text, the baseline sits at the
        // content-area baseline, which is roughly font-size*0.75 below
        // the content top. Inline-block/inline-flex boxes add their own
        // padding and border above that content area, so their baseline
        // offset must include padding-top + border-top. Replaced
        // elements (images) and inline blocks with no in-flow text align
        // their bottom with the line baseline.
        let has_text = if box_type == BoxType::Text {
            children[j].text.is_some()
        } else {
            layout_box_has_text(&children[j])
        };
        let is_text_box = box_type == BoxType::Text;
        let is_text_inline = (box_type == BoxType::Inline
            || box_type == BoxType::InlineBlock
            || box_type == BoxType::InlineFlex)
            && has_text;
        let ascent = if is_text_box || is_text_inline {
            let content_top_offset = if box_type == BoxType::InlineBlock
                || box_type == BoxType::InlineFlex
                || box_type == BoxType::Inline
            {
                child_style.padding_top_px(child_containing_width) + child_style.border_top_width
            } else {
                0.0
            };
            content_top_offset + child_style.font_size * 0.75
        } else {
            child_height
        };
        let descent = child_height - ascent;

        max_ascent = max_ascent.max(ascent);
        max_descent = max_descent.max(descent);
    }

    // If no text content on this line, use CSS line-height as baseline.
    let has_text_content = (start..end).any(|j| {
        if children[j].box_type == BoxType::Text {
            children[j].text.is_some()
        } else {
            layout_box_has_text(&children[j])
        }
    });
    if !has_text_content {
        max_ascent = css_line_height * 0.75;
        max_descent = css_line_height - max_ascent;
    }

    let baseline_y = max_ascent;

    for j in start..end {
        let child_style = styles
            .get(&children[j].node_id)
            .cloned()
            .unwrap_or_default();
        let child_height = children[j].height;
        let box_type = children[j].box_type;
        let vertical_offset = match child_style.vertical_align {
            incognidium_style::VerticalAlign::Top => 0.0,
            incognidium_style::VerticalAlign::Bottom => line_height - child_height,
            incognidium_style::VerticalAlign::Middle => (line_height - child_height) / 2.0,
            incognidium_style::VerticalAlign::TextTop => {
                let text_top = baseline_y - max_ascent;
                text_top
            }
            incognidium_style::VerticalAlign::TextBottom => {
                let text_bottom = baseline_y + max_descent;
                text_bottom - child_height
            }
            incognidium_style::VerticalAlign::Super => -(child_style.font_size * 0.4),
            incognidium_style::VerticalAlign::Sub => child_style.font_size * 0.25,
            _ => {
                let is_text = box_type == BoxType::Text;
                let is_inline = box_type == BoxType::Inline;
                let is_inline_block = box_type == BoxType::InlineBlock;
                let is_inline_flex = box_type == BoxType::InlineFlex;
                let has_text_content = if is_text {
                    children[j].text.is_some()
                } else {
                    layout_box_has_text(&children[j])
                };

                if is_text || ((is_inline || is_inline_block || is_inline_flex) && has_text_content)
                {
                    let content_top_offset = if is_inline || is_inline_block || is_inline_flex {
                        child_style.padding_top_px(child_containing_width)
                            + child_style.border_top_width
                    } else {
                        0.0
                    };
                    let text_ascent = content_top_offset + child_style.font_size * 0.75;
                    baseline_y - text_ascent
                } else {
                    baseline_y - child_height
                }
            }
        };

        if vertical_offset != 0.0 {
            children[j].y += vertical_offset;
        }
    }

    // Prevent inline runs from being pushed above the current line top.
    let min_child_y = (start..end)
        .map(|j| children[j].y)
        .min_by(|a, b| a.partial_cmp(b).unwrap())
        .unwrap_or(line_top);
    if min_child_y < line_top - 0.5 {
        let shift = line_top - min_child_y;
        for j in start..end {
            children[j].y += shift;
        }
    }

    (start..end)
        .map(|j| children[j].y + children[j].height)
        .max_by(|a, b| a.partial_cmp(b).unwrap())
        .unwrap_or(line_top)
        - line_top
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
        // Outside list markers are positioned in the padding area and should not
        // shift with text alignment.
        let is_outside_marker =
            child.is_list_marker && child.list_style_position == ListStylePosition::Outside;
        if !is_outside_marker {
            child.x += shift;
        }
    }
}

/// Layout an inline element (e.g. <a>, <span>): shrink-to-fit width.
fn layout_inline(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    containing_height: f32,
    image_sizes: &ImageSizes,
) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    let padding_left = style.padding_left_px(containing_width);
    let padding_right = style.padding_right_px(containing_width);
    let padding_top = style.padding_top_px(containing_width);
    let padding_bottom = style.padding_bottom_px(containing_width);
    let border_left = style.border_left_width;
    let border_right = style.border_right_width;
    let border_top = style.border_top_width;
    let border_bottom = style.border_bottom_width;
    let margin_left = style.margin_left;
    let margin_right = style.margin_right;

    // Layout all children first to get their natural sizes
    for child in &mut layout_box.children {
        compute_layout(
            child,
            styles,
            containing_width.max(0.0),
            containing_height,
            image_sizes,
        );
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
        // Horizontal margins are part of an inline-level box's advance, the
        // same way the block-container inline run applies them.
        let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
        let child_margin_left = child_style.margin_left;
        let child_margin_right = child_style.margin_right;
        if line_x + child_margin_left + child.width + child_margin_right > containing_width + 0.5
            && line_x > margin_left
        {
            max_line_width = max_line_width.max(line_x);
            total_height += line_height;
            line_x = margin_left;
            line_height = 0.0;
        }
        child.x = line_x + child_margin_left + padding_left + border_left;
        child.y = total_height + padding_top + border_top;
        line_x += child_margin_left + child.width + child_margin_right;
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

/// Min-content inline contribution of a replaced element (`<img>`, `<video>`,
/// `<canvas>`, etc.) when computing a flex item's automatic minimum size.
///
/// In shrink-to-fit / min-content contexts, a cyclic percentage in `width` is
/// resolved against zero (CSS Box Sizing §5.2). Therefore a replaced element
/// with `width: 100%` contributes 0 to the min-content inline size, allowing a
/// flex item containing a responsive image to shrink instead of being pinned
/// to the container width. A replaced element with a definite pixel width or
/// auto width still reports its intrinsic/used width.
fn replaced_min_content_contribution(lb: &LayoutBox, styles: &StyleMap) -> f32 {
    let style = styles.get(&lb.node_id).cloned().unwrap_or_default();
    match style.width {
        SizeValue::Px(w) if w > 0.0 => w,
        // Percentage widths are cyclic in min-content sizing and resolve to zero.
        SizeValue::Percent(_) => 0.0,
        SizeValue::Auto | SizeValue::None => lb.content_width.max(0.0).min(lb.width),
        // Math functions containing percentages are evaluated against a zero
        // containing-block width here so they collapse to their length-only
        // portion (or to zero). This mirrors the cyclic-percentage handling for
        // replaced elements without a definite size suggestion.
        _ => evaluate_size_value(&style.width, 0.0, style.font_size)
            .map(|v| v.max(0.0))
            .unwrap_or(0.0),
    }
}

/// Min-content main-axis size of a flex item. This is the narrowest size the
/// item can take without overflowing its own content. For text it is the width
/// of the longest unbreakable word; for images it is the intrinsic width; for
/// row flex containers it is the sum of their children's min-content; for blocks
/// it is the widest child.
fn flex_item_min_content_main(child: &LayoutBox, is_row: bool, styles: &StyleMap) -> f32 {
    if !is_row {
        // No column min-content support yet; fall back to the laid-out extent.
        // Do not add child.y, which is a stale relative position from a previous
        // layout pass and would inflate the item's minimum main-axis size.
        return child.height.max(0.0);
    }
    let style = styles.get(&child.node_id).cloned().unwrap_or_default();
    let container_pb = style.padding_left_px(0.0)
        + style.padding_right_px(0.0)
        + style.border_left_width
        + style.border_right_width;
    match child.box_type {
        BoxType::Text => {
            if let Some(ref text) = child.text {
                if !is_collapsible_whitespace_only(text) {
                    let style = styles.get(&child.node_id).cloned().unwrap_or_default();
                    // Non-wrapping text cannot break at spaces, so its
                    // min-content main size is the full line, not the longest
                    // word (e.g. `white-space:nowrap` nav links).
                    if matches!(
                        style.white_space,
                        incognidium_style::WhiteSpace::NoWrap
                            | incognidium_style::WhiteSpace::Pre
                            | incognidium_style::WhiteSpace::PreWrap
                    ) {
                        return measure_text_width(text, style.font_size, &style);
                    }
                    return split_css_words(text)
                        .iter()
                        .map(|w| measure_text_width(w, style.font_size, &style))
                        .fold(0.0_f32, |a, b| a.max(b));
                }
            }
            0.0
        }
        BoxType::Image => replaced_min_content_contribution(child, styles),
        BoxType::Flex | BoxType::InlineFlex => {
            let child_is_row = matches!(
                style.flex_direction,
                FlexDirection::Row | FlexDirection::RowReverse
            );
            let wrapping = style.flex_wrap != FlexWrap::NoWrap;
            if child_is_row && !wrapping {
                let gap = if style.column_gap > 0.0 {
                    style.column_gap
                } else {
                    style.gap
                };
                let mut total = 0.0_f32;
                let mut count = 0usize;
                for c in &child.children {
                    if c.box_type == BoxType::None {
                        continue;
                    }
                    let cs = styles.get(&c.node_id).cloned().unwrap_or_default();
                    if cs.position == Position::Absolute || cs.position == Position::Fixed {
                        continue;
                    }
                    total += flex_item_min_content_main(c, true, styles)
                        + cs.margin_left
                        + cs.margin_right;
                    if count > 0 {
                        total += gap;
                    }
                    count += 1;
                }
                (total + container_pb).max(0.0)
            } else if !child_is_row {
                // Column flex container inside a row flex/block parent: its
                // min-content inline size is the widest item it can hold, not
                // its laid-out height. Use each child's min-content width so
                // percentage-width images do not pin the container to the full
                // available width (e.g. a sidebar column beside main content).
                let mut max = 0.0_f32;
                for c in &child.children {
                    if c.box_type == BoxType::None {
                        continue;
                    }
                    let cs = styles.get(&c.node_id).cloned().unwrap_or_default();
                    if cs.position == Position::Absolute || cs.position == Position::Fixed {
                        continue;
                    }
                    let m = flex_item_min_content_main(c, true, styles)
                        + cs.margin_left
                        + cs.margin_right;
                    if m > max {
                        max = m;
                    }
                }
                (max + container_pb).max(0.0)
            } else {
                calculate_intrinsic_width(child, styles) + container_pb
            }
        }
        BoxType::Block => {
            let mut max = 0.0_f32;
            for c in &child.children {
                if c.box_type == BoxType::None {
                    continue;
                }
                let cs = styles.get(&c.node_id).cloned().unwrap_or_default();
                if cs.position == Position::Absolute || cs.position == Position::Fixed {
                    continue;
                }
                let m =
                    flex_item_min_content_main(c, true, styles) + cs.margin_left + cs.margin_right;
                if m > max {
                    max = m;
                }
            }
            (max + container_pb).max(0.0)
        }
        BoxType::Inline | BoxType::InlineBlock => {
            let mut total = 0.0_f32;
            for c in &child.children {
                if c.box_type == BoxType::None {
                    continue;
                }
                let cs = styles.get(&c.node_id).cloned().unwrap_or_default();
                if cs.position == Position::Absolute || cs.position == Position::Fixed {
                    continue;
                }
                total +=
                    flex_item_min_content_main(c, true, styles) + cs.margin_left + cs.margin_right;
            }
            total + container_pb
        }
        BoxType::Grid => {
            // A grid flex item's minimum is its content-derived intrinsic width;
            // per-track min-content sizing is not modeled here, and returning 0
            // let shrink flatten icon-only controls (a grid `width: 100%` button
            // inside an auto-width slot) down to nothing.
            (calculate_intrinsic_width(child, styles) + container_pb).max(0.0)
        }
        _ => 0.0,
    }
}

/// Max-content main-axis size of a flex item measured after laying it out with
/// a very large containing size. For blockified wrappers this is the width/height
/// actually required by their children, not the container-filling width the block
/// layout reports.
fn flex_item_max_content_main(child: &LayoutBox, is_row: bool, styles: &StyleMap) -> f32 {
    // For the main axis, the natural size of a flex item is its intrinsic width.
    // Use the same helper as everywhere else so that nested flex containers sum
    // their children (e.g. a nav link with text + icon), blocks take
    // their widest child, and text/image use their natural content size rather
    // than a width that was clamped during distribution.
    if is_row {
        let intrinsic = calculate_intrinsic_width(child, styles);
        if intrinsic > 0.0 {
            return intrinsic;
        }
    }
    // Fallback to the laid-out extent for the cross axis or when no intrinsic
    // width could be determined. Do not add child.x / child.y, because those are
    // relative positions from a previous layout pass and will be stale when a
    // parent re-lays out this child in a new container.
    if is_row {
        child.width.max(0.0)
    } else {
        child.height.max(0.0)
    }
}

/// Resolve a flex item's content-box main-axis width against the flex
/// container's content width, applying `width`/`flex-basis` and
/// `max-width`/`min-width`. Percentage limits resolve against the flex
/// container, not the item's own basis. `intrinsic_content_width` is the
/// content width the item had when measured at max-content and is only used
/// when no definite `width`/`flex-basis` is supplied.
fn flex_item_resolved_content_width(
    style: &ComputedStyle,
    container_content_width: f32,
    intrinsic_content_width: f32,
) -> f32 {
    let is_border_box = style.box_sizing == incognidium_style::BoxSizing::BorderBox;
    let pb_width = style.padding_left
        + style.padding_right
        + style.border_left_width
        + style.border_right_width;

    // Determine the target total main-axis size from flex-basis or width.
    let mut total_width = match style.flex_basis {
        SizeValue::Px(v) => v,
        SizeValue::Percent(p) => container_content_width * p / 100.0,
        SizeValue::Auto | SizeValue::None => match style.width {
            SizeValue::Px(w) => w,
            SizeValue::Percent(p) => container_content_width * p / 100.0,
            SizeValue::Auto | SizeValue::None => {
                if is_border_box {
                    intrinsic_content_width + pb_width
                } else {
                    intrinsic_content_width
                }
            }
            _ => {
                if let Some(v) =
                    evaluate_size_value(&style.width, container_content_width, style.font_size)
                {
                    v
                } else if is_border_box {
                    intrinsic_content_width + pb_width
                } else {
                    intrinsic_content_width
                }
            }
        },
        _ => {
            if let Some(v) =
                evaluate_size_value(&style.flex_basis, container_content_width, style.font_size)
            {
                v
            } else {
                0.0
            }
        }
    };

    // Apply max-width and min-width. They resolve against the flex container's
    // content width (the containing block), not the item's own basis.
    let mut apply_width_limit = |limit: &SizeValue, is_max: bool| {
        let resolved = match *limit {
            SizeValue::Px(v) => Some(v),
            SizeValue::Percent(p) => Some(container_content_width * p / 100.0),
            SizeValue::Calc(_)
            | SizeValue::Min(_)
            | SizeValue::Max(_)
            | SizeValue::Clamp { .. } => {
                evaluate_size_value(limit, container_content_width, style.font_size)
            }
            _ => None,
        };
        if let Some(v) = resolved {
            if is_max && total_width > v {
                total_width = v;
            } else if !is_max && total_width < v {
                total_width = v;
            }
        }
    };

    apply_width_limit(&style.max_width, true);
    apply_width_limit(&style.min_width, false);

    if is_border_box {
        (total_width - pb_width).max(0.0)
    } else {
        total_width.max(0.0)
    }
}

/// Resolve a column flex item's cross-axis (width) size, including min/max-width.
///
/// In a column flex container the main axis is vertical, so `flex-basis` does not
/// determine the item's width. This function evaluates `width`/`max-width`/`min-width`
/// against the flex container's content width and returns the content-box width.
fn flex_cross_item_resolved_content_width(
    style: &ComputedStyle,
    container_content_width: f32,
    intrinsic_content_width: f32,
) -> f32 {
    let is_border_box = style.box_sizing == incognidium_style::BoxSizing::BorderBox;
    let pb_width = style.padding_left
        + style.padding_right
        + style.border_left_width
        + style.border_right_width;

    // Determine the target total cross-axis width from `width` only.
    let mut total_width = match style.width {
        SizeValue::Px(w) => w,
        SizeValue::Percent(p) => container_content_width * p / 100.0,
        SizeValue::Auto | SizeValue::None => {
            if is_border_box {
                intrinsic_content_width + pb_width
            } else {
                intrinsic_content_width
            }
        }
        _ => {
            if let Some(v) =
                evaluate_size_value(&style.width, container_content_width, style.font_size)
            {
                v
            } else if is_border_box {
                intrinsic_content_width + pb_width
            } else {
                intrinsic_content_width
            }
        }
    };

    // Apply max-width and min-width. They resolve against the flex container's
    // content width (the containing block), not the item's own intrinsic size.
    let mut apply_width_limit = |limit: &SizeValue, is_max: bool| {
        let resolved = match *limit {
            SizeValue::Px(v) => Some(v),
            SizeValue::Percent(p) => Some(container_content_width * p / 100.0),
            SizeValue::Calc(_)
            | SizeValue::Min(_)
            | SizeValue::Max(_)
            | SizeValue::Clamp { .. } => {
                evaluate_size_value(limit, container_content_width, style.font_size)
            }
            _ => None,
        };
        if let Some(v) = resolved {
            if is_max && total_width > v {
                total_width = v;
            } else if !is_max && total_width < v {
                total_width = v;
            }
        }
    };

    apply_width_limit(&style.max_width, true);
    apply_width_limit(&style.min_width, false);

    if is_border_box {
        (total_width - pb_width).max(0.0)
    } else {
        total_width.max(0.0)
    }
}

/// Clamp a flex item's total main-axis size by its max-width/min-width limits,
/// resolving percentages against the flex container's content width.
fn flex_item_clamp_main_total(
    total_width: f32,
    style: &ComputedStyle,
    container_content_width: f32,
) -> f32 {
    let is_border_box = style.box_sizing == incognidium_style::BoxSizing::BorderBox;
    let pb_width = style.padding_left
        + style.padding_right
        + style.border_left_width
        + style.border_right_width;

    let mut clamped = if is_border_box {
        total_width
    } else {
        // total_width passed in is already the content width for content-box;
        // apply limits directly.
        total_width
    };

    let mut apply_width_limit = |limit: &SizeValue, is_max: bool| {
        let resolved = match *limit {
            SizeValue::Px(v) => Some(v),
            SizeValue::Percent(p) => Some(container_content_width * p / 100.0),
            SizeValue::Calc(_)
            | SizeValue::Min(_)
            | SizeValue::Max(_)
            | SizeValue::Clamp { .. } => {
                evaluate_size_value(limit, container_content_width, style.font_size)
            }
            _ => None,
        };
        if let Some(v) = resolved {
            if is_max && clamped > v {
                clamped = v;
            } else if !is_max && clamped < v {
                clamped = v;
            }
        }
    };

    apply_width_limit(&style.max_width, true);
    apply_width_limit(&style.min_width, false);

    // Avoid shrinking below the element's own padding/border for border-box.
    if is_border_box {
        clamped.max(pb_width)
    } else {
        clamped.max(0.0)
    }
}

/// Clamp a flex item's total main-axis height to its min/max-height constraints,
/// respecting box-sizing. This mirrors `flex_item_clamp_main_total` for the block
/// axis so column flex items (e.g. a top ad slot container) honor a
/// definite `min-height` even when they have no flex-grow/shrink distribution.
fn flex_item_clamp_main_total_height(
    total_height: f32,
    style: &ComputedStyle,
    container_content_height: f32,
) -> f32 {
    let is_border_box = style.box_sizing == incognidium_style::BoxSizing::BorderBox;
    let pb_height = style.padding_top
        + style.padding_bottom
        + style.border_top_width
        + style.border_bottom_width;

    let mut clamped = total_height;

    let mut apply_height_limit = |limit: &SizeValue, is_max: bool| {
        let resolved = match *limit {
            SizeValue::Px(v) => Some(v),
            SizeValue::Percent(p) => Some(container_content_height * p / 100.0),
            SizeValue::Calc(_)
            | SizeValue::Min(_)
            | SizeValue::Max(_)
            | SizeValue::Clamp { .. } => {
                evaluate_size_value(limit, container_content_height, style.font_size)
            }
            _ => None,
        };
        if let Some(v) = resolved {
            if is_max && clamped > v {
                clamped = v;
            } else if !is_max && clamped < v {
                clamped = v;
            }
        }
    };

    apply_height_limit(&style.max_height, true);
    apply_height_limit(&style.min_height, false);

    if is_border_box {
        clamped.max(pb_height)
    } else {
        clamped.max(0.0)
    }
}

fn layout_flex(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    containing_height: f32,
    image_sizes: &ImageSizes,
) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    let padding_left = style.padding_left_px(containing_width);
    let padding_right = style.padding_right_px(containing_width);
    let padding_top = style.padding_top_px(containing_width);
    let padding_bottom = style.padding_bottom_px(containing_width);
    let border_left = style.border_left_width;
    let border_right = style.border_right_width;
    let border_top = style.border_top_width;
    let border_bottom = style.border_bottom_width;

    let is_border_box = style.box_sizing == incognidium_style::BoxSizing::BorderBox;

    // A parent flex container may have already resolved this item's content-box
    // main size against the container. Honor that forced width instead of
    // recomputing it from the item's own style, so percentage widths/flex-basis
    // inside nested flex containers do not collapse the item a second time.
    let content_width = if let Some(forced) = layout_box.forced_content_width.take() {
        forced.max(0.0)
    } else {
        match style.width {
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
            // CSS Math Functions (calc()/min()/max()/clamp()) are definite when
            // they evaluate; fall back to auto otherwise.
            SizeValue::Calc(_)
            | SizeValue::Min(_)
            | SizeValue::Max(_)
            | SizeValue::Clamp { .. } => {
                evaluate_size_value(&style.width, containing_width, style.font_size)
                    .map(|w| {
                        if is_border_box {
                            (w - padding_left - padding_right - border_left - border_right).max(0.0)
                        } else {
                            w
                        }
                    })
                    .unwrap_or_else(|| {
                        containing_width
                            - style.margin_left
                            - style.margin_right
                            - padding_left
                            - padding_right
                            - border_left
                            - border_right
                    })
            }
            _ => {
                containing_width
                    - style.margin_left
                    - style.margin_right
                    - padding_left
                    - padding_right
                    - border_left
                    - border_right
            }
        }
    };

    // Clamp the resolved width by max-width/min-width. Math functions
    // (calc()/min()/max()/clamp()) are definite when they evaluate.
    let content_width = {
        let mut w = content_width.max(0.0);
        let max_w = match &style.max_width {
            SizeValue::Px(v) => Some(*v),
            SizeValue::Percent(p) => Some(containing_width * p / 100.0),
            SizeValue::Calc(_)
            | SizeValue::Min(_)
            | SizeValue::Max(_)
            | SizeValue::Clamp { .. } => {
                evaluate_size_value(&style.max_width, containing_width, style.font_size)
            }
            _ => None,
        };
        let min_w = match &style.min_width {
            SizeValue::Px(v) => Some(*v),
            SizeValue::Percent(p) => Some(containing_width * p / 100.0),
            SizeValue::Calc(_)
            | SizeValue::Min(_)
            | SizeValue::Max(_)
            | SizeValue::Clamp { .. } => {
                evaluate_size_value(&style.min_width, containing_width, style.font_size)
            }
            _ => None,
        };
        if let Some(mw) = max_w {
            w = w.min(mw.max(0.0));
        }
        if let Some(mw) = min_w {
            w = w.max(mw);
        }
        w
    };
    layout_box.content_width = content_width;
    layout_box.width =
        layout_box.content_width + padding_left + padding_right + border_left + border_right;

    let is_row = matches!(
        style.flex_direction,
        FlexDirection::Row | FlexDirection::RowReverse
    );
    let is_row_reverse = style.flex_direction == FlexDirection::RowReverse;

    // CSS gap uses row-gap/column-gap. For row flex the main-axis gap is
    // column-gap; for column flex it is row-gap. Fall back to the legacy
    // single-value gap when the directional gap is zero.
    let main_gap = if is_row {
        if style.column_gap > 0.0 {
            style.column_gap
        } else {
            style.gap
        }
    } else if style.row_gap > 0.0 {
        style.row_gap
    } else {
        style.gap
    };
    let cross_gap = if is_row {
        style.row_gap
    } else {
        style.column_gap
    };

    let wrapping = style.flex_wrap != FlexWrap::NoWrap;

    let pb_height = padding_top + padding_bottom + border_top + border_bottom;
    // Resolve an explicit total height (content-box) from style.height. Percentage
    // heights resolve against the containing block when it is definite, matching
    // block layout. This lets body/html height:100% and column flex wrappers like
    // generic `#page`/`#app` wrappers fill the viewport instead of collapsing to content.
    let explicit_content_height: Option<f32> = match style.height {
        SizeValue::Px(h) => Some((h - pb_height).max(0.0)),
        SizeValue::Percent(p) if containing_height > 0.0 => {
            Some(((containing_height * p / 100.0) - pb_height).max(0.0))
        }
        SizeValue::Calc(_) | SizeValue::Min(_) | SizeValue::Max(_) | SizeValue::Clamp { .. } => {
            evaluate_size_value(&style.height, containing_height, style.font_size)
                .map(|h| (h - pb_height).max(0.0))
        }
        _ => None,
    };

    // When the flex container has no explicit height but does have an
    // aspect-ratio, derive a definite main-axis (column) / cross-axis (row)
    // size from its content-box width. This lets column flex containers use
    // justify-content:center to position text at the bottom of an aspect-ratio
    // box instead of collapsing to the top.
    let explicit_content_height = explicit_content_height.or_else(|| {
        if let Some(ref ar) = style.aspect_ratio {
            let ratio = ar.width / ar.height.max(0.001);
            if ratio > 0.0 && layout_box.content_width > 0.0 {
                Some((layout_box.content_width / ratio - pb_height).max(0.0))
            } else {
                None
            }
        } else {
            None
        }
    });

    // When an absolutely/fixed positioned flex container has both vertical insets
    // resolved and auto height, the absolute-positioning pass stretches it to
    // fill the available space and stores that stretched content height before
    // re-laying it out. Use that height as a definite main-axis size so
    // justify-content:center (and wrapping) works inside the stretched box
    // instead of collapsing to the natural content height, such as a hero overlay.
    let explicit_content_height = explicit_content_height.or_else(|| {
        if (style.position == Position::Absolute || style.position == Position::Fixed)
            && !matches!(style.top, SizeValue::Auto | SizeValue::None)
            && !matches!(style.bottom, SizeValue::Auto | SizeValue::None)
            && layout_box.content_height > 0.0
        {
            Some(layout_box.content_height)
        } else {
            None
        }
    });

    // Container main-axis size for wrapping decisions
    let container_main = if is_row {
        content_width
    } else {
        // Column flex with an explicit height (or aspect-ratio) wraps against
        // that height; auto height uses a sentinel so items do not wrap.
        explicit_content_height.unwrap_or(f32::MAX)
    };

    // Compute the explicit container cross-axis size if any (for row wrapping)
    let container_cross_explicit = if is_row {
        explicit_content_height
    } else {
        Some(content_width)
    };
    // For row flex containers with an explicit height, children with percentage
    // heights (e.g. `height: 100%` logos) need a definite containing block height
    // to resolve against. Otherwise they fall back to their intrinsic size and
    // blow up the flex line cross-axis.
    let row_cross_height = if is_row {
        container_cross_explicit.unwrap_or(0.0)
    } else {
        0.0
    };

    // Blockify inline flex children (CSS spec: flex items are blockified)
    for child in &mut layout_box.children {
        if child.box_type == BoxType::Inline {
            child.box_type = BoxType::Block;
        }
    }

    // Identify outside list markers. They are not flex items and are positioned
    // separately in the left padding area after the flex items are laid out.
    let outside_marker_ids: std::collections::HashSet<NodeId> = layout_box
        .children
        .iter()
        .filter(|c| c.is_list_marker && c.list_style_position == ListStylePosition::Outside)
        .map(|c| c.node_id)
        .collect();

    // Remove absolutely/fixed positioned children from flex flow
    let abs_child_ids: Vec<NodeId> = layout_box
        .children
        .iter()
        .filter(|c| {
            let cs = styles.get(&c.node_id).cloned().unwrap_or_default();
            cs.position == Position::Absolute || cs.position == Position::Fixed
        })
        .map(|c| c.node_id)
        .collect();

    // Sort children by CSS order property (stable sort preserves source order for same value)
    layout_box
        .children
        .sort_by_key(|child| styles.get(&child.node_id).map(|s| s.order).unwrap_or(0));

    // First pass: compute natural sizes of children that participate in flex layout.
    // Outside markers and absolute children are skipped here.
    let num_children = layout_box
        .children
        .iter()
        .filter(|c| !abs_child_ids.contains(&c.node_id) && !outside_marker_ids.contains(&c.node_id))
        .count();
    let mut base_sizes: Vec<f32> = vec![0.0; layout_box.children.len()];
    let mut is_auto_basis: Vec<bool> = vec![false; layout_box.children.len()];
    for (i, child) in layout_box.children.iter_mut().enumerate() {
        if abs_child_ids.contains(&child.node_id) || outside_marker_ids.contains(&child.node_id) {
            continue;
        }
        let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
        let basis = match child_style.flex_basis {
            SizeValue::Px(v) => v,
            SizeValue::Percent(p) => {
                if is_row && content_width <= 10000.0 {
                    content_width * p / 100.0
                } else {
                    0.0
                }
            }
            SizeValue::Auto | SizeValue::None => {
                // Auto basis: try width (row) or height (column). Math functions
                // are evaluated so that e.g. `width: calc(25% - 1rem)` supplies a
                // real flex basis; otherwise fall back to measuring content.
                if is_row {
                    match child_style.width {
                        SizeValue::Px(w) => w,
                        SizeValue::Percent(p) => {
                            if content_width <= 10000.0 {
                                content_width * p / 100.0
                            } else {
                                0.0
                            }
                        }
                        SizeValue::Auto | SizeValue::None => 0.0, // Will be determined by content
                        _ => {
                            if content_width <= 10000.0 {
                                evaluate_size_value(
                                    &child_style.width,
                                    content_width,
                                    child_style.font_size,
                                )
                                .unwrap_or(0.0)
                            } else {
                                0.0
                            }
                        }
                    }
                } else {
                    match child_style.height {
                        SizeValue::Px(h) => h,
                        SizeValue::Percent(p) => {
                            if content_width <= 10000.0 {
                                content_width * p / 100.0
                            } else {
                                0.0
                            }
                        }
                        SizeValue::Auto | SizeValue::None => 0.0,
                        _ => {
                            if content_width <= 10000.0 {
                                evaluate_size_value(
                                    &child_style.height,
                                    content_width,
                                    child_style.font_size,
                                )
                                .unwrap_or(0.0)
                            } else {
                                0.0
                            }
                        }
                    }
                }
            }
            _ => {
                // Explicit `flex-basis: calc(...)` / min / max / clamp. Evaluate it
                // against the flex container's content width so it participates in
                // line grouping and flex-grow/shrink distribution. Skip evaluation
                // during the huge max-content measuring pass to avoid inflating
                // the base size artificially.
                if content_width <= 10000.0 {
                    evaluate_size_value(
                        &child_style.flex_basis,
                        content_width,
                        child_style.font_size,
                    )
                    .unwrap_or(0.0)
                } else {
                    0.0
                }
            }
        };

        // Distinguish:
        //   - true "auto" flex basis: `flex-basis: auto` (or unset) with no
        //     explicit width/height to supply the basis, so the item must be
        //     measured at max-content.
        //   - explicit zero basis (`0` or `0%`): common in `flex: 1 0 0` and must
        //     start from 0 so it can grow into the container. Measuring
        //     max-content here made `flex-shrink: 0` items overflow by thousands
        //     of pixels because they could not shrink back from max-content.
        //   - explicit non-zero basis (e.g. `flex-basis: 25%` or `width: 25%`
        //     with `flex-basis: auto`): the resolved value is the real basis and
        //     must not be re-measured at max-content.
        let is_auto_basis_value =
            matches!(child_style.flex_basis, SizeValue::Auto | SizeValue::None);
        let is_zero_basis = !is_auto_basis_value
            && (matches!(child_style.flex_basis, SizeValue::Px(0.0))
                || matches!(child_style.flex_basis, SizeValue::Percent(0.0)));
        let is_auto_content = is_auto_basis_value && basis <= 0.0;
        // Items whose main size is not fixed by an explicit style can participate
        // in flex-grow/shrink distribution.
        is_auto_basis[i] = is_auto_content || is_zero_basis;

        if is_row {
            let width_is_percent = matches!(child_style.width, SizeValue::Percent(_));
            let initial_width = if is_auto_content {
                // When flex-basis is auto, let content determine its natural size.
                // For non-wrapping containers that establish an overflow clip
                // (e.g. horizontal carousels), do not give the item an enormous
                // measuring width, or a huge intrinsic image will max-content-size
                // the whole flex line to thousands of pixels and produce a massive
                // page height. Clamp auto-basis measurement to the container width.
                //
                // Use a generous measuring width so block-level auto-basis items
                // shrink to their content rather than filling the whole container
                // line. A wrapping flex container (e.g. a multi-item top nav) needs
                // its <li> items measured at max-content so they can share a line.
                // Clamp the resulting base size to the available main-axis space so
                // percentage-width descendants cannot blow up a non-wrapping line.
                // Measure auto-basis items at max-content so they do not greedily
                // fill the whole container and starve sibling `flex-grow` items.
                // Non-wrapping flex headers (e.g. a news site masthead) rely on
                // this: the centered logo should only use its intrinsic width,
                // leaving the remaining space for the `flex: 1 0 0` left/right spacers.
                content_width.max(10000.0)
            } else if is_auto_basis_value && width_is_percent && content_width <= 10000.0 {
                // When flex-basis is auto, percentage widths on flex items resolve
                // against the flex container's content width, not the resolved flex
                // basis. Passing the basis as the containing width made e.g. framework
                // grid columns (width:25%) resolve against 256 px and render at
                // 64 px inside a 1024 px row. If flex-basis is explicit, it wins.
                //
                // Only do this for real container widths (less than the max-content
                // measuring sentinel). Inside an indefinite auto-width container that
                // is being measured at max-content, a percentage width should behave
                // like auto and size to its content, not fill the huge measuring width.
                content_width
            } else if is_zero_basis {
                // Explicit zero basis: start from 0 width; the item will be grown
                // (or left at 0 if it does not grow) in the second pass.
                0.0
            } else {
                basis
            };
            compute_layout(child, styles, initial_width, row_cross_height, image_sizes);
            base_sizes[i] = if is_auto_content {
                // Auto-basis items start from their max-content main size. For block,
                // inline, and inline-block children, measuring them at the container
                // width makes them fill the whole line and breaks wrapping flex
                // containers (e.g. a multi-item nav). Use the intrinsic content
                // width instead and add padding/border for border-box elements. For
                // nested flex/grid containers, the already-laid-out child positions
                // give a good max-content line width, so keep using them.
                let available_main =
                    container_main - child_style.margin_left - child_style.margin_right;
                let intrinsic_main =
                    if child.box_type == BoxType::Flex || child.box_type == BoxType::InlineFlex {
                        let content = flex_item_max_content_main(child, true, styles).max(0.0);
                        if child_style.box_sizing == incognidium_style::BoxSizing::BorderBox {
                            content
                                + child_style.padding_left
                                + child_style.padding_right
                                + child_style.border_left_width
                                + child_style.border_right_width
                        } else {
                            content
                        }
                    } else {
                        let content = calculate_intrinsic_width(child, styles).max(0.0);
                        if child_style.box_sizing == incognidium_style::BoxSizing::BorderBox {
                            content
                                + child_style.padding_left
                                + child_style.padding_right
                                + child_style.border_left_width
                                + child_style.border_right_width
                        } else {
                            content
                        }
                    };
                intrinsic_main.min(available_main.max(0.0))
            } else if is_zero_basis {
                0.0
            } else {
                child.width
            };
            if is_auto_content {
                // Re-layout the auto item at its measured width so it does not
                // keep the huge measurement width when no flex-grow/shrink applies.
                // Clamp to the container width for overflow-clipped carousels.
                let final_width =
                    if !wrapping && style.overflow != incognidium_style::Overflow::Visible {
                        base_sizes[i].min(content_width.max(0.0))
                    } else {
                        base_sizes[i]
                    };
                // The measured size is the item's intrinsic main-axis size. The
                // item's containing block is the flex container's content box, so
                // percentage-based max-width/min-width must resolve against the
                // container width, not the item's own intrinsic size. Lock the
                // content width and lay the child out against the full container
                // width so those percentages resolve correctly.
                let padding_border = child_style.padding_left
                    + child_style.padding_right
                    + child_style.border_left_width
                    + child_style.border_right_width;
                let content_main =
                    if child_style.box_sizing == incognidium_style::BoxSizing::BorderBox {
                        (final_width - padding_border).max(0.0)
                    } else {
                        final_width
                    };
                child.forced_content_width = Some(content_main);
                compute_layout(child, styles, content_width, row_cross_height, image_sizes);
            }
        } else {
            // Column flex: measure against the container width. The flex base
            // size for auto items is the natural height of their contents.
            // Pass the container's definite height (if any) as the available
            // height; otherwise leave it indefinite. Using the container width as
            // a height placeholder caused replaced elements such as a site
            // wordmark SVG (height: 100%) to stretch to the cross-axis width
            // instead of their natural aspect ratio.
            let initial_height = if is_zero_basis {
                0.0
            } else {
                explicit_content_height.unwrap_or(0.0)
            };
            compute_layout(child, styles, content_width, initial_height, image_sizes);
            let column_height_is_definite = explicit_content_height.is_some();
            base_sizes[i] = if is_auto_content {
                flex_item_max_content_main(child, false, styles)
            } else if is_zero_basis && !column_height_is_definite {
                // When a column flex container has no definite height, zero-basis
                // items must start from their content size suggestion, not 0. Using
                // 0 here made `min-height` act as the container's main size and
                // truncated long content (e.g. a body with min-height: 100vh and
                // main { flex: 1; }).
                flex_item_max_content_main(child, false, styles)
            } else if is_zero_basis {
                0.0
            } else {
                child.height
            };
            if is_auto_content || (is_zero_basis && !column_height_is_definite) {
                child.height = base_sizes[i];
                child.content_height = base_sizes[i];
            }

            // Cross-axis alignment other than stretch needs the item sized to its
            // resolved width (intrinsic if `width: auto`, otherwise the explicit
            // width value). Without this pass, blockified flex items fill the
            // whole container width and centering/flex-start alignment has no
            // visible effect (e.g. a centered promo-box CTA pill). The
            // explicit-width branch keeps percentage widths like a hero grid from
            // collapsing to their intrinsic content width.
            if style.align_items != AlignItems::Stretch {
                let intrinsic_content_cross = calculate_intrinsic_width(child, styles).max(0.0);
                let target_content_width = flex_cross_item_resolved_content_width(
                    &child_style,
                    content_width,
                    intrinsic_content_cross,
                );
                if (target_content_width - child.content_width).abs() > 0.5 {
                    child.forced_content_width = Some(target_content_width);
                    compute_layout(child, styles, content_width, initial_height, image_sizes);
                    base_sizes[i] = child.height;
                    child.content_height = child.height;
                }
            }
        }
    }

    // Re-layout row flex items with their width/max-width/min-width resolved
    // against the flex container's content width. This fixes percentage
    // max-width/min-width on flex items that previously resolved against the
    // item's own resolved basis.
    if is_row {
        for (i, child) in layout_box.children.iter_mut().enumerate() {
            if abs_child_ids.contains(&child.node_id) || outside_marker_ids.contains(&child.node_id)
            {
                continue;
            }
            let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();

            // Zero-basis items keep their 0 flex base size; they will be sized
            // by flex-grow/shrink and re-laid out afterwards.
            let is_zero_basis =
                !matches!(child_style.flex_basis, SizeValue::Auto | SizeValue::None)
                    && (matches!(child_style.flex_basis, SizeValue::Px(0.0))
                        || matches!(child_style.flex_basis, SizeValue::Percent(0.0)));
            if is_zero_basis {
                continue;
            }

            let target_content_width =
                flex_item_resolved_content_width(&child_style, content_width, child.content_width);
            // The item's containing block is the flex container's content box, so
            // pass the container's full content width for percentage padding and
            // descendant resolution. Lock the item's own content width via
            // forced_content_width so it does not re-resolve against a narrower
            // relayout width.
            child.forced_content_width = Some(target_content_width);

            // Stretched row items should fill the flex line's cross size. Without
            // this, auto-height items such as an inline-SVG placeholder inside a
            // fixed-height header measure themselves from their intrinsic content
            // and expand the whole flex container (e.g. an inline-SVG wordmark
            // making a fixed-height header taller than intended).
            let child_align = match child_style.place_self.0 {
                incognidium_style::AlignSelf::Auto => style.align_items,
                incognidium_style::AlignSelf::FlexStart => incognidium_style::AlignItems::FlexStart,
                incognidium_style::AlignSelf::FlexEnd => incognidium_style::AlignItems::FlexEnd,
                incognidium_style::AlignSelf::Center => incognidium_style::AlignItems::Center,
                incognidium_style::AlignSelf::Stretch => incognidium_style::AlignItems::Stretch,
                incognidium_style::AlignSelf::Baseline => incognidium_style::AlignItems::Baseline,
            };
            // Only stretch a row flex item to a *definite* cross size. When the
            // flex container itself has an auto cross size (e.g. a grid item in an
            // auto row), forcing the item to the sentinel 0 collapses its content and
            // makes the whole row disappear. Let the item size to its intrinsic
            // content instead; the flex line's cross size will be derived from that.
            if child_align == incognidium_style::AlignItems::Stretch && row_cross_height > 0.0 {
                let pb_height = child_style.padding_top
                    + child_style.padding_bottom
                    + child_style.border_top_width
                    + child_style.border_bottom_width;
                let margin_height = child_style.margin_top + child_style.margin_bottom;
                let stretched_content_height =
                    (row_cross_height - margin_height - pb_height).max(0.0);
                child.forced_content_height = Some(stretched_content_height);
            }

            compute_layout(child, styles, content_width, row_cross_height, image_sizes);
            base_sizes[i] = child.width;
        }
    }

    // Filter out whitespace-only text nodes and outside list markers from
    // flex children (markers are positioned separately).
    let mut flex_children: Vec<usize> = Vec::new();
    for (i, child) in layout_box.children.iter().enumerate() {
        if abs_child_ids.contains(&child.node_id) || outside_marker_ids.contains(&child.node_id) {
            continue;
        }
        if child.box_type == BoxType::Text {
            if let Some(ref text) = child.text {
                if is_collapsible_whitespace_only(text) {
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
                base_sizes[i] + child_style.margin_left + child_style.margin_right
            } else {
                base_sizes[i] + child_style.margin_top + child_style.margin_bottom
            };
            let gap_before = if idx > line_start { main_gap } else { 0.0 };

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

        // Compute total main size for this line, including each item's main-axis
        // margins. Margins (including negative ones) must participate in free-space
        // distribution so that e.g. `width:100vw; margin-left:-50vw` does not get
        // flex-shrunk to fit the container.
        let line_main_size: f32 = line_child_indices
            .iter()
            .map(|i| {
                let child_style = styles
                    .get(&layout_box.children[*i].node_id)
                    .cloned()
                    .unwrap_or_default();
                base_sizes[*i]
                    + if is_row {
                        child_style.margin_left + child_style.margin_right
                    } else {
                        child_style.margin_top + child_style.margin_bottom
                    }
            })
            .sum();

        let line_gap_total = main_gap * (line_count.saturating_sub(1) as f32);

        let line_available = if is_row {
            content_width
        } else {
            // Column flex: explicit height gives items a definite main-axis to fill.
            // Otherwise use natural content size (with px min-height as a floor).
            explicit_content_height.unwrap_or_else(|| match style.min_height {
                SizeValue::Px(mh) => line_main_size.max(mh),
                _ => line_main_size,
            })
        } - line_gap_total;

        let line_free = line_available - line_main_size;

        // Distribute positive free space using flex-grow. Any item with a
        // non-zero flex-grow value participates, not just auto-basis items.
        // Fixed items (flex-grow:0) keep their resolved basis, matching the
        // flexbox algorithm.
        if line_free > 0.0 {
            let line_total_grow: f32 = line_child_indices
                .iter()
                .map(|i| {
                    styles
                        .get(&layout_box.children[*i].node_id)
                        .map(|s| s.flex_grow)
                        .unwrap_or(0.0)
                })
                .sum();
            if line_total_grow > 0.0 {
                for &i in &line_child_indices {
                    let grow = styles
                        .get(&layout_box.children[i].node_id)
                        .map(|s| s.flex_grow)
                        .unwrap_or(0.0);
                    if grow <= 0.0 {
                        continue;
                    }
                    let extra = line_free * (grow / line_total_grow);
                    base_sizes[i] += extra;
                    if is_row {
                        let child_style = styles
                            .get(&layout_box.children[i].node_id)
                            .cloned()
                            .unwrap_or_default();
                        let padding_border = child_style.padding_left
                            + child_style.padding_right
                            + child_style.border_left_width
                            + child_style.border_right_width;
                        // Clamp the flexed size by max-width/min-width, which
                        // resolve against the flex container.
                        base_sizes[i] =
                            flex_item_clamp_main_total(base_sizes[i], &child_style, content_width);
                        // The clamped base_sizes[i] is the item's total main-axis
                        // size (border-box), measured as child.width. Subtract the
                        // item's own padding/border to get the content width to
                        // force, regardless of box-sizing: box-sizing only changes
                        // how `width` is written, not the relationship between the
                        // total size and the content size. Treating a content-box
                        // item's total as its content width re-added the padding
                        // after every grow/shrink, re-inflating padded items beyond
                        // their flexed size.
                        let content_main = (base_sizes[i] - padding_border).max(0.0);
                        layout_box.children[i].forced_content_width = Some(content_main);
                        compute_layout(
                            &mut layout_box.children[i],
                            styles,
                            content_width,
                            row_cross_height,
                            image_sizes,
                        );
                    } else {
                        layout_box.children[i].height = base_sizes[i];
                        layout_box.children[i].content_height = base_sizes[i];
                    }
                }
            }
        }

        // Distribute negative free space using weighted flex-shrink. Larger items
        // shrink more than smaller ones, matching the CSS flexbox algorithm.
        // Any item with a non-zero flex-shrink value participates, including
        // explicit-basis items such as `flex-basis:100%` sidebars.
        //
        // The distribution is iterative: an item that reaches its automatic
        // minimum size (`min-width: auto` resolving to min-content, e.g. a nav
        // of unbreakable links) cannot shrink further, and its unused share of
        // the overflow must be redistributed to the items that can still shrink
        // (CSS Flexbox §9.7 "fix min/max violations"). A single proportional
        // pass left overflowing header rows stretched past the viewport.
        if line_free < 0.0 && (!wrapping || line_count == 1) {
            let mut remaining = -line_free;
            let mut frozen: Vec<bool> = line_child_indices
                .iter()
                .map(|&i| {
                    styles
                        .get(&layout_box.children[i].node_id)
                        .map(|s| s.flex_shrink)
                        .unwrap_or(1.0)
                        <= 0.0
                })
                .collect();
            let min_mains: Vec<f32> = line_child_indices
                .iter()
                .map(|&i| {
                    let child_style = styles
                        .get(&layout_box.children[i].node_id)
                        .cloned()
                        .unwrap_or_default();
                    if is_row {
                        match child_style.min_width {
                            SizeValue::Auto | SizeValue::None => {
                                flex_item_min_content_main(&layout_box.children[i], true, styles)
                            }
                            _ => evaluate_size_value(
                                &child_style.min_width,
                                content_width,
                                child_style.font_size,
                            )
                            .unwrap_or(0.0),
                        }
                    } else {
                        match child_style.min_height {
                            SizeValue::Auto | SizeValue::None => {
                                flex_item_min_content_main(&layout_box.children[i], false, styles)
                            }
                            _ => evaluate_size_value(
                                &child_style.min_height,
                                containing_height,
                                child_style.font_size,
                            )
                            .unwrap_or(0.0),
                        }
                    }
                })
                .collect();
            let shrinks: Vec<f32> = line_child_indices
                .iter()
                .map(|&i| {
                    styles
                        .get(&layout_box.children[i].node_id)
                        .map(|s| s.flex_shrink)
                        .unwrap_or(1.0)
                })
                .collect();
            for _ in 0..line_child_indices.len() {
                let total_scaled_shrink: f32 = line_child_indices
                    .iter()
                    .enumerate()
                    .filter(|(k, _)| !frozen[*k])
                    .map(|(k, &i)| base_sizes[i] * shrinks[k])
                    .sum();
                if remaining <= 0.5 || total_scaled_shrink <= 0.0 {
                    break;
                }
                let mut any_frozen = false;
                for (k, &i) in line_child_indices.iter().enumerate() {
                    if frozen[k] {
                        continue;
                    }
                    let scaled = base_sizes[i] * shrinks[k];
                    let reduction = remaining * (scaled / total_scaled_shrink);
                    let new_size = base_sizes[i] - reduction;
                    if new_size < min_mains[k] - 0.5 {
                        // The item hit its minimum: freeze it there and leave
                        // the unused reduction for the next round.
                        remaining -= base_sizes[i] - min_mains[k];
                        base_sizes[i] = min_mains[k];
                        frozen[k] = true;
                        any_frozen = true;
                    } else {
                        base_sizes[i] = new_size;
                    }
                }
                if !any_frozen {
                    break;
                }
            }
            for (k, &i) in line_child_indices.iter().enumerate() {
                let shrink = shrinks[k];
                if shrink <= 0.0 {
                    continue;
                }
                let child_style = styles
                    .get(&layout_box.children[i].node_id)
                    .cloned()
                    .unwrap_or_default();
                if is_row {
                    let padding_border = child_style.padding_left
                        + child_style.padding_right
                        + child_style.border_left_width
                        + child_style.border_right_width;
                    // Clamp the flexed size by max-width/min-width, which
                    // resolve against the flex container.
                    base_sizes[i] =
                        flex_item_clamp_main_total(base_sizes[i], &child_style, content_width);
                    // The clamped base_sizes[i] is the item's total main-axis
                    // size (border-box), measured as child.width. Subtract the
                    // item's own padding/border to get the content width to
                    // force, regardless of box-sizing: box-sizing only changes
                    // how `width` is written, not the relationship between the
                    // total size and the content size. Treating a content-box
                    // item's total as its content width re-added the padding
                    // after every grow/shrink, re-inflating padded items beyond
                    // their flexed size.
                    let content_main = (base_sizes[i] - padding_border).max(0.0);
                    layout_box.children[i].forced_content_width = Some(content_main);
                    compute_layout(
                        &mut layout_box.children[i],
                        styles,
                        content_width,
                        row_cross_height,
                        image_sizes,
                    );
                } else {
                    layout_box.children[i].height = base_sizes[i];
                    layout_box.children[i].content_height = base_sizes[i];
                }
            }
        }

        // Final min/max constraint pass. Even when there is no free space to
        // distribute, each item must still honor its own min/max constraints
        // (e.g. a column flex item with `min-height: 294px` and no flex-grow).
        for &i in &line_child_indices {
            let child_style = styles
                .get(&layout_box.children[i].node_id)
                .cloned()
                .unwrap_or_default();
            if is_row {
                let clamped =
                    flex_item_clamp_main_total(base_sizes[i], &child_style, content_width);
                if (clamped - base_sizes[i]).abs() > 0.5
                    || (clamped - layout_box.children[i].width).abs() > 0.5
                {
                    base_sizes[i] = clamped;
                    let padding_border = child_style.padding_left
                        + child_style.padding_right
                        + child_style.border_left_width
                        + child_style.border_right_width;
                    // base_sizes hold total (border-box) main sizes here, so the
                    // content width is the total minus this item's own
                    // padding/border regardless of box-sizing.
                    let content_main = (clamped - padding_border).max(0.0);
                    layout_box.children[i].forced_content_width = Some(content_main);
                    compute_layout(
                        &mut layout_box.children[i],
                        styles,
                        content_width,
                        row_cross_height,
                        image_sizes,
                    );
                }
            } else {
                let clamped = flex_item_clamp_main_total_height(
                    base_sizes[i],
                    &child_style,
                    explicit_content_height.unwrap_or(0.0),
                );
                if (clamped - base_sizes[i]).abs() > 0.5
                    || (clamped - layout_box.children[i].height).abs() > 0.5
                {
                    base_sizes[i] = clamped;
                    let padding_border = child_style.padding_top
                        + child_style.padding_bottom
                        + child_style.border_top_width
                        + child_style.border_bottom_width;
                    // base_sizes hold total main sizes here, so the content
                    // height is the total minus this item's own padding/border
                    // regardless of box-sizing.
                    let content_main = (clamped - padding_border).max(0.0);
                    layout_box.children[i].forced_content_height = Some(content_main);
                    compute_layout(
                        &mut layout_box.children[i],
                        styles,
                        content_width,
                        row_cross_height,
                        image_sizes,
                    );
                }
            }
        }

        // Position items on this line. The line's used main-axis space includes
        // each item's main-axis margins so that justify-content leaves the correct
        // amount of leftover space (negative margins can make an item consume zero
        // or even negative space, which is valid for full-bleed hacks).
        let final_line_main: f32 = line_child_indices
            .iter()
            .map(|i| {
                let c = &layout_box.children[*i];
                let cs = styles.get(&c.node_id).cloned().unwrap_or_default();
                if is_row {
                    c.width + cs.margin_left + cs.margin_right
                } else {
                    c.height + cs.margin_top + cs.margin_bottom
                }
            })
            .sum();
        let line_remaining = line_available - final_line_main;

        let (mut main_cursor, gap_between) = match style.justify_content {
            JustifyContent::FlexStart => (0.0_f32, main_gap),
            JustifyContent::FlexEnd => (line_remaining.max(0.0), main_gap),
            JustifyContent::Center => (line_remaining.max(0.0) / 2.0, main_gap),
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
                let item_total = layout_box.children[i].width
                    + child_style.margin_left
                    + child_style.margin_right;
                if is_row_reverse {
                    layout_box.children[i].x = content_x + content_width - main_cursor - item_total
                        + child_style.margin_left;
                } else {
                    layout_box.children[i].x = content_x + main_cursor + child_style.margin_left;
                }
                layout_box.children[i].y = content_y + cross_cursor + child_style.margin_top;
                main_cursor += item_total;
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
    let content_height = if let Some(h) = explicit_content_height {
        h
    } else if is_row {
        total_cross
    } else {
        // For column direction, main axis is vertical
        // Use the longest line's main cursor
        // We need to recompute: take the max main size across all lines
        let mut max_main: f32 = 0.0;
        for &(line_start, line_end) in &lines {
            let line_main: f32 = flex_children[line_start..line_end]
                .iter()
                .map(|&i| {
                    let cs = styles
                        .get(&layout_box.children[i].node_id)
                        .cloned()
                        .unwrap_or_default();
                    layout_box.children[i].height + cs.margin_top + cs.margin_bottom
                })
                .sum();
            let line_gap = main_gap * ((line_end - line_start).saturating_sub(1) as f32);
            max_main = max_main.max(line_main + line_gap);
        }
        max_main
    };

    // When height is auto, honor an explicit aspect-ratio while still allowing
    // in-flow content to make the flex container taller. Layouts commonly wrap
    // absolutely positioned cover images in a flex container with `aspect-ratio`,
    // relying on the flex wrapper to supply the intrinsic cross-axis size.
    let height_is_auto = matches!(style.height, SizeValue::Auto | SizeValue::None);
    let content_height = if height_is_auto {
        if let Some(ref ar) = style.aspect_ratio {
            let ratio = ar.width / ar.height.max(0.001);
            if ratio > 0.0 && layout_box.content_width > 0.0 {
                (layout_box.content_width / ratio).max(content_height)
            } else {
                content_height
            }
        } else {
            content_height
        }
    } else {
        content_height
    };

    // Apply min-height for flex containers (e.g. min-height: 100vh)
    let content_height = if let Some(mh) =
        evaluate_size_value(&style.min_height, containing_height, style.font_size)
    {
        content_height.max(mh)
    } else {
        content_height
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

    // For row flex, update content_width to actual children usage. This is only
    // needed during max-content intrinsic measurement (when the containing width
    // is the large sentinel), so a nested flex container can report its real
    // intrinsic width to its parent. During final layout the container has a
    // definite containing width and must keep it — block-level flex containers
    // fill their containing block, and flex items keep the size assigned by
    // their parent flex line.
    if is_row && lines.len() == 1 && containing_width >= 10000.0 {
        let (ls, le) = lines[0];
        let actual_main: f32 = (ls..le)
            .filter(|i| !abs_child_ids.contains(&layout_box.children[flex_children[*i]].node_id))
            .map(|i| {
                let child_idx = flex_children[i];
                let cs = styles
                    .get(&layout_box.children[child_idx].node_id)
                    .cloned()
                    .unwrap_or_default();
                layout_box.children[child_idx].width + cs.margin_left + cs.margin_right
            })
            .sum::<f32>()
            + main_gap * (le - ls).saturating_sub(1) as f32;
        if actual_main < layout_box.content_width {
            layout_box.content_width = actual_main;
        }
    }
    // When measured with a zero-width containing block (e.g. grid content-based
    // track sizing), a flex container with width:auto collapses to zero. Update
    // both content_width and width to the actual children usage so the container
    // reports its real intrinsic width to the track-sizing algorithm.
    if is_row && lines.len() == 1 && containing_width <= 0.0 {
        let (ls, le) = lines[0];
        let actual_main: f32 = (ls..le)
            .filter(|i| !abs_child_ids.contains(&layout_box.children[flex_children[*i]].node_id))
            .map(|i| {
                let child_idx = flex_children[i];
                let cs = styles
                    .get(&layout_box.children[child_idx].node_id)
                    .cloned()
                    .unwrap_or_default();
                layout_box.children[child_idx].width + cs.margin_left + cs.margin_right
            })
            .sum::<f32>()
            + main_gap * (le - ls).saturating_sub(1) as f32;
        layout_box.content_width = actual_main;
        layout_box.width = actual_main + padding_left + padding_right + border_left + border_right;
    }
    // For column flex with a zero-width containing block, update the container
    // width to the widest child's cross-axis size so the container reports its
    // real intrinsic width during grid content-based track sizing.
    if !is_row && containing_width <= 0.0 {
        let max_child_width: f32 = lines
            .iter()
            .flat_map(|(ls, le)| *ls..*le)
            .filter(|i| !abs_child_ids.contains(&layout_box.children[flex_children[*i]].node_id))
            .map(|i| {
                let child_idx = flex_children[i];
                let cs = styles
                    .get(&layout_box.children[child_idx].node_id)
                    .cloned()
                    .unwrap_or_default();
                layout_box.children[child_idx].width + cs.margin_left + cs.margin_right
            })
            .fold(0.0_f32, f32::max);
        layout_box.content_width = max_child_width;
        layout_box.width =
            max_child_width + padding_left + padding_right + border_left + border_right;
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
            let child_idx = flex_children[i];
            let child_style = styles
                .get(&layout_box.children[child_idx].node_id)
                .cloned()
                .unwrap_or_default();
            let child_align = match child_style.place_self.0 {
                AlignSelf::Auto => style.align_items,
                AlignSelf::FlexStart => AlignItems::FlexStart,
                AlignSelf::FlexEnd => AlignItems::FlexEnd,
                AlignSelf::Center => AlignItems::Center,
                AlignSelf::Stretch => AlignItems::Stretch,
                AlignSelf::Baseline => AlignItems::Baseline,
            };
            if is_row {
                match child_align {
                    AlignItems::Center => {
                        layout_box.children[child_idx].y = content_y
                            + cross_offset
                            + (line_cross - layout_box.children[child_idx].height) / 2.0;
                    }
                    AlignItems::FlexEnd => {
                        layout_box.children[child_idx].y = content_y + cross_offset + line_cross
                            - layout_box.children[child_idx].height
                            - child_style.margin_bottom;
                    }
                    AlignItems::Stretch => {
                        layout_box.children[child_idx].height =
                            line_cross - child_style.margin_top - child_style.margin_bottom;
                    }
                    _ => {} // FlexStart and Baseline keep default position
                }
            } else {
                match child_align {
                    AlignItems::Center => {
                        layout_box.children[child_idx].x = content_x
                            + cross_offset
                            + (line_cross - layout_box.children[child_idx].width) / 2.0;
                    }
                    AlignItems::FlexEnd => {
                        layout_box.children[child_idx].x = content_x + cross_offset + line_cross
                            - layout_box.children[child_idx].width
                            - child_style.margin_right;
                    }
                    AlignItems::Stretch => {
                        let child = &mut layout_box.children[child_idx];
                        child.width =
                            line_cross - child_style.margin_left - child_style.margin_right;
                        // Re-layout the child against its stretched cross size so nested
                        // grids/flex containers recalculate their tracks/children. Without
                        // this the child keeps the shrink-wrapped width it got during the
                        // first measuring pass.
                        let pb = child_style.padding_left
                            + child_style.padding_right
                            + child_style.border_left_width
                            + child_style.border_right_width;
                        let desired_content_width = (child.width - pb).max(0.0);
                        child.forced_content_width = Some(desired_content_width);
                        let child_height = child.height;
                        compute_layout(child, styles, line_cross, child_height, image_sizes);
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

    // Position absolutely/fixed positioned children using the final flex container
    // as their containing block. They were skipped during flex line distribution,
    // so without this pass they remain at zero size. compute_layout dispatches to
    // layout_absolute, which already sets the child's (x, y); the parent must not
    // overwrite it.
    let container_w = layout_box.width;
    let container_h = layout_box.height;
    for child in &mut layout_box.children {
        let cs = styles.get(&child.node_id).cloned().unwrap_or_default();
        if cs.position != Position::Absolute && cs.position != Position::Fixed {
            continue;
        }
        let abs_width = match cs.width {
            SizeValue::Px(w) => w,
            // Pass the original container width for percentages so layout_absolute
            // resolves them once instead of squaring them.
            _ => container_w,
        };
        compute_layout(child, styles, abs_width, container_h, image_sizes);
    }

    // Apply relative positioning offsets to flex items. Like in block layout,
    // this shifts the item from its normal-flow position without changing the
    // flex container's size. Percentage offsets resolve against the flex
    // container's content box, which is the flex item's containing block.
    let rel_container_w = layout_box.content_width;
    let rel_container_h = layout_box.content_height;
    for child in &mut layout_box.children {
        let cs = styles.get(&child.node_id).cloned().unwrap_or_default();
        if cs.position != Position::Relative {
            continue;
        }
        let content_w = rel_container_w
            - cs.padding_left
            - cs.padding_right
            - cs.border_left_width
            - cs.border_right_width;
        let offset_x = if let Some(v) =
            resolve_offset(&cs.left, rel_container_w, content_w, cs.font_size)
        {
            v
        } else if let Some(v) = resolve_offset(&cs.right, rel_container_w, content_w, cs.font_size)
        {
            -v
        } else {
            0.0
        };
        let offset_y = if let Some(v) =
            resolve_offset(&cs.top, rel_container_h, rel_container_h, cs.font_size)
        {
            v
        } else if let Some(v) =
            resolve_offset(&cs.bottom, rel_container_h, rel_container_h, cs.font_size)
        {
            -v
        } else {
            0.0
        };
        // Clamp extreme relative offsets that would push the box entirely off-canvas,
        // mirroring the guard in layout_block.
        let clamped_offset_x = if offset_x < -child.width && child.width > 0.0 {
            0.0
        } else {
            offset_x
        };
        let clamped_offset_y = if offset_y < -child.height && child.height > 0.0 {
            0.0
        } else {
            offset_y
        };
        child.x += clamped_offset_x;
        child.y += clamped_offset_y;
    }

    // Position outside list markers in the left padding/margin area, aligned
    // with the first flex line. They were excluded from the flex item flow above.
    if !outside_marker_ids.is_empty() {
        let first_item_top = layout_box
            .children
            .iter()
            .find(|c| {
                !outside_marker_ids.contains(&c.node_id) && !abs_child_ids.contains(&c.node_id)
            })
            .map(|c| c.y)
            .unwrap_or(content_y);
        for child in &mut layout_box.children {
            if !outside_marker_ids.contains(&child.node_id) {
                continue;
            }
            compute_layout(child, styles, 10000.0, 0.0, image_sizes);
            let marker_width = child.width + 5.0;
            child.x = (content_x - marker_width).max(0.0);
            child.y = first_item_top;
        }
    }
}

/// Layout an inline-flex element: establishes a flex formatting context but
/// participates in inline flow, sizing to its intrinsic width when width is auto.
fn layout_inline_flex(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    containing_height: f32,
    image_sizes: &ImageSizes,
) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    // Check if width is explicitly set (px, percent, calc, min, max, clamp)
    let has_explicit_width = matches!(
        style.width,
        SizeValue::Px(_)
            | SizeValue::Percent(_)
            | SizeValue::Calc(_)
            | SizeValue::Min(_)
            | SizeValue::Max(_)
            | SizeValue::Clamp { .. }
    );

    if !has_explicit_width {
        // For auto-width inline-flex, do a measuring pass with a large width
        // so flex items can lay out at their natural sizes. Then use the
        // actual main-axis usage as the forced content width.
        // If a parent flex container already resolved and forced the item's
        // content-box width, honor that instead of recomputing it.
        let content_width_to_use = if let Some(forced) = layout_box.forced_content_width.take() {
            forced.max(0.0)
        } else {
            let measure_width = 10_000.0_f32;
            layout_flex(
                layout_box,
                styles,
                measure_width,
                containing_height,
                image_sizes,
            );

            let is_row = matches!(
                style.flex_direction,
                FlexDirection::Row | FlexDirection::RowReverse
            );

            // After the measuring pass, read the actual used main-axis size.
            // For single-line row flex layout_flex already narrows content_width.
            // For multi-line row or column flex we compute from children.
            let measured_content_width = if is_row {
                if layout_box.content_width < measure_width - 1.0 {
                    // layout_flex narrowed content_width to actual_main
                    layout_box.content_width
                } else {
                    // Multi-line: compute the widest line from laid-out children.
                    let main_gap = if style.column_gap > 0.0 {
                        style.column_gap
                    } else {
                        style.gap
                    };
                    let mut max_line_width: f32 = 0.0;
                    let mut current_line_width: f32 = 0.0;
                    let mut count: usize = 0;
                    let mut prev_y: Option<f32> = None;
                    for child in &layout_box.children {
                        if child.box_type == BoxType::None {
                            continue;
                        }
                        let cs = styles.get(&child.node_id).cloned().unwrap_or_default();
                        let item_total = child.width + cs.margin_left + cs.margin_right;
                        if count > 0 {
                            current_line_width += main_gap;
                        }
                        current_line_width += item_total;
                        if let Some(py) = prev_y {
                            if (child.y - py).abs() > 0.5 {
                                max_line_width =
                                    max_line_width.max(current_line_width - item_total - main_gap);
                                current_line_width = item_total;
                            }
                        }
                        prev_y = Some(child.y);
                        count += 1;
                    }
                    max_line_width.max(current_line_width)
                }
            } else {
                // Column flex: width is the widest child
                let mut max_width: f32 = 0.0;
                for child in &layout_box.children {
                    if child.box_type == BoxType::None {
                        continue;
                    }
                    let cs = styles.get(&child.node_id).cloned().unwrap_or_default();
                    let item_total = child.width + cs.margin_left + cs.margin_right;
                    max_width = max_width.max(item_total);
                }
                max_width
            };

            measured_content_width
        };

        // Apply max-width and min-width constraints
        let mut final_content_width = content_width_to_use;
        match style.max_width {
            SizeValue::Px(mw) if final_content_width > mw => final_content_width = mw,
            SizeValue::Percent(p) => {
                let mw = containing_width * p / 100.0;
                if final_content_width > mw {
                    final_content_width = mw;
                }
            }
            // Math functions (calc()/min()/max()/clamp()) are definite when
            // they evaluate.
            SizeValue::Calc(_)
            | SizeValue::Min(_)
            | SizeValue::Max(_)
            | SizeValue::Clamp { .. } => {
                if let Some(mw) =
                    evaluate_size_value(&style.max_width, containing_width, style.font_size)
                {
                    if final_content_width > mw {
                        final_content_width = mw;
                    }
                }
            }
            _ => {}
        }
        match style.min_width {
            SizeValue::Px(mw) if final_content_width < mw => final_content_width = mw,
            SizeValue::Percent(p) => {
                let mw = containing_width * p / 100.0;
                if final_content_width < mw {
                    final_content_width = mw;
                }
            }
            SizeValue::Calc(_)
            | SizeValue::Min(_)
            | SizeValue::Max(_)
            | SizeValue::Clamp { .. } => {
                if let Some(mw) =
                    evaluate_size_value(&style.min_width, containing_width, style.font_size)
                {
                    if final_content_width < mw {
                        final_content_width = mw;
                    }
                }
            }
            _ => {}
        }

        layout_box.forced_content_width = Some(final_content_width);
    }

    // Final layout pass: forced_content_width overrides auto width.
    layout_flex(
        layout_box,
        styles,
        containing_width,
        containing_height,
        image_sizes,
    );
}

fn layout_grid(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    containing_height: f32,
    image_sizes: &ImageSizes,
) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    let padding_left = style.padding_left_px(containing_width);
    let padding_right = style.padding_right_px(containing_width);
    let padding_top = style.padding_top_px(containing_width);
    let padding_bottom = style.padding_bottom_px(containing_width);
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
    let mut content_width = content_width.max(0.0);

    // When a grid container is itself a grid item, the parent grid sets the
    // child's content box width via forced_content_width. Respect it instead of
    // re-deriving a wider width from style.width/height.
    if let Some(forced) = layout_box.forced_content_width.take() {
        content_width = forced.max(0.0);
    }

    let num_children = layout_box.children.len();

    if num_children == 0 {
        layout_box.content_width = content_width;
        layout_box.width =
            layout_box.content_width + padding_left + padding_right + border_left + border_right;
        layout_box.content_height = 0.0;
        layout_box.height = padding_top + padding_bottom + border_top + border_bottom;
        return;
    }

    let col_gap = style.column_gap;
    let row_gap = style.row_gap;

    // Expand deferred repeat() tracks using the actual container content width.
    // This is essential for repeat(auto-fill, ...) so we don't hard-code a
    // 1024px viewport at CSS parse time.
    let expanded_template_columns = expand_repeats(
        &style.grid_template_columns,
        content_width,
        col_gap,
        style.font_size,
        content_width,
        content_width,
    );
    let expanded_template_rows = expand_repeats(
        &style.grid_template_rows,
        containing_height,
        row_gap,
        style.font_size,
        content_width,
        content_width,
    );

    // grid-template-areas can override the number of columns
    // Each row in grid-template-areas defines column positions
    let num_cols_from_areas = style.grid_template_areas.iter().map(|row| row.len()).max();
    let mut num_cols = if expanded_template_columns.is_empty() {
        num_cols_from_areas.unwrap_or(1)
    } else {
        expanded_template_columns.len()
    };

    // Column-based auto-flow with explicit rows but no explicit columns needs
    // enough implicit columns to hold the grid items. Without this the grid
    // collapses to a single column and later items overwrite earlier ones,
    // breaking carousels and multi-column lists (e.g. a language dropdown).
    let is_column_flow = matches!(
        style.grid_auto_flow,
        incognidium_style::GridAutoFlow::Column | incognidium_style::GridAutoFlow::ColumnDense
    );
    let num_explicit_rows = expanded_template_rows.len();
    if expanded_template_columns.is_empty() && is_column_flow && num_explicit_rows > 0 {
        let grid_item_count = layout_box
            .children
            .iter()
            .filter(|c| {
                if c.box_type == BoxType::Text {
                    if let Some(ref text) = c.text {
                        return !is_collapsible_whitespace_only(text);
                    }
                }
                true
            })
            .count();
        let required_cols = ((grid_item_count + num_explicit_rows - 1) / num_explicit_rows).max(1);
        if required_cols > num_cols {
            num_cols = required_cols;
        }
    }

    // Column widths will be resolved after placement so that `min-content`,
    // `max-content`, and similar content-based tracks can be measured from the
    // items that occupy them.
    // Get auto-column size for implicit columns
    let auto_col_size = style
        .grid_auto_columns
        .first()
        .map(|t| match t {
            incognidium_style::GridTrackSize::Px(px) => *px,
            incognidium_style::GridTrackSize::Percent(p) => content_width * p / 100.0,
            incognidium_style::GridTrackSize::Calc(expr) => expr
                .evaluate(style.font_size, content_width, content_width, content_width)
                .max(0.0),
            _ => 100.0, // Default fallback
        })
        .unwrap_or(100.0); // Default if not specified

    // Resolve explicit row heights
    let explicit_row_tracks = &expanded_template_rows;
    let content_x = padding_left + border_left;
    let content_y = padding_top + border_top;

    // Grid placement: resolve each child's (col_start, col_end, row_start, row_end).
    // CSS grid lines are 1-indexed. Negative values count from the end.
    // Children without explicit placement get auto-placed into the next free cell.

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

    /// Find the first row in which `col_start..col_start+col_span` is free for
    /// `row_span` rows. Used for grid items with a definite column but an
    /// auto-placed row so that consecutive items in the same column do not pile
    /// up at row 0 (e.g. a homepage grid).
    fn find_first_free_row_for_columns(
        occupied: &mut Vec<Vec<bool>>,
        col_start: usize,
        col_span: usize,
        row_span: usize,
        num_cols: usize,
    ) -> usize {
        // Clamp the requested span to the columns that actually exist so a span
        // larger than the grid cannot loop forever looking for free space.
        let col_start = col_start.min(num_cols.saturating_sub(1));
        let col_span = col_span.min(num_cols.saturating_sub(col_start)).max(1);
        let mut row: usize = 0;
        let mut iterations = 0;
        const MAX_ITERATIONS: usize = 10_000;
        loop {
            iterations += 1;
            if iterations > MAX_ITERATIONS {
                // Safety fallback: place far enough below existing content.
                return occupied.len();
            }
            ensure_rows(occupied, row + row_span, num_cols);
            let fits = col_start + col_span <= num_cols
                && (0..row_span)
                    .all(|dr| (0..col_span).all(|dc| !occupied[row + dr][col_start + dc]));
            if fits {
                return row;
            }
            row += 1;
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
        // Clamp the requested span to the grid width so a span larger than the
        // grid does not scan indefinitely.
        let col_span = col_span.min(num_cols).max(1);
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
        // Clamp the requested span to the grid width so a span larger than the
        // grid does not scan indefinitely.
        let col_span = col_span.min(num_cols).max(1);
        // Column-based auto-flow: fill columns first.
        // When the grid has explicit rows, we pack items top-to-bottom in each
        // column before moving to the next column. When there are no explicit
        // rows, the implicit row is shared by all columns, so we place items
        // left-to-right across columns before creating new implicit rows.
        // This matches browsers for column-based carousels and highlight grids.
        let mut iterations = 0;
        const MAX_ITERATIONS: usize = 10_000;
        if num_explicit_rows == 0 {
            loop {
                iterations += 1;
                if iterations > MAX_ITERATIONS {
                    return (*auto_col, *auto_row);
                }
                ensure_rows(occupied, *auto_row + row_span, num_cols);
                let mut c = *auto_col;
                while c + col_span <= num_cols {
                    let fits = (0..row_span)
                        .all(|dr| (0..col_span).all(|dc| !occupied[*auto_row + dr][c + dc]));
                    if fits {
                        let result = (c, *auto_row);
                        *auto_col = c + col_span;
                        if *auto_col >= num_cols {
                            *auto_col = 0;
                            *auto_row += row_span;
                        }
                        return result;
                    }
                    c += 1;
                }
                // No free column span in this implicit row; create a new row.
                *auto_row += 1;
                *auto_col = 0;
            }
        }
        loop {
            iterations += 1;
            if iterations > MAX_ITERATIONS {
                return (*auto_col, *auto_row);
            }
            ensure_rows(occupied, *auto_row + row_span, num_cols);

            let row_limit = if *auto_col + col_span <= num_cols {
                num_explicit_rows
            } else {
                usize::MAX // No limit for implicit columns
            };

            if *auto_row >= row_limit || *auto_row + row_span > row_limit {
                *auto_row = 0;
                *auto_col += col_span;
                continue;
            }

            let fits = if *auto_col + col_span <= num_cols {
                *auto_row + row_span <= occupied.len()
                    && (0..row_span)
                        .all(|dr| (0..col_span).all(|dc| !occupied[*auto_row + dr][*auto_col + dc]))
            } else {
                true
            };

            if fits {
                let result = (*auto_col, *auto_row);
                *auto_row += row_span;
                return result;
            }

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

    // Resolve a GridLine (number, named line, or shorthand area reference) to a
    // 1-indexed CSS grid line. Named lines are looked up in the container's
    // stored line-name maps. If a bare name is not defined, the standard CSS
    // Grid area fallback uses `<name>-start` for start properties and
    // `<name>-end` for end properties. Shorthand area references (`grid-column:
    // <name>`) always use the area bounds, even when a line with the bare name
    // exists.
    let resolve_grid_line = |line: &Option<GridLine>,
                             names: &std::collections::HashMap<String, Vec<usize>>,
                             max_tracks: usize,
                             is_end: bool|
     -> Option<i32> {
        let fallback_name = |name: &str| {
            if is_end {
                format!("{}-end", name)
            } else {
                format!("{}-start", name)
            }
        };
        match line {
            None => None,
            Some(GridLine::Number(n)) => Some(*n),
            Some(GridLine::Name(name)) => {
                // Exact line name takes precedence.
                if let Some(v) = names.get(name) {
                    return v
                        .first()
                        .map(|&idx| ((idx as i32) + 1).min(max_tracks as i32 + 1));
                }
                // Standard area fallback for longhand properties.
                names
                    .get(&fallback_name(name))
                    .and_then(|v| v.first())
                    .map(|&idx| {
                        // idx is the 0-indexed track position before which the line appears;
                        // convert to 1-indexed CSS line number, clamped to the valid range.
                        ((idx as i32) + 1).min(max_tracks as i32 + 1)
                    })
            }
            Some(GridLine::Area(name)) => {
                // Shorthand area references always use the implicit area lines.
                names
                    .get(&fallback_name(name))
                    .and_then(|v| v.first())
                    .map(|&idx| ((idx as i32) + 1).min(max_tracks as i32 + 1))
            }
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

    // grid-template-areas also creates implicit named lines for every area
    // boundary: <area>-start and <area>-end for both rows and columns. Merge
    // these with the explicit line names from grid-template-* so items can place
    // themselves with grid-row: header-start / header-end even when the row
    // tracks themselves are not explicitly named.
    let mut col_line_names = style.grid_template_columns_names.clone();
    let mut row_line_names = style.grid_template_rows_names.clone();
    for (area_name, &(r0, c0, r1, c1)) in &area_lookup {
        col_line_names
            .entry(format!("{}-start", area_name))
            .or_default()
            .push(c0);
        col_line_names
            .entry(format!("{}-end", area_name))
            .or_default()
            .push(c1);
        row_line_names
            .entry(format!("{}-start", area_name))
            .or_default()
            .push(r0);
        row_line_names
            .entry(format!("{}-end", area_name))
            .or_default()
            .push(r1);
    }

    for (child_idx, child) in layout_box.children.iter().enumerate() {
        // Skip whitespace-only text nodes - they shouldn't be grid items
        if child.box_type == BoxType::Text {
            if let Some(ref text) = child.text {
                if is_collapsible_whitespace_only(text) {
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

        let col_line_start =
            resolve_grid_line(&cs.grid_column_start, &col_line_names, num_cols, false);
        let col_line_end = resolve_grid_line(&cs.grid_column_end, &col_line_names, num_cols, true);
        let row_line_start = resolve_grid_line(&cs.grid_row_start, &row_line_names, 100, false);
        let row_line_end = resolve_grid_line(&cs.grid_row_end, &row_line_names, 100, true);

        let has_col = col_line_start.is_some() || col_line_end.is_some();
        let has_row = row_line_start.is_some() || row_line_end.is_some();
        let col_span = cs.grid_column_span.unwrap_or(1).max(1) as usize;
        let row_span = cs.grid_row_span.unwrap_or(1).max(1) as usize;

        let (col_start, col_end, row_start, row_end) = if has_col || has_row {
            if has_col && has_row {
                // Both grid lines are definite: place the item exactly where
                // specified.
                let c0 = col_line_start
                    .map(|v| resolve_line(v, num_cols))
                    .unwrap_or(0);
                let r0 = row_line_start
                    .map(|v| resolve_line(v, 100))
                    .unwrap_or(auto_row);
                let c1 = col_line_end
                    .map(|v| resolve_line(v, num_cols))
                    .unwrap_or_else(|| {
                        ((c0 as i32) + col_span as i32).min(num_cols as i32).max(0) as usize
                    });
                let r1 = row_line_end
                    .map(|v| resolve_line(v, 100))
                    .unwrap_or_else(|| ((r0 as i32) + row_span as i32).max(0) as usize);
                (c0, c1, r0, r1)
            } else if has_col {
                // Definite column but auto row: find the first free row in that
                // column span instead of stacking every item at row 0.
                let c0 = col_line_start
                    .map(|v| resolve_line(v, num_cols))
                    .unwrap_or(0);
                let r0 = row_line_start
                    .map(|v| resolve_line(v, 100))
                    .unwrap_or_else(|| {
                        find_first_free_row_for_columns(
                            &mut occupied,
                            c0,
                            col_span,
                            row_span,
                            num_cols,
                        )
                    });
                let c1 = col_line_end
                    .map(|v| resolve_line(v, num_cols))
                    .unwrap_or_else(|| {
                        ((c0 as i32) + col_span as i32).min(num_cols as i32).max(0) as usize
                    });
                let r1 = row_line_end
                    .map(|v| resolve_line(v, 100))
                    .unwrap_or_else(|| ((r0 as i32) + row_span as i32).max(0) as usize);
                (c0, c1, r0, r1)
            } else {
                // Definite row but auto column: place in the first free columns
                // of the specified row.
                let r0 = row_line_start
                    .map(|v| resolve_line(v, 100))
                    .unwrap_or(auto_row);
                let c0 = if col_line_start.is_some() {
                    col_line_start
                        .map(|v| resolve_line(v, num_cols))
                        .unwrap_or(0)
                } else {
                    auto_row = r0;
                    auto_col = 0;
                    let (c, _r) = find_next_free_row(
                        &mut occupied,
                        col_span,
                        row_span,
                        num_cols,
                        &mut auto_row,
                        &mut auto_col,
                    );
                    c
                };
                let c1 = col_line_end
                    .map(|v| resolve_line(v, num_cols))
                    .unwrap_or_else(|| {
                        ((c0 as i32) + col_span as i32).min(num_cols as i32).max(0) as usize
                    });
                let r1 = row_line_end
                    .map(|v| resolve_line(v, 100))
                    .unwrap_or_else(|| ((r0 as i32) + row_span as i32).max(0) as usize);
                (c0, c1, r0, r1)
            }
        } else if cs.grid_column_span.is_some() || cs.grid_row_span.is_some() {
            // Auto-placement with a span (e.g. grid-column: span 3).
            let is_column_flow = matches!(
                style.grid_auto_flow,
                incognidium_style::GridAutoFlow::Column
            );
            let num_explicit_rows = style.grid_template_rows.len();
            // Clamp spans to the available tracks. A span larger than the grid can
            // never fit in the placement search and causes the safety fallback to
            // dump items thousands of rows/columns away, exploding the page height.
            let col_span = col_span.min(num_cols.max(1));
            let row_span = if is_column_flow && num_explicit_rows > 0 {
                row_span.min(num_explicit_rows.max(1))
            } else {
                row_span
            };
            let (c, r) = if is_column_flow {
                find_next_free_column(
                    &mut occupied,
                    col_span,
                    row_span,
                    num_cols,
                    num_explicit_rows,
                    &mut auto_row,
                    &mut auto_col,
                )
            } else {
                find_next_free_row(
                    &mut occupied,
                    col_span,
                    row_span,
                    num_cols,
                    &mut auto_row,
                    &mut auto_col,
                )
            };
            (c, c + col_span, r, r + row_span)
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

        // Guard malformed/negative/out-of-bounds grid spans so later sizing
        // arithmetic is safe.
        let col_start = col_start.min(num_cols.saturating_sub(1));
        let col_end = col_end.min(num_cols).max(col_start + 1);
        let row_end = row_end.max(row_start + 1);

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

    // Resolve content-based column tracks using the placed items, then determine
    // the final column widths.
    let expanded_template_columns = resolve_content_based_tracks(
        &expanded_template_columns,
        &placements,
        &mut layout_box.children,
        styles,
        image_sizes,
        content_width,
        col_gap,
        style.font_size,
        content_width,
        content_width,
    );
    let col_widths = if expanded_template_columns.is_empty() {
        if num_cols <= 1 {
            vec![content_width]
        } else {
            // Implicit columns in column-flow grids (no explicit
            // grid-template-columns) should all be sized by grid-auto-columns,
            // not have the first column swallow the entire container width.
            let mut widths = vec![0.0_f32; num_cols];
            let total_gap = col_gap * (num_cols.saturating_sub(1) as f32);
            let available_for_tracks = (content_width - total_gap).max(0.0);
            for c in 0..num_cols {
                let auto_size = style
                    .grid_auto_columns
                    .get(c)
                    .or_else(|| style.grid_auto_columns.last())
                    .map(|t| match t {
                        incognidium_style::GridTrackSize::Px(px) => *px,
                        incognidium_style::GridTrackSize::Percent(p) => content_width * p / 100.0,
                        incognidium_style::GridTrackSize::Calc(expr) => expr
                            .evaluate(style.font_size, content_width, content_width, content_width)
                            .max(0.0),
                        _ => 0.0,
                    })
                    .unwrap_or(0.0);
                widths[c] = auto_size;
            }
            let explicit_total: f32 = widths.iter().sum();
            // If grid-auto-columns did not give every column an explicit positive
            // size (e.g. `auto`), share the available space equally. Otherwise
            // respect the explicit sizes even if they overflow.
            if explicit_total <= 0.0 {
                let equal = available_for_tracks / num_cols as f32;
                for w in widths.iter_mut() {
                    *w = equal;
                }
            }
            widths
        }
    } else {
        resolve_track_sizes(
            &expanded_template_columns,
            content_width,
            col_gap,
            style.font_size,
            content_width,
            content_width,
        )
    };
    // Helper to get column width (explicit or implicit)
    let get_col_width = |c: usize| -> f32 {
        if c < col_widths.len() {
            col_widths[c]
        } else {
            auto_col_size
        }
    };

    // Get auto-row size for implicit rows
    let auto_row_size = style
        .grid_auto_rows
        .first()
        .map(|t| match t {
            incognidium_style::GridTrackSize::Px(px) => *px,
            incognidium_style::GridTrackSize::Percent(p) => content_width * p / 100.0,
            incognidium_style::GridTrackSize::Calc(expr) => expr
                .evaluate(style.font_size, content_width, content_width, content_width)
                .max(0.0),
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
                if is_collapsible_whitespace_only(text) {
                    continue;
                }
            }
        }
        let p = match placement_iter.next() {
            Some(p) => p,
            None => break, // Should not happen if counts match
        };
        // Cell width spans multiple columns. Guard reversed placements (col_end
        // can be less than col_start when authors supply invalid grid lines) so
        // the arithmetic never underflows.
        let col_span = if p.col_end > p.col_start {
            p.col_end - p.col_start
        } else {
            1
        };
        let cell_width: f32 = (p.col_start..p.col_start + col_span)
            .map(|c| get_col_width(c))
            .sum::<f32>()
            + (col_span.saturating_sub(1)) as f32 * col_gap;

        let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
        // Absolutely positioned grid items are removed from normal flow and must
        // not contribute to row track sizing; their final size is resolved later
        // against the assigned grid area.
        if child_style.position == Position::Absolute {
            continue;
        }

        // Resolve the item's inline-axis self-alignment. `justify-items: stretch`
        // is the grid default and means the item should fill the cell width. Any
        // other alignment (start/end/center) is a shrink-to-fit alignment: the item
        // is first measured at max-content, and if that is narrower than the cell
        // it is placed according to its alignment rather than stretched.
        let justify_item = if child_style.place_self.1 != JustifySelf::Auto {
            match child_style.place_self.1 {
                JustifySelf::FlexStart => JustifyItems::FlexStart,
                JustifySelf::FlexEnd => JustifyItems::FlexEnd,
                JustifySelf::Center => JustifyItems::Center,
                JustifySelf::Stretch => JustifyItems::Stretch,
                _ => style.place_items.1,
            }
        } else {
            style.place_items.1
        };
        // Grid items default to stretch. An explicit start/end/center alignment
        // means the item should shrink to its intrinsic width and be aligned.
        let is_stretch =
            justify_item == JustifyItems::Stretch || justify_item == JustifyItems::Auto;
        let child_content_width = (cell_width
            - child_style.margin_left
            - child_style.margin_right
            - child_style.padding_left_px(cell_width)
            - child_style.padding_right_px(cell_width)
            - child_style.border_left_width
            - child_style.border_right_width)
            .max(0.0);

        let width_is_auto = matches!(child_style.width, SizeValue::Auto | SizeValue::None);
        if !is_stretch && width_is_auto {
            // Measure at max-content so the item can shrink to its natural width.
            const MAX_CONTENT: f32 = 10_000.0;
            // A row flex container with an auto inline size fills whatever
            // width it is laid out at, so a probe pass cannot reveal its
            // intrinsic size: free-space distribution (justify-content) pushes
            // its items deep into the probe width and the rightmost-edge
            // measurement below reports the whole probe as content. Compute
            // the flex container's max-content main size from its items and
            // lay it out at that width instead, so its inner positions match
            // the size justify-self aligns against.
            let child_is_row_flex = matches!(
                child_style.flex_direction,
                FlexDirection::Row | FlexDirection::RowReverse
            );
            if matches!(child.box_type, BoxType::Flex | BoxType::InlineFlex) && child_is_row_flex {
                let pb_width = child_style.padding_left_px(cell_width)
                    + child_style.padding_right_px(cell_width)
                    + child_style.border_left_width
                    + child_style.border_right_width;
                let intrinsic_total = calculate_intrinsic_width(child, styles) + pb_width;
                let probe_content_width = (intrinsic_total - pb_width)
                    .min(child_content_width)
                    .max(0.0);
                child.forced_content_width = Some(probe_content_width);
                compute_layout(child, styles, cell_width, 0.0, image_sizes);
                let _ = child.forced_content_width.take();
            } else {
                child.forced_content_width = None;
                compute_layout(child, styles, MAX_CONTENT, 0.0, image_sizes);
            }

            // `compute_layout` with an enormous containing width makes an auto-width
            // block fill the containing width, so `child.width` is not the intrinsic
            // size. Derive the natural border-box width from the rightmost child edge.
            let pb_width = child_style.padding_left_px(cell_width)
                + child_style.padding_right_px(cell_width)
                + child_style.border_left_width
                + child_style.border_right_width;
            let natural_content_width = child
                .children
                .iter()
                .map(|c| c.x + c.width)
                .fold(0.0_f32, f32::max)
                .max(0.0);
            let natural_total_width = natural_content_width + pb_width;

            if natural_total_width > cell_width {
                // Natural width overflows the cell, so fall back to a forced cell-width
                // layout so the content wraps correctly and row height is accurate.
                child.forced_content_width = Some(child_content_width);
                compute_layout(child, styles, cell_width, 0.0, image_sizes);
                let _ = child.forced_content_width.take();
            } else {
                // Use the measured intrinsic size instead of the stretched cell width.
                child.width = natural_total_width;
                child.content_width = natural_content_width;
            }

            // Clamp the resolved width to the cell so the positioning pass can apply
            // justify-self offsets without the item spilling past the grid area.
            let max_width = cell_width - child_style.margin_left - child_style.margin_right;
            if child.width > max_width {
                child.width = max_width.max(0.0);
                child.content_width = (child.width - pb_width).max(0.0);
            }
        } else if !is_stretch && !width_is_auto {
            // The item has a definite width and a non-stretch justify-self: keep
            // its own width so the positioning pass can align it within the cell.
            // Forcing the cell content width here would clobber the item's declared
            // width and defeat justify-self: end/center.
            compute_layout(child, styles, cell_width, 0.0, image_sizes);
            // A percentage-resolved width is relative to the grid area, so the
            // item together with its inline margins must fit inside the area
            // (`width:100%; margin-inline:16px` is a common layout pattern).
            // Shrink such an item and re-layout it at the final width so its
            // own tracks and wrapping see the clamped size; fixed pixel widths
            // keep the standard overflow behavior.
            if matches!(child_style.width, SizeValue::Percent(_)) {
                let max_width =
                    (cell_width - child_style.margin_left - child_style.margin_right).max(0.0);
                if child.width > max_width {
                    let pb = child_style.padding_left_px(cell_width)
                        + child_style.padding_right_px(cell_width)
                        + child_style.border_left_width
                        + child_style.border_right_width;
                    child.forced_content_width = Some((max_width - pb).max(0.0));
                    compute_layout(child, styles, cell_width, 0.0, image_sizes);
                    let _ = child.forced_content_width.take();
                }
            }
        } else {
            child.forced_content_width = Some(child_content_width);
            compute_layout(child, styles, cell_width, 0.0, image_sizes);
            let _ = child.forced_content_width.take();
        }
        let child_h = child.height + child_style.margin_top + child_style.margin_bottom;
        // Distribute height across spanned rows (attribute to first row for simplicity)
        let row_span = if p.row_end > p.row_start {
            p.row_end - p.row_start
        } else {
            1
        };
        let per_row_h = child_h / row_span as f32;
        for row_height in row_heights
            .iter_mut()
            .take((p.row_start + row_span).min(num_rows))
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
            match &explicit_row_tracks[r] {
                GridTrackSize::Px(px) => *rh = *px,
                GridTrackSize::Percent(p) => *rh = content_width * *p / 100.0,
                GridTrackSize::Calc(expr) => {
                    *rh = expr
                        .evaluate(style.font_size, content_width, content_width, content_width)
                        .max(0.0);
                }
                GridTrackSize::Auto => {}
                GridTrackSize::Fr(fr) => {
                    // A 0fr track collapses to zero height, which is used by CSS-only
                    // accordions (grid-template-rows: 0fr -> 1fr). For auto-height
                    // grids, non-zero fr tracks behave like content-sized tracks,
                    // so leave the measured content height in place.
                    if *fr == 0.0 {
                        *rh = 0.0;
                    }
                }
                GridTrackSize::MinMax(min, _) => {
                    if let Some(min_px) = track_breadth_to_px(
                        min.as_ref(),
                        content_width,
                        style.font_size,
                        content_width,
                        content_width,
                    ) {
                        if *rh < min_px {
                            *rh = min_px;
                        }
                    }
                }
                GridTrackSize::Repeat(..) => {}
                GridTrackSize::MinContent
                | GridTrackSize::MaxContent
                | GridTrackSize::FitContent => {
                    // Content-based row tracks are not yet measured; leave the
                    // content-derived height in place.
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
                if is_collapsible_whitespace_only(text) {
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

        // Absolutely positioned grid items use their assigned grid area as the
        // containing block. Re-layout them against the final cell dimensions so
        // `inset:0` and percentage sizes resolve to the grid area rather than the
        // indefinite height used during track sizing.
        if child_style.position == Position::Absolute {
            compute_layout(child, styles, cell_width, cell_height, image_sizes);
            // layout_absolute positions the box relative to its containing block;
            // add the grid-area origin within the container to get coordinates
            // relative to the grid container's border box.
            child.x += content_x + cell_x;
            child.y += content_y + cell_y;
            continue;
        }

        // Calculate item position within cell based on place-items (align-items,
        // justify-items), overridden by the item's own place-self when it is not
        // auto.
        let align = if child_style.place_self.0 != AlignSelf::Auto {
            match child_style.place_self.0 {
                AlignSelf::FlexStart => AlignItems::FlexStart,
                AlignSelf::FlexEnd => AlignItems::FlexEnd,
                AlignSelf::Center => AlignItems::Center,
                AlignSelf::Stretch => AlignItems::Stretch,
                AlignSelf::Baseline => AlignItems::Baseline,
                _ => style.place_items.0,
            }
        } else {
            style.place_items.0
        };
        let justify = if child_style.place_self.1 != JustifySelf::Auto {
            match child_style.place_self.1 {
                JustifySelf::FlexStart => JustifyItems::FlexStart,
                JustifySelf::FlexEnd => JustifyItems::FlexEnd,
                JustifySelf::Center => JustifyItems::Center,
                JustifySelf::Stretch => JustifyItems::Stretch,
                _ => style.place_items.1,
            }
        } else {
            style.place_items.1
        };

        // Apply justify-items (horizontal alignment within cell). Alignment
        // positions the item's margin box inside the cell, so the item width
        // here includes the inline margins.
        let item_width = child.width + child_style.margin_left + child_style.margin_right;
        let x_offset = match justify {
            JustifyItems::Center => ((cell_width - item_width) / 2.0).max(0.0),
            JustifyItems::FlexEnd => (cell_width - item_width).max(0.0),
            JustifyItems::Stretch => {
                // Stretch to fill cell width
                let new_width = cell_width - child_style.margin_left - child_style.margin_right;
                if new_width > child.width {
                    child.width = new_width;
                    child.content_width = child.width
                        - child_style.padding_left_px(cell_width)
                        - child_style.padding_right_px(cell_width)
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
                let new_height = if let Some(mh) =
                    evaluate_size_value(&child_style.min_height, cell_height, child_style.font_size)
                {
                    new_height.max(mh)
                } else {
                    new_height
                };
                if let Some(mh) =
                    evaluate_size_value(&child_style.max_height, cell_height, child_style.font_size)
                {
                    child.height = new_height.min(mh);
                } else {
                    child.height = new_height;
                }
                0.0
            }
            _ => 0.0, // FlexStart/Baseline default to start
        };

        child.x = content_x + cell_x + child_style.margin_left + x_offset;
        child.y = content_y + cell_y + child_style.margin_top + y_offset;

        // When a grid item is stretched to a definite cell height, re-layout it
        // with that height so percentage-height children resolve correctly.
        if align == AlignItems::Stretch {
            let pb_height = child_style.padding_top_px(cell_width)
                + child_style.padding_bottom_px(cell_width)
                + child_style.border_top_width
                + child_style.border_bottom_width;
            let new_content_height = (child.height - pb_height).max(0.0);
            if (new_content_height - child.content_height).abs() > 0.5 {
                child.forced_content_width = Some(child.content_width);
                child.forced_content_height = Some(new_content_height);
                compute_layout(child, styles, cell_width, new_content_height, image_sizes);
            }
        }

        // Ensure width fills cell for stretch
        if justify == JustifyItems::Stretch && child.width < cell_width {
            child.width = cell_width - child_style.margin_left - child_style.margin_right;
            child.content_width = child.width
                - child_style.padding_left_px(cell_width)
                - child_style.padding_right_px(cell_width)
                - child_style.border_left_width
                - child_style.border_right_width;
        }
    }

    // Apply relative positioning offsets to grid items. Mirrors the treatment in
    // block and flex layout: the shift is applied after flow placement without
    // changing the grid container's size.
    let rel_container_w = content_width;
    let rel_container_h = row_heights.iter().sum::<f32>();
    for child in &mut layout_box.children {
        if child.box_type == BoxType::Text {
            if let Some(ref text) = child.text {
                if is_collapsible_whitespace_only(text) {
                    continue;
                }
            }
        }
        let cs = styles.get(&child.node_id).cloned().unwrap_or_default();
        // Absolutely positioned items had their offsets resolved against the
        // grid area already; only normal-flow relative items shift here.
        if cs.position != Position::Relative {
            continue;
        }
        let offset_x = if let Some(v) =
            resolve_offset(&cs.left, rel_container_w, rel_container_w, cs.font_size)
        {
            v
        } else if let Some(v) =
            resolve_offset(&cs.right, rel_container_w, rel_container_w, cs.font_size)
        {
            -v
        } else {
            0.0
        };
        let offset_y = if let Some(v) =
            resolve_offset(&cs.top, rel_container_h, rel_container_h, cs.font_size)
        {
            v
        } else if let Some(v) =
            resolve_offset(&cs.bottom, rel_container_h, rel_container_h, cs.font_size)
        {
            -v
        } else {
            0.0
        };
        // Clamp extreme relative offsets that would push the box entirely
        // off-canvas, mirroring the guard in layout_block.
        let clamped_offset_x = if offset_x < -child.width && child.width > 0.0 {
            0.0
        } else {
            offset_x
        };
        let clamped_offset_y = if offset_y < -child.height && child.height > 0.0 {
            0.0
        } else {
            offset_y
        };
        child.x += clamped_offset_x;
        child.y += clamped_offset_y;
    }

    // Compute total height
    let total_row_height: f32 = row_heights.iter().sum();
    let total_gap_height = row_gap * (num_rows.saturating_sub(1)) as f32;
    let auto_content_height = total_row_height + total_gap_height;

    // When height is auto (or a percentage that cannot be resolved against an
    // indefinite containing height), honor an explicit aspect-ratio. This is
    // essential for modern players/cards such as a video container that uses
    // `display:grid; width:100%; height:100%` and sets
    // `aspect-ratio: var(--video-aspect-ratio)` inline. Without this, the grid
    // collapses to the intrinsic height of its children (or to zero) and the
    // player poster explodes to intrinsic size.
    let height_is_auto = matches!(style.height, SizeValue::Auto | SizeValue::None)
        || (matches!(style.height, SizeValue::Percent(_)) && containing_height <= 0.0);
    let content_height = if height_is_auto {
        if let Some(ref ar) = style.aspect_ratio {
            let ratio = ar.width / ar.height.max(0.001);
            if ratio > 0.0 && layout_box.content_width > 0.0 {
                (layout_box.content_width / ratio).max(auto_content_height)
            } else {
                auto_content_height
            }
        } else {
            auto_content_height
        }
    } else {
        auto_content_height
    };

    // Apply explicit pixel height if set (overrides aspect-ratio).
    // Percentage heights against an indefinite containing block are treated as
    // auto for intrinsic sizing; don't let evaluate_size_value resolve them
    // to zero and collapse the grid.
    let content_height = if matches!(style.height, SizeValue::Percent(_))
        && containing_height <= 0.0
    {
        content_height
    } else if let Some(h) = evaluate_size_value(&style.height, containing_height, style.font_size) {
        h
    } else {
        content_height
    };

    let content_height = if let Some(mh) =
        evaluate_size_value(&style.min_height, containing_height, style.font_size)
    {
        content_height.max(mh)
    } else {
        content_height
    };

    // SAFETY CAP: Prevent extreme grid container heights
    let content_height = content_height.min(100_000.0);

    layout_box.content_width = content_width;
    layout_box.width =
        layout_box.content_width + padding_left + padding_right + border_left + border_right;
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

    let padding_left = style.padding_left_px(containing_width);
    let padding_right = style.padding_right_px(containing_width);
    let padding_top = style.padding_top_px(containing_width);
    let padding_bottom = style.padding_bottom_px(containing_width);
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
    let mut max_column_height: f32 = 0.0;
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
            max_column_height = max_column_height.max(current_column_height);
            current_column += 1;
            current_column_height = 0.0;
            column_start_y = content_y;
        }

        // Position in column
        child.x = content_x + current_column as f32 * (column_width + column_gap);
        child.y = column_start_y + current_column_height + margin_top;

        current_column_height += child_total_height;
    }
    max_column_height = max_column_height.max(current_column_height);

    // Calculate final container height from the actual tallest column so
    // backgrounds/borders enclose all distributed content. The balanced
    // column_height is only a distribution target, not a clip rect.
    let final_height = if style.height == SizeValue::Auto {
        max_column_height
    } else {
        match style.height {
            SizeValue::Px(h) => h,
            _ => max_column_height,
        }
    };

    layout_box.content_width = content_width;
    layout_box.width =
        layout_box.content_width + padding_left + padding_right + border_left + border_right;
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

/// Resolve a single grid track breadth to pixels when possible.
fn track_breadth_to_px(
    track: &GridTrackSize,
    available: f32,
    font_size: f32,
    viewport_width: f32,
    viewport_height: f32,
) -> Option<f32> {
    match track {
        GridTrackSize::Px(px) => Some(*px),
        GridTrackSize::Percent(p) => Some(available * *p / 100.0),
        GridTrackSize::Calc(expr) => Some(
            expr.evaluate(font_size, viewport_width, viewport_height, available)
                .max(0.0),
        ),
        GridTrackSize::MinMax(min, _) => track_breadth_to_px(
            min.as_ref(),
            available,
            font_size,
            viewport_width,
            viewport_height,
        ),
        GridTrackSize::Fr(_)
        | GridTrackSize::Auto
        | GridTrackSize::Repeat(..)
        | GridTrackSize::MinContent
        | GridTrackSize::MaxContent
        | GridTrackSize::FitContent => None,
    }
}

/// Approximate the fixed/minimum width of a single track for the purpose of
/// deciding how many `auto-fill`/`auto-fit` repetitions fit in a container.
fn approximate_track_width(
    track: &GridTrackSize,
    available: f32,
    font_size: f32,
    viewport_width: f32,
    viewport_height: f32,
) -> f32 {
    track_breadth_to_px(track, available, font_size, viewport_width, viewport_height).unwrap_or(0.0)
}

/// Expand any `GridTrackSize::Repeat` entries into concrete tracks. For fixed
/// counts this is a straight repetition. For `auto-fill` / `auto-fit` the count
/// is derived from the actual available space, so we no longer hard-code a
/// 1024px viewport at parse time.
fn expand_repeats(
    tracks: &[GridTrackSize],
    available: f32,
    gap: f32,
    font_size: f32,
    viewport_width: f32,
    viewport_height: f32,
) -> Vec<GridTrackSize> {
    let mut out = Vec::new();
    for track in tracks {
        match track {
            GridTrackSize::Repeat(count, repeated) => {
                let count = match count {
                    RepeatCount::Number(n) => *n,
                    RepeatCount::AutoFill | RepeatCount::AutoFit => {
                        let group_width: f32 = repeated
                            .iter()
                            .map(|t| {
                                approximate_track_width(
                                    t,
                                    available,
                                    font_size,
                                    viewport_width,
                                    viewport_height,
                                )
                            })
                            .sum();
                        let inner_gap = gap * repeated.len().saturating_sub(1) as f32;
                        let group_total = group_width + inner_gap;
                        if group_total <= 0.0 {
                            1usize
                        } else {
                            ((available + gap) / (group_total + gap)).floor().max(0.0) as usize
                        }
                    }
                };
                for _ in 0..count.max(1) {
                    out.extend(repeated.iter().cloned());
                }
            }
            _ => out.push(track.clone()),
        }
    }
    out
}

/// Placement of a single grid item in 0-indexed cell coordinates.
#[derive(Debug)]
struct CellPlacement {
    col_start: usize, // 0-indexed column
    col_end: usize,   // exclusive
    row_start: usize, // 0-indexed row
    row_end: usize,   // exclusive
}

/// Returns true when a track size depends on the contents of its items and
/// therefore cannot be resolved without first measuring those items.
fn is_content_based_track(track: &GridTrackSize) -> bool {
    matches!(
        track,
        GridTrackSize::MinContent | GridTrackSize::MaxContent | GridTrackSize::FitContent
    )
}

/// Replace `min-content`, `max-content`, and bare `fit-content` track sizes with
/// concrete `Px(...)` values by measuring the grid items that occupy them.
///
/// This is a first-pass, single-span approximation: an item's intrinsic width
/// is divided equally across the columns it spans and the per-track maximum is
/// used for both `min-content` and `max-content` tracks. It is enough to stop
/// navigation headers from collapsing content-sized tracks to 0 px.
fn resolve_content_based_tracks(
    tracks: &[GridTrackSize],
    placements: &[CellPlacement],
    children: &mut [LayoutBox],
    styles: &StyleMap,
    image_sizes: &ImageSizes,
    available: f32,
    gap: f32,
    font_size: f32,
    viewport_width: f32,
    viewport_height: f32,
) -> Vec<GridTrackSize> {
    let needs_content = tracks.iter().any(is_content_based_track)
        || tracks
            .iter()
            .any(|t| matches!(t, GridTrackSize::Auto | GridTrackSize::FitContent))
        || tracks.iter().any(|t| {
            if let GridTrackSize::MinMax(min, max) = t {
                is_content_based_track(min) || is_content_based_track(max)
            } else {
                false
            }
        });
    if !needs_content {
        return tracks.to_vec();
    }

    let mut max_content = vec![0.0_f32; tracks.len()];
    let mut min_content = vec![0.0_f32; tracks.len()];
    let mut placement_iter = placements.iter();

    for child in children.iter_mut() {
        // Match the whitespace-skipping logic used to build `placements`.
        if child.box_type == BoxType::Text {
            if let Some(ref text) = child.text {
                if is_collapsible_whitespace_only(text) {
                    continue;
                }
            }
        }
        let p = match placement_iter.next() {
            Some(p) => p,
            None => break,
        };

        // Lay the item out under a zero-width measuring pass so percentage
        // widths resolve like auto and we read the item's intrinsic/max-content
        // border-box width. The final layout pass later will redo this at the
        // resolved cell width, so any side effects here are harmless.
        let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
        // Absolutely positioned grid items are removed from normal flow and do
        // not contribute to content-based track sizing.
        if child_style.position == Position::Absolute {
            continue;
        }
        compute_layout(child, styles, 0.0, 0.0, image_sizes);
        let contribution = child.width + child_style.margin_left + child_style.margin_right;
        let col_span = (p.col_end - p.col_start).max(1);
        let per_track = contribution / col_span as f32;
        for c in p.col_start..p.col_end.min(tracks.len()) {
            max_content[c] = max_content[c].max(per_track);
            min_content[c] = min_content[c].max(per_track);
        }
    }

    // Replace content-based keywords with measured pixel values.
    let resolved: Vec<GridTrackSize> = tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let max_c = max_content.get(i).copied().unwrap_or(0.0);
            let min_c = min_content.get(i).copied().unwrap_or(0.0);
            match t {
                GridTrackSize::MinContent => GridTrackSize::Px(min_c),
                GridTrackSize::MaxContent => GridTrackSize::Px(max_c),
                GridTrackSize::Auto => GridTrackSize::Px(max_c),
                GridTrackSize::FitContent => GridTrackSize::Auto,
                GridTrackSize::MinMax(min, max) => {
                    let min2 = if is_content_based_track(min) {
                        GridTrackSize::Px(min_c)
                    } else {
                        *min.clone()
                    };
                    let max2 = if is_content_based_track(max) {
                        GridTrackSize::Px(max_c)
                    } else {
                        *max.clone()
                    };
                    GridTrackSize::MinMax(Box::new(min2), Box::new(max2))
                }
                _ => t.clone(),
            }
        })
        .collect();

    // Run the normal track-resolution algorithm on the concrete tracks.
    let mut widths = resolve_track_sizes(
        &resolved,
        available,
        gap,
        font_size,
        viewport_width,
        viewport_height,
    );

    // Content-sized tracks can ask for more than the container has. In a real
    // browser, authors usually prevent that with wrapping, overflow, or JS
    // (e.g. a "More" overflow menu). We gracefully clamp the used widths to the
    // available grid space while honoring `minmax()` minimums.
    let n = tracks.len();
    let total_gap = gap * (n.saturating_sub(1) as f32);
    let space = (available - total_gap).max(0.0);
    let total_width: f32 = widths.iter().sum();
    if total_width > space {
        let mut lower = vec![0.0_f32; n];
        for (i, t) in tracks.iter().enumerate() {
            match t {
                // `min-content` is itself a hard lower bound.
                GridTrackSize::MinContent => {
                    lower[i] = min_content.get(i).copied().unwrap_or(0.0);
                }
                // `minmax(min-content, ...)` carries that lower bound too.
                GridTrackSize::MinMax(min, _) if is_content_based_track(min) => {
                    lower[i] = min_content.get(i).copied().unwrap_or(0.0);
                }
                GridTrackSize::MinMax(min, _) => {
                    lower[i] = track_breadth_to_px(
                        min.as_ref(),
                        space,
                        font_size,
                        viewport_width,
                        viewport_height,
                    )
                    .unwrap_or(0.0);
                }
                _ => {}
            }
        }
        let lower_sum: f32 = lower.iter().sum();
        if lower_sum < space {
            let remaining = space - lower_sum;
            let above_sum: f32 = widths
                .iter()
                .zip(lower.iter())
                .map(|(w, l)| (w - l).max(0.0))
                .sum();
            if above_sum > 0.0 {
                let scale = remaining / above_sum;
                for i in 0..n {
                    widths[i] = lower[i] + (widths[i] - lower[i]).max(0.0) * scale;
                }
            }
        } else {
            // The minimums alone do not fit; scale the minimums proportionally.
            let scale = space / lower_sum.max(1.0);
            for i in 0..n {
                widths[i] = lower[i] * scale;
            }
        }
    }

    // Stretch auto tracks to fill a definite-size container. A track whose
    // growth limit is `auto` (a bare `auto` column, or `minmax(x, auto)`)
    // shares the leftover space equally with the others instead of staying at
    // its content width. Flexible tracks take the free space first, so this
    // only applies when no `fr` track is present.
    let has_flex = tracks.iter().any(|t| match t {
        GridTrackSize::Fr(_) => true,
        GridTrackSize::MinMax(_, max) => matches!(max.as_ref(), GridTrackSize::Fr(_)),
        _ => false,
    });
    let auto_stretch: Vec<usize> = tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| match t {
            GridTrackSize::Auto => true,
            GridTrackSize::MinMax(_, max) => matches!(max.as_ref(), GridTrackSize::Auto),
            _ => false,
        })
        .map(|(i, _)| i)
        .collect();
    if !has_flex && !auto_stretch.is_empty() {
        let total_width: f32 = widths.iter().sum();
        let free = (space - total_width).max(0.0);
        if free > 0.0 {
            let share = free / auto_stretch.len() as f32;
            for &i in &auto_stretch {
                widths[i] += share;
            }
        }
    }

    widths.into_iter().map(GridTrackSize::Px).collect()
}

/// Resolve grid track sizes to actual pixel widths given the available space.
fn resolve_track_sizes(
    tracks: &[GridTrackSize],
    available: f32,
    gap: f32,
    font_size: f32,
    viewport_width: f32,
    viewport_height: f32,
) -> Vec<f32> {
    let n = tracks.len();
    if n == 0 {
        return vec![available];
    }

    let total_gap = gap * (n.saturating_sub(1)) as f32;
    let space = (available - total_gap).max(0.0);

    // First pass: classify each track into a base size, a growth limit, and a
    // flex factor (0 means inflexible). Auto tracks are sized to their content;
    // when no item occupies them we approximate that as 0 and let `fr` tracks
    // consume the remaining space. Previously auto tracks were treated as 1fr,
    // which squeezed real `1fr` content columns on multi-column desktop grids.
    let mut widths = vec![0.0_f32; n];
    let mut flex = vec![0.0_f32; n]; // flex factor per track (0 = inflexible)
    let mut limits = vec![0.0_f32; n]; // growth limit; 0 means not growable
    let mut auto_indices = Vec::new();

    for (i, track) in tracks.iter().enumerate() {
        match track {
            GridTrackSize::Px(px) => widths[i] = *px,
            GridTrackSize::Percent(p) => widths[i] = space * *p / 100.0,
            GridTrackSize::Calc(expr) => {
                widths[i] = expr
                    .evaluate(font_size, viewport_width, viewport_height, available)
                    .max(0.0)
            }
            GridTrackSize::Fr(fr) => flex[i] = *fr,
            GridTrackSize::Auto | GridTrackSize::FitContent => {
                auto_indices.push(i);
            }
            GridTrackSize::MinContent | GridTrackSize::MaxContent => {
                // These should have been replaced with concrete Px(...) values
                // before resolve_track_sizes() is called; treat any stragglers as
                // auto tracks so they do not collapse to zero.
                auto_indices.push(i);
            }
            GridTrackSize::MinMax(min, max) => {
                let min_px = track_breadth_to_px(
                    min.as_ref(),
                    space,
                    font_size,
                    viewport_width,
                    viewport_height,
                )
                .unwrap_or(0.0);
                match max.as_ref() {
                    GridTrackSize::Fr(fr) => {
                        flex[i] = *fr;
                        widths[i] = min_px;
                    }
                    _ => {
                        let max_px = track_breadth_to_px(
                            max.as_ref(),
                            space,
                            font_size,
                            viewport_width,
                            viewport_height,
                        )
                        .unwrap_or(min_px);
                        widths[i] = min_px;
                        limits[i] = max_px.max(min_px);
                    }
                }
            }
            GridTrackSize::Repeat(..) => {
                // Repeats should be flattened by expand_repeats() before this
                // function is called; treat any survivors as auto tracks.
                auto_indices.push(i);
            }
        }
    }

    // Second pass: grow inflexible tracks from their base toward their growth
    // limit with the free space, sharing it equally and freezing tracks that
    // reach their limit. This happens before `fr` expansion, so a capped track
    // like `minmax(0, 48rem)` shrinks to what is left over instead of claiming
    // its full maximum and pushing the track list past the container.
    let mut growable: Vec<usize> = (0..n)
        .filter(|&i| flex[i] == 0.0 && limits[i] > widths[i] + 0.0001)
        .collect();
    let mut free = (space - widths.iter().sum::<f32>()).max(0.0);
    while free > 0.0001 && !growable.is_empty() {
        let share = free / growable.len() as f32;
        let min_headroom = growable
            .iter()
            .map(|&i| limits[i] - widths[i])
            .fold(f32::INFINITY, f32::min);
        if min_headroom <= 0.0001 {
            break;
        }
        if share <= min_headroom {
            for &i in &growable {
                widths[i] += share;
            }
            free = 0.0;
        } else {
            for &i in &growable {
                widths[i] += min_headroom;
            }
            free -= min_headroom * growable.len() as f32;
            growable.retain(|&i| limits[i] > widths[i] + 0.0001);
        }
    }

    // Third pass: expand flexible tracks into the space left after the
    // inflexible tracks have been sized. A flexible track whose base size
    // exceeds the hypothetical fr size keeps its base and stops being
    // flexible, matching how browsers resolve `minmax(min, Nfr)` tracks.
    let mut flexible: Vec<usize> = (0..n).filter(|&i| flex[i] > 0.0).collect();
    loop {
        if flexible.is_empty() {
            break;
        }
        let nonflex_used: f32 = (0..n).filter(|&i| flex[i] == 0.0).map(|i| widths[i]).sum();
        let leftover = (space - nonflex_used).max(0.0);
        let flex_sum: f32 = flexible.iter().map(|&i| flex[i]).sum();
        let hyp_fr = leftover / flex_sum.max(1.0);
        let violated: Vec<usize> = flexible
            .iter()
            .cloned()
            .filter(|&i| widths[i] > hyp_fr + 0.0001)
            .collect();
        if violated.is_empty() {
            for &i in &flexible {
                widths[i] = widths[i].max(hyp_fr * flex[i]);
            }
            break;
        }
        for &i in &violated {
            flex[i] = 0.0;
        }
        flexible.retain(|&i| flex[i] > 0.0);
    }

    if flexible.is_empty() && !auto_indices.is_empty() {
        // No flexible tracks: share the remaining space among auto tracks.
        let used: f32 = widths.iter().sum::<f32>();
        let fr_space = (space - used).max(0.0);
        let share = fr_space / auto_indices.len() as f32;
        for i in auto_indices {
            widths[i] = share;
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

/// Apply the CSS `text-transform` value to a string so that layout measures the
/// same glyphs that paint will later draw. The transformations are idempotent,
/// so applying them again at paint time is safe.
fn apply_text_transform(text: &str, transform: &incognidium_style::TextTransform) -> String {
    use incognidium_style::TextTransform;
    match transform {
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
        TextTransform::None | TextTransform::MathAuto => text.to_string(),
        TextTransform::FullWidth => text
            .chars()
            .map(|c| {
                if (0x21..=0x7E).contains(&(c as u32)) {
                    char::from_u32(c as u32 + 0xFEE0).unwrap_or(c)
                } else if c == ' ' {
                    '\u{3000}'
                } else {
                    c
                }
            })
            .collect(),
        TextTransform::FullSizeKana => text
            .chars()
            .map(|c| match c {
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
            })
            .collect(),
    }
}

fn layout_text(layout_box: &mut LayoutBox, styles: &StyleMap, containing_width: f32) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();
    // `text` below is rewritten with the line-broken result, and measuring
    // passes can run at narrow widths that split long words into fragments.
    // Always re-break from the pristine source so a later pass at the final
    // width never inherits fragments from an earlier, narrower one.
    let source = layout_box
        .source_text
        .clone()
        .or_else(|| layout_box.text.clone());
    layout_box.source_text = source.clone();
    let text = source.unwrap_or_default();

    // Expand tabs to spaces based on tab-size property
    let text = expand_tabs(&text, style.tab_size);

    // Process soft hyphens based on hyphens property
    let text = process_soft_hyphens(&text, &style.hyphens);

    // Apply text-transform so width measurements match the rendered glyphs.
    // Paint applies the same transform again; the common values are idempotent.
    let text = apply_text_transform(&text, &style.text_transform);

    // Process soft hyphens based on hyphens property
    let text = process_soft_hyphens(&text, &style.hyphens);

    if text.is_empty() {
        layout_box.width = 0.0;
        layout_box.height = 0.0;
        return;
    }

    let line_height = style.font_size * style.line_height;
    let space_width = measure_text_width(" ", style.font_size, &style) + style.word_spacing;

    // Whether this context preserves source whitespace instead of collapsing
    // it (`white-space: pre`/`pre-wrap`, or the CSS Text Level 4 equivalents).
    // Whitespace-only runs are real content there: a run of spaces has width
    // and must not be flattened away.
    let whitespace_preserved = matches!(
        style.white_space,
        incognidium_style::WhiteSpace::Pre | incognidium_style::WhiteSpace::PreWrap
    ) || matches!(
        style.white_space_collapse,
        WhiteSpaceCollapse::Preserve | WhiteSpaceCollapse::BreakSpaces
    );

    // A whitespace-only run never has any visible width in a collapsing
    // context, regardless of white-space or text-wrap settings. Treat it the
    // same as a single space so it does not participate in flex sizing and
    // cannot push real content out of its container.
    if is_collapsible_whitespace_only(&text) && !whitespace_preserved {
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
    let white_space_nowrap = matches!(
        style.white_space,
        incognidium_style::WhiteSpace::NoWrap | incognidium_style::WhiteSpace::Pre
    ) || text_wrap_nowrap;
    // When the container has no resolved width (e.g. shrink-to-fit), do not wrap
    // lines so the natural width can be measured. Also avoid breaking words into
    // individual characters when the available width is unreasonably small (a
    // measuring pass can produce a 1px width due to negative margins); otherwise
    // the broken text is cached and later real layouts render it vertically.
    let nowrap = white_space_nowrap || containing_width <= 1.0;

    // Determine white-space collapsing behavior from white-space-collapse property
    // This is the CSS Text Level 4 way to control whitespace handling
    let collapse_spaces = matches!(style.white_space_collapse, WhiteSpaceCollapse::Collapse);
    let preserve_spaces = matches!(style.white_space_collapse, WhiteSpaceCollapse::Preserve);
    let preserve_breaks_only = matches!(
        style.white_space_collapse,
        WhiteSpaceCollapse::PreserveBreaks
    );
    let break_spaces = matches!(style.white_space_collapse, WhiteSpaceCollapse::BreakSpaces);

    // Check if newlines should be preserved (legacy white-space property)
    let preserve_newlines_legacy = matches!(
        style.white_space,
        incognidium_style::WhiteSpace::Pre
            | incognidium_style::WhiteSpace::PreWrap
            | incognidium_style::WhiteSpace::PreLine
    );

    // Combine legacy and new property behavior
    let preserve_newlines =
        preserve_newlines_legacy || preserve_spaces || preserve_breaks_only || break_spaces;

    // Check if this is pre-wrap (preserves newlines AND wraps words)
    let is_pre_wrap_legacy = matches!(style.white_space, incognidium_style::WhiteSpace::PreWrap);
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
    let words: Vec<String> = if white_space_nowrap {
        if preserve_newlines {
            // Split on newlines but keep each line as a word
            text.split('\n').map(|s| s.to_string()).collect()
        } else {
            // nowrap: collapse runs of whitespace to a single space so that
            // trailing/leading formatting whitespace does not inflate the width
            // of the word. The text still cannot wrap because nowrap is in effect.
            let normalized = split_css_words(&text).join(" ");
            if normalized.is_empty() {
                vec![]
            } else {
                vec![normalized]
            }
        }
    } else if preserve_spaces {
        // white-space-collapse: preserve - split on newlines only
        text.split('\n').map(|s| s.to_string()).collect()
    } else if preserve_breaks_only {
        // white-space-collapse: preserve-breaks - collapse spaces, keep newlines
        // First normalize spaces on each line, then collect non-empty lines
        text.split('\n')
            .filter_map(|line| {
                let normalized = split_css_words(line).join(" ");
                if normalized.is_empty() {
                    None
                } else {
                    Some(normalized)
                }
            })
            .collect()
    } else {
        // Default: collapse all collapsible whitespace (NBSP is content and
        // stays glued inside its word).
        split_css_words(&text)
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
                line_info.push((
                    current_line_space_indices.clone(),
                    current_line_word_count,
                    current_line_width,
                ));
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
        line_info.push((
            current_line_space_indices,
            current_line_word_count,
            current_line_width,
        ));

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
                    if space_idx < broken_text_parts.len() && broken_text_parts[space_idx] == " " {
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
    let clamped_lines = style
        .line_clamp
        .map(|max| (lines as i32).min(max))
        .unwrap_or(lines as i32);
    layout_box.content_height = clamped_lines as f32 * line_height;
    // Constrain to containing width when the container has a real width.
    // A containing width of 0 means shrink-to-fit (inline-block / flex auto),
    // so keep the natural width; otherwise text-overflow: ellipsis and nowrap
    // depend on the constraint.
    // When nowrap is in effect (white-space: nowrap or an unreasonably small
    // container), do not clamp the width. Inline-level shrink-to-fit containers
    // size to their content and may overflow; clamping here made nowrap text
    // inside inline-flex / inline-block collapse to the narrow available space.
    layout_box.width = if containing_width > 0.0 && !white_space_nowrap && containing_width > 1.0 {
        natural_width.min(containing_width)
    } else {
        natural_width
    };
    layout_box.height = clamped_lines as f32 * line_height;
}

/// Split a text box that wraps around a float so the lines above the float keep
/// the reduced line-box width and the remaining lines can expand to the full
/// container width once the float ends. This mirrors CSS line-box
/// shortening/expansion around floats.
/// Whether the pristine source separates the last word of a split's first
/// fragment from the first word of its remainder with collapsible white
/// space. Inserting a line break at a wrap consumes that white space, so a
/// split must record it on the fragments: if the fragments ever end up placed
/// adjacent on one line (the line-width model that decided the split can
/// disagree with the placement pass), the inter-word gap is still rendered
/// instead of joining the two words together.
fn split_consumed_source_whitespace(source: &str, first_text: &str, rest_text: &str) -> bool {
    let Some(last_word) = first_text.split_whitespace().next_back() else {
        return false;
    };
    let Some(next_word) = rest_text.split_whitespace().next() else {
        return false;
    };
    let mut from = 0usize;
    while let Some(rel) = source[from..].find(last_word) {
        let mut idx = from + rel + last_word.len();
        let mut saw_space = false;
        while let Some(c) = source[idx..].chars().next() {
            if is_css_whitespace(c) {
                saw_space = true;
                idx += c.len_utf8();
            } else {
                break;
            }
        }
        if saw_space && source[idx..].starts_with(next_word) {
            return true;
        }
        from += rel + last_word.len();
    }
    false
}

fn split_text_at_float_boundary(
    text_box: &LayoutBox,
    styles: &StyleMap,
    beside_width: f32,
    full_width: f32,
    start_y: f32,
    float_bottom: f32,
) -> Vec<LayoutBox> {
    if text_box.box_type != BoxType::Text {
        return vec![text_box.clone()];
    }
    let style = styles.get(&text_box.node_id).cloned().unwrap_or_default();
    let line_height = style.font_size * style.line_height;
    if line_height <= 0.0 || float_bottom <= start_y {
        let mut full = text_box.clone();
        layout_text(&mut full, styles, full_width);
        return vec![full];
    }
    let total_lines = (text_box.height / line_height).round().max(1.0) as usize;
    let fit_height = (float_bottom - start_y).max(0.0);
    let lines_before = (fit_height / line_height).floor() as usize;
    if lines_before == 0 || lines_before >= total_lines {
        let width = if lines_before == 0 {
            full_width
        } else {
            beside_width
        };
        let mut b = text_box.clone();
        layout_text(&mut b, styles, width);
        return vec![b];
    }
    let text = text_box.text.clone().unwrap_or_default();
    let all_lines: Vec<&str> = text.split('\n').collect();
    if all_lines.len() <= lines_before {
        let mut b = text_box.clone();
        layout_text(&mut b, styles, beside_width);
        return vec![b];
    }
    let (first_lines, rest_lines) = all_lines.split_at(lines_before);
    let first_text = first_lines.join("\n");
    let rest_text = rest_lines.join("\n");
    let source = text_box
        .source_text
        .as_deref()
        .unwrap_or_else(|| text_box.text.as_deref().unwrap_or_default());
    let boundary_space = split_consumed_source_whitespace(source, &first_text, &rest_text);
    let mut first = text_box.clone();
    first.text = Some(first_text.clone());
    // The fragment's text IS its source: without this, layout_text would fall
    // back to the cloned full-text source and each fragment would re-render
    // the whole text.
    first.source_text = Some(first_text.clone());
    first.text_leading_space = text_box.text_leading_space;
    first.text_trailing_space = first_text.ends_with(char::is_whitespace) || boundary_space;
    layout_text(&mut first, styles, beside_width);
    let mut rest = text_box.clone();
    rest.text = Some(rest_text.clone());
    rest.source_text = Some(rest_text.clone());
    rest.text_leading_space = rest_text.starts_with(char::is_whitespace) || boundary_space;
    rest.text_trailing_space = text_box.text_trailing_space;
    layout_text(&mut rest, styles, full_width);
    rest.force_below_float = true;
    rest.force_line_break_before = true;
    vec![first, rest]
}

/// Split a wrapping text box so its first line fills a shortened first-line
/// width (e.g. the space remaining beside an inline-block badge), while the
/// rest of the text is laid out using the full line width. This mirrors the
/// per-line width behavior that real inline layout requires.
fn split_text_at_first_line_width(
    text_box: &LayoutBox,
    styles: &StyleMap,
    first_line_width: f32,
    full_width: f32,
) -> Vec<LayoutBox> {
    if text_box.box_type != BoxType::Text {
        return vec![text_box.clone()];
    }
    if first_line_width <= 1.0 || first_line_width + 0.5 >= full_width {
        return vec![text_box.clone()];
    }
    let style = styles.get(&text_box.node_id).cloned().unwrap_or_default();
    let nowrap = matches!(
        style.white_space,
        incognidium_style::WhiteSpace::NoWrap | incognidium_style::WhiteSpace::Pre
    ) || matches!(style.text_wrap, TextWrap::NoWrap);
    let preserve_newlines = matches!(
        style.white_space,
        incognidium_style::WhiteSpace::Pre
            | incognidium_style::WhiteSpace::PreWrap
            | incognidium_style::WhiteSpace::PreLine
    ) || matches!(
        style.white_space_collapse,
        WhiteSpaceCollapse::Preserve
            | WhiteSpaceCollapse::PreserveBreaks
            | WhiteSpaceCollapse::BreakSpaces
    );
    if nowrap || preserve_newlines {
        return vec![text_box.clone()];
    }

    // Re-layout with the shortened first-line width so layout_text inserts
    // newlines at the correct first-line boundary.
    let mut narrow = text_box.clone();
    layout_text(&mut narrow, styles, first_line_width);
    let narrow_text = narrow.text.clone().unwrap_or_default();
    let Some(newline_pos) = narrow_text.find('\n') else {
        return vec![text_box.clone()];
    };
    let first_text = narrow_text[..newline_pos].trim_end().to_string();
    let rest_source = narrow_text[newline_pos + 1..].replace('\n', " ");
    let rest_text = rest_source.trim_start().to_string();
    if first_text.is_empty() || rest_text.is_empty() {
        return vec![text_box.clone()];
    }
    let source = text_box
        .source_text
        .as_deref()
        .unwrap_or_else(|| text_box.text.as_deref().unwrap_or_default());
    let boundary_space = split_consumed_source_whitespace(source, &first_text, &rest_text);

    let mut first = text_box.clone();
    first.text = Some(first_text.clone());
    // The fragment's text IS its source (see split_text_at_float_boundary).
    first.source_text = Some(first_text.clone());
    first.text_leading_space = text_box.text_leading_space;
    first.text_trailing_space = first_text.ends_with(char::is_whitespace) || boundary_space;
    layout_text(&mut first, styles, first_line_width);
    let mut rest = text_box.clone();
    rest.text = Some(rest_text.clone());
    rest.source_text = Some(rest_text.clone());
    rest.text_leading_space = rest_text.starts_with(char::is_whitespace) || boundary_space;
    rest.text_trailing_space = text_box.text_trailing_space;
    layout_text(&mut rest, styles, full_width);
    rest.force_line_break_before = true;
    vec![first, rest]
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
            .take_while(|c| is_css_whitespace(*c) && *c != '\n')
            .collect();

        // Split this source line into words (NBSP is content and stays
        // inside its word), keeping the exact whitespace run that precedes
        // each word: pre-wrap preserves every run of whitespace between
        // words, not just a single separating space.
        let mut words: Vec<String> = Vec::new();
        let mut separators: Vec<String> = Vec::new();
        let mut trailing_sep = String::new();
        {
            let core = &source_line[leading_spaces.len()..];
            let mut cur_word = String::new();
            let mut cur_sep = String::new();
            for c in core.chars() {
                if is_css_whitespace(c) && c != '\n' {
                    if !cur_word.is_empty() {
                        words.push(std::mem::take(&mut cur_word));
                        separators.push(std::mem::take(&mut cur_sep));
                    }
                    cur_sep.push(c);
                } else {
                    cur_word.push(c);
                }
            }
            if !cur_word.is_empty() {
                words.push(cur_word);
                separators.push(cur_sep);
            } else {
                // The line ends with whitespace: keep the run so it can be
                // appended after the last word.
                trailing_sep = cur_sep;
            }
        }

        if words.is_empty() {
            // Whitespace-only source line. pre-wrap preserves the spaces
            // themselves (e.g. the gap between two inline spans inside a
            // `<pre>`), so emit them as content; a line with no spaces at all
            // is a blank line and just contributes its newline.
            if !leading_spaces.is_empty() {
                if line_idx > 0 || !all_parts.is_empty() {
                    all_parts.push("\n".to_string());
                }
                all_parts.push(leading_spaces.clone());
                let spaces_width = measure_text_width(&leading_spaces, style.font_size, style);
                max_line_width = max_line_width.max(spaces_width);
            } else if line_idx > 0 || !all_parts.is_empty() {
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
            // Width of the exact whitespace run preceding this word (falls
            // back to a single space plus word-spacing for the first word).
            let sep_width = measure_text_width(
                separators.get(i).map(|s| s.as_str()).unwrap_or(" "),
                style.font_size,
                style,
            ) + style.word_spacing;
            let space_w = if first_word { 0.0 } else { sep_width };
            let needed = space_w + word_width;

            // Check if word needs to be broken (too long for container)
            if word_width > containing_width + 0.5 && can_break_word {
                // Need to break this word
                if !first_word {
                    let sep = separators
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| " ".to_string());
                    all_parts.push(sep);
                    current_line_width += sep_width;
                }

                let mut remaining: &str = word;
                let mut first_piece = true;

                while !remaining.is_empty() {
                    let mut fit_len = 0usize;
                    let mut piece_width = 0.0f32;

                    for (idx, ch) in remaining.char_indices() {
                        let ch_width =
                            measure_text_width(
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
                    let sep = separators
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| " ".to_string());
                    all_parts.push(sep);
                }
                all_parts.push(word.to_string());
                current_line_width += needed;
            }
            first_word = false;
        }

        // Preserve a whitespace run that follows the last word.
        if !trailing_sep.is_empty() {
            all_parts.push(trailing_sep.clone());
            current_line_width +=
                measure_text_width(&trailing_sep, style.font_size, style) + style.word_spacing;
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

/// Measure the rendered width of `text` at `font_size` using fontdue.
/// Falls back to a rough approximation if no TTF is installed.
pub fn measure_text_width(
    text: &str,
    font_size: f32,
    style: &incognidium_style::ComputedStyle,
) -> f32 {
    // NBSP measures with the same advance as a plain space.
    let nbsp_owned;
    let text = if text.contains('\u{00a0}') {
        nbsp_owned = text.replace('\u{00a0}', " ");
        &nbsp_owned
    } else {
        text
    };
    let char_count = text.chars().count() as f32;
    let letter_spacing = style.letter_spacing;

    // A registered @font-face matching the element's named family wins over
    // the built-in fallback fonts: pages size their text (and expect
    // shrink-to-fit to work) against their own fonts, not ours.
    if let Some(family) = style.web_font_family.as_deref() {
        if let Some(font) = get_webfont_font(
            family,
            font_weight_number(&style.font_weight),
            style.font_style == incognidium_style::FontStyle::Italic,
        ) {
            let mut w = 0.0_f32;
            let mut prev = None;
            for ch in text.chars() {
                if let Some(p) = prev {
                    w += font.horizontal_kern(p, ch, font_size).unwrap_or(0.0);
                }
                let metrics = font.metrics(ch, font_size);
                w += metrics.advance_width;
                w += letter_spacing;
                prev = Some(ch);
            }
            if char_count > 0.0 {
                w -= letter_spacing;
            }
            return w;
        }
    }

    if let Some(font) = get_layout_font(
        style.font_weight == incognidium_style::FontWeight::Bold,
        style.font_style == incognidium_style::FontStyle::Italic,
    ) {
        let mut w = 0.0_f32;
        let mut prev = None;
        for ch in text.chars() {
            if let Some(p) = prev {
                w += font.horizontal_kern(p, ch, font_size).unwrap_or(0.0);
            }
            let metrics = font.metrics(ch, font_size);
            w += metrics.advance_width;
            w += letter_spacing;
            prev = Some(ch);
        }
        if char_count > 0.0 {
            w -= letter_spacing;
        }
        w
    } else {
        char_count * font_size * 0.52 + (char_count - 1.0).max(0.0) * letter_spacing
    }
}

static LAYOUT_FONTS: std::sync::OnceLock<Option<LayoutFonts>> = std::sync::OnceLock::new();

struct LayoutFonts {
    regular: fontdue::Font,
    bold: fontdue::Font,
    italic: fontdue::Font,
    bold_italic: fontdue::Font,
}

fn load_layout_fonts() -> Option<LayoutFonts> {
    // 1) Try embedded Roboto fonts first (same fonts the paint crate uses)
    let try_embedded = || -> Option<LayoutFonts> {
        let regular = fontdue::Font::from_bytes(
            include_bytes!("../../../assets/fonts/Roboto-Regular.ttf").to_vec(),
            fontdue::FontSettings::default(),
        )
        .ok()?;
        let bold = fontdue::Font::from_bytes(
            include_bytes!("../../../assets/fonts/Roboto-Bold.ttf").to_vec(),
            fontdue::FontSettings::default(),
        )
        .ok()?;
        let italic = fontdue::Font::from_bytes(
            include_bytes!("../../../assets/fonts/Roboto-Italic.ttf").to_vec(),
            fontdue::FontSettings::default(),
        )
        .ok()?;
        let bold_italic = fontdue::Font::from_bytes(
            include_bytes!("../../../assets/fonts/Roboto-BoldItalic.ttf").to_vec(),
            fontdue::FontSettings::default(),
        )
        .ok()?;
        Some(LayoutFonts {
            regular,
            bold,
            italic,
            bold_italic,
        })
    };
    if let Some(fonts) = try_embedded() {
        return Some(fonts);
    }

    // 2) Fall back to system font directories
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
                fontdue::Font::from_bytes(rr, fontdue::FontSettings::default()),
                fontdue::Font::from_bytes(br, fontdue::FontSettings::default()),
                fontdue::Font::from_bytes(ir, fontdue::FontSettings::default()),
                fontdue::Font::from_bytes(bir, fontdue::FontSettings::default()),
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

fn get_layout_font(bold: bool, italic: bool) -> Option<&'static fontdue::Font> {
    let fonts = LAYOUT_FONTS.get_or_init(load_layout_fonts).as_ref()?;
    Some(match (bold, italic) {
        (true, true) => &fonts.bold_italic,
        (true, false) => &fonts.bold,
        (false, true) => &fonts.italic,
        (false, false) => &fonts.regular,
    })
}

/// Numeric weight for web font matching (`FontWeight` carries CSS keywords).
fn font_weight_number(weight: &incognidium_style::FontWeight) -> u16 {
    match weight {
        incognidium_style::FontWeight::Normal | incognidium_style::FontWeight::Lighter => 400,
        incognidium_style::FontWeight::Bold | incognidium_style::FontWeight::Bolder => 700,
        incognidium_style::FontWeight::Number(n) => (*n).clamp(1, 1000),
    }
}

/// Fonts decoded from registered @font-face data, keyed by
/// (family, weight, italic) so each face is parsed once.
static WEBFONT_FONTS: std::sync::OnceLock<
    std::sync::RwLock<
        std::collections::HashMap<(String, u16, bool), std::sync::Arc<fontdue::Font>>,
    >,
> = std::sync::OnceLock::new();

/// Get (building and caching if needed) the registered web font for a
/// family/weight/style combination.
fn get_webfont_font(
    family: &str,
    weight: u16,
    italic: bool,
) -> Option<std::sync::Arc<fontdue::Font>> {
    use std::sync::Arc;
    let cache =
        WEBFONT_FONTS.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()));
    let key = (family.to_lowercase(), weight, italic);
    if let Ok(map) = cache.read() {
        if let Some(f) = map.get(&key) {
            return Some(Arc::clone(f));
        }
    }
    let data = incognidium_css::webfonts::lookup(family, weight, italic)?;
    let font = match fontdue::Font::from_bytes(&data[..], fontdue::FontSettings::default()) {
        Ok(f) => std::sync::Arc::new(f),
        Err(_) => return None,
    };
    if let Ok(mut map) = cache.write() {
        map.insert(key.clone(), Arc::clone(&font));
    }
    Some(font)
}

fn layout_image(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    containing_height: f32,
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

    // Intrinsic dimensions (may be missing for alt-only or canvas images).
    let (iw, ih) = actual_dims
        .map(|(w, h)| (*w as f32, *h as f32))
        .unwrap_or((0.0, 0.0));

    // The layout engine uses a large sentinel width (10_000) for max-content
    // measurements and other indefinite containing-block situations. In those
    // cases CSS says percentage widths cannot be resolved, so they behave like
    // `auto`. Treating them as the intrinsic width keeps images from ballooning
    // to the sentinel width and inflating the document height.
    const INDEFINITE_WIDTH: f32 = 9_999.0;
    let width_indefinite = containing_width >= INDEFINITE_WIDTH;
    let height_indefinite = containing_height <= 0.0 || containing_height >= INDEFINITE_WIDTH;

    // Determine whether each dimension is set explicitly (px/percent) or is auto.
    let width_auto = matches!(style.width, SizeValue::Auto | SizeValue::None);
    let height_auto = matches!(style.height, SizeValue::Auto | SizeValue::None);

    // Initial used width from explicit style or intrinsic width.
    let mut w = match style.width {
        SizeValue::Px(v) => v,
        SizeValue::Percent(p) if !width_indefinite => containing_width * p / 100.0,
        SizeValue::Calc(_) | SizeValue::Min(_) | SizeValue::Max(_) | SizeValue::Clamp { .. }
            if !width_indefinite =>
        {
            evaluate_size_value(&style.width, containing_width, style.font_size).unwrap_or(iw)
        }
        _ => iw,
    };

    // Apply min/max-width constraints so CSS rules like `img { max-width: 100%; }`
    // scale oversized images to their containing block instead of overflowing it.
    // Skip percent-based constraints when the containing width is indefinite;
    // otherwise a `max-width: 100%` would resolve to the sentinel and have no
    // effect, while a `min-width: 100%` would blow up the image.
    let mut w_clamped = false;
    if !width_indefinite {
        if let Some(mw) = evaluate_size_value(&style.max_width, containing_width, style.font_size) {
            if w > mw {
                w = mw;
                w_clamped = true;
            }
        }
        if let Some(mw) = evaluate_size_value(&style.min_width, containing_width, style.font_size) {
            if w < mw {
                w = mw;
                w_clamped = true;
            }
        }
    } else {
        // When the containing width is the engine's max-content sentinel we have
        // no meaningful containing block. Cap the used width at the headless
        // viewport width (1024) so huge intrinsic images do not inflate the
        // document during measurement passes. This is a pragmatic guard; explicit
        // pixel widths are still honored.
        if !matches!(style.width, SizeValue::Px(_)) {
            w = w.min(1024.0);
        }
    }

    // Initial used height: honor explicit style, otherwise preserve aspect ratio
    // against the (possibly clamped) width.
    let mut h = match style.height {
        SizeValue::Px(v) => v,
        SizeValue::Percent(p)
            if containing_height > 0.0 && containing_height < INDEFINITE_WIDTH =>
        {
            containing_height * p / 100.0
        }
        SizeValue::Calc(_) | SizeValue::Min(_) | SizeValue::Max(_) | SizeValue::Clamp { .. }
            if containing_height > 0.0 && containing_height < INDEFINITE_WIDTH =>
        {
            evaluate_size_value(&style.height, containing_height, style.font_size)
                .unwrap_or_else(|| if iw > 0.0 { w * ih / iw } else { ih })
        }
        _ => {
            if iw > 0.0 {
                w * ih / iw
            } else {
                ih
            }
        }
    };

    // Apply min/max-height constraints and record whether they actually changed
    // the used height so that an explicit width can be recalculated to preserve
    // the intrinsic aspect ratio (e.g. an <img width=197 height=65> with
    // max-height:100% inside a 40 px header should become ~121 px wide).
    let mut h_clamped = false;
    if !height_indefinite {
        if let Some(mh) = evaluate_size_value(&style.max_height, containing_height, style.font_size)
        {
            if h > mh {
                h = mh;
                h_clamped = true;
            }
        }
        if let Some(mh) = evaluate_size_value(&style.min_height, containing_height, style.font_size)
        {
            if h < mh {
                h = mh;
                h_clamped = true;
            }
        }
    }

    // Preserve the intrinsic aspect ratio when exactly one dimension is auto:
    // a fixed height with auto width derives the width from the height, and vice
    // versa. This fixes replaced elements such as logos that use `height: 100%;
    // width: auto` with a source whose intrinsic size is much larger than the
    // rendered container.
    let has_intrinsic_ratio = iw > 0.0 && ih > 0.0;
    if width_auto && !height_auto && has_intrinsic_ratio {
        w = h * iw / ih;
        if !width_indefinite {
            if let Some(mw) =
                evaluate_size_value(&style.max_width, containing_width, style.font_size)
            {
                w = w.min(mw);
            }
            if let Some(mw) =
                evaluate_size_value(&style.min_width, containing_width, style.font_size)
            {
                w = w.max(mw);
            }
        }
    } else if !width_auto && height_auto && has_intrinsic_ratio {
        h = w * ih / iw;
        if !height_indefinite {
            if let Some(mh) =
                evaluate_size_value(&style.max_height, containing_height, style.font_size)
            {
                h = h.min(mh);
            }
            if let Some(mh) =
                evaluate_size_value(&style.min_height, containing_height, style.font_size)
            {
                h = h.max(mh);
            }
        }
    } else if !width_auto && !height_auto && has_intrinsic_ratio {
        // Both dimensions are specified (e.g. HTML width/height attributes) but a
        // max/min constraint changed one of them. Recompute the unconstrained
        // dimension from the intrinsic ratio so the image is not stretched.
        if h_clamped && !w_clamped {
            w = h * iw / ih;
            if !width_indefinite {
                if let Some(mw) =
                    evaluate_size_value(&style.max_width, containing_width, style.font_size)
                {
                    w = w.min(mw);
                }
                if let Some(mw) =
                    evaluate_size_value(&style.min_width, containing_width, style.font_size)
                {
                    w = w.max(mw);
                }
            }
        } else if w_clamped && !h_clamped {
            h = w * ih / iw;
            if !height_indefinite {
                if let Some(mh) =
                    evaluate_size_value(&style.max_height, containing_height, style.font_size)
                {
                    h = h.min(mh);
                }
                if let Some(mh) =
                    evaluate_size_value(&style.min_height, containing_height, style.font_size)
                {
                    h = h.max(mh);
                }
            }
        }
    }

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
    let boxes = flatten_with_clip(
        layout_box, offset_x, offset_y, None, false, false, styles, 0, None, None,
    );
    boxes
}

fn flatten_with_clip(
    layout_box: &LayoutBox,
    offset_x: f32,
    offset_y: f32,
    parent_clip: Option<(f32, f32, f32, f32)>,
    in_fixed_subtree: bool,
    in_absolute_subtree: bool,
    styles: &StyleMap,
    depth: u32,
    stacking_context_root: Option<NodeId>,
    parent_clip_path: Option<&incognidium_style::ClipPath>,
) -> Vec<FlatBox> {
    let mut result = Vec::new();
    let abs_x = offset_x + layout_box.x;
    let abs_y = offset_y + layout_box.y;

    // Determine clip rect: if this box has overflow:hidden, clip children to its bounds
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();
    let own_clip_path = style
        .clip_path
        .as_ref()
        .filter(|cp| **cp != incognidium_style::ClipPath::None);
    let effective_clip_path = own_clip_path.or(parent_clip_path);
    let has_hidden_overflow = matches!(style.overflow, Overflow::Hidden | Overflow::Scroll)
        || matches!(style.overflow, Overflow::Auto);
    // CSS clip:rect(0 0 0 0) (and the -webkit- variant) is the standard accessibility-only
    // pattern for screen-reader text. A zero-area clip removes the element from the visible
    // rendering entirely, so skip the whole subtree.
    let fully_clipped = matches!(
        style.clip,
        ClipRect::Rect {
            top,
            right,
            bottom,
            left,
        } if left >= right || top >= bottom
    );
    if fully_clipped {
        return result;
    }
    // Fixed-positioned boxes (and their descendants) are relative to the viewport
    // and must not influence the normal-flow document height.
    let in_fixed = in_fixed_subtree || style.position == Position::Fixed;
    // Absolutely-positioned boxes (and their descendants) are removed from normal
    // flow and should not influence the normal-flow document height.
    let in_absolute = in_absolute_subtree || style.position == Position::Absolute;
    // For overflow clipping the spec clips descendants to the PADDING box, not
    // the content box. Clipping to the content box would hide children that
    // legitimately fill the padding area, which is how common aspect-ratio
    // wrappers are built: a zero content box (height:0) whose padding-bottom
    // reserves the box height, with an absolutely-positioned image stretched to
    // the containing block's padding box inside it. Those wrappers would be
    // clipped to nothing and lose their images.
    let content_clip_bounds = (
        abs_x + style.border_left_width,
        abs_y + style.border_top_width,
        (layout_box.width - style.border_left_width - style.border_right_width).max(0.0),
        (layout_box.height - style.border_top_width - style.border_bottom_width).max(0.0),
    );

    // Determine if this box establishes a new stacking context. Simplified criteria:
    // positioned element with non-auto z-index, opacity < 1, transform, filter,
    // clip-path, or isolation. New contexts clip descendants visually to the
    // context, so we group them for painting.
    let has_clip_path = style
        .clip_path
        .as_ref()
        .map_or(false, |cp| *cp != incognidium_style::ClipPath::None);
    let establishes_stacking_context = (style.position != Position::Static
        && style.z_index.is_some())
        || style.opacity < 1.0
        || !style.transform.is_empty()
        || !style.filter.is_empty()
        || has_clip_path;
    let own_root = if establishes_stacking_context {
        Some(layout_box.node_id)
    } else {
        stacking_context_root
    };

    // The effective clip is the intersection of parent clip and own bounds (if overflow:hidden).
    // If this overflow:hidden container itself has zero width/height, keep the parent clip
    // instead of collapsing to an empty clip. Zero-height wrappers are common in modern
    // layouts (flex/grid parents with only positioned children) and our engine may report a
    // zero height even though their children are visible, so clipping them away would hide
    // all content.
    //
    // For full-page renders, never let the root (<html>, depth 1) or its immediate child
    // (<body>, depth 2) establish an overflow clip. The layout tree root is the document node
    // (depth 0), so body is depth 2, not 1. Many sites set body{overflow:hidden;height:100%} or
    // body{overflow-y:auto;height:100vh} to prevent scrolling on desktop, which would otherwise
    // clip all content below the fold.
    let clip = if has_hidden_overflow && depth > 2 {
        if content_clip_bounds.2 <= 0.0 || content_clip_bounds.3 <= 0.0 {
            // Zero-size overflow:hidden containers are often wrappers that should
            // have measured non-zero height but don't due to missing layout features
            // (e.g. wrappers holding only absolutely-positioned children). Keep
            // the parent clip in that case so positioned descendants remain visible.
            // If the container has normal-flow children, however, CSS requires the
            // overflow clip to apply even when the container is zero-height; this
            // makes CSS-only accordions (grid-template-rows: 0fr) collapse.
            let has_static_children = layout_box.children.iter().any(|c| {
                let cs = styles.get(&c.node_id).cloned().unwrap_or_default();
                cs.position == Position::Static && c.box_type != BoxType::Text
            });
            if has_static_children {
                Some((0.0, 0.0, 0.0, 0.0))
            } else {
                parent_clip
            }
        } else {
            match parent_clip {
                Some((px, py, pw, ph)) => {
                    // Intersect parent clip with own bounds
                    let x1 = px.max(content_clip_bounds.0);
                    let y1 = py.max(content_clip_bounds.1);
                    let x2 = (px + pw).min(content_clip_bounds.0 + content_clip_bounds.2);
                    let y2 = (py + ph).min(content_clip_bounds.1 + content_clip_bounds.3);
                    if x2 > x1 && y2 > y1 {
                        Some((x1, y1, x2 - x1, y2 - y1))
                    } else {
                        Some((0.0, 0.0, 0.0, 0.0)) // Empty clip = nothing visible
                    }
                }
                None => Some(content_clip_bounds),
            }
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
        // Check if this box is entirely outside the clip.
        // Use strict `<` for the top/left edges so that zero-height or zero-width
        // ancestors that sit exactly on the clip boundary are still recursed into;
        // they may contain positioned children that are visible (e.g. search-style
        // page wrappers where all content is inside fixed/absolute descendants).
        if abs_x + layout_box.width < cx
            || abs_y + layout_box.height < cy
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
            .map(|t| is_collapsible_whitespace_only(t))
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
            clip_path: own_clip_path.cloned().or(parent_clip_path.cloned()),
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
            in_fixed_subtree: in_fixed,
            in_absolute_subtree: in_absolute,
            depth,
            stacking_context_root: own_root,
            parent_stacking_context: stacking_context_root,
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
        let mut child_boxes = flatten_with_clip(
            child,
            child_offset.0,
            child_offset.1,
            clip,
            in_fixed,
            in_absolute,
            styles,
            depth + 1,
            own_root,
            effective_clip_path,
        );
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

    // If this box has a CSS transform, it establishes a local coordinate system for
    // its descendants. Flattening records absolute positions, so we must apply
    // the parent transform to every descendant flat box now; otherwise text/image
    // fragments inside inline or block containers with transforms (e.g. an
    // off-screen skip link) remain at their untransformed positions and render as
    // visible clutter.
    if !style.transform.is_empty() {
        apply_transform_to_flat_boxes(
            &style.transform,
            style.transform_origin,
            abs_x,
            abs_y,
            layout_box.width,
            layout_box.height,
            &mut result,
        );
    }

    result
}

/// Apply a CSS transform list to a set of flat-box positions.
/// Transforms are applied around the transform-origin point of the box that
/// owns them. Only the 2-D position is updated; for rotate/scale/skew this
/// is a simplification that still fixes translate-based off-screen positioning.
fn apply_transform_to_flat_boxes(
    transforms: &[incognidium_style::Transform],
    origin: (f32, f32),
    abs_x: f32,
    abs_y: f32,
    width: f32,
    height: f32,
    boxes: &mut [FlatBox],
) {
    let origin_x = abs_x + width * origin.0;
    let origin_y = abs_y + height * origin.1;

    // Build a 2x3 affine matrix from the transform list.
    // M = [a b c; d e f] so that (x,y) -> (a*x + b*y + c, d*x + e*y + f).
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
        // M' = N * M
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
    // Each flat box is remapped to the axis-aligned bounding box of its
    // transformed rectangle: mapping only the top-left corner would displace
    // the box by up to its full width/height under rotation (a 90° icon
    // rotation would shift it exactly one edge length sideways), and the
    // painter draws boxes axis-aligned into x/y/width/height.
    for fb in boxes.iter_mut() {
        let x0 = fb.x;
        let y0 = fb.y;
        let x1 = fb.x + fb.width;
        let y1 = fb.y + fb.height;
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for (px, py) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
            let dx = px - origin_x;
            let dy = py - origin_y;
            let tx = origin_x + a * dx + b * dy + c;
            let ty = origin_y + d * dx + e * dy + f;
            min_x = min_x.min(tx);
            min_y = min_y.min(ty);
            max_x = max_x.max(tx);
            max_y = max_y.max(ty);
        }
        fb.x = min_x;
        fb.y = min_y;
        fb.width = max_x - min_x;
        fb.height = max_y - min_y;
    }
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
    /// CSS clip-path inherited from the nearest ancestor that defines one.
    /// Applied during paint to clip this box's content.
    pub clip_path: Option<incognidium_style::ClipPath>,
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
    /// Whether this box is inside a fixed-positioned subtree. Such boxes are
    /// relative to the viewport and must not contribute to the document's
    /// scrollable content height.
    pub in_fixed_subtree: bool,
    /// Whether this box is inside an absolutely-positioned subtree. Absolutely
    /// positioned boxes are removed from normal flow and should not influence
    /// the normal-flow document height, even if their descendants are laid out.
    pub in_absolute_subtree: bool,
    /// Tree depth from the root layout box. Used when deciding whether an
    /// out-of-flow subtree is a deep off-canvas menu (exclude) or the root/body
    /// itself (keep, even if it has a positioning quirk).
    pub depth: u32,
    /// The nearest ancestor node id that establishes a CSS stacking context.
    /// Boxes inside the same stacking context must be painted as a group so that
    /// the context root's background/borders are not painted on top of its
    /// descendants by a global z-index sort.
    pub stacking_context_root: Option<incognidium_dom::NodeId>,
    /// The stacking context in which this box is painted. For a box that
    /// establishes its own stacking context, this is its parent context; for
    /// all other boxes it equals `stacking_context_root`.
    pub parent_stacking_context: Option<incognidium_dom::NodeId>,
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
        'α', 'β', 'γ', 'δ', 'ε', 'ζ', 'η', 'θ', 'ι', 'κ', 'λ', 'μ', 'ν', 'ξ', 'ο', 'π', 'ρ', 'σ',
        'τ', 'υ', 'φ', 'χ', 'ψ', 'ω',
    ];
    let greek_upper = [
        'Α', 'Β', 'Γ', 'Δ', 'Ε', 'Ζ', 'Η', 'Θ', 'Ι', 'Κ', 'Λ', 'Μ', 'Ν', 'Ξ', 'Ο', 'Π', 'Ρ', 'Σ',
        'Τ', 'Υ', 'Φ', 'Χ', 'Ψ', 'Ω',
    ];

    let letters = if uppercase {
        &greek_upper
    } else {
        &greek_lower
    };
    let base = letters.len();

    if n <= base {
        letters
            .get(n - 1)
            .map(|c| c.to_string())
            .unwrap_or_default()
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
        (9000, "Ք"),
        (8000, "Փ"),
        (7000, "Ւ"),
        (6000, "Ց"),
        (5000, "Ր"),
        (4000, "Տ"),
        (3000, "Վ"),
        (2000, "Ս"),
        (1000, "Ռ"),
        (900, "Ջ"),
        (800, "Պ"),
        (700, "Չ"),
        (600, "Ո"),
        (500, "Շ"),
        (400, "Ն"),
        (300, "Յ"),
        (200, "Մ"),
        (100, "Ճ"),
        (90, "Ղ"),
        (80, "Ձ"),
        (70, "Հ"),
        (60, "Կ"),
        (50, "Ծ"),
        (40, "Խ"),
        (30, "Լ"),
        (20, "Ի"),
        (10, "Ժ"),
        (9, "Թ"),
        (8, "Ը"),
        (7, "Է"),
        (6, "Զ"),
        (5, "Ե"),
        (4, "Դ"),
        (3, "Գ"),
        (2, "Բ"),
        (1, "Ա"),
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
        (10000, "ჯ"),
        (9000, "ჴ"),
        (8000, ""),
        (7000, ""),
        (6000, ""),
        (5000, "ჰ"),
        (4000, "ჳ"),
        (3000, "ჲ"),
        (2000, "ჱ"),
        (1000, "ჺ"),
        (900, "ჵ"),
        (800, ""),
        (700, ""),
        (600, ""),
        (500, "ჭ"),
        (400, ""),
        (300, ""),
        (200, ""),
        (100, "რ"),
        (90, ""),
        (80, ""),
        (70, ""),
        (60, ""),
        (50, "ნ"),
        (40, ""),
        (30, ""),
        (20, ""),
        (10, "ი"),
        (9, "შ"),
        (8, "ყ"),
        (7, "ღ"),
        (6, "ქ"),
        (5, "ფ"),
        (4, "ჳ"),
        (3, "ბ"),
        (2, "გ"),
        (1, "ა"),
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
        (400, "ת"),
        (300, "ש"),
        (200, "ר"),
        (100, "ק"),
        (90, "צ"),
        (80, "פ"),
        (70, "ע"),
        (60, "ס"),
        (50, "נ"),
        (40, "מ"),
        (30, "ל"),
        (20, "כ"),
        (10, "י"),
        (9, "ט"),
        (8, "ח"),
        (7, "ז"),
        (6, "ו"),
        (5, "ה"),
        (4, "ד"),
        (3, "ג"),
        (2, "ב"),
        (1, "א"),
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
        "あ", "い", "う", "え", "お", "か", "き", "く", "け", "こ", "さ", "し", "す", "せ", "そ",
        "た", "ち", "つ", "て", "と", "な", "に", "ぬ", "ね", "の", "は", "ひ", "ふ", "へ", "ほ",
        "ま", "み", "む", "め", "も", "や", "ゆ", "よ", "ら", "り", "る", "れ", "ろ", "わ", "ゐ",
        "ゑ", "を", "ん",
    ];
    hiragana.get(n - 1).unwrap_or(&"").to_string()
}

fn number_to_katakana(mut n: usize) -> String {
    if n == 0 || n > 48 {
        return format!("{}", n);
    }
    // Katakana equivalent pattern
    let katakana = [
        "ア", "イ", "ウ", "エ", "オ", "カ", "キ", "ク", "ケ", "コ", "サ", "シ", "ス", "セ", "ソ",
        "タ", "チ", "ツ", "テ", "ト", "ナ", "ニ", "ヌ", "ネ", "ノ", "ハ", "ヒ", "フ", "ヘ", "ホ",
        "マ", "ミ", "ム", "メ", "モ", "ヤ", "ユ", "ヨ", "ラ", "リ", "ル", "レ", "ロ", "ワ", "ヰ",
        "ヱ", "ヲ", "ン",
    ];
    katakana.get(n - 1).unwrap_or(&"").to_string()
}

fn number_to_hiragana_iroha(mut n: usize) -> String {
    if n == 0 || n > 47 {
        return format!("{}", n);
    }
    // Iroha sequence - traditional Japanese ordering
    let iroha = [
        "い", "ろ", "は", "に", "ほ", "へ", "と", "ち", "り", "ぬ", "る", "を", "わ", "か", "よ",
        "た", "れ", "そ", "つ", "ね", "な", "ら", "む", "う", "の", "お", "く", "き", "ま", "け",
        "ふ", "こ", "え", "て", "あ", "さ", "き", "ゆ", "め", "み", "し", "ゑ", "ひ", "も", "せ",
        "す",
    ];
    iroha.get(n - 1).unwrap_or(&"").to_string()
}

fn number_to_katakana_iroha(mut n: usize) -> String {
    if n == 0 || n > 47 {
        return format!("{}", n);
    }
    // Katakana Iroha sequence
    let iroha = [
        "イ", "ロ", "ハ", "ニ", "ホ", "ヘ", "ト", "チ", "リ", "ヌ", "ル", "ヲ", "ワ", "カ", "ヨ",
        "タ", "レ", "ソ", "ツ", "ネ", "ナ", "ラ", "ム", "ウ", "ノ", "オ", "ク", "キ", "マ", "ケ",
        "フ", "コ", "エ", "テ", "ア", "サ", "キ", "ユ", "メ", "ミ", "シ", "ヱ", "ヒ", "モ", "セ",
        "ス",
    ];
    iroha.get(n - 1).unwrap_or(&"").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use incognidium_dom::{Document, ElementData, NodeData, TextData};

    #[test]
    fn test_comment_node_takes_no_layout_space() {
        // Comments must never render. A comment inside a styled container used
        // to inherit the container's full style (height:100%, background), so
        // the empty comment block survived empty-box pruning as a full-height
        // phantom that pushed every following sibling out of the container's
        // scroll clip, hiding the container's content entirely.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut outer = ElementData::new("div");
        outer
            .attributes
            .insert("class".to_string(), "outer".to_string());
        let outer_id = doc.add_node(body, NodeData::Element(outer));
        let mut scroller = ElementData::new("div");
        scroller
            .attributes
            .insert("class".to_string(), "scroller".to_string());
        let scroller_id = doc.add_node(outer_id, NodeData::Element(scroller));
        let _comment = doc.add_node(scroller_id, NodeData::Comment("nav marker".to_string()));
        let h2 = doc.add_node(scroller_id, NodeData::Element(ElementData::new("h2")));
        let _h2_text = doc.add_node(
            h2,
            NodeData::Text(TextData {
                content: "Section".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            "body { margin: 0; } \
             .outer { position: fixed; top: 0; height: 500px; width: 300px; } \
             .scroller { height: 100%; background-color: #eee; overflow-y: scroll; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_element_box(
            root: &LayoutBox,
            node_id: incognidium_dom::NodeId,
        ) -> Option<LayoutBox> {
            if root.node_id == node_id {
                return Some(root.clone());
            }
            root.children
                .iter()
                .find_map(|c| find_element_box(c, node_id))
        }
        let h2_box = find_element_box(&root, h2).expect("h2 box found");
        assert!(
            h2_box.y < 100.0,
            "comment must not take layout space, h2 landed at y={}",
            h2_box.y
        );
    }

    #[test]
    fn test_text_after_inline_element_fills_remaining_line() {
        // A text node that follows an inline element and does not fit in the
        // space left on the line must fill that remaining space and wrap onto
        // the following lines. Pushing the whole node to a fresh line left a
        // ragged gap after the inline element on every line it preceded.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let p = doc.add_node(body, NodeData::Element(ElementData::new("p")));
        let _lead = doc.add_node(
            p,
            NodeData::Text(TextData {
                content: "Cascading Style Sheets is a ".to_string(),
            }),
        );
        let anchor = doc.add_node(p, NodeData::Element(ElementData::new("a")));
        let _anchor_text = doc.add_node(
            anchor,
            NodeData::Text(TextData {
                content: "stylesheet language".to_string(),
            }),
        );
        let _rest = doc.add_node(
            p,
            NodeData::Text(TextData {
                content: "used to describe the presentation of a document written in HTML"
                    .to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            "body { margin: 0; } \
             p { width: 500px; margin: 0; font-size: 16px; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_element_box(
            root: &LayoutBox,
            node_id: incognidium_dom::NodeId,
        ) -> Option<LayoutBox> {
            if root.node_id == node_id {
                return Some(root.clone());
            }
            root.children
                .iter()
                .find_map(|c| find_element_box(c, node_id))
        }
        let p_box = find_element_box(&root, p).expect("p box found");
        // Direct text fragments of the paragraph, in box order.
        let fragments: Vec<_> = p_box
            .children
            .iter()
            .filter(|c| c.box_type == BoxType::Text && c.text.is_some())
            .collect();
        // The trailing text node must be split so its first fragment continues
        // on the first line after the anchor. Direct text fragments of the
        // paragraph are: lead text, then the continuation's fragments (the
        // anchor's text lives inside the anchor box).
        assert!(
            fragments.len() >= 3,
            "trailing text should split into at least two fragments, got {}",
            fragments.len()
        );
        let lead = fragments[0];
        let rest_first = fragments[1];
        assert!(
            (rest_first.y - lead.y).abs() < 1.0,
            "continuation text should start on the same line as the lead text, \
             lead y={} rest y={}",
            lead.y,
            rest_first.y
        );
        assert!(
            rest_first.x > 300.0,
            "continuation text should sit after the inline element, got x={}",
            rest_first.x
        );
    }

    #[test]
    fn test_text_relayout_uses_pristine_source_after_narrow_pass() {
        // A measuring pass at a narrow width can split long words into
        // fragments (overflow-wrap: anywhere). Re-laying the same text box at
        // a wider width must re-break from the pristine text, not from the
        // previous pass's fragments, which would render as separate words.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut p = ElementData::new("p");
        p.attributes.insert("class".to_string(), "t".to_string());
        let p_id = doc.add_node(body, NodeData::Element(p));
        let text_id = doc.add_node(
            p_id,
            NodeData::Text(TextData {
                content: "Empowering everyone to build reliable software".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            "body { margin: 0; } .t { width: 60px; overflow-wrap: anywhere; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        // Find the laid-out text box (its text now holds the narrow-width
        // line-broken form).
        fn find_text_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<LayoutBox> {
            if root.node_id == node_id && root.box_type == BoxType::Text {
                return Some(root.clone());
            }
            root.children.iter().find_map(|c| find_text_box(c, node_id))
        }
        let mut text_box = find_text_box(&root, text_id).expect("text box found");
        assert!(
            text_box.text.as_ref().unwrap().contains('\n'),
            "narrow pass should have broken the word"
        );

        // Re-lay the same box at a wider width, as the final pass does.
        layout_text(&mut text_box, &styles, 400.0);
        let rendered = text_box.text.as_ref().unwrap();
        let words: Vec<&str> = rendered.split_whitespace().collect();
        assert!(
            words.contains(&"Empowering") && words.contains(&"everyone"),
            "wide pass must re-break from the pristine text, got: {rendered:?}"
        );
    }

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

    #[test]
    fn test_negative_margin_collapses_upward() {
        // Regression for hero headline overlays: a block with a negative top
        // margin should overlap the preceding sibling instead of being treated
        // as a zero top margin.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let top = doc.add_node(body, NodeData::Element(ElementData::new("div")));
        let bottom = doc.add_node(body, NodeData::Element(ElementData::new("div")));
        let _ = doc.add_node(
            bottom,
            NodeData::Text(TextData {
                content: "overlap".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            "body { margin: 0; } \
             div { display: block; } \
             div:first-child { height: 100px; background: #ccc; } \
             div:last-child { margin-top: -60px; height: auto; background: #f00; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 800.0, 600.0);
        let root = layout(&doc, &styles, 800.0, 600.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let top_box = find_box(&root, top).expect("top box found");
        let bottom_box = find_box(&root, bottom).expect("bottom box found");

        assert!(
            bottom_box.y < top_box.y + top_box.height,
            "negative margin should pull second block over the first: bottom y {} vs top bottom {}",
            bottom_box.y,
            top_box.y + top_box.height
        );
    }

    #[test]
    fn test_flex_input_with_grow_fills_remaining_space() {
        // Regression for header search bars: a text <input> with an explicit
        // intrinsic width and `flex: 1 0 0` inside a flex wrapper should grow to
        // fill the free space of its flex container, not stay at its intrinsic
        // width.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut form_el = ElementData::new("form");
        form_el
            .attributes
            .insert("class".to_string(), "search-form".to_string());
        let form = doc.add_node(body, NodeData::Element(form_el));
        let mut wrapper_el = ElementData::new("div");
        wrapper_el
            .attributes
            .insert("class".to_string(), "input-wrapper".to_string());
        let wrapper = doc.add_node(form, NodeData::Element(wrapper_el));
        let mut input_el = ElementData::new("input");
        input_el
            .attributes
            .insert("class".to_string(), "search-input".to_string());
        input_el
            .attributes
            .insert("type".to_string(), "text".to_string());
        let input = doc.add_node(wrapper, NodeData::Element(input_el));

        let stylesheet = incognidium_css::parse_css(
            "body { margin: 0; } \
             .search-form { display: flex; width: 600px; height: 46px; } \
             .input-wrapper { display: flex; flex: 1 0 0; height: 20px; } \
             .search-input { width: 200px; flex: 1 0 0; border: none; padding: 0; height: 46px; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let input_box = find_box(&root, input).expect("input layout box found");
        println!("input width: {}", input_box.width);
        // The wrapper is the only flex item in the 600px form, so the input inside
        // the wrapper should grow to nearly the full width (minus any UA defaults).
        assert!(
            input_box.width > 500.0,
            "input should grow to fill flex space, got {}",
            input_box.width
        );
    }

    #[test]
    fn test_wrapping_flex_container_auto_width_uses_single_line_max_content() {
        // Regression for multi-item top navigation bars: a row flex container with
        // `flex-wrap: wrap` and `width: auto` should report a max-content intrinsic
        // width equal to the sum of its items on a single line, not the width of
        // the widest item. Otherwise the container collapses to the widest item and
        // its children stack vertically.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut outer_el = ElementData::new("nav");
        outer_el
            .attributes
            .insert("class".to_string(), "outer".to_string());
        let outer = doc.add_node(body, NodeData::Element(outer_el));
        let mut logo_el = ElementData::new("div");
        logo_el
            .attributes
            .insert("class".to_string(), "logo".to_string());
        let logo = doc.add_node(outer, NodeData::Element(logo_el));
        let _logo_text = doc.add_node(
            logo,
            NodeData::Text(TextData {
                content: "Logo".to_string(),
            }),
        );
        let mut nav_el = ElementData::new("ul");
        nav_el
            .attributes
            .insert("class".to_string(), "nav".to_string());
        let nav = doc.add_node(outer, NodeData::Element(nav_el));

        let items = ["Install", "Learn", "Playground", "Tools", "Governance"];
        let mut item_nodes = Vec::new();
        for text in &items {
            let mut li_el = ElementData::new("li");
            li_el
                .attributes
                .insert("class".to_string(), "item".to_string());
            let li = doc.add_node(nav, NodeData::Element(li_el));
            let _ = doc.add_node(
                li,
                NodeData::Text(TextData {
                    content: (*text).to_string(),
                }),
            );
            item_nodes.push(li);
        }

        let stylesheet = incognidium_css::parse_css(
            "body { margin: 0; font-family: sans-serif; } \
             .outer { display: flex; width: 800px; } \
             .logo { width: 100px; flex: none; } \
             .nav { display: flex; flex-wrap: wrap; width: auto; flex: none; padding: 0 16px; gap: 8px; } \
             .item { display: block; padding: 0 8px; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let nav_box = find_box(&root, nav).expect("nav layout box found");
        // With ~5 short labels plus gaps and padding, the nav should be wide
        // enough for a single row. The old buggy behavior gave it only the width
        // of the widest item (~90 px plus padding), so all items stacked.
        assert!(
            nav_box.width > 300.0,
            "wrapping auto-width nav should use single-line max-content width, got {}",
            nav_box.width
        );
        assert!(
            nav_box.height < 80.0,
            "wrapping auto-width nav should stay on one line, height got {}",
            nav_box.height
        );

        // All list items should share roughly the same y position (one line).
        let first_item_box = find_box(&root, item_nodes[0]).expect("first item found");
        let last_item_box = find_box(&root, *item_nodes.last().unwrap()).expect("last item found");
        assert!(
            (first_item_box.y - last_item_box.y).abs() < 5.0,
            "nav items should be on the same line: first y={}, last y={}",
            first_item_box.y,
            last_item_box.y
        );
    }

    #[test]
    fn test_flex_input_with_explicit_width_and_zero_basis_grows() {
        // Regression for header search bars: an <input> with an explicit
        // `width: 200px` and `flex: 1 1 0%` inside a nested flex wrapper should
        // still grow to fill the remaining space. The explicit width must not
        // pin the item when `flex-basis` is explicit zero and `flex-grow` is
        // non-zero.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut form_el = ElementData::new("form");
        form_el
            .attributes
            .insert("class".to_string(), "search-form".to_string());
        let form = doc.add_node(body, NodeData::Element(form_el));
        let mut wrapper_el = ElementData::new("div");
        wrapper_el
            .attributes
            .insert("class".to_string(), "input-wrapper".to_string());
        let wrapper = doc.add_node(form, NodeData::Element(wrapper_el));
        let mut input_el = ElementData::new("input");
        input_el
            .attributes
            .insert("class".to_string(), "search-input".to_string());
        input_el
            .attributes
            .insert("type".to_string(), "text".to_string());
        let input = doc.add_node(wrapper, NodeData::Element(input_el));
        let mut btn_el = ElementData::new("button");
        btn_el
            .attributes
            .insert("class".to_string(), "search-btn".to_string());
        let _btn = doc.add_node(form, NodeData::Element(btn_el));

        let stylesheet = incognidium_css::parse_css(
            "body { margin: 0; } \
             .search-form { display: flex; width: 600px; height: 46px; gap: 4px; padding: 0 10px; } \
             .input-wrapper { display: flex; flex: 1 1 0%; height: 20px; align-items: center; } \
             .search-input { width: 200px; flex: 1 1 0%; border: none; padding: 0; height: 46px; } \
             .search-btn { width: 60px; height: 40px; flex: none; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let input_box = find_box(&root, input).expect("input layout box found");
        println!(
            "wrapper width: {}, input width: {}",
            find_box(&root, wrapper).unwrap().width,
            input_box.width
        );
        // The wrapper should grow to take the free space (600 - 20 padding - 60 btn - 4 gap = 516),
        // and the input inside should fill the wrapper.
        assert!(
            input_box.width > 400.0,
            "input with flex:1 1 0% should grow to fill wrapper, got {}",
            input_box.width
        );
    }

    #[test]
    fn test_absolute_inset_stretch() {
        // An absolutely positioned child with left/right/top/bottom=0 inside a
        // sized relative parent should fill that parent, not shrink-wrap.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut parent_el = ElementData::new("div");
        parent_el
            .attributes
            .insert("class".to_string(), "parent".to_string());
        let parent = doc.add_node(body, NodeData::Element(parent_el));
        let mut child_el = ElementData::new("div");
        child_el
            .attributes
            .insert("class".to_string(), "child".to_string());
        let child = doc.add_node(parent, NodeData::Element(child_el));
        let _ = doc.add_node(
            child,
            NodeData::Text(TextData {
                content: "overlay".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            ".parent { position: relative; width: 800px; height: 400px; } \
             .child { position: absolute; left: 0; right: 0; top: 0; bottom: 0; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let child_box = find_box(&root, child).expect("child layout box found");
        assert!(
            (child_box.width - 800.0).abs() < 1.0,
            "child width should stretch to parent: got {}",
            child_box.width
        );
        assert!(
            (child_box.height - 400.0).abs() < 1.0,
            "child height should stretch to parent: got {}",
            child_box.height
        );
    }

    #[test]
    fn test_pua_icon_glyph_width() {
        // Private-use-area glyphs used by icon fonts (e.g. Font Awesome) should not
        // measure as thousands of pixels when the real font is unavailable. The
        // fallback Roboto font should report a reasonable advance width for missing
        // glyphs, but fontdue sometimes returns the design-unit default (e.g. 1000
        // units scaled by 14px/2048 → ~6.8px). This test documents the actual
        // behavior so we can detect regressions.
        let style = ComputedStyle::default();
        let w = measure_text_width("\u{e801}", 14.0, &style);
        println!("PUA glyph U+E801 width at 14px: {}", w);
        assert!(w < 100.0, "PUA glyph should not be enormous: {}", w);
    }

    #[test]
    fn test_padding_bottom_percent_expands_parent() {
        // A block child whose only vertical extent comes from percentage
        // padding-bottom (common aspect-ratio hack, e.g. The Intercept hero)
        // must still expand its parent's auto height.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut wrapper_el = ElementData::new("div");
        wrapper_el
            .attributes
            .insert("class".to_string(), "wrapper".to_string());
        let wrapper = doc.add_node(body, NodeData::Element(wrapper_el));
        let mut child_el = ElementData::new("div");
        child_el
            .attributes
            .insert("class".to_string(), "link".to_string());
        let child = doc.add_node(wrapper, NodeData::Element(child_el));
        // Add an absolutely-positioned text child so the link is not collapsed
        // to BoxType::None during tree building, but the text does not
        // contribute to the link's in-flow content height (mirrors The
        // Intercept hero, whose image/text overlay are absolute).
        let mut span_el = ElementData::new("span");
        span_el
            .attributes
            .insert("class".to_string(), "span".to_string());
        let span = doc.add_node(child, NodeData::Element(span_el));
        let _ = doc.add_node(
            span,
            NodeData::Text(TextData {
                content: "x".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            ".wrapper { width: 1024px; } \
             .link { display: block; padding-bottom: 50%; position: relative; } \
             .link span { position: absolute; left: 0; top: 0; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let wrapper_box = find_box(&root, wrapper).expect("wrapper layout box found");
        assert!(
            (wrapper_box.height - 512.0).abs() < 1.0,
            "wrapper should expand to child's padding-bottom height: got {}",
            wrapper_box.height
        );
    }

    #[test]
    fn test_calc_width_with_border_box_subtracts_padding_border() {
        // width: calc(...) with box-sizing: border-box sets the total border-box
        // width, so the content box must be the evaluated expression minus
        // padding and border. Previously calc()/min()/max()/clamp() were used
        // directly as the content width, making boxes too wide.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut el = ElementData::new("div");
        el.attributes
            .insert("class".to_string(), "sized".to_string());
        let node = doc.add_node(body, NodeData::Element(el));

        let stylesheet = incognidium_css::parse_css(
            ".sized { width: calc(100% - 40px); box-sizing: border-box; padding: 10px; border: 2px solid black; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let box_node = find_box(&root, node).expect("sized box found");
        // Total border-box width should equal calc(100% - 40px) = 984.
        assert!(
            (box_node.width - 984.0).abs() < 1.0,
            "border-box total width should be 984, got {}",
            box_node.width
        );
        // Content width should be 984 - 10 - 10 - 2 - 2 = 960.
        assert!(
            (box_node.content_width - 960.0).abs() < 1.0,
            "border-box content width should be 960, got {}",
            box_node.content_width
        );
    }

    #[test]
    fn test_floated_calc_width_with_border_box() {
        // A floated element with width: calc(...) and box-sizing: border-box
        // should have its total width equal the evaluated expression, and its
        // content width reduced by padding/border. The float code must pass the
        // original containing width to layout_block so percentages inside the
        // expression are evaluated exactly once.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut container_el = ElementData::new("div");
        container_el
            .attributes
            .insert("class".to_string(), "container".to_string());
        let container = doc.add_node(body, NodeData::Element(container_el));
        let mut float_el = ElementData::new("div");
        float_el
            .attributes
            .insert("class".to_string(), "floated".to_string());
        let floated = doc.add_node(container, NodeData::Element(float_el));
        let _ = doc.add_node(
            floated,
            NodeData::Text(TextData {
                content: "x".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            ".container { width: 1024px; overflow: hidden; } \
             .floated { float: left; width: calc(50% - 20px); box-sizing: border-box; padding: 10px; border: 2px solid black; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let float_box = find_box(&root, floated).expect("floated box found");
        // calc(50% - 20px) against 1024px = 492px total border-box width.
        assert!(
            (float_box.width - 492.0).abs() < 1.0,
            "floated border-box width should be 492, got {}",
            float_box.width
        );
        // Content width should be 492 - 10 - 10 - 2 - 2 = 468.
        assert!(
            (float_box.content_width - 468.0).abs() < 1.0,
            "floated border-box content width should be 468, got {}",
            float_box.content_width
        );
    }

    #[test]
    fn test_box_sizing_inherit_propagates_border_box() {
        // Some CSS frameworks set `html { box-sizing: border-box; }` and use
        // `* { box-sizing: inherit; }` so percentage-width padded floats fit side
        // by side. Without honoring `box-sizing: inherit`, columns fall back to
        // content-box and overflow their row.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut row_el = ElementData::new("div");
        row_el
            .attributes
            .insert("class".to_string(), "row".to_string());
        let row = doc.add_node(body, NodeData::Element(row_el));

        let mut col1_el = ElementData::new("div");
        col1_el
            .attributes
            .insert("class".to_string(), "col".to_string());
        let col1 = doc.add_node(row, NodeData::Element(col1_el));
        let _ = doc.add_node(
            col1,
            NodeData::Text(TextData {
                content: "A".to_string(),
            }),
        );

        let mut col2_el = ElementData::new("div");
        col2_el
            .attributes
            .insert("class".to_string(), "col".to_string());
        let col2 = doc.add_node(row, NodeData::Element(col2_el));
        let _ = doc.add_node(
            col2,
            NodeData::Text(TextData {
                content: "B".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            "html { box-sizing: border-box; } \
             *, *:before, *:after { box-sizing: inherit; } \
             .row { width: 1024px; overflow: hidden; } \
             .col { float: left; width: 49.99999%; padding: 3%; background: #eee; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let col1_box = find_box(&root, col1).expect("col1 box found");
        let col2_box = find_box(&root, col2).expect("col2 box found");

        // With border-box, each column's total width is ~50% of 1024 = 512.
        // Padding is inside the border box, so the two columns should exactly
        // fill the row without overflowing.
        assert!(
            (col1_box.width - 512.0).abs() < 1.0,
            "col1 border-box width should be ~512, got {}",
            col1_box.width
        );
        assert!(
            (col2_box.width - 512.0).abs() < 1.0,
            "col2 border-box width should be ~512, got {}",
            col2_box.width
        );
        assert!(
            (col1_box.x + col1_box.width - col2_box.x).abs() < 1.0,
            "col2 should start where col1 ends: col1.x={} col1.w={} col2.x={}",
            col1_box.x,
            col1_box.width,
            col2_box.x
        );
        assert!(
            (col2_box.x + col2_box.width - 1024.0).abs() < 1.0,
            "row should not overflow: col2 ends at {}, expected 1024",
            col2_box.x + col2_box.width
        );
    }

    #[test]
    fn test_floated_inline_shrink_width_includes_word_space() {
        // A float whose content ends in a text run that starts with a
        // collapsible space (e.g. "<a>modules</a> |") must count that word
        // space in its shrink-to-fit width, or the pipe wraps to a second
        // line even though the float is wide enough.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let ul = doc.add_node(body, NodeData::Element(ElementData::new("ul")));
        let li = doc.add_node(ul, NodeData::Element(ElementData::new("li")));
        let a = doc.add_node(li, NodeData::Element(ElementData::new("a")));
        let _ = doc.add_node(
            a,
            NodeData::Text(TextData {
                content: "modules".to_string(),
            }),
        );
        let _ = doc.add_node(
            li,
            NodeData::Text(TextData {
                content: " |".to_string(),
            }),
        );

        let stylesheet =
            incognidium_css::parse_css("ul { list-style: none; } li { float: right; }");
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let li_box = find_box(&root, li).expect("li box found");
        assert!(
            li_box.height < 30.0,
            "pipe should stay on the first line inside the float, but the float is only {}px wide and {}px tall",
            li_box.width,
            li_box.height
        );
    }

    fn test_floated_columns_wrap_to_next_line() {
        // When more percentage-width floats fit per line than the container allows,
        // later floats must wrap to a new line instead of overflowing horizontally.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut row_el = ElementData::new("div");
        row_el
            .attributes
            .insert("class".to_string(), "row".to_string());
        let row = doc.add_node(body, NodeData::Element(row_el));

        let mut make_col = |class: &str, text: &str| {
            let mut col_el = ElementData::new("div");
            col_el
                .attributes
                .insert("class".to_string(), class.to_string());
            let col = doc.add_node(row, NodeData::Element(col_el));
            let _ = doc.add_node(
                col,
                NodeData::Text(TextData {
                    content: text.to_string(),
                }),
            );
            col
        };
        let col1 = make_col("col", "A");
        let col2 = make_col("col", "B");
        let col3 = make_col("col", "C");

        let stylesheet = incognidium_css::parse_css(
            ".row { width: 1024px; overflow: hidden; } \
             .col { float: left; width: 50%; padding: 10px; box-sizing: border-box; background: #eee; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let col1_box = find_box(&root, col1).expect("col1 box found");
        let col2_box = find_box(&root, col2).expect("col2 box found");
        let col3_box = find_box(&root, col3).expect("col3 box found");

        // First two 50% columns should fit side by side on the first line.
        assert!(
            col1_box.x.abs() < 1.0,
            "col1 should be at left edge, got x={}",
            col1_box.x
        );
        assert!(
            (col1_box.x + col1_box.width - col2_box.x).abs() < 1.0,
            "col2 should sit next to col1"
        );
        // Third column must wrap to the next line, not extend past the row.
        assert!(
            col3_box.y > col1_box.y + col1_box.height - 1.0,
            "col3 should be below col1/col2, got y={} col1.y={} col1.h={}",
            col3_box.y,
            col1_box.y,
            col1_box.height
        );
        assert!(
            col3_box.x.abs() < 1.0,
            "col3 should start at left edge of new line, got x={}",
            col3_box.x
        );
        assert!(
            (col3_box.width - 512.0).abs() < 1.0,
            "col3 should keep its 50% width after wrapping, got {}",
            col3_box.width
        );
        assert!(
            (col3_box.x + col3_box.width - 512.0).abs() < 1.0,
            "col3 should not overflow the row, got x={} width={}",
            col3_box.x,
            col3_box.width
        );
    }

    #[test]
    fn test_text_below_float_uses_full_width() {
        // Text that wraps around a short float should use the reduced line-box
        // width for lines beside the float, then expand to the full container
        // width for lines that fall below the float.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut wrap_el = ElementData::new("div");
        wrap_el
            .attributes
            .insert("class".to_string(), "wrap".to_string());
        let wrap = doc.add_node(body, NodeData::Element(wrap_el));

        let mut badge_el = ElementData::new("span");
        badge_el
            .attributes
            .insert("class".to_string(), "badge".to_string());
        let badge = doc.add_node(wrap, NodeData::Element(badge_el));
        let _ = doc.add_node(
            badge,
            NodeData::Text(TextData {
                content: "LIVE".to_string(),
            }),
        );

        let mut heading_el = ElementData::new("h2");
        heading_el
            .attributes
            .insert("class".to_string(), "headline".to_string());
        let heading = doc.add_node(wrap, NodeData::Element(heading_el));
        let _ = doc.add_node(
            heading,
            NodeData::Text(TextData {
                content: "A long headline that wraps onto multiple lines".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            "body { margin: 0; } \
             .wrap { width: 300px; } \
             .badge { float: left; background: red; color: white; padding: 4px 8px; \
                      margin-right: 8px; height: 24px; line-height: 24px; } \
             .headline { margin: 0; font-size: 20px; line-height: 28px; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let heading_box = find_box(&root, heading).expect("heading box found");
        // Collect the text fragments inside the h2.
        let text_fragments: Vec<_> = heading_box
            .children
            .iter()
            .filter(|c| c.box_type == BoxType::Text && c.text.is_some())
            .collect();

        // We expect at least two fragments: one beside the float and one below it.
        assert!(
            text_fragments.len() >= 2,
            "headline text should split into at least two fragments, got {}",
            text_fragments.len()
        );

        // The first fragment should sit beside the float (indented).
        let first = text_fragments.first().unwrap();
        assert!(
            first.x > 40.0,
            "first fragment should be beside the float, got x={}",
            first.x
        );

        // The last fragment should start at the left edge of the container,
        // using the full width now that the float has ended.
        let last = text_fragments.last().unwrap();
        assert!(
            last.x < 30.0,
            "last fragment should start near the left edge below the float, got x={}",
            last.x
        );

        // The last fragment should start below the float so it lays out in the
        // full container width instead of the shortened float-side sliver.
        assert!(
            last.y >= first.y + 24.0,
            "last fragment should start below the 24px float: first_y={} last_y={}",
            first.y,
            last.y
        );
    }

    #[test]
    fn test_floated_children_of_inline_box_lay_out_side_by_side() {
        // A floated child of an inline box is out of flow and must lay out
        // through the float-aware block path. The inline path has no float
        // placement and stacked the floated children vertically, one per line.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let list = doc.add_node(body, NodeData::Element(ElementData::new("ul")));
        let mut item_els = Vec::new();
        for label in ["One", "Two", "Three"] {
            let item = doc.add_node(list, NodeData::Element(ElementData::new("li")));
            let anchor = doc.add_node(item, NodeData::Element(ElementData::new("a")));
            let _ = doc.add_node(
                anchor,
                NodeData::Text(TextData {
                    content: label.to_string(),
                }),
            );
            item_els.push(item);
        }

        let stylesheet = incognidium_css::parse_css(
            "body { margin: 0; } \
             ul { display: inline; margin: 0; padding: 0; } \
             li { float: left; list-style-type: none; margin-right: 8px; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let items: Vec<&LayoutBox> = item_els
            .iter()
            .filter_map(|id| find_box(&root, *id))
            .collect();
        assert_eq!(items.len(), 3, "all three list items should have boxes");

        // The floated items should line up horizontally: each item starts to
        // the right of the previous one instead of stacking at the same x.
        assert!(
            items[0].x < items[1].x && items[1].x < items[2].x,
            "floated children of an inline box should float side by side, got x={} x={} x={}",
            items[0].x,
            items[1].x,
            items[2].x
        );
        // No item should span the full container width, which is the signature
        // of a float that never shrank to fit its content.
        for item in &items {
            assert!(
                item.width < 1024.0,
                "floated item should shrink-wrap to its content, got width={}",
                item.width
            );
        }
    }

    #[test]
    fn test_text_beside_inline_block_uses_remaining_first_line_width() {
        // Text that follows an inline-block badge on the same line should use
        // the remaining width for its first line and the full container width
        // for subsequent lines.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut wrap_el = ElementData::new("div");
        wrap_el
            .attributes
            .insert("class".to_string(), "wrap".to_string());
        let wrap = doc.add_node(body, NodeData::Element(wrap_el));

        let mut h2_el = ElementData::new("h2");
        h2_el
            .attributes
            .insert("class".to_string(), "headline".to_string());
        let h2 = doc.add_node(wrap, NodeData::Element(h2_el));

        let mut badge_el = ElementData::new("span");
        badge_el
            .attributes
            .insert("class".to_string(), "badge".to_string());
        let badge = doc.add_node(h2, NodeData::Element(badge_el));
        let _ = doc.add_node(
            badge,
            NodeData::Text(TextData {
                content: "LIVE".to_string(),
            }),
        );

        let _ = doc.add_node(
            h2,
            NodeData::Text(TextData {
                content: "A long headline that wraps onto multiple lines".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            "body { margin: 0; } \
             .wrap { width: 300px; } \
             .badge { display: inline-block; background: red; color: white; \
                      padding: 4px 8px; margin-right: 8px; height: 24px; line-height: 24px; } \
             .headline { margin: 0; font-size: 20px; line-height: 28px; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let h2_box = find_box(&root, h2).expect("h2 box found");
        let text_fragments: Vec<_> = h2_box
            .children
            .iter()
            .filter(|c| c.box_type == BoxType::Text && c.text.is_some())
            .collect();

        assert!(
            text_fragments.len() >= 2,
            "headline text should split into at least two fragments, got {}",
            text_fragments.len()
        );

        // The first fragment should sit beside the inline-block badge.
        let first = text_fragments.first().unwrap();
        assert!(
            first.x > 40.0,
            "first fragment should be beside the badge, got x={}",
            first.x
        );

        // The last fragment should start at the left edge of a new line and use
        // the full container width.
        let last = text_fragments.last().unwrap();
        assert!(
            last.x < 10.0,
            "last fragment should start near the left edge, got x={}",
            last.x
        );
        assert!(
            last.width > first.width + 10.0,
            "last fragment should be wider than first: first={} last={}",
            first.width,
            last.width
        );
    }

    #[test]
    fn test_split_fragments_keep_word_gaps_and_line_edges() {
        // When a wrapping text node is split at a line boundary, the placement
        // pass can put the fragments back on the same line (the line-width model
        // that decided the split disagrees with the final placement). The split
        // consumed the inter-word white space, so the fragments must still
        // render with a word gap, and a multi-line fragment must never keep its
        // first-line x for its wrapped lines: the wrapped lines must start at
        // the container's left edge.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut wrap_el = ElementData::new("div");
        wrap_el
            .attributes
            .insert("class".to_string(), "wrap".to_string());
        let wrap = doc.add_node(body, NodeData::Element(wrap_el));

        let mut box_el = ElementData::new("div");
        box_el
            .attributes
            .insert("class".to_string(), "box".to_string());
        let _box_node = doc.add_node(wrap, NodeData::Element(box_el));

        let para = doc.add_node(wrap, NodeData::Element(ElementData::new("p")));
        let anchor = doc.add_node(para, NodeData::Element(ElementData::new("a")));
        let _ = doc.add_node(
            anchor,
            NodeData::Text(TextData {
                content: "downloaded".to_string(),
            }),
        );
        let long_text = doc.add_node(
            para,
            NodeData::Text(TextData {
                content: " from the server. Once the materials have been fetched, the rendering engine converts the resources into an interactive visual representation of the document for the user to read".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            "body { margin: 0; } \
             .wrap { width: 620px; } \
             .box { float: right; width: 200px; height: 26px; background: #eee; } \
             p { margin: 0; font-size: 14px; line-height: 18px; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let para_box = find_box(&root, para).expect("paragraph box found");
        let mut fragments: Vec<&LayoutBox> = para_box
            .children
            .iter()
            .filter(|c| c.box_type == BoxType::Text && c.node_id == long_text)
            .collect();
        fragments.sort_by(|a, b| {
            a.y.partial_cmp(&b.y)
                .unwrap()
                .then(a.x.partial_cmp(&b.x).unwrap())
        });

        assert!(
            fragments.len() >= 2,
            "the wrapping text node should be split into fragments, got {}",
            fragments.len()
        );

        // A multi-line fragment must only ever start at the container's left
        // edge, so its wrapped lines line up with the paragraph text instead of
        // floating mid-line.
        for f in &fragments {
            if f.height > 19.0 {
                assert!(
                    f.x < 2.0,
                    "a multi-line fragment must start at the container's left edge, got x={} h={}",
                    f.x,
                    f.height
                );
            }
        }

        // Consecutive fragments of the same node that share a line must not
        // butt together: the split consumed a source space, so a word gap must
        // remain between them.
        for pair in fragments.windows(2) {
            let (prev, next) = (pair[0], pair[1]);
            if (next.y - prev.y).abs() < 1.0 && next.x >= prev.x {
                let gap = next.x - (prev.x + prev.width);
                assert!(
                    gap > 1.0,
                    "adjacent split fragments lost the inter-word gap: prev ends at {}, next starts at {}",
                    prev.x + prev.width,
                    next.x
                );
            }
        }
    }

    #[test]
    fn test_inline_replaced_element_gap_before_text() {
        // A source space between an inline replaced element (e.g. an SVG icon
        // rasterized to an image) and the following text should produce a normal
        // single-space gap, not make the text butt against the icon.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut wrap_el = ElementData::new("div");
        wrap_el
            .attributes
            .insert("class".to_string(), "wrap".to_string());
        let wrap = doc.add_node(body, NodeData::Element(wrap_el));

        let mut img_el = ElementData::new("img");
        img_el
            .attributes
            .insert("class".to_string(), "icon".to_string());
        img_el
            .attributes
            .insert("src".to_string(), "__placeholder__.png".to_string());
        let img = doc.add_node(wrap, NodeData::Element(img_el));

        let _ = doc.add_node(
            wrap,
            NodeData::Text(TextData {
                content: " Text after the icon".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            "body { margin: 0; } \
             .wrap { width: 400px; padding: 10px; font-size: 16px; } \
             .icon { display: inline; width: 16px; height: 16px; vertical-align: middle; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let wrap_box = find_box(&root, wrap).expect("wrap box found");
        let text_boxes: Vec<_> = wrap_box
            .children
            .iter()
            .filter(|b| b.box_type == BoxType::Text && b.text.is_some())
            .collect();

        let img_box = find_box(&root, img).expect("img box found");
        let text_box = text_boxes.first().expect("text box found");
        // The text must start after the image plus at least a single space.
        assert!(
            text_box.x >= img_box.x + img_box.width + 2.0,
            "text should be separated from image by a gap, got img at {} width {} text at {}",
            img_box.x,
            img_box.width,
            text_box.x
        );
    }

    #[test]
    fn test_adjacent_inline_replaced_no_extra_gap() {
        // When an inline replaced element and the following text are adjacent in
        // the source (no whitespace), there should be no automatic inter-word gap
        // between them.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut wrap_el = ElementData::new("div");
        wrap_el
            .attributes
            .insert("class".to_string(), "wrap".to_string());
        let wrap = doc.add_node(body, NodeData::Element(wrap_el));

        let mut img_el = ElementData::new("img");
        img_el
            .attributes
            .insert("class".to_string(), "icon".to_string());
        img_el
            .attributes
            .insert("src".to_string(), "__placeholder__.png".to_string());
        let img = doc.add_node(wrap, NodeData::Element(img_el));

        let _ = doc.add_node(
            wrap,
            NodeData::Text(TextData {
                content: "Text after the icon".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            "body { margin: 0; } \
             .wrap { width: 400px; padding: 10px; font-size: 16px; } \
             .icon { display: inline; width: 16px; height: 16px; vertical-align: middle; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let wrap_box = find_box(&root, wrap).expect("wrap box found");
        let text_boxes: Vec<_> = wrap_box
            .children
            .iter()
            .filter(|b| b.box_type == BoxType::Text && b.text.is_some())
            .collect();

        let img_box = find_box(&root, img).expect("img box found");
        let text_box = text_boxes.first().expect("text box found");
        // With no source whitespace, the text should butt directly against the image.
        assert!(
            text_box.x <= img_box.x + img_box.width + 1.0,
            "text should not be separated from image when no whitespace, got img at {} width {} text at {}",
            img_box.x,
            img_box.width,
            text_box.x
        );
    }

    #[test]
    fn test_absolute_calc_width_with_border_box() {
        // An absolutely positioned element with width: calc(...) and
        // box-sizing: border-box should evaluate the expression against its
        // containing block once and use the result as the total border-box width.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut parent_el = ElementData::new("div");
        parent_el
            .attributes
            .insert("class".to_string(), "parent".to_string());
        let parent = doc.add_node(body, NodeData::Element(parent_el));
        let mut child_el = ElementData::new("div");
        child_el
            .attributes
            .insert("class".to_string(), "child".to_string());
        let child = doc.add_node(parent, NodeData::Element(child_el));
        let _ = doc.add_node(
            child,
            NodeData::Text(TextData {
                content: "x".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            ".parent { position: relative; width: 600px; } \
             .child { position: absolute; left: 100px; width: calc(100% - 40px); box-sizing: border-box; padding: 10px; border: 2px solid black; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let child_box = find_box(&root, child).expect("absolute child box found");
        // Containing block is 600px; calc(100% - 40px) = 560px total border-box.
        assert!(
            (child_box.width - 560.0).abs() < 1.0,
            "absolute border-box width should be 560, got {}",
            child_box.width
        );
        // Content width should be 560 - 10 - 10 - 2 - 2 = 536.
        assert!(
            (child_box.content_width - 536.0).abs() < 1.0,
            "absolute border-box content width should be 536, got {}",
            child_box.content_width
        );
        // Left offset is 100px.
        assert!(
            (child_box.x - 100.0).abs() < 1.0,
            "absolute left offset should be 100, got {}",
            child_box.x
        );
    }

    #[test]
    fn test_flex_item_percent_max_width_resolves_against_container() {
        // A flex item with a percentage width and max-width must resolve both
        // against the flex container's content width, not against the resolved
        // basis. Previously the resolved basis was passed as the containing
        // width, so `max-width: 63%` clamped the item to 63% of 63% of the
        // container.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut container_el = ElementData::new("div");
        container_el
            .attributes
            .insert("class".to_string(), "container".to_string());
        let container = doc.add_node(body, NodeData::Element(container_el));
        let mut item_el = ElementData::new("div");
        item_el
            .attributes
            .insert("class".to_string(), "item".to_string());
        let item = doc.add_node(container, NodeData::Element(item_el));
        let _ = doc.add_node(
            item,
            NodeData::Text(TextData {
                content: "x".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            ".container { display: flex; width: 1000px; } \
             .item { width: 100%; max-width: 63%; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let item_box = find_box(&root, item).expect("flex item found");
        assert!(
            (item_box.width - 630.0).abs() < 1.0,
            "flex item width should be 630 (63% of 1000), got {}",
            item_box.width
        );
    }

    #[test]
    fn test_flex_container_math_function_widths() {
        // Flex containers must evaluate calc()/min()/max()/clamp() widths
        // (previously treated as auto, giving the container full width), and an
        // explicit width must still be clamped by a math-function max-width.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut min_el = ElementData::new("div");
        min_el
            .attributes
            .insert("class".to_string(), "minbox".to_string());
        let minbox = doc.add_node(body, NodeData::Element(min_el));
        let mut calc_el = ElementData::new("div");
        calc_el
            .attributes
            .insert("class".to_string(), "calcbox".to_string());
        let calcbox = doc.add_node(body, NodeData::Element(calc_el));
        let mut clamped_el = ElementData::new("div");
        clamped_el
            .attributes
            .insert("class".to_string(), "clamped".to_string());
        let clamped = doc.add_node(body, NodeData::Element(clamped_el));
        let _ = doc.add_node(
            minbox,
            NodeData::Text(TextData {
                content: "x".to_string(),
            }),
        );
        let _ = doc.add_node(
            calcbox,
            NodeData::Text(TextData {
                content: "x".to_string(),
            }),
        );
        let _ = doc.add_node(
            clamped,
            NodeData::Text(TextData {
                content: "x".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            ".minbox { display: flex; width: min(25vw, 350px); height: 10px; } \
             .calcbox { display: flex; width: calc(25vw); height: 10px; } \
             .clamped { display: flex; width: 600px; max-width: min(25vw, 350px); height: 10px; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        // 25vw at a 1024px viewport is 256px.
        for (node, expected) in [(minbox, 256.0), (calcbox, 256.0), (clamped, 256.0)] {
            let b = find_box(&root, node).expect("flex container found");
            assert!(
                (b.width - expected).abs() < 1.0,
                "flex container width should be {}, got {}",
                expected,
                b.width
            );
        }
    }

    #[test]
    fn test_flex_item_auto_width_percent_max_width_resolves_against_container() {
        // A row flex item with width:auto and a percentage max-width must resolve
        // the limit against the flex container, then lay out its contents at
        // that used width. Previously the item was re-laid out at its own
        // intrinsic width, so max-width:74% clamped it to 74% of itself and
        // forced text to wrap one character per line (e.g. percentage-clamped
        // section headings).
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut container_el = ElementData::new("div");
        container_el
            .attributes
            .insert("class".to_string(), "container".to_string());
        let container = doc.add_node(body, NodeData::Element(container_el));
        let mut item_el = ElementData::new("div");
        item_el
            .attributes
            .insert("class".to_string(), "item".to_string());
        let item = doc.add_node(container, NodeData::Element(item_el));
        let h2 = doc.add_node(item, NodeData::Element(ElementData::new("h2")));
        let _ = doc.add_node(
            h2,
            NodeData::Text(TextData {
                content: "News".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            ".container { display: flex; width: 1000px; justify-content: space-between; } \
             .item { max-width: 74%; } \
             h2 { font-size: 28px; margin: 0; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let item_box = find_box(&root, item).expect("flex item found");
        let h2_box = find_box(&root, h2).expect("h2 found");
        // The item's natural text width is well under 74% of 1000px, so it
        // should keep its intrinsic width and not be clamped to a fraction
        // of itself.
        assert!(
            item_box.width > 50.0 && item_box.width < 740.0,
            "flex item width should be intrinsic (50..740), got {}",
            item_box.width
        );
        // The h2 inside must be wide enough to hold the unbroken word.
        assert!(
            h2_box.width >= item_box.width - 1.0,
            "h2 should fill the flex item, h2={} item={}",
            h2_box.width,
            item_box.width
        );
    }

    #[test]
    fn test_expand_repeats_autofill_respects_container_and_gap() {
        // A dense grid using repeat(auto-fill, 34px) inside a 760px content area
        // with a 64px gutter. The old code expanded against a hard-coded 1024px
        // viewport and produced 30 tracks; layout-time expansion should produce 8.
        let tracks = vec![GridTrackSize::Repeat(
            RepeatCount::AutoFill,
            vec![GridTrackSize::Px(34.0)],
        )];
        let expanded = expand_repeats(&tracks, 760.0, 64.0, 16.0, 1024.0, 768.0);
        assert_eq!(expanded.len(), 8);
        assert!(expanded.iter().all(|t| *t == GridTrackSize::Px(34.0)));
    }

    #[test]
    fn test_expand_repeats_fixed_count() {
        let tracks = vec![GridTrackSize::Repeat(
            RepeatCount::Number(3),
            vec![GridTrackSize::Fr(1.0)],
        )];
        let expanded = expand_repeats(&tracks, 300.0, 0.0, 16.0, 1024.0, 768.0);
        assert_eq!(
            expanded,
            vec![
                GridTrackSize::Fr(1.0),
                GridTrackSize::Fr(1.0),
                GridTrackSize::Fr(1.0),
            ]
        );
    }

    #[test]
    fn test_grid_max_content_not_inflated_by_percent_width_spanned_item() {
        // A homepage collage uses `grid-template-columns: 1fr max-content 1fr 1fr`
        // with a heading/section wrapper spanning all four columns and `width: 100%`.
        // The old content-based measuring pass laid that spanned item out under a
        // 9999px constraint, so its percentage width resolved to ~9999px and the
        // per-track contribution inflated the `max-content` track to ~880px,
        // starving all `1fr` tracks to 0px. The fix measures intrinsic sizes
        // under a zero-width pass so percentage widths behave like auto.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut grid_el = ElementData::new("div");
        grid_el
            .attributes
            .insert("class".to_string(), "grid".to_string());
        let grid = doc.add_node(body, NodeData::Element(grid_el));

        let mut wide_el = ElementData::new("div");
        wide_el
            .attributes
            .insert("class".to_string(), "wide".to_string());
        let wide = doc.add_node(grid, NodeData::Element(wide_el));
        let _ = doc.add_node(
            wide,
            NodeData::Text(TextData {
                content: "Spanned heading".to_string(),
            }),
        );

        let mut center_el = ElementData::new("div");
        center_el
            .attributes
            .insert("class".to_string(), "center".to_string());
        let center = doc.add_node(grid, NodeData::Element(center_el));
        let _ = doc.add_node(
            center,
            NodeData::Text(TextData {
                content: "Center".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            ".grid { display: grid; width: 800px; grid-template-columns: 1fr max-content 1fr; \
             grid-template-areas: \"wide wide wide\"; gap: 0; } \
             .wide { grid-area: wide; width: 100%; } \
             .center { grid-column-start: 2; grid-row-start: 1; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let grid_box = find_box(&root, grid).expect("grid layout box found");
        let center_box = find_box(&root, center).expect("center layout box found");

        // Grid container content width is 800px (no padding/border).
        assert!(
            (grid_box.width - 800.0).abs() < 1.0,
            "grid width should be 800px: got {}",
            grid_box.width
        );

        // The max-content (center) track should be sized to its own text, not
        // to the full grid width. "Center" at the default 16px font is well
        // under 200px.
        assert!(
            center_box.width < 200.0,
            "max-content track should stay small: got {}",
            center_box.width
        );

        // Because the center track is small, the two 1fr tracks share most of
        // the width and the center box sits somewhere in the middle of the grid.
        assert!(
            center_box.x > 100.0 && center_box.x + center_box.width < 700.0,
            "center box should be in the middle of the grid: x={} w={}",
            center_box.x,
            center_box.width
        );
    }

    #[test]
    fn test_auto_flex_item_with_percent_width_grid_button_stays_content_sized() {
        // An icon-only search trigger is `display: grid; width: 100%` inside an
        // auto-width flex item. During the max-content measuring pass the
        // percentage resolved against the sentinel width, and reporting that
        // laid-out width as the item's intrinsic size inflated the item to the
        // whole line, pushing every following flex item off-screen. The
        // percentage is cyclic during intrinsic measurement: the item must size
        // to its content (the icon) instead.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let bar = doc.add_node(body, NodeData::Element(ElementData::new("nav")));
        let mut slot_el = ElementData::new("div");
        slot_el
            .attributes
            .insert("class".to_string(), "slot".to_string());
        let slot = doc.add_node(bar, NodeData::Element(slot_el));
        let button = doc.add_node(slot, NodeData::Element(ElementData::new("button")));
        let svg = doc.add_node(button, NodeData::Element(ElementData::new("svg")));

        let cta = doc.add_node(bar, NodeData::Element(ElementData::new("a")));
        let _ = doc.add_node(
            cta,
            NodeData::Text(TextData {
                content: "Sign in".to_string(),
            }),
        );
        let _ = doc.add_node(
            svg,
            NodeData::Text(TextData {
                content: "icon".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            "nav { display: flex; align-items: center; } \
             .slot { width: auto; } \
             button { display: grid; width: 100%; padding: 8px; } \
             svg { width: 16px; height: 16px; display: block; } \
             a { white-space: nowrap; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let slot_box = find_box(&root, slot).expect("slot found");
        let cta_box = find_box(&root, cta).expect("cta found");
        assert!(
            slot_box.width < 80.0,
            "auto flex item with a percent-width grid button should size to its icon, got {}",
            slot_box.width
        );
        assert!(
            cta_box.x + cta_box.width <= 1024.0,
            "following flex item should stay inside the viewport: x={} w={}",
            cta_box.x,
            cta_box.width
        );
    }

    #[test]
    fn test_flex_basis_calc_row_reverse() {
        // `flex: 0 0 calc(...)` must be evaluated and used as the item's main-axis
        // size, not treated as auto. Calc-based flex-basis values are commonly used
        // to size story art and content columns in article layouts.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut container_el = ElementData::new("div");
        container_el
            .attributes
            .insert("class".to_string(), "container".to_string());
        let container = doc.add_node(body, NodeData::Element(container_el));

        let mut art_el = ElementData::new("div");
        art_el
            .attributes
            .insert("class".to_string(), "art".to_string());
        let art = doc.add_node(container, NodeData::Element(art_el));
        let _ = doc.add_node(
            art,
            NodeData::Text(TextData {
                content: "Art".to_string(),
            }),
        );

        let mut text_el = ElementData::new("div");
        text_el
            .attributes
            .insert("class".to_string(), "text".to_string());
        let text = doc.add_node(container, NodeData::Element(text_el));
        let _ = doc.add_node(
            text,
            NodeData::Text(TextData {
                content: "Text".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            ".container { display: flex; flex-flow: row-reverse nowrap; width: 1000px; gap: 0; } \
             .art { flex: 0 0 calc(600px); } \
             .text { flex: 0 0 calc(400px); }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1000.0, 800.0);
        let root = layout(&doc, &styles, 1000.0, 800.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let art_box = find_box(&root, art).expect("art flex item found");
        let text_box = find_box(&root, text).expect("text flex item found");
        let container_box = find_box(&root, container).expect("container found");

        assert!(
            (art_box.width - 600.0).abs() < 1.0,
            "art width should be 600 (calc basis), got {}",
            art_box.width
        );
        assert!(
            (text_box.width - 400.0).abs() < 1.0,
            "text width should be 400 (calc basis), got {}",
            text_box.width
        );
        // Row-reverse: the first source child (art) is placed at the right edge.
        assert!(
            (art_box.x + art_box.width - 1000.0).abs() < 1.0,
            "art should end at the right edge of the container, got x={} w={}",
            art_box.x,
            art_box.width
        );
        assert!(
            text_box.x.abs() < 1.0,
            "text should start at the left edge, got x={}",
            text_box.x
        );
        // Everything on a single line: container height should be roughly one text line.
        assert!(
            container_box.height < 100.0,
            "container should be a single flex line, got height={}",
            container_box.height
        );
    }

    #[test]
    fn test_flex_basis_calc_resolves_vw_against_viewport() {
        // `calc(Nvw)` inside a nested flex-basis must resolve against the real
        // viewport, not the flex container's content width. A 500px-wide nested
        // container at a 1000px viewport should make `calc(50vw)` evaluate to 500px.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut wrapper_el = ElementData::new("div");
        wrapper_el
            .attributes
            .insert("class".to_string(), "wrapper".to_string());
        let wrapper = doc.add_node(body, NodeData::Element(wrapper_el));
        let mut container_el = ElementData::new("div");
        container_el
            .attributes
            .insert("class".to_string(), "container".to_string());
        let container = doc.add_node(wrapper, NodeData::Element(container_el));
        let mut item_el = ElementData::new("div");
        item_el
            .attributes
            .insert("class".to_string(), "item".to_string());
        let item = doc.add_node(container, NodeData::Element(item_el));
        let _ = doc.add_node(
            item,
            NodeData::Text(TextData {
                content: "x".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            ".wrapper { width: 500px; } \
             .container { display: flex; } \
             .item { flex: 0 0 calc(50vw); }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1000.0, 800.0);
        let root = layout(&doc, &styles, 1000.0, 800.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let item_box = find_box(&root, item).expect("flex item found");
        assert!(
            (item_box.width - 500.0).abs() < 1.0,
            "item width should be 500 (50vw of 1000px viewport), got {}",
            item_box.width
        );
    }

    #[test]
    fn test_grid_equal_fr_tracks_span_half_width() {
        // A common 12-column grid uses `grid-template-columns: repeat(12, minmax(0, 5fr))`
        // and a six-track span uses `grid-column: 1 / 7`. The six spanned tracks
        // must receive half the grid width; this test isolates the grid sizing.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut grid_el = ElementData::new("main");
        grid_el.attributes.insert(
            "class".to_string(),
            "container__inner three-col-layout__inner".to_string(),
        );
        let grid = doc.add_node(body, NodeData::Element(grid_el));

        let mut hero_el = ElementData::new("article");
        hero_el
            .attributes
            .insert("class".to_string(), "top-story-article".to_string());
        let hero = doc.add_node(grid, NodeData::Element(hero_el));
        let _ = doc.add_node(
            hero,
            NodeData::Text(TextData {
                content: "Hero headline text".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            "*{box-sizing:border-box} \
             .container__inner { width: 100%; display: grid; align-items: start; grid-gap: 0; \
             grid-template-columns: repeat(12, minmax(0, 5fr)); padding: 0 15px; } \
             .top-story-article { grid-row: 1 / -1; grid-column: 1 / 7; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let grid_box = find_box(&root, grid).expect("grid layout box found");
        let hero_box = find_box(&root, hero).expect("hero layout box found");

        // Content width = 1024 - 2*15 = 994.
        assert!(
            (grid_box.width - 1024.0).abs() < 1.0,
            "grid width should be 1024px: got {}",
            grid_box.width
        );
        // Hero spans six of twelve equal fr tracks -> half the content width.
        assert!(
            (hero_box.width - 497.0).abs() < 15.0,
            "hero should span half the content width: got {} expected ~497",
            hero_box.width
        );
    }

    #[test]
    fn test_grid_auto_columns_stretch_to_fill_container() {
        // Auto-sized tracks must stretch equally to fill a definite-size grid
        // container instead of staying at their content width, which left the
        // items clustered in the container's corner.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut grid_el = ElementData::new("div");
        grid_el
            .attributes
            .insert("class".to_string(), "c".to_string());
        let grid = doc.add_node(body, NodeData::Element(grid_el));
        let mut items = Vec::new();
        for label in ["1", "2", "3"] {
            let item = doc.add_node(grid, NodeData::Element(ElementData::new("div")));
            let _ = doc.add_node(
                item,
                NodeData::Text(TextData {
                    content: label.to_string(),
                }),
            );
            items.push(item);
        }

        let stylesheet = incognidium_css::parse_css(
            "body { margin: 0; } .c { width: 900px; display: grid; gap: 0; \
             grid-template-columns: auto auto auto; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        for (i, &item) in items.iter().enumerate() {
            let box_ = find_box(&root, item).expect("item box found");
            assert!(
                (box_.width - 300.0).abs() < 1.0,
                "auto column {i} should stretch to 300px: got {}",
                box_.width
            );
        }
    }

    #[test]
    fn test_grid_stretch_passes_definite_height_to_percent_child() {
        // A hero grid item wrapper stretched to the cell height, with a card
        // inside set to height: 100%. Without re-laying out the stretched item, the
        // card resolved its percentage height against an indefinite containing block
        // and stayed at its intrinsic size.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));

        let mut grid_el = ElementData::new("div");
        grid_el
            .attributes
            .insert("class".to_string(), "grid".to_string());
        let grid = doc.add_node(body, NodeData::Element(grid_el));

        let mut tall_el = ElementData::new("div");
        tall_el
            .attributes
            .insert("class".to_string(), "tall".to_string());
        let tall = doc.add_node(grid, NodeData::Element(tall_el));
        let _ = doc.add_node(
            tall,
            NodeData::Text(TextData {
                content: "Tall".to_string(),
            }),
        );

        let mut wrap_el = ElementData::new("div");
        wrap_el
            .attributes
            .insert("class".to_string(), "wrap".to_string());
        let wrap = doc.add_node(grid, NodeData::Element(wrap_el));

        let mut fill_el = ElementData::new("div");
        fill_el
            .attributes
            .insert("class".to_string(), "fill".to_string());
        let fill = doc.add_node(wrap, NodeData::Element(fill_el));
        let _ = doc.add_node(
            fill,
            NodeData::Text(TextData {
                content: "Fill".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            "* { box-sizing: border-box; margin: 0; } \
             .grid { display: grid; width: 200px; grid-template-columns: 100px 100px; } \
             .tall { height: 100px; } \
             .fill { height: 100%; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let wrap_box = find_box(&root, wrap).expect("wrap layout box found");
        let fill_box = find_box(&root, fill).expect("fill layout box found");

        assert!(
            (wrap_box.height - 100.0).abs() < 1.0,
            "stretched grid item wrapper should fill 100px cell: got {}",
            wrap_box.height
        );
        assert!(
            (fill_box.height - 100.0).abs() < 1.0,
            "percentage-height child should fill stretched wrapper: got {}",
            fill_box.height
        );
    }

    #[test]
    fn test_auto_table_columns_ignore_zero_width_spacer_cells() {
        // Nested-table comment rows use a spacer <img width="0"> in the indent
        // cell, a vote cell, and a text cell. The old intrinsic width measuring
        // pass laid that spacer cell out at 10000px and fell back to the cell
        // width, so the indent column stole almost the entire table width and
        // collapsed the comment text to a narrow strip.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));

        let mut outer_table_el = ElementData::new("table");
        outer_table_el
            .attributes
            .insert("width".to_string(), "85%".to_string());
        let outer_table = doc.add_node(body, NodeData::Element(outer_table_el));
        let outer_tbody = doc.add_node(outer_table, NodeData::Element(ElementData::new("tbody")));
        let outer_tr = doc.add_node(outer_tbody, NodeData::Element(ElementData::new("tr")));
        let outer_td = doc.add_node(outer_tr, NodeData::Element(ElementData::new("td")));

        let mut comment_tree_el = ElementData::new("table");
        comment_tree_el
            .attributes
            .insert("class".to_string(), "comment-tree".to_string());
        let comment_tree = doc.add_node(outer_td, NodeData::Element(comment_tree_el));
        let ct_body = doc.add_node(comment_tree, NodeData::Element(ElementData::new("tbody")));

        let mut add_row = |doc: &mut Document, indent: u32, text: &str| {
            let tr = doc.add_node(ct_body, NodeData::Element(ElementData::new("tr")));
            let td = doc.add_node(tr, NodeData::Element(ElementData::new("td")));
            let inner_table = doc.add_node(td, NodeData::Element(ElementData::new("table")));
            let inner_tbody =
                doc.add_node(inner_table, NodeData::Element(ElementData::new("tbody")));
            let inner_tr = doc.add_node(inner_tbody, NodeData::Element(ElementData::new("tr")));

            let mut ind_td_el = ElementData::new("td");
            ind_td_el
                .attributes
                .insert("class".to_string(), "ind".to_string());
            let ind_td = doc.add_node(inner_tr, NodeData::Element(ind_td_el));
            let mut img_el = ElementData::new("img");
            img_el
                .attributes
                .insert("src".to_string(), "s.gif".to_string());
            img_el
                .attributes
                .insert("width".to_string(), indent.to_string());
            img_el
                .attributes
                .insert("height".to_string(), "1".to_string());
            let _img = doc.add_node(ind_td, NodeData::Element(img_el));

            let mut vote_td_el = ElementData::new("td");
            vote_td_el
                .attributes
                .insert("class".to_string(), "votelinks".to_string());
            let vote_td = doc.add_node(inner_tr, NodeData::Element(vote_td_el));
            let _vote = doc.add_node(vote_td, NodeData::Element(ElementData::new("div")));

            let mut default_td_el = ElementData::new("td");
            default_td_el
                .attributes
                .insert("class".to_string(), "default".to_string());
            let default_td = doc.add_node(inner_tr, NodeData::Element(default_td_el));
            let _txt = doc.add_node(
                default_td,
                NodeData::Text(TextData {
                    content: text.to_string(),
                }),
            );
            default_td
        };

        let top_default = add_row(&mut doc, 0, "Top level comment text here.");
        let _reply_default = add_row(&mut doc, 40, "Reply comment text.");

        let stylesheet = incognidium_css::parse_css(
            "table.comment-tree { width: 100%; } \
             .votelinks { width: 30px; } \
             .ind img { display: block; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let default_box = find_box(&root, top_default).expect("default cell found");
        // The comment text cell must receive most of the table width.
        assert!(
            default_box.width > 300.0,
            "comment column should be wide, got {}px",
            default_box.width
        );

        // The indent column must not blow up to the 10000px measuring width.
        let ct_body_box = find_box(&root, comment_tree)
            .expect("comment-tree table")
            .children
            .first()
            .expect("tbody");
        let top_row = ct_body_box.children.first().expect("first comment row");
        let top_td = top_row.children.first().expect("row td");
        let inner_table = top_td.children.first().expect("inner table");
        let inner_tbody = inner_table.children.first().expect("inner tbody");
        let inner_row = inner_tbody.children.first().expect("inner row");
        let ind_cell = inner_row.children.first().expect("indent cell");
        assert!(
            ind_cell.width < 200.0,
            "indent column must stay small, got {}px",
            ind_cell.width
        );
    }

    #[test]
    fn test_auto_width_derives_from_definite_height_and_aspect_ratio() {
        // A media card wrapper with `height: 75px` and `aspect-ratio: 1`. When
        // width is auto, the box should size to the ratio-derived width (75px)
        // instead of measuring its child image.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));

        let mut card_el = ElementData::new("div");
        card_el
            .attributes
            .insert("class".to_string(), "card".to_string());
        let card = doc.add_node(body, NodeData::Element(card_el));

        let mut img_el = ElementData::new("img");
        img_el
            .attributes
            .insert("width".to_string(), "288".to_string());
        img_el
            .attributes
            .insert("height".to_string(), "288".to_string());
        let _img = doc.add_node(card, NodeData::Element(img_el));

        let stylesheet = incognidium_css::parse_css(
            "* { margin: 0; padding: 0; border: none; box-sizing: border-box; } \
             .card { height: 75px; aspect-ratio: 1 / 1; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let card_box = find_box(&root, card).expect("card layout box found");
        assert!(
            (card_box.width - 75.0).abs() < 1.0,
            "auto-width box with height:75px and aspect-ratio:1 should be 75px wide, got {}",
            card_box.width
        );
    }

    #[test]
    fn test_inline_block_shrink_to_fit_relayouts_children_when_clamped() {
        // Regression for article-style image figures: an auto-width inline-block
        // with `max-width: 100%` that shrinks to fit a wide image must re-layout
        // that image inside the clamped width.  Otherwise `width: 100%` on the
        // image is resolved during the max-content measure pass and the image
        // overflows its figure container.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));

        let mut figure_el = ElementData::new("figure");
        figure_el
            .attributes
            .insert("class".to_string(), "thumb".to_string());
        let figure = doc.add_node(body, NodeData::Element(figure_el));

        let mut img_el = ElementData::new("img");
        img_el
            .attributes
            .insert("src".to_string(), "big.png".to_string());
        img_el
            .attributes
            .insert("alt".to_string(), "photo".to_string());
        let img = doc.add_node(figure, NodeData::Element(img_el));

        let stylesheet = incognidium_css::parse_css(
            "body { margin: 0; width: 600px; } \
             .thumb { display: inline-block; max-width: 100%; } \
             img { width: 100%; display: block; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);

        let mut image_sizes = ImageSizes::new();
        image_sizes.insert("big.png".to_string(), (1200, 600));
        let root = layout_with_images(&doc, &styles, 1024.0, 768.0, &image_sizes);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let figure_box = find_box(&root, figure).expect("figure layout box found");
        let img_box = find_box(&root, img).expect("image layout box found");

        assert!(
            figure_box.width <= 600.0 + 0.5,
            "inline-block should be clamped to containing width, got {}",
            figure_box.width
        );
        assert!(
            img_box.width <= figure_box.width + 0.5,
            "image with width:100% should fit inside clamped inline-block, got {} vs figure {}",
            img_box.width,
            figure_box.width
        );
        assert!(
            img_box.width < 1000.0,
            "image should not keep its intrinsic width after clamping, got {}",
            img_box.width
        );
    }

    #[test]
    fn test_replaced_element_percentage_max_height_resolves_against_height_axis() {
        // Percentage max-height and min-height on replaced elements must resolve
        // against the containing block height, not its width. Otherwise a wide
        // container makes a percentage height constraint far larger than intended
        // and the image overflows its intended bounds.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));

        let mut container_el = ElementData::new("div");
        container_el
            .attributes
            .insert("class".to_string(), "container".to_string());
        let container = doc.add_node(body, NodeData::Element(container_el));

        let mut img_el = ElementData::new("img");
        img_el
            .attributes
            .insert("src".to_string(), "red.png".to_string());
        let img = doc.add_node(container, NodeData::Element(img_el));

        let stylesheet = incognidium_css::parse_css(
            "body { margin: 0; } \
             .container { width: 120px; height: 600px; } \
             img { max-width: none; max-height: 25%; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);

        let mut image_sizes = ImageSizes::new();
        image_sizes.insert("red.png".to_string(), (400, 200));
        let root = layout_with_images(&doc, &styles, 1024.0, 768.0, &image_sizes);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let img_box = find_box(&root, img).expect("image layout box found");

        assert!(
            (img_box.height - 150.0).abs() < 1.0,
            "image height should be clamped to 25% of 600px container = 150px, got {}",
            img_box.height
        );
    }

    #[test]
    fn test_empty_element_with_min_height_is_not_collapsed() {
        // Empty block-level and flex containers with `min-height` (or
        // `min-width`) must still produce a layout box. Otherwise an empty ad
        // placeholder or hero skeleton collapses to nothing and the following
        // content shifts upward, breaking the intended page rhythm.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));

        let mut app_el = ElementData::new("div");
        app_el
            .attributes
            .insert("class".to_string(), "app".to_string());
        let app = doc.add_node(body, NodeData::Element(app_el));

        let mut banner_el = ElementData::new("div");
        banner_el
            .attributes
            .insert("class".to_string(), "banner".to_string());
        let banner = doc.add_node(app, NodeData::Element(banner_el));

        let mut ad_el = ElementData::new("div");
        ad_el
            .attributes
            .insert("class".to_string(), "ad".to_string());
        let ad = doc.add_node(banner, NodeData::Element(ad_el));

        let stylesheet = incognidium_css::parse_css(
            "body { margin: 0; } \
             .app { display: flex; flex-direction: column; } \
             .banner { display: flex; justify-content: center; background: #f6f6f6; } \
             .ad { min-height: 250px; min-width: 728px; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let ad_box = find_box(&root, ad).expect("ad layout box found");
        assert!(
            (ad_box.height - 250.0).abs() < 1.0,
            "empty ad placeholder should keep min-height of 250px, got {}",
            ad_box.height
        );

        let banner_box = find_box(&root, banner).expect("banner layout box found");
        assert!(
            (banner_box.height - 250.0).abs() < 1.0,
            "banner should be sized by its empty min-height child, got {}",
            banner_box.height
        );
    }

    #[test]
    fn test_img_with_srcset_but_no_src_gets_a_layout_box() {
        // Responsive images often omit a legacy `src` and rely entirely on
        // `srcset`. Without a usable image URL the box collapses to nothing,
        // leaving empty placeholders and breaking grid/flex article layouts.
        // The layout engine should derive an image source from the first srcset
        // candidate so it can be fetched and measured.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));

        let mut img_el = ElementData::new("img");
        img_el.attributes.insert(
            "srcset".to_string(),
            "hero-400.jpg 400w, hero-800.jpg 800w".to_string(),
        );
        img_el
            .attributes
            .insert("width".to_string(), "200".to_string());
        img_el
            .attributes
            .insert("height".to_string(), "100".to_string());
        img_el
            .attributes
            .insert("alt".to_string(), "hero".to_string());
        let img = doc.add_node(body, NodeData::Element(img_el));

        let stylesheet = incognidium_css::parse_css("body { margin: 0; }");
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);

        let mut image_sizes = ImageSizes::new();
        image_sizes.insert("hero-400.jpg".to_string(), (400, 200));
        let root = layout_with_images(&doc, &styles, 1024.0, 768.0, &image_sizes);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let img_box = find_box(&root, img).expect("image layout box found");
        assert_eq!(
            img_box.image_src.as_deref(),
            Some("hero-400.jpg"),
            "srcset-only image should use the first candidate as its source"
        );
        assert!(
            (img_box.width - 200.0).abs() < 1.0,
            "srcset-only image should keep its explicit width, got {}",
            img_box.width
        );
        assert!(
            (img_box.height - 100.0).abs() < 1.0,
            "srcset-only image should keep its explicit height, got {}",
            img_box.height
        );
    }

    #[test]
    fn test_picture_img_without_src_uses_first_source_srcset() {
        // Responsive `<picture>` elements often ship an `<img>` with no
        // `src` attribute, relying on a `<source srcset>` to supply the image.
        // The layout engine must fall back to that source candidate so the image
        // is sized and painted instead of collapsing to zero.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));

        let mut picture_el = ElementData::new("picture");
        picture_el
            .attributes
            .insert("class".to_string(), "hero".to_string());
        let picture = doc.add_node(body, NodeData::Element(picture_el));

        let mut source_el = ElementData::new("source");
        source_el.attributes.insert(
            "srcset".to_string(),
            "hero-400.jpg 400w, hero-800.jpg 2x".to_string(),
        );
        let _source = doc.add_node(picture, NodeData::Element(source_el));

        let mut img_el = ElementData::new("img");
        img_el
            .attributes
            .insert("width".to_string(), "200".to_string());
        img_el
            .attributes
            .insert("height".to_string(), "100".to_string());
        img_el
            .attributes
            .insert("alt".to_string(), "hero".to_string());
        let img = doc.add_node(picture, NodeData::Element(img_el));

        let stylesheet = incognidium_css::parse_css("body { margin: 0; }");
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);

        let mut image_sizes = ImageSizes::new();
        image_sizes.insert("hero-400.jpg".to_string(), (400, 200));
        let root = layout_with_images(&doc, &styles, 1024.0, 768.0, &image_sizes);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let img_box = find_box(&root, img).expect("image layout box found");
        assert_eq!(
            img_box.image_src.as_deref(),
            Some("hero-400.jpg"),
            "picture img without src should use the first source srcset candidate"
        );
        assert!(
            (img_box.width - 200.0).abs() < 1.0,
            "picture image should keep its explicit width, got {}",
            img_box.width
        );
        assert!(
            (img_box.height - 100.0).abs() < 1.0,
            "picture image should keep its explicit height, got {}",
            img_box.height
        );
    }

    #[test]
    fn test_definite_width_block_beside_float_wraps_text_in_its_own_column() {
        // Regression: a definite-width block (e.g.
        // `width: 50%`) placed beside a wide right-floated image must wrap its
        // inline text within its own content box.  Previously the float's width
        // was subtracted from the tiny available inline width, leaving <= 1 px,
        // which triggered the text-layout "nowrap on tiny widths" guard and
        // produced a single overflowing line.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));

        let mut wrap_el = ElementData::new("div");
        wrap_el
            .attributes
            .insert("class".to_string(), "wrap".to_string());
        let wrap = doc.add_node(body, NodeData::Element(wrap_el));

        let mut figure_el = ElementData::new("figure");
        figure_el
            .attributes
            .insert("class".to_string(), "thumb".to_string());
        let figure = doc.add_node(wrap, NodeData::Element(figure_el));

        let mut img_el = ElementData::new("img");
        img_el
            .attributes
            .insert("src".to_string(), "pic.png".to_string());
        img_el
            .attributes
            .insert("width".to_string(), "400".to_string());
        img_el
            .attributes
            .insert("height".to_string(), "200".to_string());
        let _img = doc.add_node(figure, NodeData::Element(img_el));

        let mut story_el = ElementData::new("div");
        story_el
            .attributes
            .insert("class".to_string(), "story".to_string());
        let story = doc.add_node(wrap, NodeData::Element(story_el));

        let mut h3_el = ElementData::new("h3");
        h3_el
            .attributes
            .insert("class".to_string(), "title".to_string());
        let h3 = doc.add_node(story, NodeData::Element(h3_el));
        let _h3_text = doc.add_node(
            h3,
            NodeData::Text(TextData {
                content: "This is a deliberately long generic headline used to verify that a narrow story column wraps text across multiple lines instead of overflowing".to_string(),
            }),
        );

        let mut p_el = ElementData::new("p");
        p_el.attributes
            .insert("class".to_string(), "teaser".to_string());
        let p = doc.add_node(story, NodeData::Element(p_el));
        let _p_text = doc.add_node(
            p,
            NodeData::Text(TextData {
                content: "A sample teaser paragraph used to verify that a floated thumbnail beside a text block leaves the story column wide enough to read.".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            "* { margin: 0; padding: 0; border: none; box-sizing: border-box; } \
             body { width: 994px; } \
             .wrap { width: 100%; } \
             .thumb { float: right; width: 50%; } \
             .story { width: 50%; padding: 20px; } \
             .title { font-size: 16px; } \
             .teaser { font-size: 16px; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let story_box = find_box(&root, story).expect("story layout box");
        let h3_box = find_box(&root, h3).expect("h3 layout box");
        let p_box = find_box(&root, p).expect("p layout box");

        let h3_text = h3_box
            .children
            .iter()
            .find(|c| c.box_type == BoxType::Text)
            .expect("h3 text box");
        let p_text = p_box
            .children
            .iter()
            .find(|c| c.box_type == BoxType::Text)
            .expect("p text box");

        assert!(
            h3_text.width <= h3_box.width + 1.0,
            "h3 text must wrap inside the h3 box ({} vs {})",
            h3_text.width,
            h3_box.width
        );
        assert!(
            p_text.width <= p_box.width + 1.0,
            "p text must wrap inside the p box ({} vs {})",
            p_text.width,
            p_box.width
        );
        assert!(
            story_box.width <= 497.0 + 1.0,
            "story column should be about half the body width, got {}",
            story_box.width
        );
        assert!(
            h3_text.height > 30.0,
            "h3 text should span multiple wrapped lines, got height {}",
            h3_text.height
        );
    }

    #[test]
    fn test_inline_block_siblings_separated_by_whitespace_get_a_space_gap() {
        // Regression: inline-block items with
        // whitespace-only text nodes between them should render with a single
        // inter-word space, not concatenated.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));

        let mut ul_el = ElementData::new("ul");
        ul_el
            .attributes
            .insert("class".to_string(), "menu".to_string());
        let ul = doc.add_node(body, NodeData::Element(ul_el));

        let make_li = |doc: &mut Document, parent, label: &str| {
            let mut li_el = ElementData::new("li");
            li_el
                .attributes
                .insert("class".to_string(), "item".to_string());
            let li = doc.add_node(parent, NodeData::Element(li_el));
            let mut span_el = ElementData::new("span");
            span_el
                .attributes
                .insert("class".to_string(), "label".to_string());
            let span = doc.add_node(li, NodeData::Element(span_el));
            doc.add_node(
                span,
                NodeData::Text(TextData {
                    content: label.to_string(),
                }),
            );
            li
        };

        let li1 = make_li(&mut doc, ul, "U.S.");
        let _ws1 = doc.add_node(
            ul,
            NodeData::Text(TextData {
                content: "\n  ".to_string(),
            }),
        );
        let li2 = make_li(&mut doc, ul, "Intl");
        let _ws2 = doc.add_node(
            ul,
            NodeData::Text(TextData {
                content: "\n  ".to_string(),
            }),
        );
        let li3 = make_li(&mut doc, ul, "Can");

        let stylesheet = incognidium_css::parse_css(
            "* { margin: 0; padding: 0; border: none; box-sizing: border-box; } \
             body { width: 1024px; } \
             ul { list-style: none; } \
             li { display: inline-block; } \
             span { font-size: 16px; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let b1 = find_box(&root, li1).expect("li1 box");
        let b2 = find_box(&root, li2).expect("li2 box");
        let b3 = find_box(&root, li3).expect("li3 box");

        let space_width =
            measure_text_width(" ", 16.0, &incognidium_style::ComputedStyle::default());
        assert!(
            b2.x >= b1.x + b1.width + space_width * 0.5,
            "li2 should start after li1 plus a space gap: li1 ends at {} but li2 starts at {}",
            b1.x + b1.width,
            b2.x
        );
        assert!(
            b3.x >= b2.x + b2.width + space_width * 0.5,
            "li3 should start after li2 plus a space gap: li2 ends at {} but li3 starts at {}",
            b2.x + b2.width,
            b3.x
        );
    }

    #[test]
    fn test_text_transform_inflates_layout_width() {
        // Regression: text-transform: uppercase must be
        // accounted for when measuring text, or the rendered glyphs overflow.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));

        let mut span_el = ElementData::new("span");
        span_el
            .attributes
            .insert("class".to_string(), "label".to_string());
        let span = doc.add_node(body, NodeData::Element(span_el));
        doc.add_node(
            span,
            NodeData::Text(TextData {
                content: "International".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            "* { margin: 0; padding: 0; border: none; box-sizing: border-box; } \
             body { width: 1024px; } \
             span { display: inline-block; font-size: 16px; text-transform: uppercase; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let span_box = find_box(&root, span).expect("span layout box");
        let upper_width = measure_text_width(
            "INTERNATIONAL",
            16.0,
            &incognidium_style::ComputedStyle::default(),
        );
        let lower_width = measure_text_width(
            "International",
            16.0,
            &incognidium_style::ComputedStyle::default(),
        );
        assert!(
            upper_width > lower_width,
            "uppercase measurement should be wider than mixed-case"
        );
        assert!(
            span_box.width >= upper_width - 1.0,
            "inline-block width should match uppercase text width ({} vs {})",
            span_box.width,
            upper_width
        );
    }

    #[test]
    fn test_inline_block_float_grid_rows_do_not_overlap() {
        // Regression: explicit-width inline-blocks containing left-floated grid
        // items must use block formatting context layout so floats wrap into rows
        // and the container encloses every row. Previously the children were
        // stacked vertically, which broke multi-column float grids.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));

        let mut container_el = ElementData::new("div");
        container_el
            .attributes
            .insert("class".to_string(), "grid".to_string());
        let container = doc.add_node(body, NodeData::Element(container_el));

        let mut items = Vec::new();
        for i in 0..6 {
            let mut el = ElementData::new("div");
            el.attributes
                .insert("class".to_string(), format!("item item{}", i + 1));
            let node = doc.add_node(container, NodeData::Element(el));
            doc.add_node(
                node,
                NodeData::Text(TextData {
                    content: format!("Item {}", i + 1),
                }),
            );
            items.push(node);
        }

        let stylesheet = incognidium_css::parse_css(
            "* { margin: 0; padding: 0; border: none; box-sizing: border-box; } \
             body { width: 1024px; } \
             .grid { display: inline-block; width: 600px; } \
             .item { float: left; width: 33%; height: 80px; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let container_box = find_box(&root, container).expect("container layout box");
        // Three items fit across 600px (33% ≈ 198px each, 3 × 198 = 594).
        // The container must be tall enough for two rows of 80px.
        assert!(
            container_box.height >= 160.0,
            "inline-block container should enclose two float rows: got height {}",
            container_box.height
        );

        // First row y positions should be identical; second row should be 80px below.
        let row0_y = find_box(&root, items[0]).unwrap().y;
        for i in 1..3 {
            let y = find_box(&root, items[i]).unwrap().y;
            assert!(
                (y - row0_y).abs() < 1.0,
                "first-row item {} should share baseline y: got {}",
                i + 1,
                y
            );
        }
        let row1_y = find_box(&root, items[3]).unwrap().y;
        assert!(
            row1_y >= row0_y + 80.0 - 1.0,
            "second row should start below first row ({} >= {} + 80)",
            row1_y,
            row0_y
        );
    }

    #[test]
    fn test_inline_block_float_grid_with_rem_height_items() {
        // Regression for a common responsive portal pattern: an inline-block
        // container holding left-floated items, each with a fixed rem height and
        // an inline-block link wrapper containing title/tagline text.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));

        let mut container_el = ElementData::new("div");
        container_el
            .attributes
            .insert("class".to_string(), "projects".to_string());
        let container = doc.add_node(body, NodeData::Element(container_el));

        let mut items = Vec::new();
        for i in 0..6 {
            let mut item_el = ElementData::new("div");
            item_el
                .attributes
                .insert("class".to_string(), "project".to_string());
            let item = doc.add_node(container, NodeData::Element(item_el));

            let mut link_el = ElementData::new("a");
            link_el
                .attributes
                .insert("class".to_string(), "project-link".to_string());
            let link = doc.add_node(item, NodeData::Element(link_el));

            let mut text_el = ElementData::new("span");
            text_el
                .attributes
                .insert("class".to_string(), "project-text".to_string());
            let text = doc.add_node(link, NodeData::Element(text_el));

            let mut title_el = ElementData::new("span");
            title_el
                .attributes
                .insert("class".to_string(), "project-title".to_string());
            let title = doc.add_node(text, NodeData::Element(title_el));
            doc.add_node(
                title,
                NodeData::Text(TextData {
                    content: format!("Project {}", i + 1),
                }),
            );

            let mut tag_el = ElementData::new("span");
            tag_el
                .attributes
                .insert("class".to_string(), "project-tagline".to_string());
            let tag = doc.add_node(text, NodeData::Element(tag_el));
            doc.add_node(
                tag,
                NodeData::Text(TextData {
                    content: "Short description".to_string(),
                }),
            );

            items.push(item);
        }

        let stylesheet = incognidium_css::parse_css(
            "* { margin: 0; padding: 0; border: none; box-sizing: border-box; } \
             body { width: 1024px; font-size: 16px; } \
             .projects { display: inline-block; width: 65%; } \
             .project { float: left; position: relative; width: 33%; height: 9rem; } \
             .project-link { display: inline-block; min-height: 50px; width: 90%; padding: 1em; white-space: nowrap; } \
             .project-text { display: inline-block; max-width: 65%; font-size: 1.4rem; vertical-align: middle; white-space: normal; } \
             .project-title { display: block; } \
             .project-tagline { display: block; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let container_box = find_box(&root, container).expect("container layout box");
        // 9rem = 144px at default 16px root; two rows should be ~288px content.
        assert!(
            container_box.height >= 288.0,
            "container should enclose two rows of 9rem floats: got height {}",
            container_box.height
        );

        let row0_y = find_box(&root, items[0]).unwrap().y;
        let row1_y = find_box(&root, items[3]).unwrap().y;
        assert!(
            row1_y >= row0_y + 144.0 - 1.0,
            "second row should start below 9rem first row ({} >= {} + 144)",
            row1_y,
            row0_y
        );
    }

    #[test]
    fn test_column_flex_item_percentage_width_resolves_against_container() {
        // Regression: a column flex item with `display: inline-block` and a
        // percentage `width` should resolve that width against the flex
        // container's content box, not collapse to its intrinsic content width.
        // This keeps multi-column float grids inside inline-block flex items
        // wide enough for their content to fit.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));

        let mut footer_el = ElementData::new("footer");
        footer_el
            .attributes
            .insert("class".to_string(), "footer".to_string());
        let footer = doc.add_node(body, NodeData::Element(footer_el));

        let mut sidebar_el = ElementData::new("div");
        sidebar_el
            .attributes
            .insert("class".to_string(), "footer-sidebar".to_string());
        let sidebar = doc.add_node(footer, NodeData::Element(sidebar_el));
        doc.add_node(
            sidebar,
            NodeData::Text(TextData {
                content: "Sidebar text".to_string(),
            }),
        );

        let mut projects_el = ElementData::new("div");
        projects_el
            .attributes
            .insert("class".to_string(), "projects".to_string());
        let projects = doc.add_node(footer, NodeData::Element(projects_el));

        let mut items = Vec::new();
        for i in 0..3 {
            let mut item_el = ElementData::new("div");
            item_el
                .attributes
                .insert("class".to_string(), "project".to_string());
            let item = doc.add_node(projects, NodeData::Element(item_el));
            doc.add_node(
                item,
                NodeData::Text(TextData {
                    content: format!("Item {}", i + 1),
                }),
            );
            items.push(item);
        }

        let stylesheet = incognidium_css::parse_css(
            "* { margin: 0; padding: 0; border: none; box-sizing: border-box; font-size: 10px; } \
             body { width: 1000px; } \
             .footer { display: flex; flex-direction: column; padding: 1.28rem; } \
             .footer-sidebar { order: 1; width: 35%; } \
             .projects { order: 2; display: inline-block; width: 65%; } \
             .project { float: left; width: 33%; height: 9rem; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1000.0, 600.0);
        let root = layout(&doc, &styles, 1000.0, 600.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let projects_box = find_box(&root, projects).expect("projects layout box");
        // Body content width is 1000 - no body padding in test. Footer content
        // width is 1000 - 2*12.8 = 974.4. 65% of that is ~633px.
        assert!(
            projects_box.width >= 600.0,
            "column flex item should resolve 65% width against ~974px container: got {}",
            projects_box.width
        );

        // With a wide enough container, three 33% floats should fit in one row.
        let first_y = find_box(&root, items[0]).unwrap().y;
        for i in 1..3 {
            let y = find_box(&root, items[i]).unwrap().y;
            assert!(
                (y - first_y).abs() < 1.0,
                "floats should share a single row when container is wide enough: item {} y={}",
                i + 1,
                y
            );
        }
    }

    #[test]
    fn test_inline_block_wraps_around_stacked_cleared_floats() {
        // Regression: when two left floats both have clear:left, a following
        // inline-block must wrap around the whole stack starting at the top of
        // the first float, not be pushed down to the top of the last (cleared)
        // float. It must also use the float-reduced inline width so it sits
        // beside the floats instead of overlapping them.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));

        let mut a_el = ElementData::new("div");
        a_el.attributes.insert("class".to_string(), "a".to_string());
        let a = doc.add_node(body, NodeData::Element(a_el));

        let mut b_el = ElementData::new("div");
        b_el.attributes.insert("class".to_string(), "b".to_string());
        let b = doc.add_node(body, NodeData::Element(b_el));

        let mut c_el = ElementData::new("span");
        c_el.attributes.insert("class".to_string(), "c".to_string());
        let c = doc.add_node(body, NodeData::Element(c_el));

        let stylesheet = incognidium_css::parse_css(
            "* { margin: 0; padding: 0; border: none; box-sizing: border-box; } \
             body { width: 500px; } \
             .a { float: left; clear: left; width: 100px; height: 100px; margin: 5px; } \
             .b { float: left; clear: left; width: 100px; height: 100px; margin: 5px; } \
             .c { display: inline-block; width: 300px; height: 300px; margin: 5px; vertical-align: top; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 500.0, 600.0);
        let root = layout(&doc, &styles, 500.0, 600.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let a_box = find_box(&root, a).expect("float a");
        let b_box = find_box(&root, b).expect("float b");
        let c_box = find_box(&root, c).expect("inline-block c");

        // Both floats are stacked on the left because of clear:left.
        assert!(
            (a_box.x - b_box.x).abs() < 0.5,
            "stacked floats share the same left edge"
        );
        assert!(b_box.y > a_box.y, "cleared float b sits below float a");

        // The inline-block must sit to the right of the floats (not at the left
        // content edge) and must start no lower than the top of the first float.
        let float_right_edge = a_box.x + a_box.width;
        assert!(
            c_box.x >= float_right_edge - 0.5,
            "inline-block should be placed beside the floats: c.x={} float_right={}",
            c_box.x,
            float_right_edge
        );
        assert!(
            c_box.y <= a_box.y + 0.5,
            "inline-block should wrap around the first float top: c.y={} a.y={}",
            c_box.y,
            a_box.y
        );
    }

    #[test]
    fn test_table_cell_inline_children_form_single_line_run() {
        // Regression: a table cell whose children are all inline-level must lay
        // them out on a shared line, not stack them vertically. This mirrors the
        // block formatting context that real table cells establish.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let table = doc.add_node(body, NodeData::Element(ElementData::new("table")));
        let row = doc.add_node(table, NodeData::Element(ElementData::new("tr")));

        let mut pad_el = ElementData::new("td");
        pad_el
            .attributes
            .insert("colspan".to_string(), "2".to_string());
        let _pad = doc.add_node(row, NodeData::Element(pad_el));

        let mut cell_el = ElementData::new("td");
        cell_el
            .attributes
            .insert("class".to_string(), "meta-cell".to_string());
        let cell = doc.add_node(row, NodeData::Element(cell_el));

        let mut age_el = ElementData::new("span");
        age_el
            .attributes
            .insert("class".to_string(), "age".to_string());
        let age = doc.add_node(cell, NodeData::Element(age_el));
        let _age_text = doc.add_node(
            age,
            NodeData::Text(TextData {
                content: "6 hours ago".to_string(),
            }),
        );
        let _separator = doc.add_node(
            cell,
            NodeData::Text(TextData {
                content: " | ".to_string(),
            }),
        );
        let mut action_el = ElementData::new("a");
        action_el
            .attributes
            .insert("href".to_string(), "#".to_string());
        let action = doc.add_node(cell, NodeData::Element(action_el));
        let _action_text = doc.add_node(
            action,
            NodeData::Text(TextData {
                content: "hide".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            "* { margin: 0; padding: 0; border: none; font-family: sans-serif; font-size: 10pt; } \
             table { border-collapse: collapse; width: 500px; } \
             td { padding: 0; } \
             .meta-cell { font-size: 7pt; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 500.0, 600.0);
        let root = layout(&doc, &styles, 500.0, 600.0);

        fn find_box(root: &LayoutBox, node_id: incognidium_dom::NodeId) -> Option<&LayoutBox> {
            if root.node_id == node_id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_box(c, node_id))
        }

        let cell_box = find_box(&root, cell).expect("meta-cell td");
        let age_box = find_box(&root, age).expect("age span");
        let action_box = find_box(&root, action).expect("action link");

        // The cell should be roughly one line tall, not three.
        assert!(
            cell_box.height < 20.0,
            "inline cell should stay single-line, got height {}",
            cell_box.height
        );

        // The inline children should share the same baseline line.
        assert!(
            (age_box.y - action_box.y).abs() < 0.5,
            "inline children should sit on the same line: age.y={} action.y={}",
            age_box.y,
            action_box.y
        );

        // The action link should be to the right of the age span.
        assert!(
            action_box.x > age_box.x + age_box.width - 0.5,
            "action link should follow the age span horizontally: action.x={} age.x+age.w={}",
            action_box.x,
            age_box.x + age_box.width
        );
    }

    #[test]
    fn test_pseudo_element_attr_content() {
        // ::before/::after content can use attr() to pull text from the originating
        // element. This is a common, standards-compliant pattern used for labels,
        // link annotations, and generated prefixes.
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let body = doc.add_node(html, NodeData::Element(ElementData::new("body")));
        let mut el = ElementData::new("div");
        el.attributes
            .insert("data-label".to_string(), "Prefix:".to_string());
        el.attributes
            .insert("href".to_string(), "https://example.com".to_string());
        let div = doc.add_node(body, NodeData::Element(el));
        let _ = doc.add_node(
            div,
            NodeData::Text(TextData {
                content: " content".to_string(),
            }),
        );

        let stylesheet = incognidium_css::parse_css(
            "div::before { content: attr(data-label); } \
             div::after { content: ' [' attr(href) ']'; }",
        );
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet, 1024.0, 768.0);
        let root = layout(&doc, &styles, 1024.0, 768.0);

        fn collect_text(boxes: &[LayoutBox]) -> Vec<String> {
            let mut out = Vec::new();
            for b in boxes {
                if let Some(ref t) = b.text {
                    out.push(t.clone());
                }
                out.extend(collect_text(&b.children));
            }
            out
        }
        let texts = collect_text(&root.children);
        let joined = texts.join("");
        assert!(
            joined.contains("Prefix:"),
            "::before should resolve attr(data-label): got {:?}",
            texts
        );
        assert!(
            joined.contains("[https://example.com]"),
            "::after should resolve attr(href): got {:?}",
            texts
        );
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

/// Resolve the colspan value stored in `flex_grow` for a table cell, clamped
/// to the remaining available columns.
fn table_cell_colspan(cell: &LayoutBox, styles: &StyleMap, max_span: usize) -> usize {
    styles
        .get(&cell.node_id)
        .map(|s| s.flex_grow.max(1.0) as usize)
        .unwrap_or(1)
        .min(max_span)
        .max(1)
}

/// Returns true when a table cell carries no visible content. Empty cells should
/// not dominate intrinsic column-width calculations.
fn is_empty_table_cell(cell: &LayoutBox) -> bool {
    if let Some(ref t) = cell.text {
        if !is_collapsible_whitespace_only(t) {
            return false;
        }
    }
    if cell.children.is_empty() {
        return true;
    }
    cell.children.iter().all(|c| {
        matches!(
            c.box_type,
            BoxType::Text | BoxType::None | BoxType::LineBreak
        ) && c
            .text
            .as_ref()
            .map(|t| is_collapsible_whitespace_only(t))
            .unwrap_or(true)
            && c.children.is_empty()
    })
}

/// Compute intrinsic widths for each column of an auto-width table by measuring
/// the natural widths of all cells. Returns a vector of column widths sized to
/// the number of columns in the widest row.
fn compute_auto_table_column_widths(
    table_box: &LayoutBox,
    styles: &StyleMap,
    image_sizes: &ImageSizes,
    table_content_width: f32,
) -> Vec<f32> {
    // Collect all rows (flatten through thead/tbody/tfoot sections).
    let mut rows: Vec<&LayoutBox> = Vec::new();
    for child in &table_box.children {
        if child.box_type == BoxType::TableSection {
            rows.extend(child.children.iter());
        } else if child.box_type == BoxType::TableRow {
            rows.push(child);
        }
    }
    if rows.is_empty() {
        return Vec::new();
    }

    // Determine the number of columns from the maximum colspan sum across rows.
    let num_cols = rows
        .iter()
        .map(|r| {
            let mut total = 0;
            for cell in &r.children {
                total += table_cell_colspan(cell, styles, 1000);
            }
            total
        })
        .max()
        .unwrap_or(1);
    let mut col_intrinsics: Vec<f32> = vec![0.0; num_cols];

    // Measure each non-empty cell's intrinsic width, accounting for colspan so
    // that cells in rows with spanning cells are assigned to the right columns.
    // Empty cells (including empty colspan pads in nested-table layouts)
    // are skipped; they must not inflate the columns they span.
    // Cells with `max-width` cap their columns: the auto layout prefers the
    // intrinsic width but never exceeds the cap, and leftover table width only
    // stretches columns without a cap (mirroring how browsers grow the free
    // column of a two-column description table instead of the capped one).
    let mut col_caps: Vec<Option<f32>> = vec![None; num_cols];
    // Cells with an explicit `width` pin their column to at least that width.
    // The auto table layout treats a specified cell width as a fixed
    // contribution; the remaining width is shared by the columns without one.
    let mut col_specs: Vec<Option<f32>> = vec![None; num_cols];
    for row in &rows {
        let mut col_start = 0usize;
        for cell in &row.children {
            if col_start >= num_cols {
                break;
            }
            let colspan = table_cell_colspan(cell, styles, num_cols - col_start);
            if !is_empty_table_cell(cell) {
                let cs = styles.get(&cell.node_id).cloned().unwrap_or_default();
                let resolved_spec = match cs.width {
                    SizeValue::Px(v) => Some(v),
                    SizeValue::Percent(p) => Some(table_content_width * p / 100.0),
                    SizeValue::Calc(_)
                    | SizeValue::Min(_)
                    | SizeValue::Max(_)
                    | SizeValue::Clamp { .. } => {
                        evaluate_size_value(&cs.width, table_content_width, cs.font_size)
                    }
                    _ => None,
                };
                if let Some(v) = resolved_spec {
                    // The stored column widths are content widths, so subtract
                    // the padding/border the width has to cover under the
                    // cell's box-sizing.
                    let pb = cs.padding_left_px(table_content_width)
                        + cs.padding_right_px(table_content_width)
                        + cs.border_left_width
                        + cs.border_right_width;
                    let content_spec = if cs.box_sizing == incognidium_style::BoxSizing::BorderBox {
                        (v - pb).max(0.0)
                    } else {
                        v
                    };
                    for c in 0..colspan {
                        let idx = col_start + c;
                        col_specs[idx] = match col_specs[idx] {
                            Some(existing) => Some(existing.max(content_spec)),
                            None => Some(content_spec),
                        };
                    }
                }
                let resolved_cap = match cs.max_width {
                    SizeValue::Px(v) => Some(v),
                    SizeValue::Percent(p) => Some(table_content_width * p / 100.0),
                    SizeValue::Calc(_)
                    | SizeValue::Min(_)
                    | SizeValue::Max(_)
                    | SizeValue::Clamp { .. } => {
                        evaluate_size_value(&cs.max_width, table_content_width, cs.font_size)
                    }
                    _ => None,
                };
                if let Some(v) = resolved_cap {
                    // The intrinsic measured below is the cell's content
                    // width, so subtract the padding/border the cap has to
                    // cover (default content-box sizing).
                    let pb = cs.padding_left_px(table_content_width)
                        + cs.padding_right_px(table_content_width)
                        + cs.border_left_width
                        + cs.border_right_width;
                    let content_cap = (v - pb).max(0.0);
                    for c in 0..colspan {
                        let idx = col_start + c;
                        col_caps[idx] = match col_caps[idx] {
                            Some(existing) => Some(existing.min(content_cap)),
                            None => Some(content_cap),
                        };
                    }
                }
                let mut cell_clone = cell.clone();
                compute_layout_with_floats(
                    &mut cell_clone,
                    styles,
                    10_000.0,
                    0.0,
                    image_sizes,
                    FloatState::default(),
                );
                let intrinsic = calculate_intrinsic_width(&cell_clone, styles);
                let per_col = intrinsic / colspan as f32;
                for c in 0..colspan {
                    let idx = col_start + c;
                    col_intrinsics[idx] = col_intrinsics[idx].max(per_col);
                }
            }
            col_start += colspan;
        }
    }

    // Total intrinsic demand.
    let total_intrinsic: f32 = col_intrinsics.iter().sum();

    if total_intrinsic <= 0.0 {
        // No useful intrinsic widths; fall back to equal division.
        return vec![table_content_width / num_cols as f32; num_cols];
    }

    // Scale if the table is narrower than the intrinsic demand; otherwise
    // distribute leftover space proportionally to each column's intrinsic width.
    let scale = if total_intrinsic > table_content_width {
        table_content_width / total_intrinsic
    } else {
        1.0
    };
    let mut widths: Vec<f32> = col_intrinsics.iter().map(|w| w * scale).collect();

    // Enforce cell max-width caps before redistributing the leftover space.
    for (i, w) in widths.iter_mut().enumerate() {
        if let Some(cap) = col_caps[i] {
            if *w > cap {
                *w = cap;
            }
        }
    }

    // Specified cell widths win over the scaled intrinsic share (a max-width
    // cap still wins over the specification).
    for (i, w) in widths.iter_mut().enumerate() {
        if let Some(spec) = col_specs[i] {
            if *w < spec {
                *w = spec;
            }
        }
    }

    if total_intrinsic < table_content_width {
        let leftover = table_content_width - widths.iter().sum::<f32>();
        let uncapped_intrinsic: f32 = col_intrinsics
            .iter()
            .enumerate()
            .filter(|(i, _)| col_caps[*i].is_none() && col_specs[*i].is_none())
            .map(|(_, v)| *v)
            .sum();
        if leftover > 0.0 && uncapped_intrinsic > 0.0 {
            // Distribute leftover proportionally by intrinsic weight, among
            // the columns no max-width caps or explicit widths.
            for (i, w) in widths.iter_mut().enumerate() {
                if col_caps[i].is_some() || col_specs[i].is_some() {
                    continue;
                }
                *w += leftover * col_intrinsics[i] / uncapped_intrinsic;
            }
        }
    }

    widths
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
    let padding_left = style.padding_left_px(containing_width);
    let padding_right = style.padding_right_px(containing_width);
    let padding_top = style.padding_top_px(containing_width);
    let padding_bottom = style.padding_bottom_px(containing_width);
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
            if is_border_box {
                let border_box_width = (containing_width - margin_left - margin_right).max(0.0);
                (border_box_width - padding_left - padding_right - border_left - border_right)
                    .max(0.0)
            } else {
                (containing_width - margin_left - margin_right).max(0.0)
            }
        }
        // CSS Math Functions - treat as auto for now
        _ => {
            if is_border_box {
                let border_box_width = (containing_width - margin_left - margin_right).max(0.0);
                (border_box_width - padding_left - padding_right - border_left - border_right)
                    .max(0.0)
            } else {
                (containing_width - margin_left - margin_right).max(0.0)
            }
        }
    };

    layout_box.content_width = content_width;
    layout_box.width = content_width + padding_left + padding_right + border_left + border_right;

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
    let border_top = style.border_top_width;
    let border_bottom = style.border_bottom_width;
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
    let mut y_offset = padding_top
        + border_top
        + if caption_at_bottom {
            0.0
        } else {
            caption_height
        };
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

    // Compute intrinsic column widths for auto-layout tables so that cells
    // with narrow fixed content (e.g. spacer images) do not steal space from
    // the main text column. Save/restore any outer table widths so nested
    // tables do not corrupt the widths of their parent.
    let prev_col_widths = TABLE_COL_WIDTHS.with(|cw| cw.borrow().clone());
    if style.table_layout == incognidium_style::TableLayout::Auto {
        let auto_widths =
            compute_auto_table_column_widths(layout_box, styles, image_sizes, content_width);
        TABLE_COL_WIDTHS.with(|cw| *cw.borrow_mut() = auto_widths);
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
                    right: right,   // Will be resolved when we process the next cell
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
        let table_content_height = y_offset
            - padding_top
            - border_top
            - (if caption_at_bottom {
                0.0
            } else {
                caption_height
            });
        for &idx in &caption_indices {
            layout_box.children[idx].y = padding_top + border_top + table_content_height;
        }
    }

    let content_height = y_offset - padding_top - border_top
        + border_v
        + if caption_at_bottom {
            caption_height
        } else {
            0.0
        };
    layout_box.content_height = content_height.max(0.0);
    layout_box.height = content_height + padding_top + padding_bottom + border_top + border_bottom;

    // Restore outer table column widths so sibling nested tables do not leak.
    TABLE_COL_WIDTHS.with(|cw| *cw.borrow_mut() = prev_col_widths);
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
    // Use intrinsic column widths computed by layout_table for auto tables.
    // Fall back to equal division when no widths are stored (e.g. fixed layout).
    let col_widths = TABLE_COL_WIDTHS.with(|cw| cw.borrow().clone());
    let num_cols = col_widths.len();
    let use_auto_widths = num_cols >= num_cells;
    let default_cell_width = containing_width / num_cells as f32;
    let (border_h, border_v) = if is_collapsed {
        (0.0, 0.0)
    } else {
        style.border_spacing
    };

    // Precompute each cell's starting column and colspan so that spanning
    // cells (common on nested-table subtext rows) receive the correct width
    // and horizontal position.
    let mut spans: Vec<(usize, usize)> = Vec::with_capacity(num_cells);
    let mut col_start = 0usize;
    for child in &layout_box.children {
        let colspan = if use_auto_widths && col_start < num_cols {
            table_cell_colspan(child, styles, num_cols - col_start)
        } else {
            1
        };
        spans.push((col_start, colspan));
        col_start += colspan;
    }

    let mut max_cell_height = 0.0f32;
    let mut x_offset = border_h;

    // First pass: layout all cells to get their natural heights
    for (cell_idx, child) in layout_box.children.iter_mut().enumerate() {
        let (start, span) = spans[cell_idx];
        let cell_width = if use_auto_widths && start + span <= num_cols {
            col_widths[start..start + span].iter().sum::<f32>()
        } else {
            default_cell_width
        };
        // In border-collapse mode, cells include their borders in the width
        let available_width = if is_collapsed {
            cell_width
        } else {
            (cell_width - border_h * 2.0).max(0.0)
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

    // An explicit height on a table row acts as a minimum row height.
    // This is common on nested-table layouts, where spacer rows use
    // inline `style="height:5px"` to create vertical rhythm.
    let explicit_min_height = match style.height {
        SizeValue::Px(h) => Some(h),
        SizeValue::Calc(_) | SizeValue::Min(_) | SizeValue::Max(_) | SizeValue::Clamp { .. } => {
            evaluate_size_value(&style.height, containing_width, style.font_size)
        }
        _ => None,
    };
    let row_content_height = explicit_min_height
        .map(|h| h.max(max_cell_height))
        .unwrap_or(max_cell_height);

    // Second pass: set positions and stretch cells to row height
    for (cell_idx, child) in layout_box.children.iter_mut().enumerate() {
        let (start, _span) = spans[cell_idx];
        if use_auto_widths && start < num_cols {
            child.x = border_h + col_widths[..start].iter().sum::<f32>();
        } else {
            child.x = x_offset;
            x_offset += child.width + border_h * 2.0;
        }
        child.y = border_v;
        // Stretch cell to match tallest cell in row (for equal-height cells)
        if child.height < row_content_height {
            let extra = row_content_height - child.height;
            child.height = row_content_height;
            child.content_height = row_content_height - child.y - border_v;
            // vertical-align positions a stretched cell's content within the
            // extra space the stretch created (CSS 2.1 §17.5.3). Baseline and
            // top-aligned cells keep their content where it was laid out.
            let cell_style = styles.get(&child.node_id).cloned().unwrap_or_default();
            let shift = match cell_style.vertical_align {
                incognidium_style::VerticalAlign::Middle => extra / 2.0,
                incognidium_style::VerticalAlign::Bottom => extra,
                _ => 0.0,
            };
            if shift > 0.0 {
                fn shift_box_y(box_to_shift: &mut LayoutBox, delta: f32) {
                    box_to_shift.y += delta;
                    for c in &mut box_to_shift.children {
                        shift_box_y(c, delta);
                    }
                }
                for c in &mut child.children {
                    shift_box_y(c, shift);
                }
            }
        }
    }

    layout_box.width = containing_width;
    layout_box.height = row_content_height + border_v * 2.0;
    layout_box.content_width = containing_width;
    layout_box.content_height = row_content_height;
}

fn layout_table_cell(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    image_sizes: &ImageSizes,
    _parent_floats: FloatState,
) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    let padding_left = style.padding_left_px(containing_width);
    let padding_right = style.padding_right_px(containing_width);
    let padding_top = style.padding_top_px(containing_width);
    let padding_bottom = style.padding_bottom_px(containing_width);

    // Use collapsed borders if set, otherwise use style borders
    let (border_top, border_right, border_bottom, border_left) =
        if let Some(cb) = layout_box.collapsed_borders {
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

    // Table cells establish a block formatting context. When all children are
    // inline-level they must be laid out as a single line run so that metadata
    // rows with multiple inline siblings (e.g. "6 hours ago | hide") do not
    // stack vertically and balloon the row height.
    let all_inline = children_are_inline_level(&layout_box.children, styles);
    let content_height = if all_inline {
        for child in &mut layout_box.children {
            compute_layout_with_floats(
                child,
                styles,
                content_width,
                0.0,
                image_sizes,
                FloatState::default(),
            );
        }
        let (_used_width, run_height) = layout_inline_children_run(
            &mut layout_box.children,
            styles,
            content_width,
            padding_left,
            padding_top,
            border_left,
            border_top,
        );
        run_height
    } else {
        // Layout children as a block, applying margin collapse so block-level
        // children's margins contribute to the cell's final height. Without this,
        // margins such as `.votelinks a { display:block; margin-bottom:9px }` on
        // nested-table comment rows are ignored and rows become too short.
        let mut y_offset = padding_top + border_top;
        let mut prev_margin_bottom: f32 = 0.0;
        for child in &mut layout_box.children {
            compute_layout_with_floats(
                child,
                styles,
                content_width,
                0.0,
                image_sizes,
                FloatState::default(),
            );
            let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
            let is_block_child = !is_inline_level_styled(child.box_type, styles, child.node_id);

            if is_block_child {
                // Margin collapse: positive margins keep the larger; a negative margin
                // is added to the previous sibling's margin so it pulls the child up.
                let collapsed_margin_top =
                    if child_style.margin_top >= 0.0 && prev_margin_bottom >= 0.0 {
                        child_style.margin_top.max(prev_margin_bottom)
                    } else {
                        child_style.margin_top + prev_margin_bottom
                    };
                child.x = padding_left + border_left;
                child.y = y_offset + collapsed_margin_top - prev_margin_bottom;
                y_offset += collapsed_margin_top + child.height;
                prev_margin_bottom = child_style.margin_bottom;
            } else {
                // Inline-level children do not participate in block margin collapse.
                child.x = padding_left + border_left;
                child.y = y_offset;
                y_offset += child.height;
                prev_margin_bottom = 0.0;
            }
        }
        y_offset + prev_margin_bottom - padding_top - border_top
    };
    layout_box.content_width = content_width.max(0.0);
    layout_box.content_height = content_height.max(0.0);
    layout_box.width = containing_width;
    layout_box.height = content_height + padding_top + padding_bottom + border_top + border_bottom;

    // Check for empty-cells: hide
    // An empty cell has no meaningful content (no text, no children with content)
    let is_empty = layout_box.children.is_empty()
        || layout_box.children.iter().all(|c| match c.box_type {
            BoxType::Text => c
                .text
                .as_ref()
                .map(|t| is_collapsible_whitespace_only(t))
                .unwrap_or(true),
            BoxType::None => true,
            _ => false,
        });

    if is_empty && style.empty_cells == incognidium_style::EmptyCells::Hide {
        layout_box.hide_empty_cell = true;
    }
}
