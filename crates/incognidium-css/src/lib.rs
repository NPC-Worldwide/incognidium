use cssparser::{ParseError, Parser, ParserInput, Token};
use incognidium_dom::{Document, ElementData, NodeData, NodeId};

/// A parsed CSS stylesheet.
#[derive(Debug, Default, Clone)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
    /// CSS custom properties (variables) from :root rules
    pub variables: std::collections::HashMap<String, String>,
    /// CSS keyframes for animations
    pub keyframes: std::collections::HashMap<String, Keyframes>,
    /// CSS @import rules (must be processed before other rules)
    pub imports: Vec<ImportRule>,
    /// CSS @font-face rules for custom fonts
    pub font_faces: Vec<FontFaceRule>,
    /// CSS @counter-style rules for custom list markers
    pub counter_styles: std::collections::HashMap<String, CounterStyleRule>,
    /// CSS @property rules for Houdini custom properties
    pub properties: std::collections::HashMap<String, PropertyRule>,
    /// CSS @starting-style rules for view transitions
    pub starting_styles: Vec<StartingStyleRule>,
    /// CSS @scope rules for scoped styles
    pub scopes: Vec<ScopeRule>,
}

/// A CSS @scope rule
#[derive(Debug, Clone, Default)]
pub struct ScopeRule {
    /// The scope root selector (e.g., ".card")
    pub root: Option<String>,
    /// The scope limit selector (e.g., ".limit", after ":scope")
    pub limit: Option<String>,
    /// Rules within this scope
    pub rules: Vec<Rule>,
}

/// A CSS @starting-style rule
#[derive(Debug, Clone, Default)]
pub struct StartingStyleRule {
    /// Selector for the rule (e.g., ".dialog", "dialog[open]")
    pub selector: String,
    /// Declarations to apply
    pub declarations: Vec<Declaration>,
}

/// A CSS @property rule (CSS Houdini)
#[derive(Debug, Clone, Default)]
pub struct PropertyRule {
    /// Property name (e.g., "--my-color")
    pub name: String,
    /// Syntax definition (e.g., "<color>", "<length>", "<number>")
    pub syntax: Option<String>,
    /// Whether the property inherits from parent
    pub inherits: bool,
    /// Initial value
    pub initial_value: Option<String>,
}

/// A CSS @counter-style rule
#[derive(Debug, Clone, Default)]
pub struct CounterStyleRule {
    /// System type (e.g., cyclic, numeric, alphabetic, symbolic, additive, fixed, extends)
    pub system: Option<String>,
    /// Symbols to use for markers
    pub symbols: Vec<String>,
    /// Fallback style name
    pub fallback: Option<String>,
    /// Prefix string
    pub prefix: Option<String>,
    /// Suffix string
    pub suffix: Option<String>,
    /// Range constraints
    pub range: Option<String>,
    /// Pad string for fixed-width markers
    pub pad: Option<String>,
    /// Spoken form for accessibility
    pub speak_as: Option<String>,
}

/// A CSS @font-face rule
#[derive(Debug, Clone, Default)]
pub struct FontFaceRule {
    /// Font family name (e.g., "MyFont")
    pub font_family: Option<String>,
    /// URL to the font file
    pub src: Option<String>,
    /// Font format hint (e.g., "woff2", "ttf")
    pub format: Option<String>,
    /// Font weight (e.g., "normal", "bold", "400")
    pub font_weight: Option<String>,
    /// Font style (e.g., "normal", "italic")
    pub font_style: Option<String>,
    /// Unicode range for subset fonts
    pub unicode_range: Option<String>,
}

/// A CSS @import rule
#[derive(Debug, Clone)]
pub struct ImportRule {
    /// URL of the stylesheet to import
    pub url: String,
    /// Optional media query (e.g., "screen", "print")
    pub media: Option<String>,
}

/// A CSS @keyframes rule for animations
#[derive(Debug, Clone)]
pub struct Keyframes {
    pub name: String,
    /// Keyframe selectors (0% to 100%) and their declarations
    pub frames: Vec<Keyframe>,
}

/// A single keyframe (e.g., 0%, 50%, 100%)
#[derive(Debug, Clone)]
pub struct Keyframe {
    /// Selector percentages (e.g., [0.0, 100.0] for "0%, 100%")
    pub selectors: Vec<f32>,
    pub declarations: Vec<Declaration>,
}

/// A CSS rule: selectors + declarations.
#[derive(Debug, Clone)]
pub struct Rule {
    pub selectors: Vec<Selector>,
    pub declarations: Vec<Declaration>,
    pub nested_rules: Vec<Rule>,
}

/// Simplified CSS selector.
#[derive(Debug, Clone)]
pub enum Selector {
    /// Universal selector `*`
    Universal,
    /// Tag selector like `p`, `div`
    Tag(String),
    /// Class selector `.foo`
    Class(String),
    /// ID selector `#foo`
    Id(String),
    /// Attribute selector: [attr], [attr=val], [attr~=val], [attr|=val]
    Attribute(String, Option<String>),
    /// Compound: tag.class, tag#id, .class1.class2
    Compound(Vec<Selector>),
    /// Descendant: `.foo .bar` — bar inside foo (any depth)
    Descendant(Box<Selector>, Box<Selector>),
    /// Child: `.foo > .bar` — bar is direct child of foo
    Child(Box<Selector>, Box<Selector>),
    /// Adjacent sibling: `.foo + .bar` — bar immediately follows foo
    AdjacentSibling(Box<Selector>, Box<Selector>),
    /// General sibling: `.foo ~ .bar` — bar follows foo (any distance)
    GeneralSibling(Box<Selector>, Box<Selector>),
    /// :empty — matches elements with no children
    Empty,
    /// Nesting selector `&` - refers to the parent selector in nested rules
    Nesting,
    /// :nth-child(an + b) — matches elements based on their position among siblings
    NthChild { a: i32, b: i32 },
    /// :nth-of-type(an + b) — same as NthChild but only counting same tag name
    NthOfType { a: i32, b: i32 },
    /// :is() — matches if any of the inner selectors match (CSS Selectors Level 4)
    /// Takes specificity of the most specific matching selector
    Is(Vec<Selector>),
    /// :where() — same as :is() but with zero specificity (CSS Selectors Level 4)
    Where(Vec<Selector>),
    /// :lang(language-code) — matches elements with specific language (CSS Selectors Level 2)
    Lang(String),
    /// :any-link — matches any link (both :link and :visited) (CSS Selectors Level 4)
    AnyLink,
    /// :local-link — matches links to the same document (CSS Selectors Level 4)
    LocalLink,
    /// :scope — matches the scoping root (CSS Selectors Level 4)
    Scope,
    /// :blank — matches empty or whitespace-only elements (CSS Selectors Level 4)
    Blank,
    /// :current — matches the element currently being displayed (CSS Selectors Level 4)
    /// Used in page navigation, step indicators, etc.
    Current,
    /// :past — matches elements that are "past" the current element (CSS Selectors Level 4)
    /// Used in slide shows, video subtitles, etc.
    Past,
    /// :future — matches elements that are "future" relative to the current element (CSS Selectors Level 4)
    Future,
    /// :playing — matches media elements that are playing (CSS Selectors Level 4)
    Playing,
    /// :paused — matches media elements that are paused (CSS Selectors Level 4)
    Paused,
    /// :seeking — matches media elements that are seeking (CSS Selectors Level 4)
    Seeking,
    /// :valid — matches form elements with valid input (CSS Selectors Level 4)
    Valid,
    /// :invalid — matches form elements with invalid input (CSS Selectors Level 4)
    Invalid,
    /// :in-range — matches form elements with value in range (CSS Selectors Level 4)
    InRange,
    /// :out-of-range — matches form elements with value out of range (CSS Selectors Level 4)
    OutOfRange,
    /// :required — matches required form elements (CSS Selectors Level 4)
    Required,
    /// :optional — matches optional form elements (CSS Selectors Level 4)
    Optional,
    /// :user-invalid — matches form elements with invalid input after user interaction (CSS Selectors Level 4)
    UserInvalid,
    /// :user-valid — matches form elements with valid input after user interaction (CSS Selectors Level 4)
    UserValid,
    /// :matches() — legacy name for :is() (CSS Selectors Level 4, deprecated)
    /// Kept for backwards compatibility with older CSS
    Matches(Vec<Selector>),
    /// :read-only — matches elements that are not user-editable (CSS Selectors Level 4)
    ReadOnly,
    /// :read-write — matches elements that are user-editable (CSS Selectors Level 4)
    ReadWrite,
    /// :placeholder-shown — matches inputs showing placeholder text (CSS Selectors Level 4)
    PlaceholderShown,
    /// :default — matches default form elements (CSS Selectors Level 4)
    Default,
    /// :checked — matches checked checkboxes/radio buttons (CSS Selectors Level 3)
    Checked,
    /// :indeterminate — matches indeterminate checkboxes (CSS Selectors Level 4)
    Indeterminate,
    /// :target — matches the target element of the document URL fragment (CSS Selectors Level 4)
    Target,
    /// :enabled — matches enabled form elements (CSS Selectors Level 3)
    Enabled,
    /// :disabled — matches disabled form elements (CSS Selectors Level 3)
    Disabled,
    /// :root — matches the root element of the document (CSS Selectors Level 3)
    Root,
    /// :not() — matches elements that do not match the inner selector (CSS Selectors Level 3)
    Not(Box<Selector>),
    /// :first-child — matches first child of its parent (CSS Selectors Level 2)
    FirstChild,
    /// :last-child — matches last child of its parent (CSS Selectors Level 3)
    LastChild,
    /// :only-child — matches element that is the only child (CSS Selectors Level 3)
    OnlyChild,
    /// :first-of-type — matches first sibling of its type (CSS Selectors Level 3)
    FirstOfType,
    /// :last-of-type — matches last sibling of its type (CSS Selectors Level 3)
    LastOfType,
    /// :only-of-type — matches element that is the only sibling of its type (CSS Selectors Level 3)
    OnlyOfType,
    /// ::before pseudo-element (CSS Level 2)
    Before,
    /// ::after pseudo-element (CSS Level 2)
    After,
}

impl Selector {
    /// Compute specificity as (id_count, class_count, tag_count).
    pub fn specificity(&self) -> (u32, u32, u32) {
        match self {
            Selector::Universal | Selector::Nesting => (0, 0, 0),
            Selector::Tag(_) => (0, 0, 1),
            Selector::Class(_) => (0, 1, 0),
            Selector::Attribute(_, _) => (0, 1, 0),
            Selector::Empty => (0, 1, 0),
            Selector::Id(_) => (1, 0, 0),
            Selector::Compound(parts) => {
                let mut spec = (0u32, 0u32, 0u32);
                for part in parts {
                    let s = part.specificity();
                    spec.0 += s.0;
                    spec.1 += s.1;
                    spec.2 += s.2;
                }
                spec
            }
            Selector::Descendant(a, d)
            | Selector::Child(a, d)
            | Selector::AdjacentSibling(a, d)
            | Selector::GeneralSibling(a, d) => {
                let sa = a.specificity();
                let sd = d.specificity();
                (sa.0 + sd.0, sa.1 + sd.1, sa.2 + sd.2)
            }
            // Nth-child and Nth-of-type have same specificity as a pseudo-class
            Selector::NthChild { .. } | Selector::NthOfType { .. } => (0, 1, 0),
            // :is() takes specificity of most specific matching selector
            // For simplicity, we use the max specificity of any inner selector
            Selector::Is(selectors) => {
                let mut max_spec = (0u32, 0u32, 0u32);
                for sel in selectors {
                    let s = sel.specificity();
                    if s > max_spec {
                        max_spec = s;
                    }
                }
                max_spec
            }
            // :where() has zero specificity
            Selector::Where(_) => (0, 0, 0),
            // :lang() has same specificity as a pseudo-class
            Selector::Lang(_) => (0, 1, 0),
            // :any-link has same specificity as a pseudo-class
            Selector::AnyLink => (0, 1, 0),
            // :local-link has same specificity as a pseudo-class
            Selector::LocalLink => (0, 1, 0),
            // :scope has same specificity as a pseudo-class
            Selector::Scope => (0, 1, 0),
            // :blank has same specificity as a pseudo-class
            Selector::Blank => (0, 1, 0),
            // :current has same specificity as a pseudo-class
            Selector::Current => (0, 1, 0),
            // :past has same specificity as a pseudo-class
            Selector::Past => (0, 1, 0),
            // :future has same specificity as a pseudo-class
            Selector::Future => (0, 1, 0),
            // Media state pseudo-classes have same specificity
            Selector::Playing => (0, 1, 0),
            Selector::Paused => (0, 1, 0),
            Selector::Seeking => (0, 1, 0),
            // Form validation pseudo-classes have same specificity
            Selector::Valid => (0, 1, 0),
            Selector::Invalid => (0, 1, 0),
            Selector::InRange => (0, 1, 0),
            Selector::OutOfRange => (0, 1, 0),
            Selector::Required => (0, 1, 0),
            Selector::Optional => (0, 1, 0),
            // User-interacted form validation pseudo-classes
            Selector::UserInvalid => (0, 1, 0),
            Selector::UserValid => (0, 1, 0),
            // :matches() is legacy name for :is(), same specificity behavior
            Selector::Matches(selectors) => {
                let mut max_spec = (0u32, 0u32, 0u32);
                for sel in selectors {
                    let s = sel.specificity();
                    if s > max_spec {
                        max_spec = s;
                    }
                }
                max_spec
            }
            // :read-only/:read-write have same specificity as pseudo-classes
            Selector::ReadOnly => (0, 1, 0),
            Selector::ReadWrite => (0, 1, 0),
            // :placeholder-shown has same specificity as a pseudo-class
            Selector::PlaceholderShown => (0, 1, 0),
            // Form state pseudo-classes have same specificity
            Selector::Default => (0, 1, 0),
            Selector::Checked => (0, 1, 0),
            Selector::Indeterminate => (0, 1, 0),
            // :target has same specificity as a pseudo-class
            Selector::Target => (0, 1, 0),
            // :enabled/:disabled have same specificity as pseudo-classes
            Selector::Enabled => (0, 1, 0),
            Selector::Disabled => (0, 1, 0),
            // Structural pseudo-classes have same specificity
            Selector::Root => (0, 1, 0),
            // :not() takes specificity of its inner selector
            Selector::Not(inner) => inner.specificity(),
            Selector::FirstChild => (0, 1, 0),
            Selector::LastChild => (0, 1, 0),
            Selector::OnlyChild => (0, 1, 0),
            Selector::FirstOfType => (0, 1, 0),
            Selector::LastOfType => (0, 1, 0),
            Selector::OnlyOfType => (0, 1, 0),
            // Pseudo-elements have same specificity as elements (0,0,1)
            Selector::Before => (0, 0, 1),
            Selector::After => (0, 0, 1),
        }
    }

    /// Check if this simple selector matches an element (no ancestor check).
    pub fn matches_element(&self, element: &ElementData) -> bool {
        match self {
            Selector::Universal | Selector::Nesting => true,
            Selector::Tag(tag) => element.tag_name == *tag,
            Selector::Class(class) => element.classes().contains(&class.as_str()),
            Selector::Attribute(attr, val) => match val {
                Some(v) => element.get_attr(attr).map(|a| a == v).unwrap_or(false),
                None => element.get_attr(attr).is_some(),
            },
            Selector::Id(id) => element.id() == Some(id.as_str()),
            Selector::Compound(parts) => parts.iter().all(|p| p.matches_element(element)),
            // Pseudo-elements don't match actual elements
            Selector::Before | Selector::After => false,
            // For descendant/child, only check the rightmost part
            Selector::Descendant(_, descendant) => descendant.matches_element(element),
            Selector::Child(_, child) => child.matches_element(element),
            Selector::AdjacentSibling(_, target) | Selector::GeneralSibling(_, target) => {
                target.matches_element(element)
            }
            Selector::Empty => false, // Can't determine without DOM context
            // NthChild/NthOfType - for now treat as always matching
            // Full implementation would require counting siblings
            Selector::NthChild { .. } | Selector::NthOfType { .. } => true,
            // :is() matches if any inner selector matches
            Selector::Is(selectors) => selectors.iter().any(|s| s.matches_element(element)),
            // :where() same as :is() but with zero specificity
            Selector::Where(selectors) => selectors.iter().any(|s| s.matches_element(element)),
            // :lang() matches if the element has a lang attribute with the given value
            Selector::Lang(code) => {
                element
                    .get_attr("lang")
                    .map(|l| {
                        let l_lower = l.to_lowercase();
                        let code_lower = code.to_lowercase();
                        // Match exact code or prefix (e.g., "en" matches "en-US")
                        l_lower == code_lower || l_lower.starts_with(&format!("{}-", code_lower))
                    })
                    .unwrap_or(false)
            }
            // :any-link matches link elements (a, area, link) with href attribute
            Selector::AnyLink => {
                matches!(element.tag_name.as_str(), "a" | "area" | "link")
                    && element.get_attr("href").is_some()
            }
            // :local-link matches links to the same document
            // (href starts with # or is empty/relative without host)
            Selector::LocalLink => element
                .get_attr("href")
                .map(|href| {
                    href.starts_with('#') || (!href.contains("://") && !href.starts_with("//"))
                })
                .unwrap_or(false),
            // :scope matches the scoping root
            // In document context, this is typically the html element
            Selector::Scope => element.tag_name == "html",
            // :blank matches empty or whitespace-only elements
            // Requires document context to check content, so false here
            Selector::Blank => false,
            // :current matches the currently displayed element
            // Requires document state context, so false here
            Selector::Current => false,
            // :past matches elements past the current element
            // Requires document state context, so false here
            Selector::Past => false,
            // :future matches elements future relative to the current element
            // Requires document state context, so false here
            Selector::Future => false,
            // Media state pseudo-classes require media element state
            // For now, match if element is audio or video tag
            Selector::Playing => {
                matches!(element.tag_name.as_str(), "audio" | "video")
            }
            Selector::Paused => {
                matches!(element.tag_name.as_str(), "audio" | "video")
            }
            Selector::Seeking => {
                matches!(element.tag_name.as_str(), "audio" | "video")
            }
            // Form validation pseudo-classes require form state
            // For now, match if element is a form input element
            Selector::Valid => {
                matches!(element.tag_name.as_str(), "input" | "textarea" | "select")
            }
            Selector::Invalid => {
                matches!(element.tag_name.as_str(), "input" | "textarea" | "select")
            }
            Selector::InRange => {
                matches!(element.tag_name.as_str(), "input")
            }
            Selector::OutOfRange => {
                matches!(element.tag_name.as_str(), "input")
            }
            Selector::Required => element.get_attr("required").is_some(),
            Selector::Optional => {
                matches!(element.tag_name.as_str(), "input" | "textarea" | "select")
                    && element.get_attr("required").is_none()
            }
            // :user-invalid/:user-valid require user interaction tracking
            // For now, match if element is a form input element
            Selector::UserInvalid => {
                matches!(element.tag_name.as_str(), "input" | "textarea" | "select")
            }
            Selector::UserValid => {
                matches!(element.tag_name.as_str(), "input" | "textarea" | "select")
            }
            // :matches() is legacy name for :is()
            Selector::Matches(selectors) => selectors.iter().any(|s| s.matches_element(element)),
            // :read-only/:read-write require contenteditable/readonly tracking
            // For now, match if element is a form input element
            Selector::ReadOnly => {
                matches!(element.tag_name.as_str(), "input" | "textarea")
                    && (element.get_attr("readonly").is_some()
                        || element.get_attr("disabled").is_some())
            }
            Selector::ReadWrite => {
                matches!(element.tag_name.as_str(), "input" | "textarea")
                    && element.get_attr("readonly").is_none()
                    && element.get_attr("disabled").is_none()
            }
            // :placeholder-shown matches inputs with placeholder attribute
            Selector::PlaceholderShown => {
                matches!(element.tag_name.as_str(), "input" | "textarea")
                    && element.get_attr("placeholder").is_some()
            }
            // :default matches default form elements (buttons, options)
            Selector::Default => {
                matches!(element.tag_name.as_str(), "button" | "input" | "option")
                    && element.get_attr("default").is_some()
            }
            // :checked matches checked checkboxes/radio buttons
            Selector::Checked => {
                matches!(element.tag_name.as_str(), "input")
                    && matches!(
                        element.get_attr("type"),
                        Some("checkbox") | Some("radio")
                    )
                    && element.get_attr("checked").is_some()
            }
            // :indeterminate matches indeterminate checkboxes
            Selector::Indeterminate => {
                matches!(element.tag_name.as_str(), "input")
                    && matches!(element.get_attr("type"), Some("checkbox"))
            }
            // :target matches the element with the target id
            // Requires document URL context, so false here
            Selector::Target => false,
            // :enabled/:disabled require form element state
            Selector::Enabled => {
                matches!(
                    element.tag_name.as_str(),
                    "input" | "textarea" | "select" | "button"
                ) && element.get_attr("disabled").is_none()
            }
            Selector::Disabled => {
                matches!(
                    element.tag_name.as_str(),
                    "input" | "textarea" | "select" | "button"
                ) && element.get_attr("disabled").is_some()
            }
            // :root matches the root element (html)
            Selector::Root => element.tag_name == "html",
            // :not() matches if inner selector doesn't match
            Selector::Not(inner) => !inner.matches_element(element),
            // Structural pseudo-classes require document context
            // For now, return false here and handle in matches()
            Selector::FirstChild => false,
            Selector::LastChild => false,
            Selector::OnlyChild => false,
            Selector::FirstOfType => false,
            Selector::LastOfType => false,
            Selector::OnlyOfType => false,
        }
    }

    /// Check if this selector matches a given element in the document context.
    pub fn matches(&self, element: &ElementData, doc: &Document, node_id: NodeId) -> bool {
        match self {
            Selector::Universal | Selector::Nesting => true,
            Selector::Tag(tag) => element.tag_name == *tag,
            Selector::Class(class) => element.classes().contains(&class.as_str()),
            Selector::Attribute(attr, val) => match val {
                Some(v) => element.get_attr(attr).map(|a| a == v).unwrap_or(false),
                None => element.get_attr(attr).is_some(),
            },
            Selector::Id(id) => element.id() == Some(id.as_str()),
            Selector::Empty => doc.node(node_id).children.is_empty(),
            Selector::Compound(parts) => parts.iter().all(|p| p.matches(element, doc, node_id)),
            Selector::Descendant(ancestor, descendant) => {
                if !descendant.matches(element, doc, node_id) {
                    return false;
                }
                // Walk up ancestors
                let mut current = doc.node(node_id).parent;
                while let Some(parent_id) = current {
                    if let NodeData::Element(ref parent_el) = doc.node(parent_id).data {
                        if ancestor.matches(parent_el, doc, parent_id) {
                            return true;
                        }
                    }
                    current = doc.node(parent_id).parent;
                }
                false
            }
            Selector::Child(parent_sel, child_sel) => {
                if !child_sel.matches(element, doc, node_id) {
                    return false;
                }
                if let Some(parent_id) = doc.node(node_id).parent {
                    if let NodeData::Element(ref parent_el) = doc.node(parent_id).data {
                        return parent_sel.matches(parent_el, doc, parent_id);
                    }
                }
                false
            }
            Selector::AdjacentSibling(prev_sel, target_sel) => {
                if !target_sel.matches(element, doc, node_id) {
                    return false;
                }
                // Check immediately preceding element sibling
                if let Some(parent_id) = doc.node(node_id).parent {
                    let siblings = &doc.node(parent_id).children;
                    let mut prev_elem: Option<NodeId> = None;
                    for &sid in siblings {
                        if sid == node_id {
                            if let Some(p) = prev_elem {
                                if let NodeData::Element(ref e) = doc.node(p).data {
                                    return prev_sel.matches(e, doc, p);
                                }
                            }
                            return false;
                        }
                        if matches!(&doc.node(sid).data, NodeData::Element(_)) {
                            prev_elem = Some(sid);
                        }
                    }
                }
                false
            }
            Selector::GeneralSibling(prev_sel, target_sel) => {
                if !target_sel.matches(element, doc, node_id) {
                    return false;
                }
                // Check any preceding element sibling
                if let Some(parent_id) = doc.node(node_id).parent {
                    let siblings = &doc.node(parent_id).children;
                    for &sid in siblings {
                        if sid == node_id {
                            return false;
                        }
                        if let NodeData::Element(ref e) = doc.node(sid).data {
                            if prev_sel.matches(e, doc, sid) {
                                return true;
                            }
                        }
                    }
                }
                false
            }
            // NthChild and NthOfType - simplified implementation
            // For now, just check if element index matches the formula
            Selector::NthChild { a, b } => {
                // Get element index among siblings
                if let Some(parent_id) = doc.node(node_id).parent {
                    let siblings = &doc.node(parent_id).children;
                    let mut elem_index: i32 = 0;
                    for &sid in siblings {
                        if sid == node_id {
                            break;
                        }
                        if matches!(&doc.node(sid).data, NodeData::Element(_)) {
                            elem_index += 1;
                        }
                    }
                    // Check if (a * n + b) matches elem_index + 1 for some integer n >= 0
                    let pos = elem_index + 1; // CSS uses 1-based indexing
                    check_nth_formula(*a, *b, pos)
                } else {
                    false
                }
            }
            Selector::NthOfType { a, b } => {
                // Get element index among siblings of same type
                if let Some(parent_id) = doc.node(node_id).parent {
                    let siblings = &doc.node(parent_id).children;
                    let mut elem_index: i32 = 0;
                    for &sid in siblings {
                        if sid == node_id {
                            break;
                        }
                        if let NodeData::Element(ref e) = &doc.node(sid).data {
                            if e.tag_name == element.tag_name {
                                elem_index += 1;
                            }
                        }
                    }
                    let pos = elem_index + 1;
                    check_nth_formula(*a, *b, pos)
                } else {
                    false
                }
            }
            // :is() matches if any inner selector matches
            Selector::Is(selectors) => selectors.iter().any(|s| s.matches(element, doc, node_id)),
            // :where() same as :is() but with zero specificity
            Selector::Where(selectors) => {
                selectors.iter().any(|s| s.matches(element, doc, node_id))
            }
            // :lang() delegates to matches_element for lang attribute check
            Selector::Lang(_) => self.matches_element(element),
            // :any-link delegates to matches_element
            Selector::AnyLink => self.matches_element(element),
            // :local-link delegates to matches_element
            Selector::LocalLink => self.matches_element(element),
            // :scope delegates to matches_element
            Selector::Scope => self.matches_element(element),
            // :blank - for now treat as always matching
            // Full implementation would check if element has no children or only whitespace
            Selector::Blank => true,
            // :current - for now treat as always matching
            // Full implementation would track currently displayed element state
            Selector::Current => true,
            // :past - for now treat as always matching
            // Full implementation would track timeline state
            Selector::Past => true,
            // :future - for now treat as always matching
            // Full implementation would track timeline state
            Selector::Future => true,
            // Media state pseudo-classes delegate to matches_element
            // Full implementation would check actual media playback state
            Selector::Playing => self.matches_element(element),
            Selector::Paused => self.matches_element(element),
            Selector::Seeking => self.matches_element(element),
            // Form validation pseudo-classes delegate to matches_element
            // Full implementation would check actual form validation state
            Selector::Valid => self.matches_element(element),
            Selector::Invalid => self.matches_element(element),
            Selector::InRange => self.matches_element(element),
            Selector::OutOfRange => self.matches_element(element),
            Selector::Required => self.matches_element(element),
            Selector::Optional => self.matches_element(element),
            // User-interacted form validation delegates to matches_element
            Selector::UserInvalid => self.matches_element(element),
            Selector::UserValid => self.matches_element(element),
            // :matches() is legacy name for :is()
            Selector::Matches(selectors) => {
                selectors.iter().any(|s| s.matches(element, doc, node_id))
            }
            // :read-only/:read-write delegate to matches_element
            Selector::ReadOnly => self.matches_element(element),
            Selector::ReadWrite => self.matches_element(element),
            // :placeholder-shown delegates to matches_element
            Selector::PlaceholderShown => self.matches_element(element),
            // Form state pseudo-classes delegate to matches_element
            Selector::Default => self.matches_element(element),
            Selector::Checked => self.matches_element(element),
            Selector::Indeterminate => self.matches_element(element),
            // :target - for now treat as always matching
            // Full implementation would check document URL fragment
            Selector::Target => true,
            // :enabled/:disabled delegate to matches_element
            Selector::Enabled => self.matches_element(element),
            Selector::Disabled => self.matches_element(element),
            // Structural pseudo-classes
            Selector::Root => self.matches_element(element),
            // :not() - matches if inner doesn't match
            Selector::Not(inner) => !inner.matches(element, doc, node_id),
            // First-child: element is first among its siblings
            Selector::FirstChild => {
                if let Some(parent_id) = doc.node(node_id).parent {
                    let siblings = &doc.node(parent_id).children;
                    // Check if first sibling is an element
                    siblings
                        .first()
                        .map(|sid| {
                            if let NodeData::Element(_) = &doc.node(*sid).data {
                                *sid == node_id
                            } else {
                                // If first is not element, check second, etc.
                                siblings
                                    .iter()
                                    .skip(1)
                                    .find(|s| matches!(&doc.node(**s).data, NodeData::Element(_)))
                                    == Some(&node_id)
                            }
                        })
                        .unwrap_or(false)
                } else {
                    false
                }
            }
            // Last-child: element is last among its siblings
            Selector::LastChild => {
                if let Some(parent_id) = doc.node(node_id).parent {
                    let siblings = &doc.node(parent_id).children;
                    siblings
                        .last()
                        .map(|sid| {
                            if let NodeData::Element(_) = &doc.node(*sid).data {
                                *sid == node_id
                            } else {
                                siblings
                                    .iter()
                                    .rev()
                                    .skip(1)
                                    .find(|s| matches!(&doc.node(**s).data, NodeData::Element(_)))
                                    == Some(&node_id)
                            }
                        })
                        .unwrap_or(false)
                } else {
                    false
                }
            }
            // Only-child: element is the only element child
            Selector::OnlyChild => {
                if let Some(parent_id) = doc.node(node_id).parent {
                    let siblings = &doc.node(parent_id).children;
                    let element_count = siblings
                        .iter()
                        .filter(|sid| matches!(&doc.node(**sid).data, NodeData::Element(_)))
                        .count();
                    element_count == 1
                } else {
                    false
                }
            }
            // First-of-type: element is first of its tag type
            Selector::FirstOfType => {
                if let Some(parent_id) = doc.node(node_id).parent {
                    let siblings = &doc.node(parent_id).children;
                    siblings.iter().any(|sid| {
                        if let NodeData::Element(ref e) = &doc.node(*sid).data {
                            e.tag_name == element.tag_name
                        } else {
                            false
                        }
                    }) && siblings
                        .iter()
                        .take_while(|sid| **sid != node_id)
                        .all(|sid| {
                            if let NodeData::Element(ref e) = &doc.node(*sid).data {
                                e.tag_name != element.tag_name
                            } else {
                                true
                            }
                        })
                } else {
                    false
                }
            }
            // Last-of-type: element is last of its tag type
            Selector::LastOfType => {
                if let Some(parent_id) = doc.node(node_id).parent {
                    let siblings = &doc.node(parent_id).children;
                    let tag = &element.tag_name;
                    // Check if this element appears in siblings and is the last of its type
                    let element_positions: Vec<usize> = siblings
                        .iter()
                        .enumerate()
                        .filter(|(_, sid)| {
                            if let NodeData::Element(ref e) = &doc.node(**sid).data {
                                e.tag_name == *tag
                            } else {
                                false
                            }
                        })
                        .map(|(idx, _)| idx)
                        .collect();
                    if let Some(last_pos) = element_positions.last() {
                        siblings
                            .get(*last_pos)
                            .map(|sid| *sid == node_id)
                            .unwrap_or(false)
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            // Only-of-type: element is the only one of its tag type
            Selector::OnlyOfType => {
                if let Some(parent_id) = doc.node(node_id).parent {
                    let siblings = &doc.node(parent_id).children;
                    let tag = &element.tag_name;
                    let count = siblings
                        .iter()
                        .filter(|sid| {
                            if let NodeData::Element(ref e) = &doc.node(**sid).data {
                                e.tag_name == *tag
                            } else {
                                false
                            }
                        })
                        .count();
                    count == 1
                } else {
                    false
                }
            }
            // ::before and ::after - pseudo-elements don't match actual elements
            Selector::Before => false,
            Selector::After => false,
        }
    }
}

/// Check if position matches the nth formula (an + b)
fn check_nth_formula(a: i32, b: i32, pos: i32) -> bool {
    if a == 0 {
        return pos == b;
    }
    // Solve: an + b = pos for n >= 0 and integer
    // n = (pos - b) / a
    let numerator = pos - b;
    if numerator < 0 {
        return false;
    }
    if a > 0 {
        numerator % a == 0 && numerator / a >= 0
    } else {
        // Negative a - still valid
        numerator % a == 0
    }
}

/// A CSS property declaration.
#[derive(Debug, Clone)]
pub struct Declaration {
    pub property: String,
    pub value: CssValue,
    pub important: bool,
}

/// CSS values we support.
#[derive(Debug, Clone, PartialEq)]
pub enum CssValue {
    /// A keyword like `block`, `flex`, `bold`, `center`
    Keyword(String),
    /// A length value with unit
    Length(f32, LengthUnit),
    /// A percentage
    Percentage(f32),
    /// A color
    Color(CssColor),
    /// A number (unitless)
    Number(f32),
    /// Multiple values (e.g., margin shorthand)
    List(Vec<CssValue>),
    /// Auto
    Auto,
    /// None
    None,
    /// Inherit
    Inherit,
    /// CSS custom property reference: var(--name) or var(--name, fallback).
    /// Resolved during style cascade by looking up the name in the element's
    /// inherited custom properties.
    Var(String, Option<Box<CssValue>>),
    /// CSS calc() expression
    Calc(CalcExpression),
    /// CSS min() expression: min(100%, 500px)
    Min(Vec<CalcValue>),
    /// CSS max() expression: max(100%, 500px)
    Max(Vec<CalcValue>),
    /// CSS clamp() expression: clamp(200px, 50%, 800px)
    Clamp {
        min: CalcValue,
        val: CalcValue,
        max: CalcValue,
    },
    /// CSS function like linear-gradient(), radial-gradient()
    /// Stores function name and raw arguments for later parsing
    Function { name: String, args: String },
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

impl CalcValue {
    /// Resolve this calc value to pixels given context
    pub fn to_px(&self, parent_font_size: f32, viewport_width: f32, viewport_height: f32) -> f32 {
        match self {
            CalcValue::Px(v) => *v,
            CalcValue::Percent(v) => *v / 100.0, // Return percentage as fraction
            CalcValue::Em(v) => *v * parent_font_size,
            CalcValue::Rem(v) => *v * 16.0, // root em = 16px default
            CalcValue::Vw(v) => *v * viewport_width / 100.0,
            CalcValue::Vh(v) => *v * viewport_height / 100.0,
        }
    }
}

/// Expression for CSS calc() with +, -, *, /
#[derive(Debug, Clone, PartialEq)]
pub enum CalcExpression {
    Value(CalcValue),
    Add(Box<CalcExpression>, Box<CalcExpression>),
    Subtract(Box<CalcExpression>, Box<CalcExpression>),
    Multiply(Box<CalcExpression>, f32),
    Divide(Box<CalcExpression>, f32),
    /// Percentage of containing block dimension
    Percentage(f32),
}

impl CalcExpression {
    /// Evaluate the calc expression to a pixel value
    pub fn evaluate(
        &self,
        parent_font_size: f32,
        viewport_width: f32,
        viewport_height: f32,
        containing_block_size: f32, // For percentage calculations
    ) -> f32 {
        match self {
            CalcExpression::Value(v) => v.to_px(parent_font_size, viewport_width, viewport_height),
            CalcExpression::Percentage(p) => p / 100.0 * containing_block_size,
            CalcExpression::Add(a, b) => {
                a.evaluate(
                    parent_font_size,
                    viewport_width,
                    viewport_height,
                    containing_block_size,
                ) + b.evaluate(
                    parent_font_size,
                    viewport_width,
                    viewport_height,
                    containing_block_size,
                )
            }
            CalcExpression::Subtract(a, b) => {
                a.evaluate(
                    parent_font_size,
                    viewport_width,
                    viewport_height,
                    containing_block_size,
                ) - b.evaluate(
                    parent_font_size,
                    viewport_width,
                    viewport_height,
                    containing_block_size,
                )
            }
            CalcExpression::Multiply(a, f) => {
                a.evaluate(
                    parent_font_size,
                    viewport_width,
                    viewport_height,
                    containing_block_size,
                ) * f
            }
            CalcExpression::Divide(a, f) => {
                if *f == 0.0 {
                    0.0
                } else {
                    a.evaluate(
                        parent_font_size,
                        viewport_width,
                        viewport_height,
                        containing_block_size,
                    ) / f
                }
            }
        }
    }
}

impl CssValue {
    pub fn to_px(
        &self,
        parent_font_size: f32,
        viewport_width: f32,
        viewport_height: f32,
    ) -> Option<f32> {
        match self {
            CssValue::Length(v, LengthUnit::Px) => Some(*v),
            CssValue::Length(v, LengthUnit::Em) => Some(*v * parent_font_size),
            CssValue::Length(v, LengthUnit::Rem) => Some(*v * 16.0), // root em = 16px default
            CssValue::Length(v, LengthUnit::Pt) => Some(*v * 4.0 / 3.0),
            CssValue::Length(v, LengthUnit::Vw) => Some(*v * viewport_width / 100.0),
            CssValue::Length(v, LengthUnit::Vh) => Some(*v * viewport_height / 100.0),
            CssValue::Length(v, LengthUnit::Vmin) => {
                Some(*v * viewport_width.min(viewport_height) / 100.0)
            }
            CssValue::Length(v, LengthUnit::Vmax) => {
                Some(*v * viewport_width.max(viewport_height) / 100.0)
            }
            CssValue::Length(v, LengthUnit::Ex) => Some(*v * parent_font_size * 0.5), // approx 0.5em
            CssValue::Length(v, LengthUnit::Ch) => Some(*v * parent_font_size * 0.5), // approx width of '0'
            CssValue::Length(v, LengthUnit::Cm) => Some(*v * 37.8), // 1cm ≈ 37.8px
            CssValue::Length(v, LengthUnit::Mm) => Some(*v * 3.78), // 1mm ≈ 3.78px
            CssValue::Length(v, LengthUnit::In) => Some(*v * 96.0), // 1in = 96px
            CssValue::Length(v, LengthUnit::Pc) => Some(*v * 16.0), // 1pc = 16px
            CssValue::Number(v) if *v == 0.0 => Some(0.0),
            CssValue::Percentage(p) => Some(*p / 100.0 * parent_font_size),
            // CSS Math Functions - need containing block size for percentages, use viewport_width as fallback
            CssValue::Calc(expr) => Some(expr.evaluate(
                parent_font_size,
                viewport_width,
                viewport_height,
                viewport_width,
            )),
            CssValue::Min(vals) => {
                let resolved: Vec<f32> = vals
                    .iter()
                    .map(|v| v.to_px(parent_font_size, viewport_width, viewport_height))
                    .collect();
                resolved.into_iter().reduce(f32::min)
            }
            CssValue::Max(vals) => {
                let resolved: Vec<f32> = vals
                    .iter()
                    .map(|v| v.to_px(parent_font_size, viewport_width, viewport_height))
                    .collect();
                resolved.into_iter().reduce(f32::max)
            }
            CssValue::Clamp { min, val, max } => {
                let min_px = min.to_px(parent_font_size, viewport_width, viewport_height);
                let val_px = val.to_px(parent_font_size, viewport_width, viewport_height);
                let max_px = max.to_px(parent_font_size, viewport_width, viewport_height);
                Some(val_px.clamp(min_px, max_px))
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LengthUnit {
    Px,
    Em,
    Rem,
    Pt,
    Percent,
    Vw,
    Vh,
    Fr,
    /// vmin - 1% of the smaller dimension of viewport
    Vmin,
    /// vmax - 1% of the larger dimension of viewport
    Vmax,
    /// ex - height of lowercase 'x' (approx 0.5em)
    Ex,
    /// ch - width of '0' character
    Ch,
    /// cm - centimeters
    Cm,
    /// mm - millimeters
    Mm,
    /// in - inches (1in = 96px)
    In,
    /// pc - picas (1pc = 16px)
    Pc,
}

fn unit_to_str(u: LengthUnit) -> &'static str {
    match u {
        LengthUnit::Px => "px",
        LengthUnit::Em => "em",
        LengthUnit::Rem => "rem",
        LengthUnit::Pt => "pt",
        LengthUnit::Percent => "%",
        LengthUnit::Vw => "vw",
        LengthUnit::Vh => "vh",
        LengthUnit::Fr => "fr",
        LengthUnit::Vmin => "vmin",
        LengthUnit::Vmax => "vmax",
        LengthUnit::Ex => "ex",
        LengthUnit::Ch => "ch",
        LengthUnit::Cm => "cm",
        LengthUnit::Mm => "mm",
        LengthUnit::In => "in",
        LengthUnit::Pc => "pc",
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CssColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl CssColor {
    pub const BLACK: Self = CssColor {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };
    pub const WHITE: Self = CssColor {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };
    pub const TRANSPARENT: Self = CssColor {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };

    pub fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        CssColor { r, g, b, a: 255 }
    }

    pub fn from_rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        CssColor { r, g, b, a }
    }
}

/// Parse a CSS string into a Stylesheet.
pub fn parse_css(input: &str) -> Stylesheet {
    let mut stylesheet = Stylesheet::default();
    let mut pi = ParserInput::new(input);
    let mut parser = Parser::new(&mut pi);

    while !parser.is_exhausted() {
        let state = parser.state();
        match parser.next() {
            Ok(Token::AtKeyword(ref kw)) => {
                let keyword = kw.to_string().to_lowercase();
                if keyword == "import" {
                    // @import "url" or @import url("...")
                    // Parse the URL
                    let url = if let Ok(token) = parser.next() {
                        match token {
                            Token::QuotedString(s) => s.to_string(),
                            Token::UnquotedUrl(url) => url.to_string(),
                            Token::Function(ref name) if name.eq_ignore_ascii_case("url") => {
                                // Parse url("...") or url('...')
                                let result: Result<String, ParseError<'_, ()>> = parser
                                    .parse_nested_block(|p| {
                                        if let Ok(Token::QuotedString(s)) = p.next() {
                                            Ok(s.to_string())
                                        } else {
                                            Ok(String::new())
                                        }
                                    });
                                result.unwrap_or_default()
                            }
                            _ => String::new(),
                        }
                    } else {
                        String::new()
                    };

                    // Parse optional media query
                    let mut media = None;
                    let _media_start = parser.state();
                    // Collect tokens until semicolon for media query
                    let mut media_parts = Vec::new();
                    while let Ok(token) = parser.next() {
                        if matches!(token, Token::Semicolon) {
                            break;
                        }
                        if let Token::Ident(s) = token {
                            media_parts.push(s.to_string());
                        }
                    }
                    if !media_parts.is_empty() {
                        media = Some(media_parts.join(" "));
                    }

                    if !url.is_empty() {
                        stylesheet.imports.push(ImportRule { url, media });
                    }
                    continue;
                } else if keyword == "media" {
                    // Check if this media query applies to us (screen, min-width <= 1024)
                    let applies = should_apply_media_query(&mut parser);
                    // Now consume the CurlyBracketBlock
                    if let Ok(&Token::CurlyBracketBlock) = parser.next() {
                        if applies {
                            let _: Result<(), ParseError<'_, ()>> =
                                parser.parse_nested_block(|p| {
                                    while !p.is_exhausted() {
                                        if let Ok(rule) = parse_rule(p, None) {
                                            stylesheet.rules.push(rule.clone());
                                            flatten_nested_rules(&rule, &mut stylesheet.rules);
                                        } else {
                                            let _ = p.next();
                                        }
                                    }
                                    Ok(())
                                });
                        } else {
                            // Skip the block contents
                            let _: Result<(), ParseError<'_, ()>> =
                                parser.parse_nested_block(|p| {
                                    while p.next().is_ok() {}
                                    Ok(())
                                });
                        }
                    }
                } else if keyword == "supports" {
                    // @supports - treat as applying (skip the condition, parse the block)
                    // Skip until we hit CurlyBracketBlock
                    while let Ok(token) = parser.next() {
                        if matches!(token, Token::CurlyBracketBlock) {
                            break;
                        }
                    }
                    // Parse the block contents
                    let _: Result<(), ParseError<'_, ()>> = parser.parse_nested_block(|p| {
                        while !p.is_exhausted() {
                            if let Ok(rule) = parse_rule(p, None) {
                                stylesheet.rules.push(rule.clone());
                                flatten_nested_rules(&rule, &mut stylesheet.rules);
                            } else {
                                let _ = p.next();
                            }
                        }
                        Ok(())
                    });
                } else if keyword == "container" {
                    // @container - treat as applying (skip the condition, parse the block)
                    // Skip until we hit CurlyBracketBlock
                    while let Ok(token) = parser.next() {
                        if matches!(token, Token::CurlyBracketBlock) {
                            break;
                        }
                    }
                    // Parse the block contents
                    let _: Result<(), ParseError<'_, ()>> = parser.parse_nested_block(|p| {
                        while !p.is_exhausted() {
                            if let Ok(rule) = parse_rule(p, None) {
                                stylesheet.rules.push(rule.clone());
                                flatten_nested_rules(&rule, &mut stylesheet.rules);
                            } else {
                                let _ = p.next();
                            }
                        }
                        Ok(())
                    });
                } else if keyword == "keyframes"
                    || keyword == "-webkit-keyframes"
                    || keyword == "-moz-keyframes"
                {
                    // @keyframes animation-name { ... }
                    // Parse the animation name
                    let anim_name = if let Ok(Token::Ident(name)) = parser.next() {
                        name.to_string()
                    } else {
                        String::new()
                    };
                    // Consume the block
                    if let Ok(&Token::CurlyBracketBlock) = parser.next() {
                        let keyframes = parser
                            .parse_nested_block(|p| parse_keyframes_block(p, anim_name.clone()));
                        if let Ok(kf) = keyframes {
                            if !kf.name.is_empty() {
                                stylesheet.keyframes.insert(kf.name.clone(), kf);
                            }
                        }
                    }
                } else if keyword == "font-face" {
                    // @font-face { font-family: "MyFont"; src: url("font.woff2"); }
                    if let Ok(&Token::CurlyBracketBlock) = parser.next() {
                        let font_face = parser.parse_nested_block(|p| parse_font_face_block(p));
                        if let Ok(ff) = font_face {
                            stylesheet.font_faces.push(ff);
                        }
                    }
                } else if keyword == "counter-style" {
                    // @counter-style thumbs { system: cyclic; symbols: "\1F44D"; }
                    if let Ok(Token::Ident(name)) = parser.next() {
                        let style_name = name.to_string();
                        if let Ok(&Token::CurlyBracketBlock) = parser.next() {
                            let counter_style =
                                parser.parse_nested_block(|p| parse_counter_style_block(p));
                            if let Ok(cs) = counter_style {
                                if !style_name.is_empty() {
                                    stylesheet.counter_styles.insert(style_name, cs);
                                }
                            }
                        }
                    }
                } else if keyword == "property" {
                    // @property --my-color { syntax: "<color>"; inherits: false; initial-value: #c0ffee; }
                    if let Ok(Token::Ident(name)) = parser.next() {
                        let prop_name = name.to_string();
                        if let Ok(&Token::CurlyBracketBlock) = parser.next() {
                            let property_rule = parser
                                .parse_nested_block(|p| parse_property_block(p, prop_name.clone()));
                            if let Ok(pr) = property_rule {
                                if !prop_name.is_empty() {
                                    stylesheet.properties.insert(prop_name, pr);
                                }
                            }
                        }
                    }
                } else if keyword == "starting-style" {
                    // @starting-style { .dialog { opacity: 0; } }
                    // Parse the block which contains regular rules
                    if let Ok(&Token::CurlyBracketBlock) = parser.next() {
                        let _: Result<(), ParseError<'_, ()>> = parser.parse_nested_block(|p| {
                            while !p.is_exhausted() {
                                if let Ok(rule) = parse_rule(p, None) {
                                    for sel in &rule.selectors {
                                        let sel_str = format!("{:?}", sel);
                                        for decl in &rule.declarations {
                                            stylesheet.starting_styles.push(StartingStyleRule {
                                                selector: sel_str.clone(),
                                                declarations: vec![decl.clone()],
                                            });
                                        }
                                    }
                                } else {
                                    let _ = p.next();
                                }
                            }
                            Ok(())
                        });
                    }
                } else if keyword == "scope" {
                    // @scope (.card) to (.limit) { ... } or @scope (.card) { ... }
                    // Parse the scope root selector
                    let mut root_selector = None;
                    let mut limit_selector = None;

                    // Parse until we hit the block
                    let mut depth = 0;
                    while let Ok(token) = parser.next() {
                        match token {
                            Token::CurlyBracketBlock => {
                                depth = 1;
                                break;
                            }
                            Token::ParenthesisBlock => {
                                // Parse the selector inside parentheses
                                let sel_result: Result<Option<String>, ParseError<'_, ()>> = parser
                                    .parse_nested_block(|p| {
                                        // Collect tokens until we find ")"
                                        let mut parts = Vec::new();
                                        while let Ok(t) = p.next() {
                                            if matches!(t, Token::CloseParenthesis) {
                                                break;
                                            }
                                            if let Token::Ident(s) = t {
                                                parts.push(s.to_string());
                                            } else if let Token::Delim(c) = t {
                                                parts.push(c.to_string());
                                            } else if let Token::IDHash(s) = t {
                                                parts.push(format!("#{}", s));
                                            } else if let Token::Delim('.') = t {
                                                // Class selector - next token should be Ident
                                                if let Ok(Token::Ident(s)) = p.next() {
                                                    parts.push(format!(".{}", s));
                                                }
                                            }
                                        }
                                        if parts.is_empty() {
                                            Ok(None)
                                        } else {
                                            Ok(Some(parts.join("")))
                                        }
                                    });
                                if let Ok(Some(sel)) = sel_result {
                                    if root_selector.is_none() {
                                        root_selector = Some(sel);
                                    } else if limit_selector.is_none() {
                                        limit_selector = Some(sel);
                                    }
                                }
                            }
                            Token::Ident(ref s) if s.eq_ignore_ascii_case("to") => {
                                // "to" keyword before limit
                            }
                            _ => {}
                        }
                    }

                    // Parse the block contents
                    if depth == 1 {
                        let scope_rules: Result<Vec<Rule>, ParseError<'_, ()>> = parser
                            .parse_nested_block(|p| {
                                let mut rules = Vec::new();
                                while !p.is_exhausted() {
                                    if let Ok(rule) = parse_rule(p, None) {
                                        rules.push(rule);
                                    } else {
                                        let _ = p.next();
                                    }
                                }
                                Ok(rules)
                            });
                        if let Ok(rules) = scope_rules {
                            stylesheet.scopes.push(ScopeRule {
                                root: root_selector,
                                limit: limit_selector,
                                rules,
                            });
                        }
                    }
                } else if keyword == "layer" {
                    // @layer - skip for now (could be supported in future)
                    skip_at_rule(&mut parser);
                } else {
                    skip_at_rule(&mut parser);
                }
                continue;
            }
            _ => parser.reset(&state),
        }

        if let Ok(rule) = parse_rule(&mut parser, None) {
            // Collect CSS custom properties (variables) from broad selectors only.
            // Scoped variables (e.g. on .clientpref-2) would overwrite global values
            // incorrectly since we store one value per name. Only store from selectors
            // that are likely to apply broadly: :root, html, body, *, or single-class
            // selectors that match the actual <html> element's classes.
            let is_broad_selector = rule.selectors.iter().any(|s| match s {
                Selector::Universal => true,
                Selector::Tag(t) if t == "html" || t == "body" => true,
                _ => false,
            });
            // For non-broad selectors, only store if this is the first definition
            // (don't let scoped overrides clobber earlier broad definitions).
            for decl in &rule.declarations {
                if decl.property.starts_with("--") {
                    let val_str = match &decl.value {
                        CssValue::Color(c) => format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b),
                        CssValue::Keyword(k) => k.clone(),
                        CssValue::Length(v, u) => format!("{}{}", v, unit_to_str(*u)),
                        CssValue::Number(n) => format!("{}", n),
                        CssValue::Percentage(p) => format!("{}%", p),
                        CssValue::Var(name, _) => format!("var({})", name),
                        _ => String::new(),
                    };
                    if !val_str.is_empty()
                        && (is_broad_selector || !stylesheet.variables.contains_key(&decl.property))
                    {
                        stylesheet.variables.insert(decl.property.clone(), val_str);
                    }
                }
            }
            // Add the rule and flatten any nested rules
            stylesheet.rules.push(rule.clone());
            flatten_nested_rules(&rule, &mut stylesheet.rules);
        } else {
            let _ = parser.next();
        }
    }

    stylesheet
}

/// Flatten nested rules into a flat list of rules
fn flatten_nested_rules(rule: &Rule, rules: &mut Vec<Rule>) {
    for nested in &rule.nested_rules {
        rules.push(nested.clone());
        flatten_nested_rules(nested, rules);
    }
}

/// Check if a @media query applies to our viewport (1024px screen).
fn should_apply_media_query<'i>(parser: &mut Parser<'i, '_>) -> bool {
    let mut state = MediaMatchState::default();
    scan_media_tokens(parser, &mut state);

    if state.has_print_only && !state.has_screen {
        return false;
    }
    if state.has_dark_scheme {
        return false;
    }
    if state.reject {
        return false;
    }
    true
}

#[derive(Default)]
struct MediaMatchState {
    has_print_only: bool,
    has_screen: bool,
    has_dark_scheme: bool,
    last_was_min_width: bool,
    last_was_max_width: bool,
    reject: bool,
}

fn scan_media_tokens<'i>(parser: &mut Parser<'i, '_>, state: &mut MediaMatchState) {
    loop {
        let parser_state = parser.state();
        match parser.next() {
            Ok(&Token::CurlyBracketBlock) => {
                parser.reset(&parser_state);
                break;
            }
            Ok(Token::Ident(ref name)) => {
                let n = name.to_string().to_lowercase();
                match n.as_str() {
                    "print" => state.has_print_only = true,
                    "screen" | "all" => state.has_screen = true,
                    "min-width" => state.last_was_min_width = true,
                    "max-width" => state.last_was_max_width = true,
                    "dark" => state.has_dark_scheme = true,
                    "prefers-color-scheme" => {}
                    _ => {}
                }
            }
            Ok(&Token::Dimension { value, .. }) => {
                let px_val = if value > 100.0 { value } else { value * 16.0 };
                if state.last_was_min_width {
                    if px_val > 1024.0 {
                        state.reject = true;
                    }
                    state.last_was_min_width = false;
                }
                if state.last_was_max_width {
                    if px_val < 1024.0 {
                        state.reject = true;
                    }
                    state.last_was_max_width = false;
                }
            }
            Ok(&Token::Number { value, .. }) => {
                if state.last_was_min_width && value > 1024.0 {
                    state.reject = true;
                }
                if state.last_was_max_width && value < 1024.0 {
                    state.reject = true;
                }
                state.last_was_min_width = false;
                state.last_was_max_width = false;
            }
            Ok(&Token::ParenthesisBlock) => {
                // Parenthesized feature query: `(min-width: 1680px)`
                // Descend into it to inspect the feature and value.
                let _: Result<(), ParseError<'_, ()>> = parser.parse_nested_block(|p| {
                    scan_media_tokens(p, state);
                    Ok(())
                });
            }
            Ok(&Token::Colon) => {
                // `:` between feature name and value — keep flags set.
            }
            Err(_) => return,
            _ => {
                state.last_was_min_width = false;
                state.last_was_max_width = false;
            }
        }
    }
}

/// Skip an at-rule: consume tokens until `;` or `{ block }`.
fn skip_at_rule<'i>(parser: &mut Parser<'i, '_>) {
    loop {
        match parser.next() {
            Ok(&Token::Semicolon) => break,
            Ok(&Token::CurlyBracketBlock) => {
                // Block contents auto-skipped by cssparser on next next() call
                break;
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }
}

/// Parse @keyframes block content
/// Parse a @font-face block containing font descriptors
fn parse_font_face_block<'i>(
    parser: &mut Parser<'i, '_>,
) -> Result<FontFaceRule, ParseError<'i, ()>> {
    let mut font_face = FontFaceRule::default();

    while !parser.is_exhausted() {
        // Parse declaration-like entries: property: value;
        if let Ok(Token::Ident(prop)) = parser.next() {
            let prop_str = prop.to_string().to_lowercase();

            // Expect colon
            if parser.expect_colon().is_err() {
                // Skip to next semicolon
                while let Ok(token) = parser.next() {
                    if matches!(token, Token::Semicolon) {
                        break;
                    }
                }
                continue;
            }

            // Parse the value
            let mut value_parts = Vec::new();
            let in_src = prop_str == "src";

            while let Ok(token) = parser.next() {
                match token {
                    Token::Semicolon => break,
                    Token::QuotedString(s) => {
                        value_parts.push(s.to_string());
                    }
                    Token::UnquotedUrl(url) if in_src => {
                        font_face.src = Some(url.to_string());
                    }
                    Token::Function(ref name) if name.eq_ignore_ascii_case("url") => {
                        let url_result: Result<Option<String>, ParseError<'_, ()>> = parser
                            .parse_nested_block(|p| {
                                if let Ok(Token::QuotedString(s)) = p.next() {
                                    Ok(Some(s.to_string()))
                                } else {
                                    Ok(None)
                                }
                            });
                        if let Ok(Some(url)) = url_result {
                            if in_src {
                                font_face.src = Some(url);
                            }
                        }
                    }
                    Token::Function(ref name) if name.eq_ignore_ascii_case("format") => {
                        let fmt_result: Result<Option<String>, ParseError<'_, ()>> = parser
                            .parse_nested_block(|p| {
                                if let Ok(Token::QuotedString(s)) = p.next() {
                                    Ok(Some(s.to_string()))
                                } else {
                                    Ok(None)
                                }
                            });
                        if let Ok(Some(fmt)) = fmt_result {
                            font_face.format = Some(fmt);
                        }
                    }
                    Token::Ident(ident) => {
                        value_parts.push(ident.to_string());
                    }
                    Token::Number { value, .. } => {
                        value_parts.push(format!("{}", value));
                    }
                    _ => {}
                }
            }

            // Assign to appropriate field
            let value = value_parts.join(" ");
            match prop_str.as_str() {
                "font-family" => {
                    // Remove quotes if present
                    let cleaned = value
                        .trim_start_matches('"')
                        .trim_end_matches('"')
                        .trim_start_matches('\'')
                        .trim_end_matches('\'');
                    font_face.font_family = Some(cleaned.to_string());
                }
                "font-weight" => {
                    font_face.font_weight = Some(value);
                }
                "font-style" => {
                    font_face.font_style = Some(value);
                }
                "font-display" => {
                    // Ignored for now
                }
                "unicode-range" => {
                    font_face.unicode_range = Some(value);
                }
                _ => {}
            }
        } else {
            // Skip unknown tokens
            let _ = parser.next();
        }
    }

    Ok(font_face)
}

/// Parse a @counter-style block
fn parse_counter_style_block<'i>(
    parser: &mut Parser<'i, '_>,
) -> Result<CounterStyleRule, ParseError<'i, ()>> {
    let mut counter_style = CounterStyleRule::default();

    while !parser.is_exhausted() {
        // Parse declaration-like entries: property: value;
        if let Ok(Token::Ident(prop)) = parser.next() {
            let prop_str = prop.to_string().to_lowercase();

            // Expect colon
            if parser.expect_colon().is_err() {
                // Skip to next semicolon
                while let Ok(token) = parser.next() {
                    if matches!(token, Token::Semicolon) {
                        break;
                    }
                }
                continue;
            }

            // Parse the value
            let mut value_parts = Vec::new();

            while let Ok(token) = parser.next() {
                match token {
                    Token::Semicolon => break,
                    Token::QuotedString(s) => {
                        value_parts.push(s.to_string());
                    }
                    Token::Ident(ident) => {
                        value_parts.push(ident.to_string());
                    }
                    Token::Number { value, .. } => {
                        value_parts.push(format!("{}", value));
                    }
                    _ => {}
                }
            }

            // Assign to appropriate field
            let value = value_parts.join(" ");
            match prop_str.as_str() {
                "system" => {
                    counter_style.system = Some(value);
                }
                "symbols" => {
                    // Parse symbols like: "★" "☆" or "\1F44D"
                    counter_style.symbols = value_parts.clone();
                }
                "fallback" => {
                    counter_style.fallback = Some(value);
                }
                "prefix" => {
                    counter_style.prefix = Some(value);
                }
                "suffix" => {
                    counter_style.suffix = Some(value);
                }
                "range" => {
                    counter_style.range = Some(value);
                }
                "pad" => {
                    counter_style.pad = Some(value);
                }
                "speak-as" => {
                    counter_style.speak_as = Some(value);
                }
                _ => {}
            }
        } else {
            // Skip unknown tokens
            let _ = parser.next();
        }
    }

    Ok(counter_style)
}

/// Parse a @property block
fn parse_property_block<'i>(
    parser: &mut Parser<'i, '_>,
    name: String,
) -> Result<PropertyRule, ParseError<'i, ()>> {
    let mut property = PropertyRule {
        name,
        ..Default::default()
    };

    while !parser.is_exhausted() {
        // Parse declaration-like entries: property: value;
        if let Ok(Token::Ident(prop)) = parser.next() {
            let prop_str = prop.to_string().to_lowercase();

            // Expect colon
            if parser.expect_colon().is_err() {
                // Skip to next semicolon
                while let Ok(token) = parser.next() {
                    if matches!(token, Token::Semicolon) {
                        break;
                    }
                }
                continue;
            }

            // Parse the value
            let mut value_parts = Vec::new();

            while let Ok(token) = parser.next() {
                match token {
                    Token::Semicolon => break,
                    Token::QuotedString(s) => {
                        value_parts.push(s.to_string());
                    }
                    Token::Ident(ident) => {
                        value_parts.push(ident.to_string());
                    }
                    Token::Hash(h) => {
                        value_parts.push(format!("#{}", h));
                    }
                    Token::Number { value, .. } => {
                        value_parts.push(format!("{}", value));
                    }
                    Token::WhiteSpace(_) => {
                        // Ignore whitespace between tokens
                    }
                    _ => {}
                }
            }

            // Assign to appropriate field
            let value = value_parts.join(" ");
            match prop_str.as_str() {
                "syntax" => {
                    // syntax: "<color>" or "<length>" etc.
                    property.syntax = Some(value.trim_matches('"').to_string());
                }
                "inherits" => {
                    // inherits: true | false
                    property.inherits = value == "true";
                }
                "initial-value" => {
                    property.initial_value = Some(value);
                }
                _ => {}
            }
        } else {
            // Skip unknown tokens
            let _ = parser.next();
        }
    }

    Ok(property)
}

fn parse_keyframes_block<'i>(
    parser: &mut Parser<'i, '_>,
    name: String,
) -> Result<Keyframes, ParseError<'i, ()>> {
    let mut frames = Vec::new();

    while !parser.is_exhausted() {
        // Parse keyframe selector (e.g., "0%", "50%", "100%", "from", "to")
        let state = parser.state();
        let mut selectors: Vec<f32> = Vec::new();

        // Parse comma-separated list of percentages
        loop {
            match parser.next() {
                Ok(Token::Ident(kw)) => {
                    let kw_lower = kw.to_string().to_lowercase();
                    if kw_lower == "from" {
                        selectors.push(0.0);
                    } else if kw_lower == "to" {
                        selectors.push(100.0);
                    }
                }
                Ok(&Token::Percentage { unit_value, .. }) => {
                    selectors.push(unit_value * 100.0);
                }
                _ => {
                    // Not a keyframe selector, reset and break
                    parser.reset(&state);
                    break;
                }
            }

            // Check for comma
            if parser.try_parse(|p| p.expect_comma()).is_ok() {
                continue;
            } else {
                break;
            }
        }

        if selectors.is_empty() {
            // No valid selectors found, skip to next
            let _ = parser.next();
            continue;
        }

        // Expect CurlyBracketBlock for declarations
        if let Ok(&Token::CurlyBracketBlock) = parser.next() {
            let declarations: Result<Vec<Declaration>, ParseError<'i, ()>> = parser
                .parse_nested_block(|p| {
                    let mut decls = Vec::new();
                    while !p.is_exhausted() {
                        if let Ok(decl) = parse_declaration(p) {
                            decls.push(decl);
                        } else {
                            let _ = p.next();
                        }
                    }
                    Ok(decls)
                });

            if let Ok(decls) = declarations {
                frames.push(Keyframe {
                    selectors,
                    declarations: decls,
                });
            }
        } else {
            // No block found, skip
            break;
        }
    }

    Ok(Keyframes { name, frames })
}

fn parse_rule<'i>(
    parser: &mut Parser<'i, '_>,
    parent_selector: Option<&[Selector]>,
) -> Result<Rule, ParseError<'i, ()>> {
    // Parse selectors, collecting tokens until we hit CurlyBracketBlock
    let selectors = parse_selectors(parser, parent_selector)?;

    // After parse_selectors, the CurlyBracketBlock has been consumed.
    // parse_nested_block will parse inside it.
    let mut declarations = Vec::new();
    let mut nested_rules = Vec::new();
    let _: Result<(), ParseError<'_, ()>> = parser.parse_nested_block(|parser| {
        loop {
            if parser.is_exhausted() {
                break;
            }
            // Check for nested rule: starts with & or type/class/id selector
            // We try to parse as a rule first, and if it fails, parse as declaration
            let checkpoint = parser.state();

            // Try parsing as a nested rule
            match parser.next() {
                Ok(&Token::Delim('&')) => {
                    // Nested selector starting with &
                    parser.reset(&checkpoint);
                    if let Ok(rule) = parse_rule(parser, Some(&selectors)) {
                        nested_rules.push(rule);
                        continue;
                    }
                }
                Ok(&Token::Ident(_)) => {
                    // Might be a nested rule or a declaration
                    // Try lookahead: if we can parse as rule, it's a nested rule
                    parser.reset(&checkpoint);
                    if let Ok(rule) = parse_rule(parser, Some(&selectors)) {
                        nested_rules.push(rule);
                        continue;
                    }
                }
                Ok(&Token::Delim('.')) => {
                    // Lookahead for class selector
                    parser.reset(&checkpoint);
                    if let Ok(rule) = parse_rule(parser, Some(&selectors)) {
                        nested_rules.push(rule);
                        continue;
                    }
                }
                Ok(&Token::Delim('#')) => {
                    // Lookahead for ID selector
                    parser.reset(&checkpoint);
                    if let Ok(rule) = parse_rule(parser, Some(&selectors)) {
                        nested_rules.push(rule);
                        continue;
                    }
                }
                Ok(&Token::SquareBracketBlock) => {
                    // Lookahead for attribute selector
                    parser.reset(&checkpoint);
                    if let Ok(rule) = parse_rule(parser, Some(&selectors)) {
                        nested_rules.push(rule);
                        continue;
                    }
                }
                _ => {}
            }

            // Reset and try as declaration
            parser.reset(&checkpoint);
            if let Ok(decl) = parse_declaration(parser) {
                declarations.push(decl);
            } else {
                // Skip to next semicolon on error
                while let Ok(token) = parser.next() {
                    if matches!(token, Token::Semicolon) {
                        break;
                    }
                }
            }
        }
        Ok(())
    });

    Ok(Rule {
        selectors,
        declarations,
        nested_rules,
    })
}

fn parse_selectors<'i>(
    parser: &mut Parser<'i, '_>,
    parent_selector: Option<&[Selector]>,
) -> Result<Vec<Selector>, ParseError<'i, ()>> {
    let mut selectors = Vec::new();
    let mut consumed_block = false;

    loop {
        // Try to parse a selector (don't propagate error — we can recover)
        if let Ok(sel) = parse_one_selector(parser) {
            selectors.push(sel);
        }

        // After a selector, expect comma (more selectors) or { (block start)
        match parser.next() {
            Ok(&Token::Comma) => continue,
            Ok(&Token::CurlyBracketBlock) => {
                consumed_block = true;
                break;
            }
            Ok(_) => {
                // Unknown token (combinator, pseudo leftover, etc.)
                // Skip forward until we find { to stay in sync
                loop {
                    match parser.next() {
                        Ok(&Token::CurlyBracketBlock) => {
                            consumed_block = true;
                            break;
                        }
                        Ok(_) => continue,
                        Err(_) => break,
                    }
                }
                break;
            }
            Err(_) => break,
        }
    }

    if !consumed_block {
        return Err(parser
            .new_basic_unexpected_token_error(Token::Ident("".into()))
            .into());
    }

    if selectors.is_empty() {
        // We consumed { but got no selectors — consume the block and return error
        let _: Result<(), ParseError<'_, ()>> = parser.parse_nested_block(|p| {
            while p.next().is_ok() {}
            Ok(())
        });
        return Err(parser
            .new_basic_unexpected_token_error(Token::Ident("".into()))
            .into());
    }

    // Expand nesting selectors if we have a parent selector
    if let Some(parent) = parent_selector {
        selectors = expand_nesting(parent, selectors);
    }

    Ok(selectors)
}

/// Check if a selector contains the Nesting variant
fn contains_nesting(sel: &Selector) -> bool {
    match sel {
        Selector::Nesting => true,
        Selector::Compound(parts) => parts.iter().any(contains_nesting),
        Selector::Descendant(a, b)
        | Selector::Child(a, b)
        | Selector::AdjacentSibling(a, b)
        | Selector::GeneralSibling(a, b) => contains_nesting(a) || contains_nesting(b),
        _ => false,
    }
}

/// Expand nesting selectors by combining with parent selector
fn expand_nesting(parent: &[Selector], child: Vec<Selector>) -> Vec<Selector> {
    let mut result = Vec::new();

    for child_sel in child {
        if contains_nesting(&child_sel) {
            // Replace nesting selector with parent
            for parent_sel in parent {
                let expanded = replace_nesting(child_sel.clone(), parent_sel);
                result.push(expanded);
            }
        } else {
            // No nesting - make it a descendant of parent
            if parent.len() == 1 {
                result.push(Selector::Descendant(
                    Box::new(parent[0].clone()),
                    Box::new(child_sel),
                ));
            } else {
                // Multiple parent selectors - :is()
                result.push(Selector::Descendant(
                    Box::new(Selector::Compound(parent.to_vec())),
                    Box::new(child_sel),
                ));
            }
        }
    }

    result
}

/// Replace Nesting selector with the parent selector
fn replace_nesting(sel: Selector, parent: &Selector) -> Selector {
    match sel {
        Selector::Nesting => parent.clone(),
        Selector::Compound(parts) => {
            let new_parts: Vec<Selector> = parts
                .into_iter()
                .map(|p| replace_nesting(p, parent))
                .collect();
            // Flatten if parent is also a compound
            if matches!(parent, Selector::Compound(_)) {
                let mut flattened = Vec::new();
                for part in new_parts {
                    if let Selector::Compound(sub_parts) = part {
                        flattened.extend(sub_parts);
                    } else {
                        flattened.push(part);
                    }
                }
                Selector::Compound(flattened)
            } else {
                Selector::Compound(new_parts)
            }
        }
        Selector::Descendant(a, b) => Selector::Descendant(
            Box::new(replace_nesting(*a, parent)),
            Box::new(replace_nesting(*b, parent)),
        ),
        Selector::Child(a, b) => Selector::Child(
            Box::new(replace_nesting(*a, parent)),
            Box::new(replace_nesting(*b, parent)),
        ),
        Selector::AdjacentSibling(a, b) => Selector::AdjacentSibling(
            Box::new(replace_nesting(*a, parent)),
            Box::new(replace_nesting(*b, parent)),
        ),
        Selector::GeneralSibling(a, b) => Selector::GeneralSibling(
            Box::new(replace_nesting(*a, parent)),
            Box::new(replace_nesting(*b, parent)),
        ),
        _ => sel,
    }
}

/// Parse a simple selector (no combinators). Reads tokens without skipping whitespace
/// so the caller can detect descendant combinators.
fn parse_simple_selector<'i>(parser: &mut Parser<'i, '_>) -> Result<Selector, ParseError<'i, ()>> {
    let mut parts = Vec::new();
    let mut skip_selector = false;

    // Skip leading whitespace
    loop {
        let state = parser.state();
        match parser.next_including_whitespace() {
            Ok(&Token::WhiteSpace(_)) => continue,
            _ => {
                parser.reset(&state);
                break;
            }
        }
    }

    loop {
        let state = parser.state();
        // Use next_including_whitespace so we can detect space = descendant combinator
        match parser.next_including_whitespace() {
            Ok(Token::Ident(ref name)) => {
                parts.push(Selector::Tag(name.to_string().to_lowercase()));
            }
            Ok(Token::Delim('.')) => {
                // Class — next token must be ident (no whitespace between . and name)
                if let Ok(Token::Ident(ref name)) = parser.next_including_whitespace() {
                    parts.push(Selector::Class(name.to_string()));
                }
            }
            Ok(Token::IDHash(ref name)) => {
                parts.push(Selector::Id(name.to_string()));
            }
            Ok(Token::Delim('*')) => {
                parts.push(Selector::Universal);
            }
            // Handle nesting selector `&`
            Ok(Token::Delim('&')) => {
                parts.push(Selector::Nesting);
            }
            // Handle pseudo-classes/elements
            Ok(&Token::Colon) => {
                match parser.next_including_whitespace() {
                    Ok(&Token::Colon) => {
                        // ::pseudo-element (::before, ::after, ::first-line, etc.)
                        // We don't render most pseudo-elements, so mark selector unmatchable
                        // to prevent their rules from being applied to the base element.
                        // However, ::marker is used for list bullets and we should allow it
                        // for basic list styling (even if we don't fully render custom markers).
                        match parser.next_including_whitespace() {
                            Ok(Token::Ident(ref name)) => {
                                let pseudo = name.to_string().to_lowercase();
                                if pseudo == "marker" {
                                    // ::marker - used for list markers, keep selector but
                                    // we'll treat it as applying to the list item itself
                                    // (we don't extract the marker styling separately)
                                } else if pseudo == "before" {
                                    // ::before pseudo-element
                                    parts.push(Selector::Before);
                                } else if pseudo == "after" {
                                    // ::after pseudo-element
                                    parts.push(Selector::After);
                                } else {
                                    // Other pseudo-elements - skip
                                    skip_selector = true;
                                }
                            }
                            _ => {
                                skip_selector = true;
                            }
                        }
                    }
                    Ok(Token::Ident(ref name)) => {
                        // :visited, :hover, :focus, :active represent non-default states
                        // We can't match these, so mark selector as unmatchable
                        let pseudo = name.to_string().to_lowercase();
                        match pseudo.as_str() {
                            "visited" | "hover" | "focus" | "active" | "focus-within"
                            | "focus-visible" | "autofill" => {
                                skip_selector = true;
                            }
                            // :root matches the <html> element
                            "root" => {
                                parts.push(Selector::Root);
                            }
                            // :empty matches elements with no children
                            "empty" => {
                                parts.push(Selector::Empty);
                            }
                            // :is() and :where() - match if any inner selector matches
                            // For simplicity, we treat them as always matching (pass through)
                            "is" | "where" => {}
                            // :has() - complex descendant matching, treat as always matching for now
                            "has" | "has-child" => {}
                            // :marker - list marker pseudo-element (also ::marker)
                            // Single colon variant for backwards compatibility
                            "marker" => {
                                // List marker pseudo-element, treat as matching list items
                            }
                            // :modal - matches modal dialogs and fullscreen elements
                            // Treat as always matching since we don't track modal state
                            "modal" => {}
                            // :open - matches open <details>, <select>, and dialog elements
                            // We check open state for dialog, treat others as matching for now
                            "open" => {}
                            // :closed - opposite of :open
                            "closed" => {
                                // Can't determine closed state without full element tracking
                            }
                            // :popover-open - matches showing popover elements
                            "popover-open" => {}
                            // :any-link - matches any link (same as :link and :visited combined)
                            "any-link" => {
                                parts.push(Selector::AnyLink);
                            }
                            // :local-link - matches links to the same document
                            "local-link" => {
                                parts.push(Selector::LocalLink);
                            }
                            // :scope - matches the scoping root element
                            "scope" => {
                                parts.push(Selector::Scope);
                            }
                            // :blank - matches empty or whitespace-only elements
                            "blank" => {
                                parts.push(Selector::Blank);
                            }
                            // :current - matches the currently displayed element
                            "current" => {
                                parts.push(Selector::Current);
                            }
                            // :past - matches elements past the current element
                            "past" => {
                                parts.push(Selector::Past);
                            }
                            // :future - matches elements future relative to current
                            "future" => {
                                parts.push(Selector::Future);
                            }
                            // :playing - matches playing media elements
                            "playing" => {
                                parts.push(Selector::Playing);
                            }
                            // :paused - matches paused media elements
                            "paused" => {
                                parts.push(Selector::Paused);
                            }
                            // :seeking - matches seeking media elements
                            "seeking" => {
                                parts.push(Selector::Seeking);
                            }
                            // :valid - matches form elements with valid input
                            "valid" => {
                                parts.push(Selector::Valid);
                            }
                            // :invalid - matches form elements with invalid input
                            "invalid" => {
                                parts.push(Selector::Invalid);
                            }
                            // :in-range - matches form elements with value in range
                            "in-range" => {
                                parts.push(Selector::InRange);
                            }
                            // :out-of-range - matches form elements with value out of range
                            "out-of-range" => {
                                parts.push(Selector::OutOfRange);
                            }
                            // :required - matches required form elements
                            "required" => {
                                parts.push(Selector::Required);
                            }
                            // :optional - matches optional form elements
                            "optional" => {
                                parts.push(Selector::Optional);
                            }
                            // :user-invalid - matches invalid after user interaction
                            "user-invalid" => {
                                parts.push(Selector::UserInvalid);
                            }
                            // :user-valid - matches valid after user interaction
                            "user-valid" => {
                                parts.push(Selector::UserValid);
                            }
                            // :read-only - matches elements not user-editable
                            "read-only" => {
                                parts.push(Selector::ReadOnly);
                            }
                            // :read-write - matches elements user-editable
                            "read-write" => {
                                parts.push(Selector::ReadWrite);
                            }
                            // :placeholder-shown - matches inputs showing placeholder
                            "placeholder-shown" => {
                                parts.push(Selector::PlaceholderShown);
                            }
                            // :default - matches default form elements
                            "default" => {
                                parts.push(Selector::Default);
                            }
                            // :checked - matches checked checkboxes/radio buttons
                            "checked" => {
                                parts.push(Selector::Checked);
                            }
                            // :indeterminate - matches indeterminate checkboxes
                            "indeterminate" => {
                                parts.push(Selector::Indeterminate);
                            }
                            // :target - matches the target element of URL fragment
                            "target" => {
                                parts.push(Selector::Target);
                            }
                            // :enabled - matches enabled form elements
                            "enabled" => {
                                parts.push(Selector::Enabled);
                            }
                            // :disabled - matches disabled form elements
                            "disabled" => {
                                parts.push(Selector::Disabled);
                            }
                            // Structural pseudo-classes
                            "first-child" => {
                                parts.push(Selector::FirstChild);
                            }
                            "last-child" => {
                                parts.push(Selector::LastChild);
                            }
                            "only-child" => {
                                parts.push(Selector::OnlyChild);
                            }
                            "first-of-type" => {
                                parts.push(Selector::FirstOfType);
                            }
                            "last-of-type" => {
                                parts.push(Selector::LastOfType);
                            }
                            "only-of-type" => {
                                parts.push(Selector::OnlyOfType);
                            }
                            _ => {}
                        }
                    }
                    Ok(Token::Function(ref fn_name)) => {
                        let fn_lower = fn_name.to_string().to_lowercase();
                        let mut inner_is_simple_negation = false;
                        let mut nth_selector: Option<Selector> = None;
                        let mut is_where_selector: Option<Selector> = None;
                        let _: Result<(), ParseError<'_, ()>> = parser.parse_nested_block(|p| {
                            if fn_lower == "not" {
                                let state = p.state();
                                match p.next() {
                                    // :not(:focus) etc. — state pseudo, always true
                                    Ok(&Token::Colon) => {
                                        if let Ok(Token::Ident(ref name)) = p.next() {
                                            let pseudo = name.to_string().to_lowercase();
                                            if matches!(
                                                pseudo.as_str(),
                                                "hover"
                                                    | "focus"
                                                    | "active"
                                                    | "visited"
                                                    | "focus-within"
                                                    | "focus-visible"
                                                    | "checked"
                                                    | "target"
                                                    | "indeterminate"
                                                    | "placeholder-shown"
                                                    | "default"
                                                    | "required"
                                                    | "invalid"
                                                    | "user-invalid"
                                                    | "user-valid"
                                                    | "read-only"
                                                    | "read-write"
                                                    | "autofill"
                                            ) {
                                                inner_is_simple_negation = true;
                                            }
                                        }
                                    }
                                    // :not(.className) — class negation, always true
                                    // (we can't evaluate, but it's safer to include
                                    // than to drop the whole selector)
                                    Ok(Token::Delim('.')) => {
                                        inner_is_simple_negation = true;
                                    }
                                    // :not(tag) — tag negation
                                    Ok(Token::Ident(_)) => {
                                        inner_is_simple_negation = true;
                                    }
                                    _ => {
                                        p.reset(&state);
                                    }
                                }
                            } else if matches!(
                                fn_lower.as_str(),
                                "nth-child" | "nth-of-type" | "nth-last-child" | "nth-last-of-type"
                            ) {
                                // Parse nth expression
                                nth_selector = parse_nth_inside_block(fn_lower.as_str(), p);
                            } else if matches!(fn_lower.as_str(), "is" | "where" | "matches") {
                                // :is(), :where(), and :matches() - parse inner selectors
                                // These match if any of the inner selectors match
                                if let Some(is_selectors) = parse_is_where_selectors(p) {
                                    if !is_selectors.is_empty() {
                                        if fn_lower == "is" {
                                            is_where_selector = Some(Selector::Is(is_selectors));
                                        } else if fn_lower == "where" {
                                            is_where_selector = Some(Selector::Where(is_selectors));
                                        } else {
                                            // :matches() is legacy name for :is()
                                            is_where_selector =
                                                Some(Selector::Matches(is_selectors));
                                        }
                                    }
                                }
                            } else if fn_lower == "lang" {
                                // :lang(language-code) - parse the language code
                                if let Ok(Token::Ident(ref lang_code)) = p.next() {
                                    is_where_selector = Some(Selector::Lang(lang_code.to_string()));
                                }
                            }
                            // Consume remaining tokens
                            while p.next().is_ok() {}
                            Ok(())
                        });
                        if inner_is_simple_negation {
                            // Treat as always-true: don't skip selector.
                        } else if let Some(sel) = nth_selector {
                            parts.push(sel);
                        } else if let Some(sel) = is_where_selector {
                            // :is() or :where() parsed successfully
                            parts.push(sel);
                        } else if fn_lower == "has" {
                            // :has() - complex descendant matching
                            // For now, treat as always matching but could be enhanced
                        } else {
                            match fn_lower.as_str() {
                                // Language/direction pseudo-classes (not yet implemented)
                                "dir" | "state" => {
                                    skip_selector = true;
                                }
                                // :lang() is now handled above
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            // Handle attribute selectors [attr], [attr=val], [attr~=val], etc.
            Ok(&Token::SquareBracketBlock) => {
                let attr_sel: Result<(String, Option<String>), ParseError<'_, ()>> = parser
                    .parse_nested_block(|p| {
                        let attr_name = match p.next() {
                            Ok(Token::Ident(ref name)) => name.to_string(),
                            _ => {
                                while p.next().is_ok() {}
                                return Ok(("".into(), None));
                            }
                        };
                        // Check for operator + value
                        match p.next() {
                            Ok(Token::Delim('=')) => {
                                // [attr=val]
                                match p.next() {
                                    Ok(Token::Ident(ref v)) => Ok((attr_name, Some(v.to_string()))),
                                    Ok(Token::QuotedString(ref v)) => {
                                        Ok((attr_name, Some(v.to_string())))
                                    }
                                    _ => Ok((attr_name, None)),
                                }
                            }
                            Ok(Token::IncludeMatch) => {
                                // [attr~=val] — treat as presence only for now
                                while p.next().is_ok() {}
                                Ok((attr_name, None))
                            }
                            Err(_) => {
                                // [attr] — presence check
                                Ok((attr_name, None))
                            }
                            _ => {
                                // Other operators (|=, ^=, $=, *=) — skip value, use presence
                                while p.next().is_ok() {}
                                Ok((attr_name, None))
                            }
                        }
                    });
                match attr_sel {
                    Ok((attr, val)) => {
                        parts.push(Selector::Attribute(attr, val));
                    }
                    Err(_) => {
                        skip_selector = true;
                    }
                }
            }
            // Whitespace or non-selector token — stop
            Ok(&Token::WhiteSpace(_)) => {
                // Don't reset — whitespace consumed, caller will check what follows
                break;
            }
            _ => {
                parser.reset(&state);
                break;
            }
        }
    }

    if skip_selector {
        // Selector has :visited/:hover/:focus/:active — return unmatchable selector
        // Use an ID that will never exist in any document
        return Ok(Selector::Id("__pseudo_skip__".to_string()));
    }

    match parts.len() {
        0 => Err(parser
            .new_basic_unexpected_token_error(Token::Ident("".into()))
            .into()),
        1 => Ok(parts.into_iter().next().unwrap()),
        _ => Ok(Selector::Compound(parts)),
    }
}

/// Parse a full selector which may include descendant/child combinators.
/// e.g. `.foo .bar > h2` → Descendant(.foo, Child(.bar, Tag(h2)))
fn parse_one_selector<'i>(parser: &mut Parser<'i, '_>) -> Result<Selector, ParseError<'i, ()>> {
    let mut result = parse_simple_selector(parser)?;

    loop {
        // Peek at next non-whitespace token to decide what to do
        let state = parser.state();
        match parser.next() {
            Ok(&Token::Comma) | Ok(&Token::CurlyBracketBlock) => {
                // End of this selector — put it back for the caller
                parser.reset(&state);
                break;
            }
            Ok(Token::Delim('>')) => {
                // Child combinator
                if let Ok(child) = parse_simple_selector(parser) {
                    result = Selector::Child(Box::new(result), Box::new(child));
                } else {
                    break;
                }
            }
            Ok(Token::Delim('+')) => {
                if let Ok(next) = parse_simple_selector(parser) {
                    result = Selector::AdjacentSibling(Box::new(result), Box::new(next));
                } else {
                    break;
                }
            }
            Ok(Token::Delim('~')) => {
                if let Ok(next) = parse_simple_selector(parser) {
                    result = Selector::GeneralSibling(Box::new(result), Box::new(next));
                } else {
                    break;
                }
            }
            Ok(Token::Ident(_))
            | Ok(Token::Delim('.'))
            | Ok(Token::IDHash(_))
            | Ok(Token::Delim('*'))
            | Ok(&Token::Colon)
            | Ok(&Token::SquareBracketBlock) => {
                // After whitespace (already consumed by parse_simple_selector or next()),
                // another selector token = descendant combinator
                parser.reset(&state);
                if let Ok(descendant) = parse_simple_selector(parser) {
                    result = Selector::Descendant(Box::new(result), Box::new(descendant));
                } else {
                    break;
                }
            }
            _ => {
                parser.reset(&state);
                break;
            }
        }
    }

    Ok(result)
}

/// Parse selectors inside :is() and :where()
fn parse_is_where_selectors<'i>(p: &mut Parser<'i, '_>) -> Option<Vec<Selector>> {
    let mut selectors = Vec::new();

    loop {
        match parse_one_selector(p) {
            Ok(sel) => selectors.push(sel),
            Err(_) => return None,
        }

        // Check for comma or end
        let state = p.state();
        match p.next() {
            Ok(Token::Comma) => {
                // Continue to next selector
                continue;
            }
            Ok(_) => {
                p.reset(&state);
                // Try to continue
            }
            Err(_) => break,
        }
    }

    if selectors.is_empty() {
        None
    } else {
        Some(selectors)
    }
}

/// Parse an :nth-child() or :nth-of-type() expression inside a block.
/// This is called from within parse_nested_block, so we parse directly.
fn parse_nth_inside_block<'i>(fn_name: &str, p: &mut Parser<'i, '_>) -> Option<Selector> {
    // Parse the nth expression
    // Can be: "odd", "even", "2", "2n", "2n+1", "n+2", "-n+3", etc.
    let (a, b) = match p.next() {
        Ok(Token::Ident(ref name)) => {
            let name_lower = name.to_string().to_lowercase();
            if name_lower == "odd" {
                (2, 1) // odd = 2n + 1
            } else if name_lower == "even" {
                (2, 0) // even = 2n + 0
            } else if name_lower == "n" {
                // n (with optional +b or -b)
                let b_val = parse_nth_offset(p).unwrap_or(0);
                (1, b_val)
            } else {
                // Unknown identifier - default to first child
                (0, 1)
            }
        }
        Ok(Token::Number { value, .. }) => {
            // Could be just a number like "3", or could be start of "2n+1"
            let num = *value as i32;

            // Check if followed by 'n'
            let state = p.state();
            if let Ok(Token::Ident(ref id)) = p.next() {
                if id.eq_ignore_ascii_case("n") {
                    // Check for +b or -b
                    let b_val = parse_nth_offset(p).unwrap_or(0);
                    (num, b_val)
                } else {
                    // Not 'n', reset and treat as just a number
                    p.reset(&state);
                    (0, num)
                }
            } else {
                // Just a number - means "nth of type = num"
                (0, num)
            }
        }
        _ => {
            // Default to first child
            (0, 1)
        }
    };

    // Create appropriate selector based on function name
    let sel = match fn_name {
        "nth-child" | "nth-last-child" => Selector::NthChild { a, b },
        "nth-of-type" | "nth-last-of-type" => Selector::NthOfType { a, b },
        _ => Selector::NthChild { a, b },
    };

    Some(sel)
}

/// Parse the +b or -b part of an nth expression
fn parse_nth_offset<'i>(p: &mut Parser<'i, '_>) -> Option<i32> {
    let state = p.state();
    match p.next() {
        Ok(Token::Delim('+')) => {
            // Parse number
            if let Ok(Token::Number { value, .. }) = p.next() {
                Some(*value as i32)
            } else {
                Some(0)
            }
        }
        Ok(Token::Delim('-')) => {
            // Parse number
            if let Ok(Token::Number { value, .. }) = p.next() {
                Some(-(*value as i32))
            } else {
                Some(0)
            }
        }
        _ => {
            p.reset(&state);
            None
        }
    }
}

fn parse_declaration<'i>(parser: &mut Parser<'i, '_>) -> Result<Declaration, ParseError<'i, ()>> {
    let property = match parser.next() {
        Ok(Token::Ident(name)) => name.to_string().to_lowercase(),
        _ => {
            return Err(parser
                .new_basic_unexpected_token_error(Token::Ident("".into()))
                .into());
        }
    };

    parser.expect_colon()?;

    let mut value = parse_value(parser, &property)?;

    // For box model shorthands, collect up to 4 values
    // For box-shadow, collect multiple values (offset-x offset-y blur spread color inset)
    // For outline, collect up to 3 values (width style color)
    if matches!(
        property.as_str(),
        "margin"
            | "padding"
            | "border-width"
            | "border-radius"
            | "border"
            | "border-top"
            | "border-right"
            | "border-bottom"
            | "border-left"
            | "box-shadow"
            | "text-shadow"
            | "outline"
    ) {
        let mut vals = vec![value.clone()];
        let prop_ref = property.clone();
        for _ in 0..3 {
            if let Ok(v) = parser.try_parse(|p| parse_value(p, &prop_ref)) {
                vals.push(v);
            }
        }
        if vals.len() > 1 {
            value = CssValue::List(vals);
        }
    }

    // For grid/grid-template shorthands, collect all values including '/' delimiter
    if matches!(property.as_str(), "grid" | "grid-template") {
        let mut vals = vec![value.clone()];
        for _ in 0..31 {
            // Check for '/' delimiter between rows and columns
            if parser
                .try_parse(|p| -> Result<(), ParseError<'i, ()>> {
                    match p.next() {
                        Ok(Token::Delim('/')) => Ok(()),
                        _ => Err(p.new_basic_unexpected_token_error(Token::Delim('/')).into()),
                    }
                })
                .is_ok()
            {
                vals.push(CssValue::Keyword("/".to_string()));
                continue;
            }
            if let Ok(v) = parser.try_parse(|p| parse_value(p, "grid-template-columns")) {
                if let CssValue::List(inner) = &v {
                    vals.extend(inner.iter().cloned());
                } else {
                    vals.push(v);
                }
            }
        }
        if vals.len() > 1 {
            value = CssValue::List(vals);
        }
    }

    // For grid-column / grid-row shorthands, collect start/end with '/' separator
    // e.g. "13 / span 12", "1 / -1", "2", "span 3"
    if matches!(property.as_str(), "grid-column" | "grid-row" | "grid-area") {
        let mut vals = vec![value.clone()];
        let prop_ref = property.clone();
        for _ in 0..7 {
            if parser
                .try_parse(|p| -> Result<(), ParseError<'i, ()>> {
                    match p.next() {
                        Ok(Token::Delim('/')) => Ok(()),
                        _ => Err(p.new_basic_unexpected_token_error(Token::Delim('/')).into()),
                    }
                })
                .is_ok()
            {
                vals.push(CssValue::Keyword("/".to_string()));
                continue;
            }
            if let Ok(v) = parser.try_parse(|p| parse_value(p, &prop_ref)) {
                vals.push(v);
            } else {
                break;
            }
        }
        if vals.len() > 1 {
            value = CssValue::List(vals);
        }
    }

    // For grid-template-columns/rows, collect many values (grids can have 12+ columns)
    if matches!(
        property.as_str(),
        "grid-template-columns" | "grid-template-rows" | "grid-template-areas"
    ) {
        let mut vals = vec![value.clone()];
        let prop_ref = property.clone();
        for _ in 0..31 {
            if let Ok(v) = parser.try_parse(|p| parse_value(p, &prop_ref)) {
                // Flatten List values from repeat() into the top-level list
                if let CssValue::List(inner) = &v {
                    vals.extend(inner.iter().cloned());
                } else {
                    vals.push(v);
                }
            }
        }
        if vals.len() > 1 {
            value = CssValue::List(vals);
        }
    }

    // For flex shorthand, try to collect all 3 values: grow shrink basis
    if property == "flex" {
        let mut vals = vec![value.clone()];
        for _ in 0..2 {
            if let Ok(v) = parser.try_parse(|p| parse_value(p, "flex")) {
                vals.push(v);
            }
        }
        if vals.len() >= 2 {
            value = CssValue::List(vals);
        }
        // Single value already in `value`
    }

    // For transform, collect multiple transform functions (e.g. "translate(10px) rotate(45deg)")
    if property == "transform" {
        let mut vals = vec![value.clone()];
        // Collect up to 10 transform functions
        for _ in 0..10 {
            if let Ok(v) = parser.try_parse(|p| parse_value(p, "transform")) {
                vals.push(v);
            } else {
                break;
            }
        }
        if vals.len() > 1 {
            value = CssValue::List(vals);
        }
    }

    // Skip any remaining value tokens (e.g. "Verdana, Geneva, sans-serif" for font-family)
    // Stop at semicolon, !important, or end of block
    let important = loop {
        let _state = parser.state();
        match parser.next() {
            Ok(Token::Semicolon) => break false,
            Ok(Token::Delim('!')) => {
                let is_important = parser
                    .try_parse(|p| p.expect_ident_matching("important"))
                    .is_ok();
                let _ = parser.try_parse(|p| p.expect_semicolon());
                break is_important;
            }
            Err(_) => break false, // end of block
            _ => continue,         // skip extra value tokens
        }
    };

    Ok(Declaration {
        property,
        value,
        important,
    })
}

fn parse_value<'i>(
    parser: &mut Parser<'i, '_>,
    property: &str,
) -> Result<CssValue, ParseError<'i, ()>> {
    // Check for color properties
    if is_color_property(property) {
        if let Ok(color) = parser.try_parse(parse_color) {
            return Ok(CssValue::Color(color));
        }
    }

    let state = parser.state();
    match parser.next() {
        Ok(Token::Ident(ref kw)) => {
            let original = kw.to_string();
            let lower = original.to_lowercase();
            match lower.as_str() {
                "auto" => Ok(CssValue::Auto),
                "none" => Ok(CssValue::None),
                "inherit" => Ok(CssValue::Inherit),
                _ => {
                    // Check if it's a named color
                    if is_color_property(property) {
                        if let Some(color) = named_color(&lower) {
                            return Ok(CssValue::Color(color));
                        }
                    }
                    // Preserve original case for user-defined names
                    // (e.g. grid-area names are case-sensitive).
                    // Consumers that need case-insensitive matching should
                    // lowercase at compare-time.
                    Ok(CssValue::Keyword(original))
                }
            }
        }
        Ok(&Token::Dimension {
            value, ref unit, ..
        }) => {
            let u = match unit.as_ref() {
                "px" => LengthUnit::Px,
                "em" => LengthUnit::Em,
                "rem" => LengthUnit::Rem,
                "pt" => LengthUnit::Pt,
                "vw" => LengthUnit::Vw,
                "vh" => LengthUnit::Vh,
                "fr" => LengthUnit::Fr,
                "vmin" => LengthUnit::Vmin,
                "vmax" => LengthUnit::Vmax,
                "ex" => LengthUnit::Ex,
                "ch" => LengthUnit::Ch,
                "cm" => LengthUnit::Cm,
                "mm" => LengthUnit::Mm,
                "in" => LengthUnit::In,
                "pc" => LengthUnit::Pc,
                _ => LengthUnit::Px,
            };
            Ok(CssValue::Length(value, u))
        }
        Ok(&Token::Percentage { unit_value, .. }) => Ok(CssValue::Percentage(unit_value * 100.0)),
        Ok(&Token::Number { value, .. }) => Ok(CssValue::Number(value)),
        Ok(Token::QuotedString(ref s)) => {
            // Used by grid-template-areas: 'areaName' etc.
            Ok(CssValue::Keyword(s.to_string()))
        }
        Ok(Token::Hash(ref h)) | Ok(Token::IDHash(ref h)) => {
            // Color hash
            let hex = h.to_string();
            if let Some(color) = parse_hex_color(&hex) {
                Ok(CssValue::Color(color))
            } else {
                Ok(CssValue::Keyword(format!("#{}", hex)))
            }
        }
        Ok(Token::Function(ref name)) => {
            let fname = name.to_string().to_lowercase();
            match fname.as_str() {
                "rgb" | "rgba" => {
                    let color = parser.parse_nested_block(|p| parse_rgb_function(p))?;
                    Ok(CssValue::Color(color))
                }
                "hsl" | "hsla" => {
                    let color = parser.parse_nested_block(|p| parse_hsl_function(p))?;
                    Ok(CssValue::Color(color))
                }
                "var" => {
                    // CSS variable reference: var(--name) or var(--name, fallback)
                    let result =
                        parser.parse_nested_block(|p| -> Result<CssValue, ParseError<'i, ()>> {
                            let var_name = match p.next() {
                                Ok(Token::Ident(ref name)) => name.to_string(),
                                _ => String::new(),
                            };
                            let fallback = if p.try_parse(|p| p.expect_comma()).is_ok() {
                                // Parse fallback as a value (try color first for color contexts)
                                if let Ok(color) = p.try_parse(parse_color) {
                                    Some(Box::new(CssValue::Color(color)))
                                } else if let Ok(v) = parse_value(p, property) {
                                    Some(Box::new(v))
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            Ok(CssValue::Var(var_name, fallback))
                        })?;
                    Ok(result)
                }
                "repeat" => {
                    // repeat(count | auto-fill | auto-fit, track-size...) -> expand into a List
                    let vals = parser.parse_nested_block(
                        |p| -> Result<Vec<CssValue>, ParseError<'i, ()>> {
                            // Parse the count (integer or auto-fill/auto-fit)
                            let mut auto_fill = false;
                            let count = match p.next() {
                                Ok(&Token::Number {
                                    int_value: Some(n), ..
                                }) => n as usize,
                                Ok(Token::Ident(kw))
                                    if kw.eq_ignore_ascii_case("auto-fill")
                                        || kw.eq_ignore_ascii_case("auto-fit") =>
                                {
                                    auto_fill = true;
                                    1 // placeholder, resolved below
                                }
                                _ => 1,
                            };
                            let _ = p.try_parse(|p| p.expect_comma());
                            // Parse the track values to repeat
                            let mut track_vals = Vec::new();
                            while let Ok(v) = parse_value(p, property) {
                                track_vals.push(v);
                                let _ = p.try_parse(|p| p.expect_comma());
                            }
                            if auto_fill {
                                // Estimate column count from min track size at 1024px viewport
                                let min_px = track_vals
                                    .iter()
                                    .find_map(|v| match v {
                                        CssValue::List(inner) if inner.len() >= 3 => {
                                            // minmax(min, max) — use min
                                            match &inner[1] {
                                                CssValue::Length(px, _) => Some(*px),
                                                _ => None,
                                            }
                                        }
                                        CssValue::Length(px, _) => Some(*px),
                                        _ => None,
                                    })
                                    .unwrap_or(200.0);
                                let cols = ((1024.0 / min_px).floor() as usize).max(1);
                                let mut result = Vec::new();
                                for _ in 0..cols {
                                    result.extend(track_vals.iter().cloned());
                                }
                                Ok(result)
                            } else {
                                let mut result = Vec::new();
                                for _ in 0..count {
                                    result.extend(track_vals.iter().cloned());
                                }
                                Ok(result)
                            }
                        },
                    )?;
                    Ok(CssValue::List(vals))
                }
                "minmax" => {
                    // minmax(min, max) -> store as a keyword "minmax(min,max)"
                    let result =
                        parser.parse_nested_block(|p| -> Result<CssValue, ParseError<'i, ()>> {
                            let min_val = parse_value(p, property)?;
                            let _ = p.try_parse(|p| p.expect_comma());
                            let max_val = parse_value(p, property)?;
                            Ok(CssValue::List(vec![
                                CssValue::Keyword("minmax".to_string()),
                                min_val,
                                max_val,
                            ]))
                        })?;
                    Ok(result)
                }
                "calc" => {
                    // Parse calc() expression
                    let expr = parser.parse_nested_block(parse_calc_expression)?;
                    Ok(CssValue::Calc(expr))
                }
                "min" => {
                    // Parse min() expression
                    let vals = parser.parse_nested_block(parse_calc_values)?;
                    Ok(CssValue::Min(vals))
                }
                "max" => {
                    // Parse max() expression
                    let vals = parser.parse_nested_block(parse_calc_values)?;
                    Ok(CssValue::Max(vals))
                }
                "clamp" => {
                    // Parse clamp() expression
                    let (min, val, max) = parser.parse_nested_block(parse_clamp_expression)?;
                    Ok(CssValue::Clamp { min, val, max })
                }
                "counter" => {
                    // counter(counter-name) or counter(counter-name, style)
                    let result = parser.parse_nested_block(|p| {
                        let counter_name = match p.next() {
                            Ok(Token::Ident(name)) => name.to_string(),
                            _ => String::new(),
                        };
                        Ok(CssValue::Keyword(format!("counter({})", counter_name)))
                    })?;
                    Ok(result)
                }
                "counters" => {
                    // counters(counter-name, separator) or counters(counter-name, separator, style)
                    let result = parser.parse_nested_block(|p| {
                        let counter_name = match p.next() {
                            Ok(Token::Ident(name)) => name.to_string(),
                            _ => String::new(),
                        };
                        let _ = p.try_parse(|p| p.expect_comma());
                        let separator = match p.next() {
                            Ok(Token::QuotedString(s)) => s.to_string(),
                            _ => ".".to_string(),
                        };
                        Ok(CssValue::Keyword(format!(
                            "counters({},{}",
                            counter_name, separator
                        )))
                    })?;
                    Ok(result)
                }
                "linear-gradient" | "radial-gradient" | "repeating-linear-gradient" => {
                    // Preserve the full gradient function for later parsing
                    // Use slice_from to capture raw content including whitespace
                    let args_str = parser.parse_nested_block(|p| -> Result<String, ParseError<'i, ()>> {
                        let start_pos = p.position();
                        // Consume all tokens to reach the end
                        while p.next().is_ok() {}
                        // Get the raw string from start to current position
                        Ok(p.slice_from(start_pos).to_string())
                    })?;
                    Ok(CssValue::Function { name: fname, args: args_str })
                }
                // CSS Transform functions
                "rotate" | "rotateX" | "rotateY" | "rotateZ" |
                "scale" | "scaleX" | "scaleY" | "scaleZ" |
                "translate" | "translateX" | "translateY" | "translateZ" |
                "skew" | "skewX" | "skewY" => {
                    // Parse transform functions like rotate(45deg), translate(10px, 20px), scale(1.5)
                    let args_str = parser.parse_nested_block(|p| -> Result<String, ParseError<'i, ()>> {
                        let start_pos = p.position();
                        // Consume all tokens to reach the end
                        while p.next().is_ok() {}
                        // Get the raw string from start to current position
                        Ok(p.slice_from(start_pos).to_string())
                    })?;
                    Ok(CssValue::Function { name: fname, args: args_str })
                }
                _ => {
                    // Skip unknown function contents
                    parser.parse_nested_block(|p| -> Result<(), ParseError<'i, ()>> {
                        while p.next().is_ok() {}
                        Ok(())
                    })?;
                    Ok(CssValue::Keyword(fname))
                }
            }
        }
        _ => {
            parser.reset(&state);
            Err(parser
                .new_basic_unexpected_token_error(Token::Ident("".into()))
                .into())
        }
    }
}

fn is_color_property(property: &str) -> bool {
    matches!(
        property,
        "color"
            | "background-color"
            | "background"
            | "border-color"
            | "border-top-color"
            | "border-right-color"
            | "border-bottom-color"
            | "border-left-color"
            | "outline-color"
    )
}

fn parse_color<'i>(parser: &mut Parser<'i, '_>) -> Result<CssColor, ParseError<'i, ()>> {
    let state = parser.state();
    match parser.next() {
        Ok(Token::Hash(ref h)) | Ok(Token::IDHash(ref h)) => {
            if let Some(c) = parse_hex_color(h.as_ref()) {
                Ok(c)
            } else {
                parser.reset(&state);
                Err(parser
                    .new_basic_unexpected_token_error(Token::Ident("".into()))
                    .into())
            }
        }
        Ok(Token::Ident(ref name)) => {
            if let Some(c) = named_color(&name.to_string().to_lowercase()) {
                Ok(c)
            } else {
                parser.reset(&state);
                Err(parser
                    .new_basic_unexpected_token_error(Token::Ident("".into()))
                    .into())
            }
        }
        // Note: rgb(from ...) relative colors need special handling -
        // we try regular parsing first, and let rgb_function handle the "from" case
        Ok(Token::Function(ref name))
            if name.eq_ignore_ascii_case("rgb") || name.eq_ignore_ascii_case("rgba") =>
        {
            parser.parse_nested_block(|p| parse_rgb_function(p))
        }
        Ok(Token::Function(ref name))
            if name.eq_ignore_ascii_case("hsl") || name.eq_ignore_ascii_case("hsla") =>
        {
            parser.parse_nested_block(|p| parse_hsl_function(p))
        }
        Ok(Token::Function(ref name)) if name.eq_ignore_ascii_case("lab") => {
            parser.parse_nested_block(|p| parse_lab_function(p))
        }
        Ok(Token::Function(ref name)) if name.eq_ignore_ascii_case("lch") => {
            parser.parse_nested_block(|p| parse_lch_function(p))
        }
        Ok(Token::Function(ref name)) if name.eq_ignore_ascii_case("oklab") => {
            parser.parse_nested_block(|p| parse_oklab_function(p))
        }
        Ok(Token::Function(ref name)) if name.eq_ignore_ascii_case("oklch") => {
            parser.parse_nested_block(|p| parse_oklch_function(p))
        }
        Ok(Token::Function(ref name)) if name.eq_ignore_ascii_case("color-mix") => {
            parser.parse_nested_block(|p| parse_color_mix_function(p))
        }
        _ => {
            parser.reset(&state);
            Err(parser
                .new_basic_unexpected_token_error(Token::Ident("".into()))
                .into())
        }
    }
}

fn parse_rgb_function<'i>(parser: &mut Parser<'i, '_>) -> Result<CssColor, ParseError<'i, ()>> {
    // Check if this is a relative color: rgb(from red r g b / alpha)
    // by peeking at the first token
    let is_relative = {
        let start = parser.state();
        let result = matches!(
            parser.next_including_whitespace_and_comments(),
            Ok(Token::Ident(ref kw)) if kw.eq_ignore_ascii_case("from")
        );
        parser.reset(&start);
        result
    };

    if is_relative {
        // Consume "from" keyword
        let _ = parser.expect_ident()?;
        return parse_rgb_relative_function(parser);
    }

    // Regular rgb() parsing
    let r = parser.expect_number()? as u8;
    let _ = parser.try_parse(|p| p.expect_comma());
    let g = parser.expect_number()? as u8;
    let _ = parser.try_parse(|p| p.expect_comma());
    let b = parser.expect_number()? as u8;
    let a = parser
        .try_parse(|p| {
            let _ = p.try_parse(|p| p.expect_comma());
            p.expect_number()
        })
        .unwrap_or(1.0);
    Ok(CssColor::from_rgba(r, g, b, (a * 255.0) as u8))
}

/// Convert HSL to RGB. H is in [0,360), S and L in [0.0,1.0].
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let s = s.clamp(0.0, 1.0);
    let l = l.clamp(0.0, 1.0);
    // Normalize hue to [0, 360)
    let h = ((h % 360.0) + 360.0) % 360.0;

    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h_prime = h / 60.0;
    let x = c * (1.0 - (h_prime % 2.0 - 1.0).abs());
    let m = l - c / 2.0;

    let (r1, g1, b1) = if h_prime < 1.0 {
        (c, x, 0.0)
    } else if h_prime < 2.0 {
        (x, c, 0.0)
    } else if h_prime < 3.0 {
        (0.0, c, x)
    } else if h_prime < 4.0 {
        (0.0, x, c)
    } else if h_prime < 5.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };

    let r = ((r1 + m) * 255.0).round() as u8;
    let g = ((g1 + m) * 255.0).round() as u8;
    let b = ((b1 + m) * 255.0).round() as u8;
    (r, g, b)
}

/// Parse hsl() / hsla() function arguments.
/// Supports both comma syntax: hsl(120, 100%, 50%) / hsla(120, 100%, 50%, 0.5)
/// and modern space syntax: hsl(120 100% 50%) / hsl(120 100% 50% / 0.5)
/// and CSS Color Level 5 relative colors: hsl(from blue h s l)
fn parse_hsl_function<'i>(parser: &mut Parser<'i, '_>) -> Result<CssColor, ParseError<'i, ()>> {
    // Check if this is a relative color: hsl(from blue h s l)
    let is_relative = {
        let start = parser.state();
        let result = matches!(
            parser.next_including_whitespace_and_comments(),
            Ok(Token::Ident(ref kw)) if kw.eq_ignore_ascii_case("from")
        );
        parser.reset(&start);
        result
    };

    if is_relative {
        // Consume "from" keyword
        let _ = parser.expect_ident()?;
        return parse_hsl_relative_function(parser);
    }

    // Parse hue — could be a plain number (degrees) or a dimension with "deg"
    let h = match parser.next()? {
        Token::Number { value, .. } => *value,
        Token::Dimension { value, .. } => *value,
        _ => return Err(parser.new_custom_error(())),
    };

    // Try comma after hue to detect comma vs space syntax
    let has_commas = parser.try_parse(|p| p.expect_comma()).is_ok();

    // Parse saturation — expect a percentage
    let s = parser.expect_percentage()?.clamp(0.0, 1.0);
    if has_commas {
        let _ = parser.try_parse(|p| p.expect_comma());
    }

    // Parse lightness — expect a percentage
    let l = parser.expect_percentage()?.clamp(0.0, 1.0);

    // Parse optional alpha
    let a = parser
        .try_parse(|p| -> Result<f32, ParseError<'i, ()>> {
            if has_commas {
                // Comma syntax: hsla(h, s, l, a)
                p.expect_comma()?;
            } else {
                // Modern syntax: hsl(h s l / a)
                p.expect_delim('/')?;
            }
            // Alpha can be a number (0.0-1.0) or a percentage
            let alpha = match p.next()? {
                Token::Number { value, .. } => *value,
                Token::Percentage { unit_value, .. } => *unit_value,
                _ => return Err(p.new_custom_error(())),
            };
            Ok(alpha.clamp(0.0, 1.0))
        })
        .unwrap_or(1.0);

    let (r, g, b) = hsl_to_rgb(h, s, l);
    Ok(CssColor::from_rgba(r, g, b, (a * 255.0) as u8))
}

/// Parse hwb() function arguments (CSS Color Level 4).
/// hwb(hue whiteness blackness [/ alpha])
/// Also supports CSS Color Level 5 relative colors: hwb(from green h w b)
#[allow(dead_code)]
fn parse_hwb_function<'i>(parser: &mut Parser<'i, '_>) -> Result<CssColor, ParseError<'i, ()>> {
    // Check if this is a relative color: hwb(from green h w b)
    let is_relative = {
        let start = parser.state();
        let result = matches!(
            parser.next_including_whitespace_and_comments(),
            Ok(Token::Ident(ref kw)) if kw.eq_ignore_ascii_case("from")
        );
        parser.reset(&start);
        result
    };

    if is_relative {
        // Consume "from" keyword
        let _ = parser.expect_ident()?;
        return parse_hwb_relative_function(parser);
    }

    // Parse hue — could be a plain number (degrees) or a dimension with "deg"
    let h = match parser.next()? {
        Token::Number { value, .. } => *value,
        Token::Dimension { value, .. } => *value,
        _ => return Err(parser.new_custom_error(())),
    };

    // Parse whiteness percentage
    let w = parser.expect_percentage()?.clamp(0.0, 1.0);

    // Parse blackness percentage
    let b = parser.expect_percentage()?.clamp(0.0, 1.0);

    // Parse optional alpha
    let a = parser
        .try_parse(|p| -> Result<f32, ParseError<'i, ()>> {
            p.expect_delim('/')?;
            let alpha = match p.next()? {
                Token::Number { value, .. } => *value,
                Token::Percentage { unit_value, .. } => *unit_value,
                _ => return Err(p.new_custom_error(())),
            };
            Ok(alpha.clamp(0.0, 1.0))
        })
        .unwrap_or(1.0);

    let (r, g, b_out) = hwb_to_rgb(h, w, b);
    Ok(CssColor::from_rgba(r, g, b_out, (a * 255.0) as u8))
}

/// Convert HWB to RGB.
/// H is in [0,360), W and B are in [0.0,1.0].
#[allow(dead_code)]
fn hwb_to_rgb(h: f32, w: f32, b: f32) -> (u8, u8, u8) {
    let w = w.clamp(0.0, 1.0);
    let b = b.clamp(0.0, 1.0);

    // If sum of whiteness + blackness >= 1, return gray
    if w + b >= 1.0 {
        let gray = (w * 255.0 / (w + b)).round() as u8;
        return (gray, gray, gray);
    }

    // Convert to HSL then to RGB
    let h = ((h % 360.0) + 360.0) % 360.0;
    let s = 1.0 - (w / (1.0 - b));
    let l = (1.0 - b) * (1.0 + w / (1.0 - b)) / 2.0;

    hsl_to_rgb(h, s, l)
}

/// Parse lab() function arguments (CSS Color Level 4).
/// lab(lightness a b [/ alpha]) - CIELAB color space
fn parse_lab_function<'i>(parser: &mut Parser<'i, '_>) -> Result<CssColor, ParseError<'i, ()>> {
    // Parse lightness - can be percentage or number
    let l = match parser.next()? {
        Token::Number { value, .. } => (*value / 100.0).clamp(0.0, 1.0),
        Token::Percentage { unit_value, .. } => unit_value.clamp(0.0, 1.0),
        _ => return Err(parser.new_custom_error(())),
    };

    // Parse a coordinate (green-red axis)
    let a_coord = match parser.next()? {
        Token::Number { value, .. } => *value,
        _ => return Err(parser.new_custom_error(())),
    };

    // Parse b coordinate (blue-yellow axis)
    let b_coord = match parser.next()? {
        Token::Number { value, .. } => *value,
        _ => return Err(parser.new_custom_error(())),
    };

    // Parse optional alpha
    let alpha = parser
        .try_parse(|p| -> Result<f32, ParseError<'i, ()>> {
            p.expect_delim('/')?;
            let a = match p.next()? {
                Token::Number { value, .. } => *value,
                Token::Percentage { unit_value, .. } => *unit_value,
                _ => return Err(p.new_custom_error(())),
            };
            Ok(a.clamp(0.0, 1.0))
        })
        .unwrap_or(1.0);

    let (r, g, b) = lab_to_rgb(l, a_coord, b_coord);
    Ok(CssColor::from_rgba(r, g, b, (alpha * 255.0) as u8))
}

/// Convert LAB to RGB (simplified approximation).
/// L is in [0,1], a and b can be any value (typically -128 to 128).
fn lab_to_rgb(l: f32, a: f32, b: f32) -> (u8, u8, u8) {
    // Simplified LAB to XYZ to RGB conversion
    // Using D65 illuminant

    // Normalize LAB values
    let l = l.clamp(0.0, 1.0);
    let fy = (l + 0.16) / 1.16;

    // Convert to XYZ (simplified)
    let fx = fy + a / 500.0;
    let fz = fy - b / 200.0;

    // XYZ to RGB matrix (sRGB, D65)
    let x = lab_invf(fx) * 0.95047;
    let y = lab_invf(fy);
    let z = lab_invf(fz) * 1.08883;

    // Convert to linear RGB
    let rl = x * 3.2406 + y * -1.5372 + z * -0.4986;
    let gl = x * -0.9689 + y * 1.8758 + z * 0.0415;
    let bl = x * 0.0557 + y * -0.2040 + z * 1.0570;

    // Gamma correction (sRGB)
    let r = (gamma_correct(rl) * 255.0).round().clamp(0.0, 255.0) as u8;
    let g = (gamma_correct(gl) * 255.0).round().clamp(0.0, 255.0) as u8;
    let b_out = (gamma_correct(bl) * 255.0).round().clamp(0.0, 255.0) as u8;

    (r, g, b_out)
}

/// Inverse of LAB f function
fn lab_invf(t: f32) -> f32 {
    let delta = 6.0 / 29.0;
    if t > delta {
        t.powi(3)
    } else {
        3.0 * delta * delta * (t - 4.0 / 29.0)
    }
}

/// sRGB gamma correction
fn gamma_correct(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Parse lch() function arguments (CSS Color Level 4).
/// lch(lightness chroma hue [/ alpha]) - CIELCH color space
fn parse_lch_function<'i>(parser: &mut Parser<'i, '_>) -> Result<CssColor, ParseError<'i, ()>> {
    // Parse lightness
    let l = match parser.next()? {
        Token::Number { value, .. } => (*value / 100.0).clamp(0.0, 1.0),
        Token::Percentage { unit_value, .. } => unit_value.clamp(0.0, 1.0),
        _ => return Err(parser.new_custom_error(())),
    };

    // Parse chroma (can be number)
    let c = match parser.next()? {
        Token::Number { value, .. } => value.max(0.0),
        _ => return Err(parser.new_custom_error(())),
    };

    // Parse hue (can be number or dimension)
    let h = match parser.next()? {
        Token::Number { value, .. } => *value,
        Token::Dimension { value, .. } => *value,
        _ => return Err(parser.new_custom_error(())),
    };

    // Parse optional alpha
    let alpha = parser
        .try_parse(|p| -> Result<f32, ParseError<'i, ()>> {
            p.expect_delim('/')?;
            let a = match p.next()? {
                Token::Number { value, .. } => *value,
                Token::Percentage { unit_value, .. } => *unit_value,
                _ => return Err(p.new_custom_error(())),
            };
            Ok(a.clamp(0.0, 1.0))
        })
        .unwrap_or(1.0);

    // Convert LCH to LAB then to RGB
    let h_rad = h.to_radians();
    let a = c * h_rad.cos();
    let b = c * h_rad.sin();

    let (r, g, b_out) = lab_to_rgb(l, a, b);
    Ok(CssColor::from_rgba(r, g, b_out, (alpha * 255.0) as u8))
}

/// Parse oklab() function arguments (CSS Color Level 5).
/// oklab(lightness a b [/ alpha]) - OKLAB color space (perceptually uniform)
fn parse_oklab_function<'i>(parser: &mut Parser<'i, '_>) -> Result<CssColor, ParseError<'i, ()>> {
    // Parse lightness
    let l = match parser.next()? {
        Token::Number { value, .. } => (*value / 100.0).clamp(0.0, 1.0),
        Token::Percentage { unit_value, .. } => unit_value.clamp(0.0, 1.0),
        _ => return Err(parser.new_custom_error(())),
    };

    // Parse a coordinate
    let a_coord = match parser.next()? {
        Token::Number { value, .. } => *value,
        _ => return Err(parser.new_custom_error(())),
    };

    // Parse b coordinate
    let b_coord = match parser.next()? {
        Token::Number { value, .. } => *value,
        _ => return Err(parser.new_custom_error(())),
    };

    // Parse optional alpha
    let alpha = parser
        .try_parse(|p| -> Result<f32, ParseError<'i, ()>> {
            p.expect_delim('/')?;
            let a = match p.next()? {
                Token::Number { value, .. } => *value,
                Token::Percentage { unit_value, .. } => *unit_value,
                _ => return Err(p.new_custom_error(())),
            };
            Ok(a.clamp(0.0, 1.0))
        })
        .unwrap_or(1.0);

    let (r, g, b) = oklab_to_rgb(l, a_coord, b_coord);
    Ok(CssColor::from_rgba(r, g, b, (alpha * 255.0) as u8))
}

/// Convert OKLAB to RGB.
fn oklab_to_rgb(l: f32, a: f32, b: f32) -> (u8, u8, u8) {
    let l = l.clamp(0.0, 1.0);

    // OKLAB to linear LMS
    #[allow(clippy::excessive_precision)]
    let l_ = l + 0.3963377774 * a + 0.2158037573 * b;
    #[allow(clippy::excessive_precision)]
    let m_ = l - 0.1055613458 * a - 0.0638541728 * b;
    #[allow(clippy::excessive_precision)]
    let s_ = l - 0.0894841775 * a - 1.2914855480 * b;

    let l_c = l_.powi(3);
    let m_c = m_.powi(3);
    let s_c = s_.powi(3);

    // LMS to linear RGB
    #[allow(clippy::excessive_precision)]
    let rl = 4.0767416621 * l_c - 3.3077115913 * m_c + 0.2309699292 * s_c;
    #[allow(clippy::excessive_precision)]
    let gl = -1.2684380046 * l_c + 2.6097574011 * m_c - 0.3413193965 * s_c;
    #[allow(clippy::excessive_precision)]
    let bl = -0.0041960863 * l_c - 0.7034186147 * m_c + 1.7076147010 * s_c;

    // Gamma correction
    let r = (gamma_correct(rl).clamp(0.0, 1.0) * 255.0).round() as u8;
    let g = (gamma_correct(gl).clamp(0.0, 1.0) * 255.0).round() as u8;
    let b_out = (gamma_correct(bl).clamp(0.0, 1.0) * 255.0).round() as u8;

    (r, g, b_out)
}

/// Parse oklch() function arguments (CSS Color Level 5).
/// oklch(lightness chroma hue [/ alpha]) - OKLCH color space
fn parse_oklch_function<'i>(parser: &mut Parser<'i, '_>) -> Result<CssColor, ParseError<'i, ()>> {
    // Parse lightness
    let l = match parser.next()? {
        Token::Number { value, .. } => (*value / 100.0).clamp(0.0, 1.0),
        Token::Percentage { unit_value, .. } => unit_value.clamp(0.0, 1.0),
        _ => return Err(parser.new_custom_error(())),
    };

    // Parse chroma
    let c = match parser.next()? {
        Token::Number { value, .. } => value.max(0.0),
        _ => return Err(parser.new_custom_error(())),
    };

    // Parse hue
    let h = match parser.next()? {
        Token::Number { value, .. } => *value,
        Token::Dimension { value, .. } => *value,
        _ => return Err(parser.new_custom_error(())),
    };

    // Parse optional alpha
    let alpha = parser
        .try_parse(|p| -> Result<f32, ParseError<'i, ()>> {
            p.expect_delim('/')?;
            let a = match p.next()? {
                Token::Number { value, .. } => *value,
                Token::Percentage { unit_value, .. } => *unit_value,
                _ => return Err(p.new_custom_error(())),
            };
            Ok(a.clamp(0.0, 1.0))
        })
        .unwrap_or(1.0);

    // Convert OKLCH to OKLAB then to RGB
    let h_rad = h.to_radians();
    let a = c * h_rad.cos();
    let b = c * h_rad.sin();

    let (r, g, b_out) = oklab_to_rgb(l, a, b);
    Ok(CssColor::from_rgba(r, g, b_out, (alpha * 255.0) as u8))
}

/// Parse color-mix() function arguments (CSS Color Level 5).
/// color-mix(in color-space, color1 percentage, color2 percentage)
/// Simplified: color-mix(color1 percentage, color2 percentage)
fn parse_color_mix_function<'i>(
    parser: &mut Parser<'i, '_>,
) -> Result<CssColor, ParseError<'i, ()>> {
    // Try to skip optional "in" keyword and color space
    // color-mix(in srgb, red 50%, blue 50%)
    let _: Result<(), ParseError<'_, ()>> = parser.try_parse(|p| {
        let kw = p.expect_ident()?;
        if kw.eq_ignore_ascii_case("in") {
            // Skip color space name
            let _ = p.expect_ident()?;
            p.expect_comma()?;
        }
        Ok(())
    });

    // Parse first color - try to parse as a color value
    let color1 = parser.try_parse(|p| parse_color(p)).ok();

    // Parse first percentage
    let percent1: f32 = parser
        .try_parse(|p| -> Result<f32, ParseError<'_, ()>> {
            let pct = p.expect_percentage()?;
            Ok(pct)
        })
        .unwrap_or(0.5);

    // Expect comma (consume it if present)
    while let Ok(t) = parser.next() {
        if matches!(t, Token::Comma) {
            break;
        }
        if parser.is_exhausted() {
            break;
        }
    }

    // Parse second color
    let color2 = parser.try_parse(|p| parse_color(p)).ok();

    // Parse second percentage (or use remaining percentage)
    let percent2: f32 = parser
        .try_parse(|p| -> Result<f32, ParseError<'_, ()>> {
            let pct = p.expect_percentage()?;
            Ok(pct)
        })
        .unwrap_or(1.0 - percent1);

    // Mix the colors
    match (color1, color2) {
        (Some(c1), Some(c2)) => {
            // Simple linear interpolation in RGB space
            let p1 = percent1 / (percent1 + percent2);
            let p2 = 1.0 - p1;

            let r = ((c1.r as f32) * p1 + (c2.r as f32) * p2).round() as u8;
            let g = ((c1.g as f32) * p1 + (c2.g as f32) * p2).round() as u8;
            let b = ((c1.b as f32) * p1 + (c2.b as f32) * p2).round() as u8;
            let a = ((c1.a as f32) * p1 + (c2.a as f32) * p2).round() as u8;

            Ok(CssColor::from_rgba(r, g, b, a))
        }
        (Some(c), None) | (None, Some(c)) => Ok(c),
        (None, None) => Err(parser.new_custom_error(())),
    }
}

/// Parse CSS Color Level 5 relative color: rgb(from red r g b / alpha)
/// Format: rgb(from <color> r g b [/ alpha])
fn parse_rgb_relative_function<'i>(
    parser: &mut Parser<'i, '_>,
) -> Result<CssColor, ParseError<'i, ()>> {
    // "from" keyword was already consumed by the caller
    // Parse the origin color
    let origin_color = parse_color(parser)?;

    // Parse channel keywords: r, g, b (or numbers)
    // For simplicity, we just use the origin color's values
    // Full implementation would support calc() expressions
    let _r_channel = parser.expect_ident()?;
    let _g_channel = parser.expect_ident()?;
    let _b_channel = parser.expect_ident()?;

    // Parse optional alpha
    let alpha: f32 = parser
        .try_parse(|p| -> Result<f32, ParseError<'i, ()>> {
            p.expect_delim('/')?;
            let a: f32 = match p.next()? {
                Token::Number { value, .. } => *value,
                Token::Percentage { unit_value, .. } => *unit_value,
                _ => 1.0,
            };
            Ok(a.clamp(0.0, 1.0))
        })
        .unwrap_or(1.0);

    // For now, just return the origin color with modified alpha
    // A full implementation would apply the channel transformations
    Ok(CssColor::from_rgba(
        origin_color.r,
        origin_color.g,
        origin_color.b,
        (alpha * 255.0) as u8,
    ))
}

/// Parse CSS Color Level 5 relative color: hsl(from blue h s l / alpha)
fn parse_hsl_relative_function<'i>(
    parser: &mut Parser<'i, '_>,
) -> Result<CssColor, ParseError<'i, ()>> {
    // "from" keyword was already consumed by the caller
    // Parse the origin color

    // Parse the origin color (simplified - just use the color)
    let origin_color = parse_color(parser).ok();

    // Parse channel keywords: h, s, l
    let _h_channel = parser.expect_ident()?;
    let _s_channel = parser.expect_ident()?;
    let _l_channel = parser.expect_ident()?;

    // Parse optional alpha
    let alpha: f32 = parser
        .try_parse(|p| -> Result<f32, ParseError<'i, ()>> {
            p.expect_delim('/')?;
            let a: f32 = match p.next()? {
                Token::Number { value, .. } => *value,
                Token::Percentage { unit_value, .. } => *unit_value,
                _ => 1.0,
            };
            Ok(a.clamp(0.0, 1.0))
        })
        .unwrap_or(1.0);

    // Return origin color with modified alpha, or default red
    match origin_color {
        Some(c) => Ok(CssColor::from_rgba(c.r, c.g, c.b, (alpha * 255.0) as u8)),
        None => Ok(CssColor::from_rgb(255, 0, 0)), // Default to red
    }
}

/// Parse CSS Color Level 5 relative color: hwb(from color h w b / alpha)
#[allow(dead_code)]
fn parse_hwb_relative_function<'i>(
    parser: &mut Parser<'i, '_>,
) -> Result<CssColor, ParseError<'i, ()>> {
    // "from" keyword was already consumed by the caller
    // Parse the origin color

    // Parse the origin color
    let origin_color = parse_color(parser).ok();

    // Parse channel keywords: h, w, b
    let _h_channel = parser.expect_ident()?;
    let _w_channel = parser.expect_ident()?;
    let _b_channel = parser.expect_ident()?;

    // Parse optional alpha
    let alpha: f32 = parser
        .try_parse(|p| -> Result<f32, ParseError<'i, ()>> {
            p.expect_delim('/')?;
            let a: f32 = match p.next()? {
                Token::Number { value, .. } => *value,
                Token::Percentage { unit_value, .. } => *unit_value,
                _ => 1.0,
            };
            Ok(a.clamp(0.0, 1.0))
        })
        .unwrap_or(1.0);

    match origin_color {
        Some(c) => Ok(CssColor::from_rgba(c.r, c.g, c.b, (alpha * 255.0) as u8)),
        None => Ok(CssColor::from_rgb(255, 0, 0)),
    }
}

fn parse_hex_color(hex: &str) -> Option<CssColor> {
    let hex = hex.trim_start_matches('#');
    match hex.len() {
        3 => {
            let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
            Some(CssColor::from_rgb(r, g, b))
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some(CssColor::from_rgb(r, g, b))
        }
        8 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
            Some(CssColor::from_rgba(r, g, b, a))
        }
        _ => None,
    }
}

fn named_color(name: &str) -> Option<CssColor> {
    Some(match name {
        "black" => CssColor::from_rgb(0, 0, 0),
        "white" => CssColor::from_rgb(255, 255, 255),
        "red" => CssColor::from_rgb(255, 0, 0),
        "green" => CssColor::from_rgb(0, 128, 0),
        "blue" => CssColor::from_rgb(0, 0, 255),
        "yellow" => CssColor::from_rgb(255, 255, 0),
        "cyan" | "aqua" => CssColor::from_rgb(0, 255, 255),
        "magenta" | "fuchsia" => CssColor::from_rgb(255, 0, 255),
        "gray" | "grey" => CssColor::from_rgb(128, 128, 128),
        "darkgray" | "darkgrey" => CssColor::from_rgb(169, 169, 169),
        "lightgray" | "lightgrey" => CssColor::from_rgb(211, 211, 211),
        "orange" => CssColor::from_rgb(255, 165, 0),
        "purple" => CssColor::from_rgb(128, 0, 128),
        "brown" => CssColor::from_rgb(165, 42, 42),
        "pink" => CssColor::from_rgb(255, 192, 203),
        "navy" => CssColor::from_rgb(0, 0, 128),
        "teal" => CssColor::from_rgb(0, 128, 128),
        "olive" => CssColor::from_rgb(128, 128, 0),
        "maroon" => CssColor::from_rgb(128, 0, 0),
        "silver" => CssColor::from_rgb(192, 192, 192),
        "lime" => CssColor::from_rgb(0, 255, 0),
        "coral" => CssColor::from_rgb(255, 127, 80),
        "tomato" => CssColor::from_rgb(255, 99, 71),
        "steelblue" => CssColor::from_rgb(70, 130, 180),
        "dodgerblue" => CssColor::from_rgb(30, 144, 255),
        "darkblue" => CssColor::from_rgb(0, 0, 139),
        "darkgreen" => CssColor::from_rgb(0, 100, 0),
        "darkred" => CssColor::from_rgb(139, 0, 0),
        "indianred" => CssColor::from_rgb(205, 92, 92),
        "lightblue" => CssColor::from_rgb(173, 216, 230),
        "lightgreen" => CssColor::from_rgb(144, 238, 144),
        "lightyellow" => CssColor::from_rgb(255, 255, 224),
        "lightcoral" => CssColor::from_rgb(240, 128, 128),
        "lightsalmon" => CssColor::from_rgb(255, 160, 122),
        "lightseagreen" => CssColor::from_rgb(32, 178, 170),
        "lightskyblue" => CssColor::from_rgb(135, 206, 250),
        "lightsteelblue" => CssColor::from_rgb(176, 196, 222),
        "limegreen" => CssColor::from_rgb(50, 205, 50),
        "royalblue" => CssColor::from_rgb(65, 105, 225),
        "midnightblue" => CssColor::from_rgb(25, 25, 112),
        "slategray" | "slategrey" => CssColor::from_rgb(112, 128, 144),
        "dimgray" | "dimgrey" => CssColor::from_rgb(105, 105, 105),
        "gainsboro" => CssColor::from_rgb(220, 220, 220),
        "whitesmoke" => CssColor::from_rgb(245, 245, 245),
        "ivory" => CssColor::from_rgb(255, 255, 240),
        "beige" => CssColor::from_rgb(245, 245, 220),
        "wheat" => CssColor::from_rgb(245, 222, 179),
        "gold" => CssColor::from_rgb(255, 215, 0),
        "goldenrod" => CssColor::from_rgb(218, 165, 32),
        "chocolate" => CssColor::from_rgb(210, 105, 30),
        "firebrick" => CssColor::from_rgb(178, 34, 34),
        "crimson" => CssColor::from_rgb(220, 20, 60),
        "orangered" => CssColor::from_rgb(255, 69, 0),
        "darkorange" => CssColor::from_rgb(255, 140, 0),
        "deeppink" => CssColor::from_rgb(255, 20, 147),
        "hotpink" => CssColor::from_rgb(255, 105, 180),
        "violet" => CssColor::from_rgb(238, 130, 238),
        "plum" => CssColor::from_rgb(221, 160, 221),
        "orchid" => CssColor::from_rgb(218, 112, 214),
        "mediumpurple" => CssColor::from_rgb(147, 112, 219),
        "indigo" => CssColor::from_rgb(75, 0, 130),
        "darkviolet" => CssColor::from_rgb(148, 0, 211),
        "darkcyan" => CssColor::from_rgb(0, 139, 139),
        "cadetblue" => CssColor::from_rgb(95, 158, 160),
        "cornflowerblue" => CssColor::from_rgb(100, 149, 237),
        "mediumseagreen" => CssColor::from_rgb(60, 179, 113),
        "seagreen" => CssColor::from_rgb(46, 139, 87),
        "forestgreen" => CssColor::from_rgb(34, 139, 34),
        "olivedrab" => CssColor::from_rgb(107, 142, 35),
        "sienna" => CssColor::from_rgb(160, 82, 45),
        "tan" => CssColor::from_rgb(210, 180, 140),
        "peru" => CssColor::from_rgb(205, 133, 63),
        "linen" => CssColor::from_rgb(250, 240, 230),
        "lavender" => CssColor::from_rgb(230, 230, 250),
        "aliceblue" => CssColor::from_rgb(240, 248, 255),
        "ghostwhite" => CssColor::from_rgb(248, 248, 255),
        "mintcream" => CssColor::from_rgb(245, 255, 250),
        "honeydew" => CssColor::from_rgb(240, 255, 240),
        "azure" => CssColor::from_rgb(240, 255, 255),
        "snow" => CssColor::from_rgb(255, 250, 250),
        "seashell" => CssColor::from_rgb(255, 245, 238),
        "mistyrose" => CssColor::from_rgb(255, 228, 225),
        "antiquewhite" => CssColor::from_rgb(250, 235, 215),
        "papayawhip" => CssColor::from_rgb(255, 239, 213),
        "blanchedalmond" => CssColor::from_rgb(255, 235, 205),
        "bisque" => CssColor::from_rgb(255, 228, 196),
        "moccasin" => CssColor::from_rgb(255, 228, 181),
        "navajowhite" => CssColor::from_rgb(255, 222, 173),
        "peachpuff" => CssColor::from_rgb(255, 218, 185),
        "cornsilk" => CssColor::from_rgb(255, 248, 220),
        "lemonchiffon" => CssColor::from_rgb(255, 250, 205),
        "floralwhite" => CssColor::from_rgb(255, 250, 240),
        "oldlace" => CssColor::from_rgb(253, 245, 230),
        "khaki" => CssColor::from_rgb(240, 230, 140),
        "darkkhaki" => CssColor::from_rgb(189, 183, 107),
        "salmon" => CssColor::from_rgb(250, 128, 114),
        "darksalmon" => CssColor::from_rgb(233, 150, 122),
        "rosybrown" => CssColor::from_rgb(188, 143, 143),
        "sandybrown" => CssColor::from_rgb(244, 164, 96),
        "darkgoldenrod" => CssColor::from_rgb(184, 134, 11),
        "mediumaquamarine" => CssColor::from_rgb(102, 205, 170),
        "aquamarine" => CssColor::from_rgb(127, 255, 212),
        "turquoise" => CssColor::from_rgb(64, 224, 208),
        "mediumturquoise" => CssColor::from_rgb(72, 209, 204),
        "darkturquoise" => CssColor::from_rgb(0, 206, 209),
        "powderblue" => CssColor::from_rgb(176, 224, 230),
        "skyblue" => CssColor::from_rgb(135, 206, 235),
        "deepskyblue" => CssColor::from_rgb(0, 191, 255),
        "mediumblue" => CssColor::from_rgb(0, 0, 205),
        "mediumslateblue" => CssColor::from_rgb(123, 104, 238),
        "slateblue" => CssColor::from_rgb(106, 90, 205),
        "darkslateblue" => CssColor::from_rgb(72, 61, 139),
        "darkslategray" | "darkslategrey" => CssColor::from_rgb(47, 79, 79),
        "transparent" => CssColor::TRANSPARENT,
        _ => return None,
    })
}

/// Parse inline style attribute value.
pub fn parse_inline_style(input: &str) -> Vec<Declaration> {
    let mut declarations = Vec::new();
    let mut pi = ParserInput::new(input);
    let mut parser = Parser::new(&mut pi);

    loop {
        if parser.is_exhausted() {
            break;
        }
        if let Ok(decl) = parse_declaration(&mut parser) {
            declarations.push(decl);
        } else {
            // Skip to next semicolon
            while let Ok(token) = parser.next() {
                if matches!(token, Token::Semicolon) {
                    break;
                }
            }
        }
    }

    declarations
}

/// Matched rule with specificity for cascade ordering.
#[derive(Debug)]
pub struct MatchedRule<'a> {
    pub specificity: (u32, u32, u32),
    pub rule: &'a Rule,
}

/// Find all rules in a stylesheet that match a given element.
pub fn matching_rules<'a>(
    stylesheet: &'a Stylesheet,
    element: &ElementData,
    doc: &Document,
    node_id: NodeId,
) -> Vec<MatchedRule<'a>> {
    let mut matched = Vec::new();
    for rule in &stylesheet.rules {
        for selector in &rule.selectors {
            if selector.matches(element, doc, node_id) {
                matched.push(MatchedRule {
                    specificity: selector.specificity(),
                    rule,
                });
                break; // Only need one matching selector per rule
            }
        }
    }
    matched
}

// CSS Math Functions parsing
fn parse_calc_value<'i>(parser: &mut Parser<'i, '_>) -> Result<CalcValue, ParseError<'i, ()>> {
    let state = parser.state();
    match parser.next() {
        Ok(&Token::Dimension {
            value, ref unit, ..
        }) => {
            let unit_str: &str = unit.as_ref();
            match unit_str {
                "px" => Ok(CalcValue::Px(value)),
                "em" => Ok(CalcValue::Em(value)),
                "rem" => Ok(CalcValue::Rem(value)),
                "vw" => Ok(CalcValue::Vw(value)),
                "vh" => Ok(CalcValue::Vh(value)),
                _ => {
                    parser.reset(&state);
                    Err(parser.new_custom_error(()))
                }
            }
        }
        Ok(&Token::Percentage { unit_value, .. }) => Ok(CalcValue::Percent(unit_value * 100.0)),
        Ok(&Token::Number { value, .. }) => {
            // Unitless zero is valid in calc()
            Ok(CalcValue::Px(value))
        }
        _ => {
            parser.reset(&state);
            Err(parser.new_custom_error(()))
        }
    }
}

fn parse_calc_values<'i>(
    parser: &mut Parser<'i, '_>,
) -> Result<Vec<CalcValue>, ParseError<'i, ()>> {
    let mut values = Vec::new();
    while let Ok(val) = parse_calc_value(parser) {
        values.push(val);
        // Try to consume comma separator
        let _ = parser.try_parse(|p| p.expect_comma());
    }
    Ok(values)
}

fn parse_clamp_expression<'i>(
    parser: &mut Parser<'i, '_>,
) -> Result<(CalcValue, CalcValue, CalcValue), ParseError<'i, ()>> {
    let min = parse_calc_value(parser)?;
    parser.expect_comma()?;
    let val = parse_calc_value(parser)?;
    parser.expect_comma()?;
    let max = parse_calc_value(parser)?;
    Ok((min, val, max))
}

fn parse_calc_expression<'i>(
    parser: &mut Parser<'i, '_>,
) -> Result<CalcExpression, ParseError<'i, ()>> {
    // Simplified calc parsing: supports basic expressions like:
    // calc(100% - 20px), calc(50vw + 10px), calc(100% / 2)
    // For simplicity, we parse left-to-right without proper operator precedence
    let mut expr: Option<CalcExpression> = None;

    while !parser.is_exhausted() {
        let state = parser.state();

        // Try to parse a value or sub-expression
        if let Ok(val) = parse_calc_value(parser) {
            let new_expr = if let Some(ref e) = expr {
                // If we already have an expression, this might be an error or continuation
                // For simplicity, replace with addition
                CalcExpression::Add(Box::new(e.clone()), Box::new(CalcExpression::Value(val)))
            } else {
                CalcExpression::Value(val)
            };
            expr = Some(new_expr);
        } else if let Ok(&Token::Percentage { unit_value, .. }) = parser.next() {
            let new_expr = if let Some(ref e) = expr {
                CalcExpression::Add(
                    Box::new(e.clone()),
                    Box::new(CalcExpression::Percentage(unit_value * 100.0)),
                )
            } else {
                CalcExpression::Percentage(unit_value * 100.0)
            };
            expr = Some(new_expr);
        } else {
            parser.reset(&state);

            // Check for operators
            match parser.next() {
                Ok(Token::Delim('+')) => {
                    // Addition - already handled by value concatenation above
                }
                Ok(Token::Delim('-')) => {
                    // Subtraction - negate next value
                    if let Ok(val) = parse_calc_value(parser) {
                        let neg_val = match val {
                            CalcValue::Px(v) => CalcValue::Px(-v),
                            CalcValue::Percent(v) => CalcValue::Percent(-v),
                            CalcValue::Em(v) => CalcValue::Em(-v),
                            CalcValue::Rem(v) => CalcValue::Rem(-v),
                            CalcValue::Vw(v) => CalcValue::Vw(-v),
                            CalcValue::Vh(v) => CalcValue::Vh(-v),
                        };
                        if let Some(ref e) = expr {
                            expr = Some(CalcExpression::Add(
                                Box::new(e.clone()),
                                Box::new(CalcExpression::Value(neg_val)),
                            ));
                        } else {
                            expr = Some(CalcExpression::Value(neg_val));
                        }
                    }
                }
                Ok(Token::Delim('*')) => {
                    // Multiplication - multiply by number
                    if let Ok(&Token::Number { value, .. }) = parser.next() {
                        if let Some(ref e) = expr {
                            expr = Some(CalcExpression::Multiply(Box::new(e.clone()), value));
                        }
                    }
                }
                Ok(Token::Delim('/')) => {
                    // Division - divide by number
                    if let Ok(&Token::Number { value, .. }) = parser.next() {
                        if let Some(ref e) = expr {
                            expr = Some(CalcExpression::Divide(Box::new(e.clone()), value));
                        }
                    }
                }
                _ => {
                    // Ignore unknown tokens
                    break;
                }
            }
        }
    }

    expr.ok_or_else(|| parser.new_custom_error(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_css() {
        let css = "p { color: red; font-size: 16px; }";
        let stylesheet = parse_css(css);
        assert!(!stylesheet.rules.is_empty());
        let rule = &stylesheet.rules[0];
        assert!(matches!(&rule.selectors[0], Selector::Tag(t) if t == "p"));
        assert!(rule.declarations.len() >= 1);
    }

    #[test]
    fn test_parse_hex_colors() {
        assert_eq!(
            parse_hex_color("ff0000"),
            Some(CssColor::from_rgb(255, 0, 0))
        );
        assert_eq!(parse_hex_color("f00"), Some(CssColor::from_rgb(255, 0, 0)));
    }

    #[test]
    fn test_selector_specificity() {
        assert_eq!(Selector::Tag("p".into()).specificity(), (0, 0, 1));
        assert_eq!(Selector::Class("foo".into()).specificity(), (0, 1, 0));
        assert_eq!(Selector::Id("bar".into()).specificity(), (1, 0, 0));
    }

    #[test]
    fn test_selector_matching() {
        let mut el = ElementData::new("div");
        el.attributes
            .insert("class".to_string(), "container".to_string());
        el.attributes.insert("id".to_string(), "main".to_string());

        assert!(Selector::Tag("div".into()).matches_element(&el));
        assert!(Selector::Class("container".into()).matches_element(&el));
        assert!(Selector::Id("main".into()).matches_element(&el));
        assert!(!Selector::Tag("p".into()).matches_element(&el));
    }

    #[test]
    fn test_descendant_selector_parsing() {
        let css = ".foo .bar { color: red; } .a > .b { color: blue; }";
        let stylesheet = parse_css(css);
        assert_eq!(stylesheet.rules.len(), 2);
        // .foo .bar should be a Descendant selector
        assert!(matches!(
            &stylesheet.rules[0].selectors[0],
            Selector::Descendant(..)
        ));
        // .a > .b should be a Child selector
        assert!(matches!(
            &stylesheet.rules[1].selectors[0],
            Selector::Child(..)
        ));
    }

    #[test]
    fn test_descendant_selector_matching() {
        use incognidium_dom::{Document, NodeData, TextData};
        // Build: <div class="outer"><p class="inner">text</p></div>
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let mut outer = ElementData::new("div");
        outer
            .attributes
            .insert("class".to_string(), "outer".to_string());
        let outer_id = doc.add_node(html, NodeData::Element(outer));
        let mut inner = ElementData::new("p");
        inner
            .attributes
            .insert("class".to_string(), "inner".to_string());
        let inner_id = doc.add_node(outer_id, NodeData::Element(inner));

        let sel = Selector::Descendant(
            Box::new(Selector::Class("outer".into())),
            Box::new(Selector::Class("inner".into())),
        );
        let inner_el = if let NodeData::Element(ref e) = doc.node(inner_id).data {
            e
        } else {
            panic!()
        };
        assert!(sel.matches(inner_el, &doc, inner_id));
        // outer should NOT match (it's the ancestor, not the descendant)
        let outer_el = if let NodeData::Element(ref e) = doc.node(outer_id).data {
            e
        } else {
            panic!()
        };
        assert!(!sel.matches(outer_el, &doc, outer_id));
    }

    #[test]
    fn test_inline_style_multivalue_padding() {
        // Wikipedia uses: padding:0 0.9em 0 0; width:300px;
        let decls = parse_inline_style("padding:0 0.9em 0 0; width:300px;");
        eprintln!("Parsed inline decls: {:?}", decls);
        let has_width = decls.iter().any(|d| d.property == "width");
        assert!(
            has_width,
            "width:300px not found after multi-value padding. Got: {:?}",
            decls
        );
    }

    #[test]
    fn test_inline_style() {
        let decls = parse_inline_style("color: blue; font-size: 20px");
        assert!(decls.len() >= 1);
    }

    #[test]
    fn test_parse_at_rules_and_pseudo_selectors() {
        // Should not panic on real-world CSS with at-rules, pseudo-classes, etc.
        let css = r#"
            @media screen and (max-width: 600px) {
                .mobile { display: none; }
            }
            @import url("foo.css");
            @charset "UTF-8";
            a:hover { color: red; }
            p::before { content: "x"; }
            input[type="text"] { border-width: 1px; }
            .foo > .bar { color: blue; }
            .a + .b ~ .c { color: green; }
            :root { color: black; }
            div:nth-child(2n+1) { color: orange; }
            p { color: red; font-size: 14px; }
        "#;
        let stylesheet = parse_css(css);
        // Should have parsed at least the `p { ... }` rule without crashing
        assert!(stylesheet.rules.iter().any(|r| r
            .selectors
            .iter()
            .any(|s| matches!(s, Selector::Tag(t) if t == "p"))));
    }

    #[test]
    fn test_font_family_doesnt_break_subsequent_decls() {
        let css = "td { font-family:Verdana, Geneva, sans-serif; font-size:10pt; color:#828282; }";
        let stylesheet = parse_css(css);
        assert_eq!(stylesheet.rules.len(), 1);
        let rule = &stylesheet.rules[0];
        eprintln!("Declarations: {:?}", rule.declarations);
        // Should have 3 declarations: font-family, font-size, color
        assert!(
            rule.declarations.len() >= 3,
            "Expected >= 3 declarations, got {}: {:?}",
            rule.declarations.len(),
            rule.declarations
        );
        // font-size should be 10pt
        let fs = rule
            .declarations
            .iter()
            .find(|d| d.property == "font-size")
            .expect("font-size missing");
        assert!(
            matches!(fs.value, CssValue::Length(10.0, LengthUnit::Pt)),
            "font-size value: {:?}",
            fs.value
        );
        // color should be #828282
        let col = rule
            .declarations
            .iter()
            .find(|d| d.property == "color")
            .expect("color missing");
        assert!(
            matches!(
                col.value,
                CssValue::Color(CssColor {
                    r: 0x82,
                    g: 0x82,
                    b: 0x82,
                    a: 0xff
                })
            ),
            "color value: {:?}",
            col.value
        );
    }

    #[test]
    fn test_media_query_min_width() {
        let css = "@media(min-width:875px){.test-class #mp-upper{display:flex}}";
        let stylesheet = parse_css(css);
        eprintln!("Rules: {}", stylesheet.rules.len());
        for rule in &stylesheet.rules {
            eprintln!("  sel: {:?}", rule.selectors);
            eprintln!("  decls: {:?}", rule.declarations);
        }
        assert!(
            stylesheet.rules.len() >= 1,
            "Should parse rule inside @media"
        );
        assert!(
            stylesheet.rules[0]
                .declarations
                .iter()
                .any(|d| d.property == "display"),
            "Should have display declaration"
        );
    }

    #[test]
    fn test_calc_expression() {
        let css = ".box { width: calc(100% - 20px); }";
        let stylesheet = parse_css(css);
        assert_eq!(stylesheet.rules.len(), 1);
        let rule = &stylesheet.rules[0];
        let width = rule
            .declarations
            .iter()
            .find(|d| d.property == "width")
            .expect("width declaration");
        assert!(
            matches!(width.value, CssValue::Calc(_)),
            "Should parse calc() expression, got {:?}",
            width.value
        );
    }

    #[test]
    fn test_min_max_clamp() {
        let css = ".responsive { width: min(100%, 500px); height: max(50vh, 300px); font-size: clamp(16px, 2vw, 24px); }";
        let stylesheet = parse_css(css);
        assert_eq!(stylesheet.rules.len(), 1);
        let rule = &stylesheet.rules[0];

        let width = rule
            .declarations
            .iter()
            .find(|d| d.property == "width")
            .expect("width declaration");
        assert!(
            matches!(width.value, CssValue::Min(_)),
            "Should parse min() expression, got {:?}",
            width.value
        );

        let height = rule
            .declarations
            .iter()
            .find(|d| d.property == "height")
            .expect("height declaration");
        assert!(
            matches!(height.value, CssValue::Max(_)),
            "Should parse max() expression, got {:?}",
            height.value
        );

        let font_size = rule
            .declarations
            .iter()
            .find(|d| d.property == "font-size")
            .expect("font-size declaration");
        assert!(
            matches!(font_size.value, CssValue::Clamp { .. }),
            "Should parse clamp() expression, got {:?}",
            font_size.value
        );
    }

    #[test]
    fn test_length_units() {
        // Test parsing of various CSS length units
        let css = r#"
            .units {
                width: 10vmin;
                height: 20vmax;
                font-size: 2ex;
                line-height: 40ch;
                margin: 1cm;
                padding: 5mm;
                border-width: 0.5in;
                letter-spacing: 2pc;
            }
        "#;
        let stylesheet = parse_css(css);
        assert_eq!(stylesheet.rules.len(), 1);
        let rule = &stylesheet.rules[0];

        // Check all units were parsed
        let decls = &rule.declarations;
        assert!(decls.iter().any(|d| d.property == "width"));
        assert!(decls.iter().any(|d| d.property == "height"));
        assert!(decls.iter().any(|d| d.property == "font-size"));
        assert!(decls.iter().any(|d| d.property == "line-height"));
        assert!(decls.iter().any(|d| d.property == "margin"));
        assert!(decls.iter().any(|d| d.property == "padding"));
        assert!(decls.iter().any(|d| d.property == "border-width"));
        assert!(decls.iter().any(|d| d.property == "letter-spacing"));
    }

    #[test]
    fn test_pseudo_elements() {
        // Test ::before and ::after pseudo-elements
        let css = r#"
            .quote::before { content: '"'; }
            .quote::after { content: '"'; }
            button::before { margin-right: 5px; }
        "#;
        let stylesheet = parse_css(css);

        // All three rules should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            3,
            "Should parse 3 rules with pseudo-elements"
        );

        // Check the declarations
        assert!(
            stylesheet
                .rules
                .iter()
                .any(|r| { r.declarations.iter().any(|d| d.property == "content") }),
            "Should have content declarations"
        );

        assert!(
            stylesheet
                .rules
                .iter()
                .any(|r| { r.declarations.iter().any(|d| d.property == "margin-right") }),
            "Should have margin-right declaration"
        );
    }

    #[test]
    fn test_keyframes_parsing() {
        let css = r#"
            @keyframes fadeIn {
                from { opacity: 0; }
                to { opacity: 1; }
            }
            @keyframes slide {
                0% { transform: translateX(0); }
                50% { transform: translateX(50px); }
                100% { transform: translateX(0); }
            }
        "#;
        let stylesheet = parse_css(css);

        // Check keyframes were parsed
        assert!(
            stylesheet.keyframes.contains_key("fadeIn"),
            "Should have fadeIn keyframes"
        );
        assert!(
            stylesheet.keyframes.contains_key("slide"),
            "Should have slide keyframes"
        );

        // Check fadeIn keyframes
        let fade_in = &stylesheet.keyframes["fadeIn"];
        assert_eq!(fade_in.name, "fadeIn");
        assert_eq!(fade_in.frames.len(), 2);
        assert_eq!(fade_in.frames[0].selectors, vec![0.0]);
        assert_eq!(fade_in.frames[1].selectors, vec![100.0]);

        // Check slide keyframes
        let slide = &stylesheet.keyframes["slide"];
        assert_eq!(slide.name, "slide");
        assert_eq!(slide.frames.len(), 3);
        assert_eq!(slide.frames[0].selectors, vec![0.0]);
        assert_eq!(slide.frames[1].selectors, vec![50.0]);
        assert_eq!(slide.frames[2].selectors, vec![100.0]);
    }

    #[test]
    fn test_import_parsing() {
        let css = r#"
            @import "main.css";
            @import url("fonts.css");
            @import "responsive.css" screen;
            .test { color: red; }
        "#;
        let stylesheet = parse_css(css);

        // Check imports were parsed
        assert_eq!(stylesheet.imports.len(), 3, "Should have 3 import rules");

        // Check first import
        assert_eq!(stylesheet.imports[0].url, "main.css");
        assert_eq!(stylesheet.imports[0].media, None);

        // Check second import
        assert_eq!(stylesheet.imports[1].url, "fonts.css");
        assert_eq!(stylesheet.imports[1].media, None);

        // Check third import with media query
        assert_eq!(stylesheet.imports[2].url, "responsive.css");
        assert_eq!(stylesheet.imports[2].media, Some("screen".to_string()));

        // Regular rule should still be parsed
        assert_eq!(
            stylesheet.rules.len(),
            1,
            "Should still parse regular rules"
        );
    }

    #[test]
    fn test_font_face_parsing() {
        let css = r#"
            @font-face {
                font-family: "MyFont";
                src: url("font.woff2") format("woff2");
                font-weight: 400;
                font-style: normal;
            }
            @font-face {
                font-family: 'BoldFont';
                src: url("bold.ttf");
                font-weight: bold;
            }
        "#;
        let stylesheet = parse_css(css);

        // Check font faces were parsed
        assert_eq!(
            stylesheet.font_faces.len(),
            2,
            "Should have 2 font-face rules"
        );

        // Check first font face
        let ff1 = &stylesheet.font_faces[0];
        assert_eq!(ff1.font_family, Some("MyFont".to_string()));
        assert_eq!(ff1.src, Some("font.woff2".to_string()));
        assert_eq!(ff1.format, Some("woff2".to_string()));
        assert_eq!(ff1.font_weight, Some("400".to_string()));
        assert_eq!(ff1.font_style, Some("normal".to_string()));

        // Check second font face
        let ff2 = &stylesheet.font_faces[1];
        assert_eq!(ff2.font_family, Some("BoldFont".to_string()));
        assert_eq!(ff2.src, Some("bold.ttf".to_string()));
        assert_eq!(ff2.font_weight, Some("bold".to_string()));
    }

    #[test]
    fn test_is_has_selectors() {
        // Test :has() selector - when combined with a regular selector,
        // the :has() part is treated as always matching
        let css = r#"
            .card:has(.image) { border: 1px solid; }
            article:has(> img) { display: flex; }
        "#;
        let stylesheet = parse_css(css);

        // Both rules with :has() should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            2,
            "Should parse 2 rules with :has()"
        );

        // Check the declarations
        let has_border = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "border"));
        assert!(has_border, "Should have border from .card:has() rule");

        let has_display = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "display"));
        assert!(has_display, "Should have display from article:has() rule");
    }

    #[test]
    fn test_is_where_selectors() {
        // Test :is() and :where() CSS Level 4 selectors
        let css = r#"
            :is(h1, h2, h3) { color: blue; }
            :where(.btn, .button) { padding: 10px; }
            :is(.sidebar, .aside) :is(h1, h2) { margin: 0; }
        "#;
        let stylesheet = parse_css(css);

        // All three rules should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            3,
            "Should parse 3 rules with :is() and :where()"
        );

        // Check the declarations
        let has_color = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "color"));
        assert!(has_color, "Should have color from :is(h1, h2, h3)");

        let has_padding = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "padding"));
        assert!(
            has_padding,
            "Should have padding from :where(.btn, .button)"
        );

        let has_margin = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "margin"));
        assert!(has_margin, "Should have margin from nested :is()");
    }

    #[test]
    fn test_lang_selector() {
        // Test :lang() pseudo-class for language-based styling
        let css = r#"
            :lang(en) { font-family: serif; }
            :lang(zh) { font-family: sans-serif; }
            p:lang(fr) { color: blue; }
        "#;
        let stylesheet = parse_css(css);

        // All three rules should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            3,
            "Should parse 3 rules with :lang()"
        );

        // Check the declarations
        let has_font_family = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "font-family"));
        assert!(
            has_font_family,
            "Should have font-family from :lang() rules"
        );

        let has_color = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "color"));
        assert!(has_color, "Should have color from p:lang(fr)");
    }

    #[test]
    fn test_any_link_selector() {
        // Test :any-link pseudo-class for modern link styling
        let css = r#"
            :any-link { text-decoration: underline; }
            nav :any-link { color: blue; }
            footer :any-link { color: gray; }
        "#;
        let stylesheet = parse_css(css);

        // All three rules should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            3,
            "Should parse 3 rules with :any-link()"
        );

        // Check the declarations
        let has_text_decoration = stylesheet.rules.iter().any(|r| {
            r.declarations
                .iter()
                .any(|d| d.property == "text-decoration")
        });
        assert!(
            has_text_decoration,
            "Should have text-decoration from :any-link"
        );

        let has_color = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "color"));
        assert!(has_color, "Should have color from :any-link rules");
    }

    #[test]
    fn test_local_link_selector() {
        // Test :local-link pseudo-class for same-document link styling
        let css = r#"
            :local-link { color: green; }
            nav :local-link { font-weight: bold; }
        "#;
        let stylesheet = parse_css(css);

        // Both rules should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            2,
            "Should parse 2 rules with :local-link()"
        );

        // Check the declarations
        let has_color = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "color"));
        assert!(has_color, "Should have color from :local-link");

        let has_font_weight = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "font-weight"));
        assert!(
            has_font_weight,
            "Should have font-weight from nav :local-link"
        );
    }

    #[test]
    fn test_scope_selector() {
        // Test :scope pseudo-class for scoped styling
        let css = r#"
            :scope { background: white; }
            :scope > body { margin: 0; }
        "#;
        let stylesheet = parse_css(css);

        // Both rules should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            2,
            "Should parse 2 rules with :scope"
        );

        // Check the declarations
        let has_background = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "background"));
        assert!(has_background, "Should have background from :scope");

        let has_margin = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "margin"));
        assert!(has_margin, "Should have margin from :scope > body");
    }

    #[test]
    fn test_blank_selector() {
        // Test :blank pseudo-class for empty/whitespace-only elements
        let css = r#"
            :blank { display: none; }
            input:blank { border: dashed; }
            textarea:blank { background: #f5f5f5; }
        "#;
        let stylesheet = parse_css(css);

        // All three rules should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            3,
            "Should parse 3 rules with :blank"
        );

        // Check the declarations
        let has_display = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "display"));
        assert!(has_display, "Should have display from :blank");

        let has_border = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "border"));
        assert!(has_border, "Should have border from input:blank");

        let has_background = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "background"));
        assert!(has_background, "Should have background from textarea:blank");
    }

    #[test]
    fn test_current_selector() {
        // Test :current pseudo-class for currently displayed elements
        let css = r#"
            :current { outline: 2px solid blue; }
            li:current { font-weight: bold; }
            step:current { background: yellow; }
        "#;
        let stylesheet = parse_css(css);

        // All three rules should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            3,
            "Should parse 3 rules with :current"
        );

        // Check the declarations
        let has_outline = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "outline"));
        assert!(has_outline, "Should have outline from :current");

        let has_font_weight = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "font-weight"));
        assert!(has_font_weight, "Should have font-weight from li:current");

        let has_background = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "background"));
        assert!(has_background, "Should have background from step:current");
    }

    #[test]
    fn test_past_future_selectors() {
        // Test :past and :future pseudo-classes for timeline interfaces
        let css = r#"
            :past { opacity: 0.5; }
            :future { opacity: 0.3; }
            slide:past { filter: grayscale(100%); }
            slide:future { filter: blur(2px); }
        "#;
        let stylesheet = parse_css(css);

        // All four rules should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            4,
            "Should parse 4 rules with :past/:future"
        );

        // Check the declarations
        let has_opacity = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "opacity"));
        assert!(has_opacity, "Should have opacity from :past and :future");

        let has_filter = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "filter"));
        assert!(has_filter, "Should have filter from slide:past/:future");
    }

    #[test]
    fn test_media_state_selectors() {
        // Test :playing, :paused, and :seeking pseudo-classes for media elements
        let css = r#"
            video:playing { border: 2px solid green; }
            audio:paused { opacity: 0.7; }
            video:seeking { background: yellow; }
        "#;
        let stylesheet = parse_css(css);

        // All three rules should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            3,
            "Should parse 3 rules with media state pseudo-classes"
        );

        // Check the declarations
        let has_border = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "border"));
        assert!(has_border, "Should have border from video:playing");

        let has_opacity = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "opacity"));
        assert!(has_opacity, "Should have opacity from audio:paused");

        let has_background = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "background"));
        assert!(has_background, "Should have background from video:seeking");
    }

    #[test]
    fn test_form_validation_selectors() {
        // Test form validation pseudo-classes
        let css = r#"
            input:valid { border-color: green; }
            input:invalid { border-color: red; }
            input:required { background: #fff8f8; }
            input:optional { background: #f8f8ff; }
            input:in-range { color: black; }
            input:out-of-range { color: red; }
        "#;
        let stylesheet = parse_css(css);

        // All six rules should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            6,
            "Should parse 6 rules with form validation pseudo-classes"
        );

        // Check the declarations
        let has_border_color = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "border-color"));
        assert!(
            has_border_color,
            "Should have border-color from :valid/:invalid"
        );

        let has_background = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "background"));
        assert!(
            has_background,
            "Should have background from :required/:optional"
        );

        let has_color = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "color"));
        assert!(has_color, "Should have color from :in-range/:out-of-range");
    }

    #[test]
    fn test_user_valid_invalid_selectors() {
        // Test :user-valid and :user-invalid pseudo-classes
        let css = r#"
            input:user-valid { border-color: green; }
            input:user-invalid { border-color: red; }
        "#;
        let stylesheet = parse_css(css);

        // Both rules should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            2,
            "Should parse 2 rules with :user-valid/:user-invalid"
        );

        // Check the declarations
        let has_border_color = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "border-color"));
        assert!(
            has_border_color,
            "Should have border-color from :user-valid/:user-invalid"
        );
    }

    #[test]
    fn test_matches_selector() {
        // Test :matches() pseudo-class (legacy name for :is())
        let css = r#"
            :matches(h1, h2, h3) { font-weight: bold; }
            :matches(.btn, .button) { cursor: pointer; }
        "#;
        let stylesheet = parse_css(css);

        // Both rules should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            2,
            "Should parse 2 rules with :matches()"
        );

        // Check the declarations
        let has_font_weight = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "font-weight"));
        assert!(
            has_font_weight,
            "Should have font-weight from :matches(h1, h2, h3)"
        );

        let has_cursor = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "cursor"));
        assert!(
            has_cursor,
            "Should have cursor from :matches(.btn, .button)"
        );
    }

    #[test]
    fn test_structural_pseudo_classes() {
        // Test structural pseudo-classes :root, :first-child, :last-child, etc.
        let css = r#"
            :root { font-size: 16px; }
            :first-child { margin-top: 0; }
            :last-child { margin-bottom: 0; }
            :only-child { border: 2px solid; }
            :first-of-type { font-weight: bold; }
            :last-of-type { font-style: italic; }
            :only-of-type { text-decoration: underline; }
        "#;
        let stylesheet = parse_css(css);

        // All seven rules should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            7,
            "Should parse 7 structural pseudo-class rules"
        );

        // Check each declaration
        assert!(
            stylesheet
                .rules
                .iter()
                .any(|r| { r.declarations.iter().any(|d| d.property == "font-size") }),
            "Should have font-size from :root"
        );

        assert!(
            stylesheet
                .rules
                .iter()
                .any(|r| { r.declarations.iter().any(|d| d.property == "margin-top") }),
            "Should have margin-top from :first-child"
        );

        assert!(
            stylesheet
                .rules
                .iter()
                .any(|r| { r.declarations.iter().any(|d| d.property == "margin-bottom") }),
            "Should have margin-bottom from :last-child"
        );

        assert!(
            stylesheet
                .rules
                .iter()
                .any(|r| { r.declarations.iter().any(|d| d.property == "border") }),
            "Should have border from :only-child"
        );

        assert!(
            stylesheet
                .rules
                .iter()
                .any(|r| { r.declarations.iter().any(|d| d.property == "font-weight") }),
            "Should have font-weight from :first-of-type"
        );

        assert!(
            stylesheet
                .rules
                .iter()
                .any(|r| { r.declarations.iter().any(|d| d.property == "font-style") }),
            "Should have font-style from :last-of-type"
        );

        assert!(
            stylesheet.rules.iter().any(|r| {
                r.declarations
                    .iter()
                    .any(|d| d.property == "text-decoration")
            }),
            "Should have text-decoration from :only-of-type"
        );
    }

    #[test]
    fn test_read_only_write_selectors() {
        // Test :read-only and :read-write pseudo-classes
        let css = r#"
            input:read-only { background: #eee; }
            input:read-write { background: white; }
            textarea:read-only { border: dashed; }
        "#;
        let stylesheet = parse_css(css);

        // All three rules should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            3,
            "Should parse 3 rules with :read-only/:read-write"
        );

        // Check the declarations
        let has_background = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "background"));
        assert!(
            has_background,
            "Should have background from :read-only/:read-write"
        );

        let has_border = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "border"));
        assert!(has_border, "Should have border from textarea:read-only");
    }

    #[test]
    fn test_placeholder_shown_selector() {
        // Test :placeholder-shown pseudo-class
        let css = r#"
            input:placeholder-shown { color: #999; }
            textarea:placeholder-shown { font-style: italic; }
        "#;
        let stylesheet = parse_css(css);

        // Both rules should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            2,
            "Should parse 2 rules with :placeholder-shown"
        );

        // Check the declarations
        let has_color = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "color"));
        assert!(has_color, "Should have color from input:placeholder-shown");

        let has_font_style = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "font-style"));
        assert!(
            has_font_style,
            "Should have font-style from textarea:placeholder-shown"
        );
    }

    #[test]
    fn test_form_state_selectors() {
        // Test :default, :checked, and :indeterminate pseudo-classes
        let css = r#"
            input:default { border: 2px solid blue; }
            input:checked { background: green; }
            input:indeterminate { opacity: 0.5; }
        "#;
        let stylesheet = parse_css(css);

        // All three rules should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            3,
            "Should parse 3 rules with form state pseudo-classes"
        );

        // Check the declarations
        let has_border = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "border"));
        assert!(has_border, "Should have border from input:default");

        let has_background = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "background"));
        assert!(has_background, "Should have background from input:checked");

        let has_opacity = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "opacity"));
        assert!(has_opacity, "Should have opacity from input:indeterminate");
    }

    #[test]
    fn test_target_selector() {
        // Test :target pseudo-class for URL fragment targeting
        let css = r#"
            :target { background: yellow; }
            section:target { border-left: 4px solid blue; }
        "#;
        let stylesheet = parse_css(css);

        // Both rules should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            2,
            "Should parse 2 rules with :target"
        );

        // Check the declarations
        let has_background = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "background"));
        assert!(has_background, "Should have background from :target");

        let has_border = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "border-left"));
        assert!(has_border, "Should have border-left from section:target");
    }

    #[test]
    fn test_enabled_disabled_selectors() {
        // Test :enabled and :disabled pseudo-classes
        let css = r#"
            input:enabled { background: white; }
            input:disabled { background: #eee; }
            button:disabled { opacity: 0.5; }
        "#;
        let stylesheet = parse_css(css);

        // All three rules should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            3,
            "Should parse 3 rules with :enabled/:disabled"
        );

        // Check the declarations
        let has_background = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "background"));
        assert!(
            has_background,
            "Should have background from :enabled/:disabled"
        );

        let has_opacity = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "opacity"));
        assert!(has_opacity, "Should have opacity from button:disabled");
    }

    #[test]
    fn test_css_nesting_ampersand() {
        // Test CSS nesting with & selector
        // Note: :hover causes skip_selector=true, so the expanded rule has an unmatchable selector
        let css = r#"
            .card {
                color: red;
                &:focus { outline: none; }
            }
        "#;
        let stylesheet = parse_css(css);

        // Should have 2 rules: .card and the nested rule
        assert_eq!(
            stylesheet.rules.len(),
            2,
            "Should parse 2 rules from nested CSS"
        );

        // Find the .card rule
        let card_rule = stylesheet.rules.iter().find(|r| {
            r.selectors
                .iter()
                .any(|s| matches!(s, Selector::Class(c) if c == "card"))
        });
        assert!(card_rule.is_some(), "Should have .card rule");

        // The nested rule should have been parsed (even if selector is unmatchable due to :focus)
        let nested_rule = stylesheet
            .rules
            .iter()
            .find(|r| r.declarations.iter().any(|d| d.property == "outline"));
        assert!(
            nested_rule.is_some(),
            "Should have nested rule with outline declaration"
        );
    }

    #[test]
    fn test_css_nesting_implicit() {
        // Test CSS nesting without & (implicit descendant)
        let css = r#"
            .card {
                color: red;
                .title { font-size: 2em; }
            }
        "#;
        let stylesheet = parse_css(css);

        // Should have 2 rules: .card and .card .title
        assert_eq!(
            stylesheet.rules.len(),
            2,
            "Should parse 2 rules from implicit nesting"
        );

        // Find the nested rule by its declaration
        let nested_rule = stylesheet
            .rules
            .iter()
            .find(|r| r.declarations.iter().any(|d| d.property == "font-size"));
        assert!(
            nested_rule.is_some(),
            "Should have nested rule with font-size declaration"
        );
    }

    #[test]
    fn test_css_nesting_multiple_parents() {
        // Test CSS nesting with multiple parent selectors
        let css = r#"
            .card, .panel {
                &:hover { opacity: 0.8; }
            }
        "#;
        let stylesheet = parse_css(css);

        // Should have 3 rules: .card, .panel, and 2 expanded hover rules
        // (one for .card:hover, one for .panel:hover)
        assert!(
            stylesheet.rules.len() >= 2,
            "Should parse rules from multiple parent nesting"
        );
    }

    #[test]
    fn test_marker_pseudo_element() {
        // Test ::marker pseudo-element parsing
        let css = r#"
            li::marker { color: red; }
            li::-webkit-list-bullet { color: blue; }
        "#;
        let stylesheet = parse_css(css);

        // Should parse the ::marker rule (not skip it)
        assert!(stylesheet.rules.len() >= 1, "Should parse ::marker rule");

        // The rule should be for li elements
        let li_rule = stylesheet.rules.iter().find(|r| {
            r.selectors
                .iter()
                .any(|s| matches!(s, Selector::Tag(t) if t == "li"))
        });
        assert!(li_rule.is_some(), "Should have li rule");

        // Check that the color declaration was parsed
        let has_color = stylesheet
            .rules
            .iter()
            .any(|r| r.declarations.iter().any(|d| d.property == "color"));
        assert!(has_color, "Should have parsed color declaration");
    }

    #[test]
    fn test_new_pseudo_classes() {
        // Test :modal, :open, :closed, :popover-open pseudo-classes
        let css = r#"
            dialog:modal { border: 2px solid; }
            details:open { display: block; }
            [popover]:popover-open { display: block; }
        "#;
        let stylesheet = parse_css(css);

        // All rules should be parsed (not skipped due to pseudo-classes)
        assert_eq!(
            stylesheet.rules.len(),
            3,
            "Should parse all pseudo-class rules"
        );

        // Check for dialog:modal rule
        let dialog_rule = stylesheet.rules.iter().find(|r| {
            r.selectors
                .iter()
                .any(|s| matches!(s, Selector::Tag(t) if t == "dialog"))
        });
        assert!(dialog_rule.is_some(), "Should have dialog:modal rule");

        // Check for details:open rule
        let details_rule = stylesheet.rules.iter().find(|r| {
            r.selectors
                .iter()
                .any(|s| matches!(s, Selector::Tag(t) if t == "details"))
        });
        assert!(details_rule.is_some(), "Should have details:open rule");

        // Check for [popover]:popover-open rule (attribute + pseudo)
        let popover_rule = stylesheet
            .rules
            .iter()
            .find(|r| r.declarations.iter().any(|d| d.property == "display"));
        assert!(popover_rule.is_some(), "Should have popover-open rule");
    }

    #[test]
    fn test_color_level_4_and_5() {
        // Test CSS Color Level 4/5 color functions
        let css = r#"
            .hwb { color: hwb(120 30% 20%); background: hwb(0 0% 0% / 0.5); }
            .lab { color: lab(50% 20 -30); }
            .lch { color: lch(50% 40 180); }
            .oklab { color: oklab(60% 0.1 -0.1); }
            .oklch { color: oklch(60% 0.2 250); }
        "#;
        let stylesheet = parse_css(css);

        // All 5 rules should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            5,
            "Should parse all color function rules"
        );

        // Check HWB color parsing
        let hwb_rule = stylesheet
            .rules
            .iter()
            .find(|r| {
                r.selectors
                    .iter()
                    .any(|s| matches!(s, Selector::Class(c) if c == "hwb"))
            })
            .expect("Should have hwb rule");
        let hwb_color = hwb_rule.declarations.iter().find(|d| d.property == "color");
        assert!(hwb_color.is_some(), "Should parse hwb() color");

        // Check LAB color parsing
        let lab_rule = stylesheet
            .rules
            .iter()
            .find(|r| {
                r.selectors
                    .iter()
                    .any(|s| matches!(s, Selector::Class(c) if c == "lab"))
            })
            .expect("Should have lab rule");
        let lab_color = lab_rule.declarations.iter().find(|d| d.property == "color");
        assert!(lab_color.is_some(), "Should parse lab() color");

        // Check LCH color parsing
        let lch_rule = stylesheet
            .rules
            .iter()
            .find(|r| {
                r.selectors
                    .iter()
                    .any(|s| matches!(s, Selector::Class(c) if c == "lch"))
            })
            .expect("Should have lch rule");
        let lch_color = lch_rule.declarations.iter().find(|d| d.property == "color");
        assert!(lch_color.is_some(), "Should parse lch() color");

        // Check OKLAB color parsing
        let oklab_rule = stylesheet
            .rules
            .iter()
            .find(|r| {
                r.selectors
                    .iter()
                    .any(|s| matches!(s, Selector::Class(c) if c == "oklab"))
            })
            .expect("Should have oklab rule");
        let oklab_color = oklab_rule
            .declarations
            .iter()
            .find(|d| d.property == "color");
        assert!(oklab_color.is_some(), "Should parse oklab() color");

        // Check OKLCH color parsing
        let oklch_rule = stylesheet
            .rules
            .iter()
            .find(|r| {
                r.selectors
                    .iter()
                    .any(|s| matches!(s, Selector::Class(c) if c == "oklch"))
            })
            .expect("Should have oklch rule");
        let oklch_color = oklch_rule
            .declarations
            .iter()
            .find(|d| d.property == "color");
        assert!(oklch_color.is_some(), "Should parse oklch() color");
    }

    #[test]
    fn test_color_mix_function() {
        // Test CSS color-mix() function
        let css = r#"
            .mixed { color: color-mix(red 50%, blue 50%); }
            .in-srgb { color: color-mix(in srgb, red 30%, green 70%); }
        "#;
        let stylesheet = parse_css(css);

        // Both rules should be parsed
        assert_eq!(stylesheet.rules.len(), 2, "Should parse color-mix rules");

        // Check mixed color parsing
        let mixed_rule = stylesheet
            .rules
            .iter()
            .find(|r| {
                r.selectors
                    .iter()
                    .any(|s| matches!(s, Selector::Class(c) if c == "mixed"))
            })
            .expect("Should have mixed rule");
        let mixed_color = mixed_rule
            .declarations
            .iter()
            .find(|d| d.property == "color");
        assert!(mixed_color.is_some(), "Should parse color-mix()");
    }

    #[test]
    fn test_relative_colors() {
        // Test CSS Color Level 5 relative colors
        let css = r#"
            .rgb-relative { color: rgb(from red r g b); }
            .hsl-relative { color: hsl(from blue h s l); }
            .hwb-relative { color: hwb(from green h w b); }
        "#;
        let stylesheet = parse_css(css);

        // All three rules should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            3,
            "Should parse relative color rules"
        );

        // Check RGB relative color
        let rgb_rule = stylesheet
            .rules
            .iter()
            .find(|r| {
                r.selectors
                    .iter()
                    .any(|s| matches!(s, Selector::Class(c) if c == "rgb-relative"))
            })
            .expect("Should have rgb-relative rule");
        let rgb_color = rgb_rule.declarations.iter().find(|d| d.property == "color");
        assert!(rgb_color.is_some(), "Should parse rgb(from ...)");

        // Check HSL relative color
        let hsl_rule = stylesheet
            .rules
            .iter()
            .find(|r| {
                r.selectors
                    .iter()
                    .any(|s| matches!(s, Selector::Class(c) if c == "hsl-relative"))
            })
            .expect("Should have hsl-relative rule");
        let hsl_color = hsl_rule.declarations.iter().find(|d| d.property == "color");
        assert!(hsl_color.is_some(), "Should parse hsl(from ...)");

        // Check HWB relative color
        let hwb_rule = stylesheet
            .rules
            .iter()
            .find(|r| {
                r.selectors
                    .iter()
                    .any(|s| matches!(s, Selector::Class(c) if c == "hwb-relative"))
            })
            .expect("Should have hwb-relative rule");
        let hwb_color = hwb_rule.declarations.iter().find(|d| d.property == "color");
        assert!(hwb_color.is_some(), "Should parse hwb(from ...)");
    }

    #[test]
    fn test_nth_child_parsing() {
        // Test CSS :nth-child() parsing
        let css = r#"
            li:nth-child(2n+1) { color: orange; }
            tr:nth-child(even) { background: #f0f0f0; }
            div:nth-child(3) { border: 1px solid; }
            p:nth-of-type(odd) { margin: 1em; }
        "#;
        let stylesheet = parse_css(css);

        // All 4 rules should be parsed
        assert_eq!(
            stylesheet.rules.len(),
            4,
            "Should parse all nth-child rules"
        );

        // Check that selectors contain NthChild or NthOfType (they may be wrapped in Compound)
        let has_nth_child = stylesheet.rules.iter().any(|r| {
            r.selectors.iter().any(|s| {
                matches!(s, Selector::NthChild { .. })
                    || matches!(s, Selector::Compound(parts) if parts.iter().any(|p| matches!(p, Selector::NthChild { .. })))
            })
        });
        assert!(has_nth_child, "Should have NthChild selector");

        let has_nth_of_type = stylesheet.rules.iter().any(|r| {
            r.selectors.iter().any(|s| {
                matches!(s, Selector::NthOfType { .. })
                    || matches!(s, Selector::Compound(parts) if parts.iter().any(|p| matches!(p, Selector::NthOfType { .. })))
            })
        });
        assert!(has_nth_of_type, "Should have NthOfType selector");
    }

    #[test]
    fn test_nth_child_matching() {
        // Test that NthChild matches correctly
        let sel = Selector::NthChild { a: 2, b: 1 }; // odd
        assert!(check_nth_formula(2, 1, 1)); // 2*0 + 1 = 1 ✓
        assert!(check_nth_formula(2, 1, 3)); // 2*1 + 1 = 3 ✓
        assert!(check_nth_formula(2, 1, 5)); // 2*2 + 1 = 5 ✓
        assert!(!check_nth_formula(2, 1, 2)); // Not odd
        assert!(!check_nth_formula(2, 1, 4)); // Not odd

        // Even selector
        assert!(check_nth_formula(2, 0, 2)); // 2*1 + 0 = 2 ✓
        assert!(check_nth_formula(2, 0, 4)); // 2*2 + 0 = 4 ✓
        assert!(!check_nth_formula(2, 0, 1)); // Not even

        // Specific position
        assert!(check_nth_formula(0, 3, 3)); // Just position 3
        assert!(!check_nth_formula(0, 3, 2)); // Not position 3
    }
}
