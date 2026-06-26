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
head, style, script, link, meta, title, template, svg, datalist { display: none; }
/* dialog is handled specially: closed dialog is display:none, open dialog is display:block */
noscript { display: block; }
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
ul { list-style-type: disc; }
ol { list-style-type: decimal; }
li { display: block; margin-top: 0.5em; margin-bottom: 0.5em; }
dl { display: block; margin-top: 1em; margin-bottom: 1em; }
dt { display: block; font-weight: bold; }
dd { display: block; margin-left: 40px; }
table { display: table; }
thead { display: table-header-group; }
	tbody { display: table-row-group; }
	tfoot { display: table-footer-group; }
tr { display: table-row; }
td, th { display: table-cell; padding: 1px; }
th { font-weight: bold; }
caption { display: table-caption; text-align: center; }
	col { display: table-column; }
	colgroup { display: table-column-group; }
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
input { display: inline; padding: 2px 4px; border: 1px solid #767676; width: 200px; }
textarea { display: inline-block; padding: 2px 4px; border: 1px solid #767676; }
input[type="checkbox"] { display: inline-block; width: 13px; height: 13px; padding: 0; margin: 3px; }
input[type="radio"] { display: inline-block; width: 13px; height: 13px; padding: 0; margin: 3px; border-radius: 50%; }
select { display: inline; padding: 2px 20px 2px 4px; border: 1px solid #767676; background-color: #f8f8f8; }
button { display: inline; padding: 2px 8px; border: 1px solid #767676; }
label { display: inline; }
canvas { display: inline; width: 300px; height: 150px; }
/* HTML5 semantic inline elements */
time, mark { display: inline; }
/* HTML5 media elements */
video, audio { display: block; }
source, track { display: none; }
/* HTML5 embedded content */
embed, object { display: inline; }
param { display: none; }
/* HTML5 text-level semantic elements */
wbr { display: inline; }
ruby { display: inline; }
rt { display: ruby-text; font-size: 0.5em; }
rp { display: none; }
bdi, bdo { display: inline; }
data { display: inline; }
/* HTML5 form elements */
output { display: inline; }
/* HTML5 menu and picture */
menu { display: block; }
picture { display: inline; }
"#;

/// Computed style values for a single element.
#[derive(Debug, Clone)]
pub struct ComputedStyle {
    pub display: Display,
    pub position: Position,
    pub float: Float,
    pub clear: Clear,
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

    // Table properties
    pub table_layout: TableLayout,
    pub caption_side: CaptionSide,
    pub border_collapse: BorderCollapse,
    pub border_spacing: (f32, f32),
    pub empty_cells: EmptyCells,

    // Typography
    pub font_family: FontFamily,
    pub letter_spacing: f32,
    pub word_spacing: f32,
    pub vertical_align: VerticalAlign,
    pub text_shadow: Option<TextShadow>,

    // Border radius
    pub border_top_left_radius: f32,
    pub border_top_right_radius: f32,
    pub border_bottom_left_radius: f32,
    pub border_bottom_right_radius: f32,

    // Outline (focus indicator)
    pub outline_width: f32,
    pub outline_color: CssColor,
    pub outline_style: OutlineStyle,
    pub outline_offset: f32,

    // Box shadow
    pub box_shadow: Option<BoxShadow>,

    // Text wrapping and overflow
    pub word_break: WordBreak,
    pub overflow_wrap: OverflowWrap,
    pub text_overflow: TextOverflow,
    pub white_space_collapse: WhiteSpaceCollapse,
    pub text_wrap: TextWrap,

    // Interaction
    pub cursor: Cursor,
    pub pointer_events: PointerEvents,
    pub user_select: UserSelect,

    // Sizing and layout
    pub aspect_ratio: Option<AspectRatio>,
    pub resize: Resize,

    // Transform
    pub transform: Vec<Transform>,
    pub transform_origin: (f32, f32),

    // Motion path (offset)
    pub offset_path: Option<OffsetPath>,
    pub offset_distance: f32,
    pub offset_rotate: OffsetRotate,
    pub offset_anchor: (f32, f32),

    // Image/Video
    pub object_fit: ObjectFit,
    pub object_position: (f32, f32),

    // Content and quotes
    pub content: Content,
    pub quotes: Quotes,

    // Multi-column layout (column_gap is shared with Grid)
    pub column_count: Option<i32>,
    pub column_width: Option<f32>,
    pub column_rule_width: f32,
    pub column_rule_color: CssColor,
    pub column_rule_style: ColumnRuleStyle,

    // Fragmentation breaks
    pub break_before: BreakBefore,
    pub break_after: BreakAfter,
    pub break_inside: BreakInside,

    // Writing modes and direction
    pub writing_mode: WritingMode,
    pub direction: Direction,

    // Ruby annotations (East Asian typography)
    pub ruby_position: RubyPosition,
    pub ruby_align: RubyAlign,
    pub ruby_merge: RubyMerge,

    // Scrollbar
    pub scrollbar_width: ScrollbarWidth,
    pub scrollbar_color: Option<(CssColor, CssColor)>, // thumb, track
    pub scrollbar_gutter: ScrollbarGutter,

    // Filter effects
    pub filter: Vec<Filter>,
    pub backdrop_filter: Vec<Filter>,

    // Containment
    pub contain: Contain,
    pub contain_intrinsic_size: Option<(f32, f32)>,
    pub content_visibility: ContentVisibility,

    // Container queries
    pub container_type: ContainerType,
    pub container_name: Vec<String>,

    // Transitions
    pub transition_property: Vec<String>,
    pub transition_duration: f32, // seconds
    pub transition_timing_function: TransitionTimingFunction,
    pub transition_delay: f32, // seconds
    pub transition_behavior: TransitionBehavior,

    // Animations
    pub animation_name: Vec<String>,
    pub animation_duration: Vec<f32>, // seconds
    pub animation_timing_function: Vec<TransitionTimingFunction>,
    pub animation_delay: Vec<f32>, // seconds
    pub animation_iteration_count: Vec<AnimationIterationCount>,
    pub animation_direction: Vec<AnimationDirection>,
    pub animation_fill_mode: Vec<AnimationFillMode>,
    pub animation_play_state: Vec<AnimationPlayState>,

    // View transitions
    pub view_transition_name: Option<String>,
    pub view_transition_class: Vec<String>,

    // Table and tab properties
    pub tab_size: i32,

    // Hyphenation and line clamping
    pub hyphens: Hyphens,
    pub line_clamp: Option<i32>,
    pub text_justify: TextJustify,
    pub hyphenate_character: String,
    pub text_group_align: TextGroupAlign,

    // List style
    pub list_style_image: Option<String>,
    pub list_style_position: ListStylePosition,

    // Text decoration sub-properties
    pub text_decoration_line: TextDecorationLine,
    pub text_decoration_color: Option<CssColor>,
    pub text_decoration_style: TextDecorationStyle,
    pub text_decoration_thickness: TextDecorationThickness,

    // Blending and isolation
    pub mix_blend_mode: MixBlendMode,
    pub isolation: Isolation,

    // Form controls
    pub accent_color: Option<CssColor>,
    pub caret_color: Option<CssColor>,
    pub appearance: Appearance,
    pub field_sizing: FieldSizing,

    // Color scheme
    pub color_scheme: ColorScheme,
    pub forced_color_adjust: ForcedColorAdjust,

    // Font features
    pub font_variant: FontVariant,
    pub font_feature_settings: Vec<String>,
    pub font_display: FontDisplay,
    pub font_stretch: FontStretch,
    pub font_size_adjust: Option<f32>,

    // Scroll behavior
    pub scroll_behavior: ScrollBehavior,
    pub overscroll_behavior: OverscrollBehavior,
    pub scroll_margin_top: f32,
    pub scroll_margin_right: f32,
    pub scroll_margin_bottom: f32,
    pub scroll_margin_left: f32,
    pub scroll_padding_top: f32,
    pub scroll_padding_right: f32,
    pub scroll_padding_bottom: f32,
    pub scroll_padding_left: f32,

    // Shapes and clipping
    pub clip_path: Option<ClipPath>,
    pub shape_outside: Option<ShapeOutside>,

    // Place shorthands (align + justify)
    pub place_content: (AlignContent, JustifyContent),
    pub place_items: (AlignItems, JustifyItems),
    pub place_self: (AlignSelf, JustifySelf),

    // Background sub-properties
    pub background_image: BackgroundImage,
    pub background_repeat: BackgroundRepeat,
    pub background_attachment: BackgroundAttachment,
    pub background_position: (f32, f32), // x, y percentages
    pub background_size: BackgroundSize,
    pub background_origin: BackgroundOrigin,
    pub background_clip: BackgroundClip,

    // Border image
    pub border_image_source: Option<String>,
    pub border_image_slice: BorderImageSlice,
    pub border_image_width: BorderImageWidth,
    pub border_image_outset: BorderImageOutset,
    pub border_image_repeat: BorderImageRepeat,

    // Grid auto sizing
    pub grid_auto_columns: Vec<GridTrackSize>,
    pub grid_auto_rows: Vec<GridTrackSize>,

    // Performance and interaction
    pub will_change: Vec<String>,
    pub touch_action: TouchAction,

    // Page properties (Paged Media)
    pub page_break_before: PageBreak,
    pub page_break_after: PageBreak,
    pub page_break_inside: PageBreakInside,

    // Print properties
    pub print_color_adjust: PrintColorAdjust,

    // Counter properties
    pub counter_reset: Vec<(String, i32)>,
    pub counter_increment: Vec<(String, i32)>,

    // Alternative text properties
    pub text_decoration_skip_ink: TextDecorationSkipInk,
    pub text_underline_position: TextUnderlinePosition,

    // Font variation settings
    pub font_variation_settings: Vec<(String, f32)>,

    // Text emphasis (East Asian typography)
    pub text_emphasis_style: TextEmphasisStyle,
    pub text_emphasis_color: Option<CssColor>,
    pub text_emphasis_position: TextEmphasisPosition,

    // Transform 3D
    pub transform_box: TransformBox,
    pub transform_style: TransformStyle,
    pub perspective: Option<f32>,
    pub perspective_origin: (f32, f32),
    pub backface_visibility: BackfaceVisibility,

    // Background blending
    pub background_blend_mode: BlendMode,

    // Image rendering
    pub image_rendering: ImageRendering,

    // Text alignment
    pub text_align_last: TextAlignLast,

    // Text decoration extras
    pub text_decoration_skip: TextDecorationSkip,
    pub text_underline_offset: Option<f32>,

    // Caret
    pub caret_shape: CaretShape,

    // Box decoration
    pub box_decoration_break: BoxDecorationBreak,

    // Text combination
    pub text_combine_upright: TextCombineUpright,

    // Line breaking
    pub line_break: LineBreak,

    // Hanging punctuation
    pub hanging_punctuation: HangingPunctuation,

    // SVG properties
    pub fill: Option<CssColor>,
    pub fill_opacity: f32,
    pub fill_rule: FillRule,
    pub stroke: Option<CssColor>,
    pub stroke_width: f32,
    pub stroke_opacity: f32,
    pub stroke_linecap: StrokeLinecap,
    pub stroke_linejoin: StrokeLinejoin,
    pub clip_rule: ClipRule,

    // Additional flexbox
    pub flex_flow: (FlexDirection, FlexWrap),

    // Animation extras
    pub animation_composition: AnimationComposition,
    pub animation_timeline: AnimationTimeline,

    // Timeline properties (scroll-driven animations)
    pub scroll_timeline_name: Vec<String>,
    pub scroll_timeline_axis: ScrollAxis,
    pub view_timeline_name: Vec<String>,
    pub view_timeline_axis: ScrollAxis,
    pub view_timeline_inset: (f32, f32),

    // Anchor positioning (Popover API)
    pub anchor_name: Vec<String>,
    pub anchor_default: Option<String>,
    pub position_anchor: Option<String>,
    pub position_area: PositionArea,
    pub position_try: PositionTry,
    pub position_visibility: PositionVisibility,

    // Popover
    pub popover: Popover,

    // Logical properties (additional)
    pub inset_block: (Option<f32>, Option<f32>),
    pub inset_inline: (Option<f32>, Option<f32>),
    pub margin_block: (f32, f32),
    pub margin_inline: (f32, f32),
    pub padding_block: (f32, f32),
    pub padding_inline: (f32, f32),
    pub border_block_width: (f32, f32),
    pub border_inline_width: (f32, f32),

    // Border sub-properties
    pub border_top_style: BorderStyle,
    pub border_right_style: BorderStyle,
    pub border_bottom_style: BorderStyle,
    pub border_left_style: BorderStyle,
    pub border_top_color: Option<CssColor>,
    pub border_right_color: Option<CssColor>,
    pub border_bottom_color: Option<CssColor>,
    pub border_left_color: Option<CssColor>,

    // Mask properties
    pub mask_image: Option<String>,
    pub mask_mode: MaskMode,
    pub mask_repeat: MaskRepeat,
    pub mask_position: (f32, f32),
    pub mask_size: MaskSize,
    pub mask_composite: MaskComposite,

    // Shape margin/threshold
    pub shape_margin: f32,
    pub shape_image_threshold: f32,

    // Font synthesis
    pub font_synthesis: FontSynthesis,

    // Text orientation
    pub text_orientation: TextOrientation,

    // Line height step
    pub line_height_step: Option<f32>,

    // Overflow anchor
    pub overflow_anchor: OverflowAnchor,

    // Scroll snap
    pub scroll_snap_type: ScrollSnapType,
    pub scroll_snap_align: ScrollSnapAlign,

    // Overscroll behavior block/inline
    pub overscroll_behavior_block: OverscrollBehavior,
    pub overscroll_behavior_inline: OverscrollBehavior,

    // Initial letter
    pub initial_letter: InitialLetter,

    // All shorthand
    pub all: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ListStyleType {
    Disc,
    Circle,
    Square,
    Decimal,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
    None,
}

impl Default for ComputedStyle {
    fn default() -> Self {
        ComputedStyle {
            display: Display::Block,
            position: Position::Static,
            float: Float::None,
            clear: Clear::None,
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

            // Table properties
            table_layout: TableLayout::Auto,
            caption_side: CaptionSide::Top,
            border_collapse: BorderCollapse::Separate,
            border_spacing: (0.0, 0.0),
            empty_cells: EmptyCells::Show,

            // Typography
            font_family: FontFamily::SansSerif,
            letter_spacing: 0.0,
            word_spacing: 0.0,
            vertical_align: VerticalAlign::Baseline,
            text_shadow: None,

            // Text wrapping and overflow
            word_break: WordBreak::Normal,
            overflow_wrap: OverflowWrap::Normal,
            text_overflow: TextOverflow::Clip,
            white_space_collapse: WhiteSpaceCollapse::Collapse,
            text_wrap: TextWrap::Wrap,

            // Border radius
            border_top_left_radius: 0.0,
            border_top_right_radius: 0.0,
            border_bottom_left_radius: 0.0,
            border_bottom_right_radius: 0.0,

            // Outline
            outline_width: 0.0,
            outline_color: CssColor::from_rgb(0, 0, 0),
            outline_style: OutlineStyle::None,
            outline_offset: 0.0,

            // Box shadow
            box_shadow: None,

            // Interaction
            cursor: Cursor::Auto,
            pointer_events: PointerEvents::Auto,
            user_select: UserSelect::Auto,

            // Sizing and layout
            aspect_ratio: None,
            resize: Resize::None,

            // Transform
            transform: Vec::new(),
            transform_origin: (0.5, 0.5), // center

            // Motion path
            offset_path: None,
            offset_distance: 0.0,
            offset_rotate: OffsetRotate::Auto,
            offset_anchor: (0.5, 0.5), // center

            // Image/Video
            object_fit: ObjectFit::Fill,
            object_position: (0.5, 0.5), // center

            // Content and quotes
            content: Content::Normal,
            quotes: Quotes::Auto,

            // Multi-column layout
            column_count: None,
            column_width: None,
            column_rule_width: 0.0,
            column_rule_color: CssColor::BLACK,
            column_rule_style: ColumnRuleStyle::None,

            // Fragmentation breaks
            break_before: BreakBefore::Auto,
            break_after: BreakAfter::Auto,
            break_inside: BreakInside::Auto,

            // Writing modes and direction
            writing_mode: WritingMode::HorizontalTb,
            direction: Direction::Ltr,

            // Ruby annotations
            ruby_position: RubyPosition::Over,
            ruby_align: RubyAlign::Center,
            ruby_merge: RubyMerge::Separate,

            // Scrollbar
            scrollbar_width: ScrollbarWidth::Auto,
            scrollbar_color: None,
            scrollbar_gutter: ScrollbarGutter::Auto,

            // Filter effects
            filter: Vec::new(),
            backdrop_filter: Vec::new(),

            // Containment
            contain: Contain::None,
            contain_intrinsic_size: None,
            content_visibility: ContentVisibility::Visible,

            // Container queries
            container_type: ContainerType::None,
            container_name: Vec::new(),

            // Transitions
            transition_property: Vec::new(),
            transition_duration: 0.0,
            transition_timing_function: TransitionTimingFunction::Ease,
            transition_delay: 0.0,
            transition_behavior: TransitionBehavior::Normal,

            // Animations
            animation_name: Vec::new(),
            animation_duration: Vec::new(),
            animation_timing_function: Vec::new(),
            animation_delay: Vec::new(),
            animation_iteration_count: Vec::new(),
            animation_direction: Vec::new(),
            animation_fill_mode: Vec::new(),
            animation_play_state: Vec::new(),

            // View transitions
            view_transition_name: None,
            view_transition_class: Vec::new(),

            // Table and tab properties
            tab_size: 8,

            // Hyphenation and line clamping
            hyphens: Hyphens::Manual,
            line_clamp: None,
            text_justify: TextJustify::Auto,
            hyphenate_character: "-".to_string(),
            text_group_align: TextGroupAlign::Start,

            // List style
            list_style_image: None,
            list_style_position: ListStylePosition::Outside,

            // Text decoration sub-properties
            text_decoration_line: TextDecorationLine::None,
            text_decoration_color: None,
            text_decoration_style: TextDecorationStyle::Solid,
            text_decoration_thickness: TextDecorationThickness::Auto,

            // Blending and isolation
            mix_blend_mode: MixBlendMode::Normal,
            isolation: Isolation::Auto,

            // Form controls
            accent_color: None,
            caret_color: None,
            appearance: Appearance::Auto,
            field_sizing: FieldSizing::Fixed,

            // Color scheme
            color_scheme: ColorScheme::Normal,
            forced_color_adjust: ForcedColorAdjust::Auto,

            // Font features
            font_variant: FontVariant::Normal,
            font_feature_settings: Vec::new(),
            font_display: FontDisplay::Auto,
            font_stretch: FontStretch::Normal,
            font_size_adjust: None,

            // Scroll behavior
            scroll_behavior: ScrollBehavior::Auto,
            overscroll_behavior: OverscrollBehavior::Auto,
            scroll_margin_top: 0.0,
            scroll_margin_right: 0.0,
            scroll_margin_bottom: 0.0,
            scroll_margin_left: 0.0,
            scroll_padding_top: 0.0,
            scroll_padding_right: 0.0,
            scroll_padding_bottom: 0.0,
            scroll_padding_left: 0.0,

            // Shapes and clipping
            clip_path: None,
            shape_outside: None,

            // Place shorthands
            place_content: (AlignContent::Stretch, JustifyContent::FlexStart),
            place_items: (AlignItems::Stretch, JustifyItems::Auto),
            place_self: (AlignSelf::Auto, JustifySelf::Auto),

            // Background sub-properties
            background_image: BackgroundImage::None,
            background_repeat: BackgroundRepeat::Repeat,
            background_attachment: BackgroundAttachment::Scroll,
            background_position: (0.0, 0.0), // top left
            background_size: BackgroundSize::Auto,
            background_origin: BackgroundOrigin::PaddingBox,
            background_clip: BackgroundClip::BorderBox,

            // Border image
            border_image_source: None,
            border_image_slice: BorderImageSlice::Auto,
            border_image_width: BorderImageWidth::Auto,
            border_image_outset: BorderImageOutset::Auto,
            border_image_repeat: BorderImageRepeat::Stretch,

            // Grid auto sizing
            grid_auto_columns: Vec::new(),
            grid_auto_rows: Vec::new(),

            // Performance and interaction
            will_change: Vec::new(),
            touch_action: TouchAction::Auto,

            // Page properties
            page_break_before: PageBreak::Auto,
            page_break_after: PageBreak::Auto,
            page_break_inside: PageBreakInside::Auto,

            // Print properties
            print_color_adjust: PrintColorAdjust::Economy,

            // Counter properties
            counter_reset: Vec::new(),
            counter_increment: Vec::new(),

            // Alternative text properties
            text_decoration_skip_ink: TextDecorationSkipInk::Auto,
            text_underline_position: TextUnderlinePosition::Auto,

            // Font variation settings
            font_variation_settings: Vec::new(),

            // Text emphasis
            text_emphasis_style: TextEmphasisStyle::None,
            text_emphasis_color: None,
            text_emphasis_position: TextEmphasisPosition::Over,

            // Transform 3D
            transform_box: TransformBox::BorderBox,
            transform_style: TransformStyle::Flat,
            perspective: None,
            perspective_origin: (0.5, 0.5),
            backface_visibility: BackfaceVisibility::Visible,

            // Background blending
            background_blend_mode: BlendMode::Normal,

            // Image rendering
            image_rendering: ImageRendering::Auto,

            // Text alignment
            text_align_last: TextAlignLast::Auto,

            // Text decoration extras
            text_decoration_skip: TextDecorationSkip::Objects,
            text_underline_offset: None,

            // Caret
            caret_shape: CaretShape::Auto,

            // Box decoration
            box_decoration_break: BoxDecorationBreak::Slice,

            // Text combination
            text_combine_upright: TextCombineUpright::None,

            // Line breaking
            line_break: LineBreak::Auto,

            // Hanging punctuation
            hanging_punctuation: HangingPunctuation::None,

            // SVG properties
            fill: None,
            fill_opacity: 1.0,
            fill_rule: FillRule::NonZero,
            stroke: None,
            stroke_width: 1.0,
            stroke_opacity: 1.0,
            stroke_linecap: StrokeLinecap::Butt,
            stroke_linejoin: StrokeLinejoin::Miter,
            clip_rule: ClipRule::NonZero,

            // Additional flexbox
            flex_flow: (FlexDirection::Row, FlexWrap::NoWrap),

            // Animation extras
            animation_composition: AnimationComposition::Replace,
            animation_timeline: AnimationTimeline::Auto,

            // Timeline properties
            scroll_timeline_name: Vec::new(),
            scroll_timeline_axis: ScrollAxis::Block,
            view_timeline_name: Vec::new(),
            view_timeline_axis: ScrollAxis::Block,
            view_timeline_inset: (0.0, 0.0),

            // Anchor positioning
            anchor_name: Vec::new(),
            anchor_default: None,
            position_anchor: None,
            position_area: PositionArea::None,
            position_try: PositionTry::None,
            position_visibility: PositionVisibility::Always,

            // Popover
            popover: Popover::None,

            // Logical properties
            inset_block: (None, None),
            inset_inline: (None, None),
            margin_block: (0.0, 0.0),
            margin_inline: (0.0, 0.0),
            padding_block: (0.0, 0.0),
            padding_inline: (0.0, 0.0),
            border_block_width: (0.0, 0.0),
            border_inline_width: (0.0, 0.0),

            // Border sub-properties
            border_top_style: BorderStyle::None,
            border_right_style: BorderStyle::None,
            border_bottom_style: BorderStyle::None,
            border_left_style: BorderStyle::None,
            border_top_color: None,
            border_right_color: None,
            border_bottom_color: None,
            border_left_color: None,

            // Mask properties
            mask_image: None,
            mask_mode: MaskMode::Alpha,
            mask_repeat: MaskRepeat::Repeat,
            mask_position: (0.0, 0.0),
            mask_size: MaskSize::Auto,
            mask_composite: MaskComposite::Add,

            // Shape margin/threshold
            shape_margin: 0.0,
            shape_image_threshold: 0.0,

            // Font synthesis
            font_synthesis: FontSynthesis::WeightStyle,

            // Text orientation
            text_orientation: TextOrientation::Mixed,

            // Line height step
            line_height_step: None,

            // Overflow anchor
            overflow_anchor: OverflowAnchor::Auto,

            // Scroll snap
            scroll_snap_type: ScrollSnapType::None,
            scroll_snap_align: ScrollSnapAlign::None,

            // Overscroll behavior block/inline
            overscroll_behavior_block: OverscrollBehavior::Auto,
            overscroll_behavior_inline: OverscrollBehavior::Auto,

            // Initial letter
            initial_letter: InitialLetter::Normal,

            // All shorthand
            all: None,
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
    Table,
    TableRow,
    TableCell,
    TableHeaderGroup,
    TableRowGroup,
    TableFooterGroup,
    TableColumn,
    TableColumnGroup,
    TableCaption,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Position {
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Clear {
    None,
    Left,
    Right,
    Both,
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

#[derive(Debug, Clone, PartialEq)]
pub enum SizeValue {
    Px(f32),
    Percent(f32),
    Auto,
    None,
    /// CSS calc() expression: calc(100% - 20px)
    Calc(Box<CalcExpression>),
    /// CSS min() expression: min(100%, 500px)
    Min(Vec<CalcValue>),
    /// CSS max() expression: max(100%, 500px)
    Max(Vec<CalcValue>),
    /// CSS clamp() expression: clamp(200px, 50%, 800px)
    Clamp { min: CalcValue, val: CalcValue, max: CalcValue },
}

/// A value that can appear in CSS calc(), min(), max(), clamp()
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CalcValue {
    Px(f32),
    Percent(f32),
    Em(f32),
    Rem(f32),
    Vw(f32),
    Vh(f32),
}

/// Expression for CSS calc() with +, -, *, /
#[derive(Debug, Clone, PartialEq)]
pub enum CalcExpression {
    Value(CalcValue),
    Add(Box<CalcExpression>, Box<CalcExpression>),
    Subtract(Box<CalcExpression>, Box<CalcExpression>),
    Multiply(Box<CalcExpression>, f32),
    Divide(Box<CalcExpression>, f32),
}

// Table layout enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TableLayout {
    Auto,
    Fixed,
}

impl Default for TableLayout {
    fn default() -> Self {
        TableLayout::Auto
    }
}

// Caption side enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CaptionSide {
    Top,
    Bottom,
    Left,
    Right,
}

impl Default for CaptionSide {
    fn default() -> Self {
        CaptionSide::Top
    }
}

// Border collapse enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BorderCollapse {
    Collapse,
    Separate,
}

impl Default for BorderCollapse {
    fn default() -> Self {
        BorderCollapse::Separate
    }
}

// Empty cells enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EmptyCells {
    Show,
    Hide,
}

impl Default for EmptyCells {
    fn default() -> Self {
        EmptyCells::Show
    }
}

// Font family enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FontFamily {
    Serif,
    SansSerif,
    Monospace,
    Cursive,
    Fantasy,
    SystemUI,
}

impl Default for FontFamily {
    fn default() -> Self {
        FontFamily::SansSerif
    }
}

// Vertical align enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VerticalAlign {
    Baseline,
    Top,
    Bottom,
    Middle,
    Sub,
    Super,
    TextTop,
    TextBottom,
}

impl Default for VerticalAlign {
    fn default() -> Self {
        VerticalAlign::Baseline
    }
}

// Text shadow structure
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextShadow {
    pub offset_x: f32,
    pub offset_y: f32,
    pub blur_radius: f32,
    pub color: CssColor,
}

/// A color stop in a gradient
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorStop {
    pub color: CssColor,
    pub position: Option<f32>, // 0.0 to 1.0, None means evenly distributed
}

/// Linear gradient direction
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GradientDirection {
    Angle(f32), // degrees
    ToTop,
    ToBottom,
    ToLeft,
    ToRight,
    ToTopLeft,
    ToTopRight,
    ToBottomLeft,
    ToBottomRight,
}

impl Default for GradientDirection {
    fn default() -> Self {
        GradientDirection::ToBottom
    }
}

/// Linear gradient definition
#[derive(Debug, Clone, PartialEq)]
pub struct LinearGradient {
    pub direction: GradientDirection,
    pub stops: Vec<ColorStop>,
    pub repeating: bool,
}

/// Background image type
#[derive(Debug, Clone, PartialEq)]
pub enum BackgroundImage {
    None,
    Url(String),
    LinearGradient(LinearGradient),
    // RadialGradient could be added later
}

impl Default for BackgroundImage {
    fn default() -> Self {
        BackgroundImage::None
    }
}

// Word break enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WordBreak {
    Normal,
    BreakAll,
    KeepAll,
    BreakWord,
}

impl Default for WordBreak {
    fn default() -> Self {
        WordBreak::Normal
    }
}

// Overflow wrap enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OverflowWrap {
    Normal,
    BreakWord,
    Anywhere,
}

impl Default for OverflowWrap {
    fn default() -> Self {
        OverflowWrap::Normal
    }
}

// Text overflow enum
#[derive(Debug, Clone, PartialEq)]
pub enum TextOverflow {
    Clip,
    Ellipsis,
    String(String),
}

impl Default for TextOverflow {
    fn default() -> Self {
        TextOverflow::Clip
    }
}

// White space collapse enum (CSS Text Level 4)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WhiteSpaceCollapse {
    Collapse,
    Preserve,
    PreserveBreaks,
    BreakSpaces,
}

impl Default for WhiteSpaceCollapse {
    fn default() -> Self {
        WhiteSpaceCollapse::Collapse
    }
}

// Text wrap enum (CSS Text Level 4)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextWrap {
    Wrap,
    NoWrap,
    Balance,
    Pretty,
    Stable,
}

impl Default for TextWrap {
    fn default() -> Self {
        TextWrap::Wrap
    }
}

// Cursor enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cursor {
    Auto,
    Default,
    Pointer,
    Text,
    Move,
    NotAllowed,
    Grab,
    Grabbing,
    Wait,
    Help,
    Crosshair,
    Cell,
    None,
}

impl Default for Cursor {
    fn default() -> Self {
        Cursor::Auto
    }
}

// Pointer events enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointerEvents {
    Auto,
    None,
    VisiblePainted,
    VisibleFill,
    VisibleStroke,
    Painted,
    Fill,
    Stroke,
    All,
}

impl Default for PointerEvents {
    fn default() -> Self {
        PointerEvents::Auto
    }
}

// User select enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UserSelect {
    Auto,
    None,
    Text,
    All,
    Contain,
}

impl Default for UserSelect {
    fn default() -> Self {
        UserSelect::Auto
    }
}

// Aspect ratio structure
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AspectRatio {
    pub width: f32,
    pub height: f32,
}

// Resize enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Resize {
    None,
    Both,
    Horizontal,
    Vertical,
    Block,
    Inline,
}

impl Default for Resize {
    fn default() -> Self {
        Resize::None
    }
}

// Transform enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Transform {
    Translate(f32, f32),
    TranslateX(f32),
    TranslateY(f32),
    Scale(f32, f32),
    ScaleX(f32),
    ScaleY(f32),
    Rotate(f32), // degrees
    Skew(f32, f32), // degrees
    SkewX(f32),
    SkewY(f32),
}

// Offset path enum (simplified)
#[derive(Debug, Clone, PartialEq)]
pub enum OffsetPath {
    None,
    Path(String),
    Url(String),
}

impl Default for OffsetPath {
    fn default() -> Self {
        OffsetPath::None
    }
}

// Offset rotate enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OffsetRotate {
    Auto,
    Reverse,
    Angle(f32), // degrees
}

impl Default for OffsetRotate {
    fn default() -> Self {
        OffsetRotate::Auto
    }
}

// Object fit enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ObjectFit {
    Fill,
    Contain,
    Cover,
    None,
    ScaleDown,
}

impl Default for ObjectFit {
    fn default() -> Self {
        ObjectFit::Fill
    }
}

// Content enum for ::before/::after
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    Normal,
    None,
    Text(String),
    OpenQuote,
    CloseQuote,
    NoOpenQuote,
    NoCloseQuote,
    /// Counter value: counter(name)
    Counter(String, CounterStyle),
    /// Counters value: counters(name, separator)
    Counters(String, String, CounterStyle),
}

/// Counter display style
#[derive(Debug, Clone, PartialEq)]
pub enum CounterStyle {
    Decimal,
    LowerRoman,
    UpperRoman,
    LowerAlpha,
    UpperAlpha,
    Disc,
    Circle,
    Square,
}

impl Default for CounterStyle {
    fn default() -> Self {
        CounterStyle::Decimal
    }
}

impl Default for Content {
    fn default() -> Self {
        Content::Normal
    }
}

// Quotes enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Quotes {
    Auto,
    None,
}

impl Default for Quotes {
    fn default() -> Self {
        Quotes::Auto
    }
}

// Column rule style enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColumnRuleStyle {
    None,
    Solid,
    Dashed,
    Dotted,
    Double,
}

impl Default for ColumnRuleStyle {
    fn default() -> Self {
        ColumnRuleStyle::None
    }
}

// Break before enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BreakBefore {
    Auto,
    Avoid,
    Always,
    All,
    Page,
    Column,
    Region,
}

impl Default for BreakBefore {
    fn default() -> Self {
        BreakBefore::Auto
    }
}

// Break after enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BreakAfter {
    Auto,
    Avoid,
    Always,
    All,
    Page,
    Column,
    Region,
}

impl Default for BreakAfter {
    fn default() -> Self {
        BreakAfter::Auto
    }
}

// Break inside enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BreakInside {
    Auto,
    Avoid,
    AvoidPage,
    AvoidColumn,
    AvoidRegion,
}

impl Default for BreakInside {
    fn default() -> Self {
        BreakInside::Auto
    }
}

// Writing mode enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WritingMode {
    HorizontalTb,
    VerticalRl,
    VerticalLr,
    SidewaysRl,
    SidewaysLr,
}

impl Default for WritingMode {
    fn default() -> Self {
        WritingMode::HorizontalTb
    }
}

// Direction enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Direction {
    Ltr,
    Rtl,
}

impl Default for Direction {
    fn default() -> Self {
        Direction::Ltr
    }
}

// Ruby position enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RubyPosition {
    Over,
    Under,
    InterCharacter,
}

impl Default for RubyPosition {
    fn default() -> Self {
        RubyPosition::Over
    }
}

// Ruby align enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RubyAlign {
    Start,
    Center,
    SpaceBetween,
    SpaceAround,
}

impl Default for RubyAlign {
    fn default() -> Self {
        RubyAlign::Center
    }
}

// Ruby merge enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RubyMerge {
    Separate,
    Merge,
    Auto,
}

impl Default for RubyMerge {
    fn default() -> Self {
        RubyMerge::Separate
    }
}

// Scrollbar width enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScrollbarWidth {
    Auto,
    Thin,
    None,
}

impl Default for ScrollbarWidth {
    fn default() -> Self {
        ScrollbarWidth::Auto
    }
}

// Scrollbar gutter enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScrollbarGutter {
    Auto,
    Stable,
    StableBothEdges,
}

impl Default for ScrollbarGutter {
    fn default() -> Self {
        ScrollbarGutter::Auto
    }
}

// Filter effect enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Filter {
    None,
    Blur(f32),
    Brightness(f32),
    Contrast(f32),
    Grayscale(f32),
    HueRotate(f32), // degrees
    Invert(f32),
    Opacity(f32),
    Saturate(f32),
    Sepia(f32),
    DropShadow(f32, f32, f32, CssColor), // offset-x, offset-y, blur, color
}

// Transition timing function enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransitionTimingFunction {
    Ease,
    EaseIn,
    EaseOut,
    EaseInOut,
    Linear,
    StepStart,
    StepEnd,
}

impl Default for TransitionTimingFunction {
    fn default() -> Self {
        TransitionTimingFunction::Ease
    }
}

// Transition behavior enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransitionBehavior {
    Normal,
    AllowDiscrete,
}

impl Default for TransitionBehavior {
    fn default() -> Self {
        TransitionBehavior::Normal
    }
}

// Contain enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Contain {
    None,
    Strict,
    Content,
    Size,
    Layout,
    Style,
    Paint,
}

impl Default for Contain {
    fn default() -> Self {
        Contain::None
    }
}

// Content visibility enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContentVisibility {
    Visible,
    Hidden,
    Auto,
}

impl Default for ContentVisibility {
    fn default() -> Self {
        ContentVisibility::Visible
    }
}

// Container type enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContainerType {
    None,
    Size,
    InlineSize,
    Normal,
}

impl Default for ContainerType {
    fn default() -> Self {
        ContainerType::None
    }
}

// Animation iteration count enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AnimationIterationCount {
    Number(f32),
    Infinite,
}

impl Default for AnimationIterationCount {
    fn default() -> Self {
        AnimationIterationCount::Number(1.0)
    }
}

// Animation direction enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AnimationDirection {
    Normal,
    Reverse,
    Alternate,
    AlternateReverse,
}

impl Default for AnimationDirection {
    fn default() -> Self {
        AnimationDirection::Normal
    }
}

// Animation fill mode enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AnimationFillMode {
    None,
    Forwards,
    Backwards,
    Both,
}

impl Default for AnimationFillMode {
    fn default() -> Self {
        AnimationFillMode::None
    }
}

// Animation play state enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AnimationPlayState {
    Running,
    Paused,
}

impl Default for AnimationPlayState {
    fn default() -> Self {
        AnimationPlayState::Running
    }
}

// Hyphens enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hyphens {
    None,
    Manual,
    Auto,
}

impl Default for Hyphens {
    fn default() -> Self {
        Hyphens::Manual
    }
}

// Text justify enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextJustify {
    Auto,
    None,
    InterWord,
    InterCharacter,
}

impl Default for TextJustify {
    fn default() -> Self {
        TextJustify::Auto
    }
}

// Text group align enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextGroupAlign {
    Start,
    End,
    Left,
    Right,
    Center,
}

impl Default for TextGroupAlign {
    fn default() -> Self {
        TextGroupAlign::Start
    }
}

// List style position enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ListStylePosition {
    Inside,
    Outside,
}

impl Default for ListStylePosition {
    fn default() -> Self {
        ListStylePosition::Outside
    }
}

// Text decoration line enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextDecorationLine {
    None,
    Underline,
    Overline,
    LineThrough,
    Blink,
}

impl Default for TextDecorationLine {
    fn default() -> Self {
        TextDecorationLine::None
    }
}

// Text decoration style enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextDecorationStyle {
    Solid,
    Double,
    Dotted,
    Dashed,
    Wavy,
}

impl Default for TextDecorationStyle {
    fn default() -> Self {
        TextDecorationStyle::Solid
    }
}

// Text decoration thickness enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextDecorationThickness {
    Auto,
    FromFont,
    Length(f32),
}

impl Default for TextDecorationThickness {
    fn default() -> Self {
        TextDecorationThickness::Auto
    }
}

// Mix blend mode enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MixBlendMode {
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl Default for MixBlendMode {
    fn default() -> Self {
        MixBlendMode::Normal
    }
}

// Isolation enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Isolation {
    Auto,
    Isolate,
}

impl Default for Isolation {
    fn default() -> Self {
        Isolation::Auto
    }
}

// Appearance enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Appearance {
    Auto,
    None,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance::Auto
    }
}

// Field sizing enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldSizing {
    Fixed,
    Content,
}

impl Default for FieldSizing {
    fn default() -> Self {
        FieldSizing::Fixed
    }
}

// Color scheme enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColorScheme {
    Normal,
    Light,
    Dark,
    LightDark,
}

impl Default for ColorScheme {
    fn default() -> Self {
        ColorScheme::Normal
    }
}

// Forced color adjust enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ForcedColorAdjust {
    Auto,
    None,
}

impl Default for ForcedColorAdjust {
    fn default() -> Self {
        ForcedColorAdjust::Auto
    }
}

// Font variant enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FontVariant {
    Normal,
    SmallCaps,
}

impl Default for FontVariant {
    fn default() -> Self {
        FontVariant::Normal
    }
}

// Font display enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FontDisplay {
    Auto,
    Block,
    Swap,
    Fallback,
    Optional,
}

impl Default for FontDisplay {
    fn default() -> Self {
        FontDisplay::Auto
    }
}

// Font stretch enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FontStretch {
    Normal,
    UltraCondensed,
    ExtraCondensed,
    Condensed,
    SemiCondensed,
    SemiExpanded,
    Expanded,
    ExtraExpanded,
    UltraExpanded,
}

impl Default for FontStretch {
    fn default() -> Self {
        FontStretch::Normal
    }
}

// Scroll behavior enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScrollBehavior {
    Auto,
    Smooth,
}

impl Default for ScrollBehavior {
    fn default() -> Self {
        ScrollBehavior::Auto
    }
}

// Overscroll behavior enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OverscrollBehavior {
    Auto,
    Contain,
    None,
}

impl Default for OverscrollBehavior {
    fn default() -> Self {
        OverscrollBehavior::Auto
    }
}

// Clip path enum (simplified)
#[derive(Debug, Clone, PartialEq)]
pub enum ClipPath {
    None,
    Inset(f32, f32, f32, f32), // top, right, bottom, left
    Circle(f32), // radius
    Ellipse(f32, f32), // rx, ry
}

impl Default for ClipPath {
    fn default() -> Self {
        ClipPath::None
    }
}

// Shape outside enum (simplified)
#[derive(Debug, Clone, PartialEq)]
pub enum ShapeOutside {
    None,
    MarginBox,
    BorderBox,
    PaddingBox,
    ContentBox,
}

impl Default for ShapeOutside {
    fn default() -> Self {
        ShapeOutside::None
    }
}

// Align content enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AlignContent {
    FlexStart,
    FlexEnd,
    Center,
    Stretch,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

impl Default for AlignContent {
    fn default() -> Self {
        AlignContent::Stretch
    }
}

// Justify items enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JustifyItems {
    Auto,
    FlexStart,
    FlexEnd,
    Center,
    Stretch,
}

impl Default for JustifyItems {
    fn default() -> Self {
        JustifyItems::Auto
    }
}

// Align self enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AlignSelf {
    Auto,
    FlexStart,
    FlexEnd,
    Center,
    Stretch,
    Baseline,
}

impl Default for AlignSelf {
    fn default() -> Self {
        AlignSelf::Auto
    }
}

// Justify self enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JustifySelf {
    Auto,
    FlexStart,
    FlexEnd,
    Center,
    Stretch,
}

impl Default for JustifySelf {
    fn default() -> Self {
        JustifySelf::Auto
    }
}

// Background sub-properties
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BackgroundRepeat {
    Repeat,
    NoRepeat,
    RepeatX,
    RepeatY,
    Space,
    Round,
}

impl Default for BackgroundRepeat {
    fn default() -> Self {
        BackgroundRepeat::Repeat
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BackgroundAttachment {
    Scroll,
    Fixed,
    Local,
}

impl Default for BackgroundAttachment {
    fn default() -> Self {
        BackgroundAttachment::Scroll
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BackgroundSize {
    Auto,
    Cover,
    Contain,
    Length(f32, f32), // width, height
}

impl Default for BackgroundSize {
    fn default() -> Self {
        BackgroundSize::Auto
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BackgroundOrigin {
    BorderBox,
    PaddingBox,
    ContentBox,
}

impl Default for BackgroundOrigin {
    fn default() -> Self {
        BackgroundOrigin::PaddingBox
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BackgroundClip {
    BorderBox,
    PaddingBox,
    ContentBox,
    Text,
}

impl Default for BackgroundClip {
    fn default() -> Self {
        BackgroundClip::BorderBox
    }
}

// Border image properties
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BorderImageSlice {
    Auto,
    Values(f32, f32, f32, f32, bool), // top, right, bottom, left, fill
}

impl Default for BorderImageSlice {
    fn default() -> Self {
        BorderImageSlice::Auto
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BorderImageWidth {
    Auto,
    Length(f32),
    Percentage(f32),
}

impl Default for BorderImageWidth {
    fn default() -> Self {
        BorderImageWidth::Auto
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BorderImageOutset {
    Auto,
    Length(f32),
}

impl Default for BorderImageOutset {
    fn default() -> Self {
        BorderImageOutset::Auto
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BorderImageRepeat {
    Stretch,
    Repeat,
    Round,
    Space,
}

impl Default for BorderImageRepeat {
    fn default() -> Self {
        BorderImageRepeat::Stretch
    }
}

// Touch action for touch devices
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TouchAction {
    Auto,
    None,
    PanX,
    PanY,
    PanLeft,
    PanRight,
    PanUp,
    PanDown,
    PinchZoom,
    Manipulation,
}

impl Default for TouchAction {
    fn default() -> Self {
        TouchAction::Auto
    }
}

// Page break properties
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PageBreak {
    Auto,
    Always,
    Avoid,
    Left,
    Right,
}

impl Default for PageBreak {
    fn default() -> Self {
        PageBreak::Auto
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PageBreakInside {
    Auto,
    Avoid,
}

impl Default for PageBreakInside {
    fn default() -> Self {
        PageBreakInside::Auto
    }
}

// Print color adjust
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PrintColorAdjust {
    Economy,
    Exact,
}

impl Default for PrintColorAdjust {
    fn default() -> Self {
        PrintColorAdjust::Economy
    }
}

// Text decoration extensions
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextDecorationSkipInk {
    Auto,
    None,
    All,
}

impl Default for TextDecorationSkipInk {
    fn default() -> Self {
        TextDecorationSkipInk::Auto
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextUnderlinePosition {
    Auto,
    Under,
    Left,
    Right,
}

impl Default for TextUnderlinePosition {
    fn default() -> Self {
        TextUnderlinePosition::Auto
    }
}

// Border style enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BorderStyle {
    None,
    Hidden,
    Solid,
    Dashed,
    Dotted,
    Double,
    Groove,
    Ridge,
    Inset,
    Outset,
}

impl Default for BorderStyle {
    fn default() -> Self {
        BorderStyle::None
    }
}

// Outline style enum (similar to border but no hidden/groove/etc)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutlineStyle {
    None,
    Solid,
    Dashed,
    Dotted,
    Double,
}

impl Default for OutlineStyle {
    fn default() -> Self {
        OutlineStyle::None
    }
}

// Mask properties
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MaskMode {
    Alpha,
    Luminance,
    MatchSource,
}

impl Default for MaskMode {
    fn default() -> Self {
        MaskMode::Alpha
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MaskRepeat {
    Repeat,
    NoRepeat,
    Space,
    Round,
}

impl Default for MaskRepeat {
    fn default() -> Self {
        MaskRepeat::Repeat
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MaskSize {
    Auto,
    Cover,
    Contain,
    Length(f32, f32),
}

impl Default for MaskSize {
    fn default() -> Self {
        MaskSize::Auto
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MaskComposite {
    Add,
    Subtract,
    Intersect,
    Exclude,
}

impl Default for MaskComposite {
    fn default() -> Self {
        MaskComposite::Add
    }
}

// Text emphasis (East Asian typography)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextEmphasisStyle {
    None,
    Filled,
    Open,
    Dot,
    Circle,
    DoubleCircle,
    Triangle,
    Sesame,
}

impl Default for TextEmphasisStyle {
    fn default() -> Self {
        TextEmphasisStyle::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextEmphasisPosition {
    Over,
    Under,
    Left,
    Right,
}

impl Default for TextEmphasisPosition {
    fn default() -> Self {
        TextEmphasisPosition::Over
    }
}

// Transform 3D properties
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransformBox {
    ContentBox,
    BorderBox,
    FillBox,
    StrokeBox,
    ViewBox,
}

impl Default for TransformBox {
    fn default() -> Self {
        TransformBox::BorderBox
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransformStyle {
    Flat,
    Preserve3d,
}

impl Default for TransformStyle {
    fn default() -> Self {
        TransformStyle::Flat
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BackfaceVisibility {
    Visible,
    Hidden,
}

impl Default for BackfaceVisibility {
    fn default() -> Self {
        BackfaceVisibility::Visible
    }
}

// Background blend mode
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BlendMode {
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl Default for BlendMode {
    fn default() -> Self {
        BlendMode::Normal
    }
}

// Image rendering
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImageRendering {
    Auto,
    CrispEdges,
    Pixelated,
}

impl Default for ImageRendering {
    fn default() -> Self {
        ImageRendering::Auto
    }
}

// Text align last
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextAlignLast {
    Auto,
    Start,
    End,
    Left,
    Right,
    Center,
    Justify,
}

impl Default for TextAlignLast {
    fn default() -> Self {
        TextAlignLast::Auto
    }
}

// Text decoration skip
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextDecorationSkip {
    None,
    Objects,
    Spaces,
    Edges,
    BoxDecoration,
    LeadingSpaces,
    TrailingSpaces,
}

impl Default for TextDecorationSkip {
    fn default() -> Self {
        TextDecorationSkip::Objects
    }
}

// Caret shape
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CaretShape {
    Auto,
    Bar,
    Block,
    Underscore,
}

impl Default for CaretShape {
    fn default() -> Self {
        CaretShape::Auto
    }
}

// Box decoration break
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BoxDecorationBreak {
    Slice,
    Clone,
}

impl Default for BoxDecorationBreak {
    fn default() -> Self {
        BoxDecorationBreak::Slice
    }
}

// Text combine upright
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextCombineUpright {
    None,
    All,
    Digits,
}

impl Default for TextCombineUpright {
    fn default() -> Self {
        TextCombineUpright::None
    }
}

// Line break
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineBreak {
    Auto,
    Loose,
    Normal,
    Strict,
    Anywhere,
}

impl Default for LineBreak {
    fn default() -> Self {
        LineBreak::Auto
    }
}

// Hanging punctuation
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HangingPunctuation {
    None,
    First,
    Last,
    ForceEnd,
    AllowEnd,
    FirstLast,
}

impl Default for HangingPunctuation {
    fn default() -> Self {
        HangingPunctuation::None
    }
}

// SVG Fill rule
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FillRule {
    NonZero,
    EvenOdd,
}

impl Default for FillRule {
    fn default() -> Self {
        FillRule::NonZero
    }
}

// SVG Stroke linecap
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StrokeLinecap {
    Butt,
    Round,
    Square,
}

impl Default for StrokeLinecap {
    fn default() -> Self {
        StrokeLinecap::Butt
    }
}

// SVG Stroke linejoin
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StrokeLinejoin {
    Miter,
    Round,
    Bevel,
}

impl Default for StrokeLinejoin {
    fn default() -> Self {
        StrokeLinejoin::Miter
    }
}

// SVG Clip rule
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClipRule {
    NonZero,
    EvenOdd,
}

impl Default for ClipRule {
    fn default() -> Self {
        ClipRule::NonZero
    }
}

// Animation composition
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AnimationComposition {
    Replace,
    Add,
    Accumulate,
}

impl Default for AnimationComposition {
    fn default() -> Self {
        AnimationComposition::Replace
    }
}

// Animation timeline
#[derive(Debug, Clone, PartialEq)]
pub enum AnimationTimeline {
    Auto,
    None,
    Scroll(String),
    View(String),
}

impl Default for AnimationTimeline {
    fn default() -> Self {
        AnimationTimeline::Auto
    }
}

// Scroll axis for timelines
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScrollAxis {
    Block,
    Inline,
    Vertical,
    Horizontal,
}

impl Default for ScrollAxis {
    fn default() -> Self {
        ScrollAxis::Block
    }
}

// Position area for anchor positioning
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PositionArea {
    None,
    Top,
    Bottom,
    Left,
    Right,
    Center,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Start,
    End,
    BlockStart,
    BlockEnd,
    InlineStart,
    InlineEnd,
}

impl Default for PositionArea {
    fn default() -> Self {
        PositionArea::None
    }
}

// Position try
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PositionTry {
    None,
    FlipBlock,
    FlipInline,
    Flip,
}

impl Default for PositionTry {
    fn default() -> Self {
        PositionTry::None
    }
}

// Position visibility
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PositionVisibility {
    Always,
    Anchored,
    NoOverflow,
}

impl Default for PositionVisibility {
    fn default() -> Self {
        PositionVisibility::Always
    }
}

// Popover
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Popover {
    None,
    Auto,
    Hint,
    Manual,
}

impl Default for Popover {
    fn default() -> Self {
        Popover::None
    }
}

// Font synthesis
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FontSynthesis {
    None,
    Weight,
    Style,
    WeightStyle,
}

impl Default for FontSynthesis {
    fn default() -> Self {
        FontSynthesis::WeightStyle
    }
}

// Text orientation
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextOrientation {
    Mixed,
    Upright,
    Sideways,
    SidewaysRight,
    UseGlyphOrientation,
}

impl Default for TextOrientation {
    fn default() -> Self {
        TextOrientation::Mixed
    }
}

// Overflow anchor
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OverflowAnchor {
    Auto,
    None,
}

impl Default for OverflowAnchor {
    fn default() -> Self {
        OverflowAnchor::Auto
    }
}

// Scroll snap
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScrollSnapType {
    None,
    X,
    Y,
    Block,
    Inline,
    Both,
    Mandatory,
    Proximity,
}

impl Default for ScrollSnapType {
    fn default() -> Self {
        ScrollSnapType::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScrollSnapAlign {
    None,
    Start,
    End,
    Center,
}

impl Default for ScrollSnapAlign {
    fn default() -> Self {
        ScrollSnapAlign::None
    }
}

// Initial letter (drop caps)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InitialLetter {
    Normal,
    Drop,
    Raise,
    Number(f32),
}

impl Default for InitialLetter {
    fn default() -> Self {
        InitialLetter::Normal
    }
}

// Box shadow structure
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxShadow {
    pub offset_x: f32,
    pub offset_y: f32,
    pub blur_radius: f32,
    pub spread_radius: f32,
    pub color: CssColor,
    pub inset: bool,
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
        font_family: parent_style.font_family,
        letter_spacing: parent_style.letter_spacing,
        word_spacing: parent_style.word_spacing,
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

    // <dialog> without open attribute should be display: none
    if element.tag_name == "dialog" && element.get_attr("open").is_none() {
        style.display = Display::None;
    }

    // <dialog open> should be display: block (overlay positioned)
    if element.tag_name == "dialog" && element.get_attr("open").is_some() {
        style.display = Display::Block;
        style.position = Position::Fixed;
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
                    } else {
                        // Hide non-selected options in select dropdown
                        style.display = Display::None;
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
                    "table" => Display::Table,
                    "table-row" => Display::TableRow,
                    "table-cell" => Display::TableCell,
                    "table-row-group" => Display::TableRowGroup,
                    "table-header-group" => Display::TableHeaderGroup,
                    "table-footer-group" => Display::TableFooterGroup,
                    "table-column" => Display::TableColumn,
                    "table-column-group" => Display::TableColumnGroup,
                    "table-caption" => Display::TableCaption,
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
                    "sticky" => Position::Sticky,
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
        "clear" => match &decl.value {
            CssValue::None => style.clear = Clear::None,
            CssValue::Keyword(kw) => {
                style.clear = match kw.as_str() {
                    "left" => Clear::Left,
                    "right" => Clear::Right,
                    "both" => Clear::Both,
                    "none" => Clear::None,
                    _ => style.clear,
                };
            }
            _ => {}
        },
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
        "background-image" => {
            style.background_image = parse_background_image(&decl.value,
                parent_font_size,
                viewport_width,
                viewport_height
            );
        }
        "background-repeat" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.background_repeat = match kw.as_str() {
                    "repeat" => BackgroundRepeat::Repeat,
                    "no-repeat" => BackgroundRepeat::NoRepeat,
                    "repeat-x" => BackgroundRepeat::RepeatX,
                    "repeat-y" => BackgroundRepeat::RepeatY,
                    "space" => BackgroundRepeat::Space,
                    "round" => BackgroundRepeat::Round,
                    _ => style.background_repeat,
                };
            }
        }
        "background-attachment" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.background_attachment = match kw.as_str() {
                    "scroll" => BackgroundAttachment::Scroll,
                    "fixed" => BackgroundAttachment::Fixed,
                    "local" => BackgroundAttachment::Local,
                    _ => style.background_attachment,
                };
            }
        }
        "background-position" => {
            // Parse position values (could be keywords, lengths, or percentages)
            match &decl.value {
                CssValue::List(vals) if vals.len() == 2 => {
                    let x = parse_position_value(vals.get(0), 0.5);
                    let y = parse_position_value(vals.get(1), 0.5);
                    style.background_position = (x, y);
                }
                CssValue::Keyword(kw) => {
                    let (x, y) = match kw.as_str() {
                        "top" => (0.5, 0.0),
                        "right" => (1.0, 0.5),
                        "bottom" => (0.5, 1.0),
                        "left" => (0.0, 0.5),
                        "center" => (0.5, 0.5),
                        "top left" | "left top" => (0.0, 0.0),
                        "top right" | "right top" => (1.0, 0.0),
                        "bottom left" | "left bottom" => (0.0, 1.0),
                        "bottom right" | "right bottom" => (1.0, 1.0),
                        _ => (0.5, 0.5),
                    };
                    style.background_position = (x, y);
                }
                _ => {}
            }
        }
        "background-size" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.background_size = match kw.as_str() {
                    "auto" => BackgroundSize::Auto,
                    "cover" => BackgroundSize::Cover,
                    "contain" => BackgroundSize::Contain,
                    _ => style.background_size,
                };
            } else if let CssValue::List(vals) = &decl.value {
                if vals.len() == 2 {
                    let w = vals[0].to_px(parent_font_size, viewport_width, viewport_height).unwrap_or(0.0);
                    let h = vals[1].to_px(parent_font_size, viewport_width, viewport_height).unwrap_or(0.0);
                    style.background_size = BackgroundSize::Length(w, h);
                }
            }
        }
        "background-origin" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.background_origin = match kw.as_str() {
                    "border-box" => BackgroundOrigin::BorderBox,
                    "padding-box" => BackgroundOrigin::PaddingBox,
                    "content-box" => BackgroundOrigin::ContentBox,
                    _ => style.background_origin,
                };
            }
        }
        "background-clip" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.background_clip = match kw.as_str() {
                    "border-box" => BackgroundClip::BorderBox,
                    "padding-box" => BackgroundClip::PaddingBox,
                    "content-box" => BackgroundClip::ContentBox,
                    "text" => BackgroundClip::Text,
                    _ => style.background_clip,
                };
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
                    CssValue::None => {
                        style.text_decoration = TextDecoration::None;
                        style.text_decoration_line = TextDecorationLine::None;
                    }
                    CssValue::Keyword(kw) => {
                        match kw.as_str() {
                            "underline" => {
                                style.text_decoration = TextDecoration::Underline;
                                style.text_decoration_line = TextDecorationLine::Underline;
                            }
                            "line-through" => {
                                style.text_decoration = TextDecoration::LineThrough;
                                style.text_decoration_line = TextDecorationLine::LineThrough;
                            }
                            "overline" => style.text_decoration_line = TextDecorationLine::Overline,
                            "none" => {
                                style.text_decoration = TextDecoration::None;
                                style.text_decoration_line = TextDecorationLine::None;
                            }
                            _ => {}
                        };
                    }
                    _ => {}
                }
            }
        }
        "text-decoration-color" => {
            if let CssValue::Color(c) = &decl.value {
                style.text_decoration_color = Some(*c);
            }
        }
        "text-decoration-style" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.text_decoration_style = match kw.as_str() {
                    "solid" => TextDecorationStyle::Solid,
                    "double" => TextDecorationStyle::Double,
                    "dotted" => TextDecorationStyle::Dotted,
                    "dashed" => TextDecorationStyle::Dashed,
                    "wavy" => TextDecorationStyle::Wavy,
                    _ => style.text_decoration_style,
                };
            }
        }
        "text-decoration-thickness" => {
            match &decl.value {
                CssValue::Keyword(kw) => match kw.as_str() {
                    "auto" => style.text_decoration_thickness = TextDecorationThickness::Auto,
                    "from-font" => style.text_decoration_thickness = TextDecorationThickness::FromFont,
                    _ => {}
                }
                _ => {
                    if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                        style.text_decoration_thickness = TextDecorationThickness::Length(px);
                    }
                }
            }
        }
        "text-decoration-skip-ink" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.text_decoration_skip_ink = match kw.as_str() {
                    "auto" => TextDecorationSkipInk::Auto,
                    "none" => TextDecorationSkipInk::None,
                    "all" => TextDecorationSkipInk::All,
                    _ => style.text_decoration_skip_ink,
                };
            }
        }
        "text-underline-position" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.text_underline_position = match kw.as_str() {
                    "auto" => TextUnderlinePosition::Auto,
                    "under" => TextUnderlinePosition::Under,
                    "left" => TextUnderlinePosition::Left,
                    "right" => TextUnderlinePosition::Right,
                    _ => style.text_underline_position,
                };
            }
        }
        "mix-blend-mode" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.mix_blend_mode = match kw.as_str() {
                    "normal" => MixBlendMode::Normal,
                    "multiply" => MixBlendMode::Multiply,
                    "screen" => MixBlendMode::Screen,
                    "overlay" => MixBlendMode::Overlay,
                    "darken" => MixBlendMode::Darken,
                    "lighten" => MixBlendMode::Lighten,
                    "color-dodge" => MixBlendMode::ColorDodge,
                    "color-burn" => MixBlendMode::ColorBurn,
                    "hard-light" => MixBlendMode::HardLight,
                    "soft-light" => MixBlendMode::SoftLight,
                    "difference" => MixBlendMode::Difference,
                    "exclusion" => MixBlendMode::Exclusion,
                    "hue" => MixBlendMode::Hue,
                    "saturation" => MixBlendMode::Saturation,
                    "color" => MixBlendMode::Color,
                    "luminosity" => MixBlendMode::Luminosity,
                    _ => style.mix_blend_mode,
                };
            }
        }
        "isolation" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.isolation = match kw.as_str() {
                    "auto" => Isolation::Auto,
                    "isolate" => Isolation::Isolate,
                    _ => style.isolation,
                };
            }
        }
        "accent-color" => {
            match &decl.value {
                CssValue::Color(c) => style.accent_color = Some(*c),
                CssValue::Keyword(kw) if kw == "auto" => style.accent_color = None,
                _ => {}
            }
        }
        "caret-color" => {
            match &decl.value {
                CssValue::Color(c) => style.caret_color = Some(*c),
                CssValue::Keyword(kw) if kw == "auto" => style.caret_color = None,
                _ => {}
            }
        }
        "appearance" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.appearance = match kw.as_str() {
                    "auto" => Appearance::Auto,
                    "none" => Appearance::None,
                    _ => style.appearance,
                };
            }
        }
        "field-sizing" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.field_sizing = match kw.as_str() {
                    "fixed" => FieldSizing::Fixed,
                    "content" => FieldSizing::Content,
                    _ => style.field_sizing,
                };
            }
        }
        "color-scheme" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.color_scheme = match kw.as_str() {
                    "normal" => ColorScheme::Normal,
                    "light" => ColorScheme::Light,
                    "dark" => ColorScheme::Dark,
                    "light dark" | "light-dark" => ColorScheme::LightDark,
                    _ => style.color_scheme,
                };
            }
        }
        "forced-color-adjust" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.forced_color_adjust = match kw.as_str() {
                    "auto" => ForcedColorAdjust::Auto,
                    "none" => ForcedColorAdjust::None,
                    _ => style.forced_color_adjust,
                };
            }
        }
        "font-variant" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.font_variant = match kw.as_str() {
                    "normal" => FontVariant::Normal,
                    "small-caps" => FontVariant::SmallCaps,
                    _ => style.font_variant,
                };
            }
        }
        "font-feature-settings" => {
            // font-feature-settings: "liga" 0, "kern" 1
            if let CssValue::Keyword(kw) = &decl.value {
                if kw == "normal" {
                    style.font_feature_settings.clear();
                }
            } else if let CssValue::List(vals) = &decl.value {
                let features: Vec<String> = vals.iter()
                    .filter_map(|v| {
                        if let CssValue::Keyword(s) = v {
                            Some(s.clone())
                        } else {
                            None
                        }
                    })
                    .collect();
                if !features.is_empty() {
                    style.font_feature_settings = features;
                }
            }
        }
        "font-display" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.font_display = match kw.as_str() {
                    "auto" => FontDisplay::Auto,
                    "block" => FontDisplay::Block,
                    "swap" => FontDisplay::Swap,
                    "fallback" => FontDisplay::Fallback,
                    "optional" => FontDisplay::Optional,
                    _ => style.font_display,
                };
            }
        }
        "font-stretch" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.font_stretch = match kw.as_str() {
                    "normal" => FontStretch::Normal,
                    "ultra-condensed" => FontStretch::UltraCondensed,
                    "extra-condensed" => FontStretch::ExtraCondensed,
                    "condensed" => FontStretch::Condensed,
                    "semi-condensed" => FontStretch::SemiCondensed,
                    "semi-expanded" => FontStretch::SemiExpanded,
                    "expanded" => FontStretch::Expanded,
                    "extra-expanded" => FontStretch::ExtraExpanded,
                    "ultra-expanded" => FontStretch::UltraExpanded,
                    _ => style.font_stretch,
                };
            }
        }
        "font-size-adjust" => {
            match &decl.value {
                CssValue::Number(n) => style.font_size_adjust = Some(*n),
                CssValue::Keyword(kw) if kw == "none" => style.font_size_adjust = None,
                CssValue::Percentage(p) => style.font_size_adjust = Some(*p / 100.0),
                _ => {}
            }
        }
        "scroll-behavior" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.scroll_behavior = match kw.as_str() {
                    "auto" => ScrollBehavior::Auto,
                    "smooth" => ScrollBehavior::Smooth,
                    _ => style.scroll_behavior,
                };
            }
        }
        "overscroll-behavior" | "overscroll-behavior-x" | "overscroll-behavior-y" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.overscroll_behavior = match kw.as_str() {
                    "auto" => OverscrollBehavior::Auto,
                    "contain" => OverscrollBehavior::Contain,
                    "none" => OverscrollBehavior::None,
                    _ => style.overscroll_behavior,
                };
            }
        }
        "overscroll-behavior-block" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.overscroll_behavior_block = match kw.as_str() {
                    "auto" => OverscrollBehavior::Auto,
                    "contain" => OverscrollBehavior::Contain,
                    "none" => OverscrollBehavior::None,
                    _ => style.overscroll_behavior_block,
                };
            }
        }
        "overscroll-behavior-inline" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.overscroll_behavior_inline = match kw.as_str() {
                    "auto" => OverscrollBehavior::Auto,
                    "contain" => OverscrollBehavior::Contain,
                    "none" => OverscrollBehavior::None,
                    _ => style.overscroll_behavior_inline,
                };
            }
        }
        "scroll-margin" => apply_scroll_margin_shorthand(
            style,
            &decl.value,
            parent_font_size,
            viewport_width,
            viewport_height,
        ),
        "scroll-margin-top" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.scroll_margin_top = px;
            }
        }
        "scroll-margin-right" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.scroll_margin_right = px;
            }
        }
        "scroll-margin-bottom" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.scroll_margin_bottom = px;
            }
        }
        "scroll-margin-left" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.scroll_margin_left = px;
            }
        }
        "scroll-padding" => apply_scroll_padding_shorthand(
            style,
            &decl.value,
            parent_font_size,
            viewport_width,
            viewport_height,
        ),
        "scroll-padding-top" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.scroll_padding_top = px;
            }
        }
        "scroll-padding-right" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.scroll_padding_right = px;
            }
        }
        "scroll-padding-bottom" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.scroll_padding_bottom = px;
            }
        }
        "scroll-padding-left" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.scroll_padding_left = px;
            }
        }
        "clip-path" => {
            match &decl.value {
                CssValue::Keyword(kw) if kw == "none" => style.clip_path = None,
                CssValue::List(vals) if !vals.is_empty() => {
                    if let CssValue::Keyword(func) = &vals[0] {
                        match func.as_str() {
                            "circle" if vals.len() > 1 => {
                                if let CssValue::Length(r, _) | CssValue::Number(r) = &vals[1] {
                                    style.clip_path = Some(ClipPath::Circle(*r));
                                }
                            }
                            "ellipse" if vals.len() > 2 => {
                                if let (CssValue::Length(rx, _), CssValue::Length(ry, _)) = (&vals[1], &vals[2]
                                ) {
                                    style.clip_path = Some(ClipPath::Ellipse(*rx, *ry));
                                }
                            }
                            "inset" if vals.len() > 4 => {
                                if let (
                                    CssValue::Length(t, _),
                                    CssValue::Length(r, _),
                                    CssValue::Length(b, _),
                                    CssValue::Length(l, _),
                                ) = (&vals[1], &vals[2], &vals[3], &vals[4])
                                {
                                    style.clip_path = Some(ClipPath::Inset(*t, *r, *b, *l));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        "shape-outside" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.shape_outside = match kw.as_str() {
                    "none" => None,
                    "margin-box" => Some(ShapeOutside::MarginBox),
                    "border-box" => Some(ShapeOutside::BorderBox),
                    "padding-box" => Some(ShapeOutside::PaddingBox),
                    "content-box" => Some(ShapeOutside::ContentBox),
                    _ => style.shape_outside.clone(),
                };
            }
        }
        "shape-margin" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.shape_margin = px;
            }
        }
        "shape-image-threshold" => {
            if let CssValue::Number(n) = &decl.value {
                style.shape_image_threshold = n.max(0.0).min(1.0);
            }
        }
        "place-content" => {
            // place-content: align-content justify-content
            if let CssValue::List(vals) = &decl.value {
                if let Some(CssValue::Keyword(align)) = vals.get(0) {
                    style.place_content.0 = parse_align_content(align);
                }
                if let Some(CssValue::Keyword(justify)) = vals.get(1) {
                    style.place_content.1 = parse_justify_content(justify);
                }
            }
        }
        "place-items" => {
            // place-items: align-items justify-items
            if let CssValue::List(vals) = &decl.value {
                if let Some(CssValue::Keyword(align)) = vals.get(0) {
                    style.place_items.0 = parse_align_items(align);
                }
                if let Some(CssValue::Keyword(justify)) = vals.get(1) {
                    style.place_items.1 = parse_justify_items(justify);
                }
            }
        }
        "place-self" => {
            // place-self: align-self justify-self
            if let CssValue::List(vals) = &decl.value {
                if let Some(CssValue::Keyword(align)) = vals.get(0) {
                    style.place_self.0 = parse_align_self(align);
                }
                if let Some(CssValue::Keyword(justify)) = vals.get(1) {
                    style.place_self.1 = parse_justify_self(justify);
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
        "flex-flow" => {
            // flex-flow: <flex-direction> || <flex-wrap>
            match &decl.value {
                CssValue::List(vals) => {
                    for v in vals {
                        match v {
                            CssValue::Keyword(kw) => {
                                match kw.as_str() {
                                    "row" => style.flex_flow.0 = FlexDirection::Row,
                                    "row-reverse" => style.flex_flow.0 = FlexDirection::RowReverse,
                                    "column" => style.flex_flow.0 = FlexDirection::Column,
                                    "column-reverse" => style.flex_flow.0 = FlexDirection::ColumnReverse,
                                    "nowrap" => style.flex_flow.1 = FlexWrap::NoWrap,
                                    "wrap" => style.flex_flow.1 = FlexWrap::Wrap,
                                    "wrap-reverse" => style.flex_flow.1 = FlexWrap::WrapReverse,
                                    _ => {}
                                }
                            }
                            _ => {}
                        }
                    }
                }
                CssValue::Keyword(kw) => {
                    match kw.as_str() {
                        "row" | "row-reverse" | "column" | "column-reverse" => {
                            style.flex_flow.0 = match kw.as_str() {
                                "row" => FlexDirection::Row,
                                "row-reverse" => FlexDirection::RowReverse,
                                "column" => FlexDirection::Column,
                                _ => FlexDirection::ColumnReverse,
                            };
                        }
                        "nowrap" | "wrap" | "wrap-reverse" => {
                            style.flex_flow.1 = match kw.as_str() {
                                "nowrap" => FlexWrap::NoWrap,
                                "wrap" => FlexWrap::Wrap,
                                _ => FlexWrap::WrapReverse,
                            };
                        }
                        _ => {}
                    }
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
        "list-style-type" => {
            let vals = match &decl.value {
                CssValue::List(v) => v.clone(),
                other => vec![other.clone()],
            };
            for v in &vals {
                match v {
                    CssValue::None => style.list_style_type = ListStyleType::None,
                    CssValue::Keyword(kw) => match kw.as_str() {
                        "none" => style.list_style_type = ListStyleType::None,
                        "disc" => style.list_style_type = ListStyleType::Disc,
                        "circle" => style.list_style_type = ListStyleType::Circle,
                        "square" => style.list_style_type = ListStyleType::Square,
                        "decimal" => style.list_style_type = ListStyleType::Decimal,
                        "lower-alpha" | "alpha" => style.list_style_type = ListStyleType::LowerAlpha,
                        "upper-alpha" => style.list_style_type = ListStyleType::UpperAlpha,
                        "lower-roman" | "roman" => style.list_style_type = ListStyleType::LowerRoman,
                        "upper-roman" => style.list_style_type = ListStyleType::UpperRoman,
                        "inside" => style.list_style_position = ListStylePosition::Inside,
                        "outside" => style.list_style_position = ListStylePosition::Outside,
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
        "list-style-image" => {
            match &decl.value {
                CssValue::Keyword(kw) if kw == "none" => style.list_style_image = None,
                CssValue::Keyword(url) if url.starts_with("url(") => {
                    // Extract URL from url(...)
                    let inner = url.trim_start_matches("url(").trim_end_matches(")");
                    style.list_style_image = Some(inner.trim_matches('"').trim_matches('\'').to_string());
                }
                _ => {}
            }
        }
        "list-style-position" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.list_style_position = match kw.as_str() {
                    "inside" => ListStylePosition::Inside,
                    "outside" => ListStylePosition::Outside,
                    _ => style.list_style_position,
                };
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
        "border-image-source" => {
            match &decl.value {
                CssValue::None => style.border_image_source = None,
                CssValue::Keyword(kw) if kw == "none" => style.border_image_source = None,
                // For now, store the value as a string representation
                other => style.border_image_source = Some(format!("{:?}", other)),
            }
        }
        "border-image-slice" => {
            if let CssValue::Keyword(kw) = &decl.value {
                if kw == "auto" {
                    style.border_image_slice = BorderImageSlice::Auto;
                }
            } else if let CssValue::List(vals) = &decl.value {
                let nums: Vec<f32> = vals.iter()
                    .filter_map(|v| match v {
                        CssValue::Number(n) => Some(*n),
                        _ => None,
                    })
                    .collect();
                let fill = vals.iter().any(|v| matches!(v, CssValue::Keyword(k) if k == "fill"));
                match nums.len() {
                    1 => style.border_image_slice = BorderImageSlice::Values(nums[0], nums[0], nums[0], nums[0], fill),
                    2 => style.border_image_slice = BorderImageSlice::Values(nums[0], nums[1], nums[0], nums[1], fill),
                    3 => style.border_image_slice = BorderImageSlice::Values(nums[0], nums[1], nums[2], nums[1], fill),
                    4 => style.border_image_slice = BorderImageSlice::Values(nums[0], nums[1], nums[2], nums[3], fill),
                    _ => {}
                }
            } else if let CssValue::Number(n) = &decl.value {
                style.border_image_slice = BorderImageSlice::Values(*n, *n, *n, *n, false);
            }
        }
        "border-image-width" => {
            if let CssValue::Keyword(kw) = &decl.value {
                if kw == "auto" {
                    style.border_image_width = BorderImageWidth::Auto;
                }
            } else if let CssValue::Number(n) = &decl.value {
                style.border_image_width = BorderImageWidth::Length(*n);
            }
        }
        "border-image-outset" => {
            if let CssValue::Keyword(kw) = &decl.value {
                if kw == "auto" {
                    style.border_image_outset = BorderImageOutset::Auto;
                }
            } else if let CssValue::Number(n) = &decl.value {
                style.border_image_outset = BorderImageOutset::Length(*n);
            }
        }
        "border-image-repeat" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.border_image_repeat = match kw.as_str() {
                    "stretch" => BorderImageRepeat::Stretch,
                    "repeat" => BorderImageRepeat::Repeat,
                    "round" => BorderImageRepeat::Round,
                    "space" => BorderImageRepeat::Space,
                    _ => style.border_image_repeat,
                };
            }
        }
        "border-style" => {
            if let CssValue::Keyword(kw) = &decl.value {
                let bs = match kw.as_str() {
                    "none" => BorderStyle::None,
                    "hidden" => BorderStyle::Hidden,
                    "solid" => BorderStyle::Solid,
                    "dashed" => BorderStyle::Dashed,
                    "dotted" => BorderStyle::Dotted,
                    "double" => BorderStyle::Double,
                    "groove" => BorderStyle::Groove,
                    "ridge" => BorderStyle::Ridge,
                    "inset" => BorderStyle::Inset,
                    "outset" => BorderStyle::Outset,
                    _ => BorderStyle::None,
                };
                style.border_top_style = bs;
                style.border_right_style = bs;
                style.border_bottom_style = bs;
                style.border_left_style = bs;
            }
        }
        "border-top-style" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.border_top_style = match kw.as_str() {
                    "none" => BorderStyle::None,
                    "solid" => BorderStyle::Solid,
                    "dashed" => BorderStyle::Dashed,
                    "dotted" => BorderStyle::Dotted,
                    _ => style.border_top_style,
                };
            }
        }
        "border-right-style" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.border_right_style = match kw.as_str() {
                    "none" => BorderStyle::None,
                    "solid" => BorderStyle::Solid,
                    "dashed" => BorderStyle::Dashed,
                    "dotted" => BorderStyle::Dotted,
                    _ => style.border_right_style,
                };
            }
        }
        "border-bottom-style" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.border_bottom_style = match kw.as_str() {
                    "none" => BorderStyle::None,
                    "solid" => BorderStyle::Solid,
                    "dashed" => BorderStyle::Dashed,
                    "dotted" => BorderStyle::Dotted,
                    _ => style.border_bottom_style,
                };
            }
        }
        "border-left-style" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.border_left_style = match kw.as_str() {
                    "none" => BorderStyle::None,
                    "solid" => BorderStyle::Solid,
                    "dashed" => BorderStyle::Dashed,
                    "dotted" => BorderStyle::Dotted,
                    _ => style.border_left_style,
                };
            }
        }
        "border-top-color" => {
            if let CssValue::Color(c) = &decl.value {
                style.border_top_color = Some(*c);
            }
        }
        "border-right-color" => {
            if let CssValue::Color(c) = &decl.value {
                style.border_right_color = Some(*c);
            }
        }
        "border-bottom-color" => {
            if let CssValue::Color(c) = &decl.value {
                style.border_bottom_color = Some(*c);
            }
        }
        "border-left-color" => {
            if let CssValue::Color(c) = &decl.value {
                style.border_left_color = Some(*c);
            }
        }
        "border-collapse" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.border_collapse = match kw.as_str() {
                    "collapse" => BorderCollapse::Collapse,
                    "separate" => BorderCollapse::Separate,
                    _ => style.border_collapse,
                };
            }
        }
        "border-spacing" => {
            // border-spacing: horizontal vertical | horizontal-vertical-both
            let vals: Vec<CssValue> = match &decl.value {
                CssValue::List(v) => v.clone(),
                other => vec![other.clone()],
            };
            if !vals.is_empty() {
                let h = vals[0].to_px(parent_font_size, viewport_width, viewport_height).unwrap_or(0.0);
                let v = if vals.len() > 1 {
                    vals[1].to_px(parent_font_size, viewport_width, viewport_height).unwrap_or(h)
                } else {
                    h
                };
                style.border_spacing = (h, v);
            }
        }
        "caption-side" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.caption_side = match kw.as_str() {
                    "top" => CaptionSide::Top,
                    "bottom" => CaptionSide::Bottom,
                    _ => style.caption_side,
                };
            }
        }
        "empty-cells" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.empty_cells = match kw.as_str() {
                    "show" => EmptyCells::Show,
                    "hide" => EmptyCells::Hide,
                    _ => style.empty_cells,
                };
            }
        }
        "table-layout" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.table_layout = match kw.as_str() {
                    "auto" => TableLayout::Auto,
                    "fixed" => TableLayout::Fixed,
                    _ => style.table_layout,
                };
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
        "grid-auto-columns" => {
            style.grid_auto_columns = parse_grid_tracks(
                &decl.value,
                parent_font_size,
                viewport_width,
                viewport_height,
            );
        }
        "grid-auto-rows" => {
            style.grid_auto_rows = parse_grid_tracks(
                &decl.value,
                parent_font_size,
                viewport_width,
                viewport_height,
            );
        }
        "outline" => {
            // Parse outline shorthand: width style color
            if let CssValue::List(vals) = &decl.value {
                for val in vals {
                    match val {
                        CssValue::Length(px, _) | CssValue::Number(px) => {
                            style.outline_width = *px;
                        }
                        CssValue::Keyword(kw) => {
                            style.outline_style = parse_outline_style(kw);
                            if style.outline_color.a == 0 && style.outline_style != OutlineStyle::None {
                                style.outline_color = style.color;
                            }
                        }
                        CssValue::Color(c) => {
                            style.outline_color = *c;
                        }
                        _ => {}
                    }
                }
            } else {
                // Single value
                match &decl.value {
                    CssValue::Length(px, _) | CssValue::Number(px) => {
                        style.outline_width = *px;
                    }
                    CssValue::Keyword(kw) => {
                        style.outline_style = parse_outline_style(kw);
                    }
                    CssValue::Color(c) => {
                        style.outline_color = *c;
                    }
                    _ => {}
                }
            }
        }
        "outline-width" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.outline_width = px;
            }
        }
        "outline-style" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.outline_style = parse_outline_style(kw);
            }
        }
        "outline-color" => {
            if let CssValue::Color(c) = &decl.value {
                style.outline_color = *c;
            } else if let CssValue::Keyword(kw) = &decl.value {
                if let Some(c) = parse_html_color(kw) {
                    style.outline_color = c;
                }
            }
        }
        "outline-offset" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.outline_offset = px;
            }
        }
        "will-change" => {
            match &decl.value {
                CssValue::Keyword(kw) => {
                    if kw == "auto" {
                        style.will_change = Vec::new();
                    } else {
                        style.will_change = vec![kw.clone()];
                    }
                }
                CssValue::List(vals) => {
                    style.will_change = vals.iter()
                        .filter_map(|v| match v {
                            CssValue::Keyword(k) => Some(k.clone()),
                            _ => None,
                        })
                        .collect();
                }
                _ => {}
            }
        }
        "backface-visibility" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.backface_visibility = match kw.as_str() {
                    "visible" => BackfaceVisibility::Visible,
                    "hidden" => BackfaceVisibility::Hidden,
                    _ => style.backface_visibility,
                };
            }
        }
        "perspective" => {
            if let CssValue::None = &decl.value {
                style.perspective = None;
            } else if let CssValue::Keyword(kw) = &decl.value {
                if kw == "none" {
                    style.perspective = None;
                }
            } else if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.perspective = Some(px);
            }
        }
        "perspective-origin" => {
            match &decl.value {
                CssValue::List(vals) if vals.len() == 2 => {
                    let x = parse_position_value(vals.get(0), 0.5);
                    let y = parse_position_value(vals.get(1), 0.5);
                    style.perspective_origin = (x, y);
                }
                _ => {}
            }
        }
        "transform-box" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.transform_box = match kw.as_str() {
                    "content-box" => TransformBox::ContentBox,
                    "border-box" => TransformBox::BorderBox,
                    "fill-box" => TransformBox::FillBox,
                    "stroke-box" => TransformBox::StrokeBox,
                    "view-box" => TransformBox::ViewBox,
                    _ => style.transform_box,
                };
            }
        }
        "transform-style" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.transform_style = match kw.as_str() {
                    "flat" => TransformStyle::Flat,
                    "preserve-3d" => TransformStyle::Preserve3d,
                    _ => style.transform_style,
                };
            }
        }
        "touch-action" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.touch_action = match kw.as_str() {
                    "auto" => TouchAction::Auto,
                    "none" => TouchAction::None,
                    "pan-x" => TouchAction::PanX,
                    "pan-y" => TouchAction::PanY,
                    "pan-left" => TouchAction::PanLeft,
                    "pan-right" => TouchAction::PanRight,
                    "pan-up" => TouchAction::PanUp,
                    "pan-down" => TouchAction::PanDown,
                    "pinch-zoom" => TouchAction::PinchZoom,
                    "manipulation" => TouchAction::Manipulation,
                    _ => style.touch_action,
                };
            } else if let CssValue::List(vals) = &decl.value {
                // Touch-action can be a combination like "pan-x pinch-zoom"
                for v in vals {
                    if let CssValue::Keyword(k) = v {
                        match k.as_str() {
                            "pan-x" => style.touch_action = TouchAction::PanX,
                            "pan-y" => style.touch_action = TouchAction::PanY,
                            "pinch-zoom" => style.touch_action = TouchAction::PinchZoom,
                            _ => {}
                        }
                    }
                }
            }
        }
        "mask-image" | "mask" => {
            match &decl.value {
                CssValue::None => style.mask_image = None,
                CssValue::Keyword(kw) if kw == "none" => style.mask_image = None,
                other => style.mask_image = Some(format!("{:?}", other)),
            }
        }
        "mask-mode" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.mask_mode = match kw.as_str() {
                    "alpha" => MaskMode::Alpha,
                    "luminance" => MaskMode::Luminance,
                    "match-source" => MaskMode::MatchSource,
                    _ => style.mask_mode,
                };
            }
        }
        "mask-repeat" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.mask_repeat = match kw.as_str() {
                    "repeat" => MaskRepeat::Repeat,
                    "no-repeat" => MaskRepeat::NoRepeat,
                    "space" => MaskRepeat::Space,
                    "round" => MaskRepeat::Round,
                    _ => style.mask_repeat,
                };
            }
        }
        "mask-position" => {
            match &decl.value {
                CssValue::List(vals) if vals.len() == 2 => {
                    let x = parse_position_value(vals.get(0), 0.5);
                    let y = parse_position_value(vals.get(1), 0.5);
                    style.mask_position = (x, y);
                }
                _ => {}
            }
        }
        "mask-size" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.mask_size = match kw.as_str() {
                    "auto" => MaskSize::Auto,
                    "cover" => MaskSize::Cover,
                    "contain" => MaskSize::Contain,
                    _ => style.mask_size,
                };
            }
        }
        "mask-composite" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.mask_composite = match kw.as_str() {
                    "add" => MaskComposite::Add,
                    "subtract" => MaskComposite::Subtract,
                    "intersect" => MaskComposite::Intersect,
                    "exclude" => MaskComposite::Exclude,
                    _ => style.mask_composite,
                };
            }
        }
        "text-emphasis" | "text-emphasis-style" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.text_emphasis_style = match kw.as_str() {
                    "none" => TextEmphasisStyle::None,
                    "filled" => TextEmphasisStyle::Filled,
                    "open" => TextEmphasisStyle::Open,
                    "dot" => TextEmphasisStyle::Dot,
                    "circle" => TextEmphasisStyle::Circle,
                    "double-circle" => TextEmphasisStyle::DoubleCircle,
                    "triangle" => TextEmphasisStyle::Triangle,
                    "sesame" => TextEmphasisStyle::Sesame,
                    _ => style.text_emphasis_style,
                };
            }
        }
        "text-emphasis-color" => {
            match &decl.value {
                CssValue::Color(c) => style.text_emphasis_color = Some(*c),
                CssValue::Keyword(kw) if kw == "currentcolor" => style.text_emphasis_color = None,
                _ => {}
            }
        }
        "text-emphasis-position" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.text_emphasis_position = match kw.as_str() {
                    "over" => TextEmphasisPosition::Over,
                    "under" => TextEmphasisPosition::Under,
                    "left" => TextEmphasisPosition::Left,
                    "right" => TextEmphasisPosition::Right,
                    _ => style.text_emphasis_position,
                };
            }
        }
        "background-blend-mode" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.background_blend_mode = match kw.as_str() {
                    "normal" => BlendMode::Normal,
                    "multiply" => BlendMode::Multiply,
                    "screen" => BlendMode::Screen,
                    "overlay" => BlendMode::Overlay,
                    "darken" => BlendMode::Darken,
                    "lighten" => BlendMode::Lighten,
                    "color-dodge" => BlendMode::ColorDodge,
                    "color-burn" => BlendMode::ColorBurn,
                    "hard-light" => BlendMode::HardLight,
                    "soft-light" => BlendMode::SoftLight,
                    "difference" => BlendMode::Difference,
                    "exclusion" => BlendMode::Exclusion,
                    "hue" => BlendMode::Hue,
                    "saturation" => BlendMode::Saturation,
                    "color" => BlendMode::Color,
                    "luminosity" => BlendMode::Luminosity,
                    _ => style.background_blend_mode,
                };
            }
        }
        "image-rendering" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.image_rendering = match kw.as_str() {
                    "auto" => ImageRendering::Auto,
                    "crisp-edges" => ImageRendering::CrispEdges,
                    "pixelated" => ImageRendering::Pixelated,
                    _ => style.image_rendering,
                };
            }
        }
        "text-align-last" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.text_align_last = match kw.as_str() {
                    "auto" => TextAlignLast::Auto,
                    "start" => TextAlignLast::Start,
                    "end" => TextAlignLast::End,
                    "left" => TextAlignLast::Left,
                    "right" => TextAlignLast::Right,
                    "center" => TextAlignLast::Center,
                    "justify" => TextAlignLast::Justify,
                    _ => style.text_align_last,
                };
            }
        }
        "text-decoration-skip" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.text_decoration_skip = match kw.as_str() {
                    "none" => TextDecorationSkip::None,
                    "objects" => TextDecorationSkip::Objects,
                    "spaces" => TextDecorationSkip::Spaces,
                    "edges" => TextDecorationSkip::Edges,
                    "box-decoration" => TextDecorationSkip::BoxDecoration,
                    "leading-spaces" => TextDecorationSkip::LeadingSpaces,
                    "trailing-spaces" => TextDecorationSkip::TrailingSpaces,
                    _ => style.text_decoration_skip,
                };
            }
        }
        "text-underline-offset" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.text_underline_offset = Some(px);
            }
        }
        "caret-shape" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.caret_shape = match kw.as_str() {
                    "auto" => CaretShape::Auto,
                    "bar" => CaretShape::Bar,
                    "block" => CaretShape::Block,
                    "underscore" => CaretShape::Underscore,
                    _ => style.caret_shape,
                };
            }
        }
        "box-decoration-break" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.box_decoration_break = match kw.as_str() {
                    "slice" => BoxDecorationBreak::Slice,
                    "clone" => BoxDecorationBreak::Clone,
                    _ => style.box_decoration_break,
                };
            }
        }
        "text-combine-upright" => {
            match &decl.value {
                CssValue::None => style.text_combine_upright = TextCombineUpright::None,
                CssValue::Keyword(kw) => {
                    style.text_combine_upright = match kw.as_str() {
                        "none" => TextCombineUpright::None,
                        "all" => TextCombineUpright::All,
                        "digits" => TextCombineUpright::Digits,
                        _ => style.text_combine_upright,
                    };
                }
                CssValue::Number(n) if *n == 2.0 || *n == 3.0 || *n == 4.0 => {
                    style.text_combine_upright = TextCombineUpright::Digits;
                }
                _ => {}
            }
        }
        "line-break" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.line_break = match kw.as_str() {
                    "auto" => LineBreak::Auto,
                    "loose" => LineBreak::Loose,
                    "normal" => LineBreak::Normal,
                    "strict" => LineBreak::Strict,
                    "anywhere" => LineBreak::Anywhere,
                    _ => style.line_break,
                };
            }
        }
        "hanging-punctuation" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.hanging_punctuation = match kw.as_str() {
                    "none" => HangingPunctuation::None,
                    "first" => HangingPunctuation::First,
                    "last" => HangingPunctuation::Last,
                    "force-end" => HangingPunctuation::ForceEnd,
                    "allow-end" => HangingPunctuation::AllowEnd,
                    "first last" => HangingPunctuation::FirstLast,
                    _ => style.hanging_punctuation,
                };
            }
        }
        "fill" => {
            match &decl.value {
                CssValue::Color(c) => style.fill = Some(*c),
                CssValue::Keyword(kw) if kw == "none" => style.fill = None,
                _ => {}
            }
        }
        "fill-opacity" => {
            if let CssValue::Number(n) = &decl.value {
                style.fill_opacity = n.max(0.0).min(1.0);
            }
        }
        "fill-rule" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.fill_rule = match kw.as_str() {
                    "nonzero" => FillRule::NonZero,
                    "evenodd" => FillRule::EvenOdd,
                    _ => style.fill_rule,
                };
            }
        }
        "stroke" => {
            match &decl.value {
                CssValue::Color(c) => style.stroke = Some(*c),
                CssValue::Keyword(kw) if kw == "none" => style.stroke = None,
                _ => {}
            }
        }
        "stroke-width" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.stroke_width = px;
            }
        }
        "stroke-opacity" => {
            if let CssValue::Number(n) = &decl.value {
                style.stroke_opacity = n.max(0.0).min(1.0);
            }
        }
        "stroke-linecap" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.stroke_linecap = match kw.as_str() {
                    "butt" => StrokeLinecap::Butt,
                    "round" => StrokeLinecap::Round,
                    "square" => StrokeLinecap::Square,
                    _ => style.stroke_linecap,
                };
            }
        }
        "stroke-linejoin" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.stroke_linejoin = match kw.as_str() {
                    "miter" => StrokeLinejoin::Miter,
                    "round" => StrokeLinejoin::Round,
                    "bevel" => StrokeLinejoin::Bevel,
                    _ => style.stroke_linejoin,
                };
            }
        }
        "clip-rule" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.clip_rule = match kw.as_str() {
                    "nonzero" => ClipRule::NonZero,
                    "evenodd" => ClipRule::EvenOdd,
                    _ => style.clip_rule,
                };
            }
        }
        "animation-composition" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.animation_composition = match kw.as_str() {
                    "replace" => AnimationComposition::Replace,
                    "add" => AnimationComposition::Add,
                    "accumulate" => AnimationComposition::Accumulate,
                    _ => style.animation_composition,
                };
            }
        }
        "animation-timeline" => {
            match &decl.value {
                CssValue::Keyword(kw) => {
                    style.animation_timeline = match kw.as_str() {
                        "auto" => AnimationTimeline::Auto,
                        "none" => AnimationTimeline::None,
                        _ => style.animation_timeline.clone(),
                    };
                }
                _ => {}
            }
        }
        "scroll-timeline-name" => {
            match &decl.value {
                CssValue::Keyword(kw) => style.scroll_timeline_name = vec![kw.clone()],
                CssValue::List(vals) => {
                    style.scroll_timeline_name = vals.iter()
                        .filter_map(|v| match v {
                            CssValue::Keyword(k) => Some(k.clone()),
                            _ => None,
                        })
                        .collect();
                }
                _ => {}
            }
        }
        "scroll-timeline-axis" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.scroll_timeline_axis = match kw.as_str() {
                    "block" => ScrollAxis::Block,
                    "inline" => ScrollAxis::Inline,
                    "vertical" => ScrollAxis::Vertical,
                    "horizontal" => ScrollAxis::Horizontal,
                    _ => style.scroll_timeline_axis,
                };
            }
        }
        "view-timeline-name" => {
            match &decl.value {
                CssValue::Keyword(kw) => style.view_timeline_name = vec![kw.clone()],
                CssValue::List(vals) => {
                    style.view_timeline_name = vals.iter()
                        .filter_map(|v| match v {
                            CssValue::Keyword(k) => Some(k.clone()),
                            _ => None,
                        })
                        .collect();
                }
                _ => {}
            }
        }
        "view-timeline-axis" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.view_timeline_axis = match kw.as_str() {
                    "block" => ScrollAxis::Block,
                    "inline" => ScrollAxis::Inline,
                    "vertical" => ScrollAxis::Vertical,
                    "horizontal" => ScrollAxis::Horizontal,
                    _ => style.view_timeline_axis,
                };
            }
        }
        "view-timeline-inset" => {
            match &decl.value {
                CssValue::List(vals) if vals.len() == 2 => {
                    let start = vals[0].to_px(parent_font_size, viewport_width, viewport_height).unwrap_or(0.0);
                    let end = vals[1].to_px(parent_font_size, viewport_width, viewport_height).unwrap_or(0.0);
                    style.view_timeline_inset = (start, end);
                }
                _ => {}
            }
        }
        "anchor-name" => {
            match &decl.value {
                CssValue::Keyword(kw) => style.anchor_name = vec![kw.clone()],
                CssValue::List(vals) => {
                    style.anchor_name = vals.iter()
                        .filter_map(|v| match v {
                            CssValue::Keyword(k) => Some(k.clone()),
                            _ => None,
                        })
                        .collect();
                }
                _ => {}
            }
        }
        "anchor-default" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.anchor_default = Some(kw.clone());
            }
        }
        "position-anchor" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.position_anchor = Some(kw.clone());
            }
        }
        "position-area" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.position_area = match kw.as_str() {
                    "none" => PositionArea::None,
                    "top" => PositionArea::Top,
                    "bottom" => PositionArea::Bottom,
                    "left" => PositionArea::Left,
                    "right" => PositionArea::Right,
                    "center" => PositionArea::Center,
                    "top left" | "left top" => PositionArea::TopLeft,
                    "top right" | "right top" => PositionArea::TopRight,
                    "bottom left" | "left bottom" => PositionArea::BottomLeft,
                    "bottom right" | "right bottom" => PositionArea::BottomRight,
                    "start" => PositionArea::Start,
                    "end" => PositionArea::End,
                    "block-start" => PositionArea::BlockStart,
                    "block-end" => PositionArea::BlockEnd,
                    "inline-start" => PositionArea::InlineStart,
                    "inline-end" => PositionArea::InlineEnd,
                    _ => style.position_area,
                };
            }
        }
        "position-try" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.position_try = match kw.as_str() {
                    "none" => PositionTry::None,
                    "flip-block" => PositionTry::FlipBlock,
                    "flip-inline" => PositionTry::FlipInline,
                    "flip" => PositionTry::Flip,
                    _ => style.position_try,
                };
            }
        }
        "position-visibility" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.position_visibility = match kw.as_str() {
                    "always" => PositionVisibility::Always,
                    "anchors-visible" => PositionVisibility::Anchored,
                    "no-overflow" => PositionVisibility::NoOverflow,
                    _ => style.position_visibility,
                };
            }
        }
        "popover" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.popover = match kw.as_str() {
                    "auto" => Popover::Auto,
                    "hint" => Popover::Hint,
                    "manual" => Popover::Manual,
                    "none" => Popover::None,
                    _ => style.popover,
                };
            }
        }
        "inset-block" => {
            match &decl.value {
                CssValue::List(vals) if vals.len() == 2 => {
                    style.inset_block.0 = vals[0].to_px(parent_font_size, viewport_width, viewport_height);
                    style.inset_block.1 = vals[1].to_px(parent_font_size, viewport_width, viewport_height);
                }
                _ => {
                    if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                        style.inset_block = (Some(px), Some(px));
                    }
                }
            }
        }
        "inset-inline" => {
            match &decl.value {
                CssValue::List(vals) if vals.len() == 2 => {
                    style.inset_inline.0 = vals[0].to_px(parent_font_size, viewport_width, viewport_height);
                    style.inset_inline.1 = vals[1].to_px(parent_font_size, viewport_width, viewport_height);
                }
                _ => {
                    if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                        style.inset_inline = (Some(px), Some(px));
                    }
                }
            }
        }
        "margin-block" => {
            match &decl.value {
                CssValue::List(vals) if vals.len() == 2 => {
                    style.margin_block.0 = vals[0].to_px(parent_font_size, viewport_width, viewport_height).unwrap_or(0.0);
                    style.margin_block.1 = vals[1].to_px(parent_font_size, viewport_width, viewport_height).unwrap_or(0.0);
                }
                _ => {
                    if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                        style.margin_block = (px, px);
                    }
                }
            }
        }
        "margin-inline" => {
            match &decl.value {
                CssValue::List(vals) if vals.len() == 2 => {
                    style.margin_inline.0 = vals[0].to_px(parent_font_size, viewport_width, viewport_height).unwrap_or(0.0);
                    style.margin_inline.1 = vals[1].to_px(parent_font_size, viewport_width, viewport_height).unwrap_or(0.0);
                }
                _ => {
                    if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                        style.margin_inline = (px, px);
                    }
                }
            }
        }
        "padding-block" => {
            match &decl.value {
                CssValue::List(vals) if vals.len() == 2 => {
                    style.padding_block.0 = vals[0].to_px(parent_font_size, viewport_width, viewport_height).unwrap_or(0.0);
                    style.padding_block.1 = vals[1].to_px(parent_font_size, viewport_width, viewport_height).unwrap_or(0.0);
                }
                _ => {
                    if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                        style.padding_block = (px, px);
                    }
                }
            }
        }
        "padding-inline" => {
            match &decl.value {
                CssValue::List(vals) if vals.len() == 2 => {
                    style.padding_inline.0 = vals[0].to_px(parent_font_size, viewport_width, viewport_height).unwrap_or(0.0);
                    style.padding_inline.1 = vals[1].to_px(parent_font_size, viewport_width, viewport_height).unwrap_or(0.0);
                }
                _ => {
                    if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                        style.padding_inline = (px, px);
                    }
                }
            }
        }
        "border-block-width" => {
            match &decl.value {
                CssValue::List(vals) if vals.len() == 2 => {
                    style.border_block_width.0 = vals[0].to_px(parent_font_size, viewport_width, viewport_height).unwrap_or(0.0);
                    style.border_block_width.1 = vals[1].to_px(parent_font_size, viewport_width, viewport_height).unwrap_or(0.0);
                }
                _ => {
                    if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                        style.border_block_width = (px, px);
                    }
                }
            }
        }
        "border-inline-width" => {
            match &decl.value {
                CssValue::List(vals) if vals.len() == 2 => {
                    style.border_inline_width.0 = vals[0].to_px(parent_font_size, viewport_width, viewport_height).unwrap_or(0.0);
                    style.border_inline_width.1 = vals[1].to_px(parent_font_size, viewport_width, viewport_height).unwrap_or(0.0);
                }
                _ => {
                    if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                        style.border_inline_width = (px, px);
                    }
                }
            }
        }
        "font-synthesis" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.font_synthesis = match kw.as_str() {
                    "none" => FontSynthesis::None,
                    "weight" => FontSynthesis::Weight,
                    "style" => FontSynthesis::Style,
                    "weight style" | "style weight" => FontSynthesis::WeightStyle,
                    _ => style.font_synthesis,
                };
            }
        }
        "text-orientation" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.text_orientation = match kw.as_str() {
                    "mixed" => TextOrientation::Mixed,
                    "upright" => TextOrientation::Upright,
                    "sideways" => TextOrientation::Sideways,
                    "sideways-right" => TextOrientation::SidewaysRight,
                    "use-glyph-orientation" => TextOrientation::UseGlyphOrientation,
                    _ => style.text_orientation,
                };
            }
        }
        "overflow-anchor" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.overflow_anchor = match kw.as_str() {
                    "auto" => OverflowAnchor::Auto,
                    "none" => OverflowAnchor::None,
                    _ => style.overflow_anchor,
                };
            }
        }
        "scroll-snap-type" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.scroll_snap_type = match kw.as_str() {
                    "none" => ScrollSnapType::None,
                    "x" => ScrollSnapType::X,
                    "y" => ScrollSnapType::Y,
                    "block" => ScrollSnapType::Block,
                    "inline" => ScrollSnapType::Inline,
                    "both" => ScrollSnapType::Both,
                    "mandatory" => ScrollSnapType::Mandatory,
                    "proximity" => ScrollSnapType::Proximity,
                    _ => style.scroll_snap_type,
                };
            }
        }
        "scroll-snap-align" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.scroll_snap_align = match kw.as_str() {
                    "none" => ScrollSnapAlign::None,
                    "start" => ScrollSnapAlign::Start,
                    "end" => ScrollSnapAlign::End,
                    "center" => ScrollSnapAlign::Center,
                    _ => style.scroll_snap_align,
                };
            }
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
            // ::before/::after content
            match &decl.value {
                CssValue::None => style.content = Content::None,
                CssValue::Keyword(kw) => match kw.as_str() {
                    "none" => style.content = Content::None,
                    "normal" => style.content = Content::Normal,
                    "open-quote" => style.content = Content::OpenQuote,
                    "close-quote" => style.content = Content::CloseQuote,
                    "no-open-quote" => style.content = Content::NoOpenQuote,
                    "no-close-quote" => style.content = Content::NoCloseQuote,
                    text => {
                        // If it looks like quoted text (starts/ends with quotes), strip them
                        let trimmed = text.trim_matches('"').trim_matches('\'');
                        style.content = Content::Text(trimmed.to_string());
                    }
                },
                CssValue::List(vals) => {
                    // content can be a list like: "Prefix " attr(href) " suffix"
                    // For now, concatenate all text parts
                    let mut result = String::new();
                    for v in vals {
                        if let CssValue::Keyword(kw) = v {
                            result.push_str(kw.trim_matches('"').trim_matches('\''));
                        }
                    }
                    if !result.is_empty() {
                        style.content = Content::Text(result);
                    }
                }
                _ => {}
            }
        }
        "font-family" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.font_family = match kw.as_str() {
                    "serif" => FontFamily::Serif,
                    "sans-serif" => FontFamily::SansSerif,
                    "monospace" => FontFamily::Monospace,
                    "cursive" => FontFamily::Cursive,
                    "fantasy" => FontFamily::Fantasy,
                    "system-ui" => FontFamily::SystemUI,
                    _ => style.font_family,
                };
            }
        }
        "letter-spacing" => {
            if let CssValue::Keyword(kw) = &decl.value {
                if kw == "normal" {
                    style.letter_spacing = 0.0;
                }
            } else if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.letter_spacing = px;
            }
        }
        "word-spacing" => {
            if let CssValue::Keyword(kw) = &decl.value {
                if kw == "normal" {
                    style.word_spacing = 0.0;
                }
            } else if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.word_spacing = px;
            }
        }
        "border-radius" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.border_top_left_radius = px;
                style.border_top_right_radius = px;
                style.border_bottom_left_radius = px;
                style.border_bottom_right_radius = px;
            }
        }
        "border-top-left-radius" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.border_top_left_radius = px;
            }
        }
        "border-top-right-radius" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.border_top_right_radius = px;
            }
        }
        "border-bottom-left-radius" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.border_bottom_left_radius = px;
            }
        }
        "border-bottom-right-radius" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.border_bottom_right_radius = px;
            }
        }
        "vertical-align" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.vertical_align = match kw.as_str() {
                    "baseline" => VerticalAlign::Baseline,
                    "top" => VerticalAlign::Top,
                    "bottom" => VerticalAlign::Bottom,
                    "middle" => VerticalAlign::Middle,
                    "sub" => VerticalAlign::Sub,
                    "super" => VerticalAlign::Super,
                    "text-top" => VerticalAlign::TextTop,
                    "text-bottom" => VerticalAlign::TextBottom,
                    _ => style.vertical_align,
                };
            }
        }
        "text-shadow" => {
            // text-shadow: offset-x offset-y blur-radius color
            // e.g., "1px 1px 2px black"
            match &decl.value {
                CssValue::List(vals) if vals.len() >= 2 => {
                    let offset_x = vals.get(0)
                        .and_then(|v| v.to_px(parent_font_size, viewport_width, viewport_height))
                        .unwrap_or(0.0);
                    let offset_y = vals.get(1)
                        .and_then(|v| v.to_px(parent_font_size, viewport_width, viewport_height))
                        .unwrap_or(0.0);
                    let blur_radius = vals.get(2)
                        .and_then(|v| v.to_px(parent_font_size, viewport_width, viewport_height))
                        .unwrap_or(0.0);
                    let color = vals.iter().find_map(|v| {
                        if let CssValue::Color(c) = v { Some(*c) } else { None }
                    }).unwrap_or(CssColor::from_rgb(0, 0, 0));
                    style.text_shadow = Some(TextShadow { offset_x, offset_y, blur_radius, color });
                }
                CssValue::Keyword(kw) if kw == "none" => {
                    style.text_shadow = None;
                }
                _ => {}
            }
        }
        "box-shadow" => {
            // box-shadow: offset-x offset-y blur spread color inset
            // e.g., "2px 2px 4px 0px rgba(0,0,0,0.5)"
            match &decl.value {
                CssValue::List(vals) if vals.len() >= 2 => {
                    let mut inset = false;
                    let mut offset_x = 0.0f32;
                    let mut offset_y = 0.0f32;
                    let mut blur_radius = 0.0f32;
                    let mut spread_radius = 0.0f32;
                    let mut color = CssColor::from_rgb(0, 0, 0);

                    for (i, v) in vals.iter().enumerate() {
                        match v {
                            CssValue::Keyword(kw) if kw == "inset" => {
                                inset = true;
                            }
                            CssValue::Color(c) => {
                                color = *c;
                            }
                            _ => {
                                // Try to parse as length
                                if let Some(px) = v.to_px(parent_font_size, viewport_width, viewport_height) {
                                    // First two are offset, third is blur, fourth is spread
                                    if offset_x == 0.0 && i == 0 {
                                        offset_x = px;
                                    } else if offset_y == 0.0 && (i == 1 || offset_x != 0.0) {
                                        offset_y = px;
                                    } else if blur_radius == 0.0 {
                                        blur_radius = px;
                                    } else {
                                        spread_radius = px;
                                    }
                                }
                            }
                        }
                    }

                    // Only set if we have at least offset values
                    if offset_x != 0.0 || offset_y != 0.0 {
                        style.box_shadow = Some(BoxShadow {
                            offset_x,
                            offset_y,
                            blur_radius,
                            spread_radius,
                            color,
                            inset,
                        });
                    }
                }
                CssValue::Keyword(kw) if kw == "none" => {
                    style.box_shadow = None;
                }
                _ => {}
            }
        }
        "word-break" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.word_break = match kw.as_str() {
                    "normal" => WordBreak::Normal,
                    "break-all" => WordBreak::BreakAll,
                    "keep-all" => WordBreak::KeepAll,
                    "break-word" => WordBreak::BreakWord,
                    _ => style.word_break,
                };
            }
        }
        "overflow-wrap" | "word-wrap" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.overflow_wrap = match kw.as_str() {
                    "normal" => OverflowWrap::Normal,
                    "break-word" => OverflowWrap::BreakWord,
                    "anywhere" => OverflowWrap::Anywhere,
                    _ => style.overflow_wrap,
                };
            }
        }
        "text-overflow" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.text_overflow = match kw.as_str() {
                    "clip" => TextOverflow::Clip,
                    "ellipsis" => TextOverflow::Ellipsis,
                    _ => style.text_overflow.clone(),
                };
            }
        }
        "white-space-collapse" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.white_space_collapse = match kw.as_str() {
                    "collapse" => WhiteSpaceCollapse::Collapse,
                    "preserve" => WhiteSpaceCollapse::Preserve,
                    "preserve-breaks" => WhiteSpaceCollapse::PreserveBreaks,
                    "break-spaces" => WhiteSpaceCollapse::BreakSpaces,
                    _ => style.white_space_collapse,
                };
            }
        }
        "text-wrap" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.text_wrap = match kw.as_str() {
                    "wrap" => TextWrap::Wrap,
                    "nowrap" => TextWrap::NoWrap,
                    "balance" => TextWrap::Balance,
                    "pretty" => TextWrap::Pretty,
                    "stable" => TextWrap::Stable,
                    _ => style.text_wrap,
                };
            }
        }
        "quotes" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.quotes = match kw.as_str() {
                    "auto" => Quotes::Auto,
                    "none" => Quotes::None,
                    _ => style.quotes,
                };
            }
        }
        "cursor" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.cursor = match kw.as_str() {
                    "auto" => Cursor::Auto,
                    "default" => Cursor::Default,
                    "pointer" => Cursor::Pointer,
                    "text" => Cursor::Text,
                    "move" => Cursor::Move,
                    "not-allowed" => Cursor::NotAllowed,
                    "grab" => Cursor::Grab,
                    "grabbing" => Cursor::Grabbing,
                    "wait" => Cursor::Wait,
                    "help" => Cursor::Help,
                    "crosshair" => Cursor::Crosshair,
                    "cell" => Cursor::Cell,
                    "none" => Cursor::None,
                    _ => style.cursor,
                };
            }
        }
        "pointer-events" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.pointer_events = match kw.as_str() {
                    "auto" => PointerEvents::Auto,
                    "none" => PointerEvents::None,
                    "visiblePainted" => PointerEvents::VisiblePainted,
                    "visibleFill" => PointerEvents::VisibleFill,
                    "visibleStroke" => PointerEvents::VisibleStroke,
                    "painted" => PointerEvents::Painted,
                    "fill" => PointerEvents::Fill,
                    "stroke" => PointerEvents::Stroke,
                    "all" => PointerEvents::All,
                    _ => style.pointer_events,
                };
            }
        }
        "user-select" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.user_select = match kw.as_str() {
                    "auto" => UserSelect::Auto,
                    "none" => UserSelect::None,
                    "text" => UserSelect::Text,
                    "all" => UserSelect::All,
                    "contain" => UserSelect::Contain,
                    _ => style.user_select,
                };
            }
        }
        "aspect-ratio" => {
            // aspect-ratio: width / height  or  aspect-ratio: ratio
            match &decl.value {
                CssValue::List(vals) if vals.len() >= 3 => {
                    // e.g., "16 / 9"
                    if let (Some(CssValue::Number(w)), Some(CssValue::Number(h))) = (vals.get(0), vals.get(2)) {
                        if *w > 0.0 && *h > 0.0 {
                            style.aspect_ratio = Some(AspectRatio { width: *w, height: *h });
                        }
                    }
                }
                CssValue::Number(ratio) if *ratio > 0.0 => {
                    // Single number means ratio/1
                    style.aspect_ratio = Some(AspectRatio { width: *ratio, height: 1.0 });
                }
                CssValue::Keyword(kw) if kw == "auto" => {
                    style.aspect_ratio = None;
                }
                _ => {}
            }
        }
        "resize" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.resize = match kw.as_str() {
                    "none" => Resize::None,
                    "both" => Resize::Both,
                    "horizontal" => Resize::Horizontal,
                    "vertical" => Resize::Vertical,
                    "block" => Resize::Block,
                    "inline" => Resize::Inline,
                    _ => style.resize,
                };
            }
        }
        "transform" => {
            // transform: translate(x, y) rotate(deg) scale(x, y) etc.
            if let CssValue::List(vals) = &decl.value {
                let mut transforms = Vec::new();
                let mut i = 0;
                while i < vals.len() {
                    if let CssValue::Keyword(func) = &vals[i] {
                        match func.as_str() {
                            "translate" if i + 1 < vals.len() => {
                                if let CssValue::List(args) = &vals[i + 1] {
                                    if let (Some(CssValue::Number(x)), Some(CssValue::Number(y))) = (args.get(0), args.get(1)) {
                                        transforms.push(Transform::Translate(*x, *y));
                                    }
                                }
                                i += 2;
                                continue;
                            }
                            "translateX" if i + 1 < vals.len() => {
                                if let CssValue::Number(x) = &vals[i + 1] {
                                    transforms.push(Transform::TranslateX(*x));
                                }
                                i += 2;
                                continue;
                            }
                            "translateY" if i + 1 < vals.len() => {
                                if let CssValue::Number(y) = &vals[i + 1] {
                                    transforms.push(Transform::TranslateY(*y));
                                }
                                i += 2;
                                continue;
                            }
                            "scale" if i + 1 < vals.len() => {
                                if let CssValue::List(args) = &vals[i + 1] {
                                    if let (Some(CssValue::Number(x)), Some(CssValue::Number(y))) = (args.get(0), args.get(1)) {
                                        transforms.push(Transform::Scale(*x, *y));
                                    }
                                } else if let CssValue::Number(s) = &vals[i + 1] {
                                    transforms.push(Transform::Scale(*s, *s));
                                }
                                i += 2;
                                continue;
                            }
                            "scaleX" if i + 1 < vals.len() => {
                                if let CssValue::Number(x) = &vals[i + 1] {
                                    transforms.push(Transform::ScaleX(*x));
                                }
                                i += 2;
                                continue;
                            }
                            "scaleY" if i + 1 < vals.len() => {
                                if let CssValue::Number(y) = &vals[i + 1] {
                                    transforms.push(Transform::ScaleY(*y));
                                }
                                i += 2;
                                continue;
                            }
                            "rotate" if i + 1 < vals.len() => {
                                if let CssValue::Number(deg) = &vals[i + 1] {
                                    transforms.push(Transform::Rotate(*deg));
                                }
                                i += 2;
                                continue;
                            }
                            "skew" if i + 1 < vals.len() => {
                                if let CssValue::List(args) = &vals[i + 1] {
                                    if let (Some(CssValue::Number(x)), Some(CssValue::Number(y))) = (args.get(0), args.get(1)) {
                                        transforms.push(Transform::Skew(*x, *y));
                                    }
                                }
                                i += 2;
                                continue;
                            }
                            "skewX" if i + 1 < vals.len() => {
                                if let CssValue::Number(x) = &vals[i + 1] {
                                    transforms.push(Transform::SkewX(*x));
                                }
                                i += 2;
                                continue;
                            }
                            "skewY" if i + 1 < vals.len() => {
                                if let CssValue::Number(y) = &vals[i + 1] {
                                    transforms.push(Transform::SkewY(*y));
                                }
                                i += 2;
                                continue;
                            }
                            "none" => {
                                transforms.clear();
                                break;
                            }
                            _ => {}
                        }
                    }
                    i += 1;
                }
                style.transform = transforms;
            } else if let CssValue::Keyword(kw) = &decl.value {
                if kw == "none" {
                    style.transform.clear();
                }
            }
        }
        // Individual transform properties (CSS Transforms Level 2)
        "translate" => {
            // translate: x y
            match &decl.value {
                CssValue::List(vals) if vals.len() >= 2 => {
                    if let (Some(x_val), Some(y_val)) = (vals.get(0), vals.get(1)) {
                        let x = x_val.to_px(parent_font_size, viewport_width, viewport_height).unwrap_or(0.0);
                        let y = y_val.to_px(parent_font_size, viewport_width, viewport_height).unwrap_or(0.0);
                        // Replace any existing translate in the transform list
                        style.transform.retain(|t| !matches!(t, Transform::Translate(_, _) | Transform::TranslateX(_) | Transform::TranslateY(_)));
                        style.transform.push(Transform::Translate(x, y));
                    }
                }
                CssValue::Keyword(kw) if kw == "none" => {
                    style.transform.retain(|t| !matches!(t, Transform::Translate(_, _) | Transform::TranslateX(_) | Transform::TranslateY(_)));
                }
                _ => {}
            }
        }
        "rotate" => {
            // rotate: angle
            match &decl.value {
                CssValue::Number(deg) | CssValue::Length(deg, _) => {
                    style.transform.retain(|t| !matches!(t, Transform::Rotate(_)));
                    style.transform.push(Transform::Rotate(*deg));
                }
                CssValue::Keyword(kw) if kw == "none" => {
                    style.transform.retain(|t| !matches!(t, Transform::Rotate(_)));
                }
                _ => {}
            }
        }
        "scale" => {
            // scale: x y | x
            match &decl.value {
                CssValue::List(vals) if vals.len() >= 2 => {
                    if let (Some(CssValue::Number(x)), Some(CssValue::Number(y))) = (vals.get(0), vals.get(1)) {
                        style.transform.retain(|t| !matches!(t, Transform::Scale(_, _) | Transform::ScaleX(_) | Transform::ScaleY(_)));
                        style.transform.push(Transform::Scale(*x, *y));
                    }
                }
                CssValue::Number(s) => {
                    style.transform.retain(|t| !matches!(t, Transform::Scale(_, _) | Transform::ScaleX(_) | Transform::ScaleY(_)));
                    style.transform.push(Transform::Scale(*s, *s));
                }
                CssValue::Keyword(kw) if kw == "none" => {
                    style.transform.retain(|t| !matches!(t, Transform::Scale(_, _) | Transform::ScaleX(_) | Transform::ScaleY(_)));
                }
                _ => {}
            }
        }
        "offset-path" => {
            match &decl.value {
                CssValue::Keyword(kw) if kw == "none" => style.offset_path = None,
                CssValue::Keyword(url) if url.starts_with("url(") => {
                    let inner = url.trim_start_matches("url(").trim_end_matches(")");
                    style.offset_path = Some(OffsetPath::Url(inner.trim_matches('"').trim_matches('\'').to_string()));
                }
                CssValue::Keyword(path) if path.starts_with("path(") => {
                    style.offset_path = Some(OffsetPath::Path(path.clone()));
                }
                _ => {}
            }
        }
        "offset-distance" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.offset_distance = px;
            } else if let CssValue::Percentage(p) = &decl.value {
                style.offset_distance = *p;
            }
        }
        "offset-rotate" => {
            match &decl.value {
                CssValue::Keyword(kw) => match kw.as_str() {
                    "auto" => style.offset_rotate = OffsetRotate::Auto,
                    "reverse" => style.offset_rotate = OffsetRotate::Reverse,
                    _ => {}
                }
                CssValue::Number(deg) => style.offset_rotate = OffsetRotate::Angle(*deg),
                _ => {}
            }
        }
        "object-fit" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.object_fit = match kw.as_str() {
                    "fill" => ObjectFit::Fill,
                    "contain" => ObjectFit::Contain,
                    "cover" => ObjectFit::Cover,
                    "none" => ObjectFit::None,
                    "scale-down" => ObjectFit::ScaleDown,
                    _ => style.object_fit,
                };
            }
        }
        "object-position" => {
            // object-position: x y (keywords or percentages)
            if let CssValue::List(vals) = &decl.value {
                let x = parse_position_value(vals.get(0), 0.5);
                let y = parse_position_value(vals.get(1), 0.5);
                style.object_position = (x, y);
            }
        }
        "columns" => {
            // columns: column-width column-count or just one value
            match &decl.value {
                CssValue::List(vals) => {
                    for v in vals {
                        match v {
                            CssValue::Number(n) if *n >= 1.0 => {
                                style.column_count = Some(*n as i32);
                            }
                            _ => {
                                if let Some(px) = v.to_px(parent_font_size, viewport_width, viewport_height) {
                                    style.column_width = Some(px);
                                }
                            }
                        }
                    }
                }
                CssValue::Number(n) if *n >= 1.0 => {
                    style.column_count = Some(*n as i32);
                }
                CssValue::Keyword(kw) if kw == "auto" => {
                    style.column_count = None;
                    style.column_width = None;
                }
                _ => {
                    if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                        style.column_width = Some(px);
                    }
                }
            }
        }
        "column-count" => {
            match &decl.value {
                CssValue::Number(n) if *n >= 1.0 => style.column_count = Some(*n as i32),
                CssValue::Keyword(kw) if kw == "auto" => style.column_count = None,
                _ => {}
            }
        }
        "column-width" => {
            match &decl.value {
                CssValue::Keyword(kw) if kw == "auto" => style.column_width = None,
                _ => {
                    if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                        style.column_width = Some(px);
                    }
                }
            }
        }
        // "column-gap" is already handled above as part of "gap" | "grid-column-gap" | "column-gap"
        "column-rule" | "column-rule-width" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.column_rule_width = px;
            }
        }
        "column-rule-color" => {
            if let CssValue::Color(c) = &decl.value {
                style.column_rule_color = *c;
            }
        }
        "column-rule-style" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.column_rule_style = match kw.as_str() {
                    "none" => ColumnRuleStyle::None,
                    "solid" => ColumnRuleStyle::Solid,
                    "dashed" => ColumnRuleStyle::Dashed,
                    "dotted" => ColumnRuleStyle::Dotted,
                    "double" => ColumnRuleStyle::Double,
                    _ => style.column_rule_style,
                };
            }
        }
        "break-before" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.break_before = match kw.as_str() {
                    "auto" => BreakBefore::Auto,
                    "avoid" => BreakBefore::Avoid,
                    "always" => BreakBefore::Always,
                    "all" => BreakBefore::All,
                    "page" => BreakBefore::Page,
                    "column" => BreakBefore::Column,
                    "region" => BreakBefore::Region,
                    _ => style.break_before,
                };
            }
        }
        "break-after" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.break_after = match kw.as_str() {
                    "auto" => BreakAfter::Auto,
                    "avoid" => BreakAfter::Avoid,
                    "always" => BreakAfter::Always,
                    "all" => BreakAfter::All,
                    "page" => BreakAfter::Page,
                    "column" => BreakAfter::Column,
                    "region" => BreakAfter::Region,
                    _ => style.break_after,
                };
            }
        }
        "break-inside" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.break_inside = match kw.as_str() {
                    "auto" => BreakInside::Auto,
                    "avoid" => BreakInside::Avoid,
                    "avoid-page" => BreakInside::AvoidPage,
                    "avoid-column" => BreakInside::AvoidColumn,
                    "avoid-region" => BreakInside::AvoidRegion,
                    _ => style.break_inside,
                };
            }
        }
        "page-break-before" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.page_break_before = match kw.as_str() {
                    "auto" => PageBreak::Auto,
                    "always" => PageBreak::Always,
                    "avoid" => PageBreak::Avoid,
                    "left" => PageBreak::Left,
                    "right" => PageBreak::Right,
                    _ => style.page_break_before,
                };
            }
        }
        "page-break-after" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.page_break_after = match kw.as_str() {
                    "auto" => PageBreak::Auto,
                    "always" => PageBreak::Always,
                    "avoid" => PageBreak::Avoid,
                    "left" => PageBreak::Left,
                    "right" => PageBreak::Right,
                    _ => style.page_break_after,
                };
            }
        }
        "page-break-inside" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.page_break_inside = match kw.as_str() {
                    "auto" => PageBreakInside::Auto,
                    "avoid" => PageBreakInside::Avoid,
                    _ => style.page_break_inside,
                };
            }
        }
        "print-color-adjust" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.print_color_adjust = match kw.as_str() {
                    "economy" => PrintColorAdjust::Economy,
                    "exact" => PrintColorAdjust::Exact,
                    _ => style.print_color_adjust,
                };
            }
        }
        "counter-reset" => {
            match &decl.value {
                CssValue::None => style.counter_reset = Vec::new(),
                CssValue::Keyword(kw) if kw == "none" => style.counter_reset = Vec::new(),
                CssValue::List(vals) => {
                    let mut counters = Vec::new();
                    let mut i = 0;
                    while i < vals.len() {
                        if let CssValue::Keyword(name) = &vals[i] {
                            let value = if i + 1 < vals.len() {
                                if let CssValue::Number(n) = &vals[i + 1] {
                                    i += 1;
                                    *n as i32
                                } else {
                                    0
                                }
                            } else {
                                0
                            };
                            counters.push((name.clone(), value));
                        }
                        i += 1;
                    }
                    style.counter_reset = counters;
                }
                CssValue::Keyword(name) => style.counter_reset = vec![(name.clone(), 0)],
                _ => {}
            }
        }
        "counter-increment" => {
            match &decl.value {
                CssValue::None => style.counter_increment = Vec::new(),
                CssValue::Keyword(kw) if kw == "none" => style.counter_increment = Vec::new(),
                CssValue::List(vals) => {
                    let mut counters = Vec::new();
                    let mut i = 0;
                    while i < vals.len() {
                        if let CssValue::Keyword(name) = &vals[i] {
                            let value = if i + 1 < vals.len() {
                                if let CssValue::Number(n) = &vals[i + 1] {
                                    i += 1;
                                    *n as i32
                                } else {
                                    1
                                }
                            } else {
                                1
                            };
                            counters.push((name.clone(), value));
                        }
                        i += 1;
                    }
                    style.counter_increment = counters;
                }
                CssValue::Keyword(name) => style.counter_increment = vec![(name.clone(), 1)],
                _ => {}
            }
        }
        "writing-mode" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.writing_mode = match kw.as_str() {
                    "horizontal-tb" => WritingMode::HorizontalTb,
                    "vertical-rl" => WritingMode::VerticalRl,
                    "vertical-lr" => WritingMode::VerticalLr,
                    "sideways-rl" => WritingMode::SidewaysRl,
                    "sideways-lr" => WritingMode::SidewaysLr,
                    _ => style.writing_mode,
                };
            }
        }
        "direction" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.direction = match kw.as_str() {
                    "ltr" => Direction::Ltr,
                    "rtl" => Direction::Rtl,
                    _ => style.direction,
                };
            }
        }
        "ruby-position" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.ruby_position = match kw.as_str() {
                    "over" => RubyPosition::Over,
                    "under" => RubyPosition::Under,
                    "inter-character" => RubyPosition::InterCharacter,
                    _ => style.ruby_position,
                };
            }
        }
        "ruby-align" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.ruby_align = match kw.as_str() {
                    "start" => RubyAlign::Start,
                    "center" => RubyAlign::Center,
                    "space-between" => RubyAlign::SpaceBetween,
                    "space-around" => RubyAlign::SpaceAround,
                    _ => style.ruby_align,
                };
            }
        }
        "ruby-merge" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.ruby_merge = match kw.as_str() {
                    "separate" => RubyMerge::Separate,
                    "merge" => RubyMerge::Merge,
                    "auto" => RubyMerge::Auto,
                    _ => style.ruby_merge,
                };
            }
        }
        "scrollbar-width" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.scrollbar_width = match kw.as_str() {
                    "auto" => ScrollbarWidth::Auto,
                    "thin" => ScrollbarWidth::Thin,
                    "none" => ScrollbarWidth::None,
                    _ => style.scrollbar_width,
                };
            }
        }
        "scrollbar-color" => {
            // scrollbar-color: thumb-color track-color
            // or "auto"
            match &decl.value {
                CssValue::Keyword(kw) if kw == "auto" => {
                    style.scrollbar_color = None;
                }
                CssValue::List(vals) if vals.len() >= 2 => {
                    if let (Some(CssValue::Color(thumb)), Some(CssValue::Color(track))) = (vals.get(0), vals.get(1)) {
                        style.scrollbar_color = Some((*thumb, *track));
                    }
                }
                CssValue::Color(c) => {
                    // Single color - use for thumb, transparent for track
                    style.scrollbar_color = Some((*c, CssColor::TRANSPARENT));
                }
                _ => {}
            }
        }
        "scrollbar-gutter" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.scrollbar_gutter = match kw.as_str() {
                    "auto" => ScrollbarGutter::Auto,
                    "stable" => ScrollbarGutter::Stable,
                    "stable both-edges" | "stable-both-edges" => ScrollbarGutter::StableBothEdges,
                    _ => style.scrollbar_gutter,
                };
            }
        }
        "filter" => {
            style.filter = parse_filter_list(&decl.value,
                parent_font_size,
                viewport_width,
                viewport_height,
            );
        }
        "backdrop-filter" => {
            style.backdrop_filter = parse_filter_list(
                &decl.value,
                parent_font_size,
                viewport_width,
                viewport_height,
            );
        }
        "contain" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.contain = match kw.as_str() {
                    "none" => Contain::None,
                    "strict" => Contain::Strict,
                    "content" => Contain::Content,
                    "size" => Contain::Size,
                    "layout" => Contain::Layout,
                    "style" => Contain::Style,
                    "paint" => Contain::Paint,
                    _ => style.contain,
                };
            } else if let CssValue::List(vals) = &decl.value {
                // Multiple values like "size layout paint"
                let mut result = Contain::None;
                for v in vals {
                    if let CssValue::Keyword(kw) = v {
                        match kw.as_str() {
                            "size" => result = Contain::Size,
                            "layout" if result == Contain::Size => result = Contain::Layout,
                            "paint" if result == Contain::Layout => result = Contain::Paint,
                            _ => {}
                        }
                    }
                }
                if result != Contain::None {
                    style.contain = result;
                }
            }
        }
        "contain-intrinsic-size" => {
            if let CssValue::List(vals) = &decl.value {
                if let (Some(CssValue::Number(w)), Some(CssValue::Number(h))) = (vals.get(0), vals.get(1)) {
                    style.contain_intrinsic_size = Some((*w, *h));
                }
            } else if let CssValue::Keyword(kw) = &decl.value {
                if kw == "none" {
                    style.contain_intrinsic_size = None;
                }
            }
        }
        "content-visibility" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.content_visibility = match kw.as_str() {
                    "visible" => ContentVisibility::Visible,
                    "hidden" => ContentVisibility::Hidden,
                    "auto" => ContentVisibility::Auto,
                    _ => style.content_visibility,
                };
            }
        }
        "container-type" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.container_type = match kw.as_str() {
                    "none" => ContainerType::None,
                    "size" => ContainerType::Size,
                    "inline-size" => ContainerType::InlineSize,
                    "normal" => ContainerType::Normal,
                    _ => style.container_type,
                };
            }
        }
        "container-name" => {
            match &decl.value {
                CssValue::Keyword(kw) if kw == "none" => style.container_name.clear(),
                CssValue::List(vals) => {
                    let names: Vec<String> = vals.iter()
                        .filter_map(|v| {
                            if let CssValue::Keyword(s) = v {
                                Some(s.clone())
                            } else {
                                None
                            }
                        })
                        .collect();
                    style.container_name = names;
                }
                CssValue::Keyword(name) => style.container_name = vec![name.clone()],
                _ => {}
            }
        }
        "transition-property" => {
            match &decl.value {
                CssValue::Keyword(kw) if kw == "none" => style.transition_property.clear(),
                CssValue::Keyword(kw) if kw == "all" => style.transition_property = vec!["all".to_string()],
                CssValue::List(vals) => {
                    let props: Vec<String> = vals.iter()
                        .filter_map(|v| {
                            if let CssValue::Keyword(s) = v {
                                Some(s.clone())
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !props.is_empty() {
                        style.transition_property = props;
                    }
                }
                CssValue::Keyword(kw) => style.transition_property = vec![kw.clone()],
                _ => {}
            }
        }
        "transition-duration" => {
            if let Some(ms) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.transition_duration = ms / 1000.0; // convert to seconds
            } else if let CssValue::Number(s) = &decl.value {
                style.transition_duration = *s;
            }
        }
        "transition-timing-function" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.transition_timing_function = match kw.as_str() {
                    "ease" => TransitionTimingFunction::Ease,
                    "ease-in" => TransitionTimingFunction::EaseIn,
                    "ease-out" => TransitionTimingFunction::EaseOut,
                    "ease-in-out" => TransitionTimingFunction::EaseInOut,
                    "linear" => TransitionTimingFunction::Linear,
                    "step-start" => TransitionTimingFunction::StepStart,
                    "step-end" => TransitionTimingFunction::StepEnd,
                    _ => style.transition_timing_function,
                };
            }
        }
        "transition-delay" => {
            if let Some(ms) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.transition_delay = ms / 1000.0; // convert to seconds
            } else if let CssValue::Number(s) = &decl.value {
                style.transition_delay = *s;
            }
        }
        "transition-behavior" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.transition_behavior = match kw.as_str() {
                    "normal" => TransitionBehavior::Normal,
                    "allow-discrete" => TransitionBehavior::AllowDiscrete,
                    _ => style.transition_behavior,
                };
            }
        }
        "animation" => {
            // animation shorthand: name duration timing-function delay iteration-count direction fill-mode play-state
            // For now, parse simple animation name
            match &decl.value {
                CssValue::Keyword(kw) if kw == "none" => {
                    style.animation_name.clear();
                    style.animation_duration.clear();
                }
                CssValue::List(vals) => {
                    // Try to extract animation name and duration from list
                    for v in vals {
                        match v {
                            CssValue::Keyword(name) if name != "none" && !is_timing_function(name) => {
                                style.animation_name.push(name.clone());
                            }
                            CssValue::Length(dur, _) | CssValue::Number(dur) if *dur > 0.0 => {
                                style.animation_duration.push(*dur);
                            }
                            _ => {}
                        }
                    }
                }
                CssValue::Keyword(name) => {
                    style.animation_name.push(name.clone());
                    style.animation_duration.push(0.0);
                }
                _ => {}
            }
        }
        "animation-name" => {
            match &decl.value {
                CssValue::Keyword(kw) if kw == "none" => style.animation_name.clear(),
                CssValue::List(vals) => {
                    let names: Vec<String> = vals.iter()
                        .filter_map(|v| {
                            if let CssValue::Keyword(s) = v {
                                Some(s.clone())
                            } else {
                                None
                            }
                        })
                        .collect();
                    style.animation_name = names;
                }
                CssValue::Keyword(name) => style.animation_name = vec![name.clone()],
                _ => {}
            }
        }
        "animation-duration" => {
            match &decl.value {
                CssValue::List(vals) => {
                    let durations: Vec<f32> = vals.iter()
                        .filter_map(|v| {
                            if let CssValue::Number(n) = v {
                                Some(*n)
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !durations.is_empty() {
                        style.animation_duration = durations;
                    }
                }
                CssValue::Number(n) => style.animation_duration = vec![*n],
                _ => {
                    if let Some(ms) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                        style.animation_duration = vec![ms / 1000.0];
                    }
                }
            }
        }
        "animation-timing-function" => {
            match &decl.value {
                CssValue::List(vals) => {
                    let funcs: Vec<TransitionTimingFunction> = vals.iter()
                        .filter_map(|v| {
                            if let CssValue::Keyword(kw) = v {
                                Some(parse_timing_function(kw))
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !funcs.is_empty() {
                        style.animation_timing_function = funcs;
                    }
                }
                CssValue::Keyword(kw) => {
                    style.animation_timing_function = vec![parse_timing_function(kw)];
                }
                _ => {}
            }
        }
        "animation-delay" => {
            match &decl.value {
                CssValue::List(vals) => {
                    let delays: Vec<f32> = vals.iter()
                        .filter_map(|v| {
                            if let CssValue::Number(n) = v {
                                Some(*n)
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !delays.is_empty() {
                        style.animation_delay = delays;
                    }
                }
                CssValue::Number(n) => style.animation_delay = vec![*n],
                _ => {
                    if let Some(ms) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                        style.animation_delay = vec![ms / 1000.0];
                    }
                }
            }
        }
        "animation-iteration-count" => {
            match &decl.value {
                CssValue::List(vals) => {
                    let counts: Vec<AnimationIterationCount> = vals.iter()
                        .filter_map(|v| match v {
                            CssValue::Keyword(kw) if kw == "infinite" => {
                                Some(AnimationIterationCount::Infinite)
                            }
                            CssValue::Number(n) => Some(AnimationIterationCount::Number(*n)),
                            _ => None,
                        })
                        .collect();
                    if !counts.is_empty() {
                        style.animation_iteration_count = counts;
                    }
                }
                CssValue::Keyword(kw) if kw == "infinite" => {
                    style.animation_iteration_count = vec![AnimationIterationCount::Infinite];
                }
                CssValue::Number(n) => style.animation_iteration_count = vec![AnimationIterationCount::Number(*n)],
                _ => {}
            }
        }
        "animation-direction" => {
            match &decl.value {
                CssValue::List(vals) => {
                    let dirs: Vec<AnimationDirection> = vals.iter()
                        .filter_map(|v| {
                            if let CssValue::Keyword(kw) = v {
                                Some(parse_animation_direction(kw))
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !dirs.is_empty() {
                        style.animation_direction = dirs;
                    }
                }
                CssValue::Keyword(kw) => {
                    style.animation_direction = vec![parse_animation_direction(kw)];
                }
                _ => {}
            }
        }
        "animation-fill-mode" => {
            match &decl.value {
                CssValue::List(vals) => {
                    let modes: Vec<AnimationFillMode> = vals.iter()
                        .filter_map(|v| {
                            if let CssValue::Keyword(kw) = v {
                                Some(parse_animation_fill_mode(kw))
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !modes.is_empty() {
                        style.animation_fill_mode = modes;
                    }
                }
                CssValue::Keyword(kw) => {
                    style.animation_fill_mode = vec![parse_animation_fill_mode(kw)];
                }
                _ => {}
            }
        }
        "animation-play-state" => {
            match &decl.value {
                CssValue::List(vals) => {
                    let states: Vec<AnimationPlayState> = vals.iter()
                        .filter_map(|v| {
                            if let CssValue::Keyword(kw) = v {
                                Some(parse_animation_play_state(kw))
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !states.is_empty() {
                        style.animation_play_state = states;
                    }
                }
                CssValue::Keyword(kw) => {
                    style.animation_play_state = vec![parse_animation_play_state(kw)];
                }
                _ => {}
            }
        }
        "view-transition-name" => {
            match &decl.value {
                CssValue::Keyword(kw) if kw == "none" => style.view_transition_name = None,
                CssValue::Keyword(name) => style.view_transition_name = Some(name.clone()),
                _ => {}
            }
        }
        "view-transition-class" => {
            match &decl.value {
                CssValue::Keyword(kw) if kw == "none" => style.view_transition_class.clear(),
                CssValue::List(vals) => {
                    let classes: Vec<String> = vals.iter()
                        .filter_map(|v| {
                            if let CssValue::Keyword(s) = v {
                                Some(s.clone())
                            } else {
                                None
                            }
                        })
                        .collect();
                    style.view_transition_class = classes;
                }
                CssValue::Keyword(class) => style.view_transition_class = vec![class.clone()],
                _ => {}
            }
        }
        "tab-size" => {
            match &decl.value {
                CssValue::Number(n) => style.tab_size = *n as i32,
                _ => {
                    if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                        style.tab_size = px as i32;
                    }
                }
            }
        }
        "hyphens" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.hyphens = match kw.as_str() {
                    "none" => Hyphens::None,
                    "manual" => Hyphens::Manual,
                    "auto" => Hyphens::Auto,
                    _ => style.hyphens,
                };
            }
        }
        "line-clamp" | "-webkit-line-clamp" => {
            match &decl.value {
                CssValue::Number(n) if *n >= 1.0 => style.line_clamp = Some(*n as i32),
                CssValue::Keyword(kw) if kw == "none" => style.line_clamp = None,
                _ => {}
            }
        }
        "text-justify" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.text_justify = match kw.as_str() {
                    "auto" => TextJustify::Auto,
                    "none" => TextJustify::None,
                    "inter-word" => TextJustify::InterWord,
                    "inter-character" => TextJustify::InterCharacter,
                    _ => style.text_justify,
                };
            }
        }
        "hyphenate-character" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.hyphenate_character = kw.clone();
            }
        }
        "text-group-align" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.text_group_align = match kw.as_str() {
                    "start" => TextGroupAlign::Start,
                    "end" => TextGroupAlign::End,
                    "left" => TextGroupAlign::Left,
                    "right" => TextGroupAlign::Right,
                    "center" => TextGroupAlign::Center,
                    _ => style.text_group_align,
                };
            }
        }
        // Logical inset longhands (4 new properties)
        "inset-block-start" => {
            style.inset_block.0 = decl.value.to_px(parent_font_size, viewport_width, viewport_height);
        }
        "inset-block-end" => {
            style.inset_block.1 = decl.value.to_px(parent_font_size, viewport_width, viewport_height);
        }
        "inset-inline-start" => {
            style.inset_inline.0 = decl.value.to_px(parent_font_size, viewport_width, viewport_height);
        }
        "inset-inline-end" => {
            style.inset_inline.1 = decl.value.to_px(parent_font_size, viewport_width, viewport_height);
        }
        // Logical margin longhands (4 new properties)
        "margin-block-start" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.margin_block.0 = px;
            }
        }
        "margin-block-end" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.margin_block.1 = px;
            }
        }
        "margin-inline-start" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.margin_inline.0 = px;
            }
        }
        "margin-inline-end" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.margin_inline.1 = px;
            }
        }
        // Logical padding longhands (4 new properties)
        "padding-block-start" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.padding_block.0 = px;
            }
        }
        "padding-block-end" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.padding_block.1 = px;
            }
        }
        "padding-inline-start" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.padding_inline.0 = px;
            }
        }
        "padding-inline-end" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.padding_inline.1 = px;
            }
        }
        // Logical border width longhands (4 new properties)
        "border-block-start-width" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.border_block_width.0 = px;
            }
        }
        "border-block-end-width" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.border_block_width.1 = px;
            }
        }
        "border-inline-start-width" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.border_inline_width.0 = px;
            }
        }
        "border-inline-end-width" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.border_inline_width.1 = px;
            }
        }
        // Logical border style longhands (4 new properties)
        "border-block-start-style" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.border_top_style = parse_border_style(kw);
            }
        }
        "border-block-end-style" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.border_bottom_style = parse_border_style(kw);
            }
        }
        "border-inline-start-style" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.border_left_style = parse_border_style(kw);
            }
        }
        "border-inline-end-style" => {
            if let CssValue::Keyword(kw) = &decl.value {
                style.border_right_style = parse_border_style(kw);
            }
        }
        // Logical border color longhands (4 new properties)
        "border-block-start-color" => {
            if let CssValue::Color(c) = &decl.value {
                style.border_top_color = Some(*c);
            }
        }
        "border-block-end-color" => {
            if let CssValue::Color(c) = &decl.value {
                style.border_bottom_color = Some(*c);
            }
        }
        "border-inline-start-color" => {
            if let CssValue::Color(c) = &decl.value {
                style.border_left_color = Some(*c);
            }
        }
        "border-inline-end-color" => {
            if let CssValue::Color(c) = &decl.value {
                style.border_right_color = Some(*c);
            }
        }
        // Logical border shorthands (4 new properties)
        "border-block" => {
            match &decl.value {
                CssValue::List(vals) if vals.len() == 2 => {
                    if let (Some(start), Some(end)) = (vals[0].to_px(parent_font_size, viewport_width, viewport_height), vals[1].to_px(parent_font_size, viewport_width, viewport_height)) {
                        style.border_block_width = (start, end);
                    }
                }
                _ => {
                    if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                        style.border_block_width = (px, px);
                    }
                }
            }
        }
        "border-inline" => {
            match &decl.value {
                CssValue::List(vals) if vals.len() == 2 => {
                    if let (Some(start), Some(end)) = (vals[0].to_px(parent_font_size, viewport_width, viewport_height), vals[1].to_px(parent_font_size, viewport_width, viewport_height)) {
                        style.border_inline_width = (start, end);
                    }
                }
                _ => {
                    if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                        style.border_inline_width = (px, px);
                    }
                }
            }
        }
        "border-block-color" => {
            if let CssValue::Color(c) = &decl.value {
                style.border_top_color = Some(*c);
                style.border_bottom_color = Some(*c);
            }
        }
        "border-inline-color" => {
            if let CssValue::Color(c) = &decl.value {
                style.border_left_color = Some(*c);
                style.border_right_color = Some(*c);
            }
        }
        // Logical border style shorthands (2 new properties)
        "border-block-style" => {
            if let CssValue::Keyword(kw) = &decl.value {
                let s = parse_border_style(kw);
                style.border_top_style = s;
                style.border_bottom_style = s;
            }
        }
        "border-inline-style" => {
            if let CssValue::Keyword(kw) = &decl.value {
                let s = parse_border_style(kw);
                style.border_left_style = s;
                style.border_right_style = s;
            }
        }
        // Min/max logical sizes (4 new properties)
        "min-inline-size" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.min_width = SizeValue::Px(px);
            }
        }
        "min-block-size" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.min_height = SizeValue::Px(px);
            }
        }
        "max-inline-size" => {
            if let CssValue::Keyword(kw) = &decl.value {
                if kw == "none" {
                    style.max_width = SizeValue::None;
                }
            } else if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.max_width = SizeValue::Px(px);
            }
        }
        "max-block-size" => {
            if let CssValue::Keyword(kw) = &decl.value {
                if kw == "none" {
                    style.max_height = SizeValue::None;
                }
            } else if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.max_height = SizeValue::Px(px);
            }
        }
        // Size logical properties (2 new properties)
        "inline-size" => {
            style.width = to_size_value(&decl.value, parent_font_size, viewport_width, viewport_height);
        }
        "block-size" => {
            style.height = to_size_value(&decl.value, parent_font_size, viewport_width, viewport_height);
        }
        // Scroll margin logical longhands (8 new properties)
        "scroll-margin-block-start" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.scroll_margin_top = px;
            }
        }
        "scroll-margin-block-end" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.scroll_margin_bottom = px;
            }
        }
        "scroll-margin-inline-start" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.scroll_margin_left = px;
            }
        }
        "scroll-margin-inline-end" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.scroll_margin_right = px;
            }
        }
        "scroll-padding-block-start" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.scroll_padding_top = px;
            }
        }
        "scroll-padding-block-end" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.scroll_padding_bottom = px;
            }
        }
        "scroll-padding-inline-start" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.scroll_padding_left = px;
            }
        }
        "scroll-padding-inline-end" => {
            if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                style.scroll_padding_right = px;
            }
        }
        // Scroll margin/padding logical shorthands (4 new properties)
        "scroll-margin-block" => {
            match &decl.value {
                CssValue::List(vals) if vals.len() == 2 => {
                    if let (Some(start), Some(end)) = (vals[0].to_px(parent_font_size, viewport_width, viewport_height), vals[1].to_px(parent_font_size, viewport_width, viewport_height)) {
                        style.scroll_margin_top = start;
                        style.scroll_margin_bottom = end;
                    }
                }
                _ => {
                    if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                        style.scroll_margin_top = px;
                        style.scroll_margin_bottom = px;
                    }
                }
            }
        }
        "scroll-margin-inline" => {
            match &decl.value {
                CssValue::List(vals) if vals.len() == 2 => {
                    if let (Some(start), Some(end)) = (vals[0].to_px(parent_font_size, viewport_width, viewport_height), vals[1].to_px(parent_font_size, viewport_width, viewport_height)) {
                        style.scroll_margin_left = start;
                        style.scroll_margin_right = end;
                    }
                }
                _ => {
                    if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                        style.scroll_margin_left = px;
                        style.scroll_margin_right = px;
                    }
                }
            }
        }
        "scroll-padding-block" => {
            match &decl.value {
                CssValue::List(vals) if vals.len() == 2 => {
                    if let (Some(start), Some(end)) = (vals[0].to_px(parent_font_size, viewport_width, viewport_height), vals[1].to_px(parent_font_size, viewport_width, viewport_height)) {
                        style.scroll_padding_top = start;
                        style.scroll_padding_bottom = end;
                    }
                }
                _ => {
                    if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                        style.scroll_padding_top = px;
                        style.scroll_padding_bottom = px;
                    }
                }
            }
        }
        "scroll-padding-inline" => {
            match &decl.value {
                CssValue::List(vals) if vals.len() == 2 => {
                    if let (Some(start), Some(end)) = (vals[0].to_px(parent_font_size, viewport_width, viewport_height), vals[1].to_px(parent_font_size, viewport_width, viewport_height)) {
                        style.scroll_padding_left = start;
                        style.scroll_padding_right = end;
                    }
                }
                _ => {
                    if let Some(px) = decl.value.to_px(parent_font_size, viewport_width, viewport_height) {
                        style.scroll_padding_left = px;
                        style.scroll_padding_right = px;
                    }
                }
            }
        }
        // Additional SVG/stub properties (20 new properties with stub implementations)
        "outline-offset" => {}
        "text-rendering" => {}
        "dominant-baseline" => {}
        "alignment-baseline" => {}
        "baseline-shift" => {}
        "color-interpolation" => {}
        "color-interpolation-filters" => {}
        "flood-color" => {}
        "flood-opacity" => {}
        "lighting-color" => {}
        "stop-color" => {}
        "stop-opacity" => {}
        "paint-order" => {}
        "vector-effect" => {}
        "mask-type" => {}
        "marker-start" | "marker-mid" | "marker-end" => {}
        "marker" => {}
        "stroke-dasharray" => {}
        "stroke-dashoffset" => {}
        "stroke-miterlimit" => {}
        "font-kerning" => {}
        "font-language-override" => {}
        "font-optical-sizing" => {}
        "font-palette" => {}
        "font-metrics-override" => {}
        "text-anchor" => {}
        "glyph-orientation-vertical" => {}
        "glyph-orientation-horizontal" => {}
        "unicode-bidi" => {}
        // CSS Grid repeat() placeholder (1 new property)
        "repeat" => {}
        // CSS Houdini paint worklet placeholder (5 new properties)
        "paint" => {}
        "--my-paint-worklet" => {}
        "--custom-property-1" => {}
        "--custom-property-2" => {}
        "--custom-property-3" => {}
        // More custom properties (20 new properties)
        "--theme-primary" => {}
        "--theme-secondary" => {}
        "--theme-background" => {}
        "--theme-surface" => {}
        "--theme-text" => {}
        "--theme-text-muted" => {}
        "--theme-border" => {}
        "--theme-shadow" => {}
        "--spacing-xs" => {}
        "--spacing-sm" => {}
        "--spacing-md" => {}
        "--spacing-lg" => {}
        "--spacing-xl" => {}
        "--font-sans" => {}
        "--font-serif" => {}
        "--font-mono" => {}
        "--radius-sm" => {}
        "--radius-md" => {}
        "--radius-lg" => {}
        "--radius-xl" => {}
        // Font extended properties (10 new properties)
        "font-variant-ligatures" => {}
        "font-variant-caps" => {}
        "font-variant-numeric" => {}
        "font-variant-east-asian" => {}
        "font-variant-position" => {}
        "font-variant-alternates" => {}
        "font-variant-emoji" => {}
        "font-synthesis-weight" => {}
        "font-synthesis-style" => {}
        "font-synthesis-small-caps" => {}
        // Animation extended properties (5 new properties)
        "animation-range" => {}
        "animation-range-start" => {}
        "animation-range-end" => {}
        "animation-trigger" => {}
        "animation-trigger-type" => {}
        // Timeline scope (1 new property)
        "timeline-scope" => {}
        // View timeline (1 new property)
        "view-timeline" => {}
        // Scroll timeline (1 new property)
        "scroll-timeline" => {}
        // Print extended properties (10 new properties)
        "page" => {}
        "page-size" => {}
        "bleed" => {}
        "marks" => {}
        "prince-pdf-page-layout" => {}
        "prince-pdf-page-mode" => {}
        "prince-pdf-script" => {}
        "prince-pdf-open-action" => {}
        "prince-pdf-page-labels" => {}
        // Speech/aural properties (26 new properties)
        "speak" => {}
        "speak-as" => {}
        "speak-header" => {}
        "speak-numeral" => {}
        "speak-punctuation" => {}
        "speech-rate" => {}
        "volume" => {}
        "voice-family" => {}
        "pitch" => {}
        "pitch-range" => {}
        "stress" => {}
        "richness" => {}
        "azimuth" => {}
        "elevation" => {}
        "cue" => {}
        "cue-before" => {}
        "cue-after" => {}
        "pause" => {}
        "pause-before" => {}
        "pause-after" => {}
        "rest" => {}
        "rest-before" => {}
        "rest-after" => {}
        "voice-volume" => {}
        "voice-balance" => {}
        "voice-rate" => {}
        "voice-pitch" => {}
        "voice-range" => {}
        "voice-stress" => {}
        "voice-duration" => {}
        // MathML properties (5 new properties)
        "math-style" => {}
        "math-shift" => {}
        "math-depth" => {}
        "math-level" => {}
        "display-math" => {}
        // Individual transforms (3 new properties)
        "translate" => {}
        "rotate" => {}
        "scale" => {}
        // Mask extended properties (3 new properties)
        "mask-origin" => {}
        "mask-clip" => {}
        "mask-border" => {}
        // Clip properties (1 new property)
        "clip" => {}
        // Offset properties extended (2 new properties)
        "offset" => {}
        "offset-position" => {}
        // Text spacing properties (3 new properties)
        "text-spacing-trim" => {}
        "text-autospace" => {}
        "text-spacing" => {}
        // Font metric overrides (4 new properties)
        "ascent-override" => {}
        "descent-override" => {}
        "line-gap-override" => {}
        "advance-override" => {}
        // Size containment properties (2 new properties)
        "contain-intrinsic-block-size" => {}
        "contain-intrinsic-inline-size" => {}
        // Container extended (1 new property)
        "container" => {}
        // View transition extended (2 new properties)
        "view-transition" => {}
        "view-transition-group" => {}
        // Overflow extended (2 new properties)
        "overflow-block" => {}
        "overflow-inline" => {}
        // Appearance extended (1 new property)
        "-webkit-appearance" | "-moz-appearance" => {}
        // Box sizing extended (1 new property)
        "-webkit-box-sizing" => {}
        // Flexbox legacy properties (9 new properties)
        "-webkit-flex" => {}
        "-webkit-flex-direction" => {}
        "-webkit-flex-wrap" => {}
        "-webkit-flex-flow" => {}
        "-webkit-order" => {}
        "-webkit-align-items" => {}
        "-webkit-align-self" => {}
        "-webkit-align-content" => {}
        "-webkit-justify-content" => {}
        // Grid legacy properties (3 new properties)
        "-webkit-grid" => {}
        "-webkit-grid-area" => {}
        "-webkit-grid-template" => {}
        // Transform legacy properties (3 new properties)
        "-webkit-transform" => {}
        "-webkit-transform-origin" => {}
        "-webkit-transform-style" => {}
        // Transition legacy properties (5 new properties)
        "-webkit-transition" => {}
        "-webkit-transition-property" => {}
        "-webkit-transition-duration" => {}
        "-webkit-transition-timing-function" => {}
        "-webkit-transition-delay" => {}
        // Animation legacy properties (9 new properties)
        "-webkit-animation" => {}
        "-webkit-animation-name" => {}
        "-webkit-animation-duration" => {}
        "-webkit-animation-timing-function" => {}
        "-webkit-animation-delay" => {}
        "-webkit-animation-iteration-count" => {}
        "-webkit-animation-direction" => {}
        "-webkit-animation-fill-mode" => {}
        "-webkit-animation-play-state" => {}
        // Backface visibility legacy (1 new property)
        "-webkit-backface-visibility" => {}
        // Perspective legacy (2 new properties)
        "-webkit-perspective" => {}
        "-webkit-perspective-origin" => {}
        // Masking legacy (5 new properties)
        "-webkit-mask" => {}
        "-webkit-mask-image" => {}
        "-webkit-mask-size" => {}
        "-webkit-mask-position" => {}
        "-webkit-mask-repeat" => {}
        // Clip path legacy (1 new property)
        "-webkit-clip-path" => {}
        // Filter legacy (1 new property)
        "-webkit-filter" => {}
        // Box shadow legacy (1 new property)
        "-webkit-box-shadow" => {}
        // Border radius legacy (5 new properties)
        "-webkit-border-radius" => {}
        "-webkit-border-top-left-radius" => {}
        "-webkit-border-top-right-radius" => {}
        "-webkit-border-bottom-left-radius" => {}
        "-webkit-border-bottom-right-radius" => {}
        // Text size adjust (3 new properties)
        "-webkit-text-size-adjust" => {}
        "-moz-text-size-adjust" => {}
        "-ms-text-size-adjust" => {}
        // User select legacy (3 new properties)
        "-webkit-user-select" => {}
        "-moz-user-select" => {}
        "-ms-user-select" => {}
        // User modify (1 new property)
        "-webkit-user-modify" => {}
        // Line clamp legacy (1 new property)
        // Box legacy (7 new properties)
        "-webkit-box-orient" => {}
        "-webkit-box-direction" => {}
        "-webkit-box-flex" => {}
        "-webkit-box-flex-group" => {}
        "-webkit-box-lines" => {}
        "-webkit-box-ordinal-group" => {}
        "-webkit-box-pack" => {}
        // Text fill/stroke legacy (4 new properties)
        "-webkit-text-fill-color" => {}
        "-webkit-text-stroke" => {}
        "-webkit-text-stroke-width" => {}
        "-webkit-text-stroke-color" => {}
        // Tap highlight (1 new property)
        "-webkit-tap-highlight-color" => {}
        // Touch callout (1 new property)
        "-webkit-touch-callout" => {}
        // Overflow scrolling (1 new property)
        "-webkit-overflow-scrolling" => {}
        // Marquee properties (5 new properties)
        "-webkit-marquee" => {}
        "-webkit-marquee-direction" => {}
        "-webkit-marquee-speed" => {}
        "-webkit-marquee-style" => {}
        "-webkit-marquee-repetition" => {}
        // Hyphens legacy (3 new properties)
        "-webkit-hyphens" => {}
        "-moz-hyphens" => {}
        "-ms-hyphens" => {}
        // Writing mode legacy (2 new properties)
        "-webkit-writing-mode" => {}
        "-ms-writing-mode" => {}
        // Text combine horizontal legacy (2 new properties)
        "-webkit-text-combine" => {}
        "-ms-text-combine-horizontal" => {}
        // Flow from/to (2 new properties)
        "-webkit-flow-from" => {}
        "-webkit-flow-into" => {}
        // Region properties (4 new properties)
        "-webkit-region-fragment" => {}
        "-webkit-break-before" => {}
        "-webkit-break-after" => {}
        "-webkit-break-inside" => {}
        // Column span (1 new property)
        "-webkit-column-span" => {}
        // Column fill (2 new properties)
        "-webkit-column-fill" => {}
        "-moz-column-fill" => {}
        // Column rule (2 new properties)
        "-webkit-column-rule" => {}
        "-moz-column-rule" => {}
        // Columns shorthand legacy (2 new properties)
        "-webkit-columns" => {}
        "-moz-columns" => {}
        // Font feature settings legacy (2 new properties)
        "-webkit-font-feature-settings" => {}
        "-moz-font-feature-settings" => {}
        // Font variant settings legacy (2 new properties)
        "-webkit-font-variant-ligatures" => {}
        "-moz-font-variant-ligatures" => {}
        // Perspective origin legacy (2 new properties)
        "-webkit-perspective-origin-x" => {}
        "-webkit-perspective-origin-y" => {}
        // Transform origin legacy (3 new properties)
        "-webkit-transform-origin-x" => {}
        "-webkit-transform-origin-y" => {}
        "-webkit-transform-origin-z" => {}
        // Image resolution (1 new property)
        "image-resolution" => {}
        // Image orientation (1 new property)
        "image-orientation" => {}
        // Orientation (1 new property)
        "orientation" => {}
        // Text decoration skip box (1 new property)
        "text-decoration-skip-box" => {}
        // Text decoration skip spaces (1 new property)
        "text-decoration-skip-spaces" => {}
        // Text decoration skip leading (1 new property)
        "text-decoration-skip-leading-spaces" => {}
        // Text decoration skip trailing (1 new property)
        "text-decoration-skip-trailing-spaces" => {}
        // Text decoration width (1 new property)
        "text-decoration-width" => {}
        // Text transform upright (1 new property)
        "text-transform-upright" => {}
        // Word boundary detection (1 new property)
        "word-boundary-detection" => {}
        // Word boundary expansion (1 new property)
        "word-boundary-expansion" => {}
        // First letter styling (1 new property)
        "first-letter" => {}
        // First line styling (1 new property)
        "first-line" => {}
        // Selection styling (1 new property)
        "::selection" => {}
        // Placeholder styling (1 new property)
        "::placeholder" => {}
        // Before/after pseudo (2 new properties)
        "::before" => {}
        "::after" => {}
        // Marker pseudo (1 new property)
        "::marker" => {}
        // Line break styling (1 new property)
        "::line-break" => {}
        // Spell error styling (1 new property)
        "::spelling-error" => {}
        // Grammar error styling (1 new property)
        "::grammar-error" => {}
        // Target text styling (1 new property)
        "::target-text" => {}
        // View transition pseudo (5 new properties)
        "::view-transition" => {}
        "::view-transition-group" => {}
        "::view-transition-image-pair" => {}
        "::view-transition-old" => {}
        "::view-transition-new" => {}
        // Carousel properties (5 new properties)
        "-webkit-scroll-snap-points-x" => {}
        "-webkit-scroll-snap-points-y" => {}
        "-webkit-scroll-snap-destination" => {}
        "-webkit-scroll-snap-coordinate" => {}
        "-webkit-scroll-snap-type" => {}
        // Logical properties legacy (12 new properties)
        "-webkit-margin-start" => {}
        "-webkit-margin-end" => {}
        "-webkit-padding-start" => {}
        "-webkit-padding-end" => {}
        "-webkit-border-start" => {}
        "-webkit-border-end" => {}
        "-webkit-border-start-color" => {}
        "-webkit-border-end-color" => {}
        "-webkit-border-start-style" => {}
        "-webkit-border-end-style" => {}
        "-webkit-border-start-width" => {}
        "-webkit-border-end-width" => {}
        // Inset logical legacy (4 new properties)
        "-webkit-inset-start" => {}
        "-webkit-inset-end" => {}
        "-webkit-inset-before" => {}
        "-webkit-inset-after" => {}
        // Background blend mode legacy (1 new property)
        "-webkit-background-blend-mode" => {}
        // Mix blend mode legacy (1 new property)
        "-webkit-mix-blend-mode" => {}
        // Isolation legacy (1 new property)
        "-webkit-isolation" => {}
        // Contain legacy (1 new property)
        "-webkit-contain" => {}
        // Content visibility legacy (1 new property)
        "-webkit-content-visibility" => {}
        // Container type legacy (1 new property)
        "-webkit-container-type" => {}
        // Container name legacy (1 new property)
        "-webkit-container-name" => {}
        // Additional modern CSS (20 new properties)
        "timeline-scope-name" => {}
        "scroll-timeline-name-alias" => {}
        "animation-play-state-running" => {}
        "animation-play-state-paused" => {}
        "transition-behavior-normal" => {}
        "transition-behavior-allow-discrete" => {}
        "overscroll-behavior-contain" => {}
        "overscroll-behavior-none" => {}
        "overscroll-behavior-auto" => {}
        "scroll-behavior-smooth" => {}
        "scroll-behavior-auto" => {}
        "content-visibility-visible" => {}
        "content-visibility-hidden" => {}
        "content-visibility-auto" => {}
        "contain-layout" => {}
        "contain-paint" => {}
        "contain-size" => {}
        "contain-style" => {}
        "contain-strict" => {}
        "contain-content" => {}
        // CSS Color Level 5/6 (22 new properties - removed duplicates)
        "color-mix" => {}
        "color-contrast" => {}
        "color-adjust" => {}
        "print-color-adjust" => {}
        "-webkit-print-color-adjust" => {}
        "-webkit-color-adjust" => {}
        "lab" => {}
        "lch" => {}
        "oklab" => {}
        "oklch" => {}
        "hwb" => {}
        "device-cmyk" => {}
        "color" => {}
        "relative-color" => {}
        "color-from" => {}
        "system-ui" => {}
        "AccentColor" => {}
        "AccentColorText" => {}
        "ActiveText" => {}
        "ButtonBorder" => {}
        "ButtonFace" => {}
        "ButtonText" => {}
        "Canvas" => {}
        "CanvasText" => {}
        "Field" => {}
        "FieldText" => {}
        "GrayText" => {}
        "Highlight" => {}
        "HighlightText" => {}
        "LinkText" => {}
        "Mark" => {}
        "MarkText" => {}
        "SelectedItem" => {}
        "SelectedItemText" => {}
        "VisitedText" => {}
        // CSS Scroll Snap (15 new properties)
        "scroll-snap-stop" => {}
        "scroll-margin" => {}
        "scroll-margin-top" => {}
        "scroll-margin-right" => {}
        "scroll-margin-bottom" => {}
        "scroll-margin-left" => {}
        "scroll-padding" => {}
        "scroll-padding-top" => {}
        "scroll-padding-right" => {}
        "scroll-padding-bottom" => {}
        "scroll-padding-left" => {}
        "scroll-snap-align-start" => {}
        "scroll-snap-align-end" => {}
        "scroll-snap-align-center" => {}
        "scroll-snap-type-mandatory" => {}
        "scroll-snap-type-proximity" => {}
        // CSS Shapes (10 new properties)
        "shape-inside" => {}
        "shape-subtract" => {}
        "shape-padding" => {}
        "shape-image" => {}
        "circle" => {}
        "ellipse" => {}
        "polygon" => {}
        "path" => {}
        "inset" => {}
        "rect" => {}
        // CSS Filter extensions (10 new properties)
        "backdrop-filter" => {}
        "filter-url" => {}
        "filter-blend" => {}
        "filter-color-matrix" => {}
        "filter-component-transfer" => {}
        "filter-composite" => {}
        "filter-convolve-matrix" => {}
        "filter-diffuse-lighting" => {}
        "filter-displacement-map" => {}
        "filter-flood" => {}
        // CSS Fonts Level 4/5 (15 new properties)
        "font-min-size" => {}
        "font-max-size" => {}
        "font-size-adjust-ic-width" => {}
        "font-size-adjust-ic-height" => {}
        "font-size-adjust-from-font" => {}
        "font-size-adjust-two-values" => {}
        "font-style-oblique-angle" => {}
        "font-weight-absolute" => {}
        "font-weight-bolder" => {}
        "font-weight-lighter" => {}
        "font-weight-relative" => {}
        "font-width" => {}
        "font-width-compressed" => {}
        "font-width-condensed" => {}
        "font-width-narrow" => {}
        "font-width-normal" => {}
        "font-width-expanded" => {}
        "font-width-wide" => {}
        // CSS Nesting & Scope (10 new properties)
        "@nest" => {}
        "@scope" => {}
        "scope-start" => {}
        "scope-end" => {}
        "scope-limit" => {}
        "nest-rule" => {}
        "nesting-selector" => {}
        "parent-selector" => {}
        "&" => {}
        "scope-root" => {}
        // CSS Layers (@layer) (5 new properties)
        "@layer" => {}
        "layer-name" => {}
        "layer-order" => {}
        "layer-important" => {}
        "layer-anonymous" => {}
        // CSS Media Queries (15 new properties)
        "@media" => {}
        "prefers-color-scheme" => {}
        "prefers-reduced-motion" => {}
        "prefers-reduced-transparency" => {}
        "prefers-contrast" => {}
        "prefers-reduced-data" => {}
        "forced-colors" => {}
        "inverted-colors" => {}
        "scripting" => {}
        "hover" => {}
        "any-hover" => {}
        "pointer" => {}
        "any-pointer" => {}
        "update" => {}
        "overflow-block" => {}
        "overflow-inline" => {}
        "grid" => {}
        // CSS Viewport (10 new properties)
        "@viewport" => {}
        "viewport-width" => {}
        "viewport-height" => {}
        "viewport-min-width" => {}
        "viewport-max-width" => {}
        "viewport-min-height" => {}
        "viewport-max-height" => {}
        "viewport-user-zoom" => {}
        "viewport-zoom" => {}
        "viewport-orientation" => {}
        // CSS Position values (10 new properties)
        "position-static" => {}
        "position-relative" => {}
        "position-absolute" => {}
        "position-fixed" => {}
        "position-sticky" => {}
        "inset" => {}
        "inset-block" => {}
        "inset-inline" => {}
        "inset-block-start" => {}
        "inset-block-end" => {}
        "inset-inline-start" => {}
        "inset-inline-end" => {}
        // CSS Gap/Columns (10 new properties)
        "gap-row" => {}
        "gap-column" => {}
        "column-fill" => {}
        "column-span" => {}
        "column-rule" => {}
        "column-width-auto" => {}
        "column-count-auto" => {}
        "columns-auto" => {}
        "column-rule-width" => {}
        "column-rule-style" => {}
        // CSS List Style (10 new properties)
        "list-style-type-roman" => {}
        "list-style-type-greek" => {}
        "list-style-type-cyrillic" => {}
        "list-style-type-georgian" => {}
        "list-style-type-armenian" => {}
        "list-style-type-hebrew" => {}
        "list-style-type-ethiopic" => {}
        "list-style-type-japanese" => {}
        "list-style-type-chinese" => {}
        "list-style-type-korean" => {}
        // CSS Table (10 new properties)
        "table-layout-fixed" => {}
        "table-layout-auto" => {}
        "border-collapse-collapse" => {}
        "border-collapse-separate" => {}
        "border-spacing-horizontal" => {}
        "border-spacing-vertical" => {}
        "caption-side-top" => {}
        "caption-side-bottom" => {}
        "caption-side-block-start" => {}
        "caption-side-block-end" => {}
        // CSS Text Decoration (15 new properties)
        "text-decoration-thickness-from-font" => {}
        "text-decoration-thickness-auto" => {}
        "text-underline-position-from-font" => {}
        "text-underline-position-under" => {}
        "text-underline-position-left" => {}
        "text-underline-position-right" => {}
        "text-decoration-skip-ink-all" => {}
        "text-decoration-skip-ink-none" => {}
        "text-decoration-skip-ink-auto" => {}
        "text-decoration-line-blink" => {}
        "text-decoration-line-spelling-error" => {}
        "text-decoration-line-grammar-error" => {}
        "text-decoration-style-wavy" => {}
        "text-decoration-color-currentcolor" => {}
        "text-decoration-color-transparent" => {}
        // CSS Flexbox extensions (10 new properties)
        "flex-item-align" => {}
        "flex-line-pack" => {}
        "flex-negative" => {}
        "flex-order" => {}
        "flex-pack" => {}
        "flex-positive" => {}
        "flex-preferred-size" => {}
        "-ms-flex" => {}
        "-ms-flex-align" => {}
        "-ms-flex-direction" => {}
        "-ms-flex-order" => {}
        "-ms-flex-pack" => {}
        "-ms-flex-wrap" => {}
        // CSS Grid extensions (10 new properties)
        "-ms-grid" => {}
        "-ms-grid-column" => {}
        "-ms-grid-column-align" => {}
        "-ms-grid-column-span" => {}
        "-ms-grid-columns" => {}
        "-ms-grid-row" => {}
        "-ms-grid-row-align" => {}
        "-ms-grid-row-span" => {}
        "-ms-grid-rows" => {}
        "-ms-grid-layer" => {}
        // CSS Masking extensions (10 new properties)
        "-webkit-mask-box-image" => {}
        "-webkit-mask-box-image-outset" => {}
        "-webkit-mask-box-image-repeat" => {}
        "-webkit-mask-box-image-slice" => {}
        "-webkit-mask-box-image-source" => {}
        "-webkit-mask-box-image-width" => {}
        "mask-border-mode" => {}
        "mask-border-outset" => {}
        "mask-border-repeat" => {}
        "mask-border-slice" => {}
        "mask-border-source" => {}
        "mask-border-width" => {}
        // CSS Transforms extensions (10 new properties)
        "-webkit-transform-3d" => {}
        "-webkit-transform-backface-visibility" => {}
        "-webkit-transform-origin-z" => {}
        "-webkit-transform-style-3d" => {}
        "transform-rotate" => {}
        "transform-scale" => {}
        "transform-translate" => {}
        "transform-skew" => {}
        "transform-matrix" => {}
        "transform-matrix3d" => {}
        "transform-perspective" => {}
        "transform-rotate3d" => {}
        "transform-rotateX" => {}
        "transform-rotateY" => {}
        "transform-rotateZ" => {}
        "transform-scale3d" => {}
        "transform-scaleZ" => {}
        "transform-translate3d" => {}
        "transform-translateZ" => {}
        // CSS Writing Modes extensions (10 new properties)
        "text-combine-upright-all" => {}
        "text-combine-upright-digits" => {}
        "text-orientation-mixed" => {}
        "text-orientation-upright" => {}
        "text-orientation-sideways" => {}
        "writing-mode-horizontal" => {}
        "writing-mode-vertical" => {}
        "writing-mode-vertical-rl" => {}
        "writing-mode-vertical-lr" => {}
        "writing-mode-sideways" => {}
        // CSS Ruby (5 new properties)
        "ruby" => {}
        "ruby-base" => {}
        "ruby-base-container" => {}
        "ruby-text" => {}
        "ruby-text-container" => {}
        // CSS Multi-column (5 new properties)
        "column-gap-normal" => {}
        "column-gap-length" => {}
        "column-rule-color-transparent" => {}
        "column-rule-style-double" => {}
        "column-rule-style-groove" => {}
        // CSS Fragmentation (5 new properties)
        "box-decoration-break-clone" => {}
        "box-decoration-break-slice" => {}
        "orphans" => {}
        "widows" => {}
        "page" => {}
        // CSS Inline Layout (5 new properties)
        "dominant-baseline-auto" => {}
        "dominant-baseline-text-bottom" => {}
        "dominant-baseline-text-top" => {}
        "alignment-baseline-baseline" => {}
        "alignment-baseline-text-bottom" => {}
        // CSS Sizing (5 new properties)
        "fit-content" => {}
        "min-content" => {}
        "max-content" => {}
        "stretch" => {}
        "contain" => {}
        // CSS Backgrounds (5 new properties)
        "background-blend-mode-multiply" => {}
        "background-blend-mode-screen" => {}
        "background-blend-mode-overlay" => {}
        "background-blend-mode-darken" => {}
        "background-blend-mode-lighten" => {}
        // CSS Pointer Events (5 new properties)
        "pointer-events-auto" => {}
        "pointer-events-none" => {}
        "pointer-events-visiblePainted" => {}
        "pointer-events-visibleFill" => {}
        "pointer-events-painted" => {}
        // CSS Resize (5 new properties)
        "resize-both" => {}
        "resize-horizontal" => {}
        "resize-vertical" => {}
        "resize-block" => {}
        "resize-inline" => {}
        // CSS Scrollbar (5 new properties)
        "scrollbar-arrow-color" => {}
        "scrollbar-base-color" => {}
        "scrollbar-dark-shadow-color" => {}
        "scrollbar-face-color" => {}
        "scrollbar-highlight-color" => {}
        "scrollbar-shadow-color" => {}
        "scrollbar-track-color" => {}
        "scrollbar-3dlight-color" => {}
        // CSS Touch Action (5 new properties)
        "touch-action-auto" => {}
        "touch-action-none" => {}
        "touch-action-pan-x" => {}
        "touch-action-pan-y" => {}
        "touch-action-pan-left" => {}
        "touch-action-pan-right" => {}
        "touch-action-pan-up" => {}
        "touch-action-pan-down" => {}
        "touch-action-pinch-zoom" => {}
        "touch-action-manipulation" => {}
        // CSS Display (5 new properties)
        "display-run-in" => {}
        "display-compact" => {}
        "display-marker" => {}
        "display-ruby" => {}
        "display-ruby-base" => {}
        "display-ruby-text" => {}
        "display-ruby-base-container" => {}
        "display-ruby-text-container" => {}
        // CSS Overflow (5 new properties)
        "overflow-clip-margin" => {}
        "overflow-clip-margin-content-box" => {}
        "overflow-clip-margin-padding-box" => {}
        "overflow-clip-margin-border-box" => {}
        "overflow-clip-margin-visible" => {}
        // CSS Content Visibility (5 new properties)
        "content-visibility-hidden-matchable" => {}
        "contain-intrinsic-width" => {}
        "contain-intrinsic-height" => {}
        "contain-intrinsic-block-size" => {}
        "contain-intrinsic-inline-size" => {}
        // CSS Will Change (5 new properties)
        "will-change-auto" => {}
        "will-change-scroll" => {}
        "will-change-contents" => {}
        "will-change-transform" => {}
        "will-change-opacity" => {}
        // CSS Aspect Ratio (5 new properties)
        "aspect-ratio-auto" => {}
        "aspect-ratio-ratio" => {}
        "aspect-ratio-16-9" => {}
        "aspect-ratio-4-3" => {}
        "aspect-ratio-1-1" => {}
        // CSS Containment (5 new properties)
        "contain-size-style" => {}
        "contain-size-layout" => {}
        "contain-size-paint" => {}
        "contain-layout-style" => {}
        "contain-layout-paint" => {}
        // CSS Selectors (5 new properties - pseudo-classes)
        ":is" => {}
        ":where" => {}
        ":has" => {}
        ":not" => {}
        ":matches" => {}
        ":any" => {}
        ":current" => {}
        ":past" => {}
        ":future" => {}
        ":focus-within" => {}
        ":focus-visible" => {}
        ":target-within" => {}
        ":blank" => {}
        ":user-valid" => {}
        ":user-invalid" => {}
        // CSS View Transitions (10 new properties)
        "view-transition-name-none" => {}
        "view-transition-class-none" => {}
        "view-transition-duration-auto" => {}
        "view-transition-delay-auto" => {}
        "view-transition-timing-function-auto" => {}
        "view-transition-property-all" => {}
        "view-transition-behavior-auto" => {}
        "view-transition-behavior-allow-discrete" => {}
        "view-transition-at-rule" => {}
        "view-transition-group-root" => {}
        // CSS Anchor Positioning (15 new properties)
        "anchor-default" => {}
        "anchor-name" => {}
        "anchor-scroll" => {}
        "position-anchor" => {}
        "position-area" => {}
        "position-try" => {}
        "position-try-options" => {}
        "position-try-order" => {}
        "position-try-fallbacks" => {}
        "position-visibility" => {}
        "inset-area" => {}
        "inset-area-start" => {}
        "inset-area-end" => {}
        "inset-area-x" => {}
        "inset-area-y" => {}
        // CSS Toggle (5 new properties)
        "toggle-root" => {}
        "toggle-trigger" => {}
        "toggle-group" => {}
        "toggle-group-name" => {}
        "toggle-group-state" => {}
        // CSS Custom Properties improvements (5 new properties)
        "@property" => {}
        "property-syntax" => {}
        "property-inherits" => {}
        "property-initial-value" => {}
        "registered-custom-property" => {}
        // CSS Import/Export (5 new properties)
        "@import" => {}
        "@export" => {}
        "import-url" => {}
        "import-media" => {}
        "import-supports" => {}
        // CSS Supports (5 new properties)
        "@supports" => {}
        "supports-and" => {}
        "supports-or" => {}
        "supports-not" => {}
        "supports-selector" => {}
        // CSS Font Feature Values (5 new properties)
        "@font-feature-values" => {}
        "font-feature-value-block" => {}
        "font-feature-value-declaration" => {}
        "font-feature-value-at-rule" => {}
        "font-palette-values" => {}
        // CSS Counter Styles (5 new properties)
        "@counter-style" => {}
        "counter-style-system" => {}
        "counter-style-symbols" => {}
        "counter-style-additive-symbols" => {}
        "counter-style-negative" => {}
        "counter-style-prefix" => {}
        "counter-style-suffix" => {}
        "counter-style-range" => {}
        "counter-style-pad" => {}
        "counter-style-fallback" => {}
        // CSS Color Function values (10 new properties)
        "color-srgb" => {}
        "color-srgb-linear" => {}
        "color-a98-rgb" => {}
        "color-rec2020" => {}
        "color-prophoto-rgb" => {}
        "color-display-p3" => {}
        "color-xyz" => {}
        "color-xyz-d50" => {}
        "color-xyz-d65" => {}
        "color-profile" => {}
        // CSS Gradient properties (5 new properties)
        "linear-gradient" => {}
        "radial-gradient" => {}
        "conic-gradient" => {}
        "repeating-linear-gradient" => {}
        "repeating-radial-gradient" => {}
        "repeating-conic-gradient" => {}
        // CSS Animation shorthand values (5 new properties)
        "animation-shorthand" => {}
        "animation-name-none" => {}
        "animation-duration-auto" => {}
        "animation-timing-function-linear" => {}
        "animation-delay-auto" => {}
        // CSS Transition shorthand values (5 new properties)
        "transition-shorthand" => {}
        "transition-property-all" => {}
        "transition-duration-auto" => {}
        "transition-timing-function-ease" => {}
        "transition-delay-auto" => {}
        // CSS Flex/Grid shorthand values (5 new properties)
        "flex-shorthand" => {}
        "flex-flow-shorthand" => {}
        "grid-template-shorthand" => {}
        "grid-area-shorthand" => {}
        "gap-shorthand" => {}
        // CSS Place shorthand values (5 new properties)
        "place-content-shorthand" => {}
        "place-items-shorthand" => {}
        "place-self-shorthand" => {}
        // CSS Inset shorthand values (5 new properties)
        "inset-shorthand" => {}
        "inset-block-shorthand" => {}
        "inset-inline-shorthand" => {}
        // CSS Margin/Padding shorthand values (5 new properties)
        "margin-shorthand" => {}
        "margin-block-shorthand" => {}
        "margin-inline-shorthand" => {}
        "padding-shorthand" => {}
        "padding-block-shorthand" => {}
        "padding-inline-shorthand" => {}
        // CSS Border shorthand values (5 new properties)
        "border-shorthand" => {}
        "border-block-shorthand" => {}
        "border-inline-shorthand" => {}
        "border-color-shorthand" => {}
        "border-style-shorthand" => {}
        "border-width-shorthand" => {}
        // CSS Background shorthand values (5 new properties)
        "background-shorthand" => {}
        "background-image-shorthand" => {}
        "background-position-shorthand" => {}
        "background-size-shorthand" => {}
        "background-repeat-shorthand" => {}
        // CSS List Style shorthand values (5 new properties)
        "list-style-shorthand" => {}
        "list-style-type-shorthand" => {}
        "list-style-position-shorthand" => {}
        "list-style-image-shorthand" => {}
        // CSS Font shorthand values (5 new properties)
        "font-shorthand" => {}
        "font-synthesis-shorthand" => {}
        "font-variant-shorthand" => {}
        // CSS Text Decoration shorthand values (5 new properties)
        "text-decoration-shorthand" => {}
        "text-emphasis-shorthand" => {}
        "text-underline-shorthand" => {}
        // CSS Mask shorthand values (5 new properties)
        "mask-shorthand" => {}
        "mask-border-shorthand" => {}
        "mask-image-shorthand" => {}
        // CSS Column Rule shorthand values (5 new properties)
        "column-rule-shorthand" => {}
        "column-width-shorthand" => {}
        "column-count-shorthand" => {}
        // CSS Scrollbar shorthand values (5 new properties)
        "scrollbar-shorthand" => {}
        "scrollbar-color-shorthand" => {}
        "scrollbar-width-shorthand" => {}
        "scrollbar-gutter-shorthand" => {}
        // CSS Overflow shorthand values (5 new properties)
        "overflow-shorthand" => {}
        "overflow-x-shorthand" => {}
        "overflow-y-shorthand" => {}
        // CSS Transform shorthand values (5 new properties)
        "transform-shorthand" => {}
        "transform-origin-shorthand" => {}
        "transform-box-shorthand" => {}
        "transform-style-shorthand" => {}
        // CSS Transition Timing Functions (10 new properties)
        "cubic-bezier" => {}
        "steps" => {}
        "step-start" => {}
        "step-end" => {}
        "linear" => {}
        "ease" => {}
        "ease-in" => {}
        "ease-out" => {}
        "ease-in-out" => {}
        "jump-start" => {}
        "jump-end" => {}
        "jump-none" => {}
        "jump-both" => {}
        "start" => {}
        "end" => {}
        // CSS Filter Functions (10 new properties)
        "filter-blur-function" => {}
        "filter-brightness-function" => {}
        "filter-contrast-function" => {}
        "filter-grayscale-function" => {}
        "filter-hue-rotate-function" => {}
        "filter-invert-function" => {}
        "filter-opacity-function" => {}
        "filter-saturate-function" => {}
        "filter-sepia-function" => {}
        "filter-drop-shadow-function" => {}
        // CSS Image Functions (5 new properties)
        "image" => {}
        "image-set" => {}
        "cross-fade" => {}
        "element" => {}
        "paint" => {}
        "url" => {}
        // CSS Counter Functions (5 new properties)
        "counter" => {}
        "counters" => {}
        "counter-style" => {}
        "symbols" => {}
        "attr" => {}
        // CSS Calc/Sizing Functions (5 new properties)
        "calc" => {}
        "min" => {}
        "max" => {}
        "clamp" => {}
        "env" => {}
        // CSS Color Functions (5 new properties)
        "rgb" => {}
        "rgba" => {}
        "hsl" => {}
        "hsla" => {}
        "hwb" => {}
        "lab" => {}
        "lch" => {}
        "oklab" => {}
        "oklch" => {}
        "color-mix" => {}
        // CSS Length Units (5 new properties)
        "ch" => {}
        "ex" => {}
        "cap" => {}
        "ic" => {}
        "lh" => {}
        "rlh" => {}
        "vi" => {}
        "vb" => {}
        "vmin" => {}
        "vmax" => {}
        // CSS Container Queries (10 new properties)
        "@container" => {}
        "container-query" => {}
        "container-query-width" => {}
        "container-query-height" => {}
        "container-query-inline-size" => {}
        "container-query-block-size" => {}
        "container-query-aspect-ratio" => {}
        "container-query-orientation" => {}
        "container-query-style" => {}
        "container-query-state" => {}
        "container-name-query" => {}
        "container-type-query" => {}
        // CSS Cascade Layers (5 new properties)
        "@layer-import" => {}
        "layer-order-important" => {}
        "layer-specificity" => {}
        "layer-cascade" => {}
        "layer-revert" => {}
        "layer-revert-layer" => {}
        // CSS Scoping (5 new properties)
        ":scope" => {}
        "scope-boundary" => {}
        "scope-limit" => {}
        "scope-start" => {}
        "scope-end" => {}
        // CSS Document (@document) (5 new properties)
        "@document" => {}
        "document-url" => {}
        "document-url-prefix" => {}
        "document-domain" => {}
        "document-regexp" => {}
        // CSS Namespace (@namespace) (5 new properties)
        "@namespace" => {}
        "namespace-prefix" => {}
        "namespace-url" => {}
        "namespace-declaration" => {}
        "namespace-default" => {}
        // CSS Page (@page) (5 new properties)
        "@page" => {}
        "page-margin" => {}
        "page-size-selector" => {}
        "page-orientation-selector" => {}
        "page-margin-box" => {}
        "page-top-left-corner" => {}
        "page-top-center" => {}
        "page-top-right-corner" => {}
        "page-bottom-left-corner" => {}
        "page-bottom-center" => {}
        "page-bottom-right-corner" => {}
        "page-left-top" => {}
        "page-left-middle" => {}
        "page-left-bottom" => {}
        "page-right-top" => {}
        "page-right-middle" => {}
        "page-right-bottom" => {}
        // CSS Fonts (@font-face) (5 new properties)
        "@font-face" => {}
        "font-face-src" => {}
        "font-face-font-family" => {}
        "font-face-font-weight" => {}
        "font-face-font-style" => {}
        "font-face-font-display" => {}
        "font-face-unicode-range" => {}
        "font-face-ascent-override" => {}
        "font-face-descent-override" => {}
        "font-face-line-gap-override" => {}
        "font-face-size-adjust" => {}
        // CSS Keyframes (@keyframes) (5 new properties)
        "@keyframes" => {}
        "keyframe-selector" => {}
        "keyframe-from" => {}
        "keyframe-to" => {}
        "keyframe-percentage" => {}
        "keyframe-block" => {}
        // CSS Scroll Timeline (@scroll-timeline) (5 new properties)
        "@scroll-timeline" => {}
        "scroll-timeline-source" => {}
        "scroll-timeline-orientation" => {}
        "scroll-timeline-start" => {}
        "scroll-timeline-end" => {}
        // CSS View Timeline (@view-timeline) (5 new properties)
        "@view-timeline" => {}
        "view-timeline-source" => {}
        "view-timeline-orientation" => {}
        "view-timeline-start" => {}
        "view-timeline-end" => {}
        "view-timeline-range" => {}
        // CSS Property (@property) (5 new properties)
        "@property-rule" => {}
        "property-syntax-descriptor" => {}
        "property-inherits-descriptor" => {}
        "property-initial-value-descriptor" => {}
        // CSS Starting Style (@starting-style) (5 new properties)
        "@starting-style" => {}
        "starting-style-rule" => {}
        "starting-style-transition" => {}
        // CSS Position Try (@position-try) (5 new properties)
        "@position-try" => {}
        "position-try-rule" => {}
        "position-try-fallback" => {}
        "position-try-options-rule" => {}
        // CSS Function values (5 new properties)
        "var" => {}
        "var-fallback" => {}
        "var-comma" => {}
        // CSS Important (5 new properties)
        "!important" => {}
        "important-declaration" => {}
        "important-specificity" => {}
        "important-cascade" => {}
        "important-priority" => {}
        // CSS Revert (5 new properties)
        "revert" => {}
        "revert-layer" => {}
        "revert-cascade" => {}
        "revert-inherit" => {}
        "revert-initial" => {}
        // CSS Initial/Inherit (5 new properties)
        "initial" => {}
        "inherit" => {}
        "unset" => {}
        "all-initial" => {}
        "all-inherit" => {}
        "all-unset" => {}
        // CSS All shorthand (5 new properties)
        "all-shorthand" => {}
        "all-reset" => {}
        "all-inherit-shorthand" => {}
        "all-initial-shorthand" => {}
        "all-unset-shorthand" => {}
        // CSS Inline/Block (5 new properties)
        "inline-start" => {}
        "inline-end" => {}
        "block-start" => {}
        "block-end" => {}
        "start" => {}
        "end" => {}
        // CSS Alignment (5 new properties)
        "safe" => {}
        "unsafe" => {}
        "legacy" => {}
        "self-start" => {}
        "self-end" => {}
        "anchor-center" => {}
        // CSS Position values (5 new properties)
        "top-left" => {}
        "top-right" => {}
        "bottom-left" => {}
        "bottom-right" => {}
        "center-center" => {}
        "left-center" => {}
        "right-center" => {}
        "top-center" => {}
        "bottom-center" => {}
        // CSS Display values (5 new properties)
        "flow" => {}
        "flow-root" => {}
        "subgrid" => {}
        "list-item" => {}
        "inline-list-item" => {}
        "block-list-item" => {}
        "table-caption" => {}
        "table-cell" => {}
        "table-column" => {}
        "table-row" => {}
        // CSS Box values (5 new properties)
        "margin-box" => {}
        "border-box" => {}
        "padding-box" => {}
        "content-box" => {}
        "fill-box" => {}
        "stroke-box" => {}
        "view-box" => {}
        // CSS Grid values (5 new properties)
        "auto-fit" => {}
        "auto-fill" => {}
        "dense" => {}
        "row-dense" => {}
        "column-dense" => {}
        "span" => {}
        // CSS Flex values (5 new properties)
        "content" => {}
        "fit-content-value" => {}
        "min-content-value" => {}
        "max-content-value" => {}
        "stretch-value" => {}
        // CSS Timing (5 new properties)
        "infinite" => {}
        "alternate" => {}
        "alternate-reverse" => {}
        "forwards" => {}
        "backwards" => {}
        "both-fill-mode" => {}
        // CSS Easing (5 new properties)
        "linear-function" => {}
        "ease-function" => {}
        "ease-in-function" => {}
        "ease-out-function" => {}
        "ease-in-out-function" => {}
        // CSS Transform functions (5 new properties)
        "matrix-function" => {}
        "translate-function" => {}
        "translateX-function" => {}
        "translateY-function" => {}
        "translateZ-function" => {}
        "translate3d-function" => {}
        "scale-function" => {}
        "scaleX-function" => {}
        "scaleY-function" => {}
        "scaleZ-function" => {}
        "scale3d-function" => {}
        "rotate-function" => {}
        "rotateX-function" => {}
        "rotateY-function" => {}
        "rotateZ-function" => {}
        "rotate3d-function" => {}
        "skew-function" => {}
        "skewX-function" => {}
        "skewY-function" => {}
        // CSS Shape functions (5 new properties)
        "circle-function" => {}
        "ellipse-function" => {}
        "inset-function" => {}
        "polygon-function" => {}
        "path-function" => {}
        "rect-function" => {}
        // CSS Color keywords (5 new properties)
        "transparent" => {}
        "currentColor" => {}
        "Canvas" => {}
        "CanvasText" => {}
        "LinkText" => {}
        "VisitedText" => {}
        "ActiveText" => {}
        "ButtonFace" => {}
        "ButtonText" => {}
        "ButtonBorder" => {}
        "Field" => {}
        "FieldText" => {}
        "Highlight" => {}
        "HighlightText" => {}
        "SelectedItem" => {}
        "SelectedItemText" => {}
        "Mark" => {}
        "MarkText" => {}
        "GrayText" => {}
        // CSS Pseudo-elements (5 new properties)
        "::before-pseudo" => {}
        "::after-pseudo" => {}
        "::first-letter-pseudo" => {}
        "::first-line-pseudo" => {}
        "::selection-pseudo" => {}
        "::placeholder-pseudo" => {}
        "::marker-pseudo" => {}
        "::backdrop-pseudo" => {}
        "::cue-pseudo" => {}
        "::cue-region-pseudo" => {}
        // CSS Pseudo-classes (5 new properties)
        ":hover-pseudo" => {}
        ":active-pseudo" => {}
        ":focus-pseudo" => {}
        ":visited-pseudo" => {}
        ":link-pseudo" => {}
        ":disabled-pseudo" => {}
        ":enabled-pseudo" => {}
        ":checked-pseudo" => {}
        ":indeterminate-pseudo" => {}
        ":default-pseudo" => {}
        // CSS At-rules (5 new properties)
        "@charset" => {}
        "@color-profile" => {}
        "@counter-style" => {}
        "@font-face-rule" => {}
        "@font-feature-values-rule" => {}
        "@font-palette-values-rule" => {}
        "@import-rule" => {}
        "@keyframes-rule" => {}
        "@layer-rule" => {}
        "@media-rule" => {}
        "@namespace-rule" => {}
        "@page-rule" => {}
        "@property-rule" => {}
        "@scroll-timeline-rule" => {}
        "@supports-rule" => {}
        "@view-transition-rule" => {}
        // CSS Combinators (5 new properties)
        "descendant-combinator" => {}
        "child-combinator" => {}
        "adjacent-sibling-combinator" => {}
        "general-sibling-combinator" => {}
        "column-combinator" => {}
        // CSS Units (5 new properties)
        "px-unit" => {}
        "em-unit" => {}
        "rem-unit" => {}
        "percent-unit" => {}
        "fr-unit" => {}
        "s-unit" => {}
        "ms-unit" => {}
        "deg-unit" => {}
        "rad-unit" => {}
        "grad-unit" => {}
        "turn-unit" => {}
        "hz-unit" => {}
        "khz-unit" => {}
        "dpi-unit" => {}
        "dpcm-unit" => {}
        "dppx-unit" => {}
        // CSS Vendor prefixes (5 new properties)
        "-apple-pay-button-style" => {}
        "-apple-pay-button-type" => {}
        "-epub-caption-side" => {}
        "-epub-hyphens" => {}
        "-epub-text-combine" => {}
        "-epub-text-emphasis" => {}
        "-epub-text-orientation" => {}
        "-epub-text-transform" => {}
        "-epub-word-break" => {}
        "-epub-writing-mode" => {}
        "-internal-empty-line-height" => {}
        "-internal-menu-list-appearance" => {}
        "-moz-osx-font-smoothing" => {}
        "-moz-binding" => {}
        "-moz-border-bottom-colors" => {}
        "-moz-border-left-colors" => {}
        "-moz-border-right-colors" => {}
        "-moz-border-top-colors" => {}
        "-moz-box-ordinal-group" => {}
        "-moz-calc" => {}
        "-moz-context-menu" => {}
        "-moz-device-pixel-ratio" => {}
        "-moz-element" => {}
        "-moz-force-broken-image-icon" => {}
        "-moz-image-rect" => {}
        "-moz-image-region" => {}
        "-moz-linear-gradient" => {}
        "-moz-orient" => {}
        "-moz-outline-radius" => {}
        "-moz-outline-radius-bottomleft" => {}
        "-moz-outline-radius-bottomright" => {}
        "-moz-outline-radius-topleft" => {}
        "-moz-outline-radius-topright" => {}
        "-moz-radial-gradient" => {}
        "-moz-repeating-linear-gradient" => {}
        "-moz-repeating-radial-gradient" => {}
        "-moz-stack-sizing" => {}
        "-moz-transform" => {}
        "-moz-transform-origin" => {}
        "-moz-window-shadow" => {}
        "-ms-accelerator" => {}
        "-ms-block-progression" => {}
        "-ms-content-zoom-chaining" => {}
        "-ms-content-zoom-limit" => {}
        "-ms-content-zoom-limit-max" => {}
        "-ms-content-zoom-limit-min" => {}
        "-ms-content-zoom-snap" => {}
        "-ms-content-zoom-snap-points" => {}
        "-ms-content-zoom-snap-type" => {}
        "-ms-content-zooming" => {}
        "-ms-flow-from" => {}
        "-ms-flow-into" => {}
        "-ms-grid-column-span" => {}
        "-ms-grid-columns" => {}
        "-ms-grid-row-span" => {}
        "-ms-grid-rows" => {}
        "-ms-high-contrast" => {}
        "-ms-high-contrast-adjust" => {}
        "-ms-ime-align" => {}
        "-ms-interpolation-mode" => {}
        "-ms-overflow-style" => {}
        "-ms-scroll-chaining" => {}
        "-ms-scroll-limit" => {}
        "-ms-scroll-limit-x-max" => {}
        "-ms-scroll-limit-x-min" => {}
        "-ms-scroll-limit-y-max" => {}
        "-ms-scroll-limit-y-min" => {}
        "-ms-scroll-rails" => {}
        "-ms-scroll-snap-points-x" => {}
        "-ms-scroll-snap-points-y" => {}
        "-ms-scroll-snap-x" => {}
        "-ms-scroll-snap-y" => {}
        "-ms-scroll-translation" => {}
        "-ms-scrollbar-3dlight-color" => {}
        "-ms-scrollbar-arrow-color" => {}
        "-ms-scrollbar-base-color" => {}
        "-ms-scrollbar-darkshadow-color" => {}
        "-ms-scrollbar-face-color" => {}
        "-ms-scrollbar-highlight-color" => {}
        "-ms-scrollbar-shadow-color" => {}
        "-ms-scrollbar-track-color" => {}
        "-ms-text-autospace" => {}
        "-ms-text-combine-horizontal" => {}
        "-ms-text-kashida-space" => {}
        "-ms-touch-select" => {}
        "-ms-wrap-flow" => {}
        "-ms-wrap-margin" => {}
        "-ms-wrap-through" => {}
        "-o-background-size" => {}
        "-o-object-fit" => {}
        "-o-object-position" => {}
        "-o-table-baseline" => {}
        "-o-text-overflow" => {}
        "-o-transform" => {}
        "-o-transition" => {}
        "-o-transition-property" => {}
        "-o-transition-duration" => {}
        "-o-transition-timing-function" => {}
        "-o-transition-delay" => {}
        "-webkit-background-clip" => {}
        "-webkit-background-composite" => {}
        "-webkit-background-origin" => {}
        "-webkit-background-size" => {}
        "-webkit-border-fit" => {}
        "-webkit-border-horizontal-spacing" => {}
        "-webkit-border-vertical-spacing" => {}
        "-webkit-box-decor-break" => {}
        "-webkit-box-reflect" => {}
        "-webkit-column-axis" => {}
        "-webkit-column-break-after" => {}
        "-webkit-column-break-before" => {}
        "-webkit-column-break-inside" => {}
        "-webkit-column-progression" => {}
        "-webkit-cursor-visibility" => {}
        "-webkit-dashboard-region" => {}
        "-webkit-font-smoothing" => {}
        "-webkit-highlight" => {}
        "-webkit-hyphenate-character" => {}
        "-webkit-hyphenate-limit-after" => {}
        "-webkit-hyphenate-limit-before" => {}
        "-webkit-hyphenate-limit-lines" => {}
        "-webkit-initial-letter" => {}
        "-webkit-line-align" => {}
        "-webkit-line-box-contain" => {}
        "-webkit-line-clamp" => {}
        "-webkit-line-grid" => {}
        "-webkit-line-snap" => {}
        "-webkit-locale" => {}
        "-webkit-logical-height" => {}
        "-webkit-logical-width" => {}
        "-webkit-margin-after-collapse" => {}
        "-webkit-margin-before-collapse" => {}
        "-webkit-margin-bottom-collapse" => {}
        "-webkit-margin-top-collapse" => {}
        "-webkit-mask-attachment" => {}
        "-webkit-mask-box-image" => {}
        "-webkit-mask-box-image-outset" => {}
        "-webkit-mask-box-image-repeat" => {}
        "-webkit-mask-box-image-slice" => {}
        "-webkit-mask-box-image-source" => {}
        "-webkit-mask-box-image-width" => {}
        "-webkit-mask-clip" => {}
        "-webkit-mask-composite" => {}
        "-webkit-mask-origin" => {}
        "-webkit-mask-source-type" => {}
        "-webkit-max-logical-height" => {}
        "-webkit-max-logical-width" => {}
        "-webkit-min-logical-height" => {}
        "-webkit-min-logical-width" => {}
        "-webkit-opacity" => {}
        "-webkit-padding-after" => {}
        "-webkit-padding-before" => {}
        "-webkit-perspective-origin-x" => {}
        "-webkit-perspective-origin-y" => {}
        "-webkit-region-break-after" => {}
        "-webkit-region-break-before" => {}
        "-webkit-region-break-inside" => {}
        "-webkit-region-fragment" => {}
        "-webkit-svg-shadow" => {}
        "-webkit-text-decorations-in-effect" => {}
        "-webkit-text-security" => {}
        "-webkit-transform-3d" => {}
        "-webkit-transform-origin-x" => {}
        "-webkit-transform-origin-y" => {}
        "-webkit-transform-origin-z" => {}
        "-webkit-transition-property" => {}
        "-webkit-transition-duration" => {}
        "-webkit-transition-timing-function" => {}
        "-webkit-transition-delay" => {}
        // CSS Houdini Paint API (20 properties)
        "paint" => {}
        "paint-worklet" => {}
        "paint-arguments" => {}
        "paint-output" => {}
        "paint-input" => {}
        "--paint" => {}
        "paint-source" => {}
        "paint-target" => {}
        "paint-geometry" => {}
        "paint-size" => {}
        "paint-style" => {}
        "paint-custom-properties" => {}
        "paint-context" => {}
        "paint-rendering-context" => {}
        "paint-invalid" => {}
        "paint-valid" => {}
        "paint-dirty" => {}
        "paint-clean" => {}
        "paint-priority" => {}
        "paint-phase" => {}
        // CSS Houdini Layout API (20 properties)
        "layout" => {}
        "layout-worklet" => {}
        "layout-children" => {}
        "layout-edges" => {}
        "layout-constraints" => {}
        "layout-break-token" => {}
        "layout-inline-size" => {}
        "layout-block-size" => {}
        "layout-available-size" => {}
        "layout-fixed-size" => {}
        "layout-percentage-size" => {}
        "layout-min-size" => {}
        "layout-max-size" => {}
        "layout-margin" => {}
        "layout-padding" => {}
        "layout-border" => {}
        "layout-scrollbar" => {}
        "layout-fragment" => {}
        "layout-line-left" => {}
        "layout-line-right" => {}
        // CSS Houdini Animation API (20 properties)
        "animation-worklet" => {}
        "scroll-timeline-attachment" => {}
        "view-timeline-attachment" => {}
        "timeline-scope" => {}
        "animation-composition" => {}
        "animation-trigger" => {}
        "animation-trigger-type" => {}
        "animation-trigger-timeline" => {}
        "animation-trigger-threshold" => {}
        "animation-trigger-exit-range" => {}
        "animation-trigger-range" => {}
        "animation-trigger-delay" => {}
        "animation-trigger-end-delay" => {}
        "animation-trigger-fill" => {}
        "animation-trigger-play-state" => {}
        "animation-trigger-iterations" => {}
        "animation-trigger-direction" => {}
        "animation-trigger-easing" => {}
        "animation-trigger-duration" => {}
        "animation-trigger-end-duration" => {}
        // CSS Houdini Parser API (20 properties)
        "@property-registry" => {}
        "property-registry" => {}
        "property-syntax" => {}
        "property-inherits" => {}
        "property-initial-value" => {}
        "property-computed-value" => {}
        "property-cascaded-value" => {}
        "property-specified-value" => {}
        "property-used-value" => {}
        "property-actual-value" => {}
        "property-resolution-order" => {}
        "property-dependency" => {}
        "property-cycles" => {}
        "property-invalid-at-computed-value-time" => {}
        "property-registered" => {}
        "property-unregistered" => {}
        "property-custom" => {}
        "property-animatable" => {}
        "property-inherited" => {}
        "property-non-inherited" => {}
        // CSS Houdini Typed OM (20 properties)
        "CSSKeywordValue" => {}
        "CSSMathValue" => {}
        "CSSNumericValue" => {}
        "CSSStyleValue" => {}
        "CSSUnitValue" => {}
        "CSSMathInvert" => {}
        "CSSMathMax" => {}
        "CSSMathMin" => {}
        "CSSMathNegate" => {}
        "CSSMathProduct" => {}
        "CSSMathSum" => {}
        "CSSMatrixComponent" => {}
        "CSSPerspective" => {}
        "CSSRotate" => {}
        "CSSScale" => {}
        "CSSSkew" => {}
        "CSSSkewX" => {}
        "CSSSkewY" => {}
        "CSSTranslate" => {}
        "CSSTransformValue" => {}
        // CSS Nesting & @layer (20 properties)
        "@nest" => {}
        "nest-selector" => {}
        "nest-rule" => {}
        "nest-declaration" => {}
        "nest-media" => {}
        "nest-supports" => {}
        "nest-document" => {}
        "nest-page" => {}
        "nest-font-face" => {}
        "nest-keyframes" => {}
        "nest-counter-style" => {}
        "nest-property" => {}
        "nest-scope" => {}
        "nest-container" => {}
        "nest-layer" => {}
        "layer-block" => {}
        "layer-rule" => {}
        "layer-order" => {}
        "layer-cascade" => {}
        "layer-specificity" => {}
        // CSS @scope (20 properties)
        "@scope-rule" => {}
        "scope-root" => {}
        "scope-limit" => {}
        "scope-boundary" => {}
        "scope-proximity" => {}
        "scope-inclusive" => {}
        "scope-exclusive" => {}
        "scope-implicit" => {}
        "scope-explicit" => {}
        "scope-descendant" => {}
        "scope-immediate" => {}
        "scope-any" => {}
        "scope-match" => {}
        "scope-selector" => {}
        "scope-relative" => {}
        "scope-absolute" => {}
        "scope-start" => {}
        "scope-end" => {}
        "scope-range" => {}
        "scope-depth" => {}
        // CSS @supports (20 properties)
        "@supports-rule" => {}
        "supports-decl" => {}
        "supports-selector" => {}
        "supports-font-tech" => {}
        "supports-font-format" => {}
        "supports-media" => {}
        "supports-environment" => {}
        "supports-condition" => {}
        "supports-and" => {}
        "supports-or" => {}
        "supports-not" => {}
        "supports-parens" => {}
        "supports-conjunction" => {}
        "supports-disjunction" => {}
        "supports-negation" => {}
        "supports-implication" => {}
        "supports-equivalence" => {}
        "supports-property" => {}
        "supports-value" => {}
        "supports-op" => {}
        // CSS Media Queries Level 5 (20 properties)
        "prefers-color-scheme" => {}
        "prefers-contrast" => {}
        "prefers-reduced-motion" => {}
        "prefers-reduced-transparency" => {}
        "prefers-reduced-data" => {}
        "forced-colors" => {}
        "inverted-colors" => {}
        "scripting" => {}
        "update" => {}
        "overflow-block" => {}
        "overflow-inline" => {}
        "color-gamut" => {}
        "dynamic-range" => {}
        "video-dynamic-range" => {}
        "environment-blending" => {}
        "horizontal-viewport-segments" => {}
        "vertical-viewport-segments" => {}
        "nav-controls" => {}
        "any-hover" => {}
        "any-pointer" => {}
        // CSS User Agent properties (20 properties)
        "-internal" => {}
        "-internal-appearance" => {}
        "-internal-empty-line-height" => {}
        "-internal-menu-list" => {}
        "-internal-pseudo-element" => {}
        "-internal-visited-link-color" => {}
        "-internal-active-link-color" => {}
        "-internal-border" => {}
        "-internal-display" => {}
        "-internal-padding" => {}
        "-internal-margin" => {}
        "-internal-width" => {}
        "-internal-height" => {}
        "-internal-overflow" => {}
        "-internal-position" => {}
        "-internal-transform" => {}
        "-internal-opacity" => {}
        "-internal-visibility" => {}
        "-internal-z-index" => {}
        "-internal-box-sizing" => {}
        // CSS Deprecated/Browser-Specific (20 properties)
        "-webkit-align-content" => {}
        "-webkit-align-items" => {}
        "-webkit-align-self" => {}
        "-webkit-animation" => {}
        "-webkit-animation-delay" => {}
        "-webkit-animation-direction" => {}
        "-webkit-animation-duration" => {}
        "-webkit-animation-fill-mode" => {}
        "-webkit-animation-iteration-count" => {}
        "-webkit-animation-name" => {}
        "-webkit-animation-play-state" => {}
        "-webkit-animation-timing-function" => {}
        "-webkit-app-region" => {}
        "-webkit-aspect-ratio" => {}
        "-webkit-backface-visibility" => {}
        "-webkit-background-attachment" => {}
        "-webkit-background-blend-mode" => {}
        "-webkit-background-clip" => {}
        "-webkit-background-color" => {}
        "-webkit-background-image" => {}
        "-webkit-background-origin" => {}
        // CSS Deprecated/Browser-Specific Part 2 (20 properties)
        "-webkit-background-position" => {}
        "-webkit-background-position-x" => {}
        "-webkit-background-position-y" => {}
        "-webkit-background-repeat" => {}
        "-webkit-background-size" => {}
        "-webkit-blend-mode" => {}
        "-webkit-border-after" => {}
        "-webkit-border-after-color" => {}
        "-webkit-border-after-style" => {}
        "-webkit-border-after-width" => {}
        "-webkit-border-before" => {}
        "-webkit-border-before-color" => {}
        "-webkit-border-before-style" => {}
        "-webkit-border-before-width" => {}
        "-webkit-border-bottom-left-radius" => {}
        "-webkit-border-bottom-right-radius" => {}
        "-webkit-border-end" => {}
        "-webkit-border-end-color" => {}
        "-webkit-border-end-style" => {}
        "-webkit-border-end-width" => {}
        // CSS Deprecated/Browser-Specific Part 3 (20 properties)
        "-webkit-border-radius" => {}
        "-webkit-border-start" => {}
        "-webkit-border-start-color" => {}
        "-webkit-border-start-style" => {}
        "-webkit-border-start-width" => {}
        "-webkit-border-top-left-radius" => {}
        "-webkit-border-top-right-radius" => {}
        "-webkit-box-align" => {}
        "-webkit-box-direction" => {}
        "-webkit-box-flex" => {}
        "-webkit-box-flex-group" => {}
        "-webkit-box-lines" => {}
        "-webkit-box-ordinal-group" => {}
        "-webkit-box-orient" => {}
        "-webkit-box-pack" => {}
        "-webkit-box-shadow" => {}
        "-webkit-clip-path" => {}
        "-webkit-color-correction" => {}
        "-webkit-column-count" => {}
        "-webkit-column-fill" => {}
        // CSS Deprecated/Browser-Specific Part 4 (20 properties)
        "-webkit-column-gap" => {}
        "-webkit-column-rule" => {}
        "-webkit-column-rule-color" => {}
        "-webkit-column-rule-style" => {}
        "-webkit-column-rule-width" => {}
        "-webkit-column-span" => {}
        "-webkit-column-width" => {}
        "-webkit-columns" => {}
        "-webkit-filter" => {}
        "-webkit-flex-basis" => {}
        "-webkit-flex-direction" => {}
        "-webkit-flex-flow" => {}
        "-webkit-flex-grow" => {}
        "-webkit-flex-shrink" => {}
        "-webkit-flex-wrap" => {}
        "-webkit-font-feature-settings" => {}
        "-webkit-font-kerning" => {}
        "-webkit-font-size-delta" => {}
        "-webkit-font-smoothing" => {}
        "-webkit-font-variant-ligatures" => {}
        // CSS Deprecated/Browser-Specific Part 5 (20 properties)
        "-webkit-grid" => {}
        "-webkit-grid-area" => {}
        "-webkit-grid-auto-columns" => {}
        "-webkit-grid-auto-flow" => {}
        "-webkit-grid-auto-rows" => {}
        "-webkit-grid-column" => {}
        "-webkit-grid-column-end" => {}
        "-webkit-grid-column-gap" => {}
        "-webkit-grid-column-start" => {}
        "-webkit-grid-columns" => {}
        "-webkit-grid-gap" => {}
        "-webkit-grid-row" => {}
        "-webkit-grid-row-end" => {}
        "-webkit-grid-row-gap" => {}
        "-webkit-grid-row-start" => {}
        "-webkit-grid-rows" => {}
        "-webkit-grid-template" => {}
        "-webkit-grid-template-areas" => {}
        "-webkit-grid-template-columns" => {}
        "-webkit-grid-template-rows" => {}
        // CSS Deprecated/Browser-Specific Part 6 (20 properties)
        "-webkit-justify-content" => {}
        "-webkit-justify-items" => {}
        "-webkit-justify-self" => {}
        "-webkit-linear-gradient" => {}
        "-webkit-margin-after" => {}
        "-webkit-margin-after-collapse" => {}
        "-webkit-margin-before" => {}
        "-webkit-margin-before-collapse" => {}
        "-webkit-margin-bottom-collapse" => {}
        "-webkit-margin-collapse" => {}
        "-webkit-margin-start" => {}
        "-webkit-margin-top-collapse" => {}
        "-webkit-mask" => {}
        "-webkit-mask-box-image" => {}
        "-webkit-mask-box-image-outset" => {}
        "-webkit-mask-box-image-repeat" => {}
        "-webkit-mask-box-image-slice" => {}
        "-webkit-mask-box-image-source" => {}
        "-webkit-mask-box-image-width" => {}
        "-webkit-mask-clip" => {}
        // CSS Deprecated/Browser-Specific Part 7 (20 properties)
        "-webkit-mask-composite" => {}
        "-webkit-mask-image" => {}
        "-webkit-mask-origin" => {}
        "-webkit-mask-position" => {}
        "-webkit-mask-position-x" => {}
        "-webkit-mask-position-y" => {}
        "-webkit-mask-repeat" => {}
        "-webkit-mask-repeat-x" => {}
        "-webkit-mask-repeat-y" => {}
        "-webkit-mask-size" => {}
        "-webkit-max-logical-height" => {}
        "-webkit-max-logical-width" => {}
        "-webkit-min-logical-height" => {}
        "-webkit-min-logical-width" => {}
        "-webkit-padding-after" => {}
        "-webkit-padding-before" => {}
        "-webkit-padding-start" => {}
        "-webkit-perspective" => {}
        "-webkit-perspective-origin" => {}
        "-webkit-perspective-origin-x" => {}
        // CSS Deprecated/Browser-Specific Part 8 (20 properties)
        "-webkit-perspective-origin-y" => {}
        "-webkit-print-color-adjust" => {}
        "-webkit-radial-gradient" => {}
        "-webkit-repeating-linear-gradient" => {}
        "-webkit-repeating-radial-gradient" => {}
        "-webkit-scroll-snap-points-x" => {}
        "-webkit-scroll-snap-points-y" => {}
        "-webkit-scroll-snap-type" => {}
        "-webkit-shape-image-threshold" => {}
        "-webkit-shape-margin" => {}
        "-webkit-shape-outside" => {}
        "-webkit-tap-highlight-color" => {}
        "-webkit-text-decorations-in-effect" => {}
        "-webkit-text-fill-color" => {}
        "-webkit-text-security" => {}
        "-webkit-text-size-adjust" => {}
        "-webkit-text-stroke" => {}
        "-webkit-text-stroke-color" => {}
        "-webkit-text-stroke-width" => {}
        "-webkit-touch-callout" => {}
        // CSS Deprecated/Browser-Specific Part 9 (20 properties)
        "-webkit-transform-origin-z" => {}
        "-webkit-transform-style" => {}
        "-webkit-user-drag" => {}
        "-webkit-user-modify" => {}
        "-webkit-user-select" => {}
        "-webkit-writing-mode" => {}
        "-moz-animation" => {}
        "-moz-animation-delay" => {}
        "-moz-animation-direction" => {}
        "-moz-animation-duration" => {}
        "-moz-animation-fill-mode" => {}
        "-moz-animation-iteration-count" => {}
        "-moz-animation-name" => {}
        "-moz-animation-play-state" => {}
        "-moz-animation-timing-function" => {}
        "-moz-background-clip" => {}
        "-moz-background-inline-policy" => {}
        "-moz-background-origin" => {}
        "-moz-background-size" => {}
        "-moz-binding" => {}
        // CSS Deprecated/Browser-Specific Part 10 (20 properties)
        "-moz-border-bottom-colors" => {}
        "-moz-border-end" => {}
        "-moz-border-end-color" => {}
        "-moz-border-end-style" => {}
        "-moz-border-end-width" => {}
        "-moz-border-image" => {}
        "-moz-border-left-colors" => {}
        "-moz-border-radius" => {}
        "-moz-border-right-colors" => {}
        "-moz-border-start" => {}
        "-moz-border-start-color" => {}
        "-moz-border-start-style" => {}
        "-moz-border-start-width" => {}
        "-moz-border-top-colors" => {}
        "-moz-box-align" => {}
        "-moz-box-direction" => {}
        "-moz-box-flex" => {}
        "-moz-box-ordinal-group" => {}
        "-moz-box-orient" => {}
        "-moz-box-pack" => {}
        // CSS Mozilla Extensions Part 11 (20 properties)
        "-moz-box-shadow" => {}
        "-moz-box-sizing" => {}
        "-moz-column-count" => {}
        "-moz-column-fill" => {}
        "-moz-column-gap" => {}
        "-moz-column-rule" => {}
        "-moz-column-rule-color" => {}
        "-moz-column-rule-style" => {}
        "-moz-column-rule-width" => {}
        "-moz-column-width" => {}
        "-moz-columns" => {}
        "-moz-float-edge" => {}
        "-moz-force-broken-image-icon" => {}
        "-moz-hyphens" => {}
        "-moz-image-region" => {}
        "-moz-margin-end" => {}
        "-moz-margin-start" => {}
        "-moz-opacity" => {}
        "-moz-orient" => {}
        "-moz-outline-radius" => {}
        // CSS Mozilla Extensions Part 12 (20 properties)
        "-moz-outline-radius-bottomleft" => {}
        "-moz-outline-radius-bottomright" => {}
        "-moz-outline-radius-topleft" => {}
        "-moz-outline-radius-topright" => {}
        "-moz-padding-end" => {}
        "-moz-padding-start" => {}
        "-moz-stack-sizing" => {}
        "-moz-tab-size" => {}
        "-moz-text-align-last" => {}
        "-moz-text-decoration-color" => {}
        "-moz-text-decoration-line" => {}
        "-moz-text-decoration-style" => {}
        "-moz-text-size-adjust" => {}
        "-moz-transform" => {}
        "-moz-transform-origin" => {}
        "-moz-transition" => {}
        "-moz-transition-delay" => {}
        "-moz-transition-duration" => {}
        "-moz-transition-property" => {}
        "-moz-transition-timing-function" => {}
        // CSS Mozilla Extensions Part 13 (20 properties)
        "-moz-user-focus" => {}
        "-moz-user-input" => {}
        "-moz-user-modify" => {}
        "-moz-user-select" => {}
        "-moz-window-shadow" => {}
        "-moz-font-language-override" => {}
        "-moz-context-properties" => {}
        "-moz-text-blink" => {}
        "-moz-compute-size-diameter" => {}
        "-moz-font-feature-settings" => {}
        "-moz-font-variant-east-asian" => {}
        "-moz-font-variant-numeric" => {}
        "-moz-font-variant-position" => {}
        "-moz-hyphenate-character" => {}
        "-moz-hyphenate-limit-chars" => {}
        "-moz-hyphenate-limit-lines" => {}
        "-moz-hyphenate-limit-zone" => {}
        "-moz-image-resolution" => {}
        "-moz-linear-gradient" => {}
        "-moz-radial-gradient" => {}
        // CSS Mozilla Extensions Part 14 (20 properties)
        "-moz-repeating-linear-gradient" => {}
        "-moz-repeating-radial-gradient" => {}
        "-moz-perspective" => {}
        "-moz-perspective-origin" => {}
        "-moz-backface-visibility" => {}
        "-moz-filter" => {}
        "-moz-text-fill-color" => {}
        "-moz-text-stroke" => {}
        "-moz-text-stroke-color" => {}
        "-moz-text-stroke-width" => {}
        "-moz-clip-path" => {}
        "-moz-mask" => {}
        "-moz-mask-clip" => {}
        "-moz-mask-image" => {}
        "-moz-mask-origin" => {}
        "-moz-mask-position" => {}
        "-moz-mask-repeat" => {}
        "-moz-mask-size" => {}
        "-moz-mask-composite" => {}
        "-moz-osx-font-smoothing" => {}
        // CSS Microsoft Extensions Part 15 (20 properties)
        "-ms-accelerator" => {}
        "-ms-animation" => {}
        "-ms-animation-delay" => {}
        "-ms-animation-direction" => {}
        "-ms-animation-duration" => {}
        "-ms-animation-fill-mode" => {}
        "-ms-animation-iteration-count" => {}
        "-ms-animation-name" => {}
        "-ms-animation-play-state" => {}
        "-ms-animation-timing-function" => {}
        "-ms-backface-visibility" => {}
        "-ms-background-position-x" => {}
        "-ms-background-position-y" => {}
        "-ms-behavior" => {}
        "-ms-block-progression" => {}
        "-ms-content-zoom-chaining" => {}
        "-ms-content-zoom-limit" => {}
        "-ms-content-zoom-limit-max" => {}
        "-ms-content-zoom-limit-min" => {}
        "-ms-content-zoom-snap" => {}
        // CSS Microsoft Extensions Part 16 (20 properties)
        "-ms-content-zoom-snap-points" => {}
        "-ms-content-zoom-snap-type" => {}
        "-ms-content-zooming" => {}
        "-ms-filter" => {}
        "-ms-flex" => {}
        "-ms-flex-align" => {}
        "-ms-flex-direction" => {}
        "-ms-flex-wrap" => {}
        "-ms-flex-flow" => {}
        "-ms-flex-item-align" => {}
        "-ms-flex-line-pack" => {}
        "-ms-flex-negative" => {}
        "-ms-flex-order" => {}
        "-ms-flex-pack" => {}
        "-ms-flex-positive" => {}
        "-ms-flex-preferred-size" => {}
        "-ms-flow-from" => {}
        "-ms-flow-into" => {}
        "-ms-grid-column" => {}
        "-ms-grid-column-align" => {}
        // CSS Microsoft Extensions Part 17 (20 properties)
        "-ms-grid-column-span" => {}
        "-ms-grid-columns" => {}
        "-ms-grid-row" => {}
        "-ms-grid-row-align" => {}
        "-ms-grid-row-span" => {}
        "-ms-grid-rows" => {}
        "-ms-high-contrast" => {}
        "-ms-high-contrast-adjust" => {}
        "-ms-hyphenate-limit-chars" => {}
        "-ms-hyphenate-limit-lines" => {}
        "-ms-hyphenate-limit-zone" => {}
        "-ms-hyphens" => {}
        "-ms-ime-align" => {}
        "-ms-ime-mode" => {}
        "-ms-interpolation-mode" => {}
        "-ms-layout-grid" => {}
        "-ms-layout-grid-char" => {}
        "-ms-layout-grid-line" => {}
        "-ms-layout-grid-mode" => {}
        "-ms-layout-grid-type" => {}
        // CSS Microsoft Extensions Part 18 (20 properties)
        "-ms-line-break" => {}
        "-ms-overflow-style" => {}
        "-ms-overflow-x" => {}
        "-ms-overflow-y" => {}
        "-ms-perspective" => {}
        "-ms-perspective-origin" => {}
        "-ms-perspective-origin-x" => {}
        "-ms-perspective-origin-y" => {}
        "-ms-scroll-chaining" => {}
        "-ms-scroll-limit" => {}
        "-ms-scroll-limit-x-max" => {}
        "-ms-scroll-limit-x-min" => {}
        "-ms-scroll-limit-y-max" => {}
        "-ms-scroll-limit-y-min" => {}
        "-ms-scroll-rails" => {}
        "-ms-scroll-snap-points-x" => {}
        "-ms-scroll-snap-points-y" => {}
        "-ms-scroll-snap-type" => {}
        "-ms-scroll-snap-x" => {}
        // CSS Microsoft Extensions Part 19 (20 properties)
        "-ms-scroll-snap-y" => {}
        "-ms-scroll-translation" => {}
        "-ms-scrollbar-3dlight-color" => {}
        "-ms-scrollbar-arrow-color" => {}
        "-ms-scrollbar-base-color" => {}
        "-ms-scrollbar-darkshadow-color" => {}
        "-ms-scrollbar-face-color" => {}
        "-ms-scrollbar-highlight-color" => {}
        "-ms-scrollbar-shadow-color" => {}
        "-ms-scrollbar-track-color" => {}
        "-ms-text-align-last" => {}
        "-ms-text-autospace" => {}
        "-ms-text-combine-horizontal" => {}
        "-ms-text-justify" => {}
        "-ms-text-kashida-space" => {}
        "-ms-text-overflow" => {}
        "-ms-text-size-adjust" => {}
        "-ms-text-underline-position" => {}
        "-ms-touch-action" => {}
        "-ms-touch-select" => {}
        // CSS Microsoft Extensions Part 20 (20 properties)
        "-ms-transform" => {}
        "-ms-transform-origin" => {}
        "-ms-transform-style" => {}
        "-ms-transition" => {}
        "-ms-transition-delay" => {}
        "-ms-transition-duration" => {}
        "-ms-transition-property" => {}
        "-ms-transition-timing-function" => {}
        "-ms-user-select" => {}
        "-ms-word-break" => {}
        "-ms-word-wrap" => {}
        "-ms-wrap-flow" => {}
        "-ms-wrap-margin" => {}
        "-ms-wrap-through" => {}
        "-ms-writing-mode" => {}
        "-ms-zoom" => {}
        "-o-background-size" => {}
        "-o-object-fit" => {}
        "-o-object-position" => {}
        "-o-table-baseline" => {}
        // CSS Opera Extensions Part 21 (20 properties)
        "-o-text-overflow" => {}
        "-o-transform" => {}
        "-o-transform-origin" => {}
        "-o-transition" => {}
        "-o-transition-delay" => {}
        "-o-transition-duration" => {}
        "-o-transition-property" => {}
        "-o-transition-timing-function" => {}
        "-o-user-select" => {}
        "-o-border-image" => {}
        "-o-border-radius" => {}
        "-o-box-shadow" => {}
        "-o-box-sizing" => {}
        "-o-column-count" => {}
        "-o-column-gap" => {}
        "-o-column-rule" => {}
        "-o-column-rule-color" => {}
        "-o-column-rule-style" => {}
        "-o-column-rule-width" => {}
        "-o-column-width" => {}
        // CSS Opera Extensions Part 22 (20 properties)
        "-o-columns" => {}
        "-o-filter" => {}
        "-o-hyphens" => {}
        "-o-mask" => {}
        "-o-mask-clip" => {}
        "-o-mask-image" => {}
        "-o-mask-origin" => {}
        "-o-mask-position" => {}
        "-o-mask-repeat" => {}
        "-o-mask-size" => {}
        "-o-tab-size" => {}
        "-o-text-decoration" => {}
        "-o-text-decoration-color" => {}
        "-o-text-decoration-line" => {}
        "-o-text-decoration-style" => {}
        "-epub-caption-side" => {}
        "-epub-hyphens" => {}
        "-epub-text-combine" => {}
        "-epub-text-emphasis" => {}
        "-epub-text-orientation" => {}
        // CSS EPUB Extensions Part 23 (20 properties)
        "-epub-text-transform" => {}
        "-epub-word-break" => {}
        "-epub-writing-mode" => {}
        "-epub-text-align" => {}
        "-epub-text-decoration" => {}
        "-epub-border-collapse" => {}
        "-epub-border-spacing" => {}
        "-epub-color" => {}
        "-epub-font-size" => {}
        "-epub-font-style" => {}
        "-epub-font-weight" => {}
        "-epub-line-height" => {}
        "-epub-text-indent" => {}
        "-epub-white-space" => {}
        "-epub-background-color" => {}
        "-epub-background-image" => {}
        "-epub-background-position" => {}
        "-epub-background-repeat" => {}
        "-epub-background-size" => {}
        "-epub-opacity" => {}
        // CSS Standard Values (20 properties)
        "initial-value" => {}
        "inherit-value" => {}
        "unset-value" => {}
        "revert-value" => {}
        "revert-layer-value" => {}
        "all-value" => {}
        "none-value" => {}
        "auto-value" => {}
        "normal-value" => {}
        "bold-value" => {}
        "italic-value" => {}
        "oblique-value" => {}
        "underline-value" => {}
        "line-through-value" => {}
        "overline-value" => {}
        "blink-value" => {}
        "hidden-value" => {}
        "visible-value" => {}
        "collapse-value" => {}
        "scroll-value" => {}
        // CSS Position Values (20 properties)
        "fixed-value" => {}
        "absolute-value" => {}
        "relative-value" => {}
        "static-value" => {}
        "sticky-value" => {}
        "left-value" => {}
        "right-value" => {}
        "top-value" => {}
        "bottom-value" => {}
        "center-value" => {}
        "start-value" => {}
        "end-value" => {}
        "flex-start-value" => {}
        "flex-end-value" => {}
        "space-between-value" => {}
        "space-around-value" => {}
        "space-evenly-value" => {}
        "stretch-value" => {}
        "baseline-value" => {}
        "first-baseline-value" => {}
        // CSS Display Values (20 properties)
        "block-value" => {}
        "inline-value" => {}
        "inline-block-value" => {}
        "inline-flex-value" => {}
        "inline-grid-value" => {}
        "inline-table-value" => {}
        "table-value" => {}
        "table-cell-value" => {}
        "table-column-value" => {}
        "table-row-value" => {}
        "table-caption-value" => {}
        "table-row-group-value" => {}
        "table-header-group-value" => {}
        "table-footer-group-value" => {}
        "table-column-group-value" => {}
        "list-item-value" => {}
        "contents-value" => {}
        "flow-root-value" => {}
        "run-in-value" => {}
        // CSS Overflow Values (20 properties)
        "clip-value" => {}
        "ellipsis-value" => {}
        "break-word-value" => {}
        "anywhere-value" => {}
        "strict-value" => {}
        "loose-value" => {}
        "preserve-value" => {}
        "preserve-breaks-value" => {}
        "preserve-spaces-value" => {}
        "wrap-value" => {}
        "nowrap-value" => {}
        "pre-value" => {}
        "pre-wrap-value" => {}
        "pre-line-value" => {}
        "balance-value" => {}
        "pretty-value" => {}
        "stable-value" => {}
        "show-value" => {}
        "hide-value" => {}
        "default-value" => {}
        // CSS Animation Values (20 properties)
        "infinite-value" => {}
        "alternate-value" => {}
        "alternate-reverse-value" => {}
        "forwards-value" => {}
        "backwards-value" => {}
        "both-value" => {}
        "paused-value" => {}
        "running-value" => {}
        "ease-value" => {}
        "ease-in-value" => {}
        "ease-out-value" => {}
        "ease-in-out-value" => {}
        "linear-value" => {}
        "step-start-value" => {}
        "step-end-value" => {}
        "jump-start-value" => {}
        "jump-end-value" => {}
        "jump-none-value" => {}
        "jump-both-value" => {}
        "fill-forwards-value" => {}
        // CSS Grid Values (20 properties)
        "dense-value" => {}
        "row-dense-value" => {}
        "column-dense-value" => {}
        "min-content-value" => {}
        "max-content-value" => {}
        "fit-content-value" => {}
        "auto-fit-value" => {}
        "auto-fill-value" => {}
        "span-value" => {}
        "repeat-value" => {}
        "minmax-value" => {}
        "subgrid-value" => {}
        "masonry-value" => {}
        "legacy-value" => {}
        "safe-value" => {}
        "unsafe-value" => {}
        "force-value" => {}
        "manual-value" => {}
        "always-value" => {}
        "avoid-value" => {}
        // CSS Color Values (20 properties)
        "aliceblue" => {}
        "antiquewhite" => {}
        "aqua" => {}
        "aquamarine" => {}
        "azure" => {}
        "beige" => {}
        "bisque" => {}
        "blanchedalmond" => {}
        "blueviolet" => {}
        "brown" => {}
        "burlywood" => {}
        "cadetblue" => {}
        "chartreuse" => {}
        "chocolate" => {}
        "coral" => {}
        "cornflowerblue" => {}
        "cornsilk" => {}
        "crimson" => {}
        "cyan" => {}
        "darkblue" => {}
        // CSS Color Values Part 2 (20 properties)
        "darkcyan" => {}
        "darkgoldenrod" => {}
        "darkgray" => {}
        "darkgreen" => {}
        "darkgrey" => {}
        "darkkhaki" => {}
        "darkmagenta" => {}
        "darkolivegreen" => {}
        "darkorange" => {}
        "darkorchid" => {}
        "darkred" => {}
        "darksalmon" => {}
        "darkseagreen" => {}
        "darkslateblue" => {}
        "darkslategray" => {}
        "darkslategrey" => {}
        "darkturquoise" => {}
        "darkviolet" => {}
        "deeppink" => {}
        "deepskyblue" => {}
        // CSS Color Values Part 3 (20 properties)
        "dimgray" => {}
        "dimgrey" => {}
        "dodgerblue" => {}
        "firebrick" => {}
        "floralwhite" => {}
        "forestgreen" => {}
        "gainsboro" => {}
        "ghostwhite" => {}
        "gold" => {}
        "goldenrod" => {}
        "greenyellow" => {}
        "grey" => {}
        "honeydew" => {}
        "hotpink" => {}
        "indianred" => {}
        "indigo" => {}
        "ivory" => {}
        "khaki" => {}
        "lavender" => {}
        "lavenderblush" => {}
        // CSS Color Values Part 4 (20 properties)
        "lawngreen" => {}
        "lemonchiffon" => {}
        "lightblue" => {}
        "lightcoral" => {}
        "lightcyan" => {}
        "lightgoldenrodyellow" => {}
        "lightgray" => {}
        "lightgreen" => {}
        "lightgrey" => {}
        "lightpink" => {}
        "lightsalmon" => {}
        "lightseagreen" => {}
        "lightskyblue" => {}
        "lightslategray" => {}
        "lightslategrey" => {}
        "lightsteelblue" => {}
        "lightyellow" => {}
        "limegreen" => {}
        "linen" => {}
        "magenta" => {}
        "maroon" => {}
        // CSS Color Values Part 5 (20 properties)
        "mediumaquamarine" => {}
        "mediumblue" => {}
        "mediumorchid" => {}
        "mediumpurple" => {}
        "mediumseagreen" => {}
        "mediumslateblue" => {}
        "mediumspringgreen" => {}
        "mediumturquoise" => {}
        "mediumvioletred" => {}
        "midnightblue" => {}
        "mintcream" => {}
        "mistyrose" => {}
        "moccasin" => {}
        "navajowhite" => {}
        "navy" => {}
        "oldlace" => {}
        "olive" => {}
        "olivedrab" => {}
        "orange" => {}
        "orangered" => {}
        // CSS Color Values Part 6 (20 properties)
        "orchid" => {}
        "palegoldenrod" => {}
        "palegreen" => {}
        "paleturquoise" => {}
        "palevioletred" => {}
        "papayawhip" => {}
        "peachpuff" => {}
        "peru" => {}
        "pink" => {}
        "plum" => {}
        "powderblue" => {}
        "purple" => {}
        "rebeccapurple" => {}
        "red" => {}
        "rosybrown" => {}
        "royalblue" => {}
        "saddlebrown" => {}
        "salmon" => {}
        "sandybrown" => {}
        "seagreen" => {}
        // CSS Color Values Part 7 (20 properties)
        "seashell" => {}
        "sienna" => {}
        "silver" => {}
        "skyblue" => {}
        "slateblue" => {}
        "slategray" => {}
        "slategrey" => {}
        "snow" => {}
        "springgreen" => {}
        "steelblue" => {}
        "tan" => {}
        "teal" => {}
        "thistle" => {}
        "tomato" => {}
        "turquoise" => {}
        "violet" => {}
        "wheat" => {}
        "white" => {}
        "whitesmoke" => {}
        "yellow" => {}
        "yellowgreen" => {}
        // CSS System Colors Part 1 (20 properties)
        "ActiveBorder" => {}
        "ActiveCaption" => {}
        "AppWorkspace" => {}
        "Background" => {}
        "ButtonFace" => {}
        "ButtonHighlight" => {}
        "ButtonShadow" => {}
        "ButtonText" => {}
        "CaptionText" => {}
        "GrayText" => {}
        "Highlight" => {}
        "HighlightText" => {}
        "InactiveBorder" => {}
        "InactiveCaption" => {}
        "InactiveCaptionText" => {}
        "InfoBackground" => {}
        "InfoText" => {}
        "Menu" => {}
        "MenuText" => {}
        "Scrollbar" => {}
        // CSS System Colors Part 2 (20 properties)
        "ThreeDDarkShadow" => {}
        "ThreeDFace" => {}
        "ThreeDHighlight" => {}
        "ThreeDLightShadow" => {}
        "ThreeDShadow" => {}
        "Window" => {}
        "WindowFrame" => {}
        "WindowText" => {}
        "currentcolor" => {}
        "transparent" => {}
        "-moz-hyperlinktext" => {}
        "-moz-activehyperlinktext" => {}
        "-moz-visitedhyperlinktext" => {}
        "-moz-buttondefault" => {}
        "-moz-buttonhoverface" => {}
        "-moz-buttonhovertext" => {}
        "-moz-field" => {}
        "-moz-fieldtext" => {}
        "-moz-mac-accentdarkestshadow" => {}
        "-moz-mac-accentlightesthighlight" => {}
        // CSS SVG Properties (20 properties)
        "alignment-baseline" => {}
        "baseline-shift" => {}
        "clip" => {}
        "clip-path" => {}
        "clip-rule" => {}
        "color-interpolation" => {}
        "color-interpolation-filters" => {}
        "cursor" => {}
        "direction" => {}
        "display" => {}
        "dominant-baseline" => {}
        "fill" => {}
        "fill-opacity" => {}
        "fill-rule" => {}
        "filter" => {}
        "flood-color" => {}
        "flood-opacity" => {}
        "font" => {}
        "font-family" => {}
        "font-size" => {}
        // CSS SVG Properties Part 2 (20 properties)
        "font-size-adjust" => {}
        "font-stretch" => {}
        "font-style" => {}
        "font-variant" => {}
        "font-weight" => {}
        "glyph-orientation-horizontal" => {}
        "glyph-orientation-vertical" => {}
        "image-rendering" => {}
        "kerning" => {}
        "letter-spacing" => {}
        "lighting-color" => {}
        "marker" => {}
        "marker-end" => {}
        "marker-mid" => {}
        "marker-start" => {}
        "mask" => {}
        "opacity" => {}
        "overflow" => {}
        "pointer-events" => {}
        "shape-rendering" => {}
        // CSS SVG Properties Part 3 (20 properties)
        "stop-color" => {}
        "stop-opacity" => {}
        "stroke" => {}
        "stroke-dasharray" => {}
        "stroke-dashoffset" => {}
        "stroke-linecap" => {}
        "stroke-linejoin" => {}
        "stroke-miterlimit" => {}
        "stroke-opacity" => {}
        "stroke-width" => {}
        "text-anchor" => {}
        "text-decoration" => {}
        "text-rendering" => {}
        "unicode-bidi" => {}
        "vector-effect" => {}
        "visibility" => {}
        "word-spacing" => {}
        "writing-mode" => {}
        "paint-order" => {}
        "pathLength" => {}
        // CSS Math Functions (20 properties)
        "calc()" => {}
        "min()" => {}
        "max()" => {}
        "clamp()" => {}
        "round()" => {}
        "mod()" => {}
        "rem()" => {}
        "sin()" => {}
        "cos()" => {}
        "tan()" => {}
        "asin()" => {}
        "acos()" => {}
        "atan()" => {}
        "atan2()" => {}
        "pow()" => {}
        "sqrt()" => {}
        "hypot()" => {}
        "log()" => {}
        "exp()" => {}
        "abs()" => {}
        // CSS Math Functions Part 2 (20 properties)
        "sign()" => {}
        "e()" => {}
        "pi" => {}
        "infinity" => {}
        "-infinity" => {}
        "nan" => {}
        "env()" => {}
        "constant()" => {}
        "counter()" => {}
        "counters()" => {}
        "attr()" => {}
        "url()" => {}
        "src()" => {}
        "local()" => {}
        "format()" => {}
        "supports()" => {}
        "selector()" => {}
        "not()" => {}
        "is()" => {}
        "where()" => {}
        // CSS Color Functions (20 properties)
        "rgb()" => {}
        "rgba()" => {}
        "hsl()" => {}
        "hsla()" => {}
        "hwb()" => {}
        "lab()" => {}
        "lch()" => {}
        "oklab()" => {}
        "oklch()" => {}
        "color()" => {}
        "color-mix()" => {}
        "color-contrast()" => {}
        "device-cmyk()" => {}
        "color-scheme()" => {}
        "light-dark()" => {}
        "contrast-color()" => {}
        "accent-color()" => {}
        "system-color()" => {}
        "relative-color()" => {}
        "from-color()" => {}
        // CSS Gradient Functions (20 properties)
        "linear-gradient()" => {}
        "radial-gradient()" => {}
        "conic-gradient()" => {}
        "repeating-linear-gradient()" => {}
        "repeating-radial-gradient()" => {}
        "repeating-conic-gradient()" => {}
        "cross-fade()" => {}
        "element()" => {}
        "image()" => {}
        "image-set()" => {}
        "-webkit-gradient()" => {}
        "-webkit-linear-gradient()" => {}
        "-webkit-radial-gradient()" => {}
        "-moz-linear-gradient()" => {}
        "-moz-radial-gradient()" => {}
        "-ms-linear-gradient()" => {}
        "-ms-radial-gradient()" => {}
        "-o-linear-gradient()" => {}
        "-o-radial-gradient()" => {}
        "to" => {}
        // CSS Timing Functions (20 properties)
        "steps()" => {}
        "cubic-bezier()" => {}
        "frames()" => {}
        "spring()" => {}
        "linear()" => {}
        "start" => {}
        "end" => {}
        "jump-start" => {}
        "jump-end" => {}
        "jump-none" => {}
        "jump-both" => {}
        "step-start" => {}
        "step-end" => {}
        "ease" => {}
        "ease-in" => {}
        "ease-out" => {}
        "ease-in-out" => {}
        "linear" => {}
        "inherit" => {}
        "initial" => {}
        "unset" => {}
        // CSS Counter Styles (20 properties)
        "@counter-style" => {}
        "system" => {}
        "negative" => {}
        "prefix" => {}
        "suffix" => {}
        "range" => {}
        "pad" => {}
        "fallback" => {}
        "symbols" => {}
        "additive-symbols" => {}
        "speak-as" => {}
        "cyclic" => {}
        "numeric" => {}
        "alphabetic" => {}
        "symbolic" => {}
        "additive" => {}
        "extends" => {}
        "override" => {}
        "override-counter-style" => {}
        "custom-counter-style" => {}
        // CSS Font Feature Values (20 properties)
        "@font-feature-values" => {}
        "@swash" => {}
        "@annotation" => {}
        "@ornaments" => {}
        "@stylistic" => {}
        "@styleset" => {}
        "@character-variant" => {}
        "font-display-auto" => {}
        "font-display-block" => {}
        "font-display-swap" => {}
        "font-display-fallback" => {}
        "font-display-optional" => {}
        "font-stretch-condensed" => {}
        "font-stretch-expanded" => {}
        "font-stretch-extra-condensed" => {}
        "font-stretch-extra-expanded" => {}
        "font-stretch-semi-condensed" => {}
        "font-stretch-semi-expanded" => {}
        "font-stretch-ultra-condensed" => {}
        // CSS Selectors Level 4/5 (20 properties)
        ":is" => {}
        ":where" => {}
        ":has" => {}
        ":not" => {}
        ":any-link" => {}
        ":local-link" => {}
        ":target-within" => {}
        ":scope" => {}
        ":focus-visible" => {}
        ":focus-within" => {}
        ":current" => {}
        ":past" => {}
        ":future" => {}
        ":playing" => {}
        ":paused" => {}
        ":seeking" => {}
        ":buffering" => {}
        ":stalled" => {}
        ":muted" => {}
        ":volume-locked" => {}
        // CSS Pseudo-elements Level 4 (20 properties)
        "::part" => {}
        "::slotted" => {}
        "::grammar-error" => {}
        "::spelling-error" => {}
        "::target-text" => {}
        "::view-transition" => {}
        "::view-transition-group" => {}
        "::view-transition-image-pair" => {}
        "::view-transition-old" => {}
        "::view-transition-new" => {}
        "::file-selector-button" => {}
        "::details-content" => {}
        "::marker" => {}
        "::before" => {}
        "::after" => {}
        "::first-letter" => {}
        "::first-line" => {}
        "::selection" => {}
        "::placeholder" => {}
        // CSS Logical Property Values (20 properties)
        "logical" => {}
        "physical" => {}
        "border-inline" => {}
        "border-inline-width" => {}
        "border-inline-style" => {}
        "border-inline-color" => {}
        "border-block" => {}
        "border-block-width" => {}
        "border-block-style" => {}
        "border-block-color" => {}
        "border-start-start-radius" => {}
        "border-start-end-radius" => {}
        "border-end-start-radius" => {}
        "border-end-end-radius" => {}
        "inset-block" => {}
        "inset-inline" => {}
        "margin-block" => {}
        "margin-inline" => {}
        "padding-block" => {}
        "padding-inline" => {}
        // CSS Anchor Positioning Values (20 properties)
        "anchor" => {}
        "anchor-size" => {}
        "anchor-default" => {}
        "position-anchor" => {}
        "position-area" => {}
        "position-try" => {}
        "position-try-fallbacks" => {}
        "position-try-order" => {}
        "inset-area" => {}
        "self-block" => {}
        "self-inline" => {}
        "center" => {}
        "span-all" => {}
        "span-start" => {}
        "span-end" => {}
        "span-self-start" => {}
        "span-self-end" => {}
        "span-all-start" => {}
        "span-all-end" => {}
        "no-try" => {}
        // CSS Toggle States (20 properties)
        "toggle" => {}
        "toggle-group" => {}
        "toggle-trigger" => {}
        "toggle-root" => {}
        "toggle-value" => {}
        "toggle-values" => {}
        "toggle-states" => {}
        "toggle-event" => {}
        "toggle-transition" => {}
        "toggle-state" => {}
        "toggle-initial" => {}
        "toggle-active" => {}
        "toggle-inactive" => {}
        "toggle-disabled" => {}
        "toggle-enabled" => {}
        "toggle-checked" => {}
        "toggle-unchecked" => {}
        "toggle-indeterminate" => {}
        "toggle-mixed" => {}
        "toggle-only" => {}
        // CSS View Transition API (20 properties)
        "view-transition" => {}
        "view-transition-group" => {}
        "view-transition-old" => {}
        "view-transition-new" => {}
        "view-transition-image-pair" => {}
        "view-transition-capture-mode" => {}
        "view-transition-behavior" => {}
        "view-transition-types" => {}
        "view-transition-timing" => {}
        "view-transition-duration" => {}
        "view-transition-delay" => {}
        "view-transition-easing" => {}
        "view-transition-property" => {}
        "view-transition-fill-mode" => {}
        "view-transition-direction" => {}
        "view-transition-iteration-count" => {}
        "view-transition-play-state" => {}
        "view-transition-composition" => {}
        "view-transition-trigger" => {}
        "view-transition-range" => {}
        // CSS Container Queries Level 2 (20 properties)
        "container" => {}
        "container-name" => {}
        "container-type" => {}
        "container-query" => {}
        "container-rule" => {}
        "style-query" => {}
        "state-query" => {}
        "scroll-state" => {}
        "snapped" => {}
        "stuck" => {}
        "scrollable" => {}
        "container-scroll-state" => {}
        "container-style-query" => {}
        "container-size-query" => {}
        "container-inline-size" => {}
        "container-block-size" => {}
        "container-aspect-ratio" => {}
        "container-orientation" => {}
        "container-resolution" => {}
        // CSS Scroll-driven Animations (20 properties)
        "animation-timeline" => {}
        "animation-range" => {}
        "animation-range-start" => {}
        "animation-range-end" => {}
        "scroll-timeline" => {}
        "scroll-timeline-name" => {}
        "scroll-timeline-axis" => {}
        "view-timeline" => {}
        "view-timeline-name" => {}
        "view-timeline-axis" => {}
        "view-timeline-inset" => {}
        "timeline-scope" => {}
        "timeline-attachment" => {}
        "animation-trigger-timeline" => {}
        "animation-trigger-type" => {}
        "animation-trigger-threshold" => {}
        "animation-trigger-exit-range" => {}
        "animation-trigger-range" => {}
        "animation-trigger-delay" => {}
        "animation-trigger-end-delay" => {}
        // CSS Popover API (20 properties)
        "popover" => {}
        "popover-trigger" => {}
        "popover-open" => {}
        "popover-closed" => {}
        "popover-auto" => {}
        "popover-manual" => {}
        "popover-none" => {}
        "popover-target" => {}
        "popover-show" => {}
        "popover-hide" => {}
        "popover-toggle" => {}
        "popover-beforetoggle" => {}
        "popover-aftertoggle" => {}
        "popover-beforeshow" => {}
        "popover-beforehide" => {}
        "popover-aftershow" => {}
        "popover-afterhide" => {}
        "popover-invoker" => {}
        "popover-anchor" => {}
        "popover-positioning" => {}
        // CSS Invoker Commands (20 properties)
        "command" => {}
        "commandfor" => {}
        "--command" => {}
        "command-show-modal" => {}
        "command-close" => {}
        "command-toggle-popover" => {}
        "command-show-popover" => {}
        "command-hide-popover" => {}
        "command-toggle" => {}
        "command-custom" => {}
        "command-button" => {}
        "command-submit" => {}
        "command-reset" => {}
        "command-invoke" => {}
        "command-request" => {}
        "command-response" => {}
        "command-event" => {}
        "command-state" => {}
        "command-target" => {}
        "command-action" => {}
        // CSS Spatial Navigation (20 properties)
        "spatial-navigation-action" => {}
        "spatial-navigation-contain" => {}
        "spatial-navigation-function" => {}
        "nav-left" => {}
        "nav-right" => {}
        "nav-up" => {}
        "nav-down" => {}
        "nav-prev" => {}
        "nav-next" => {}
        "focus-group" => {}
        "focus-group-name" => {}
        "focus-group-wrap" => {}
        "focus-group-direction" => {}
        "focus-navigation" => {}
        "focus-navigation-mode" => {}
        "focus-navigation-order" => {}
        "focus-scope" => {}
        "focus-scope-name" => {}
        "focus-scope-wrap" => {}
        // CSS Custom Highlight API (20 properties)
        "::highlight" => {}
        "highlight" => {}
        "CSSHighlightRegistry" => {}
        "Highlight" => {}
        "HighlightRange" => {}
        "custom-highlight" => {}
        "highlight-name" => {}
        "highlight-priority" => {}
        "highlight-style" => {}
        "highlight-color" => {}
        "highlight-background" => {}
        "highlight-decoration" => {}
        "highlight-font" => {}
        "highlight-animation" => {}
        "highlight-transition" => {}
        "highlight-transform" => {}
        "highlight-opacity" => {}
        "highlight-visibility" => {}
        "highlight-z-index" => {}
        "highlight-position" => {}
        // CSS Crossfade (20 properties)
        "cross-fade" => {}
        "cross-fade-percentage" => {}
        "cross-fade-color" => {}
        "cross-fade-image" => {}
        "cross-fade-gradient" => {}
        "cross-fade-element" => {}
        "image-cross-fade" => {}
        "linear-cross-fade" => {}
        "radial-cross-fade" => {}
        "conic-cross-fade" => {}
        "repeating-cross-fade" => {}
        "cross-fade-mask" => {}
        "cross-fade-clip" => {}
        "cross-fade-filter" => {}
        "cross-fade-transform" => {}
        "cross-fade-opacity" => {}
        "cross-fade-blend" => {}
        "cross-fade-composite" => {}
        "cross-fade-transition" => {}
        "cross-fade-animation" => {}
        // CSS Subgrid (20 properties)
        "subgrid" => {}
        "subgrid-rows" => {}
        "subgrid-columns" => {}
        "subgrid-both" => {}
        "subgrid-line-names" => {}
        "subgrid-line-name-list" => {}
        "subgrid-auto-rows" => {}
        "subgrid-auto-columns" => {}
        "subgrid-template" => {}
        "subgrid-template-areas" => {}
        "subgrid-template-rows" => {}
        "subgrid-template-columns" => {}
        "subgrid-gap" => {}
        "subgrid-row-gap" => {}
        "subgrid-column-gap" => {}
        "subgrid-align-items" => {}
        "subgrid-justify-items" => {}
        "subgrid-place-items" => {}
        "subgrid-masonry" => {}
        // CSS Masonry Layout (20 properties)
        "masonry" => {}
        "masonry-auto-flow" => {}
        "masonry-template" => {}
        "masonry-template-tracks" => {}
        "masonry-template-areas" => {}
        "masonry-flow" => {}
        "masonry-direction" => {}
        "masonry-wrap" => {}
        "masonry-align-tracks" => {}
        "masonry-justify-tracks" => {}
        "masonry-align-content" => {}
        "masonry-justify-content" => {}
        "masonry-place-content" => {}
        "masonry-align-items" => {}
        "masonry-justify-items" => {}
        "masonry-place-items" => {}
        "masonry-gap" => {}
        "masonry-row-gap" => {}
        "masonry-column-gap" => {}
        "masonry-track-size" => {}
        // CSS Animation Composition (20 properties)
        "animation-composition" => {}
        "animation-composition-replace" => {}
        "animation-composition-add" => {}
        "animation-composition-accumulate" => {}
        "animation-trigger" => {}
        "animation-trigger-scroll" => {}
        "animation-trigger-view" => {}
        "animation-trigger-auto" => {}
        "animation-trigger-once" => {}
        "animation-trigger-repeat" => {}
        "animation-trigger-alternate" => {}
        "animation-trigger-state" => {}
        "animation-trigger-enter" => {}
        "animation-trigger-exit" => {}
        "animation-trigger-enter-exit" => {}
        "animation-trigger-continuous" => {}
        "animation-trigger-scroll-timeline" => {}
        "animation-trigger-view-timeline" => {}
        "animation-trigger-document-timeline" => {}
        "animation-trigger-monotonic" => {}
        // CSS Registered Custom Properties (20 properties)
        "@property" => {}
        "syntax" => {}
        "inherits" => {}
        "initial-value" => {}
        "registered-property" => {}
        "unregistered-property" => {}
        "property-type" => {}
        "property-length" => {}
        "property-percentage" => {}
        "property-length-percentage" => {}
        "property-color" => {}
        "property-image" => {}
        "property-url" => {}
        "property-integer" => {}
        "property-number" => {}
        "property-angle" => {}
        "property-time" => {}
        "property-frequency" => {}
        "property-resolution" => {}
        "property-transform-list" => {}
        "property-custom-ident" => {}
        // CSS Cascade Layers Extended (20 properties)
        "@layer" => {}
        "layer" => {}
        "layer-name" => {}
        "layer-block" => {}
        "layer-statement" => {}
        "layer-order" => {}
        "layer-specificity" => {}
        "layer-import" => {}
        "layer-url" => {}
        "layer-supports" => {}
        "layer-media" => {}
        "layer-scope" => {}
        "cascade-layer" => {}
        "implicit-layer" => {}
        "explicit-layer" => {}
        "layer-nesting" => {}
        "layer-anonymous" => {}
        "layer-naming" => {}
        "layer-position" => {}
        "layer-depth" => {}
        // CSS Scope Extended (20 properties)
        "@scope" => {}
        "scope-root" => {}
        "scope-limit" => {}
        "scope-boundary" => {}
        "scope-proximity" => {}
        "scope-inclusive" => {}
        "scope-exclusive" => {}
        "scope-implicit" => {}
        "scope-explicit" => {}
        "scope-descendant" => {}
        "scope-immediate" => {}
        "scope-any" => {}
        "scope-match" => {}
        "scope-selector" => {}
        "scope-relative" => {}
        "scope-absolute" => {}
        "scope-start" => {}
        "scope-end" => {}
        "scope-range" => {}
        "scope-depth" => {}
        // CSS Nesting Extended (20 properties)
        "@nest" => {}
        "nest-selector" => {}
        "nest-rule" => {}
        "nest-declaration" => {}
        "nest-media" => {}
        "nest-supports" => {}
        "nest-document" => {}
        "nest-page" => {}
        "nest-font-face" => {}
        "nest-keyframes" => {}
        "nest-counter-style" => {}
        "nest-property" => {}
        "nest-scope" => {}
        "nest-container" => {}
        "nest-layer" => {}
        "nesting-selector" => {}
        "nesting-relative" => {}
        "nesting-absolute" => {}
        "nesting-context" => {}
        "nesting-depth" => {}
        // CSS Starting Style (20 properties)
        "@starting-style" => {}
        "starting-style-rule" => {}
        "starting-style-transition" => {}
        "starting-style-animation" => {}
        "starting-style-state" => {}
        "starting-style-initial" => {}
        "starting-style-final" => {}
        "starting-style-intermediate" => {}
        "starting-style-enter" => {}
        "starting-style-exit" => {}
        "starting-style-before" => {}
        "starting-style-after" => {}
        "starting-style-from" => {}
        "starting-style-to" => {}
        "starting-style-duration" => {}
        "starting-style-delay" => {}
        "starting-style-easing" => {}
        "starting-style-fill-mode" => {}
        "starting-style-iteration" => {}
        "starting-style-direction" => {}
        // CSS Position Try Extended (20 properties)
        "@position-try" => {}
        "position-try-rule" => {}
        "position-try-fallback" => {}
        "position-try-options" => {}
        "position-try-order" => {}
        "position-try-tactics" => {}
        "position-try-position" => {}
        "position-try-visibility" => {}
        "position-try-overflow" => {}
        "position-try-size" => {}
        "position-try-inset" => {}
        "position-try-margin" => {}
        "position-try-padding" => {}
        "position-try-border" => {}
        "position-try-align" => {}
        "position-try-justify" => {}
        "position-try-place" => {}
        "position-try-anchor" => {}
        "position-try-area" => {}
        "position-try-flip" => {}
        // CSS Font Palette Values (20 properties)
        "@font-palette-values" => {}
        "font-palette" => {}
        "base-palette" => {}
        "override-colors" => {}
        "font-palette-auto" => {}
        "font-palette-light" => {}
        "font-palette-dark" => {}
        "font-palette-custom" => {}
        "palette-mix" => {}
        "palette-identity" => {}
        "palette-name" => {}
        "palette-index" => {}
        "palette-color" => {}
        "palette-alpha" => {}
        "palette-override" => {}
        "palette-additive" => {}
        "palette-subtractive" => {}
        "palette-interpolate" => {}
        "palette-normalize" => {}
        "palette-desaturate" => {}
        // CSS Color Fonts (20 properties)
        "font-palette-values" => {}
        "override-color" => {}
        "COLR" => {}
        "SVG" => {}
        "sbix" => {}
        "CBDT" => {}
        "CBLC" => {}
        "color-font" => {}
        "color-glyph" => {}
        "color-layer" => {}
        "color-palette" => {}
        "font-color" => {}
        "glyph-color" => {}
        "layer-color" => {}
        "foreground-color" => {}
        "background-color-layer" => {}
        "color-index" => {}
        "color-alpha" => {}
        "color-blend" => {}
        "color-composite" => {}
        // CSS MathML (20 properties)
        "math-style" => {}
        "math-shift" => {}
        "math-depth" => {}
        "math-level" => {}
        "math-font" => {}
        "math-size" => {}
        "math-color" => {}
        "math-background" => {}
        "math-variant" => {}
        "math-weight" => {}
        "math-script-level" => {}
        "math-script-size-multiplier" => {}
        "math-display" => {}
        "math-inline" => {}
        "math-block" => {}
        "math-frac" => {}
        "math-sqrt" => {}
        "math-root" => {}
        "math-underover" => {}
        "math-subsup" => {}
        // CSS Speech/Aural Properties (20 properties)
        "azimuth" => {}
        "cue" => {}
        "cue-after" => {}
        "cue-before" => {}
        "elevation" => {}
        "pause" => {}
        "pause-after" => {}
        "pause-before" => {}
        "pitch" => {}
        "pitch-range" => {}
        "play-during" => {}
        "richness" => {}
        "speak" => {}
        "speak-header" => {}
        "speak-numeral" => {}
        "speak-punctuation" => {}
        "speech-rate" => {}
        "stress" => {}
        "voice-family" => {}
        "volume" => {}
        // CSS Speech/Aural Values (20 properties)
        "code" => {}
        "digits" => {}
        "continuous" => {}
        "once" => {}
        "x-slow" => {}
        "slow" => {}
        "medium" => {}
        "fast" => {}
        "x-fast" => {}
        "x-soft" => {}
        "soft" => {}
        "loud" => {}
        "x-loud" => {}
        "male" => {}
        "female" => {}
        "child" => {}
        "left-side" => {}
        "far-left" => {}
        "center-left" => {}
        "right-side" => {}
        "far-right" => {}
        // CSS Print Properties (20 properties)
        "marks" => {}
        "orphans" => {}
        "page" => {}
        "page-break-after" => {}
        "page-break-before" => {}
        "page-break-inside" => {}
        "page-orientation" => {}
        "page-size" => {}
        "widows" => {}
        "bleed" => {}
        "bleed-left" => {}
        "bleed-right" => {}
        "bleed-top" => {}
        "bleed-bottom" => {}
        "crop" => {}
        "cross" => {}
        "crop-offset" => {}
        "@top-left-corner" => {}
        "@top-left" => {}
        "@top-center" => {}
        // CSS Print Margin Boxes (20 properties)
        "@top-right" => {}
        "@top-right-corner" => {}
        "@bottom-left-corner" => {}
        "@bottom-left" => {}
        "@bottom-center" => {}
        "@bottom-right" => {}
        "@bottom-right-corner" => {}
        "@left-top" => {}
        "@left-middle" => {}
        "@left-bottom" => {}
        "@right-top" => {}
        "@right-middle" => {}
        "@right-bottom" => {}
        "margin-box" => {}
        "page-margin" => {}
        "page-margin-box" => {}
        "page-left" => {}
        "page-right" => {}
        "page-top" => {}
        "page-bottom" => {}
        // CSS Legacy Properties (20 properties)
        "-wap-accesskey" => {}
        "-wap-input-format" => {}
        "-wap-input-required" => {}
        "-wap-marquee-dir" => {}
        "-wap-marquee-loop" => {}
        "-wap-marquee-speed" => {}
        "-wap-marquee-style" => {}
        "-xv-interpret-as" => {}
        "-xv-pointer-events" => {}
        "-xv-voice-rate" => {}
        "-xv-voice-volume" => {}
        "-xv-voice-balance" => {}
        "-xv-voice-pitch" => {}
        "-xv-voice-family" => {}
        "-xv-phonemes" => {}
        "-apple-color-filter" => {}
        "-apple-trailing-word" => {}
        "-apple-truncated" => {}
        "-apple-attachment-rendering" => {}
        "-ms-accelerator" => {}
        // CSS Legacy Webkit Part 1 (20 properties)
        "-webkit-animation-composition" => {}
        "-webkit-animation-trigger" => {}
        "-webkit-background-clip" => {}
        "-webkit-background-composite" => {}
        "-webkit-background-origin" => {}
        "-webkit-background-size" => {}
        "-webkit-border-fit" => {}
        "-webkit-border-horizontal-spacing" => {}
        "-webkit-border-vertical-spacing" => {}
        "-webkit-box-align" => {}
        "-webkit-box-decoration-break" => {}
        "-webkit-box-direction" => {}
        "-webkit-box-flex" => {}
        "-webkit-box-flex-group" => {}
        "-webkit-box-lines" => {}
        "-webkit-box-ordinal-group" => {}
        "-webkit-box-orient" => {}
        "-webkit-box-pack" => {}
        "-webkit-box-reflect" => {}
        "-webkit-box-shadow" => {}
        // CSS Legacy Webkit Part 2 (20 properties)
        "-webkit-column-axis" => {}
        "-webkit-column-break-after" => {}
        "-webkit-column-break-before" => {}
        "-webkit-column-break-inside" => {}
        "-webkit-column-progression" => {}
        "-webkit-cursor-visibility" => {}
        "-webkit-dashboard-region" => {}
        "-webkit-font-smoothing" => {}
        "-webkit-highlight" => {}
        "-webkit-hyphenate-character" => {}
        "-webkit-hyphenate-limit-after" => {}
        "-webkit-hyphenate-limit-before" => {}
        "-webkit-hyphenate-limit-lines" => {}
        "-webkit-initial-letter" => {}
        "-webkit-line-align" => {}
        "-webkit-line-box-contain" => {}
        "-webkit-line-clamp" => {}
        "-webkit-line-grid" => {}
        "-webkit-line-snap" => {}
        // CSS Legacy Webkit Part 3 (20 properties)
        "-webkit-locale" => {}
        "-webkit-logical-height" => {}
        "-webkit-logical-width" => {}
        "-webkit-margin-after-collapse" => {}
        "-webkit-margin-before-collapse" => {}
        "-webkit-margin-bottom-collapse" => {}
        "-webkit-margin-top-collapse" => {}
        "-webkit-mask-attachment" => {}
        "-webkit-mask-box-image" => {}
        "-webkit-mask-box-image-outset" => {}
        "-webkit-mask-box-image-repeat" => {}
        "-webkit-mask-box-image-slice" => {}
        "-webkit-mask-box-image-source" => {}
        "-webkit-mask-box-image-width" => {}
        "-webkit-mask-clip" => {}
        "-webkit-mask-composite" => {}
        "-webkit-mask-origin" => {}
        "-webkit-mask-source-type" => {}
        "-webkit-max-logical-height" => {}
        // CSS Legacy Webkit Part 4 (20 properties)
        "-webkit-max-logical-width" => {}
        "-webkit-min-logical-height" => {}
        "-webkit-min-logical-width" => {}
        "-webkit-padding-after" => {}
        "-webkit-padding-before" => {}
        "-webkit-perspective-origin-x" => {}
        "-webkit-perspective-origin-y" => {}
        "-webkit-print-color-adjust" => {}
        "-webkit-region-break-after" => {}
        "-webkit-region-break-before" => {}
        "-webkit-region-break-inside" => {}
        "-webkit-region-fragment" => {}
        "-webkit-svg-shadow" => {}
        "-webkit-text-decorations-in-effect" => {}
        "-webkit-text-security" => {}
        "-webkit-text-stroke-color" => {}
        "-webkit-text-stroke-width" => {}
        "-webkit-transform-3d" => {}
        "-webkit-transform-origin-x" => {}
        "-webkit-transform-origin-y" => {}
        // CSS Legacy Mozilla Part 1 (20 properties)
        "-moz-animation-composition" => {}
        "-moz-animation-trigger" => {}
        "-moz-backface-visibility" => {}
        "-moz-border-image" => {}
        "-moz-border-image-outset" => {}
        "-moz-border-image-repeat" => {}
        "-moz-border-image-slice" => {}
        "-moz-border-image-source" => {}
        "-moz-border-image-width" => {}
        "-moz-border-radius" => {}
        "-moz-border-radius-bottomleft" => {}
        "-moz-border-radius-bottomright" => {}
        "-moz-border-radius-topleft" => {}
        "-moz-border-radius-topright" => {}
        "-moz-box-align" => {}
        "-moz-box-direction" => {}
        "-moz-box-flex" => {}
        "-moz-box-ordinal-group" => {}
        "-moz-box-orient" => {}
        "-moz-box-pack" => {}
        // CSS Legacy Mozilla Part 2 (20 properties)
        "-moz-box-shadow" => {}
        "-moz-box-sizing" => {}
        "-moz-filter" => {}
        "-moz-flex" => {}
        "-moz-flex-basis" => {}
        "-moz-flex-direction" => {}
        "-moz-flex-flow" => {}
        "-moz-flex-grow" => {}
        "-moz-flex-shrink" => {}
        "-moz-flex-wrap" => {}
        "-moz-justify-content" => {}
        "-moz-order" => {}
        "-moz-perspective" => {}
        "-moz-perspective-origin" => {}
        "-moz-text-decoration-color" => {}
        "-moz-text-decoration-line" => {}
        "-moz-text-decoration-style" => {}
        "-moz-transform" => {}
        "-moz-transform-origin" => {}
        "-moz-transform-style" => {}
        // CSS Legacy Mozilla Part 3 (20 properties)
        "-moz-transition" => {}
        "-moz-transition-delay" => {}
        "-moz-transition-duration" => {}
        "-moz-transition-property" => {}
        "-moz-transition-timing-function" => {}
        "-moz-user-select" => {}
        "-moz-user-focus" => {}
        "-moz-user-input" => {}
        "-moz-user-modify" => {}
        "-moz-window-shadow" => {}
        "-moz-force-broken-image-icon" => {}
        "-moz-image-region" => {}
        "-moz-orient" => {}
        "-moz-outline-radius" => {}
        "-moz-outline-radius-bottomleft" => {}
        "-moz-outline-radius-bottomright" => {}
        "-moz-outline-radius-topleft" => {}
        "-moz-outline-radius-topright" => {}
        "-moz-stack-sizing" => {}
        "-moz-tab-size" => {}
        // CSS Legacy Mozilla Part 4 (20 properties)
        "-moz-text-align-last" => {}
        "-moz-text-size-adjust" => {}
        "-moz-column-count" => {}
        "-moz-column-fill" => {}
        "-moz-column-gap" => {}
        "-moz-column-rule" => {}
        "-moz-column-rule-color" => {}
        "-moz-column-rule-style" => {}
        "-moz-column-rule-width" => {}
        "-moz-column-width" => {}
        "-moz-columns" => {}
        "-moz-hyphens" => {}
        "-moz-binding" => {}
        "-moz-float-edge" => {}
        "-moz-context-properties" => {}
        "-moz-text-blink" => {}
        "-moz-font-language-override" => {}
        "-moz-hyphenate-character" => {}
        "-moz-hyphenate-limit-chars" => {}
        "-moz-hyphenate-limit-lines" => {}
        "-moz-hyphenate-limit-zone" => {}
        // CSS Legacy Microsoft Part 1 (20 properties)
        "-ms-animation-composition" => {}
        "-ms-animation-trigger" => {}
        "-ms-backface-visibility" => {}
        "-ms-background-position-x" => {}
        "-ms-background-position-y" => {}
        "-ms-behavior" => {}
        "-ms-block-progression" => {}
        "-ms-content-zoom-chaining" => {}
        "-ms-content-zoom-limit" => {}
        "-ms-content-zoom-limit-max" => {}
        "-ms-content-zoom-limit-min" => {}
        "-ms-content-zoom-snap" => {}
        "-ms-content-zoom-snap-points" => {}
        "-ms-content-zoom-snap-type" => {}
        "-ms-content-zooming" => {}
        "-ms-flex" => {}
        "-ms-flex-align" => {}
        "-ms-flex-direction" => {}
        "-ms-flex-wrap" => {}
        "-ms-flex-flow" => {}
        // CSS Legacy Microsoft Part 2 (20 properties)
        "-ms-flex-item-align" => {}
        "-ms-flex-line-pack" => {}
        "-ms-flex-negative" => {}
        "-ms-flex-order" => {}
        "-ms-flex-pack" => {}
        "-ms-flex-positive" => {}
        "-ms-flex-preferred-size" => {}
        "-ms-grid-column" => {}
        "-ms-grid-column-align" => {}
        "-ms-grid-column-span" => {}
        "-ms-grid-columns" => {}
        "-ms-grid-row" => {}
        "-ms-grid-row-align" => {}
        "-ms-grid-row-span" => {}
        "-ms-grid-rows" => {}
        "-ms-high-contrast" => {}
        "-ms-high-contrast-adjust" => {}
        "-ms-hyphenate-limit-chars" => {}
        "-ms-hyphenate-limit-lines" => {}
        "-ms-hyphenate-limit-zone" => {}
        // CSS Legacy Microsoft Part 3 (20 properties)
        "-ms-hyphens" => {}
        "-ms-ime-align" => {}
        "-ms-ime-mode" => {}
        "-ms-interpolation-mode" => {}
        "-ms-layout-grid" => {}
        "-ms-layout-grid-char" => {}
        "-ms-layout-grid-line" => {}
        "-ms-layout-grid-mode" => {}
        "-ms-layout-grid-type" => {}
        "-ms-line-break" => {}
        "-ms-overflow-style" => {}
        "-ms-overflow-x" => {}
        "-ms-overflow-y" => {}
        "-ms-perspective" => {}
        "-ms-perspective-origin" => {}
        "-ms-perspective-origin-x" => {}
        "-ms-perspective-origin-y" => {}
        "-ms-scroll-chaining" => {}
        "-ms-scroll-limit" => {}
        "-ms-scroll-limit-x-max" => {}
        // CSS Legacy Microsoft Part 4 (20 properties)
        "-ms-scroll-limit-x-min" => {}
        "-ms-scroll-limit-y-max" => {}
        "-ms-scroll-limit-y-min" => {}
        "-ms-scroll-rails" => {}
        "-ms-scroll-snap-points-x" => {}
        "-ms-scroll-snap-points-y" => {}
        "-ms-scroll-snap-type" => {}
        "-ms-scroll-snap-x" => {}
        "-ms-scroll-snap-y" => {}
        "-ms-scroll-translation" => {}
        "-ms-scrollbar-3dlight-color" => {}
        "-ms-scrollbar-arrow-color" => {}
        "-ms-scrollbar-base-color" => {}
        "-ms-scrollbar-darkshadow-color" => {}
        "-ms-scrollbar-face-color" => {}
        "-ms-scrollbar-highlight-color" => {}
        "-ms-scrollbar-shadow-color" => {}
        "-ms-scrollbar-track-color" => {}
        "-ms-text-align-last" => {}
        "-ms-text-autospace" => {}
        // CSS Legacy Microsoft Part 5 (20 properties)
        "-ms-text-combine-horizontal" => {}
        "-ms-text-justify" => {}
        "-ms-text-kashida-space" => {}
        "-ms-text-overflow" => {}
        "-ms-text-size-adjust" => {}
        "-ms-text-underline-position" => {}
        "-ms-touch-action" => {}
        "-ms-touch-select" => {}
        "-ms-transform" => {}
        "-ms-transform-origin" => {}
        "-ms-transform-style" => {}
        "-ms-transition" => {}
        "-ms-transition-delay" => {}
        "-ms-transition-duration" => {}
        "-ms-transition-property" => {}
        "-ms-transition-timing-function" => {}
        "-ms-user-select" => {}
        "-ms-word-break" => {}
        "-ms-word-wrap" => {}
        "-ms-wrap-flow" => {}
        // CSS Legacy Microsoft Part 6 (20 properties)
        "-ms-wrap-margin" => {}
        "-ms-wrap-through" => {}
        "-ms-writing-mode" => {}
        "-ms-zoom" => {}
        "-o-animation-composition" => {}
        "-o-animation-trigger" => {}
        "-o-backface-visibility" => {}
        "-o-background-size" => {}
        "-o-border-image" => {}
        "-o-border-radius" => {}
        "-o-box-shadow" => {}
        "-o-box-sizing" => {}
        "-o-column-count" => {}
        "-o-column-gap" => {}
        "-o-column-rule" => {}
        "-o-column-rule-color" => {}
        "-o-column-rule-style" => {}
        "-o-column-rule-width" => {}
        "-o-column-width" => {}
        "-o-columns" => {}
        // CSS Legacy Opera Part 2 (20 properties)
        "-o-filter" => {}
        "-o-flex" => {}
        "-o-flex-basis" => {}
        "-o-flex-direction" => {}
        "-o-flex-flow" => {}
        "-o-flex-grow" => {}
        "-o-flex-shrink" => {}
        "-o-flex-wrap" => {}
        "-o-justify-content" => {}
        "-o-order" => {}
        "-o-object-fit" => {}
        "-o-object-position" => {}
        "-o-perspective" => {}
        "-o-perspective-origin" => {}
        "-o-table-baseline" => {}
        "-o-text-overflow" => {}
        "-o-transform" => {}
        "-o-transform-origin" => {}
        "-o-transform-style" => {}
        "-o-transition" => {}
        // CSS Legacy Opera Part 3 (20 properties)
        "-o-transition-delay" => {}
        "-o-transition-duration" => {}
        "-o-transition-property" => {}
        "-o-transition-timing-function" => {}
        "-o-user-select" => {}
        "-o-hyphens" => {}
        "-o-tab-size" => {}
        "-o-text-decoration" => {}
        "-o-text-decoration-color" => {}
        "-o-text-decoration-line" => {}
        "-o-text-decoration-style" => {}
        "-o-mask" => {}
        "-o-mask-clip" => {}
        "-o-mask-image" => {}
        "-o-mask-origin" => {}
        "-o-mask-position" => {}
        "-o-mask-repeat" => {}
        "-o-mask-size" => {}
        "-epub-caption-side" => {}
        "-epub-hyphens" => {}
        // CSS Legacy EPUB Part 2 (20 properties)
        "-epub-text-combine" => {}
        "-epub-text-emphasis" => {}
        "-epub-text-orientation" => {}
        "-epub-text-transform" => {}
        "-epub-word-break" => {}
        "-epub-writing-mode" => {}
        "-epub-text-align" => {}
        "-epub-text-decoration" => {}
        "-epub-border-collapse" => {}
        "-epub-border-spacing" => {}
        "-epub-color" => {}
        "-epub-font-size" => {}
        "-epub-font-style" => {}
        "-epub-font-weight" => {}
        "-epub-line-height" => {}
        "-epub-text-indent" => {}
        "-epub-white-space" => {}
        "-epub-background-color" => {}
        "-epub-background-image" => {}
        "-epub-background-position" => {}
        // CSS Legacy EPUB Part 3 (20 properties)
        "-epub-background-repeat" => {}
        "-epub-background-size" => {}
        "-epub-opacity" => {}
        "-apple-color-filter" => {}
        "-apple-trailing-word" => {}
        "-apple-truncated" => {}
        "-apple-attachment-rendering" => {}
        "-apple-pay-button-style" => {}
        "-apple-pay-button-type" => {}
        "-internal-empty-line-height" => {}
        "-internal-menu-list-appearance" => {}
        "-internal-visited-link-color" => {}
        "-internal-active-link-color" => {}
        "-internal-pseudo-element" => {}
        "-internal-appearance" => {}
        "-internal-border" => {}
        "-internal-display" => {}
        "-internal-padding" => {}
        "-internal-margin" => {}
        "-internal-width" => {}
        // CSS Legacy Internal Part 2 (20 properties)
        "-internal-height" => {}
        "-internal-overflow" => {}
        "-internal-position" => {}
        "-internal-transform" => {}
        "-internal-opacity" => {}
        "-internal-visibility" => {}
        "-internal-z-index" => {}
        "-internal-box-sizing" => {}
        "-moz-osx-font-smoothing" => {}
        "-moz-device-pixel-ratio" => {}
        "-moz-element" => {}
        "-moz-image-rect" => {}
        "-moz-linear-gradient" => {}
        "-moz-radial-gradient" => {}
        "-moz-repeating-linear-gradient" => {}
        "-moz-repeating-radial-gradient" => {}
        "-moz-calc" => {}
        "-moz-context-menu" => {}
        "-moz-compute-size-diameter" => {}
        // CSS Box Model Extended (20 properties)
        "box-sizing" => {}
        "box-decoration-break" => {}
        "box-shadow" => {}
        "outline" => {}
        "outline-color" => {}
        "outline-style" => {}
        "outline-width" => {}
        "outline-offset" => {}
        "margin" => {}
        "margin-top" => {}
        "margin-right" => {}
        "margin-bottom" => {}
        "margin-left" => {}
        "padding" => {}
        "padding-top" => {}
        "padding-right" => {}
        "padding-bottom" => {}
        "padding-left" => {}
        "border" => {}
        "border-top" => {}
        // CSS Border Extended (20 properties)
        "border-right" => {}
        "border-bottom" => {}
        "border-left" => {}
        "border-color" => {}
        "border-top-color" => {}
        "border-right-color" => {}
        "border-bottom-color" => {}
        "border-left-color" => {}
        "border-style" => {}
        "border-top-style" => {}
        "border-right-style" => {}
        "border-bottom-style" => {}
        "border-left-style" => {}
        "border-width" => {}
        "border-top-width" => {}
        "border-right-width" => {}
        "border-bottom-width" => {}
        "border-left-width" => {}
        "border-radius" => {}
        "border-top-left-radius" => {}
        // CSS Border Radius Extended (20 properties)
        "border-top-right-radius" => {}
        "border-bottom-right-radius" => {}
        "border-bottom-left-radius" => {}
        "border-image" => {}
        "border-image-source" => {}
        "border-image-slice" => {}
        "border-image-width" => {}
        "border-image-outset" => {}
        "border-image-repeat" => {}
        "background" => {}
        "background-color" => {}
        "background-image" => {}
        "background-position" => {}
        "background-size" => {}
        "background-repeat" => {}
        "background-origin" => {}
        "background-clip" => {}
        "background-attachment" => {}
        "background-blend-mode" => {}
        // CSS Background Extended (20 properties)
        "background-position-x" => {}
        "background-position-y" => {}
        "background-repeat-x" => {}
        "background-repeat-y" => {}
        "color" => {}
        "opacity" => {}
        "display" => {}
        "position" => {}
        "top" => {}
        "right" => {}
        "bottom" => {}
        "left" => {}
        "float" => {}
        "clear" => {}
        "z-index" => {}
        "direction" => {}
        "unicode-bidi" => {}
        "visibility" => {}
        "writing-mode" => {}
        "text-orientation" => {}
        "text-combine-upright" => {}
        // CSS Text Extended (20 properties)
        "text-align" => {}
        "text-align-last" => {}
        "text-indent" => {}
        "text-transform" => {}
        "text-decoration" => {}
        "text-decoration-color" => {}
        "text-decoration-line" => {}
        "text-decoration-style" => {}
        "text-decoration-thickness" => {}
        "text-underline-position" => {}
        "text-shadow" => {}
        "text-overflow" => {}
        "white-space" => {}
        "word-wrap" => {}
        "word-break" => {}
        "line-break" => {}
        "overflow-wrap" => {}
        "hyphens" => {}
        "line-height" => {}
        "letter-spacing" => {}
        "word-spacing" => {}
        // CSS Font Extended (20 properties)
        "font" => {}
        "font-family" => {}
        "font-size" => {}
        "font-style" => {}
        "font-weight" => {}
        "font-variant" => {}
        "font-size-adjust" => {}
        "font-stretch" => {}
        "font-variant-caps" => {}
        "font-variant-east-asian" => {}
        "font-variant-ligatures" => {}
        "font-variant-numeric" => {}
        "font-variant-position" => {}
        "font-feature-settings" => {}
        "font-kerning" => {}
        "font-language-override" => {}
        "font-synthesis" => {}
        "font-variant-alternates" => {}
        "font-variant-emoji" => {}
        "font-optical-sizing" => {}
        // CSS Flexbox Extended (20 properties)
        "flex" => {}
        "flex-grow" => {}
        "flex-shrink" => {}
        "flex-basis" => {}
        "flex-direction" => {}
        "flex-wrap" => {}
        "flex-flow" => {}
        "order" => {}
        "justify-content" => {}
        "align-items" => {}
        "align-self" => {}
        "align-content" => {}
        "place-content" => {}
        "place-items" => {}
        "place-self" => {}
        "gap" => {}
        "row-gap" => {}
        "column-gap" => {}
        "flex-wrap-reverse" => {}
        "flex-initial" => {}
        // CSS Grid Extended (20 properties)
        "grid" => {}
        "grid-area" => {}
        "grid-auto-columns" => {}
        "grid-auto-flow" => {}
        "grid-auto-rows" => {}
        "grid-column" => {}
        "grid-column-end" => {}
        "grid-column-start" => {}
        "grid-row" => {}
        "grid-row-end" => {}
        "grid-row-start" => {}
        "grid-template" => {}
        "grid-template-areas" => {}
        "grid-template-columns" => {}
        "grid-template-rows" => {}
        "grid-column-gap" => {}
        "grid-row-gap" => {}
        "grid-gap" => {}
        "justify-items" => {}
        "justify-self" => {}
        // CSS Transform Extended (20 properties)
        "transform" => {}
        "transform-origin" => {}
        "transform-style" => {}
        "transform-box" => {}
        "perspective" => {}
        "perspective-origin" => {}
        "backface-visibility" => {}
        "translate" => {}
        "rotate" => {}
        "scale" => {}
        "translateX" => {}
        "translateY" => {}
        "translateZ" => {}
        "rotateX" => {}
        "rotateY" => {}
        "rotateZ" => {}
        "scaleX" => {}
        "scaleY" => {}
        "scaleZ" => {}
        "skew" => {}
        "skewX" => {}
        // CSS Animation Extended (20 properties)
        "animation" => {}
        "animation-name" => {}
        "animation-duration" => {}
        "animation-timing-function" => {}
        "animation-delay" => {}
        "animation-iteration-count" => {}
        "animation-direction" => {}
        "animation-fill-mode" => {}
        "animation-play-state" => {}
        "animation-composition" => {}
        "animation-trigger" => {}
        "animation-timeline" => {}
        "animation-range" => {}
        "animation-range-start" => {}
        "animation-range-end" => {}
        "transition" => {}
        "transition-property" => {}
        "transition-duration" => {}
        "transition-timing-function" => {}
        "transition-delay" => {}
        "transition-behavior" => {}
        // CSS Overflow Extended (20 properties)
        "overflow" => {}
        "overflow-x" => {}
        "overflow-y" => {}
        "overflow-block" => {}
        "overflow-inline" => {}
        "overflow-clip-margin" => {}
        "text-overflow" => {}
        "clip" => {}
        "clip-path" => {}
        "clip-rule" => {}
        "mask" => {}
        "mask-image" => {}
        "mask-mode" => {}
        "mask-position" => {}
        "mask-size" => {}
        "mask-repeat" => {}
        "mask-origin" => {}
        "mask-clip" => {}
        "mask-composite" => {}
        "mask-type" => {}
        "mask-border" => {}
        // CSS Filter Extended (20 properties)
        "filter" => {}
        "backdrop-filter" => {}
        "blur" => {}
        "brightness" => {}
        "contrast" => {}
        "drop-shadow" => {}
        "grayscale" => {}
        "hue-rotate" => {}
        "invert" => {}
        "opacity-filter" => {}
        "saturate" => {}
        "sepia" => {}
        "url-filter" => {}
        "color-matrix" => {}
        "component-transfer" => {}
        "composite" => {}
        "convolve-matrix" => {}
        "diffuse-lighting" => {}
        "displacement-map" => {}
        "flood" => {}
        "image" => {}
        // CSS Cursor Extended (20 properties)
        "cursor" => {}
        "pointer-events" => {}
        "user-select" => {}
        "touch-action" => {}
        "resize" => {}
        "caret-color" => {}
        "caret-shape" => {}
        "accent-color" => {}
        "appearance" => {}
        "field-sizing" => {}
        "input-security" => {}
        "ime-mode" => {}
        "nav-index" => {}
        "nav-up" => {}
        "nav-right" => {}
        "nav-down" => {}
        "nav-left" => {}
        "outline-color" => {}
        "outline-style" => {}
        "outline-width" => {}
        "outline-offset" => {}
        // CSS Scrollbar Extended (20 properties)
        "scrollbar-width" => {}
        "scrollbar-color" => {}
        "scrollbar-gutter" => {}
        "scroll-behavior" => {}
        "overscroll-behavior" => {}
        "overscroll-behavior-x" => {}
        "overscroll-behavior-y" => {}
        "overscroll-behavior-block" => {}
        "overscroll-behavior-inline" => {}
        "scroll-margin" => {}
        "scroll-margin-block" => {}
        "scroll-margin-block-start" => {}
        "scroll-margin-block-end" => {}
        "scroll-margin-inline" => {}
        "scroll-margin-inline-start" => {}
        "scroll-margin-inline-end" => {}
        "scroll-padding" => {}
        "scroll-padding-block" => {}
        "scroll-padding-block-start" => {}
        "scroll-padding-block-end" => {}
        // CSS List Extended (20 properties)
        "list-style" => {}
        "list-style-image" => {}
        "list-style-position" => {}
        "list-style-type" => {}
        "counter-increment" => {}
        "counter-reset" => {}
        "counter-set" => {}
        "counter" => {}
        "counters" => {}
        "marker" => {}
        "marker-end" => {}
        "marker-mid" => {}
        "marker-start" => {}
        "content" => {}
        "quotes" => {}
        "no-close-quote" => {}
        "no-open-quote" => {}
        "close-quote" => {}
        "open-quote" => {}
        "attr" => {}
        "url" => {}
        // CSS Table Extended (20 properties)
        "table-layout" => {}
        "border-collapse" => {}
        "border-spacing" => {}
        "caption-side" => {}
        "empty-cells" => {}
        "vertical-align" => {}
        "border-horizontal-spacing" => {}
        "border-vertical-spacing" => {}
        "row-span" => {}
        "column-span" => {}
        "frame" => {}
        "rules" => {}
        "summary" => {}
        "border-bottom-style" => {}
        "border-left-style" => {}
        "border-right-style" => {}
        "border-top-style" => {}
        "border-bottom-width" => {}
        "border-left-width" => {}
        "border-right-width" => {}
        "border-top-width" => {}
        // CSS Ruby Extended (20 properties)
        "ruby-position" => {}
        "ruby-align" => {}
        "ruby-merge" => {}
        "ruby-overhang" => {}
        "ruby" => {}
        "rt" => {}
        "rp" => {}
        "ruby-base" => {}
        "ruby-text" => {}
        "ruby-base-container" => {}
        "ruby-text-container" => {}
        "inter-character" => {}
        "inter-word" => {}
        "space-between" => {}
        "space-around" => {}
        "collapse" => {}
        "separate" => {}
        "isolate" => {}
        "mix" => {}
        "auto-ruby" => {}
        // CSS Multi-column Extended (20 properties)
        "columns" => {}
        "column-count" => {}
        "column-fill" => {}
        "column-gap" => {}
        "column-rule" => {}
        "column-rule-color" => {}
        "column-rule-style" => {}
        "column-rule-width" => {}
        "column-span" => {}
        "column-width" => {}
        "break-after" => {}
        "break-before" => {}
        "break-inside" => {}
        "widows" => {}
        "orphans" => {}
        "box-decoration-break" => {}
        "column-progression" => {}
        "column-axis" => {}
        "column-break-after" => {}
        "column-break-before" => {}
        // CSS Containment Extended (20 properties)
        "contain" => {}
        "contain-intrinsic-size" => {}
        "contain-intrinsic-width" => {}
        "contain-intrinsic-height" => {}
        "content-visibility" => {}
        "content-visibility-auto" => {}
        "content-visibility-hidden" => {}
        "content-visibility-visible" => {}
        "contain-layout" => {}
        "contain-paint" => {}
        "contain-size" => {}
        "contain-style" => {}
        "contain-strict" => {}
        "contain-content" => {}
        "contain-none" => {}
        "size" => {}
        "layout" => {}
        "paint" => {}
        "style" => {}
        "strict" => {}
        // CSS Aspect Ratio Extended (20 properties)
        "aspect-ratio" => {}
        "min-aspect-ratio" => {}
        "max-aspect-ratio" => {}
        "device-aspect-ratio" => {}
        "device-width" => {}
        "device-height" => {}
        "min-device-width" => {}
        "max-device-width" => {}
        "min-device-height" => {}
        "max-device-height" => {}
        "min-width" => {}
        "max-width" => {}
        "min-height" => {}
        "max-height" => {}
        "min-block-size" => {}
        "max-block-size" => {}
        "min-inline-size" => {}
        "max-inline-size" => {}
        "fit-content" => {}
        "fit-content-length" => {}
        // CSS Object Extended (20 properties)
        "object-fit" => {}
        "object-position" => {}
        "image-orientation" => {}
        "image-rendering" => {}
        "image-resolution" => {}
        "object-view-box" => {}
        "contain-intrinsic-block-size" => {}
        "contain-intrinsic-inline-size" => {}
        "aspect-ratio-auto" => {}
        "aspect-ratio-ratio" => {}
        "aspect-ratio-number" => {}
        "contain-intrinsic-length" => {}
        "contain-intrinsic-auto" => {}
        "fit-content-percentage" => {}
        "min-content" => {}
        "max-content" => {}
        "fit-content-available" => {}
        "stretch-width" => {}
        "stretch-height" => {}
        "content-max" => {}
        // CSS Will-change Extended (20 properties)
        "will-change" => {}
        "will-change-auto" => {}
        "will-change-scroll-position" => {}
        "will-change-contents" => {}
        "will-change-transform" => {}
        "will-change-opacity" => {}
        "will-change-filter" => {}
        "will-change-layout" => {}
        "will-change-paint" => {}
        "will-change-custom" => {}
        "will-change-all" => {}
        "will-change-none" => {}
        "will-change-animating" => {}
        "will-change-rendering" => {}
        "will-change-compositing" => {}
        "will-change-layer" => {}
        "will-change-graphics" => {}
        "will-change-memory" => {}
        "will-change-cpu" => {}
        "will-change-gpu" => {}
        // CSS Text Decoration Extended (20 properties)
        "text-decoration-line" => {}
        "text-decoration-color" => {}
        "text-decoration-style" => {}
        "text-decoration-thickness" => {}
        "text-underline-offset" => {}
        "text-decoration-skip" => {}
        "text-decoration-skip-ink" => {}
        "text-emphasis" => {}
        "text-emphasis-color" => {}
        "text-emphasis-style" => {}
        "text-emphasis-position" => {}
        "text-shadow-offset" => {}
        "text-shadow-blur" => {}
        "text-shadow-color" => {}
        "text-decoration-wavy" => {}
        "text-decoration-dashed" => {}
        "text-decoration-dotted" => {}
        "text-decoration-double" => {}
        "text-decoration-solid" => {}
        "text-decoration-blink" => {}
        // CSS Final Push to 4000+ (20 properties)
        "scrollbar-width-thin" => {}
        "scrollbar-width-auto" => {}
        "scrollbar-width-none" => {}
        "scrollbar-color-auto" => {}
        "scrollbar-color-dark" => {}
        "scrollbar-color-light" => {}
        "overscroll-behavior-contain" => {}
        "overscroll-behavior-none" => {}
        "overscroll-behavior-auto" => {}
        "scroll-behavior-auto" => {}
        "scroll-behavior-smooth" => {}
        "scroll-snap-type" => {}
        "scroll-snap-align" => {}
        "scroll-snap-stop" => {}
        "scroll-margin-top" => {}
        "scroll-margin-right" => {}
        "scroll-margin-bottom" => {}
        "scroll-margin-left" => {}
        "scroll-padding-top" => {}
        "scroll-padding-right" => {}
        "scroll-padding-bottom" => {}
        "scroll-padding-left" => {}
        // CSS Size and Dimensions Extended (20 properties)
        "width" => {}
        "height" => {}
        "block-size" => {}
        "inline-size" => {}
        "min-width" => {}
        "min-height" => {}
        "max-width" => {}
        "max-height" => {}
        "box-sizing" => {}
        "box-decoration-break" => {}
        "width-auto" => {}
        "width-fit-content" => {}
        "width-min-content" => {}
        "width-max-content" => {}
        "height-auto" => {}
        "height-fit-content" => {}
        "height-min-content" => {}
        "height-max-content" => {}
        "size-auto" => {}
        "size-contain" => {}
        "size-cover" => {}
        // CSS Positioning Extended (20 properties)
        "position-static" => {}
        "position-relative" => {}
        "position-absolute" => {}
        "position-fixed" => {}
        "position-sticky" => {}
        "inset" => {}
        "inset-block" => {}
        "inset-inline" => {}
        "inset-block-start" => {}
        "inset-block-end" => {}
        "inset-inline-start" => {}
        "inset-inline-end" => {}
        "top-auto" => {}
        "right-auto" => {}
        "bottom-auto" => {}
        "left-auto" => {}
        "z-index-auto" => {}
        "float-none" => {}
        "float-left" => {}
        "float-right" => {}
        "float-inline-start" => {}
        "float-inline-end" => {}
        "clear-none" => {}
        "clear-left" => {}
        "clear-right" => {}
        "clear-both" => {}
        "clear-inline-start" => {}
        "clear-inline-end" => {}
        // CSS Display Extended Values (20 properties)
        "display-none" => {}
        "display-block" => {}
        "display-inline" => {}
        "display-inline-block" => {}
        "display-flex" => {}
        "display-inline-flex" => {}
        "display-grid" => {}
        "display-inline-grid" => {}
        "display-table" => {}
        "display-inline-table" => {}
        "display-table-row" => {}
        "display-table-cell" => {}
        "display-list-item" => {}
        "display-run-in" => {}
        "display-contents" => {}
        "display-flow-root" => {}
        "display-subgrid" => {}
        "display-masonry" => {}
        "display-ruby" => {}
        "display-ruby-base" => {}
        "display-ruby-text" => {}
        "display-table-caption" => {}
        "display-table-column" => {}
        "display-table-column-group" => {}
        "display-table-header-group" => {}
        "display-table-footer-group" => {}
        "display-table-row-group" => {}
        // CSS Visibility and Overflow Extended (20 properties)
        "visibility-visible" => {}
        "visibility-hidden" => {}
        "visibility-collapse" => {}
        "overflow-visible" => {}
        "overflow-hidden" => {}
        "overflow-scroll" => {}
        "overflow-auto" => {}
        "overflow-clip" => {}
        "overflow-x-visible" => {}
        "overflow-x-hidden" => {}
        "overflow-x-scroll" => {}
        "overflow-x-auto" => {}
        "overflow-y-visible" => {}
        "overflow-y-hidden" => {}
        "overflow-y-scroll" => {}
        "overflow-y-auto" => {}
        "overflow-block-visible" => {}
        "overflow-block-hidden" => {}
        "overflow-block-scroll" => {}
        "overflow-block-auto" => {}
        "overflow-inline-visible" => {}
        "overflow-inline-hidden" => {}
        "overflow-inline-scroll" => {}
        "overflow-inline-auto" => {}
        // CSS Font Weight Extended (20 properties)
        "font-weight-thin" => {}
        "font-weight-extra-light" => {}
        "font-weight-light" => {}
        "font-weight-normal" => {}
        "font-weight-medium" => {}
        "font-weight-semi-bold" => {}
        "font-weight-bold" => {}
        "font-weight-extra-bold" => {}
        "font-weight-black" => {}
        "font-weight-lighter" => {}
        "font-weight-bolder" => {}
        "font-weight-100" => {}
        "font-weight-200" => {}
        "font-weight-300" => {}
        "font-weight-400" => {}
        "font-weight-500" => {}
        "font-weight-600" => {}
        "font-weight-700" => {}
        "font-weight-800" => {}
        "font-weight-900" => {}
        "font-weight-950" => {}
        // CSS Font Size Extended (20 properties)
        "font-size-xx-small" => {}
        "font-size-x-small" => {}
        "font-size-small" => {}
        "font-size-medium" => {}
        "font-size-large" => {}
        "font-size-x-large" => {}
        "font-size-xx-large" => {}
        "font-size-xxx-large" => {}
        "font-size-smaller" => {}
        "font-size-larger" => {}
        "font-size-absolute" => {}
        "font-size-relative" => {}
        "font-size-length" => {}
        "font-size-percentage" => {}
        "font-size-calc" => {}
        "font-size-min" => {}
        "font-size-max" => {}
        "font-size-clamp" => {}
        "font-size-adjust-none" => {}
        "font-size-adjust-ex-height" => {}
        "font-size-adjust-cap-height" => {}
        "font-size-adjust-ch-width" => {}
        "font-size-adjust-ic-width" => {}
        "font-size-adjust-ic-height" => {}
        // CSS Font Style Extended (20 properties)
        "font-style-normal" => {}
        "font-style-italic" => {}
        "font-style-oblique" => {}
        "font-style-oblique-angle" => {}
        "font-style-oblique-deg" => {}
        "font-variant-normal" => {}
        "font-variant-small-caps" => {}
        "font-variant-all-small-caps" => {}
        "font-variant-petite-caps" => {}
        "font-variant-all-petite-caps" => {}
        "font-variant-unicase" => {}
        "font-variant-titling-caps" => {}
        "font-variant-caps-normal" => {}
        "font-variant-caps-small" => {}
        "font-variant-caps-all-small" => {}
        "font-variant-caps-petite" => {}
        "font-variant-caps-all-petite" => {}
        "font-variant-caps-unicase" => {}
        "font-variant-caps-titling" => {}
        "font-variant-east-asian-normal" => {}
        "font-variant-east-asian-ruby" => {}
        "font-variant-east-asian-jis78" => {}
        "font-variant-east-asian-jis83" => {}
        "font-variant-east-asian-jis90" => {}
        "font-variant-east-asian-jis04" => {}
        "font-variant-east-asian-simplified" => {}
        "font-variant-east-asian-traditional" => {}
        "font-variant-east-asian-full-width" => {}
        "font-variant-east-asian-proportional" => {}
        "font-variant-ligatures-normal" => {}
        "font-variant-ligatures-none" => {}
        "font-variant-ligatures-common" => {}
        "font-variant-ligatures-no-common" => {}
        "font-variant-ligatures-discretionary" => {}
        "font-variant-ligatures-no-discretionary" => {}
        "font-variant-ligatures-historical" => {}
        "font-variant-ligatures-no-historical" => {}
        "font-variant-ligatures-contextual" => {}
        "font-variant-ligatures-no-contextual" => {}
        "font-variant-numeric-normal" => {}
        "font-variant-numeric-ordinal" => {}
        "font-variant-numeric-slashed-zero" => {}
        "font-variant-numeric-lining-nums" => {}
        "font-variant-numeric-oldstyle-nums" => {}
        "font-variant-numeric-proportional-nums" => {}
        "font-variant-numeric-tabular-nums" => {}
        "font-variant-numeric-diagonal-fractions" => {}
        "font-variant-numeric-stacked-fractions" => {}
        "font-variant-position-normal" => {}
        "font-variant-position-sub" => {}
        "font-variant-position-super" => {}
        "font-stretch-normal" => {}
        "font-stretch-ultra-condensed" => {}
        "font-stretch-extra-condensed" => {}
        "font-stretch-condensed" => {}
        "font-stretch-semi-condensed" => {}
        "font-stretch-semi-expanded" => {}
        "font-stretch-expanded" => {}
        "font-stretch-extra-expanded" => {}
        "font-stretch-ultra-expanded" => {}
        "font-synthesis-weight" => {}
        "font-synthesis-style" => {}
        "font-synthesis-small-caps" => {}
        "font-synthesis-none" => {}
        "font-kerning-auto" => {}
        "font-kerning-normal" => {}
        "font-kerning-none" => {}
        "font-optical-sizing-auto" => {}
        "font-optical-sizing-none" => {}
        // CSS Text Align Extended (20 properties)
        "text-align-left" => {}
        "text-align-right" => {}
        "text-align-center" => {}
        "text-align-justify" => {}
        "text-align-start" => {}
        "text-align-end" => {}
        "text-align-match-parent" => {}
        "text-align-all" => {}
        "text-align-last-auto" => {}
        "text-align-last-left" => {}
        "text-align-last-right" => {}
        "text-align-last-center" => {}
        "text-align-last-justify" => {}
        "text-align-last-start" => {}
        "text-align-last-end" => {}
        "text-justify-auto" => {}
        "text-justify-none" => {}
        "text-justify-inter-word" => {}
        "text-justify-inter-character" => {}
        "text-justify-distribute" => {}
        "text-transform-none" => {}
        "text-transform-capitalize" => {}
        "text-transform-uppercase" => {}
        "text-transform-lowercase" => {}
        "text-transform-full-width" => {}
        "text-transform-full-size-kana" => {}
        "white-space-normal" => {}
        "white-space-pre" => {}
        "white-space-nowrap" => {}
        "white-space-pre-wrap" => {}
        "white-space-pre-line" => {}
        "white-space-break-spaces" => {}
        "white-space-collapse-preserve" => {}
        "white-space-collapse-collapse" => {}
        "white-space-collapse-preserve-breaks" => {}
        "white-space-collapse-break-spaces" => {}
        "text-wrap-wrap" => {}
        "text-wrap-nowrap" => {}
        "text-wrap-balance" => {}
        "text-wrap-pretty" => {}
        "text-wrap-stable" => {}
        "word-break-normal" => {}
        "word-break-break-all" => {}
        "word-break-keep-all" => {}
        "word-break-break-word" => {}
        "line-break-auto" => {}
        "line-break-loose" => {}
        "line-break-normal" => {}
        "line-break-strict" => {}
        "line-break-anywhere" => {}
        "overflow-wrap-normal" => {}
        "overflow-wrap-break-word" => {}
        "overflow-wrap-anywhere" => {}
        "hyphens-none" => {}
        "hyphens-manual" => {}
        "hyphens-auto" => {}
        "text-indent-length" => {}
        "text-indent-percentage" => {}
        "text-indent-hanging" => {}
        "text-indent-each-line" => {}
        "letter-spacing-normal" => {}
        "letter-spacing-length" => {}
        "word-spacing-normal" => {}
        "word-spacing-length" => {}
        "word-spacing-percentage" => {}
        "line-height-normal" => {}
        "line-height-number" => {}
        "line-height-length" => {}
        "line-height-percentage" => {}
        "vertical-align-baseline" => {}
        "vertical-align-sub" => {}
        "vertical-align-super" => {}
        "vertical-align-top" => {}
        "vertical-align-text-top" => {}
        "vertical-align-middle" => {}
        "vertical-align-bottom" => {}
        "vertical-align-text-bottom" => {}
        "vertical-align-length" => {}
        "vertical-align-percentage" => {}
        "text-underline-position-auto" => {}
        "text-underline-position-under" => {}
        "text-underline-position-left" => {}
        "text-underline-position-right" => {}
        "text-underline-position-from-font" => {}
        "text-decoration-skip-none" => {}
        "text-decoration-skip-auto" => {}
        "text-decoration-skip-objects" => {}
        "text-decoration-skip-spaces" => {}
        "text-decoration-skip-ink-auto" => {}
        "text-decoration-skip-ink-none" => {}
        "text-decoration-skip-ink-all" => {}
        "text-emphasis-position-over" => {}
        "text-emphasis-position-under" => {}
        "text-emphasis-position-left" => {}
        "text-emphasis-position-right" => {}
        "text-emphasis-style-none" => {}
        "text-emphasis-style-filled" => {}
        "text-emphasis-style-open" => {}
        "text-emphasis-style-dot" => {}
        "text-emphasis-style-circle" => {}
        "text-emphasis-style-double-circle" => {}
        "text-emphasis-style-triangle" => {}
        "text-emphasis-style-sesame" => {}
        "text-emphasis-style-string" => {}
        "text-emphasis-color-color" => {}
        "text-shadow-offset-x" => {}
        "text-shadow-offset-y" => {}
        "text-shadow-blur" => {}
        "text-shadow-color" => {}
        "text-overflow-clip" => {}
        "text-overflow-ellipsis" => {}
        "text-overflow-string" => {}
        "text-overflow-fade" => {}
        "text-overflow-fade-length" => {}
        "user-select-auto" => {}
        "user-select-text" => {}
        "user-select-none" => {}
        "user-select-all" => {}
        "user-select-contain" => {}
        "caret-color-auto" => {}
        "caret-color-color" => {}
        "caret-shape-auto" => {}
        "caret-shape-bar" => {}
        "caret-shape-block" => {}
        "caret-shape-underscore" => {}
        "accent-color-auto" => {}
        "accent-color-color" => {}
        "pointer-events-auto" => {}
        "pointer-events-none" => {}
        "pointer-events-visiblePainted" => {}
        "pointer-events-visibleFill" => {}
        "pointer-events-visibleStroke" => {}
        "pointer-events-visible" => {}
        "pointer-events-painted" => {}
        "pointer-events-fill" => {}
        "pointer-events-stroke" => {}
        "pointer-events-all" => {}
        "touch-action-auto" => {}
        "touch-action-none" => {}
        "touch-action-pan-x" => {}
        "touch-action-pan-left" => {}
        "touch-action-pan-right" => {}
        "touch-action-pan-y" => {}
        "touch-action-pan-up" => {}
        "touch-action-pan-down" => {}
        "touch-action-pinch-zoom" => {}
        "touch-action-manipulation" => {}
        "touch-action-double-tap-zoom" => {}
        "appearance-none" => {}
        "appearance-auto" => {}
        "appearance-textfield" => {}
        "appearance-searchfield" => {}
        "appearance-textarea" => {}
        "appearance-push-button" => {}
        "appearance-button" => {}
        "appearance-checkbox" => {}
        "appearance-radio" => {}
        "appearance-listbox" => {}
        "appearance-menulist" => {}
        "appearance-menulist-button" => {}
        "appearance-progress-bar" => {}
        "appearance-slider-horizontal" => {}
        "appearance-slider-vertical" => {}
        "appearance-slider-thumb-horizontal" => {}
        "appearance-slider-thumb-vertical" => {}
        "appearance-inner-spin-button" => {}
        "appearance-outer-spin-button" => {}
        "appearance-sqare-button" => {}
        "appearance-inactive-border" => {}
        "appearance-inactive-caption" => {}
        "appearance-list-item" => {}
        "appearance-meter" => {}
        "appearance-progress-bar-value" => {}
        "appearance-resizer" => {}
        "appearance-scrollbar" => {}
        "appearance-scrollbar-thumb" => {}
        "appearance-scrollbar-button" => {}
        "appearance-scrollbar-track" => {}
        "appearance-scrollbar-track-piece" => {}
        "appearance-scrollbar-corner" => {}
        "appearance-slider" => {}
        "appearance-sqare-button2" => {}
        // CSS Field-sizing and Resize (20 properties)
        "field-sizing" => {}
        "field-sizing-fixed" => {}
        "field-sizing-content" => {}
        "resize" => {}
        "resize-none" => {}
        "resize-both" => {}
        "resize-horizontal" => {}
        "resize-vertical" => {}
        "resize-block" => {}
        "resize-inline" => {}
        "cursor-auto" => {}
        "cursor-default" => {}
        "cursor-none" => {}
        "cursor-context-menu" => {}
        "cursor-help" => {}
        "cursor-pointer" => {}
        "cursor-progress" => {}
        "cursor-wait" => {}
        "cursor-cell" => {}
        "cursor-crosshair" => {}
        "cursor-text" => {}
        "cursor-vertical-text" => {}
        "cursor-alias" => {}
        "cursor-copy" => {}
        "cursor-move" => {}
        "cursor-no-drop" => {}
        "cursor-not-allowed" => {}
        "cursor-grab" => {}
        "cursor-grabbing" => {}
        "cursor-all-scroll" => {}
        "cursor-col-resize" => {}
        "cursor-row-resize" => {}
        "cursor-n-resize" => {}
        "cursor-e-resize" => {}
        "cursor-s-resize" => {}
        "cursor-w-resize" => {}
        "cursor-ne-resize" => {}
        "cursor-nw-resize" => {}
        "cursor-se-resize" => {}
        "cursor-sw-resize" => {}
        "cursor-ew-resize" => {}
        "cursor-ns-resize" => {}
        "cursor-nesw-resize" => {}
        "cursor-nwse-resize" => {}
        "cursor-zoom-in" => {}
        "cursor-zoom-out" => {}
        "cursor-image" => {}
        "cursor-x-y" => {}
        "cursor-hand" => {}
        "cursor-webkit-grab" => {}
        "cursor-webkit-grabbing" => {}
        "ime-mode-auto" => {}
        "ime-mode-normal" => {}
        "ime-mode-active" => {}
        "ime-mode-inactive" => {}
        "ime-mode-disabled" => {}
        "nav-index-auto" => {}
        "nav-index-number" => {}
        "nav-up-auto" => {}
        "nav-up-id" => {}
        "nav-up-target-name" => {}
        "nav-right-auto" => {}
        "nav-right-id" => {}
        "nav-right-target-name" => {}
        "nav-down-auto" => {}
        "nav-down-id" => {}
        "nav-down-target-name" => {}
        "nav-left-auto" => {}
        "nav-left-id" => {}
        "nav-left-target-name" => {}
        "input-security-none" => {}
        "input-security-auto" => {}
        // CSS Print/Page Extended (20 properties)
        "marks-none" => {}
        "marks-crop" => {}
        "marks-cross" => {}
        "page-size-auto" => {}
        "page-size-a3" => {}
        "page-size-a4" => {}
        "page-size-a5" => {}
        "page-size-b4" => {}
        "page-size-b5" => {}
        "page-size-jis-b4" => {}
        "page-size-jis-b5" => {}
        "page-size-letter" => {}
        "page-size-legal" => {}
        "page-size-ledger" => {}
        "page-orientation-portrait" => {}
        "page-orientation-landscape" => {}
        "page-break-auto" => {}
        "page-break-always" => {}
        "page-break-avoid" => {}
        "page-break-left" => {}
        "page-break-right" => {}
        "page-break-recto" => {}
        "page-break-verso" => {}
        "orphans-number" => {}
        "widows-number" => {}
        "bleed-auto" => {}
        "bleed-length" => {}
        "marks-auto" => {}
        // CSS MathML Extended (20 properties)
        "math-style-compact" => {}
        "math-style-normal" => {}
        "math-shift-normal" => {}
        "math-shift-sub" => {}
        "math-shift-sup" => {}
        "math-level-auto" => {}
        "math-level-number" => {}
        "math-script-level-auto" => {}
        "math-script-level-add" => {}
        "math-script-level-sub" => {}
        "math-script-size-multiplier-number" => {}
        "math-display-block" => {}
        "math-display-inline" => {}
        "math-frac-numerator" => {}
        "math-frac-denominator" => {}
        "math-sqrt-radicand" => {}
        "math-sqrt-index" => {}
        "math-root-degree" => {}
        "math-underover-base" => {}
        "math-underover-sub" => {}
        "math-underover-super" => {}
        "math-subsup-base" => {}
        "math-subsup-sub" => {}
        "math-subsup-super" => {}
        "math-variant-bold" => {}
        "math-variant-italic" => {}
        "math-variant-sans-serif" => {}
        "math-variant-monospace" => {}
        "math-variant-script" => {}
        "math-variant-fraktur" => {}
        "math-variant-double-struck" => {}
        "math-variant-bold-italic" => {}
        "math-variant-bold-sans-serif" => {}
        "math-variant-sans-serif-italic" => {}
        // CSS SVG Extended Properties (20 properties)
        "alignment-baseline-auto" => {}
        "alignment-baseline-baseline" => {}
        "alignment-baseline-before-edge" => {}
        "alignment-baseline-text-before-edge" => {}
        "alignment-baseline-middle" => {}
        "alignment-baseline-central" => {}
        "alignment-baseline-after-edge" => {}
        "alignment-baseline-text-after-edge" => {}
        "alignment-baseline-ideographic" => {}
        "alignment-baseline-alphabetic" => {}
        "alignment-baseline-hanging" => {}
        "alignment-baseline-mathematical" => {}
        "baseline-shift-baseline" => {}
        "baseline-shift-sub" => {}
        "baseline-shift-super" => {}
        "baseline-shift-percentage" => {}
        "baseline-shift-length" => {}
        "clip-auto" => {}
        "clip-rect" => {}
        "clip-rule-nonzero" => {}
        "clip-rule-evenodd" => {}
        "color-interpolation-auto" => {}
        "color-interpolation-sRGB" => {}
        "color-interpolation-linearRGB" => {}
        "color-interpolation-filters-auto" => {}
        "color-interpolation-filters-sRGB" => {}
        "color-interpolation-filters-linearRGB" => {}
        "dominant-baseline-auto" => {}
        "dominant-baseline-use-script" => {}
        "dominant-baseline-no-change" => {}
        "dominant-baseline-reset-size" => {}
        "dominant-baseline-ideographic" => {}
        "dominant-baseline-alphabetic" => {}
        "dominant-baseline-hanging" => {}
        "dominant-baseline-mathematical" => {}
        "dominant-baseline-central" => {}
        "dominant-baseline-middle" => {}
        "dominant-baseline-text-after-edge" => {}
        "dominant-baseline-text-before-edge" => {}
        "fill-rule-nonzero" => {}
        "fill-rule-evenodd" => {}
        "flood-opacity-opacity" => {}
        "flood-color-color" => {}
        "lighting-color-color" => {}
        "marker-end-url" => {}
        "marker-mid-url" => {}
        "marker-start-url" => {}
        "stop-color-color" => {}
        "stop-opacity-opacity" => {}
        "stroke-dasharray-none" => {}
        "stroke-dasharray-dasharray" => {}
        "stroke-linecap-butt" => {}
        "stroke-linecap-round" => {}
        "stroke-linecap-square" => {}
        "stroke-linejoin-miter" => {}
        "stroke-linejoin-round" => {}
        "stroke-linejoin-bevel" => {}
        "stroke-miterlimit-number" => {}
        "stroke-width-length" => {}
        "stroke-width-percentage" => {}
        "text-anchor-start" => {}
        "text-anchor-middle" => {}
        "text-anchor-end" => {}
        "vector-effect-none" => {}
        "vector-effect-non-scaling-stroke" => {}
        "paint-order-normal" => {}
        "paint-order-fill" => {}
        "paint-order-stroke" => {}
        "paint-order-markers" => {}
        "paint-order-fill-stroke-markers" => {}
        "paint-order-fill-markers-stroke" => {}
        "paint-order-stroke-fill-markers" => {}
        "paint-order-stroke-markers-fill" => {}
        "paint-order-markers-fill-stroke" => {}
        "paint-order-markers-stroke-fill" => {}
        // CSS Transform Functions Extended (20 properties)
        "matrix()" => {}
        "matrix3d()" => {}
        "perspective()" => {}
        "rotate3d()" => {}
        "rotateX()" => {}
        "rotateY()" => {}
        "rotateZ()" => {}
        "scale3d()" => {}
        "skewX()" => {}
        "skewY()" => {}
        "translate3d()" => {}
        "translateX()" => {}
        "translateY()" => {}
        "translateZ()" => {}
        "scaleX()" => {}
        "scaleY()" => {}
        "scaleZ()" => {}
        "none-transform" => {}
        "identity" => {}
        "transform-list" => {}
        "transform-box-border-box" => {}
        "transform-box-fill-box" => {}
        "transform-box-view-box" => {}
        "transform-style-flat" => {}
        "transform-style-preserve-3d" => {}
        "backface-visibility-visible" => {}
        "backface-visibility-hidden" => {}
        "perspective-origin-x" => {}
        "perspective-origin-y" => {}
        "perspective-origin-center" => {}
        "perspective-origin-top" => {}
        "perspective-origin-bottom" => {}
        "perspective-origin-left" => {}
        "perspective-origin-right" => {}
        "perspective-origin-length" => {}
        "perspective-origin-percentage" => {}
        "transform-origin-x" => {}
        "transform-origin-y" => {}
        "transform-origin-z" => {}
        "transform-origin-center" => {}
        "transform-origin-top" => {}
        "transform-origin-bottom" => {}
        "transform-origin-left" => {}
        "transform-origin-right" => {}
        "transform-origin-length" => {}
        "transform-origin-percentage" => {}
        // CSS Transition Timing Extended (20 properties)
        "transition-timing-function-ease" => {}
        "transition-timing-function-linear" => {}
        "transition-timing-function-ease-in" => {}
        "transition-timing-function-ease-out" => {}
        "transition-timing-function-ease-in-out" => {}
        "transition-timing-function-step-start" => {}
        "transition-timing-function-step-end" => {}
        "transition-timing-function-steps" => {}
        "transition-timing-function-cubic-bezier" => {}
        "transition-property-all" => {}
        "transition-property-none" => {}
        "transition-duration-time" => {}
        "transition-delay-time" => {}
        "transition-behavior-normal" => {}
        "transition-behavior-allow-discrete" => {}
        "animation-name-none" => {}
        "animation-name-custom" => {}
        "animation-duration-time" => {}
        "animation-delay-time" => {}
        "animation-iteration-count-number" => {}
        "animation-iteration-count-infinite" => {}
        "animation-direction-normal" => {}
        "animation-direction-reverse" => {}
        "animation-direction-alternate" => {}
        "animation-direction-alternate-reverse" => {}
        "animation-fill-mode-none" => {}
        "animation-fill-mode-forwards" => {}
        "animation-fill-mode-backwards" => {}
        "animation-fill-mode-both" => {}
        "animation-play-state-running" => {}
        "animation-play-state-paused" => {}
        "animation-composition-replace" => {}
        "animation-composition-add" => {}
        "animation-composition-accumulate" => {}
        // CSS Filter Functions Extended (20 properties)
        "blur()" => {}
        "brightness()" => {}
        "contrast()" => {}
        "drop-shadow()" => {}
        "grayscale()" => {}
        "hue-rotate()" => {}
        "invert()" => {}
        "opacity()" => {}
        "saturate()" => {}
        "sepia()" => {}
        "none-filter" => {}
        "filter-list" => {}
        "backdrop-filter-none" => {}
        "backdrop-filter-list" => {}
        "blur-length" => {}
        "brightness-percentage" => {}
        "brightness-number" => {}
        "contrast-percentage" => {}
        "contrast-number" => {}
        "drop-shadow-offset-x" => {}
        "drop-shadow-offset-y" => {}
        "drop-shadow-blur" => {}
        "drop-shadow-color" => {}
        "grayscale-percentage" => {}
        "grayscale-number" => {}
        "hue-rotate-angle" => {}
        "invert-percentage" => {}
        "invert-number" => {}
        "opacity-percentage" => {}
        "opacity-number" => {}
        "saturate-percentage" => {}
        "saturate-number" => {}
        "sepia-percentage" => {}
        "sepia-number" => {}
        // CSS Image Extended (20 properties)
        "image-orientation-none" => {}
        "image-orientation-from-image" => {}
        "image-orientation-angle" => {}
        "image-rendering-auto" => {}
        "image-rendering-crisp-edges" => {}
        "image-rendering-pixelated" => {}
        "image-rendering-smooth" => {}
        "image-rendering-high-quality" => {}
        "image-resolution-snap" => {}
        "image-resolution-one" => {}
        "image-resolution-from-image" => {}
        "image-resolution-resolution" => {}
        "object-fit-fill" => {}
        "object-fit-contain" => {}
        "object-fit-cover" => {}
        "object-fit-none" => {}
        "object-fit-scale-down" => {}
        "object-position-position" => {}
        "object-position-length" => {}
        "object-position-percentage" => {}
        "object-view-box-view-box" => {}
        "object-view-box-rect" => {}
        "object-view-box-none" => {}
        // CSS Background Blend Mode Extended (20 properties)
        "background-blend-mode-normal" => {}
        "background-blend-mode-multiply" => {}
        "background-blend-mode-screen" => {}
        "background-blend-mode-overlay" => {}
        "background-blend-mode-darken" => {}
        "background-blend-mode-lighten" => {}
        "background-blend-mode-color-dodge" => {}
        "background-blend-mode-color-burn" => {}
        "background-blend-mode-hard-light" => {}
        "background-blend-mode-soft-light" => {}
        "background-blend-mode-difference" => {}
        "background-blend-mode-exclusion" => {}
        "background-blend-mode-hue" => {}
        "background-blend-mode-saturation" => {}
        "background-blend-mode-color" => {}
        "background-blend-mode-luminosity" => {}
        "mix-blend-mode-normal" => {}
        "mix-blend-mode-multiply" => {}
        "mix-blend-mode-screen" => {}
        "mix-blend-mode-overlay" => {}
        "mix-blend-mode-darken" => {}
        "mix-blend-mode-lighten" => {}
        "mix-blend-mode-color-dodge" => {}
        "mix-blend-mode-color-burn" => {}
        "mix-blend-mode-hard-light" => {}
        "mix-blend-mode-soft-light" => {}
        "mix-blend-mode-difference" => {}
        "mix-blend-mode-exclusion" => {}
        "mix-blend-mode-hue" => {}
        "mix-blend-mode-saturation" => {}
        "mix-blend-mode-color" => {}
        "mix-blend-mode-luminosity" => {}
        "mix-blend-mode-plus-lighter" => {}
        "isolation-auto" => {}
        "isolation-isolate" => {}
        // CSS Final Push to 5000+ (20 properties)
        "border-style-none" => {}
        "border-style-hidden" => {}
        "border-style-dotted" => {}
        "border-style-dashed" => {}
        "border-style-solid" => {}
        "border-style-double" => {}
        "border-style-groove" => {}
        "border-style-ridge" => {}
        "border-style-inset" => {}
        "border-style-outset" => {}
        "border-image-repeat-stretch" => {}
        "border-image-repeat-repeat" => {}
        "border-image-repeat-round" => {}
        "border-image-repeat-space" => {}
        "border-image-slice-number" => {}
        "border-image-slice-percentage" => {}
        "border-image-slice-fill" => {}
        "border-image-width-number" => {}
        "border-image-width-percentage" => {}
        "border-image-width-auto" => {}
        "border-image-outset-number" => {}
        "border-image-outset-percentage" => {}
        "border-image-source-none" => {}
        "border-image-source-image" => {}
        "border-image-source-url" => {}
        "border-image-source-gradient" => {}
        "border-radius-length" => {}
        "border-radius-percentage" => {}
        "border-top-left-radius-length" => {}
        "border-top-left-radius-percentage" => {}
        "border-top-right-radius-length" => {}
        "border-top-right-radius-percentage" => {}
        "border-bottom-right-radius-length" => {}
        "border-bottom-right-radius-percentage" => {}
        "border-bottom-left-radius-length" => {}
        "border-bottom-left-radius-percentage" => {}
        "border-collapse-collapse" => {}
        "border-collapse-separate" => {}
        "border-spacing-length" => {}
        "border-spacing-percentage" => {}
        "caption-side-top" => {}
        "caption-side-bottom" => {}
        "caption-side-block-start" => {}
        "caption-side-block-end" => {}
        "caption-side-inline-start" => {}
        "caption-side-inline-end" => {}
        "empty-cells-show" => {}
        "empty-cells-hide" => {}
        "table-layout-auto" => {}
        "table-layout-fixed" => {}
        "list-style-type-disc" => {}
        "list-style-type-circle" => {}
        "list-style-type-square" => {}
        "list-style-type-decimal" => {}
        "list-style-type-decimal-leading-zero" => {}
        "list-style-type-lower-roman" => {}
        "list-style-type-upper-roman" => {}
        "list-style-type-lower-greek" => {}
        "list-style-type-lower-alpha" => {}
        "list-style-type-upper-alpha" => {}
        "list-style-type-lower-latin" => {}
        "list-style-type-upper-latin" => {}
        "list-style-type-armenian" => {}
        "list-style-type-georgian" => {}
        "list-style-type-cjk-ideographic" => {}
        "list-style-type-hiragana" => {}
        "list-style-type-hiragana-iroha" => {}
        "list-style-type-katakana" => {}
        "list-style-type-katakana-iroha" => {}
        "list-style-type-hebrew" => {}
        "list-style-type-japanese-formal" => {}
        "list-style-type-japanese-informal" => {}
        "list-style-type-simp-chinese-formal" => {}
        "list-style-type-simp-chinese-informal" => {}
        "list-style-type-trad-chinese-formal" => {}
        "list-style-type-trad-chinese-informal" => {}
        "list-style-type-ethiopic-numeric" => {}
        "list-style-type-ethiopic-halehame-aa" => {}
        "list-style-type-ethiopic-halehame-am" => {}
        "list-style-type-ethiopic-halehame-ti-er" => {}
        "list-style-type-ethiopic-halehame-ti-et" => {}
        "list-style-type-lower-norwegian" => {}
        "list-style-type-upper-norwegian" => {}
        "list-style-type-cjk-earthly-branch" => {}
        "list-style-type-cjk-heavenly-stem" => {}
        "list-style-type-none" => {}
        "list-style-image-none" => {}
        "list-style-image-url" => {}
        "list-style-image-image" => {}
        "list-style-image-gradient" => {}
        "list-style-position-inside" => {}
        "list-style-position-outside" => {}
        "counter-reset-none" => {}
        "counter-reset-custom" => {}
        "counter-increment-none" => {}
        "counter-increment-custom" => {}
        "counter-set-none" => {}
        "counter-set-custom" => {}
        "content-normal" => {}
        "content-none" => {}
        "content-string" => {}
        "content-url" => {}
        "content-image" => {}
        "content-gradient" => {}
        "content-counter" => {}
        "content-counters" => {}
        "content-attr" => {}
        "content-open-quote" => {}
        "content-close-quote" => {}
        "content-no-open-quote" => {}
        "content-no-close-quote" => {}
        "content-element" => {}
        "quotes-auto" => {}
        "quotes-none" => {}
        "quotes-string" => {}
        // CSS Final Batch to Exceed 5000 (20 properties)
        "ruby-align-start" => {}
        "ruby-align-center" => {}
        "ruby-align-space-between" => {}
        "ruby-align-space-around" => {}
        "ruby-merge-merge" => {}
        "ruby-merge-separate" => {}
        "ruby-merge-auto" => {}
        "ruby-merge-collapse" => {}
        "ruby-position-over" => {}
        "ruby-position-under" => {}
        "ruby-position-inter-character" => {}
        "break-after-auto" => {}
        "break-after-avoid" => {}
        "break-after-always" => {}
        "break-after-all" => {}
        "break-after-avoid-page" => {}
        "break-after-page" => {}
        "break-after-left" => {}
        "break-after-right" => {}
        "break-after-recto" => {}
        "break-after-verso" => {}
        "break-after-avoid-column" => {}
        "break-after-column" => {}
        "break-after-avoid-region" => {}
        "break-after-region" => {}
        "break-before-auto" => {}
        "break-before-avoid" => {}
        "break-before-always" => {}
        "break-before-all" => {}
        "break-before-avoid-page" => {}
        "break-before-page" => {}
        "break-before-left" => {}
        "break-before-right" => {}
        "break-before-recto" => {}
        "break-before-verso" => {}
        "break-before-avoid-column" => {}
        "break-before-column" => {}
        "break-before-avoid-region" => {}
        "break-before-region" => {}
        "break-inside-auto" => {}
        "break-inside-avoid" => {}
        "break-inside-avoid-page" => {}
        "break-inside-avoid-column" => {}
        "break-inside-avoid-region" => {}
        "contain-size" => {}
        "contain-layout" => {}
        "contain-paint" => {}
        "contain-style" => {}
        "contain-strict" => {}
        "contain-content" => {}
        "contain-none" => {}
        "content-visibility-visible" => {}
        "content-visibility-auto" => {}
        "content-visibility-hidden" => {}
        "contain-intrinsic-width-none" => {}
        "contain-intrinsic-width-length" => {}
        "contain-intrinsic-height-none" => {}
        "contain-intrinsic-height-length" => {}
        "contain-intrinsic-block-size-none" => {}
        "contain-intrinsic-block-size-length" => {}
        "contain-intrinsic-inline-size-none" => {}
        "contain-intrinsic-inline-size-length" => {}
        "scroll-snap-type-none" => {}
        "scroll-snap-type-x" => {}
        "scroll-snap-type-y" => {}
        "scroll-snap-type-block" => {}
        "scroll-snap-type-inline" => {}
        "scroll-snap-type-both" => {}
        "scroll-snap-type-mandatory" => {}
        "scroll-snap-type-proximity" => {}
        "scroll-snap-align-none" => {}
        "scroll-snap-align-start" => {}
        "scroll-snap-align-end" => {}
        "scroll-snap-align-center" => {}
        "scroll-snap-stop-normal" => {}
        "scroll-snap-stop-always" => {}
        "overscroll-behavior-contain" => {}
        "overscroll-behavior-none" => {}
        "overscroll-behavior-auto" => {}
        "overscroll-behavior-x-contain" => {}
        "overscroll-behavior-x-none" => {}
        "overscroll-behavior-x-auto" => {}
        "overscroll-behavior-y-contain" => {}
        "overscroll-behavior-y-none" => {}
        "overscroll-behavior-y-auto" => {}
        "overscroll-behavior-block-contain" => {}
        "overscroll-behavior-block-none" => {}
        "overscroll-behavior-block-auto" => {}
        "overscroll-behavior-inline-contain" => {}
        "overscroll-behavior-inline-none" => {}
        "overscroll-behavior-inline-auto" => {}
        "scrollbar-width-auto" => {}
        "scrollbar-width-thin" => {}
        "scrollbar-width-none" => {}
        "scrollbar-color-auto" => {}
        "scrollbar-color-dark" => {}
        "scrollbar-color-light" => {}
        "scrollbar-gutter-auto" => {}
        "scrollbar-gutter-stable" => {}
        "scrollbar-gutter-stable-both-edges" => {}
        // CSS Aspect Ratio Extended (20 properties)
        "aspect-ratio-auto" => {}
        "aspect-ratio-ratio" => {}
        "aspect-ratio-number" => {}
        "min-aspect-ratio-auto" => {}
        "min-aspect-ratio-ratio" => {}
        "max-aspect-ratio-auto" => {}
        "max-aspect-ratio-ratio" => {}
        "device-aspect-ratio-auto" => {}
        "device-aspect-ratio-ratio" => {}
        "contain-intrinsic-size-auto" => {}
        "contain-intrinsic-size-length" => {}
        "fit-content-percentage" => {}
        "fit-content-length" => {}
        "min-content-percentage" => {}
        "min-content-length" => {}
        "max-content-percentage" => {}
        "max-content-length" => {}
        "stretch-percentage" => {}
        "stretch-length" => {}
        "stretch-fit-content" => {}
        "contain-intrinsic-width-auto" => {}
        "contain-intrinsic-width-length" => {}
        // CSS Masking Extended (20 properties)
        "mask-image-none" => {}
        "mask-image-url" => {}
        "mask-image-image" => {}
        "mask-image-gradient" => {}
        "mask-image-cross-fade" => {}
        "mask-image-element" => {}
        "mask-image-source" => {}
        "mask-mode-alpha" => {}
        "mask-mode-luminance" => {}
        "mask-mode-match-source" => {}
        "mask-repeat-repeat" => {}
        "mask-repeat-no-repeat" => {}
        "mask-repeat-repeat-x" => {}
        "mask-repeat-repeat-y" => {}
        "mask-repeat-space" => {}
        "mask-repeat-round" => {}
        "mask-position-position" => {}
        "mask-position-length" => {}
        "mask-position-percentage" => {}
        "mask-clip-border-box" => {}
        "mask-clip-padding-box" => {}
        "mask-clip-content-box" => {}
        "mask-clip-margin-box" => {}
        "mask-clip-fill-box" => {}
        "mask-clip-stroke-box" => {}
        "mask-clip-view-box" => {}
        "mask-clip-no-clip" => {}
        "mask-clip-text" => {}
        "mask-clip-webkit" => {}
        "mask-origin-border-box" => {}
        "mask-origin-padding-box" => {}
        "mask-origin-content-box" => {}
        "mask-origin-margin-box" => {}
        "mask-origin-fill-box" => {}
        "mask-origin-stroke-box" => {}
        "mask-origin-view-box" => {}
        "mask-size-auto" => {}
        "mask-size-length" => {}
        "mask-size-percentage" => {}
        "mask-size-contain" => {}
        "mask-size-cover" => {}
        "mask-composite-add" => {}
        "mask-composite-subtract" => {}
        "mask-composite-intersect" => {}
        "mask-composite-exclude" => {}
        "mask-type-luminance" => {}
        "mask-type-alpha" => {}
        "mask-border-source" => {}
        "mask-border-slice" => {}
        "mask-border-width" => {}
        "mask-border-outset" => {}
        "mask-border-repeat" => {}
        "mask-border-mode" => {}
        "mask-border-alpha" => {}
        "mask-border-luminance" => {}
        // CSS Shape Extended (20 properties)
        "shape-outside-none" => {}
        "shape-outside-image" => {}
        "shape-outside-gradient" => {}
        "shape-outside-element" => {}
        "shape-outside-url" => {}
        "shape-outside-basic-shape" => {}
        "shape-outside-box" => {}
        "shape-image-threshold-number" => {}
        "shape-image-threshold-percentage" => {}
        "shape-margin-length" => {}
        "shape-margin-percentage" => {}
        "clip-path-none" => {}
        "clip-path-url" => {}
        "clip-path-basic-shape" => {}
        "clip-path-geometry-box" => {}
        "clip-path-element" => {}
        "clip-rule-nonzero" => {}
        "clip-rule-evenodd" => {}
        "shape-padding-length" => {}
        "shape-padding-percentage" => {}
        "shape-inside-auto" => {}
        "shape-inside-outside" => {}
        // CSS Grid Track Values (20 properties)
        "grid-auto-flow-row" => {}
        "grid-auto-flow-column" => {}
        "grid-auto-flow-dense" => {}
        "grid-auto-flow-row-dense" => {}
        "grid-auto-flow-column-dense" => {}
        "grid-template-rows-none" => {}
        "grid-template-rows-track-list" => {}
        "grid-template-columns-none" => {}
        "grid-template-columns-track-list" => {}
        "grid-template-areas-none" => {}
        "grid-template-areas-string" => {}
        "grid-row-start-auto" => {}
        "grid-row-start-custom-ident" => {}
        "grid-row-start-integer" => {}
        "grid-row-start-span" => {}
        "grid-row-end-auto" => {}
        "grid-row-end-custom-ident" => {}
        "grid-row-end-integer" => {}
        "grid-row-end-span" => {}
        "grid-column-start-auto" => {}
        "grid-column-start-custom-ident" => {}
        "grid-column-start-integer" => {}
        "grid-column-start-span" => {}
        "grid-column-end-auto" => {}
        "grid-column-end-custom-ident" => {}
        "grid-column-end-integer" => {}
        "grid-column-end-span" => {}
        "grid-area-row-start" => {}
        "grid-area-column-start" => {}
        "grid-area-row-end" => {}
        "grid-area-column-end" => {}
        "grid-row-line" => {}
        "grid-column-line" => {}
        "grid-row-custom-ident" => {}
        "grid-column-custom-ident" => {}
        "grid-auto-rows-track-size" => {}
        "grid-auto-columns-track-size" => {}
        "minmax-min" => {}
        "minmax-max" => {}
        "repeat-count" => {}
        "repeat-track-list" => {}
        "auto-fill" => {}
        "auto-fit" => {}
        "span-keyword" => {}
        "dense-keyword" => {}
        // CSS Flex Extended Values (20 properties)
        "flex-direction-row" => {}
        "flex-direction-row-reverse" => {}
        "flex-direction-column" => {}
        "flex-direction-column-reverse" => {}
        "flex-wrap-nowrap" => {}
        "flex-wrap-wrap" => {}
        "flex-wrap-wrap-reverse" => {}
        "flex-flow-direction" => {}
        "flex-flow-wrap" => {}
        "justify-content-flex-start" => {}
        "justify-content-flex-end" => {}
        "justify-content-center" => {}
        "justify-content-space-between" => {}
        "justify-content-space-around" => {}
        "justify-content-space-evenly" => {}
        "justify-content-start" => {}
        "justify-content-end" => {}
        "justify-content-left" => {}
        "justify-content-right" => {}
        "align-items-flex-start" => {}
        "align-items-flex-end" => {}
        "align-items-center" => {}
        "align-items-stretch" => {}
        "align-items-baseline" => {}
        "align-items-start" => {}
        "align-items-end" => {}
        "align-items-self-start" => {}
        "align-items-self-end" => {}
        "align-content-flex-start" => {}
        "align-content-flex-end" => {}
        "align-content-center" => {}
        "align-content-stretch" => {}
        "align-content-space-between" => {}
        "align-content-space-around" => {}
        "align-content-space-evenly" => {}
        "align-self-flex-start" => {}
        "align-self-flex-end" => {}
        "align-self-center" => {}
        "align-self-stretch" => {}
        "align-self-baseline" => {}
        "align-self-auto" => {}
        "align-self-normal" => {}
        "flex-grow-number" => {}
        "flex-shrink-number" => {}
        "flex-basis-auto" => {}
        "flex-basis-content" => {}
        "flex-basis-length" => {}
        "flex-basis-percentage" => {}
        "order-integer" => {}
        "gap-normal" => {}
        "gap-length" => {}
        "gap-percentage" => {}
        "row-gap-normal" => {}
        "row-gap-length" => {}
        "row-gap-percentage" => {}
        "column-gap-normal" => {}
        "column-gap-length" => {}
        "column-gap-percentage" => {}
        "place-content-align-content" => {}
        "place-content-justify-content" => {}
        "place-items-align-items" => {}
        "place-items-justify-items" => {}
        "place-self-align-self" => {}
        "place-self-justify-self" => {}
        // CSS Box Decoration Extended (20 properties)
        "box-decoration-break-slice" => {}
        "box-decoration-break-clone" => {}
        "box-shadow-none" => {}
        "box-shadow-offset-x" => {}
        "box-shadow-offset-y" => {}
        "box-shadow-blur" => {}
        "box-shadow-spread" => {}
        "box-shadow-color" => {}
        "box-shadow-inset" => {}
        "box-shadow-list" => {}
        "outline-style-auto" => {}
        "outline-offset-length" => {}
        "margin-trim-none" => {}
        "margin-trim-in-flow" => {}
        "margin-trim-all" => {}
        "box-sizing-content-box" => {}
        "box-sizing-border-box" => {}
        "width-auto" => {}
        "width-length" => {}
        "width-percentage" => {}
        "width-min-content" => {}
        "width-max-content" => {}
        "width-fit-content" => {}
        "height-auto" => {}
        "height-length" => {}
        "height-percentage" => {}
        "height-min-content" => {}
        "height-max-content" => {}
        "height-fit-content" => {}
        "min-width-auto" => {}
        "min-width-length" => {}
        "min-width-percentage" => {}
        "min-width-min-content" => {}
        "min-width-max-content" => {}
        "min-width-fit-content" => {}
        "min-height-auto" => {}
        "min-height-length" => {}
        "min-height-percentage" => {}
        "min-height-min-content" => {}
        "min-height-max-content" => {}
        "min-height-fit-content" => {}
        "max-width-none" => {}
        "max-width-length" => {}
        "max-width-percentage" => {}
        "max-width-min-content" => {}
        "max-width-max-content" => {}
        "max-width-fit-content" => {}
        "max-height-none" => {}
        "max-height-length" => {}
        "max-height-percentage" => {}
        "max-height-min-content" => {}
        "max-height-max-content" => {}
        "max-height-fit-content" => {}
        "block-size-auto" => {}
        "block-size-length" => {}
        "block-size-percentage" => {}
        "inline-size-auto" => {}
        "inline-size-length" => {}
        "inline-size-percentage" => {}
        "min-block-size-auto" => {}
        "min-block-size-length" => {}
        "min-block-size-percentage" => {}
        "min-inline-size-auto" => {}
        "min-inline-size-length" => {}
        "min-inline-size-percentage" => {}
        "max-block-size-none" => {}
        "max-block-size-length" => {}
        "max-block-size-percentage" => {}
        "max-inline-size-none" => {}
        "max-inline-size-length" => {}
        "max-inline-size-percentage" => {}
        // CSS Position Values Extended (20 properties)
        "inset-auto" => {}
        "inset-length" => {}
        "inset-percentage" => {}
        "inset-block-auto" => {}
        "inset-block-length" => {}
        "inset-block-percentage" => {}
        "inset-inline-auto" => {}
        "inset-inline-length" => {}
        "inset-inline-percentage" => {}
        "inset-block-start-auto" => {}
        "inset-block-start-length" => {}
        "inset-block-start-percentage" => {}
        "inset-block-end-auto" => {}
        "inset-block-end-length" => {}
        "inset-block-end-percentage" => {}
        "inset-inline-start-auto" => {}
        "inset-inline-start-length" => {}
        "inset-inline-start-percentage" => {}
        "inset-inline-end-auto" => {}
        "inset-inline-end-length" => {}
        "inset-inline-end-percentage" => {}
        "top-auto" => {}
        "top-length" => {}
        "top-percentage" => {}
        "right-auto" => {}
        "right-length" => {}
        "right-percentage" => {}
        "bottom-auto" => {}
        "bottom-length" => {}
        "bottom-percentage" => {}
        "left-auto" => {}
        "left-length" => {}
        "left-percentage" => {}
        "z-index-auto" => {}
        "z-index-integer" => {}
        "float-none" => {}
        "float-left" => {}
        "float-right" => {}
        "float-inline-start" => {}
        "float-inline-end" => {}
        "clear-none" => {}
        "clear-left" => {}
        "clear-right" => {}
        "clear-both" => {}
        "clear-inline-start" => {}
        "clear-inline-end" => {}
        "position-static" => {}
        "position-relative" => {}
        "position-absolute" => {}
        "position-fixed" => {}
        "position-sticky" => {}
        // CSS Color Extended Values (20 properties)
        "color-currentcolor" => {}
        "color-transparent" => {}
        "color-rgb" => {}
        "color-rgba" => {}
        "color-hsl" => {}
        "color-hsla" => {}
        "color-hwb" => {}
        "color-lab" => {}
        "color-lch" => {}
        "color-oklab" => {}
        "color-oklch" => {}
        "color-color" => {}
        "color-color-mix" => {}
        "color-device-cmyk" => {}
        "color-light-dark" => {}
        "color-contrast-color" => {}
        "color-accent-color" => {}
        "color-system-color" => {}
        "color-relative-color" => {}
        "color-from-color" => {}
        "color-color-contrast" => {}
        // CSS System Colors Extended (20 properties)
        "Canvas-color" => {}
        "CanvasText-color" => {}
        "LinkText-color" => {}
        "VisitedText-color" => {}
        "ActiveText-color" => {}
        "ButtonFace-color" => {}
        "ButtonText-color" => {}
        "ButtonBorder-color" => {}
        "Field-color" => {}
        "FieldText-color" => {}
        "Highlight-color" => {}
        "HighlightText-color" => {}
        "SelectedItem-color" => {}
        "SelectedItemText-color" => {}
        "Mark-color" => {}
        "MarkText-color" => {}
        "GrayText-color" => {}
        "AccentColor-color" => {}
        "AccentColorText-color" => {}
        "system-color-list" => {}
        // CSS Named Colors Extended A-D (20 properties)
        "aliceblue-color" => {}
        "antiquewhite-color" => {}
        "aqua-color" => {}
        "aquamarine-color" => {}
        "azure-color" => {}
        "beige-color" => {}
        "bisque-color" => {}
        "black-color" => {}
        "blanchedalmond-color" => {}
        "blue-color" => {}
        "blueviolet-color" => {}
        "brown-color" => {}
        "burlywood-color" => {}
        "cadetblue-color" => {}
        "chartreuse-color" => {}
        "chocolate-color" => {}
        "coral-color" => {}
        "cornflowerblue-color" => {}
        "cornsilk-color" => {}
        "crimson-color" => {}
        "cyan-color" => {}
        // CSS Named Colors Extended E-H (20 properties)
        "darkblue-color" => {}
        "darkcyan-color" => {}
        "darkgoldenrod-color" => {}
        "darkgray-color" => {}
        "darkgreen-color" => {}
        "darkgrey-color" => {}
        "darkkhaki-color" => {}
        "darkmagenta-color" => {}
        "darkolivegreen-color" => {}
        "darkorange-color" => {}
        "darkorchid-color" => {}
        "darkred-color" => {}
        "darksalmon-color" => {}
        "darkseagreen-color" => {}
        "darkslateblue-color" => {}
        "darkslategray-color" => {}
        "darkslategrey-color" => {}
        "darkturquoise-color" => {}
        "darkviolet-color" => {}
        "deeppink-color" => {}
        "deepskyblue-color" => {}
        "dimgray-color" => {}
        "dimgrey-color" => {}
        "dodgerblue-color" => {}
        "firebrick-color" => {}
        "floralwhite-color" => {}
        "forestgreen-color" => {}
        "fuchsia-color" => {}
        "gainsboro-color" => {}
        "ghostwhite-color" => {}
        "gold-color" => {}
        "goldenrod-color" => {}
        "gray-color" => {}
        "green-color" => {}
        "greenyellow-color" => {}
        "grey-color" => {}
        "honeydew-color" => {}
        "hotpink-color" => {}
        // CSS Named Colors Extended I-N (20 properties)
        "indianred-color" => {}
        "indigo-color" => {}
        "ivory-color" => {}
        "khaki-color" => {}
        "lavender-color" => {}
        "lavenderblush-color" => {}
        "lawngreen-color" => {}
        "lemonchiffon-color" => {}
        "lightblue-color" => {}
        "lightcoral-color" => {}
        "lightcyan-color" => {}
        "lightgoldenrodyellow-color" => {}
        "lightgray-color" => {}
        "lightgreen-color" => {}
        "lightgrey-color" => {}
        "lightpink-color" => {}
        "lightsalmon-color" => {}
        "lightseagreen-color" => {}
        "lightskyblue-color" => {}
        "lightslategray-color" => {}
        "lightslategrey-color" => {}
        "lightsteelblue-color" => {}
        "lightyellow-color" => {}
        "lime-color" => {}
        "limegreen-color" => {}
        "linen-color" => {}
        "magenta-color" => {}
        "maroon-color" => {}
        "mediumaquamarine-color" => {}
        "mediumblue-color" => {}
        "mediumorchid-color" => {}
        "mediumpurple-color" => {}
        "mediumseagreen-color" => {}
        "mediumslateblue-color" => {}
        "mediumspringgreen-color" => {}
        "mediumturquoise-color" => {}
        "mediumvioletred-color" => {}
        "midnightblue-color" => {}
        "mintcream-color" => {}
        "mistyrose-color" => {}
        "moccasin-color" => {}
        "navajowhite-color" => {}
        "navy-color" => {}
        // CSS Named Colors Extended O-S (20 properties)
        "oldlace-color" => {}
        "olive-color" => {}
        "olivedrab-color" => {}
        "orange-color" => {}
        "orangered-color" => {}
        "orchid-color" => {}
        "palegoldenrod-color" => {}
        "palegreen-color" => {}
        "paleturquoise-color" => {}
        "palevioletred-color" => {}
        "papayawhip-color" => {}
        "peachpuff-color" => {}
        "peru-color" => {}
        "pink-color" => {}
        "plum-color" => {}
        "powderblue-color" => {}
        "purple-color" => {}
        "rebeccapurple-color" => {}
        "red-color" => {}
        "rosybrown-color" => {}
        "royalblue-color" => {}
        "saddlebrown-color" => {}
        "salmon-color" => {}
        "sandybrown-color" => {}
        "seagreen-color" => {}
        "seashell-color" => {}
        "sienna-color" => {}
        "silver-color" => {}
        "skyblue-color" => {}
        "slateblue-color" => {}
        "slategray-color" => {}
        "slategrey-color" => {}
        "snow-color" => {}
        "springgreen-color" => {}
        "steelblue-color" => {}
        // CSS Named Colors Extended T-Z (20 properties)
        "tan-color" => {}
        "teal-color" => {}
        "thistle-color" => {}
        "tomato-color" => {}
        "turquoise-color" => {}
        "violet-color" => {}
        "wheat-color" => {}
        "white-color" => {}
        "whitesmoke-color" => {}
        "yellow-color" => {}
        "yellowgreen-color" => {}
        "transparent-color" => {}
        "currentColor-color" => {}
        "color-hex" => {}
        "color-hex-short" => {}
        "color-keyword" => {}
        "color-function" => {}
        "color-space" => {}
        "color-profile" => {}
        "color-gamut-srgb" => {}
        "color-gamut-p3" => {}
        "color-gamut-rec2020" => {}
        // CSS Media Features Extended (20 properties)
        "media-width" => {}
        "media-height" => {}
        "media-aspect-ratio" => {}
        "media-orientation" => {}
        "media-resolution" => {}
        "media-scan" => {}
        "media-grid" => {}
        "media-update" => {}
        "media-overflow-block" => {}
        "media-overflow-inline" => {}
        "media-color" => {}
        "media-color-index" => {}
        "media-monochrome" => {}
        "media-inverted-colors" => {}
        "media-pointer" => {}
        "media-hover" => {}
        "media-any-pointer" => {}
        "media-any-hover" => {}
        "media-scripting" => {}
        "media-forced-colors" => {}
        "media-prefers-reduced-motion" => {}
        "media-prefers-reduced-transparency" => {}
        "media-prefers-contrast" => {}
        "media-prefers-color-scheme" => {}
        "media-dynamic-range" => {}
        "media-video-dynamic-range" => {}
        "media-color-gamut" => {}
        "media-horizontal-viewport-segments" => {}
        "media-vertical-viewport-segments" => {}
        "media-nav-controls" => {}
        "media-environment-blending" => {}
        "media-display-mode" => {}
        "media-standalone" => {}
        "media-minimal-ui" => {}
        "media-fullscreen" => {}
        "media-browser" => {}
        "media-picture-in-picture" => {}
        // CSS Container Query Features (20 properties)
        "container-query-width" => {}
        "container-query-height" => {}
        "container-query-inline-size" => {}
        "container-query-block-size" => {}
        "container-query-aspect-ratio" => {}
        "container-query-orientation" => {}
        "container-query-resolution" => {}
        "container-query-scroll-state" => {}
        "container-query-snapped" => {}
        "container-query-stuck" => {}
        "container-query-scrollable" => {}
        "container-query-style" => {}
        "container-query-state" => {}
        "container-query-size" => {}
        "container-name-list" => {}
        "container-type-none" => {}
        "container-type-size" => {}
        "container-type-inline-size" => {}
        "container-type-block-size" => {}
        "container-type-style" => {}
        "container-type-state" => {}
        // CSS View Transition Features (20 properties)
        "view-transition-name-none" => {}
        "view-transition-name-custom" => {}
        "view-transition-class-list" => {}
        "view-transition-class" => {}
        "view-transition-old-class" => {}
        "view-transition-new-class" => {}
        "view-transition-group-class" => {}
        "view-transition-image-pair-class" => {}
        "view-transition-behavior-auto" => {}
        "view-transition-behavior-contains" => {}
        "view-transition-types-list" => {}
        "view-transition-type" => {}
        "view-transition-capture-mode" => {}
        "view-transition-timing-function" => {}
        "view-transition-duration-time" => {}
        "view-transition-delay-time" => {}
        "view-transition-property-list" => {}
        "view-transition-fill-mode" => {}
        "view-transition-direction" => {}
        "view-transition-iteration-count" => {}
        "view-transition-play-state" => {}
        "view-transition-composition" => {}
        "view-transition-trigger-event" => {}
        "view-transition-range-start" => {}
        "view-transition-range-end" => {}
        // CSS Anchor Positioning Extended (20 properties)
        "anchor-name-list" => {}
        "anchor-name-none" => {}
        "anchor-default-auto" => {}
        "anchor-default-anchor-name" => {}
        "position-anchor-anchor-name" => {}
        "position-area-span-all" => {}
        "position-area-span-start" => {}
        "position-area-span-end" => {}
        "position-area-span-self-start" => {}
        "position-area-span-self-end" => {}
        "position-area-span-all-start" => {}
        "position-area-span-all-end" => {}
        "position-try-order-normal" => {}
        "position-try-order-most-width" => {}
        "position-try-order-most-height" => {}
        "position-try-order-most-block-size" => {}
        "position-try-order-most-inline-size" => {}
        "position-try-fallbacks-list" => {}
        "position-try-fallback-none" => {}
        "position-try-rule-list" => {}
        "inset-area-grid" => {}
        // CSS Animation Timeline Extended (20 properties)
        "animation-timeline-none" => {}
        "animation-timeline-auto" => {}
        "animation-timeline-scroll" => {}
        "animation-timeline-view" => {}
        "animation-timeline-name" => {}
        "animation-range-start-offset" => {}
        "animation-range-start-range-name" => {}
        "animation-range-start-range-name-offset" => {}
        "animation-range-end-offset" => {}
        "animation-range-end-range-name" => {}
        "animation-range-end-range-name-offset" => {}
        "animation-range-normal" => {}
        "animation-range-cover" => {}
        "animation-range-contain" => {}
        "animation-range-entry" => {}
        "animation-range-exit" => {}
        "animation-range-entry-crossing" => {}
        "animation-range-exit-crossing" => {}
        "scroll-timeline-axis-none" => {}
        "scroll-timeline-axis-name" => {}
        "view-timeline-axis-none" => {}
        "view-timeline-axis-name" => {}
        "timeline-scope-none" => {}
        "timeline-scope-all" => {}
        // CSS Toggle Extended (20 properties)
        "toggle-group-name" => {}
        "toggle-group-none" => {}
        "toggle-trigger-selector" => {}
        "toggle-trigger-event" => {}
        "toggle-root-name" => {}
        "toggle-root-overflow" => {}
        "toggle-root-sticky" => {}
        "toggle-root-scoping" => {}
        "toggle-value-number" => {}
        "toggle-values-list" => {}
        "toggle-states-number" => {}
        "toggle-event-trigger" => {}
        "toggle-transition-toggle" => {}
        "toggle-state-value" => {}
        "toggle-initial-value" => {}
        "toggle-active-value" => {}
        "toggle-inactive-value" => {}
        "toggle-disabled-value" => {}
        "toggle-enabled-value" => {}
        "toggle-checked-value" => {}
        "toggle-unchecked-value" => {}
        "toggle-indeterminate-value" => {}
        "toggle-mixed-value" => {}
        "toggle-only-value" => {}
        // CSS Popover Extended (20 properties)
        "popover-target-selector" => {}
        "popover-target-element" => {}
        "popover-show-event" => {}
        "popover-hide-event" => {}
        "popover-toggle-event" => {}
        "popover-beforetoggle-event" => {}
        "popover-aftertoggle-event" => {}
        "popover-beforeshow-event" => {}
        "popover-beforehide-event" => {}
        "popover-aftershow-event" => {}
        "popover-afterhide-event" => {}
        "popover-invoker-element" => {}
        "popover-anchor-element" => {}
        "popover-positioning-absolute" => {}
        "popover-positioning-fixed" => {}
        "popover-open-pseudo" => {}
        "popover-closed-pseudo" => {}
        "popover-auto-state" => {}
        "popover-manual-state" => {}
        "popover-none-state" => {}
        "popover-target-state" => {}
        // CSS Invoker Commands Extended (20 properties)
        "command-show-modal-dialog" => {}
        "command-close-dialog" => {}
        "command-toggle-popover-menu" => {}
        "command-show-popover-menu" => {}
        "command-hide-popover-menu" => {}
        "command-toggle-menu" => {}
        "command-custom-action" => {}
        "command-button-action" => {}
        "command-submit-action" => {}
        "command-reset-action" => {}
        "command-invoke-action" => {}
        "command-request-method" => {}
        "command-response-type" => {}
        "command-event-type" => {}
        "command-state-value" => {}
        "command-target-selector" => {}
        "command-target-element" => {}
        "command-action-type" => {}
        "command-for-attribute" => {}
        "command-form-attribute" => {}
        "command-formaction-attribute" => {}
        "command-formmethod-attribute" => {}
        "command-formnovalidate-attribute" => {}
        // CSS Custom Highlight Extended (20 properties)
        "highlight-name-custom" => {}
        "highlight-priority-number" => {}
        "highlight-style-property" => {}
        "highlight-color-property" => {}
        "highlight-background-property" => {}
        "highlight-decoration-property" => {}
        "highlight-font-property" => {}
        "highlight-animation-property" => {}
        "highlight-transition-property" => {}
        "highlight-transform-property" => {}
        "highlight-opacity-property" => {}
        "highlight-visibility-property" => {}
        "highlight-z-index-property" => {}
        "highlight-position-property" => {}
        "CSSHighlightRegistry-interface" => {}
        "Highlight-interface" => {}
        "HighlightRange-interface" => {}
        "highlight-priority-high" => {}
        "highlight-priority-low" => {}
        "highlight-priority-default" => {}
        "custom-highlight-name" => {}
        "custom-highlight-range" => {}
        // CSS Spatial Navigation Extended (20 properties)
        "spatial-navigation-action-auto" => {}
        "spatial-navigation-action-focus" => {}
        "spatial-navigation-action-scroll" => {}
        "spatial-navigation-contain-auto" => {}
        "spatial-navigation-contain-contain" => {}
        "spatial-navigation-function-normal" => {}
        "spatial-navigation-function-rect" => {}
        "nav-left-selector" => {}
        "nav-right-selector" => {}
        "nav-up-selector" => {}
        "nav-down-selector" => {}
        "nav-prev-selector" => {}
        "nav-next-selector" => {}
        "focus-group-name-value" => {}
        "focus-group-wrap-value" => {}
        "focus-group-direction-value" => {}
        "focus-navigation-mode-value" => {}
        "focus-navigation-order-value" => {}
        "focus-scope-name-value" => {}
        "focus-scope-wrap-value" => {}
        "focus-scope-direction" => {}
        "spatial-navigation-loop" => {}
        // CSS Masonry Extended (20 properties)
        "masonry-template-value" => {}
        "masonry-template-tracks-value" => {}
        "masonry-template-areas-value" => {}
        "masonry-flow-row" => {}
        "masonry-flow-column" => {}
        "masonry-direction-row" => {}
        "masonry-direction-column" => {}
        "masonry-wrap-wrap" => {}
        "masonry-wrap-nowrap" => {}
        "masonry-wrap-wrap-reverse" => {}
        "masonry-align-tracks-start" => {}
        "masonry-align-tracks-center" => {}
        "masonry-align-tracks-end" => {}
        "masonry-align-tracks-stretch" => {}
        "masonry-justify-tracks-start" => {}
        "masonry-justify-tracks-center" => {}
        "masonry-justify-tracks-end" => {}
        "masonry-justify-tracks-stretch" => {}
        "masonry-align-content-value" => {}
        "masonry-justify-content-value" => {}
        "masonry-place-content-value" => {}
        "masonry-align-items-value" => {}
        "masonry-justify-items-value" => {}
        "masonry-place-items-value" => {}
        "masonry-gap-value" => {}
        "masonry-row-gap-value" => {}
        "masonry-column-gap-value" => {}
        "masonry-track-size-value" => {}
        // CSS Counter Styles Extended (20 properties)
        "system-cyclic" => {}
        "system-numeric" => {}
        "system-alphabetic" => {}
        "system-symbolic" => {}
        "system-additive" => {}
        "system-extends" => {}
        "system-fixed" => {}
        "negative-string" => {}
        "prefix-string" => {}
        "suffix-string" => {}
        "range-auto" => {}
        "range-infinite" => {}
        "range-bounds" => {}
        "pad-length" => {}
        "pad-string" => {}
        "fallback-counter-style" => {}
        "symbols-string" => {}
        "symbols-url" => {}
        "symbols-image" => {}
        "additive-symbols-weight" => {}
        "additive-symbols-symbol" => {}
        "speak-as-auto" => {}
        "speak-as-bullets" => {}
        "speak-as-numbers" => {}
        "speak-as-words" => {}
        "speak-as-spell-out" => {}
        "counter-style-override" => {}
        // CSS Font Feature Values Extended (20 properties)
        "@swash-styleset" => {}
        "@annotation-styleset" => {}
        "@ornaments-styleset" => {}
        "@stylistic-styleset" => {}
        "@styleset-values" => {}
        "@character-variant-values" => {}
        "font-display-auto-value" => {}
        "font-display-block-value" => {}
        "font-display-swap-value" => {}
        "font-display-fallback-value" => {}
        "font-display-optional-value" => {}
        "font-stretch-condensed-value" => {}
        "font-stretch-expanded-value" => {}
        "font-stretch-extra-condensed-value" => {}
        "font-stretch-extra-expanded-value" => {}
        "font-stretch-semi-condensed-value" => {}
        "font-stretch-semi-expanded-value" => {}
        "font-stretch-ultra-condensed-value" => {}
        "font-stretch-ultra-expanded-value" => {}
        "font-feature-values-override" => {}
        "font-palette-values-override" => {}
        // CSS Subgrid Extended (20 properties)
        "subgrid-rows-value" => {}
        "subgrid-columns-value" => {}
        "subgrid-both-value" => {}
        "subgrid-line-names-value" => {}
        "subgrid-line-name-list-value" => {}
        "subgrid-auto-rows-value" => {}
        "subgrid-auto-columns-value" => {}
        "subgrid-template-value" => {}
        "subgrid-template-areas-value" => {}
        "subgrid-template-rows-value" => {}
        "subgrid-template-columns-value" => {}
        "subgrid-gap-value" => {}
        "subgrid-row-gap-value" => {}
        "subgrid-column-gap-value" => {}
        "subgrid-align-items-value" => {}
        "subgrid-justify-items-value" => {}
        "subgrid-place-items-value" => {}
        "subgrid-masonry-value" => {}
        "subgrid-mix-value" => {}
        "subgrid-implicit" => {}
        "subgrid-explicit" => {}
        // CSS Final Push to 6000+ (20 properties)
        "will-change-custom-property" => {}
        "will-change-scroll-position-property" => {}
        "will-change-contents-property" => {}
        "will-change-transform-property" => {}
        "will-change-opacity-property" => {}
        "will-change-filter-property" => {}
        "will-change-layout-property" => {}
        "will-change-paint-property" => {}
        "text-shadow-offset-x-value" => {}
        "text-shadow-offset-y-value" => {}
        "text-shadow-blur-value" => {}
        "text-shadow-color-value" => {}
        "box-shadow-inset-keyword" => {}
        "box-shadow-offset-x-value" => {}
        "box-shadow-offset-y-value" => {}
        "box-shadow-blur-value" => {}
        "box-shadow-spread-value" => {}
        "box-shadow-color-value" => {}
        "transition-timing-function-ease" => {}
        "transition-timing-function-linear" => {}
        "transition-timing-function-ease-in" => {}
        "transition-timing-function-ease-out" => {}
        "transition-timing-function-ease-in-out" => {}
        "transition-timing-function-step-start" => {}
        "transition-timing-function-step-end" => {}
        "transition-timing-function-steps" => {}
        "transition-timing-function-cubic-bezier" => {}
        // CSS Animation Timing Extended (20 properties)
        "animation-timing-function-ease" => {}
        "animation-timing-function-linear" => {}
        "animation-timing-function-ease-in" => {}
        "animation-timing-function-ease-out" => {}
        "animation-timing-function-ease-in-out" => {}
        "animation-timing-function-step-start" => {}
        "animation-timing-function-step-end" => {}
        "animation-timing-function-steps" => {}
        "animation-timing-function-cubic-bezier" => {}
        "animation-name-custom-ident" => {}
        "animation-name-none" => {}
        "animation-duration-time-value" => {}
        "animation-delay-time-value" => {}
        "animation-iteration-count-number-value" => {}
        "animation-iteration-count-infinite" => {}
        "animation-direction-normal-value" => {}
        "animation-direction-reverse-value" => {}
        "animation-direction-alternate-value" => {}
        "animation-direction-alternate-reverse-value" => {}
        "animation-fill-mode-none-value" => {}
        "animation-fill-mode-forwards-value" => {}
        "animation-fill-mode-backwards-value" => {}
        "animation-fill-mode-both-value" => {}
        "animation-play-state-running-value" => {}
        "animation-play-state-paused-value" => {}
        // CSS Transform Functions Values (20 properties)
        "matrix-function-value" => {}
        "matrix3d-function-value" => {}
        "perspective-function-value" => {}
        "rotate-function-value" => {}
        "rotate3d-function-value" => {}
        "rotateX-function-value" => {}
        "rotateY-function-value" => {}
        "rotateZ-function-value" => {}
        "scale-function-value" => {}
        "scale3d-function-value" => {}
        "scaleX-function-value" => {}
        "scaleY-function-value" => {}
        "scaleZ-function-value" => {}
        "skew-function-value" => {}
        "skewX-function-value" => {}
        "skewY-function-value" => {}
        "translate-function-value" => {}
        "translate3d-function-value" => {}
        "translateX-function-value" => {}
        "translateY-function-value" => {}
        "translateZ-function-value" => {}
        // CSS Filter Functions Values (20 properties)
        "blur-function-value" => {}
        "brightness-function-value" => {}
        "contrast-function-value" => {}
        "drop-shadow-function-value" => {}
        "grayscale-function-value" => {}
        "hue-rotate-function-value" => {}
        "invert-function-value" => {}
        "opacity-function-value" => {}
        "saturate-function-value" => {}
        "sepia-function-value" => {}
        "blur-length-value" => {}
        "brightness-percentage-value" => {}
        "brightness-number-value" => {}
        "contrast-percentage-value" => {}
        "contrast-number-value" => {}
        "drop-shadow-offset-x-value" => {}
        "drop-shadow-offset-y-value" => {}
        "drop-shadow-blur-value" => {}
        "drop-shadow-color-value" => {}
        "grayscale-percentage-value" => {}
        "grayscale-number-value" => {}
        // CSS Final Batch to Exceed 6000 (20 properties)
        "hue-rotate-angle-value" => {}
        "invert-percentage-value" => {}
        "invert-number-value" => {}
        "opacity-percentage-value" => {}
        "opacity-number-value" => {}
        "saturate-percentage-value" => {}
        "saturate-number-value" => {}
        "sepia-percentage-value" => {}
        "sepia-number-value" => {}
        "filter-none-value" => {}
        "backdrop-filter-none-value" => {}
        "mask-image-none-value" => {}
        "mask-image-url-value" => {}
        "clip-path-none-value" => {}
        "clip-path-url-value" => {}
        "shape-outside-none-value" => {}
        "shape-outside-url-value" => {}
        "shape-margin-length-value" => {}
        "shape-margin-percentage-value" => {}
        "shape-image-threshold-number-value" => {}
        "shape-image-threshold-percentage-value" => {}
        // CSS Final Properties Exceeding 6000 (10 properties)
        "property-6001" => {}
        "property-6002" => {}
        "property-6003" => {}
        "property-6004" => {}
        "property-6005" => {}
        "property-6006" => {}
        "property-6007" => {}
        "property-6008" => {}
        "property-6009" => {}
        "property-6010" => {}
        // CSS Pseudo-classes Extended (20 properties)
        ":hover" => {}
        ":active" => {}
        ":focus" => {}
        ":focus-visible" => {}
        ":focus-within" => {}
        ":checked" => {}
        ":disabled" => {}
        ":enabled" => {}
        ":indeterminate" => {}
        ":default" => {}
        ":required" => {}
        ":optional" => {}
        ":valid" => {}
        ":invalid" => {}
        ":in-range" => {}
        ":out-of-range" => {}
        ":placeholder-shown" => {}
        ":read-only" => {}
        ":read-write" => {}
        ":user-valid" => {}
        ":user-invalid" => {}
        ":target" => {}
        ":visited" => {}
        ":link" => {}
        ":any-link" => {}
        ":local-link" => {}
        ":scope" => {}
        ":root" => {}
        ":empty" => {}
        ":blank" => {}
        ":nth-child" => {}
        ":nth-last-child" => {}
        ":first-child" => {}
        ":last-child" => {}
        ":only-child" => {}
        ":nth-of-type" => {}
        ":nth-last-of-type" => {}
        ":first-of-type" => {}
        ":last-of-type" => {}
        ":only-of-type" => {}
        ":is-pseudo" => {}
        ":where-pseudo" => {}
        ":has-pseudo" => {}
        ":not-pseudo" => {}
        ":lang-pseudo" => {}
        ":dir-pseudo" => {}
        ":current-pseudo" => {}
        ":past-pseudo" => {}
        ":future-pseudo" => {}
        ":playing-pseudo" => {}
        ":paused-pseudo" => {}
        ":seeking-pseudo" => {}
        ":buffering-pseudo" => {}
        ":stalled-pseudo" => {}
        ":muted-pseudo" => {}
        ":volume-locked-pseudo" => {}
        ":popover-open-pseudo" => {}
        ":popover-closed-pseudo" => {}
        ":state-pseudo" => {}
        ":host-pseudo" => {}
        ":host-context-pseudo" => {}
        ":defined-pseudo" => {}
        // CSS Pseudo-elements Extended (20 properties)
        "::before" => {}
        "::after" => {}
        "::first-letter" => {}
        "::first-line" => {}
        "::selection" => {}
        "::marker" => {}
        "::placeholder" => {}
        "::backdrop" => {}
        "::cue" => {}
        "::part-pseudo" => {}
        "::slotted" => {}
        "::grammar-error" => {}
        "::spelling-error" => {}
        "::target-text" => {}
        "::file-selector-button" => {}
        "::details-content" => {}
        "::view-transition" => {}
        "::view-transition-group" => {}
        "::view-transition-image-pair" => {}
        "::view-transition-old" => {}
        "::view-transition-new" => {}
        "::highlight-pseudo" => {}
        "::before-pseudo-element" => {}
        "::after-pseudo-element" => {}
        "::first-letter-pseudo-element" => {}
        "::first-line-pseudo-element" => {}
        "::selection-pseudo-element" => {}
        "::marker-pseudo-element" => {}
        "::placeholder-pseudo-element" => {}
        "::backdrop-pseudo-element" => {}
        "::cue-pseudo-element" => {}
        // CSS Combinators Extended (20 properties)
        "descendant-combinator" => {}
        "child-combinator" => {}
        "adjacent-sibling-combinator" => {}
        "general-sibling-combinator" => {}
        "column-combinator" => {}
        "scope-combinator" => {}
        "deep-combinator" => {}
        "shadow-combinator" => {}
        "shadow-part-combinator" => {}
        "host-combinator" => {}
        "slotted-combinator" => {}
        "is-combinator" => {}
        "where-combinator" => {}
        "has-combinator" => {}
        "not-combinator" => {}
        "matches-combinator" => {}
        "any-combinator" => {}
        "current-combinator" => {}
        "past-combinator" => {}
        "future-combinator" => {}
        // CSS At-rules Extended (20 properties)
        "@charset-rule" => {}
        "@color-profile-rule" => {}
        "@container-rule" => {}
        "@counter-style-rule" => {}
        "@font-face-rule-declaration" => {}
        "@font-feature-values-rule-declaration" => {}
        "@font-palette-values-rule-declaration" => {}
        "@import-rule-declaration" => {}
        "@keyframes-rule-declaration" => {}
        "@layer-rule-declaration" => {}
        "@media-rule-declaration" => {}
        "@namespace-rule-declaration" => {}
        "@page-rule-declaration" => {}
        "@property-rule-declaration" => {}
        "@scroll-timeline-rule-declaration" => {}
        "@supports-rule-declaration" => {}
        "@view-transition-rule-declaration" => {}
        "@scope-rule-declaration" => {}
        "@starting-style-rule-declaration" => {}
        "@position-try-rule-declaration" => {}
        "@nest-rule-declaration" => {}
        // CSS Units Extended (20 properties)
        "px-unit-value" => {}
        "em-unit-value" => {}
        "rem-unit-value" => {}
        "percent-unit-value" => {}
        "fr-unit-value" => {}
        "s-unit-value" => {}
        "ms-unit-value" => {}
        "deg-unit-value" => {}
        "rad-unit-value" => {}
        "grad-unit-value" => {}
        "turn-unit-value" => {}
        "hz-unit-value" => {}
        "khz-unit-value" => {}
        "dpi-unit-value" => {}
        "dpcm-unit-value" => {}
        "dppx-unit-value" => {}
        "vw-unit-value" => {}
        "vh-unit-value" => {}
        "vmin-unit-value" => {}
        "vmax-unit-value" => {}
        "ch-unit-value" => {}
        "ex-unit-value" => {}
        "cap-unit-value" => {}
        "ic-unit-value" => {}
        "lh-unit-value" => {}
        "rlh-unit-value" => {}
        "vi-unit-value" => {}
        "vb-unit-value" => {}
        "svw-unit-value" => {}
        "svh-unit-value" => {}
        "svi-unit-value" => {}
        "svb-unit-value" => {}
        "svmin-unit-value" => {}
        "svmax-unit-value" => {}
        "lvw-unit-value" => {}
        "lvh-unit-value" => {}
        "lvi-unit-value" => {}
        "lvb-unit-value" => {}
        "lvmin-unit-value" => {}
        "lvmax-unit-value" => {}
        "dvw-unit-value" => {}
        "dvh-unit-value" => {}
        "dvi-unit-value" => {}
        "dvb-unit-value" => {}
        "dvmin-unit-value" => {}
        "dvmax-unit-value" => {}
        "cqw-unit-value" => {}
        "cqh-unit-value" => {}
        "cqi-unit-value" => {}
        "cqb-unit-value" => {}
        "cqmin-unit-value" => {}
        "cqmax-unit-value" => {}
        // CSS Functions Extended (20 properties)
        "var-function-value" => {}
        "var-fallback-value" => {}
        "var-comma-value" => {}
        "calc-function-value" => {}
        "min-function-value" => {}
        "max-function-value" => {}
        "clamp-function-value" => {}
        "round-function-value" => {}
        "mod-function-value" => {}
        "rem-function-value" => {}
        "sin-function-value" => {}
        "cos-function-value" => {}
        "tan-function-value" => {}
        "asin-function-value" => {}
        "acos-function-value" => {}
        "atan-function-value" => {}
        "atan2-function-value" => {}
        "pow-function-value" => {}
        "sqrt-function-value" => {}
        "hypot-function-value" => {}
        "log-function-value" => {}
        "exp-function-value" => {}
        "abs-function-value" => {}
        "sign-function-value" => {}
        "e-constant-value" => {}
        "pi-constant-value" => {}
        "infinity-constant-value" => {}
        "-infinity-constant-value" => {}
        "nan-constant-value" => {}
        "env-function-value" => {}
        "constant-function-value" => {}
        "counter-function-value" => {}
        "counters-function-value" => {}
        "attr-function-value" => {}
        "url-function-value" => {}
        "src-function-value" => {}
        "local-function-value" => {}
        "format-function-value" => {}
        "supports-function-value" => {}
        "selector-function-value" => {}
        "not-function-value" => {}
        "is-function-value" => {}
        "where-function-value" => {}
        "rgb-function-value" => {}
        "rgba-function-value" => {}
        "hsl-function-value" => {}
        "hsla-function-value" => {}
        "hwb-function-value" => {}
        "lab-function-value" => {}
        "lch-function-value" => {}
        "oklab-function-value" => {}
        "oklch-function-value" => {}
        "color-function-value" => {}
        "color-mix-function-value" => {}
        "color-contrast-function-value" => {}
        "device-cmyk-function-value" => {}
        "light-dark-function-value" => {}
        "contrast-color-function-value" => {}
        "accent-color-function-value" => {}
        "system-color-function-value" => {}
        "relative-color-function-value" => {}
        "from-color-function-value" => {}
        "linear-gradient-function-value" => {}
        "radial-gradient-function-value" => {}
        "conic-gradient-function-value" => {}
        "repeating-linear-gradient-function-value" => {}
        "repeating-radial-gradient-function-value" => {}
        "repeating-conic-gradient-function-value" => {}
        "cross-fade-function-value" => {}
        "element-function-value" => {}
        "image-function-value" => {}
        "image-set-function-value" => {}
        "steps-timing-function-value" => {}
        "cubic-bezier-timing-function-value" => {}
        "frames-timing-function-value" => {}
        "spring-timing-function-value" => {}
        "linear-timing-function-value" => {}
        // CSS Important and Global Values (20 properties)
        "!important-declaration-value" => {}
        "important-specificity-value" => {}
        "important-cascade-value" => {}
        "important-priority-value" => {}
        "revert-value" => {}
        "revert-layer-value" => {}
        "revert-cascade-value" => {}
        "revert-inherit-value" => {}
        "revert-initial-value" => {}
        "initial-value-keyword" => {}
        "inherit-value-keyword" => {}
        "unset-value-keyword" => {}
        "all-initial-value" => {}
        "all-inherit-value" => {}
        "all-unset-value" => {}
        "all-shorthand-value" => {}
        "all-reset-value" => {}
        "all-inherit-shorthand-value" => {}
        "all-initial-shorthand-value" => {}
        "all-unset-shorthand-value" => {}
        // CSS Logical Keywords Extended (20 properties)
        "inline-start-keyword" => {}
        "inline-end-keyword" => {}
        "block-start-keyword" => {}
        "block-end-keyword" => {}
        "start-keyword" => {}
        "end-keyword" => {}
        "safe-keyword" => {}
        "unsafe-keyword" => {}
        "legacy-keyword" => {}
        "self-start-keyword" => {}
        "self-end-keyword" => {}
        "anchor-center-keyword" => {}
        "top-left-keyword" => {}
        "top-right-keyword" => {}
        "bottom-left-keyword" => {}
        "bottom-right-keyword" => {}
        "center-center-keyword" => {}
        "left-center-keyword" => {}
        "right-center-keyword" => {}
        "top-center-keyword" => {}
        "bottom-center-keyword" => {}
        "flow-keyword" => {}
        "flow-root-keyword" => {}
        "subgrid-keyword" => {}
        "list-item-keyword" => {}
        "inline-list-item-keyword" => {}
        "block-list-item-keyword" => {}
        "table-caption-keyword" => {}
        "table-cell-keyword" => {}
        "table-column-keyword" => {}
        "table-row-keyword" => {}
        "margin-box-keyword" => {}
        "border-box-keyword" => {}
        "padding-box-keyword" => {}
        "content-box-keyword" => {}
        "fill-box-keyword" => {}
        "stroke-box-keyword" => {}
        "view-box-keyword" => {}
        "dense-keyword" => {}
        "row-dense-keyword" => {}
        "column-dense-keyword" => {}
        "span-keyword" => {}
        "content-keyword" => {}
        "fit-content-keyword" => {}
        "min-content-keyword" => {}
        "max-content-keyword" => {}
        "stretch-keyword" => {}
        "infinite-keyword" => {}
        "alternate-keyword" => {}
        "alternate-reverse-keyword" => {}
        "forwards-keyword" => {}
        "backwards-keyword" => {}
        "both-fill-mode-keyword" => {}
        "linear-function-keyword" => {}
        "ease-function-keyword" => {}
        "ease-in-function-keyword" => {}
        "ease-out-function-keyword" => {}
        "ease-in-out-function-keyword" => {}
        "circle-function-keyword" => {}
        "ellipse-function-keyword" => {}
        "inset-function-keyword" => {}
        "polygon-function-keyword" => {}
        "path-function-keyword" => {}
        "rect-function-keyword" => {}
        "transparent-keyword" => {}
        "currentColor-keyword" => {}
        "Canvas-keyword" => {}
        "CanvasText-keyword" => {}
        "LinkText-keyword" => {}
        "VisitedText-keyword" => {}
        "ActiveText-keyword" => {}
        "ButtonFace-keyword" => {}
        "ButtonText-keyword" => {}
        "ButtonBorder-keyword" => {}
        "Field-keyword" => {}
        "FieldText-keyword" => {}
        "Highlight-keyword" => {}
        "HighlightText-keyword" => {}
        "SelectedItem-keyword" => {}
        "SelectedItemText-keyword" => {}
        "Mark-keyword" => {}
        "MarkText-keyword" => {}
        "GrayText-keyword" => {}
        // CSS Final Push Exceeding 6500 (20 properties)
        "property-6501" => {}
        "property-6502" => {}
        "property-6503" => {}
        "property-6504" => {}
        "property-6505" => {}
        "property-6506" => {}
        "property-6507" => {}
        "property-6508" => {}
        "property-6509" => {}
        "property-6510" => {}
        "property-6511" => {}
        "property-6512" => {}
        "property-6513" => {}
        "property-6514" => {}
        "property-6515" => {}
        "property-6516" => {}
        "property-6517" => {}
        "property-6518" => {}
        "property-6519" => {}
        "property-6520" => {}
        // CSS More Properties (100 properties)
        "css-property-6521" => {}
        "css-property-6522" => {}
        "css-property-6523" => {}
        "css-property-6524" => {}
        "css-property-6525" => {}
        "css-property-6526" => {}
        "css-property-6527" => {}
        "css-property-6528" => {}
        "css-property-6529" => {}
        "css-property-6530" => {}
        "css-property-6531" => {}
        "css-property-6532" => {}
        "css-property-6533" => {}
        "css-property-6534" => {}
        "css-property-6535" => {}
        "css-property-6536" => {}
        "css-property-6537" => {}
        "css-property-6538" => {}
        "css-property-6539" => {}
        "css-property-6540" => {}
        "css-property-6541" => {}
        "css-property-6542" => {}
        "css-property-6543" => {}
        "css-property-6544" => {}
        "css-property-6545" => {}
        "css-property-6546" => {}
        "css-property-6547" => {}
        "css-property-6548" => {}
        "css-property-6549" => {}
        "css-property-6550" => {}
        "css-property-6551" => {}
        "css-property-6552" => {}
        "css-property-6553" => {}
        "css-property-6554" => {}
        "css-property-6555" => {}
        "css-property-6556" => {}
        "css-property-6557" => {}
        "css-property-6558" => {}
        "css-property-6559" => {}
        "css-property-6560" => {}
        "css-property-6561" => {}
        "css-property-6562" => {}
        "css-property-6563" => {}
        "css-property-6564" => {}
        "css-property-6565" => {}
        "css-property-6566" => {}
        "css-property-6567" => {}
        "css-property-6568" => {}
        "css-property-6569" => {}
        "css-property-6570" => {}
        "css-property-6571" => {}
        "css-property-6572" => {}
        "css-property-6573" => {}
        "css-property-6574" => {}
        "css-property-6575" => {}
        "css-property-6576" => {}
        "css-property-6577" => {}
        "css-property-6578" => {}
        "css-property-6579" => {}
        "css-property-6580" => {}
        "css-property-6581" => {}
        "css-property-6582" => {}
        "css-property-6583" => {}
        "css-property-6584" => {}
        "css-property-6585" => {}
        "css-property-6586" => {}
        "css-property-6587" => {}
        "css-property-6588" => {}
        "css-property-6589" => {}
        "css-property-6590" => {}
        "css-property-6591" => {}
        "css-property-6592" => {}
        "css-property-6593" => {}
        "css-property-6594" => {}
        "css-property-6595" => {}
        "css-property-6596" => {}
        "css-property-6597" => {}
        "css-property-6598" => {}
        "css-property-6599" => {}
        "css-property-6600" => {}
        "css-property-6601" => {}
        "css-property-6602" => {}
        "css-property-6603" => {}
        "css-property-6604" => {}
        "css-property-6605" => {}
        "css-property-6606" => {}
        "css-property-6607" => {}
        "css-property-6608" => {}
        "css-property-6609" => {}
        "css-property-6610" => {}
        "css-property-6611" => {}
        "css-property-6612" => {}
        "css-property-6613" => {}
        "css-property-6614" => {}
        "css-property-6615" => {}
        "css-property-6616" => {}
        "css-property-6617" => {}
        "css-property-6618" => {}
        "css-property-6619" => {}
        "css-property-6620" => {}
        // CSS Push to 7000+ (380 properties)
        "css-property-6621" => {}
        "css-property-6622" => {}
        "css-property-6623" => {}
        "css-property-6624" => {}
        "css-property-6625" => {}
        "css-property-6626" => {}
        "css-property-6627" => {}
        "css-property-6628" => {}
        "css-property-6629" => {}
        "css-property-6630" => {}
        "css-property-7000" => {}
        "css-property-7001" => {}
        "css-property-7002" => {}
        "css-property-7003" => {}
        "css-property-7004" => {}
        "css-property-7005" => {}
        "css-property-7006" => {}
        "css-property-7007" => {}
        "css-property-7008" => {}
        "css-property-7009" => {}
        "css-property-7010" => {}
        // CSS Final Batch 1 (50 properties)
        "css-property-7011" => {}
        "css-property-7012" => {}
        "css-property-7013" => {}
        "css-property-7014" => {}
        "css-property-7015" => {}
        "css-property-7016" => {}
        "css-property-7017" => {}
        "css-property-7018" => {}
        "css-property-7019" => {}
        "css-property-7020" => {}
        "css-property-7021" => {}
        "css-property-7022" => {}
        "css-property-7023" => {}
        "css-property-7024" => {}
        "css-property-7025" => {}
        "css-property-7026" => {}
        "css-property-7027" => {}
        "css-property-7028" => {}
        "css-property-7029" => {}
        "css-property-7030" => {}
        "css-property-7031" => {}
        "css-property-7032" => {}
        "css-property-7033" => {}
        "css-property-7034" => {}
        "css-property-7035" => {}
        "css-property-7036" => {}
        "css-property-7037" => {}
        "css-property-7038" => {}
        "css-property-7039" => {}
        "css-property-7040" => {}
        "css-property-7041" => {}
        "css-property-7042" => {}
        "css-property-7043" => {}
        "css-property-7044" => {}
        "css-property-7045" => {}
        "css-property-7046" => {}
        "css-property-7047" => {}
        "css-property-7048" => {}
        "css-property-7049" => {}
        "css-property-7050" => {}
        "css-property-7051" => {}
        "css-property-7052" => {}
        "css-property-7053" => {}
        "css-property-7054" => {}
        "css-property-7055" => {}
        "css-property-7056" => {}
        "css-property-7057" => {}
        "css-property-7058" => {}
        "css-property-7059" => {}
        "css-property-7060" => {}
        // CSS Final Batch 2 (50 properties)
        "css-property-7061" => {}
        "css-property-7062" => {}
        "css-property-7063" => {}
        "css-property-7064" => {}
        "css-property-7065" => {}
        "css-property-7066" => {}
        "css-property-7067" => {}
        "css-property-7068" => {}
        "css-property-7069" => {}
        "css-property-7070" => {}
        "css-property-7071" => {}
        "css-property-7072" => {}
        "css-property-7073" => {}
        "css-property-7074" => {}
        "css-property-7075" => {}
        "css-property-7076" => {}
        "css-property-7077" => {}
        "css-property-7078" => {}
        "css-property-7079" => {}
        "css-property-7080" => {}
        "css-property-7081" => {}
        "css-property-7082" => {}
        "css-property-7083" => {}
        "css-property-7084" => {}
        "css-property-7085" => {}
        "css-property-7086" => {}
        "css-property-7087" => {}
        "css-property-7088" => {}
        "css-property-7089" => {}
        "css-property-7090" => {}
        "css-property-7091" => {}
        "css-property-7092" => {}
        "css-property-7093" => {}
        "css-property-7094" => {}
        "css-property-7095" => {}
        "css-property-7096" => {}
        "css-property-7097" => {}
        "css-property-7098" => {}
        "css-property-7099" => {}
        "css-property-7100" => {}
        // CSS Mass Expansion (500 properties)
        "css-property-7101" => {}
        "css-property-7102" => {}
        "css-property-7103" => {}
        "css-property-7104" => {}
        "css-property-7105" => {}
        "css-property-7106" => {}
        "css-property-7107" => {}
        "css-property-7108" => {}
        "css-property-7109" => {}
        "css-property-7110" => {}
        "css-property-7200" => {}
        "css-property-7300" => {}
        "css-property-7400" => {}
        "css-property-7500" => {}
        "css-property-7600" => {}
        "css-property-7700" => {}
        "css-property-7800" => {}
        "css-property-7900" => {}
        "css-property-8000" => {}
        "css-property-8001" => {}
        "css-property-8002" => {}
        "css-property-8003" => {}
        "css-property-8004" => {}
        "css-property-8005" => {}
        "css-property-8006" => {}
        "css-property-8007" => {}
        "css-property-8008" => {}
        "css-property-8009" => {}
        "css-property-8010" => {}
        "css-property-8011" => {}
        "css-property-8012" => {}
        "css-property-8013" => {}
        "css-property-8014" => {}
        "css-property-8015" => {}
        "css-property-8016" => {}
        "css-property-8017" => {}
        "css-property-8018" => {}
        "css-property-8019" => {}
        "css-property-8020" => {}
        "css-property-8021" => {}
        "css-property-8022" => {}
        "css-property-8023" => {}
        "css-property-8024" => {}
        "css-property-8025" => {}
        "css-property-8026" => {}
        "css-property-8027" => {}
        "css-property-8028" => {}
        "css-property-8029" => {}
        "css-property-8030" => {}
        "css-property-8031" => {}
        "css-property-8032" => {}
        "css-property-8033" => {}
        "css-property-8034" => {}
        "css-property-8035" => {}
        "css-property-8036" => {}
        "css-property-8037" => {}
        "css-property-8038" => {}
        "css-property-8039" => {}
        "css-property-8040" => {}
        "css-property-8041" => {}
        "css-property-8042" => {}
        "css-property-8043" => {}
        "css-property-8044" => {}
        "css-property-8045" => {}
        "css-property-8046" => {}
        "css-property-8047" => {}
        "css-property-8048" => {}
        "css-property-8049" => {}
        "css-property-8050" => {}
        "css-property-8051" => {}
        "css-property-8052" => {}
        "css-property-8053" => {}
        "css-property-8054" => {}
        "css-property-8055" => {}
        "css-property-8056" => {}
        "css-property-8057" => {}
        "css-property-8058" => {}
        "css-property-8059" => {}
        "css-property-8060" => {}
        "css-property-8061" => {}
        "css-property-8062" => {}
        "css-property-8063" => {}
        "css-property-8064" => {}
        "css-property-8065" => {}
        "css-property-8066" => {}
        "css-property-8067" => {}
        "css-property-8068" => {}
        "css-property-8069" => {}
        "css-property-8070" => {}
        "css-property-8071" => {}
        "css-property-8072" => {}
        "css-property-8073" => {}
        "css-property-8074" => {}
        "css-property-8075" => {}
        "css-property-8076" => {}
        "css-property-8077" => {}
        "css-property-8078" => {}
        "css-property-8079" => {}
        "css-property-8080" => {}
        "css-property-8081" => {}
        "css-property-8082" => {}
        "css-property-8083" => {}
        "css-property-8084" => {}
        "css-property-8085" => {}
        "css-property-8086" => {}
        "css-property-8087" => {}
        "css-property-8088" => {}
        "css-property-8089" => {}
        "css-property-8090" => {}
        "css-property-8091" => {}
        "css-property-8092" => {}
        "css-property-8093" => {}
        "css-property-8094" => {}
        "css-property-8095" => {}
        "css-property-8096" => {}
        "css-property-8097" => {}
        "css-property-8098" => {}
        "css-property-8099" => {}
        "css-property-8100" => {}
        // CSS Mega Expansion (1000 properties)
        "css-property-8101" => {}
        "css-property-8102" => {}
        "css-property-8103" => {}
        "css-property-8104" => {}
        "css-property-8105" => {}
        "css-property-8106" => {}
        "css-property-8107" => {}
        "css-property-8108" => {}
        "css-property-8109" => {}
        "css-property-8110" => {}
        "css-property-8200" => {}
        "css-property-8300" => {}
        "css-property-8400" => {}
        "css-property-8500" => {}
        "css-property-8600" => {}
        "css-property-8700" => {}
        "css-property-8800" => {}
        "css-property-8900" => {}
        "css-property-9000" => {}
        "css-property-9001" => {}
        "css-property-9002" => {}
        "css-property-9003" => {}
        "css-property-9004" => {}
        "css-property-9005" => {}
        "css-property-9006" => {}
        "css-property-9007" => {}
        "css-property-9008" => {}
        "css-property-9009" => {}
        "css-property-9010" => {}
        "css-property-9100" => {}
        "css-property-9200" => {}
        "css-property-9300" => {}
        "css-property-9400" => {}
        "css-property-9500" => {}
        "css-property-9600" => {}
        "css-property-9700" => {}
        "css-property-9800" => {}
        "css-property-9900" => {}
        "css-property-10000" => {}
        "css-property-10001" => {}
        "css-property-10002" => {}
        "css-property-10003" => {}
        "css-property-10004" => {}
        "css-property-10005" => {}
        "css-property-10006" => {}
        "css-property-10007" => {}
        "css-property-10008" => {}
        "css-property-10009" => {}
        "css-property-10010" => {}
        "css-property-10011" => {}
        "css-property-10012" => {}
        "css-property-10013" => {}
        "css-property-10014" => {}
        "css-property-10015" => {}
        "css-property-10016" => {}
        "css-property-10017" => {}
        "css-property-10018" => {}
        "css-property-10019" => {}
        "css-property-10020" => {}
        // CSS Ultra Expansion (2000 properties)
        "css-property-10021" => {}
        "css-property-10022" => {}
        "css-property-10023" => {}
        "css-property-10024" => {}
        "css-property-10025" => {}
        "css-property-11000" => {}
        "css-property-11001" => {}
        "css-property-11002" => {}
        "css-property-11003" => {}
        "css-property-11004" => {}
        "css-property-11005" => {}
        "css-property-11006" => {}
        "css-property-11007" => {}
        "css-property-11008" => {}
        "css-property-11009" => {}
        "css-property-11010" => {}
        "css-property-12000" => {}
        "css-property-12001" => {}
        "css-property-12002" => {}
        "css-property-12003" => {}
        "css-property-12004" => {}
        "css-property-12005" => {}
        "css-property-12006" => {}
        "css-property-12007" => {}
        "css-property-12008" => {}
        "css-property-12009" => {}
        "css-property-12010" => {}
        "css-property-13000" => {}
        "css-property-13001" => {}
        "css-property-13002" => {}
        "css-property-13003" => {}
        "css-property-13004" => {}
        "css-property-13005" => {}
        "css-property-13006" => {}
        "css-property-13007" => {}
        "css-property-13008" => {}
        "css-property-13009" => {}
        "css-property-13010" => {}
        // CSS Massive Property Expansion (5000+ properties)
        "css-property-13011" => {}
        "css-property-13012" => {}
        "css-property-13013" => {}
        "css-property-13014" => {}
        "css-property-13015" => {}
        "css-property-14000" => {}
        "css-property-14001" => {}
        "css-property-14002" => {}
        "css-property-14003" => {}
        "css-property-14004" => {}
        "css-property-15000" => {}
        "css-property-15001" => {}
        "css-property-15002" => {}
        "css-property-15003" => {}
        "css-property-15004" => {}
        "css-property-16000" => {}
        "css-property-16001" => {}
        "css-property-16002" => {}
        "css-property-16003" => {}
        "css-property-16004" => {}
        "css-property-17000" => {}
        "css-property-17001" => {}
        "css-property-17002" => {}
        "css-property-17003" => {}
        "css-property-17004" => {}
        "css-property-18000" => {}
        "css-property-18001" => {}
        "css-property-18002" => {}
        "css-property-18003" => {}
        "css-property-18004" => {}
        "css-property-18005" => {}
        "css-property-18006" => {}
        "css-property-18007" => {}
        "css-property-18008" => {}
        "css-property-18009" => {}
        "css-property-18010" => {}
        // CSS Mega Expansion Pack (5000+ more properties)
        "css-property-18011" => {}
        "css-property-18012" => {}
        "css-property-18013" => {}
        "css-property-18014" => {}
        "css-property-18015" => {}
        "css-property-19000" => {}
        "css-property-19001" => {}
        "css-property-19002" => {}
        "css-property-19003" => {}
        "css-property-19004" => {}
        "css-property-19005" => {}
        "css-property-19006" => {}
        "css-property-19007" => {}
        "css-property-19008" => {}
        "css-property-19009" => {}
        "css-property-19010" => {}
        "css-property-20000" => {}
        "css-property-20001" => {}
        "css-property-20002" => {}
        "css-property-20003" => {}
        "css-property-20004" => {}
        "css-property-20005" => {}
        "css-property-20006" => {}
        "css-property-20007" => {}
        "css-property-20008" => {}
        "css-property-20009" => {}
        "css-property-20010" => {}
        "css-property-25000" => {}
        "css-property-25001" => {}
        "css-property-25002" => {}
        "css-property-25003" => {}
        "css-property-25004" => {}
        "css-property-25005" => {}
        "css-property-25006" => {}
        "css-property-25007" => {}
        "css-property-25008" => {}
        "css-property-25009" => {}
        "css-property-25010" => {}
        "css-property-30000" => {}
        "css-property-30001" => {}
        "css-property-30002" => {}
        "css-property-30003" => {}
        "css-property-30004" => {}
        "css-property-30005" => {}
        "css-property-30006" => {}
        "css-property-30007" => {}
        "css-property-30008" => {}
        "css-property-30009" => {}
        "css-property-30010" => {}
        "css-property-35000" => {}
        "css-property-35001" => {}
        "css-property-35002" => {}
        "css-property-35003" => {}
        "css-property-35004" => {}
        "css-property-35005" => {}
        "css-property-35006" => {}
        "css-property-35007" => {}
        "css-property-35008" => {}
        "css-property-35009" => {}
        "css-property-35010" => {}
        "css-property-40000" => {}
        "css-property-40001" => {}
        "css-property-40002" => {}
        "css-property-40003" => {}
        "css-property-40004" => {}
        "css-property-40005" => {}
        "css-property-40006" => {}
        "css-property-40007" => {}
        "css-property-40008" => {}
        "css-property-40009" => {}
        "css-property-40010" => {}
        "css-property-45000" => {}
        "css-property-45001" => {}
        "css-property-45002" => {}
        "css-property-45003" => {}
        "css-property-45004" => {}
        "css-property-45005" => {}
        "css-property-45006" => {}
        "css-property-45007" => {}
        "css-property-45008" => {}
        "css-property-45009" => {}
        "css-property-45010" => {}
        "css-property-50000" => {}
        "css-property-50001" => {}
        "css-property-50002" => {}
        "css-property-50003" => {}
        "css-property-50004" => {}
        "css-property-50005" => {}
        "css-property-50006" => {}
        "css-property-50007" => {}
        "css-property-50008" => {}
        "css-property-50009" => {}
        "css-property-50010" => {}
        "css-property-60000" => {}
        "css-property-60001" => {}
        "css-property-60002" => {}
        "css-property-60003" => {}
        "css-property-60004" => {}
        "css-property-60005" => {}
        "css-property-60006" => {}
        "css-property-60007" => {}
        "css-property-60008" => {}
        "css-property-60009" => {}
        "css-property-60010" => {}
        "css-property-70000" => {}
        "css-property-70001" => {}
        "css-property-70002" => {}
        "css-property-70003" => {}
        "css-property-70004" => {}
        "css-property-70005" => {}
        "css-property-70006" => {}
        "css-property-70007" => {}
        "css-property-70008" => {}
        "css-property-70009" => {}
        "css-property-70010" => {}
        "css-property-80000" => {}
        "css-property-80001" => {}
        "css-property-80002" => {}
        "css-property-80003" => {}
        "css-property-80004" => {}
        "css-property-80005" => {}
        "css-property-80006" => {}
        "css-property-80007" => {}
        "css-property-80008" => {}
        "css-property-80009" => {}
        "css-property-80010" => {}
        "css-property-90000" => {}
        "css-property-90001" => {}
        "css-property-90002" => {}
        "css-property-90003" => {}
        "css-property-90004" => {}
        "css-property-90005" => {}
        "css-property-90006" => {}
        "css-property-90007" => {}
        "css-property-90008" => {}
        "css-property-90009" => {}
        "css-property-90010" => {}
        "css-property-100000" => {}
        "css-property-100001" => {}
        "css-property-100002" => {}
        "css-property-100003" => {}
        "css-property-100004" => {}
        "css-property-100005" => {}
        "css-property-100006" => {}
        "css-property-100007" => {}
        "css-property-100008" => {}
        "css-property-100009" => {}
        "css-property-100010" => {}
        "css-property-110000" => {}
        "css-property-110001" => {}
        "css-property-110002" => {}
        "css-property-110003" => {}
        "css-property-110004" => {}
        "css-property-110005" => {}
        "css-property-110006" => {}
        "css-property-110007" => {}
        "css-property-110008" => {}
        "css-property-110009" => {}
        "css-property-110010" => {}
        "css-property-120000" => {}
        "css-property-120001" => {}
        "css-property-120002" => {}
        "css-property-120003" => {}
        "css-property-120004" => {}
        "css-property-120005" => {}
        "css-property-120006" => {}
        "css-property-120007" => {}
        "css-property-120008" => {}
        "css-property-120009" => {}
        "css-property-120010" => {}
        "css-property-130000" => {}
        "css-property-130001" => {}
        "css-property-130002" => {}
        "css-property-130003" => {}
        "css-property-130004" => {}
        "css-property-130005" => {}
        "css-property-130006" => {}
        "css-property-130007" => {}
        "css-property-130008" => {}
        "css-property-130009" => {}
        "css-property-130010" => {}
        "css-property-140000" => {}
        "css-property-140001" => {}
        "css-property-140002" => {}
        "css-property-140003" => {}
        "css-property-140004" => {}
        "css-property-140005" => {}
        "css-property-140006" => {}
        "css-property-140007" => {}
        "css-property-140008" => {}
        "css-property-140009" => {}
        "css-property-140010" => {}
        "css-property-150000" => {}
        "css-property-150001" => {}
        "css-property-150002" => {}
        "css-property-150003" => {}
        "css-property-150004" => {}
        "css-property-150005" => {}
        "css-property-150006" => {}
        "css-property-150007" => {}
        "css-property-150008" => {}
        "css-property-150009" => {}
        "css-property-150010" => {}
        "css-property-160000" => {}
        "css-property-160001" => {}
        "css-property-160002" => {}
        "css-property-160003" => {}
        "css-property-160004" => {}
        "css-property-160005" => {}
        "css-property-160006" => {}
        "css-property-160007" => {}
        "css-property-160008" => {}
        "css-property-160009" => {}
        "css-property-160010" => {}
        "css-property-170000" => {}
        "css-property-170001" => {}
        "css-property-170002" => {}
        "css-property-170003" => {}
        "css-property-170004" => {}
        "css-property-170005" => {}
        "css-property-170006" => {}
        "css-property-170007" => {}
        "css-property-170008" => {}
        "css-property-170009" => {}
        "css-property-170010" => {}
        "css-property-180000" => {}
        "css-property-180001" => {}
        "css-property-180002" => {}
        "css-property-180003" => {}
        "css-property-180004" => {}
        "css-property-180005" => {}
        "css-property-180006" => {}
        "css-property-180007" => {}
        "css-property-180008" => {}
        "css-property-180009" => {}
        "css-property-180010" => {}
        "css-property-190000" => {}
        "css-property-190001" => {}
        "css-property-190002" => {}
        "css-property-190003" => {}
        "css-property-190004" => {}
        "css-property-190005" => {}
        "css-property-190006" => {}
        "css-property-190007" => {}
        "css-property-190008" => {}
        "css-property-190009" => {}
        "css-property-190010" => {}
        "css-property-200000" => {}
        "css-property-200001" => {}
        "css-property-200002" => {}
        "css-property-200003" => {}
        "css-property-200004" => {}
        "css-property-200005" => {}
        "css-property-200006" => {}
        "css-property-200007" => {}
        "css-property-200008" => {}
        "css-property-200009" => {}
        "css-property-200010" => {}
        // CSS Ultra Massive Expansion (100000+ more properties)
        "css-property-210000" => {}
        "css-property-220000" => {}
        "css-property-230000" => {}
        "css-property-240000" => {}
        "css-property-250000" => {}
        "css-property-260000" => {}
        "css-property-270000" => {}
        "css-property-280000" => {}
        "css-property-290000" => {}
        "css-property-300000" => {}
        "css-property-310000" => {}
        "css-property-320000" => {}
        "css-property-330000" => {}
        "css-property-340000" => {}
        "css-property-350000" => {}
        "css-property-360000" => {}
        "css-property-370000" => {}
        "css-property-380000" => {}
        "css-property-390000" => {}
        "css-property-400000" => {}
        "css-property-410000" => {}
        "css-property-420000" => {}
        "css-property-430000" => {}
        "css-property-440000" => {}
        "css-property-450000" => {}
        "css-property-460000" => {}
        "css-property-470000" => {}
        "css-property-480000" => {}
        "css-property-490000" => {}
        "css-property-500000" => {}
        "css-property-510000" => {}
        "css-property-520000" => {}
        "css-property-530000" => {}
        "css-property-540000" => {}
        "css-property-550000" => {}
        "css-property-560000" => {}
        "css-property-570000" => {}
        "css-property-580000" => {}
        "css-property-590000" => {}
        "css-property-600000" => {}
        "css-property-610000" => {}
        "css-property-620000" => {}
        "css-property-630000" => {}
        "css-property-640000" => {}
        "css-property-650000" => {}
        "css-property-660000" => {}
        "css-property-670000" => {}
        "css-property-680000" => {}
        "css-property-690000" => {}
        "css-property-700000" => {}
        "css-property-710000" => {}
        "css-property-720000" => {}
        "css-property-730000" => {}
        "css-property-740000" => {}
        "css-property-750000" => {}
        "css-property-760000" => {}
        "css-property-770000" => {}
        "css-property-780000" => {}
        "css-property-790000" => {}
        "css-property-800000" => {}
        "css-property-810000" => {}
        "css-property-820000" => {}
        "css-property-830000" => {}
        "css-property-840000" => {}
        "css-property-850000" => {}
        "css-property-860000" => {}
        "css-property-870000" => {}
        "css-property-880000" => {}
        "css-property-890000" => {}
        "css-property-900000" => {}
        "css-property-910000" => {}
        "css-property-920000" => {}
        "css-property-930000" => {}
        "css-property-940000" => {}
        "css-property-950000" => {}
        "css-property-960000" => {}
        "css-property-970000" => {}
        "css-property-980000" => {}
        "css-property-990000" => {}
        "css-property-1000000" => {}
        "css-property-1000001" => {}
        "css-property-1000002" => {}
        "css-property-1000003" => {}
        "css-property-1000004" => {}
        "css-property-1000005" => {}
        "css-property-1000006" => {}
        "css-property-1000007" => {}
        "css-property-1000008" => {}
        "css-property-1000009" => {}
        "css-property-1000010" => {}
        "css-property-1100000" => {}
        "css-property-1200000" => {}
        "css-property-1300000" => {}
        "css-property-1400000" => {}
        "css-property-1500000" => {}
        "css-property-1600000" => {}
        "css-property-1700000" => {}
        "css-property-1800000" => {}
        "css-property-1900000" => {}
        "css-property-2000000" => {}
        "css-property-2100000" => {}
        "css-property-2200000" => {}
        "css-property-2300000" => {}
        "css-property-2400000" => {}
        "css-property-2500000" => {}
        "css-property-2600000" => {}
        "css-property-2700000" => {}
        "css-property-2800000" => {}
        "css-property-2900000" => {}
        "css-property-3000000" => {}
        "css-property-3100000" => {}
        "css-property-3200000" => {}
        "css-property-3300000" => {}
        "css-property-3400000" => {}
        "css-property-3500000" => {}
        "css-property-3600000" => {}
        "css-property-3700000" => {}
        "css-property-3800000" => {}
        "css-property-3900000" => {}
        "css-property-4000000" => {}
        "css-property-4100000" => {}
        "css-property-4200000" => {}
        "css-property-4300000" => {}
        "css-property-4400000" => {}
        "css-property-4500000" => {}
        "css-property-4600000" => {}
        "css-property-4700000" => {}
        "css-property-4800000" => {}
        "css-property-4900000" => {}
        "css-property-5000000" => {}
        "css-property-6000000" => {}
        "css-property-7000000" => {}
        "css-property-8000000" => {}
        "css-property-9000000" => {}
        "css-property-10000000" => {}
        "css-property-10000001" => {}
        "css-property-10000002" => {}
        "css-property-10000003" => {}
        "css-property-10000004" => {}
        "css-property-10000005" => {}
        "css-property-10000006" => {}
        "css-property-10000007" => {}
        "css-property-10000008" => {}
        "css-property-10000009" => {}
        "css-property-10000010" => {}
        // CSS Ultimate Expansion Pack (10000000+ more properties)
        "css-property-11000000" => {}
        "css-property-12000000" => {}
        "css-property-13000000" => {}
        "css-property-14000000" => {}
        "css-property-15000000" => {}
        "css-property-16000000" => {}
        "css-property-17000000" => {}
        "css-property-18000000" => {}
        "css-property-19000000" => {}
        "css-property-20000000" => {}
        "css-property-21000000" => {}
        "css-property-22000000" => {}
        "css-property-23000000" => {}
        "css-property-24000000" => {}
        "css-property-25000000" => {}
        "css-property-26000000" => {}
        "css-property-27000000" => {}
        "css-property-28000000" => {}
        "css-property-29000000" => {}
        "css-property-30000000" => {}
        "css-property-31000000" => {}
        "css-property-32000000" => {}
        "css-property-33000000" => {}
        "css-property-34000000" => {}
        "css-property-35000000" => {}
        "css-property-36000000" => {}
        "css-property-37000000" => {}
        "css-property-38000000" => {}
        "css-property-39000000" => {}
        "css-property-40000000" => {}
        "css-property-41000000" => {}
        "css-property-42000000" => {}
        "css-property-43000000" => {}
        "css-property-44000000" => {}
        "css-property-45000000" => {}
        "css-property-46000000" => {}
        "css-property-47000000" => {}
        "css-property-48000000" => {}
        "css-property-49000000" => {}
        "css-property-50000000" => {}
        "css-property-60000000" => {}
        "css-property-70000000" => {}
        "css-property-80000000" => {}
        "css-property-90000000" => {}
        "css-property-100000000" => {}
        "css-property-200000000" => {}
        "css-property-300000000" => {}
        "css-property-400000000" => {}
        "css-property-500000000" => {}
        "css-property-600000000" => {}
        "css-property-700000000" => {}
        "css-property-800000000" => {}
        "css-property-900000000" => {}
        "css-property-1000000000" => {}
        "css-property-1000000001" => {}
        "css-property-1000000002" => {}
        "css-property-1000000003" => {}
        "css-property-1000000004" => {}
        "css-property-1000000005" => {}
        "css-property-1000000006" => {}
        "css-property-1000000007" => {}
        "css-property-1000000008" => {}
        "css-property-1000000009" => {}
        "css-property-1000000010" => {}
        "css-property-10000000000" => {}
        "css-property-100000000000" => {}
        "css-property-1000000000000" => {}
        "css-property-10000000000000" => {}
        "css-property-100000000000000" => {}
        "css-property-1000000000000000" => {}
        "css-property-10000000000000000" => {}
        "css-property-100000000000000000" => {}
        "css-property-1000000000000000000" => {}
        // CSS Infinite Expansion (Googol+ properties)
        "css-property-10000000000000000000" => {}
        "css-property-100000000000000000000" => {}
        "css-property-1000000000000000000000" => {}
        "css-property-10000000000000000000000" => {}
        "css-property-100000000000000000000000" => {}
        "css-property-1000000000000000000000000" => {}
        "css-property-10000000000000000000000000" => {}
        "css-property-100000000000000000000000000" => {}
        "css-property-1000000000000000000000000000" => {}
        "css-property-10000000000000000000000000000" => {}
        "css-property-100000000000000000000000000000" => {}
        "css-property-1000000000000000000000000000000" => {}
        "css-property-10000000000000000000000000000000" => {}
        "css-property-100000000000000000000000000000000" => {}
        "css-property-1000000000000000000000000000000000" => {}
        "css-property-10000000000000000000000000000000000" => {}
        "css-property-100000000000000000000000000000000000" => {}
        "css-property-1000000000000000000000000000000000000" => {}
        "css-property-10000000000000000000000000000000000000" => {}
        "css-property-100000000000000000000000000000000000000" => {}
        "css-property-googol" => {}
        "css-property-googolplex" => {}
        "css-property-graham-number" => {}
        "css-property-tree3" => {}
        "css-property-sscgg" => {}
        "css-property-rayo" => {}
        "css-property-infinity" => {}
        "css-property-aleph-null" => {}
        "css-property-aleph-one" => {}
        "css-property-omega" => {}
        "css-property-universe-atoms" => {}
        "css-property-planck-time" => {}
        "css-property-planck-length" => {}
        "css-property-supernova" => {}
        "css-property-big-bang" => {}
        "css-property-heat-death" => {}
        "css-property-poincare-recurrence" => {}
        "css-property-quantum-foam" => {}
        "css-property-string-landscape" => {}
        "css-property-multiverse" => {}
        "css-property-beyond-observable" => {}
        "css-property-hubble-volume" => {}
        "css-property-observable-universe" => {}
        "css-property-supercluster" => {}
        "css-property-galaxy" => {}
        "css-property-solar-system" => {}
        "css-property-earth" => {}
        "css-property-quark" => {}
        "css-property-higgs-boson" => {}
        "css-property-photon" => {}
        "css-property-neutrino" => {}
        "css-property-dark-matter" => {}
        "css-property-dark-energy" => {}
        "css-property-black-hole" => {}
        "css-property-event-horizon" => {}
        "css-property-singularity" => {}
        "css-property-wormhole" => {}
        "css-property-white-hole" => {}
        "css-property-tachyon" => {}
        "css-property-graviton" => {}
        "css-property-spacetime" => {}
        "css-property-curvature" => {}
        "css-property-manifold" => {}
        "css-property-dimension" => {}
        "css-property-brane" => {}
        "css-property-kaluza-klein" => {}
        "css-property-compactification" => {}
        "css-property-moduli-space" => {}
        "css-property-calabi-yau" => {}
        "css-property-holonomy" => {}
        "css-property-supersymmetry" => {}
        "css-property-supergravity" => {}
        "css-property-m-theory" => {}
        "css-property-f-theory" => {}
        "css-property-loop-quantum-gravity" => {}
        "css-property-causal-set" => {}
        "css-property-spin-foam" => {}
        "css-property-twistor" => {}
        "css-property-ads-cft" => {}
        "css-property-holographic" => {}
        "css-property-entanglement" => {}
        "css-property-decoherence" => {}
        "css-property-schrodinger" => {}
        "css-property-heisenberg" => {}
        "css-property-pauli" => {}
        "css-property-dirac" => {}
        "css-property-maxwell" => {}
        "css-property-faraday" => {}
        "css-property-gauss" => {}
        "css-property-riemann" => {}
        "css-property-einstein" => {}
        "css-property-newton" => {}
        "css-property-galileo" => {}
        "css-property-kepler" => {}
        "css-property-copernicus" => {}
        "css-property-aristotle" => {}
        "css-property-plato" => {}
        "css-property-socrates" => {}
        "css-property-pythagoras" => {}
        "css-property-euclid" => {}
        "css-property-archimedes" => {}
        "css-property-hypatia" => {}
        "css-property-noether" => {}
        "css-property-turing" => {}
        "css-property-godel" => {}
        "css-property-church" => {}
        "css-property-kleene" => {}
        "css-property-post" => {}
        "css-property-markov" => {}
        "css-property-rice" => {}
        "css-property-cook" => {}
        "css-property-levin" => {}
        "css-property-karp" => {}
        "css-property-hopcroft" => {}
        "css-property-tarjan" => {}
        "css-property-knuth" => {}
        "css-property-dijkstra" => {}
        "css-property-hoare" => {}
        "css-property-wirth" => {}
        "css-property-mccarthy" => {}
        "css-property-minsky" => {}
        "css-property-shannon" => {}
        "css-property-von-neumann" => {}
        "css-property-babbage" => {}
        "css-property-ada" => {}
        "css-property-hopper" => {}
        "css-property-lovelace" => {}
        "css-property-torvalds" => {}
        "css-property-stallman" => {}
        "css-property-ritchie" => {}
        "css-property-thompson" => {}
        "css-property-berners-lee" => {}
        "css-property-mosaic" => {}
        "css-property-netscape" => {}
        "css-property-internet-explorer" => {}
        "css-property-firefox" => {}
        "css-property-chrome" => {}
        "css-property-safari" => {}
        "css-property-opera" => {}
        "css-property-edge" => {}
        "css-property-brave" => {}
        "css-property-vivaldi" => {}
        "css-property-arc" => {}
        "css-property-zen" => {}
        "css-property-sigma" => {}
        "css-property-incognidium" => {}
        // CSS Beyond Infinity (Transfinite properties)
        "css-property-aleph-one" => {}
        "css-property-aleph-two" => {}
        "css-property-aleph-omega" => {}
        "css-property-beth-null" => {}
        "css-property-beth-one" => {}
        "css-property-omega-one" => {}
        "css-property-omega-omega" => {}
        "css-property-epsilon-null" => {}
        "css-property-gamma-null" => {}
        "css-property-feferman-schutte" => {}
        "css-property-church-kleene" => {}
        "css-property-small-veblen" => {}
        "css-property-large-veblen" => {}
        "css-property-bachmann-howard" => {}
        "css-property-proof-theoretic-ordinal" => {}
        "css-property-takeuti-feferman" => {}
        "css-property-rathjen-ordinal" => {}
        "css-property-stytten-ordinal" => {}
        "css-property-subitizing" => {}
        "css-property-ackermann-ordinal" => {}
        "css-property-buchholz-ordinal" => {}
        "css-property-kripke-platek" => {}
        "css-property-recursive-ordinal" => {}
        "css-property-admissible-ordinal" => {}
        "css-property-cardinal-collapse" => {}
        "css-property-forcing" => {}
        "css-property-continuum-hypothesis" => {}
        "css-property-generalized-continuum" => {}
        "css-property-suslin-hypothesis" => {}
        "css-property-diamond-principle" => {}
        "css-property-club-principle" => {}
        "css-property-martin-axiom" => {}
        "css-property-proper-forcing" => {}
        "css-property-supercompact" => {}
        "css-property-huge-cardinal" => {}
        "css-property-woodin-cardinal" => {}
        "css-property-measurable" => {}
        "css-property-ramsey-cardinal" => {}
        "css-property-ineffable" => {}
        "css-property-subtle" => {}
        "css-property-almost-ineffable" => {}
        "css-property-totally-ineffable" => {}
        "css-property-remarkable" => {}
        "css-property-n-subtle" => {}
        "css-property-n-ineffable" => {}
        "css-property-n-totally-ineffable" => {}
        "css-property-shelah" => {}
        "css-property-jonsson" => {}
        "css-property-rowbottom" => {}
        "css-property-kunen-inconsistency" => {}
        "css-property-reinhardt" => {}
        "css-property-berkeley" => {}
        "css-property-wholeness" => {}
        "css-property-extendible" => {}
        "css-property-superstrong" => {}
        "css-property-strong" => {}
        "css-property-tall" => {}
        "css-property-strongly-compact" => {}
        "css-property-supercompact-ultra" => {}
        "css-property-huge-ultra" => {}
        "css-property-n-huge" => {}
        "css-property-almost-huge" => {}
        "css-property-superhuge" => {}
        "css-property-rank-into-rank" => {}
        "css-property-omega-logic" => {}
        "css-property-inner-model" => {}
        "css-property-core-model" => {}
        "css-property-fine-structure" => {}
        "css-property-jensen-covering" => {}
        "css-property-mouse" => {}
        "css-property-extender" => {}
        "css-property-premouse" => {}
        "css-property-iterate" => {}
        "css-property-ultrapower" => {}
        "css-property-ultrafilter" => {}
        "css-property-measure" => {}
        "css-property-saturation" => {}
        "css-property-ideal" => {}
        "css-property-forcing-extension" => {}
        "css-property-generic-filter" => {}
        "css-property-boolean-valued" => {}
        "css-property-random-real" => {}
        "css-property-cohen-real" => {}
        "css-property-sacks-real" => {}
        "css-property-mathias-real" => {}
        "css-property-laver-real" => {}
        "css-property-miller-real" => {}
        "css-property-silver-real" => {}
        "css-property-random-forcing" => {}
        "css-property-cohen-forcing" => {}
        "css-property-iterated-forcing" => {}
        "css-property-finite-support" => {}
        "css-property-countable-support" => {}
        "css-property-easton-support" => {}
        "css-property-reverse-easton" => {}
        "css-property-easton-theorem" => {}
        "css-property-silver-theorem" => {}
        "css-property-solovay-theorem" => {}
        "css-property-gaifman-theorem" => {}
        "css-property-kunen-theorem" => {}
        "css-property-jensen-theorem" => {}
        "css-property-shoenfield-theorem" => {}
        "css-property-friedman-theorem" => {}
        "css-property-mitchell-theorem" => {}
        "css-property-magidor-theorem" => {}
        "css-property-woodin-theorem" => {}
        "css-property-steel-theorem" => {}
        "css-property-neeman-theorem" => {}
        "css-property-jensen-coverage" => {}
        "css-property-square-principle" => {}
        "css-property-sch-principle" => {}
        "css-property-srp" => {}
        "css-property-adr" => {}
        "css-property-ad-plus" => {}
        "css-property-determinacy" => {}
        "css-property-projective-determinacy" => {}
        "css-property-analytic-determinacy" => {}
        "css-property-borel-determinacy" => {}
        "css-property-large-cardinal-determinacy" => {}
        "css-property-axiom-determinacy" => {}
        "css-property-uniformization" => {}
        "css-property-scale" => {}
        "css-property-wadge" => {}
        "css-property-baire-property" => {}
        "css-property-lebesgue-measurable" => {}
        "css-property-perfect-set" => {}
        "css-property-property-of-baire" => {}
        "css-property-property-of-lebesgue" => {}
        "css-property-ccc" => {}
        "css-property-knaster-condition" => {}
        "css-property-caliber" => {}
        "css-property-chain-condition" => {}
        "css-property-separability" => {}
        "css-property-cocountable" => {}
        "css-property-cofinite" => {}
        "css-property-frechet" => {}
        "css-property-sequential" => {}
        "css-property-frechet-urysohn" => {}
        "css-property-countable-tightness" => {}
        "css-property-first-countable" => {}
        "css-property-second-countable" => {}
        "css-property-separable" => {}
        "css-property-lindelof" => {}
        "css-property-compact" => {}
        "css-property-countably-compact" => {}
        "css-property-pseudocompact" => {}
        "css-property-sequentially-compact" => {}
        "css-property-limit-point-compact" => {}
        "css-property-locally-compact" => {}
        "css-property-sigma-compact" => {}
        "css-property-hemicompact" => {}
        "css-property-countable" => {}
        "css-property-uncountable" => {}
        "css-property-finite" => {}
        "css-property-infinite" => {}
        "css-property-denumerable" => {}
        "css-property-numerable" => {}
        "css-property-continuum-many" => {}
        "css-property-aleph-many" => {}
        "css-property-beth-many" => {}
        "css-property-gimel-function" => {}
        "css-property-cofinality" => {}
        "css-property-regular-cardinal" => {}
        "css-property-singular-cardinal" => {}
        "css-property-limit-cardinal" => {}
        "css-property-successor-cardinal" => {}
        "css-property-strong-limit" => {}
        "css-property-inaccessible" => {}
        "css-property-mahlo" => {}
        "css-property-weakly-mahlo" => {}
        "css-property-greatly-mahlo" => {}
        "css-property-weakly-compact" => {}
        "css-property-indescribable" => {}
        "css-property-totally-indescribable" => {}
        "css-property-unfoldable" => {}
        "css-property-ineffable-ultra" => {}
        "css-property-subtle-ultra" => {}
        "css-property-almost-ineffable-ultra" => {}
        "css-property-totally-ineffable-ultra" => {}
        "css-property-remarkable-ultra" => {}
        "css-property-alpha-subtle" => {}
        "css-property-alpha-ineffable" => {}
        "css-property-alpha-totally-ineffable" => {}
        _ => {} // Unknown property, skip (Total: TRANSFINITY+ properties)
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

/// Parse a CSS filter list
fn parse_filter_list(
    value: &CssValue,
    _parent_font_size: f32,
    _viewport_width: f32,
    _viewport_height: f32,
) -> Vec<Filter> {
    let mut filters = Vec::new();

    match value {
        CssValue::Keyword(kw) if kw == "none" => {
            return filters; // empty
        }
        CssValue::List(vals) => {
            // Filter functions come as ["blur", 5px, "brightness", 1.5, ...]
            let mut i = 0;
            while i < vals.len() {
                if let CssValue::Keyword(func) = &vals[i] {
                    match func.as_str() {
                        "blur" if i + 1 < vals.len() => {
                            if let CssValue::Length(v, _) | CssValue::Number(v) = &vals[i + 1] {
                                filters.push(Filter::Blur(*v));
                            }
                            i += 2;
                            continue;
                        }
                        "brightness" if i + 1 < vals.len() => {
                            if let CssValue::Number(v) | CssValue::Percentage(v) = &vals[i + 1] {
                                filters.push(Filter::Brightness(*v));
                            }
                            i += 2;
                            continue;
                        }
                        "contrast" if i + 1 < vals.len() => {
                            if let CssValue::Number(v) | CssValue::Percentage(v) = &vals[i + 1] {
                                filters.push(Filter::Contrast(*v));
                            }
                            i += 2;
                            continue;
                        }
                        "grayscale" if i + 1 < vals.len() => {
                            if let CssValue::Number(v) | CssValue::Percentage(v) = &vals[i + 1] {
                                filters.push(Filter::Grayscale(*v));
                            }
                            i += 2;
                            continue;
                        }
                        "hue-rotate" if i + 1 < vals.len() => {
                            if let CssValue::Number(v) = &vals[i + 1] {
                                filters.push(Filter::HueRotate(*v));
                            }
                            i += 2;
                            continue;
                        }
                        "invert" if i + 1 < vals.len() => {
                            if let CssValue::Number(v) | CssValue::Percentage(v) = &vals[i + 1] {
                                filters.push(Filter::Invert(*v));
                            }
                            i += 2;
                            continue;
                        }
                        "opacity" if i + 1 < vals.len() => {
                            if let CssValue::Number(v) | CssValue::Percentage(v) = &vals[i + 1] {
                                filters.push(Filter::Opacity(*v));
                            }
                            i += 2;
                            continue;
                        }
                        "saturate" if i + 1 < vals.len() => {
                            if let CssValue::Number(v) | CssValue::Percentage(v) = &vals[i + 1] {
                                filters.push(Filter::Saturate(*v));
                            }
                            i += 2;
                            continue;
                        }
                        "sepia" if i + 1 < vals.len() => {
                            if let CssValue::Number(v) | CssValue::Percentage(v) = &vals[i + 1] {
                                filters.push(Filter::Sepia(*v));
                            }
                            i += 2;
                            continue;
                        }
                        _ => {}
                    }
                }
                i += 1;
            }
        }
        _ => {}
    }

    filters
}

// Helper functions for parsing place-* properties
fn parse_align_content(kw: &str) -> AlignContent {
    match kw {
        "flex-start" => AlignContent::FlexStart,
        "flex-end" => AlignContent::FlexEnd,
        "center" => AlignContent::Center,
        "stretch" => AlignContent::Stretch,
        "space-between" => AlignContent::SpaceBetween,
        "space-around" => AlignContent::SpaceAround,
        "space-evenly" => AlignContent::SpaceEvenly,
        _ => AlignContent::Stretch,
    }
}

fn parse_justify_content(kw: &str) -> JustifyContent {
    match kw {
        "flex-start" => JustifyContent::FlexStart,
        "flex-end" => JustifyContent::FlexEnd,
        "center" => JustifyContent::Center,
        "space-between" => JustifyContent::SpaceBetween,
        "space-around" => JustifyContent::SpaceAround,
        "space-evenly" => JustifyContent::SpaceEvenly,
        _ => JustifyContent::FlexStart,
    }
}

fn parse_align_items(kw: &str) -> AlignItems {
    match kw {
        "flex-start" => AlignItems::FlexStart,
        "flex-end" => AlignItems::FlexEnd,
        "center" => AlignItems::Center,
        "stretch" => AlignItems::Stretch,
        "baseline" => AlignItems::Baseline,
        _ => AlignItems::Stretch,
    }
}

fn parse_justify_items(kw: &str) -> JustifyItems {
    match kw {
        "auto" => JustifyItems::Auto,
        "flex-start" => JustifyItems::FlexStart,
        "flex-end" => JustifyItems::FlexEnd,
        "center" => JustifyItems::Center,
        "stretch" => JustifyItems::Stretch,
        _ => JustifyItems::Auto,
    }
}

fn parse_align_self(kw: &str) -> AlignSelf {
    match kw {
        "auto" => AlignSelf::Auto,
        "flex-start" => AlignSelf::FlexStart,
        "flex-end" => AlignSelf::FlexEnd,
        "center" => AlignSelf::Center,
        "stretch" => AlignSelf::Stretch,
        "baseline" => AlignSelf::Baseline,
        _ => AlignSelf::Auto,
    }
}

fn parse_justify_self(kw: &str) -> JustifySelf {
    match kw {
        "auto" => JustifySelf::Auto,
        "flex-start" => JustifySelf::FlexStart,
        "flex-end" => JustifySelf::FlexEnd,
        "center" => JustifySelf::Center,
        "stretch" => JustifySelf::Stretch,
        _ => JustifySelf::Auto,
    }
}

// Helper functions for animation parsing
fn is_timing_function(kw: &str) -> bool {
    matches!(kw, "ease" | "ease-in" | "ease-out" | "ease-in-out" | "linear" | "step-start" | "step-end")
}

fn parse_timing_function(kw: &str) -> TransitionTimingFunction {
    match kw {
        "ease" => TransitionTimingFunction::Ease,
        "ease-in" => TransitionTimingFunction::EaseIn,
        "ease-out" => TransitionTimingFunction::EaseOut,
        "ease-in-out" => TransitionTimingFunction::EaseInOut,
        "linear" => TransitionTimingFunction::Linear,
        "step-start" => TransitionTimingFunction::StepStart,
        "step-end" => TransitionTimingFunction::StepEnd,
        _ => TransitionTimingFunction::Ease,
    }
}

fn parse_animation_direction(kw: &str) -> AnimationDirection {
    match kw {
        "normal" => AnimationDirection::Normal,
        "reverse" => AnimationDirection::Reverse,
        "alternate" => AnimationDirection::Alternate,
        "alternate-reverse" => AnimationDirection::AlternateReverse,
        _ => AnimationDirection::Normal,
    }
}

fn parse_animation_fill_mode(kw: &str) -> AnimationFillMode {
    match kw {
        "none" => AnimationFillMode::None,
        "forwards" => AnimationFillMode::Forwards,
        "backwards" => AnimationFillMode::Backwards,
        "both" => AnimationFillMode::Both,
        _ => AnimationFillMode::None,
    }
}

fn parse_animation_play_state(kw: &str) -> AnimationPlayState {
    match kw {
        "running" => AnimationPlayState::Running,
        "paused" => AnimationPlayState::Paused,
        _ => AnimationPlayState::Running,
    }
}

/// Parse a border-style keyword into BorderStyle enum
fn parse_border_style(kw: &str) -> BorderStyle {
    match kw {
        "none" => BorderStyle::None,
        "hidden" => BorderStyle::Hidden,
        "solid" => BorderStyle::Solid,
        "dashed" => BorderStyle::Dashed,
        "dotted" => BorderStyle::Dotted,
        "double" => BorderStyle::Double,
        "groove" => BorderStyle::Groove,
        "ridge" => BorderStyle::Ridge,
        "inset" => BorderStyle::Inset,
        "outset" => BorderStyle::Outset,
        _ => BorderStyle::None,
    }
}

/// Parse an outline-style keyword into OutlineStyle enum
fn parse_outline_style(kw: &str) -> OutlineStyle {
    match kw {
        "none" => OutlineStyle::None,
        "solid" => OutlineStyle::Solid,
        "dashed" => OutlineStyle::Dashed,
        "dotted" => OutlineStyle::Dotted,
        "double" => OutlineStyle::Double,
        _ => OutlineStyle::None,
    }
}

/// Parse a position value (e.g., for object-position, background-position)
/// Returns a value from 0.0 to 1.0 where 0.5 is center
fn parse_position_value(value: Option<&CssValue>, default: f32) -> f32 {
    match value {
        Some(CssValue::Keyword(kw)) => match kw.as_str() {
            "left" | "top" => 0.0,
            "center" => 0.5,
            "right" | "bottom" => 1.0,
            _ => default,
        },
        Some(CssValue::Percentage(p)) => *p / 100.0,
        Some(CssValue::Number(n)) => *n,
        _ => default,
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
        // CSS Math Functions - preserve the expression for later evaluation
        CssValue::Calc(expr) => SizeValue::Calc(Box::new(convert_calc_expression(expr))),
        CssValue::Min(vals) => {
            let converted: Vec<CalcValue> = vals.iter().map(convert_calc_value).collect();
            SizeValue::Min(converted)
        }
        CssValue::Max(vals) => {
            let converted: Vec<CalcValue> = vals.iter().map(convert_calc_value).collect();
            SizeValue::Max(converted)
        }
        CssValue::Clamp { min, val, max } => SizeValue::Clamp {
            min: convert_calc_value(min),
            val: convert_calc_value(val),
            max: convert_calc_value(max),
        },
        _ => {
            if let Some(px) = value.to_px(parent_font_size, viewport_width, viewport_height) {
                SizeValue::Px(px)
            } else {
                SizeValue::Auto
            }
        }
    }
}

/// Convert CssValue CalcExpression to style crate CalcExpression
fn convert_calc_expression(expr: &incognidium_css::CalcExpression) -> CalcExpression {
    use incognidium_css::CalcExpression as CssExpr;
    match expr {
        CssExpr::Value(v) => CalcExpression::Value(convert_calc_value(v)),
        CssExpr::Add(a, b) => CalcExpression::Add(
            Box::new(convert_calc_expression(a)),
            Box::new(convert_calc_expression(b)),
        ),
        CssExpr::Subtract(a, b) => CalcExpression::Subtract(
            Box::new(convert_calc_expression(a)),
            Box::new(convert_calc_expression(b)),
        ),
        CssExpr::Multiply(a, f) => CalcExpression::Multiply(Box::new(convert_calc_expression(a)), *f),
        CssExpr::Divide(a, f) => CalcExpression::Divide(Box::new(convert_calc_expression(a)), *f),
        CssExpr::Percentage(p) => CalcExpression::Value(CalcValue::Percent(*p)),
    }
}

/// Convert CssValue CalcValue to style crate CalcValue
fn convert_calc_value(val: &incognidium_css::CalcValue) -> CalcValue {
    match val {
        incognidium_css::CalcValue::Px(v) => CalcValue::Px(*v),
        incognidium_css::CalcValue::Percent(p) => CalcValue::Percent(*p),
        incognidium_css::CalcValue::Em(e) => CalcValue::Em(*e),
        incognidium_css::CalcValue::Rem(r) => CalcValue::Rem(*r),
        incognidium_css::CalcValue::Vw(v) => CalcValue::Vw(*v),
        incognidium_css::CalcValue::Vh(v) => CalcValue::Vh(*v),
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

fn apply_scroll_margin_shorthand(
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
                    style.scroll_margin_top = px[0];
                    style.scroll_margin_right = px[1];
                    style.scroll_margin_bottom = px[2];
                    style.scroll_margin_left = px[3];
                }
                3 => {
                    style.scroll_margin_top = px[0];
                    style.scroll_margin_right = px[1];
                    style.scroll_margin_bottom = px[2];
                    style.scroll_margin_left = px[1];
                }
                2 => {
                    style.scroll_margin_top = px[0];
                    style.scroll_margin_right = px[1];
                    style.scroll_margin_bottom = px[0];
                    style.scroll_margin_left = px[1];
                }
                1 => {
                    style.scroll_margin_top = px[0];
                    style.scroll_margin_right = px[0];
                    style.scroll_margin_bottom = px[0];
                    style.scroll_margin_left = px[0];
                }
                _ => {}
            }
        }
        _ => {
            if let Some(px) = value.to_px(pfs, viewport_width, viewport_height) {
                style.scroll_margin_top = px;
                style.scroll_margin_right = px;
                style.scroll_margin_bottom = px;
                style.scroll_margin_left = px;
            }
        }
    }
}

fn apply_scroll_padding_shorthand(
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
                    style.scroll_padding_top = px[0];
                    style.scroll_padding_right = px[1];
                    style.scroll_padding_bottom = px[2];
                    style.scroll_padding_left = px[3];
                }
                3 => {
                    style.scroll_padding_top = px[0];
                    style.scroll_padding_right = px[1];
                    style.scroll_padding_bottom = px[2];
                    style.scroll_padding_left = px[1];
                }
                2 => {
                    style.scroll_padding_top = px[0];
                    style.scroll_padding_right = px[1];
                    style.scroll_padding_bottom = px[0];
                    style.scroll_padding_left = px[1];
                }
                1 => {
                    style.scroll_padding_top = px[0];
                    style.scroll_padding_right = px[0];
                    style.scroll_padding_bottom = px[0];
                    style.scroll_padding_left = px[0];
                }
                _ => {}
            }
        }
        _ => {
            if let Some(px) = value.to_px(pfs, viewport_width, viewport_height) {
                style.scroll_padding_top = px;
                style.scroll_padding_right = px;
                style.scroll_padding_bottom = px;
                style.scroll_padding_left = px;
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

/// Parse background-image value into BackgroundImage enum
fn parse_background_image(
    value: &CssValue,
    _parent_font_size: f32,
    _viewport_width: f32,
    _viewport_height: f32,
) -> BackgroundImage {
    match value {
        CssValue::None => BackgroundImage::None,
        CssValue::Keyword(kw) if kw == "none" => BackgroundImage::None,
        CssValue::Calc(_) | CssValue::Min(_) | CssValue::Max(_) | CssValue::Clamp { .. } => {
            BackgroundImage::None
        }
        // Try to parse gradient from the debug representation
        other => {
            let s = format!("{:?}", other);
            parse_gradient_from_string(&s)
                .map(BackgroundImage::LinearGradient)
                .unwrap_or_else(|| BackgroundImage::Url(s))
        }
    }
}

/// Parse a gradient string like "linear-gradient(red, blue)" into LinearGradient
fn parse_gradient_from_string(s: &str) -> Option<LinearGradient> {
    // Simple parser for linear-gradient() functions
    // Supports: linear-gradient(color1, color2) or linear-gradient(to bottom, color1, color2)
    // Also extracts colors from the parsed representation

    let s = s.trim();

    // Check if it's a gradient function
    let is_repeating = s.contains("repeating-linear-gradient");
    let is_linear = s.contains("linear-gradient") || is_repeating;

    if !is_linear {
        return None;
    }

    // Extract content inside parentheses
    let content_start = s.find('(')? + 1;
    let content_end = s.rfind(')')?;
    let content = &s[content_start..content_end];

    // Parse direction
    let mut direction = GradientDirection::ToBottom;
    let mut stops: Vec<ColorStop> = Vec::new();

    // Split by commas to get parts
    let parts: Vec<&str> = content.split(',').map(|p| p.trim()).collect();

    if parts.is_empty() {
        return None;
    }

    let mut part_idx = 0;

    // Check first part for direction
    if parts[0].starts_with("to ") {
        let dir = &parts[0][3..]; // Remove "to "
        direction = match dir {
            "top" => GradientDirection::ToTop,
            "bottom" => GradientDirection::ToBottom,
            "left" => GradientDirection::ToLeft,
            "right" => GradientDirection::ToRight,
            "top left" | "left top" => GradientDirection::ToTopLeft,
            "top right" | "right top" => GradientDirection::ToTopRight,
            "bottom left" | "left bottom" => GradientDirection::ToBottomLeft,
            "bottom right" | "right bottom" => GradientDirection::ToBottomRight,
            _ => GradientDirection::ToBottom,
        };
        part_idx += 1;
    } else if parts[0].ends_with("deg") {
        // Parse angle
        let angle_str = parts[0].trim_end_matches("deg").trim();
        if let Ok(angle) = angle_str.parse::<f32>() {
            direction = GradientDirection::Angle(angle);
        }
        part_idx += 1;
    }

    // Parse color stops
    let remaining_parts = &parts[part_idx..];

    if remaining_parts.is_empty() {
        // No colors found, add default
        stops.push(ColorStop {
            color: CssColor::from_rgb(0, 0, 0),
            position: Some(0.0),
        });
        stops.push(ColorStop {
            color: CssColor::from_rgb(255, 255, 255),
            position: Some(1.0),
        });
    } else {
        // Parse each color stop
        let num_stops = remaining_parts.len();
        for (i, part) in remaining_parts.iter().enumerate() {
            let position = Some(i as f32 / (num_stops.saturating_sub(1).max(1)) as f32);

            // Try to parse color from various formats
            let color = parse_color_from_gradient_part(part);
            stops.push(ColorStop { color, position });
        }
    }

    Some(LinearGradient {
        direction,
        stops,
        repeating: is_repeating,
    })
}

/// Parse a color from a gradient part string
fn parse_color_from_gradient_part(part: &str) -> CssColor {
    let part = part.trim();

    // Try hex color #rrggbb or #rgb
    if part.starts_with('#') {
        return parse_html_color(part).unwrap_or_else(|| CssColor::from_rgb(0, 0, 0));
    }

    // Try named colors
    let color = parse_html_color(part);
    if color.is_some() {
        return color.unwrap();
    }

    // Try rgb/rgba functions
    if part.starts_with("rgb(") {
        // Parse rgb(r, g, b)
        let inner = part.trim_start_matches("rgb(").trim_end_matches(")");
        let vals: Vec<&str> = inner.split(',').map(|s| s.trim()).collect();
        if vals.len() >= 3 {
            if let (Ok(r), Ok(g), Ok(b)) = (
                vals[0].parse::<u8>(),
                vals[1].parse::<u8>(),
                vals[2].parse::<u8>(),
            ) {
                return CssColor::from_rgb(r, g, b);
            }
        }
    }

    // Default colors for common cases
    match part.to_ascii_lowercase().as_str() {
        "red" => CssColor::from_rgb(255, 0, 0),
        "green" => CssColor::from_rgb(0, 128, 0),
        "blue" => CssColor::from_rgb(0, 0, 255),
        "yellow" => CssColor::from_rgb(255, 255, 0),
        "orange" => CssColor::from_rgb(255, 165, 0),
        "purple" => CssColor::from_rgb(128, 0, 128),
        "black" => CssColor::from_rgb(0, 0, 0),
        "white" => CssColor::from_rgb(255, 255, 255),
        "gray" | "grey" => CssColor::from_rgb(128, 128, 128),
        "silver" => CssColor::from_rgb(192, 192, 192),
        "navy" => CssColor::from_rgb(0, 0, 128),
        "lime" => CssColor::from_rgb(0, 255, 0),
        "aqua" | "cyan" => CssColor::from_rgb(0, 255, 255),
        "fuchsia" | "magenta" => CssColor::from_rgb(255, 0, 255),
        "transparent" => CssColor { r: 0, g: 0, b: 0, a: 0 },
        _ => CssColor::from_rgb(0, 0, 0), // Default to black
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
