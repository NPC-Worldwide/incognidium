use incognidium_dom::{Document, NodeData, NodeId};
use incognidium_style::{
    AlignItems, Display, FlexDirection, JustifyContent, SizeValue, StyleMap,
};

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
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BoxType {
    Block,
    Inline,
    Flex,
    Text,
    None,
}

/// Build the layout tree and compute positions.
pub fn layout(doc: &Document, styles: &StyleMap, viewport_width: f32, viewport_height: f32) -> LayoutBox {
    let root_id = doc.root();
    let mut root_box = build_layout_tree(doc, styles, root_id);
    root_box.width = viewport_width;
    compute_layout(&mut root_box, styles, viewport_width, viewport_height);
    root_box
}

fn build_layout_tree(doc: &Document, styles: &StyleMap, node_id: NodeId) -> LayoutBox {
    let node = doc.node(node_id);
    let style = styles.get(&node_id);

    let display = style.map(|s| s.display).unwrap_or(Display::Block);

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
        };
    }

    let (box_type, text) = match &node.data {
        NodeData::Text(t) => {
            let trimmed = t.content.trim();
            if trimmed.is_empty() {
                (BoxType::None, None)
            } else {
                (BoxType::Text, Some(t.content.clone()))
            }
        }
        NodeData::Element(_) => match display {
            Display::Block | Display::InlineBlock => (BoxType::Block, None),
            Display::Inline => (BoxType::Inline, None),
            Display::Flex => (BoxType::Flex, None),
            Display::None => (BoxType::None, None),
        },
        _ => (BoxType::Block, None),
    };

    let children: Vec<LayoutBox> = node
        .children
        .iter()
        .map(|&child_id| build_layout_tree(doc, styles, child_id))
        .filter(|b| b.box_type != BoxType::None)
        .collect();

    LayoutBox {
        node_id,
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
        content_width: 0.0,
        content_height: 0.0,
        children,
        box_type,
        text,
    }
}

fn compute_layout(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    _containing_height: f32,
) {
    match layout_box.box_type {
        BoxType::Block | BoxType::Inline => {
            layout_block(layout_box, styles, containing_width);
        }
        BoxType::Flex => {
            layout_flex(layout_box, styles, containing_width);
        }
        BoxType::Text => {
            layout_text(layout_box, styles, containing_width);
        }
        BoxType::None => {}
    }
}

fn layout_block(layout_box: &mut LayoutBox, styles: &StyleMap, containing_width: f32) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    // Calculate width
    let margin_left = style.margin_left;
    let margin_right = style.margin_right;
    let padding_left = style.padding_left;
    let padding_right = style.padding_right;
    let border_left = style.border_left_width;
    let border_right = style.border_right_width;

    let content_width = match style.width {
        SizeValue::Px(w) => w,
        SizeValue::Percent(p) => containing_width * p / 100.0,
        SizeValue::Auto | SizeValue::None => {
            containing_width - margin_left - margin_right - padding_left - padding_right
                - border_left - border_right
        }
    };

    layout_box.content_width = content_width.max(0.0);
    layout_box.width = content_width + padding_left + padding_right + border_left + border_right;

    // Layout children
    let child_containing_width = layout_box.content_width;
    let mut cursor_y: f32 = style.padding_top + style.border_top_width;
    let content_x = padding_left + border_left;

    // Separate inline and block children
    let mut i = 0;
    while i < layout_box.children.len() {
        let child = &layout_box.children[i];

        if child.box_type == BoxType::Text || child.box_type == BoxType::Inline {
            // Inline/text: lay out horizontally on a line
            let line_start = i;
            let mut line_x = content_x;
            let mut line_height: f32 = 0.0;

            while i < layout_box.children.len() {
                let c = &layout_box.children[i];
                if c.box_type != BoxType::Text && c.box_type != BoxType::Inline {
                    break;
                }
                compute_layout(
                    &mut layout_box.children[i],
                    styles,
                    child_containing_width,
                    0.0,
                );
                i += 1;
            }

            // Position inline children on a line
            line_x = content_x;
            for j in line_start..i {
                let c = &mut layout_box.children[j];
                // Simple line breaking: if it doesn't fit, wrap
                if line_x + c.width > content_x + child_containing_width && line_x > content_x {
                    cursor_y += line_height;
                    line_x = content_x;
                    line_height = 0.0;
                }
                c.x = line_x;
                c.y = cursor_y;
                line_x += c.width;
                line_height = line_height.max(c.height);
            }
            cursor_y += line_height;
        } else {
            // Block child
            let cm = styles
                .get(&child.node_id)
                .cloned()
                .unwrap_or_default();
            compute_layout(
                &mut layout_box.children[i],
                styles,
                child_containing_width,
                0.0,
            );
            layout_box.children[i].x = content_x + cm.margin_left;
            layout_box.children[i].y = cursor_y + cm.margin_top;
            cursor_y += cm.margin_top + layout_box.children[i].height + cm.margin_bottom;
            i += 1;
        }
    }

    // Calculate height
    let content_height = match style.height {
        SizeValue::Px(h) => h,
        _ => cursor_y - style.padding_top - style.border_top_width,
    };

    layout_box.content_height = content_height.max(0.0);
    layout_box.height =
        content_height + style.padding_top + style.padding_bottom + style.border_top_width
            + style.border_bottom_width;
}

fn layout_flex(layout_box: &mut LayoutBox, styles: &StyleMap, containing_width: f32) {
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
            compute_layout(child, styles, basis.max(50.0), 0.0);
        } else {
            compute_layout(child, styles, content_width, 0.0);
        }
    }

    // Second pass: distribute space according to flex-grow
    let total_main_size: f32 = layout_box.children.iter().map(|c| {
        if is_row { c.width } else { c.height }
    }).sum();

    let gap_total = style.gap * (layout_box.children.len().saturating_sub(1) as f32);
    let available = if is_row { content_width } else { 10000.0 } - gap_total;
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
                    compute_layout(child, styles, child.content_width, 0.0);
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

fn layout_text(layout_box: &mut LayoutBox, styles: &StyleMap, containing_width: f32) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();
    let text = layout_box.text.as_deref().unwrap_or("");
    let trimmed = text.trim();

    if trimmed.is_empty() {
        layout_box.width = 0.0;
        layout_box.height = 0.0;
        return;
    }

    // Simple text measurement: approximate character width
    let char_width = style.font_size * 0.6; // Approximate monospace-ish width
    let line_height = style.font_size * style.line_height;

    // Word wrap
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    let mut lines = 1u32;
    let mut current_line_width: f32 = 0.0;
    let space_width = char_width;
    let mut max_line_width: f32 = 0.0;

    for (i, word) in words.iter().enumerate() {
        let word_width = word.len() as f32 * char_width;
        let needed = if i == 0 {
            word_width
        } else {
            space_width + word_width
        };

        if current_line_width + needed > containing_width && current_line_width > 0.0 {
            max_line_width = max_line_width.max(current_line_width);
            lines += 1;
            current_line_width = word_width;
        } else {
            current_line_width += needed;
        }
    }
    max_line_width = max_line_width.max(current_line_width);

    layout_box.content_width = max_line_width;
    layout_box.content_height = lines as f32 * line_height;
    layout_box.width = max_line_width;
    layout_box.height = lines as f32 * line_height;
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
        });
    }

    for child in &layout_box.children {
        result.extend(flatten_layout(child, abs_x, abs_y));
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
