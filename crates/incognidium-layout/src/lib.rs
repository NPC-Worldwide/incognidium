use std::collections::HashMap;
use incognidium_dom::{Document, NodeData, NodeId};

/// Float state passed from parent blocks to child blocks.
#[derive(Clone, Copy, Default)]
pub struct FloatState {
    pub left_width: f32,
    pub right_width: f32,
    pub remaining_height: f32,
}
use incognidium_style::{
    AlignItems, Display, Float, FlexDirection, FlexWrap, GridTrackSize, JustifyContent,
    Overflow, Position, SizeValue, StyleMap, TextAlign,
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
    /// Float indent: (indent_px, num_indented_lines, is_left_float)
    /// Paint uses this to offset the first N lines of text.
    pub float_text_indent: Option<(f32, u32, bool)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BoxType {
    Block,
    InlineBlock,
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
                    Display::Block => (BoxType::Block, None, None),
                    Display::InlineBlock => (BoxType::InlineBlock, None, None),
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

    // Add list bullet/number markers for <li> elements (respect list-style-type)
    if let NodeData::Element(ref el) = node.data {
        if el.tag_name == "li" && styles.get(&node_id).map(|s| s.list_style_type) != Some(incognidium_style::ListStyleType::None) {
            let marker_type = styles.get(&node_id).map(|s| s.list_style_type).unwrap_or(incognidium_style::ListStyleType::Disc);
            let marker = if let Some(parent_id) = node.parent {
                let parent_node = doc.node(parent_id);
                let is_ordered = matches!(marker_type, incognidium_style::ListStyleType::Decimal)
                    || matches!(&parent_node.data, NodeData::Element(ref pel) if pel.tag_name == "ol");
                if is_ordered {
                    let idx = parent_node.children.iter()
                        .filter(|&&cid| {
                            matches!(&doc.node(cid).data, NodeData::Element(ref e) if e.tag_name == "li")
                        })
                        .position(|&cid| cid == node_id)
                        .unwrap_or(0);
                    format!("{}. ", idx + 1)
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
                float_text_indent: None,
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

    let effective_box_type = if (box_type == BoxType::Block || box_type == BoxType::InlineBlock || box_type == BoxType::Flex || box_type == BoxType::Grid || box_type == BoxType::Inline)
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
    }
}

fn compute_layout(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    _containing_height: f32,
    image_sizes: &ImageSizes,
) {
    compute_layout_with_floats(layout_box, styles, containing_width, _containing_height, image_sizes, FloatState::default());
}

fn compute_layout_with_floats(
    layout_box: &mut LayoutBox,
    styles: &StyleMap,
    containing_width: f32,
    _containing_height: f32,
    image_sizes: &ImageSizes,
    parent_floats: FloatState,
) {
    match layout_box.box_type {
        BoxType::Block => {
            layout_block(layout_box, styles, containing_width, image_sizes, parent_floats);
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
        BoxType::Text => {
            layout_text(layout_box, styles, containing_width);
        }
        BoxType::Image => {
            layout_image(layout_box, styles, containing_width, image_sizes);
        }
        BoxType::None => {}
    }
}

fn layout_block(layout_box: &mut LayoutBox, styles: &StyleMap, containing_width: f32, image_sizes: &ImageSizes, parent_floats: FloatState) {
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

    let mut float_right_width: f32 = parent_floats.right_width;
    let mut float_left_width: f32 = parent_floats.left_width;
    let mut float_bottom: f32 = if parent_floats.remaining_height > 0.0 {
        style.padding_top + style.border_top_width + parent_floats.remaining_height
    } else {
        0.0
    };

    // Collect indices of absolutely positioned children
    // All absolute/fixed positioned elements are removed from normal flow
    let abs_indices: Vec<usize> = layout_box.children.iter().enumerate()
        .filter(|(_, c)| {
            let cs = styles.get(&c.node_id).cloned().unwrap_or_default();
            cs.position == Position::Absolute || cs.position == Position::Fixed
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
            let mut inline_available = child_containing_width - float_right_width - float_left_width;
            let mut inline_x_start = content_x + float_left_width;

            let line_start = i;
            let mut line_height: f32 = 0.0;

            while i < layout_box.children.len() {
                let c = &layout_box.children[i];
                if !is_inline_level_styled(c.box_type, styles, c.node_id) {
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

                let child_width = layout_box.children[j].width;
                let child_height = layout_box.children[j].height;
                // Line breaking with float-aware width
                if line_x + child_width > inline_x_start + inline_available + 0.5 && line_x > inline_x_start {
                    apply_text_align(&mut layout_box.children, line_begin, j, line_x - inline_x_start, inline_available, &style);
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
                let float_content_width = match cm.width {
                    SizeValue::Px(w) => w,
                    SizeValue::Percent(p) => child_containing_width * p / 100.0 - cm.margin_left - cm.margin_right,
                    _ => {
                        // Auto width: compute at generous width, then
                        // shrink-wrap to the content_width (intrinsic).
                        compute_layout(
                            &mut layout_box.children[i],
                            styles,
                            child_containing_width - cm.margin_left - cm.margin_right,
                            0.0,
                            image_sizes,
                        );
                        // Find the tightest max-width among all descendants.
                        fn find_min_max_width(lb: &LayoutBox, styles: &StyleMap) -> Option<f32> {
                            let mut result: Option<f32> = None;
                            let st = styles.get(&lb.node_id).cloned().unwrap_or_default();
                            if let SizeValue::Px(mw) = st.max_width {
                                result = Some(mw + st.padding_left + st.padding_right
                                    + st.border_left_width + st.border_right_width);
                            }
                            for c in &lb.children {
                                if let Some(cmw) = find_min_max_width(c, styles) {
                                    result = Some(result.map(|r| r.min(cmw)).unwrap_or(cmw));
                                }
                            }
                            result
                        }
                        let child_ref = &layout_box.children[i];
                        if let Some(mw) = find_min_max_width(child_ref, styles) {
                            mw
                        } else {
                            child_ref.content_width.min(child_ref.width)
                        }
                    }
                };
                compute_layout(
                    &mut layout_box.children[i],
                    styles,
                    float_content_width,
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
                    &mut layout_box.children[i], styles, effective_width, 0.0, image_sizes, pf,
                );
            } else {
                compute_layout(
                    &mut layout_box.children[i], styles, effective_width, 0.0, image_sizes,
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
                // Center blocks that are narrower than container (auto margin behavior)
                let child_w = layout_box.children[i].width;
                let extra = (effective_width - child_w).max(0.0);
                let x_offset = if child_w < effective_width && extra > 1.0 {
                    // Center if the element has a non-auto width AND auto-ish margins
                    // (i.e. it's not a full-width block)
                    let width_fixed = !matches!(cm.width, SizeValue::Auto | SizeValue::None);
                    if width_fixed {
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

    // Calculate height — must encompass floated children (block formatting context)
    let mut auto_height = cursor_y - style.padding_top - style.border_top_width;
    // Floats and absolutely positioned children can extend below the last block child;
    // the parent must contain them (creates a BFC for overflow:hidden or when it has floats)
    let auto_content_bottom = auto_height + style.padding_top + style.border_top_width;
    for child in &layout_box.children {
        let cs = styles.get(&child.node_id).cloned().unwrap_or_default();
        if cs.float != Float::None {
            let child_bottom = child.y + child.height + cs.margin_bottom;
            if child_bottom > auto_content_bottom {
                auto_height += child_bottom - auto_content_bottom;
            }
        }
    }
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

    // Position absolutely/fixed positioned children
    let container_w = layout_box.width;
    let container_h = layout_box.height;
    for &idx in &abs_indices {
        let child = &mut layout_box.children[idx];
        let cs = styles.get(&child.node_id).cloned().unwrap_or_default();

        // Compute their layout with container width
        let abs_width = match cs.width {
            SizeValue::Px(w) => w,
            SizeValue::Percent(p) => container_w * p / 100.0,
            _ => container_w - cs.margin_left - cs.margin_right
                - cs.padding_left - cs.padding_right
                - cs.border_left_width - cs.border_right_width,
        };
        compute_layout(child, styles, abs_width, container_h, image_sizes);

        // Apply top/left/right/bottom
        child.x = match cs.left {
            SizeValue::Px(v) => v + cs.margin_left,
            SizeValue::Percent(p) => container_w * p / 100.0 + cs.margin_left,
            _ => match cs.right {
                SizeValue::Px(v) => (container_w - child.width - v - cs.margin_right).max(0.0),
                SizeValue::Percent(p) => (container_w - child.width - container_w * p / 100.0).max(0.0),
                _ => cs.margin_left,
            },
        };
        child.y = match cs.top {
            SizeValue::Px(v) => v + cs.margin_top,
            SizeValue::Percent(p) => container_h * p / 100.0 + cs.margin_top,
            _ => match cs.bottom {
                SizeValue::Px(v) => (container_h - child.height - v - cs.margin_bottom).max(0.0),
                SizeValue::Percent(p) => (container_h - child.height - container_h * p / 100.0).max(0.0),
                _ => cs.margin_top,
            },
        };
    }
}

/// Layout an inline-block element: establishes a block formatting context but
/// shrinks to fit its content width instead of expanding to the containing width.
fn layout_inline_block(layout_box: &mut LayoutBox, styles: &StyleMap, containing_width: f32, image_sizes: &ImageSizes) {
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

    // Check if width is explicitly set
    let explicit_width = match style.width {
        SizeValue::Px(w) => {
            Some(if is_border_box {
                (w - padding_left - padding_right - border_left - border_right).max(0.0)
            } else {
                w
            })
        }
        SizeValue::Percent(p) => {
            let total = containing_width * p / 100.0;
            Some(if is_border_box {
                (total - padding_left - padding_right - border_left - border_right).max(0.0)
            } else {
                total
            })
        }
        SizeValue::Auto | SizeValue::None => None,
    };

    if let Some(content_width) = explicit_width {
        // Explicit width: behave like a block with that width
        let mut content_width = content_width;

        // Apply max-width
        match style.max_width {
            SizeValue::Px(mw) => { if content_width > mw { content_width = mw; } }
            SizeValue::Percent(p) => { let mw = containing_width * p / 100.0; if content_width > mw { content_width = mw; } }
            _ => {}
        }
        // Apply min-width
        match style.min_width {
            SizeValue::Px(mw) => { if content_width < mw { content_width = mw; } }
            SizeValue::Percent(p) => { let mw = containing_width * p / 100.0; if content_width < mw { content_width = mw; } }
            _ => {}
        }

        layout_box.content_width = content_width.max(0.0);
        layout_box.width = content_width + padding_left + padding_right + border_left + border_right;

        // Layout children as a block formatting context
        let child_containing = layout_box.content_width;
        let mut cursor_y: f32 = padding_top + border_top;
        let content_x = padding_left + border_left;

        for child in &mut layout_box.children {
            compute_layout(child, styles, child_containing, 0.0, image_sizes);
            let cm = styles.get(&child.node_id).cloned().unwrap_or_default();
            if child.height > 0.0 {
                child.x = content_x + cm.margin_left;
                child.y = cursor_y + cm.margin_top;
                cursor_y += cm.margin_top + child.height + cm.margin_bottom;
            }
        }

        let auto_height = cursor_y - padding_top - border_top;
        let content_height = match style.height {
            SizeValue::Px(h) => h,
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
        layout_box.height = content_height + padding_top + padding_bottom + border_top + border_bottom;
    } else {
        // Auto width: shrink-to-fit
        // Layout children with the max available width first to get their natural sizes
        let max_available = containing_width - margin_left - margin_right - padding_left
            - padding_right - border_left - border_right;

        let content_x = padding_left + border_left;
        let mut cursor_y: f32 = padding_top + border_top;
        let mut max_child_width: f32 = 0.0;

        for child in &mut layout_box.children {
            compute_layout(child, styles, max_available.max(0.0), 0.0, image_sizes);
            let cm = styles.get(&child.node_id).cloned().unwrap_or_default();
            if child.height > 0.0 {
                child.x = content_x + cm.margin_left;
                child.y = cursor_y + cm.margin_top;
                cursor_y += cm.margin_top + child.height + cm.margin_bottom;
            }
            max_child_width = max_child_width.max(child.width + cm.margin_left + cm.margin_right);
        }

        // Shrink to fit: use the widest child, clamped by available space
        let mut content_width = max_child_width.min(max_available.max(0.0));

        // Apply max-width
        match style.max_width {
            SizeValue::Px(mw) => { if content_width > mw { content_width = mw; } }
            SizeValue::Percent(p) => { let mw = containing_width * p / 100.0; if content_width > mw { content_width = mw; } }
            _ => {}
        }
        // Apply min-width
        match style.min_width {
            SizeValue::Px(mw) => { if content_width < mw { content_width = mw; } }
            SizeValue::Percent(p) => { let mw = containing_width * p / 100.0; if content_width < mw { content_width = mw; } }
            _ => {}
        }

        layout_box.content_width = content_width.max(0.0);
        layout_box.width = content_width + padding_left + padding_right + border_left + border_right;

        let auto_height = cursor_y - padding_top - border_top;
        let content_height = match style.height {
            SizeValue::Px(h) => h,
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
        layout_box.height = content_height + padding_top + padding_bottom + border_top + border_bottom;
    }
}

/// Check if a box type participates in inline flow.
fn is_inline_level(box_type: BoxType) -> bool {
    matches!(box_type, BoxType::Text | BoxType::Inline | BoxType::InlineBlock)
}

fn is_inline_level_styled(box_type: BoxType, styles: &StyleMap, node_id: NodeId) -> bool {
    if matches!(box_type, BoxType::Text | BoxType::Inline | BoxType::InlineBlock) {
        return true;
    }
    if box_type == BoxType::Image {
        let display = styles.get(&node_id).map(|s| s.display).unwrap_or(Display::InlineBlock);
        return display != Display::Block;
    }
    false
}

/// Compute inter-element gap to prevent text concatenation like "wordword".
/// Returns a Vec of gap values to add before each child.
fn compute_inline_gaps(children: &[LayoutBox], start: usize, end: usize, styles: &StyleMap) -> Vec<f32> {
    // Use parent font size to compute accurate space width
    let parent_font_size = children.get(start)
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
    let gaps = compute_inline_gaps(&layout_box.children, 0, num_children, styles);

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

    let wrapping = style.flex_wrap != FlexWrap::NoWrap;

    // Container main-axis size for wrapping decisions
    let container_main = if is_row { content_width } else {
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
    let abs_child_ids: Vec<NodeId> = layout_box.children.iter()
        .filter(|c| {
            let cs = styles.get(&c.node_id).cloned().unwrap_or_default();
            cs.position == Position::Absolute || cs.position == Position::Fixed
        })
        .map(|c| c.node_id)
        .collect();

    // Sort children by CSS order property (stable sort preserves source order for same value)
    layout_box.children.sort_by_key(|child| {
        styles.get(&child.node_id).map(|s| s.order).unwrap_or(0)
    });

    // First pass: compute natural sizes of non-absolute children
    let num_children = layout_box.children.iter()
        .filter(|c| !abs_child_ids.contains(&c.node_id))
        .count();
    for child in &mut layout_box.children {
        if abs_child_ids.contains(&child.node_id) { continue; }
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
                // Split container width proportionally among children;
                // flex-grow redistributes remaining space.
                let n = num_children.max(1) as f32;
                (content_width / n).max(20.0)
            };
            compute_layout(child, styles, initial_width, 0.0, image_sizes);
        } else {
            compute_layout(child, styles, content_width, 0.0, image_sizes);
        }
    }

    // Group children into flex lines
    // Each line is a range [start, end) of child indices
    let mut lines: Vec<(usize, usize)> = Vec::new();
    if wrapping && num_children > 0 {
        let mut line_start = 0;
        let mut line_main_used = 0.0_f32;
        for i in 0..num_children {
            let child = &layout_box.children[i];
            let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
            let child_main = if is_row {
                child.width + child_style.margin_left + child_style.margin_right
            } else {
                child.height + child_style.margin_top + child_style.margin_bottom
            };
            let gap_before = if i > line_start { style.gap } else { 0.0 };

            if i > line_start && line_main_used + gap_before + child_main > container_main + 0.5 {
                // This item overflows; start a new line
                lines.push((line_start, i));
                line_start = i;
                line_main_used = child_main;
            } else {
                line_main_used += gap_before + child_main;
            }
        }
        lines.push((line_start, num_children));
    } else {
        // NoWrap: everything on one line
        if num_children > 0 {
            lines.push((0, num_children));
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

        // Compute total main size and total flex-grow for this line
        let line_main_size: f32 = (line_start..line_end).map(|i| {
            let c = &layout_box.children[i];
            if is_row { c.width } else { c.height }
        }).sum();

        let line_gap_total = style.gap * (line_count.saturating_sub(1) as f32);

        let line_available = if is_row {
            content_width
        } else {
            match style.height {
                SizeValue::Px(h) => h,
                _ => match style.min_height {
                    SizeValue::Px(mh) => mh,
                    _ => line_main_size, // auto height = no free space
                }
            }
        } - line_gap_total;

        let line_free = (line_available - line_main_size).max(0.0);

        let line_total_grow: f32 = (line_start..line_end).map(|i| {
            styles.get(&layout_box.children[i].node_id)
                .map(|s| s.flex_grow)
                .unwrap_or(0.0)
        }).sum();

        // Distribute flex-grow within this line
        if line_total_grow > 0.0 && line_free > 0.0 {
            for i in line_start..line_end {
                let grow = styles.get(&layout_box.children[i].node_id)
                    .map(|s| s.flex_grow)
                    .unwrap_or(0.0);
                if grow > 0.0 {
                    let extra = line_free * (grow / line_total_grow);
                    if is_row {
                        layout_box.children[i].width += extra;
                        layout_box.children[i].content_width += extra;
                        // Re-layout children with new width
                        let cw = layout_box.children[i].content_width;
                        compute_layout(&mut layout_box.children[i], styles, cw, 0.0, image_sizes);
                    } else {
                        layout_box.children[i].height += extra;
                        layout_box.children[i].content_height += extra;
                    }
                }
            }
        }

        // Handle flex-shrink when items overflow the line (only for NoWrap or when line has one item)
        if !wrapping || line_count == 1 {
            let line_main_after_grow: f32 = (line_start..line_end).map(|i| {
                let c = &layout_box.children[i];
                if is_row { c.width } else { c.height }
            }).sum();
            let overflow = line_main_after_grow + line_gap_total - (if is_row { content_width } else {
                match style.height {
                    SizeValue::Px(h) => h,
                    _ => line_main_after_grow, // auto = no overflow
                }
            });
            if overflow > 0.0 {
                let line_total_shrink: f32 = (line_start..line_end).map(|i| {
                    styles.get(&layout_box.children[i].node_id)
                        .map(|s| s.flex_shrink)
                        .unwrap_or(1.0)
                }).sum();
                if line_total_shrink > 0.0 {
                    for i in line_start..line_end {
                        let shrink = styles.get(&layout_box.children[i].node_id)
                            .map(|s| s.flex_shrink)
                            .unwrap_or(1.0);
                        if shrink > 0.0 {
                            let reduction = overflow * (shrink / line_total_shrink);
                            if is_row {
                                layout_box.children[i].width = (layout_box.children[i].width - reduction).max(0.0);
                                layout_box.children[i].content_width = (layout_box.children[i].content_width - reduction).max(0.0);
                                let cw = layout_box.children[i].content_width;
                                compute_layout(&mut layout_box.children[i], styles, cw, 0.0, image_sizes);
                            } else {
                                layout_box.children[i].height = (layout_box.children[i].height - reduction).max(0.0);
                                layout_box.children[i].content_height = (layout_box.children[i].content_height - reduction).max(0.0);
                            }
                        }
                    }
                }
            }
        }

        // Position items on this line
        let final_line_main: f32 = (line_start..line_end).map(|i| {
            let c = &layout_box.children[i];
            if is_row { c.width } else { c.height }
        }).sum();
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
        for i in line_start..line_end {
            let child_style = styles.get(&layout_box.children[i].node_id).cloned().unwrap_or_default();
            if is_row {
                layout_box.children[i].x = content_x + main_cursor + child_style.margin_left;
                layout_box.children[i].y = content_y + cross_cursor + child_style.margin_top;
                main_cursor += layout_box.children[i].width + child_style.margin_left + child_style.margin_right;
                if i < line_end - 1 {
                    main_cursor += gap_between;
                }
                line_max_cross = line_max_cross.max(
                    layout_box.children[i].height + child_style.margin_top + child_style.margin_bottom,
                );
            } else {
                layout_box.children[i].x = content_x + cross_cursor + child_style.margin_left;
                layout_box.children[i].y = content_y + main_cursor + child_style.margin_top;
                main_cursor += layout_box.children[i].height + child_style.margin_top + child_style.margin_bottom;
                if i < line_end - 1 {
                    main_cursor += gap_between;
                }
                line_max_cross = line_max_cross.max(
                    layout_box.children[i].width + child_style.margin_left + child_style.margin_right,
                );
            }
        }

        line_cross_sizes.push(line_max_cross);
        cross_cursor += line_max_cross;
    }

    // Calculate total cross-axis size from all lines
    let total_cross: f32 = line_cross_sizes.iter().sum();

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
                    let line_main: f32 = (line_start..line_end).map(|i| {
                        let cs = styles.get(&layout_box.children[i].node_id).cloned().unwrap_or_default();
                        layout_box.children[i].height + cs.margin_top + cs.margin_bottom
                    }).sum();
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

    layout_box.content_height = content_height.max(0.0);
    layout_box.height = content_height + padding_top + padding_bottom + border_top + border_bottom;

    // For wrapping column flex, adjust container width to fit all lines
    if !is_row && wrapping && lines.len() > 1 {
        let total_line_cross: f32 = line_cross_sizes.iter().sum();
        if total_line_cross > content_width {
            layout_box.content_width = total_line_cross;
            layout_box.width = total_line_cross + padding_left + padding_right + border_left + border_right;
        }
    }

    // For row flex, update content_width to actual children usage
    // (needed for shrink-to-fit when this flex is inside another flex).
    if is_row && lines.len() == 1 {
        let (ls, le) = lines[0];
        let actual_main: f32 = (ls..le)
            .filter(|&i| !abs_child_ids.contains(&layout_box.children[i].node_id))
            .map(|i| {
                let cs = styles.get(&layout_box.children[i].node_id).cloned().unwrap_or_default();
                layout_box.children[i].width + cs.margin_left + cs.margin_right
            })
            .sum::<f32>() + style.gap * (le - ls).saturating_sub(1) as f32;
        if actual_main < layout_box.content_width {
            layout_box.content_width = actual_main;
        }
    }

    // Cross-axis alignment within each line
    let mut cross_offset: f32 = 0.0;
    for (line_idx, &(line_start, line_end)) in lines.iter().enumerate() {
        let line_cross = line_cross_sizes[line_idx];
        for i in line_start..line_end {
            let child_style = styles.get(&layout_box.children[i].node_id).cloned().unwrap_or_default();
            if is_row {
                match style.align_items {
                    AlignItems::Center => {
                        layout_box.children[i].y = content_y + cross_offset + (line_cross - layout_box.children[i].height) / 2.0;
                    }
                    AlignItems::FlexEnd => {
                        layout_box.children[i].y = content_y + cross_offset + line_cross - layout_box.children[i].height - child_style.margin_bottom;
                    }
                    AlignItems::Stretch => {
                        layout_box.children[i].height = line_cross - child_style.margin_top - child_style.margin_bottom;
                    }
                    _ => {} // FlexStart and Baseline keep default position
                }
            } else {
                match style.align_items {
                    AlignItems::Center => {
                        layout_box.children[i].x = content_x + cross_offset + (line_cross - layout_box.children[i].width) / 2.0;
                    }
                    AlignItems::FlexEnd => {
                        layout_box.children[i].x = content_x + cross_offset + line_cross - layout_box.children[i].width - child_style.margin_right;
                    }
                    AlignItems::Stretch => {
                        layout_box.children[i].width = line_cross - child_style.margin_left - child_style.margin_right;
                    }
                    _ => {}
                }
            }
        }
        cross_offset += line_cross;
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
        for r in p.row_start..p.row_end {
            for c in p.col_start..p.col_end.min(num_cols) {
                occupied[r][c] = true;
            }
        }
    }

    fn find_next_free(occupied: &mut Vec<Vec<bool>>, col_span: usize, row_span: usize,
                      num_cols: usize, auto_row: &mut usize, auto_col: &mut usize) -> (usize, usize) {
        loop {
            ensure_rows(occupied, *auto_row + row_span, num_cols);
            if *auto_col + col_span <= num_cols {
                let fits = (0..row_span).all(|dr| {
                    (0..col_span).all(|dc| !occupied[*auto_row + dr][*auto_col + dc])
                });
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
    let area_lookup: std::collections::HashMap<String, (usize, usize, usize, usize)> = if !style.grid_template_areas.is_empty() {
        let mut map = std::collections::HashMap::new();
        for (row_idx, row) in style.grid_template_areas.iter().enumerate() {
            for (col_idx, area_name) in row.iter().enumerate() {
                if area_name == "." { continue; }
                let entry = map.entry(area_name.clone()).or_insert((row_idx, col_idx, row_idx + 1, col_idx + 1));
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
        let cs = styles.get(&child.node_id).cloned().unwrap_or_default();

        // Check grid-area first (named area)
        if let Some(ref area_name) = cs.grid_area {
            if let Some(&(r0, c0, r1, c1)) = area_lookup.get(area_name.as_str()) {
                let p = CellPlacement { col_start: c0, col_end: c1, row_start: r0, row_end: r1 };
                mark_occupied(&mut occupied, &p, num_cols);
                max_row = max_row.max(p.row_end);
                placements.push(p);
                continue;
            }
        }

        let has_col = cs.grid_column_start.is_some() || cs.grid_column_end.is_some();
        let has_row = cs.grid_row_start.is_some() || cs.grid_row_end.is_some();

        let (col_start, col_end, row_start, row_end) = if has_col || has_row {
            let c0 = cs.grid_column_start.map(|v| resolve_line(v, num_cols)).unwrap_or(0);
            let c1 = cs.grid_column_end.map(|v| resolve_line(v, num_cols))
                .unwrap_or_else(|| (c0 + 1).min(num_cols));
            let r0 = cs.grid_row_start.map(|v| resolve_line(v, 100)).unwrap_or(auto_row);
            let r1 = cs.grid_row_end.map(|v| resolve_line(v, 100))
                .unwrap_or(r0 + 1);
            (c0.min(num_cols.saturating_sub(1)), c1.min(num_cols), r0, r1.max(r0 + 1))
        } else {
            // Auto-placement
            let (c, r) = find_next_free(&mut occupied, 1, 1, num_cols, &mut auto_row, &mut auto_col);
            (c, c + 1, r, r + 1)
        };

        let p = CellPlacement { col_start, col_end, row_start, row_end };
        mark_occupied(&mut occupied, &p, num_cols);
        max_row = max_row.max(p.row_end);
        placements.push(p);
    }

    let num_rows = max_row.max(1);

    // First pass: compute natural heights per row
    let mut row_heights = vec![0.0_f32; num_rows];
    for (idx, child) in layout_box.children.iter_mut().enumerate() {
        let p = &placements[idx];
        // Cell width spans multiple columns
        let cell_width: f32 = (p.col_start..p.col_end).map(|c| {
            if c < col_widths.len() { col_widths[c] } else { 0.0 }
        }).sum::<f32>() + (p.col_end - p.col_start).saturating_sub(1) as f32 * col_gap;

        compute_layout(child, styles, cell_width, 0.0, image_sizes);

        let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
        let child_h = child.height + child_style.margin_top + child_style.margin_bottom;
        // Distribute height across spanned rows (attribute to first row for simplicity)
        let row_span = p.row_end - p.row_start;
        let per_row_h = child_h / row_span as f32;
        for r in p.row_start..p.row_end.min(num_rows) {
            row_heights[r] = row_heights[r].max(per_row_h);
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
                    if *rh < min { *rh = min; }
                }
            }
        }
    }

    // Second pass: position each child
    for (idx, child) in layout_box.children.iter_mut().enumerate() {
        let p = &placements[idx];

        let cell_x: f32 = (0..p.col_start).map(|c| {
            if c < col_widths.len() { col_widths[c] } else { 0.0 }
        }).sum::<f32>() + p.col_start as f32 * col_gap;
        let cell_y: f32 = (0..p.row_start).map(|r| row_heights[r]).sum::<f32>() + p.row_start as f32 * row_gap;
        let cell_width: f32 = (p.col_start..p.col_end).map(|c| {
            if c < col_widths.len() { col_widths[c] } else { 0.0 }
        }).sum::<f32>() + (p.col_end - p.col_start).saturating_sub(1) as f32 * col_gap;

        let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
        child.x = content_x + cell_x + child_style.margin_left;
        child.y = content_y + cell_y + child_style.margin_top;

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

    let line_height = style.font_size * style.line_height;

    // Measure using real font glyphs if available, else fall back to a
    // proportional-font approximation. Both paths keep paint and layout
    // in sync because paint uses the same `measure_text` impl.
    let space_width = measure_text_width(" ", style.font_size, &style);

    if text == " " {
        layout_box.content_width = 0.0;
        layout_box.content_height = 0.0;
        layout_box.width = 0.0;
        layout_box.height = 0.0;
        return;
    }

    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        // Whitespace-only text nodes collapse to zero when not part of
        // meaningful inline content (they'll be skipped in block layout).
        layout_box.width = 0.0;
        layout_box.height = 0.0;
        layout_box.content_width = 0.0;
        layout_box.content_height = 0.0;
        return;
    }

    let mut lines = 1u32;
    let mut current_line_width: f32 = 0.0;
    let mut max_line_width: f32 = 0.0;

    if text.starts_with(' ') {
        current_line_width = space_width;
    }

    let nowrap = matches!(
        style.white_space,
        incognidium_style::WhiteSpace::NoWrap | incognidium_style::WhiteSpace::Pre
    );

    for (i, word) in words.iter().enumerate() {
        let word_width = measure_text_width(word, style.font_size, &style);
        let needed = if i == 0 { word_width } else { space_width + word_width };

        if !nowrap && current_line_width + needed > containing_width + 0.5 && current_line_width > 0.0 {
            max_line_width = max_line_width.max(current_line_width);
            lines += 1;
            current_line_width = word_width;
        } else {
            current_line_width += needed;
        }
    }

    if text.ends_with(' ') {
        current_line_width += space_width;
    }
    max_line_width = max_line_width.max(current_line_width);

    layout_box.content_width = max_line_width;
    layout_box.content_height = lines as f32 * line_height;
    layout_box.width = max_line_width;
    layout_box.height = lines as f32 * line_height;
}

/// Measure the rendered width of `text` at `font_size` using the same
/// font ab_glyph will paint with. Falls back to a rough approximation if
/// no TTF is installed.
pub fn measure_text_width(text: &str, font_size: f32, style: &incognidium_style::ComputedStyle) -> f32 {
    use ab_glyph::{Font, PxScale, ScaleFont};
    if let Some(font) = get_layout_font(style.font_weight == incognidium_style::FontWeight::Bold,
                                        style.font_style == incognidium_style::FontStyle::Italic) {
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
            prev = Some(gid);
        }
        w
    } else {
        // No TTF: approximate with proportional-font average
        text.chars().count() as f32 * font_size * 0.52
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
        ("LiberationSans-Regular.ttf", "LiberationSans-Bold.ttf",
         "LiberationSans-Italic.ttf", "LiberationSans-BoldItalic.ttf"),
        ("DejaVuSans.ttf", "DejaVuSans-Bold.ttf",
         "DejaVuSans-Oblique.ttf", "DejaVuSans-BoldOblique.ttf"),
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
                return Some(LayoutFonts { regular: rf, bold: bf, italic: ifv, bold_italic: bif });
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

fn layout_image(layout_box: &mut LayoutBox, styles: &StyleMap, containing_width: f32, image_sizes: &ImageSizes) {
    let style = styles.get(&layout_box.node_id).cloned().unwrap_or_default();

    // Try to get actual image dimensions from the cache
    let actual_dims = layout_box.image_src.as_ref().and_then(|src| image_sizes.get(src));

    let explicit_w = matches!(style.width, SizeValue::Px(_) | SizeValue::Percent(_));
    let explicit_h = matches!(style.height, SizeValue::Px(_) | SizeValue::Percent(_));

    // If no actual image AND no explicit dimensions, collapse to 0
    if actual_dims.is_none() && !explicit_w && !explicit_h
        && !layout_box.image_src.as_deref().unwrap_or("").starts_with("__canvas__")
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
pub fn flatten_layout(layout_box: &LayoutBox, offset_x: f32, offset_y: f32, styles: &StyleMap) -> Vec<FlatBox> {
    let mut boxes = flatten_with_clip(layout_box, offset_x, offset_y, None, styles);
    boxes.sort_by_key(|fb| {
        styles.get(&fb.node_id).map(|s| s.z_index).unwrap_or(0)
    });
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

    // Skip zero-size text boxes (whitespace-only nodes that got laid out)
    if layout_box.box_type == BoxType::Text
        && layout_box.text.as_deref().map(|t| t.trim().is_empty()).unwrap_or(true)
        && layout_box.width <= 0.01
        && layout_box.height <= 0.01
    {
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
        });
    }

    // Propagate parent link_href to children
    let parent_href = layout_box.link_href.clone();
    for child in &layout_box.children {
        let child_style = styles.get(&child.node_id).cloned().unwrap_or_default();
        let child_offset = if child_style.position == Position::Fixed {
            (0.0, 0.0) // viewport-relative for fixed positioning
        } else {
            (abs_x, abs_y)
        };
        let mut child_boxes = flatten_with_clip(child, child_offset.0, child_offset.1, clip, styles);
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
    /// Clipping rectangle from nearest ancestor with overflow:hidden.
    /// (x, y, width, height) in absolute coordinates. None = no clipping.
    pub clip: Option<(f32, f32, f32, f32)>,
    /// Float text indent: (indent_px, num_lines, is_left)
    pub float_text_indent: Option<(f32, u32, bool)>,
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
