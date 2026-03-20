use std::collections::HashMap;
use incognidium_dom::{Document, NodeData, NodeId};
use incognidium_style::{
    AlignItems, Display, Float, FlexDirection, GridTrackSize, JustifyContent, Position,
    SizeValue, StyleMap, TextAlign,
};

/// Image dimensions: (width, height) keyed by image src.
pub type ImageSizes = HashMap<String, (u32, u32)>;

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
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BoxType {
    Block,
    Inline,
    Flex,
    Grid,
    Text,
    Image,
    None,
}

/// Build the layout tree and compute positions.
pub fn layout(doc: &Document, styles: &StyleMap, viewport_width: f32, viewport_height: f32) -> LayoutBox {
    let empty = ImageSizes::new();
    layout_with_images(doc, styles, viewport_width, viewport_height, &empty)
}

/// Build the layout tree with image size information.
pub fn layout_with_images(doc: &Document, styles: &StyleMap, viewport_width: f32, viewport_height: f32, image_sizes: &ImageSizes) -> LayoutBox {
    let root_id = doc.root();
    let mut root_box = build_layout_tree(doc, styles, root_id);
    root_box.width = viewport_width;
    compute_layout(&mut root_box, styles, viewport_width, viewport_height, image_sizes);
    root_box
}

fn build_layout_tree(doc: &Document, styles: &StyleMap, node_id: NodeId) -> LayoutBox {
    let node = doc.node(node_id);
    let style = styles.get(&node_id);

    let display = style.map(|s| s.display).unwrap_or(Display::Block);

    let position = style.map(|s| s.position).unwrap_or(Position::Static);

    // Skip fixed-position elements (sticky headers, modals, overlays)
    if display == Display::None || position == Position::Fixed {
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
        };
    }

    let (box_type, text, image_src) = match &node.data {
        NodeData::Text(t) => {
            let trimmed = t.content.trim();
            if trimmed.is_empty() {
                // Whitespace-only text node: keep as a single space if it contains
                // any whitespace (important for spacing between inline elements)
                if t.content.contains(|c: char| c.is_whitespace()) {
                    (BoxType::Text, Some(" ".to_string()), None)
                } else {
                    (BoxType::None, None, None)
                }
            } else {
                // Collapse internal whitespace runs and preserve leading/trailing single space
                let has_leading_space = t.content.starts_with(|c: char| c.is_whitespace());
                let has_trailing_space = t.content.ends_with(|c: char| c.is_whitespace());
                let mut normalized = String::new();
                if has_leading_space {
                    normalized.push(' ');
                }
                normalized.push_str(trimmed);
                if has_trailing_space {
                    normalized.push(' ');
                }
                (BoxType::Text, Some(normalized), None)
            }
        }
        NodeData::Element(el) => {
            if el.tag_name == "img" {
                let src = el.get_attr("src").map(|s| s.to_string());
                (BoxType::Image, None, src)
            } else if el.tag_name == "canvas" {
                // Canvas elements render as Image boxes with a special src key
                let canvas_src = format!("__canvas__{}", node_id);
                (BoxType::Image, None, Some(canvas_src))
            } else if el.tag_name == "input" {
                // Show value or placeholder text
                let text = el.get_attr("value")
                    .or_else(|| el.get_attr("placeholder"))
                    .map(|s| s.to_string());
                (BoxType::Block, text, None)
            } else {
                match display {
                    Display::Block | Display::InlineBlock => (BoxType::Block, None, None),
                    Display::Inline => (BoxType::Inline, None, None),
                    Display::Flex => (BoxType::Flex, None, None),
                    Display::Grid => (BoxType::Grid, None, None),
                    Display::None => (BoxType::None, None, None),
                }
            }
        }
        _ => (BoxType::Block, None, None),
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

    let mut children: Vec<LayoutBox> = node
        .children
        .iter()
        .map(|&child_id| build_layout_tree(doc, styles, child_id))
        .filter(|b| b.box_type != BoxType::None)
        .collect();

    // Add list bullet/number markers for <li> elements
    if let NodeData::Element(ref el) = node.data {
        if el.tag_name == "li" {
            // Determine marker type from parent
            let marker = if let Some(parent_id) = node.parent {
                let parent_node = doc.node(parent_id);
                if let NodeData::Element(ref pel) = parent_node.data {
                    if pel.tag_name == "ol" {
                        // Count which <li> we are among siblings
                        let idx = parent_node.children.iter()
                            .filter(|&&cid| {
                                matches!(&doc.node(cid).data, NodeData::Element(ref e) if e.tag_name == "li")
                            })
                            .position(|&cid| cid == node_id)
                            .unwrap_or(0);
                        format!("{}. ", idx + 1)
                    } else {
                        "\u{2022} ".to_string() // bullet
                    }
                } else {
                    "\u{2022} ".to_string()
                }
            } else {
                "\u{2022} ".to_string()
            };
            children.insert(0, LayoutBox {
                node_id,
                x: 0.0, y: 0.0,
                width: 0.0, height: 0.0,
                content_width: 0.0, content_height: 0.0,
                children: Vec::new(),
                box_type: BoxType::Text,
                text: Some(marker),
                image_src: None,
                link_href: None,
            });
        }
    }

    // Collapse empty containers: block/flex/inline with no meaningful content
    // This prevents empty wrapper divs from taking up space when all their content is hidden
    let has_meaningful_content = if text.as_deref().map(|t| !t.trim().is_empty()).unwrap_or(false) {
        true
    } else if children.is_empty() && image_src.is_none() {
        false
    } else {
        // Check if children have meaningful visible content
        children.iter().any(|c| {
            match c.box_type {
                BoxType::Text => {
                    c.text.as_deref().map(|t| !t.trim().is_empty()).unwrap_or(false)
                }
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

    let effective_box_type = if (box_type == BoxType::Block || box_type == BoxType::Flex || box_type == BoxType::Grid || box_type == BoxType::Inline)
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
    }
}

fn compute_layout(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    _containing_height: f32,
    image_sizes: &ImageSizes,
) {
    match layout_box.box_type {
        BoxType::Block => {
            layout_block(layout_box, styles, containing_width, image_sizes);
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
        BoxType::Text => {
            layout_text(layout_box, styles, containing_width);
        }
        BoxType::Image => {
            layout_image(layout_box, styles, containing_width, image_sizes);
        }
        BoxType::None => {}
    }
}

fn layout_block(layout_box: &mut LayoutBox, styles: &StyleMap, containing_width: f32, image_sizes: &ImageSizes) {
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
        SizeValue::Auto | SizeValue::None => {
            containing_width - margin_left - margin_right - padding_left - padding_right
                - border_left - border_right
        }
    };

    // Apply max-width constraint
    match style.max_width {
        SizeValue::Px(mw) => {
            if content_width > mw {
                content_width = mw;
            }
        }
        SizeValue::Percent(p) => {
            let mw = containing_width * p / 100.0;
            if content_width > mw {
                content_width = mw;
            }
        }
        _ => {}
    }

    // Apply min-width constraint
    match style.min_width {
        SizeValue::Px(mw) => {
            if content_width < mw {
                content_width = mw;
            }
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
    layout_box.width = content_width + padding_left + padding_right + border_left + border_right;

    // Layout children
    let child_containing_width = layout_box.content_width;
    let mut cursor_y: f32 = style.padding_top + style.border_top_width;
    let content_x = padding_left + border_left;

    // Float tracking: when a float is active, subsequent content wraps around it
    let mut float_right_width: f32 = 0.0; // width consumed by right floats
    let mut float_left_width: f32 = 0.0;  // width consumed by left floats
    let mut float_bottom: f32 = 0.0;      // y position where floats end

    // Separate inline and block children
    let mut i = 0;
    while i < layout_box.children.len() {
        let child = &layout_box.children[i];

        if is_inline_level(child.box_type) {
            // Inline/text/image: lay out horizontally on a line
            // Reduce available width if floats are active
            if cursor_y >= float_bottom {
                float_right_width = 0.0;
                float_left_width = 0.0;
            }
            let inline_available = child_containing_width - float_right_width - float_left_width;
            let inline_x_start = content_x + float_left_width;

            let line_start = i;
            let mut line_height: f32 = 0.0;

            while i < layout_box.children.len() {
                let c = &layout_box.children[i];
                if !is_inline_level(c.box_type) {
                    break;
                }
                compute_layout(
                    &mut layout_box.children[i],
                    styles,
                    inline_available,
                    0.0,
                    image_sizes,
                );
                i += 1;
            }

            // Skip inline runs that consist only of whitespace text nodes
            // (whitespace between block elements should not take up space)
            let all_whitespace = (line_start..i).all(|j| {
                layout_box.children[j].text.as_deref() == Some(" ")
            });
            if all_whitespace {
                continue;
            }

            // Compute inter-element gaps to prevent text concatenation
            let gaps = compute_inline_gaps(&layout_box.children, line_start, i);

            // Position inline children on a line with word-wrap
            let mut line_x = inline_x_start;
            let mut line_begin = line_start;
            for j in line_start..i {
                let gap = gaps[j - line_start];
                line_x += gap;

                let child_width = layout_box.children[j].width;
                let child_height = layout_box.children[j].height;
                // Line breaking with float-aware width
                if line_x + child_width > inline_x_start + inline_available + 0.5 && line_x > inline_x_start {
                    apply_text_align(&mut layout_box.children, line_begin, j, line_x - inline_x_start, inline_available, &style);
                    cursor_y += line_height;
                    line_x = inline_x_start;
                    line_height = 0.0;
                    line_begin = j;
                    // Clear floats if we've passed them
                    if cursor_y >= float_bottom {
                        // No longer constrained by floats — future lines use full width
                        // but we keep inline_x_start and inline_available for this run
                    }
                }
                layout_box.children[j].x = line_x;
                layout_box.children[j].y = cursor_y;
                line_x += child_width;
                line_height = line_height.max(child_height);
            }
            // Apply text-align to the last line
            apply_text_align(&mut layout_box.children, line_begin, i, line_x - inline_x_start, inline_available, &style);
            cursor_y += line_height;
        } else {
            // Block child
            let cm = styles
                .get(&child.node_id)
                .cloned()
                .unwrap_or_default();

            // Clear floats if cursor is past float bottom
            if cursor_y >= float_bottom {
                float_right_width = 0.0;
                float_left_width = 0.0;
            }

            // Handle floated elements
            if cm.float != Float::None {
                let float_width = match cm.width {
                    SizeValue::Px(w) => w + cm.margin_left + cm.margin_right,
                    SizeValue::Percent(p) => child_containing_width * p / 100.0,
                    _ => child_containing_width * 0.4, // default float width
                };
                compute_layout(
                    &mut layout_box.children[i],
                    styles,
                    float_width - cm.margin_left - cm.margin_right,
                    0.0,
                    image_sizes,
                );
                if cm.float == Float::Right {
                    layout_box.children[i].x = content_x + child_containing_width - layout_box.children[i].width - cm.margin_right;
                    layout_box.children[i].y = cursor_y + cm.margin_top;
                    float_right_width = layout_box.children[i].width + cm.margin_left + cm.margin_right;
                } else {
                    layout_box.children[i].x = content_x + float_left_width + cm.margin_left;
                    layout_box.children[i].y = cursor_y + cm.margin_top;
                    float_left_width += layout_box.children[i].width + cm.margin_left + cm.margin_right;
                }
                float_bottom = (cursor_y + layout_box.children[i].height + cm.margin_top + cm.margin_bottom).max(float_bottom);
                i += 1;
                continue;
            }

            // Non-floated block: reduce available width if floats are active
            let effective_width = if cursor_y < float_bottom {
                child_containing_width - float_right_width - float_left_width
            } else {
                child_containing_width
            };
            let effective_x = content_x + float_left_width;

            compute_layout(
                &mut layout_box.children[i],
                styles,
                effective_width,
                0.0,
                image_sizes,
            );
            // Skip zero-height blocks from contributing margins (empty collapsed containers)
            if layout_box.children[i].height > 0.0 {
                // Center blocks that are narrower than container (auto margin behavior)
                let child_w = layout_box.children[i].width;
                let extra = (effective_width - child_w).max(0.0);
                let x_offset = if child_w < effective_width && extra > 1.0 {
                    if cm.max_width != SizeValue::None && cm.max_width != SizeValue::Auto {
                        extra / 2.0
                    } else {
                        cm.margin_left
                    }
                } else {
                    cm.margin_left
                };
                layout_box.children[i].x = effective_x + x_offset;
                layout_box.children[i].y = cursor_y + cm.margin_top;
                cursor_y += cm.margin_top + layout_box.children[i].height + cm.margin_bottom;
            }
            i += 1;
        }
    }

    // Calculate height
    let auto_height = cursor_y - style.padding_top - style.border_top_width;
    let content_height = match style.height {
        SizeValue::Px(h) => h,
        SizeValue::Percent(_p) => auto_height, // percentage height needs containing block height
        _ => auto_height,
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
    layout_box.height =
        content_height + style.padding_top + style.padding_bottom + style.border_top_width
            + style.border_bottom_width;
}

/// Check if a box type participates in inline flow.
fn is_inline_level(box_type: BoxType) -> bool {
    matches!(box_type, BoxType::Text | BoxType::Inline | BoxType::Image)
}

/// Compute inter-element gap to prevent text concatenation like "wordword".
/// Returns a Vec of gap values to add before each child.
fn compute_inline_gaps(children: &[LayoutBox], start: usize, end: usize) -> Vec<f32> {
    let space_width = 9.6;
    let count = end - start;
    let mut gaps = vec![0.0f32; count];
    for j in 1..count {
        let prev = &children[start + j - 1];
        let curr = &children[start + j];
        if prev.width > 0.0 && curr.width > 0.0 {
            let prev_is_space = prev.text.as_deref() == Some(" ");
            let curr_is_space = curr.text.as_deref() == Some(" ");
            let prev_ends_space = prev.text.as_deref()
                .map(|t| t.ends_with(' ')).unwrap_or(false);
            let curr_starts_space = curr.text.as_deref()
                .map(|t| t.starts_with(' ')).unwrap_or(false);

            if !prev_is_space && !curr_is_space
                && !prev_ends_space && !curr_starts_space
            {
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
fn apply_text_align(
    children: &mut [LayoutBox],
    start: usize,
    end: usize,
    used_width: f32,
    container_width: f32,
    style: &incognidium_style::ComputedStyle,
) {
    let remaining = container_width - used_width;
    if remaining <= 1.0 {
        return;
    }
    let shift = match style.text_align {
        TextAlign::Center => remaining / 2.0,
        TextAlign::Right => remaining,
        TextAlign::Left | TextAlign::Justify => return,
    };
    for child in &mut children[start..end] {
        child.x += shift;
    }
}

/// Layout an inline element (e.g. <a>, <span>): shrink-to-fit width.
fn layout_inline(layout_box: &mut LayoutBox, styles: &StyleMap, containing_width: f32, image_sizes: &ImageSizes) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    let padding_left = style.padding_left;
    let padding_right = style.padding_right;
    let padding_top = style.padding_top;
    let padding_bottom = style.padding_bottom;
    let border_left = style.border_left_width;
    let border_right = style.border_right_width;
    let border_top = style.border_top_width;
    let border_bottom = style.border_bottom_width;

    // Layout all children first to get their natural sizes
    for child in &mut layout_box.children {
        compute_layout(child, styles, containing_width, 0.0, image_sizes);
    }

    // Compute inter-element gaps for inline children
    let num_children = layout_box.children.len();
    let gaps = compute_inline_gaps(&layout_box.children, 0, num_children);

    // Position children inline (horizontal flow), wrapping when needed
    let mut line_x: f32 = 0.0;
    let mut line_height: f32 = 0.0;
    let mut total_height: f32 = 0.0;
    let mut max_line_width: f32 = 0.0;

    for (idx, child) in layout_box.children.iter_mut().enumerate() {
        line_x += gaps[idx];

        // Wrap if needed (0.5px tolerance for f32 rounding)
        if line_x + child.width > containing_width + 0.5 && line_x > 0.0 {
            max_line_width = max_line_width.max(line_x);
            total_height += line_height;
            line_x = 0.0;
            line_height = 0.0;
        }
        child.x = line_x + padding_left + border_left;
        child.y = total_height + padding_top + border_top;
        line_x += child.width;
        line_height = line_height.max(child.height);
    }
    total_height += line_height;
    max_line_width = max_line_width.max(line_x);

    layout_box.content_width = max_line_width;
    layout_box.content_height = total_height;
    layout_box.width = max_line_width + padding_left + padding_right + border_left + border_right;
    layout_box.height = total_height + padding_top + padding_bottom + border_top + border_bottom;
}

fn layout_flex(layout_box: &mut LayoutBox, styles: &StyleMap, containing_width: f32, image_sizes: &ImageSizes) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    let padding_left = style.padding_left;
    let padding_right = style.padding_right;
    let padding_top = style.padding_top;
    let padding_bottom = style.padding_bottom;
    let border_left = style.border_left_width;
    let border_right = style.border_right_width;
    let border_top = style.border_top_width;
    let border_bottom = style.border_bottom_width;

    let content_width = match style.width {
        SizeValue::Px(w) => w,
        SizeValue::Percent(p) => containing_width * p / 100.0,
        SizeValue::Auto | SizeValue::None => {
            containing_width - style.margin_left - style.margin_right - padding_left
                - padding_right - border_left - border_right
        }
    };

    layout_box.content_width = content_width.max(0.0);
    layout_box.width = content_width + padding_left + padding_right + border_left + border_right;

    let is_row = matches!(
        style.flex_direction,
        FlexDirection::Row | FlexDirection::RowReverse
    );

    // First pass: compute natural sizes of all children
    let num_children = layout_box.children.len();
    for child in &mut layout_box.children {
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
            // For auto basis, give a reasonable initial width based on number of children
            let initial_width = if basis > 0.0 {
                basis
            } else {
                // Content-based: give each child a proportional share as starting point
                let n = num_children.max(1) as f32;
                (content_width / n).max(20.0)
            };
            compute_layout(child, styles, initial_width, 0.0, image_sizes);
        } else {
            compute_layout(child, styles, content_width, 0.0, image_sizes);
        }
    }

    // Second pass: distribute space according to flex-grow
    let total_main_size: f32 = layout_box.children.iter().map(|c| {
        if is_row { c.width } else { c.height }
    }).sum();

    let gap_total = style.gap * (layout_box.children.len().saturating_sub(1) as f32);
    // For column flex: only distribute extra space if container has explicit height
    // Otherwise, container wraps to content height (no free space)
    let available = if is_row {
        content_width
    } else {
        match style.height {
            SizeValue::Px(h) => h,
            _ => match style.min_height {
                SizeValue::Px(mh) => mh,
                _ => total_main_size, // auto height = no free space
            }
        }
    } - gap_total;
    let free_space = (available - total_main_size).max(0.0);

    let total_grow: f32 = layout_box
        .children
        .iter()
        .map(|c| {
            styles
                .get(&c.node_id)
                .map(|s| s.flex_grow)
                .unwrap_or(0.0)
        })
        .sum();

    if total_grow > 0.0 && free_space > 0.0 {
        for child in &mut layout_box.children {
            let grow = styles
                .get(&child.node_id)
                .map(|s| s.flex_grow)
                .unwrap_or(0.0);
            if grow > 0.0 {
                let extra = free_space * (grow / total_grow);
                if is_row {
                    child.width += extra;
                    child.content_width += extra;
                    // Re-layout children with new width
                    compute_layout(child, styles, child.content_width, 0.0, image_sizes);
                } else {
                    child.height += extra;
                    child.content_height += extra;
                }
            }
        }
    }

    // Third pass: position children
    let content_x = padding_left + border_left;
    let content_y = padding_top + border_top;

    // Calculate starting position based on justify-content
    let final_main_size: f32 = layout_box.children.iter().map(|c| {
        if is_row { c.width } else { c.height }
    }).sum();
    let remaining = available - final_main_size;

    let (mut main_cursor, gap_between) = match style.justify_content {
        JustifyContent::FlexStart => (0.0_f32, style.gap),
        JustifyContent::FlexEnd => (remaining.max(0.0), style.gap),
        JustifyContent::Center => (remaining.max(0.0) / 2.0, style.gap),
        JustifyContent::SpaceBetween => {
            let n = layout_box.children.len() as f32;
            if n > 1.0 {
                (0.0, remaining.max(0.0) / (n - 1.0))
            } else {
                (0.0, 0.0)
            }
        }
        JustifyContent::SpaceAround => {
            let n = layout_box.children.len() as f32;
            let space = remaining.max(0.0) / n;
            (space / 2.0, space)
        }
        JustifyContent::SpaceEvenly => {
            let n = layout_box.children.len() as f32;
            let space = remaining.max(0.0) / (n + 1.0);
            (space, space)
        }
    };

    let mut max_cross: f32 = 0.0;
    let num_children = layout_box.children.len();

    for (i, child) in layout_box.children.iter_mut().enumerate() {
        let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
        if is_row {
            child.x = content_x + main_cursor + child_style.margin_left;
            child.y = content_y + child_style.margin_top;
            main_cursor += child.width + child_style.margin_left + child_style.margin_right;
            if i < num_children - 1 {
                main_cursor += gap_between;
            }
            max_cross = max_cross.max(
                child.height + child_style.margin_top + child_style.margin_bottom,
            );
        } else {
            child.x = content_x + child_style.margin_left;
            child.y = content_y + main_cursor + child_style.margin_top;
            main_cursor += child.height + child_style.margin_top + child_style.margin_bottom;
            if i < num_children - 1 {
                main_cursor += gap_between;
            }
            max_cross = max_cross.max(
                child.width + child_style.margin_left + child_style.margin_right,
            );
        }
    }

    // Calculate height
    let content_height = match style.height {
        SizeValue::Px(h) => h,
        _ => {
            if is_row {
                max_cross
            } else {
                main_cursor
            }
        }
    };

    // Apply min-height for flex containers (e.g. min-height: 100vh)
    let content_height = match style.min_height {
        SizeValue::Px(mh) if content_height < mh => mh,
        _ => content_height,
    };

    layout_box.content_height = content_height.max(0.0);
    layout_box.height = content_height + padding_top + padding_bottom + border_top + border_bottom;

    // Cross-axis alignment for row flex
    if is_row {
        let cross_size = layout_box.content_height;
        for child in &mut layout_box.children {
            let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
            match style.align_items {
                AlignItems::Center => {
                    child.y = content_y + (cross_size - child.height) / 2.0;
                }
                AlignItems::FlexEnd => {
                    child.y = content_y + cross_size - child.height - child_style.margin_bottom;
                }
                AlignItems::Stretch => {
                    child.height = cross_size - child_style.margin_top - child_style.margin_bottom;
                }
                _ => {} // FlexStart and Baseline keep default position
            }
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
            containing_width - style.margin_left - style.margin_right
                - padding_left - padding_right - border_left - border_right
        }
    };
    let content_width = content_width.max(0.0);

    let num_children = layout_box.children.len();
    if num_children == 0 {
        layout_box.content_width = content_width;
        layout_box.width = content_width + padding_left + padding_right + border_left + border_right;
        layout_box.content_height = 0.0;
        layout_box.height = padding_top + padding_bottom + border_top + border_bottom;
        return;
    }

    let col_gap = style.column_gap;
    let row_gap = style.row_gap;

    // Determine column count and widths
    let num_cols = if style.grid_template_columns.is_empty() {
        // No explicit columns: single column
        1
    } else {
        style.grid_template_columns.len()
    };

    let col_widths = if style.grid_template_columns.is_empty() {
        vec![content_width]
    } else {
        resolve_track_sizes(&style.grid_template_columns, content_width, col_gap)
    };

    // Determine number of rows needed
    let num_rows = (num_children + num_cols - 1) / num_cols;

    // Resolve explicit row heights (or default to auto)
    let explicit_row_tracks = &style.grid_template_rows;

    // First pass: lay out children to determine auto row heights
    // Place children left-to-right, top-to-bottom (auto-placement)
    let content_x = padding_left + border_left;
    let content_y = padding_top + border_top;

    // Compute natural heights per row by laying out each child into its column width
    let mut row_heights = vec![0.0_f32; num_rows];
    for (idx, child) in layout_box.children.iter_mut().enumerate() {
        let col = idx % num_cols;
        let row = idx / num_cols;
        let cell_width = col_widths[col];

        compute_layout(child, styles, cell_width, 0.0, image_sizes);

        let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
        let child_h = child.height + child_style.margin_top + child_style.margin_bottom;
        row_heights[row] = row_heights[row].max(child_h);
    }

    // Override with explicit row track sizes where given
    for (r, rh) in row_heights.iter_mut().enumerate() {
        if r < explicit_row_tracks.len() {
            match explicit_row_tracks[r] {
                GridTrackSize::Px(px) => *rh = px,
                GridTrackSize::Percent(p) => *rh = content_width * p / 100.0, // approx
                GridTrackSize::Auto => {} // keep auto-computed height
                GridTrackSize::Fr(_) => {} // fr rows not fully supported; keep auto
                GridTrackSize::MinMax(min, _) => {
                    if *rh < min {
                        *rh = min;
                    }
                }
            }
        }
    }

    // Second pass: position each child in its cell
    for (idx, child) in layout_box.children.iter_mut().enumerate() {
        let col = idx % num_cols;
        let row = idx / num_cols;

        // Compute x position: sum of previous column widths + gaps
        let cell_x: f32 = (0..col).map(|c| col_widths[c]).sum::<f32>() + col as f32 * col_gap;
        // Compute y position: sum of previous row heights + gaps
        let cell_y: f32 = (0..row).map(|r| row_heights[r]).sum::<f32>() + row as f32 * row_gap;

        let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
        child.x = content_x + cell_x + child_style.margin_left;
        child.y = content_y + cell_y + child_style.margin_top;

        // Stretch child width to fill cell if it's a block-level child
        let cell_width = col_widths[col];
        if child.width < cell_width {
            child.width = cell_width - child_style.margin_left - child_style.margin_right;
            child.content_width = child.width - child_style.padding_left - child_style.padding_right
                - child_style.border_left_width - child_style.border_right_width;
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

    layout_box.content_width = content_width;
    layout_box.width = content_width + padding_left + padding_right + border_left + border_right;
    layout_box.content_height = content_height.max(0.0);
    layout_box.height = content_height + padding_top + padding_bottom + border_top + border_bottom;
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

fn layout_text(layout_box: &mut LayoutBox, styles: &StyleMap, containing_width: f32) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();
    let text = layout_box.text.as_deref().unwrap_or("");

    if text.is_empty() {
        layout_box.width = 0.0;
        layout_box.height = 0.0;
        return;
    }

    // Simple text measurement: approximate character width
    let char_width = style.font_size * 0.52; // Proportional font average width
    let line_height = style.font_size * style.line_height;

    // Single space node (whitespace between inline elements)
    if text == " " {
        layout_box.content_width = char_width;
        layout_box.content_height = line_height;
        layout_box.width = char_width;
        layout_box.height = line_height;
        return;
    }

    // Word wrap
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        layout_box.width = char_width; // At least one space width
        layout_box.height = line_height;
        return;
    }

    let mut lines = 1u32;
    let mut current_line_width: f32 = 0.0;
    let space_width = char_width;
    let mut max_line_width: f32 = 0.0;

    // Account for leading space
    let has_leading = text.starts_with(' ');
    if has_leading {
        current_line_width = space_width;
    }

    for (i, word) in words.iter().enumerate() {
        let word_width = word.len() as f32 * char_width;
        let needed = if i == 0 {
            word_width
        } else {
            space_width + word_width
        };

        if current_line_width + needed > containing_width + 0.5 && current_line_width > 0.0 {
            max_line_width = max_line_width.max(current_line_width);
            lines += 1;
            current_line_width = word_width;
        } else {
            current_line_width += needed;
        }
    }

    // Account for trailing space
    let has_trailing = text.ends_with(' ');
    if has_trailing {
        current_line_width += space_width;
    }

    max_line_width = max_line_width.max(current_line_width);

    layout_box.content_width = max_line_width;
    layout_box.content_height = lines as f32 * line_height;
    layout_box.width = max_line_width;
    layout_box.height = lines as f32 * line_height;
}

fn layout_image(layout_box: &mut LayoutBox, styles: &StyleMap, containing_width: f32, image_sizes: &ImageSizes) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    // Try to get actual image dimensions from the cache
    let actual_dims = layout_box.image_src.as_ref().and_then(|src| image_sizes.get(src));

    // If we don't have the actual image data, collapse to 0 size regardless of HTML attributes
    if actual_dims.is_none() && !layout_box.image_src.as_deref().unwrap_or("").starts_with("__canvas__") {
        layout_box.width = 0.0;
        layout_box.height = 0.0;
        layout_box.content_width = 0.0;
        layout_box.content_height = 0.0;
        return;
    }

    let w = match style.width {
        SizeValue::Px(w) => w,
        SizeValue::Percent(p) => containing_width * p / 100.0,
        _ => {
            actual_dims.map(|(w, _)| *w as f32).unwrap_or(0.0)
        }
    };
    let h = match style.height {
        SizeValue::Px(h) => h,
        _ => {
            if let Some((iw, ih)) = actual_dims {
                if style.width != SizeValue::Auto && style.width != SizeValue::None && *iw > 0 {
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
pub fn flatten_layout(layout_box: &LayoutBox, offset_x: f32, offset_y: f32) -> Vec<FlatBox> {
    let mut result = Vec::new();
    let abs_x = offset_x + layout_box.x;
    let abs_y = offset_y + layout_box.y;

    if layout_box.box_type != BoxType::None {
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
        });
    }

    // Propagate parent link_href to children
    let parent_href = layout_box.link_href.clone();
    for child in &layout_box.children {
        let mut child_boxes = flatten_layout(child, abs_x, abs_y);
        if let Some(ref href) = parent_href {
            for fb in &mut child_boxes {
                if fb.link_href.is_none() {
                    fb.link_href = Some(href.clone());
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
        let styles = incognidium_style::resolve_styles(&doc, &stylesheet);
        let root = layout(&doc, &styles, 800.0, 600.0);

        assert!(root.width > 0.0);
        assert!(root.height > 0.0);

        let flat = flatten_layout(&root, 0.0, 0.0);
        assert!(!flat.is_empty());
    }
}
