use cssparser::{ParseError, Parser, ParserInput, Token};
use incognidium_dom::{Document, ElementData, NodeData, NodeId};

/// A parsed CSS stylesheet.
#[derive(Debug, Default, Clone)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
    /// CSS custom properties (variables) from :root rules
    pub variables: std::collections::HashMap<String, String>,
}

/// A CSS rule: selectors + declarations.
#[derive(Debug, Clone)]
pub struct Rule {
    pub selectors: Vec<Selector>,
    pub declarations: Vec<Declaration>,
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
}

impl Selector {
    /// Compute specificity as (id_count, class_count, tag_count).
    pub fn specificity(&self) -> (u32, u32, u32) {
        match self {
            Selector::Universal => (0, 0, 0),
            Selector::Tag(_) => (0, 0, 1),
            Selector::Class(_) => (0, 1, 0),
            Selector::Attribute(_, _) => (0, 1, 0),
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
            Selector::Descendant(a, d) | Selector::Child(a, d)
            | Selector::AdjacentSibling(a, d) | Selector::GeneralSibling(a, d) => {
                let sa = a.specificity();
                let sd = d.specificity();
                (sa.0 + sd.0, sa.1 + sd.1, sa.2 + sd.2)
            }
        }
    }

    /// Check if this simple selector matches an element (no ancestor check).
    pub fn matches_element(&self, element: &ElementData) -> bool {
        match self {
            Selector::Universal => true,
            Selector::Tag(tag) => element.tag_name == *tag,
            Selector::Class(class) => element.classes().contains(&class.as_str()),
            Selector::Attribute(attr, val) => match val {
                Some(v) => element.get_attr(attr).map(|a| a == v).unwrap_or(false),
                None => element.get_attr(attr).is_some(),
            },
            Selector::Id(id) => element.id() == Some(id.as_str()),
            Selector::Compound(parts) => parts.iter().all(|p| p.matches_element(element)),
            // For descendant/child, only check the rightmost part
            Selector::Descendant(_, descendant) => descendant.matches_element(element),
            Selector::Child(_, child) => child.matches_element(element),
            Selector::AdjacentSibling(_, target) | Selector::GeneralSibling(_, target) => {
                target.matches_element(element)
            }
        }
    }

    /// Check if this selector matches a given element in the document context.
    pub fn matches(&self, element: &ElementData, doc: &Document, node_id: NodeId) -> bool {
        match self {
            Selector::Universal => true,
            Selector::Tag(tag) => element.tag_name == *tag,
            Selector::Class(class) => element.classes().contains(&class.as_str()),
            Selector::Attribute(attr, val) => {
                match val {
                    Some(v) => element.get_attr(attr).map(|a| a == v).unwrap_or(false),
                    None => element.get_attr(attr).is_some(),
                }
            }
            Selector::Id(id) => element.id() == Some(id.as_str()),
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
                        if sid == node_id { return false; }
                        if let NodeData::Element(ref e) = doc.node(sid).data {
                            if prev_sel.matches(e, doc, sid) {
                                return true;
                            }
                        }
                    }
                }
                false
            }
        }
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
}

impl CssValue {
    pub fn to_px(&self, parent_font_size: f32, viewport_width: f32, viewport_height: f32) -> Option<f32> {
        match self {
            CssValue::Length(v, LengthUnit::Px) => Some(*v),
            CssValue::Length(v, LengthUnit::Em) => Some(*v * parent_font_size),
            CssValue::Length(v, LengthUnit::Rem) => Some(*v * 16.0), // root em = 16px default
            CssValue::Length(v, LengthUnit::Pt) => Some(*v * 4.0 / 3.0),
            CssValue::Length(v, LengthUnit::Vw) => Some(*v * viewport_width / 100.0),
            CssValue::Length(v, LengthUnit::Vh) => Some(*v * viewport_height / 100.0),
            CssValue::Number(v) if *v == 0.0 => Some(0.0),
            CssValue::Percentage(p) => Some(*p / 100.0 * parent_font_size),
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
                if keyword == "media" {
                    // Check if this media query applies to us (screen, min-width <= 1024)
                    let applies = should_apply_media_query(&mut parser);
                    // Now consume the CurlyBracketBlock
                    match parser.next() {
                        Ok(&Token::CurlyBracketBlock) => {
                            if applies {
                                let _: Result<(), ParseError<'_, ()>> = parser.parse_nested_block(|p| {
                                    while !p.is_exhausted() {
                                        if let Ok(rule) = parse_rule(p) {
                                            stylesheet.rules.push(rule);
                                        } else {
                                            let _ = p.next();
                                        }
                                    }
                                    Ok(())
                                });
                            } else {
                                // Skip the block contents
                                let _: Result<(), ParseError<'_, ()>> = parser.parse_nested_block(|p| {
                                    while p.next().is_ok() {}
                                    Ok(())
                                });
                            }
                        }
                        _ => {} // No block, skip
                    }
                } else {
                    skip_at_rule(&mut parser);
                }
                continue;
            }
            _ => parser.reset(&state),
        }

        if let Ok(rule) = parse_rule(&mut parser) {
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
                        CssValue::Length(v, _) => format!("{}px", v),
                        CssValue::Number(n) => format!("{}", n),
                        CssValue::Percentage(p) => format!("{}%", p),
                        _ => String::new(),
                    };
                    if !val_str.is_empty() {
                        if is_broad_selector || !stylesheet.variables.contains_key(&decl.property) {
                            stylesheet.variables.insert(decl.property.clone(), val_str);
                        }
                    }
                }
            }
            stylesheet.rules.push(rule);
        } else {
            let _ = parser.next();
        }
    }

    stylesheet
}

/// Check if a @media query applies to our viewport (1024px screen).
fn should_apply_media_query<'i>(parser: &mut Parser<'i, '_>) -> bool {
    let mut state = MediaMatchState::default();
    scan_media_tokens(parser, &mut state);

    if state.has_print_only && !state.has_screen { return false; }
    if state.has_dark_scheme { return false; }
    if state.reject { return false; }
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
                    if px_val > 1024.0 { state.reject = true; }
                    state.last_was_min_width = false;
                }
                if state.last_was_max_width {
                    if px_val < 1024.0 { state.reject = true; }
                    state.last_was_max_width = false;
                }
            }
            Ok(&Token::Number { value, .. }) => {
                if state.last_was_min_width && value > 1024.0 { state.reject = true; }
                if state.last_was_max_width && value < 1024.0 { state.reject = true; }
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

fn parse_rule<'i>(parser: &mut Parser<'i, '_>) -> Result<Rule, ParseError<'i, ()>> {
    // Parse selectors, collecting tokens until we hit CurlyBracketBlock
    let selectors = parse_selectors(parser)?;

    // After parse_selectors, the CurlyBracketBlock has been consumed.
    // parse_nested_block will parse inside it.
    let mut declarations = Vec::new();
    let _: Result<(), ParseError<'_, ()>> = parser.parse_nested_block(|parser| {
        loop {
            if parser.is_exhausted() {
                break;
            }
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
    })
}

fn parse_selectors<'i>(parser: &mut Parser<'i, '_>) -> Result<Vec<Selector>, ParseError<'i, ()>> {
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
        return Err(parser.new_basic_unexpected_token_error(Token::Ident("".into())).into());
    }

    if selectors.is_empty() {
        // We consumed { but got no selectors — consume the block and return error
        let _: Result<(), ParseError<'_, ()>> = parser.parse_nested_block(|p| {
            while p.next().is_ok() {}
            Ok(())
        });
        return Err(parser.new_basic_unexpected_token_error(Token::Ident("".into())).into());
    }

    Ok(selectors)
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
            _ => { parser.reset(&state); break; }
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
            // Handle pseudo-classes/elements
            Ok(&Token::Colon) => {
                match parser.next_including_whitespace() {
                    Ok(&Token::Colon) => {
                        // ::pseudo-element (::before, ::after, ::first-line, etc.)
                        // We don't render pseudo-elements, so mark selector unmatchable
                        // to prevent their rules from being applied to the base element.
                        let _ = parser.next_including_whitespace();
                        skip_selector = true;
                    }
                    Ok(Token::Ident(ref name)) => {
                        // :visited, :hover, :focus, :active represent non-default states
                        // We can't match these, so mark selector as unmatchable
                        let pseudo = name.to_string().to_lowercase();
                        match pseudo.as_str() {
                            "visited" | "hover" | "focus" | "active" | "focus-within"
                            | "focus-visible" => {
                                skip_selector = true;
                            }
                            // :link, :first-child, :last-child, :nth-child, etc. are fine
                            _ => {}
                        }
                    }
                    Ok(Token::Function(ref fn_name)) => {
                        let fn_lower = fn_name.to_string().to_lowercase();
                        let mut inner_is_simple_negation = false;
                        let _: Result<(), ParseError<'_, ()>> = parser.parse_nested_block(|p| {
                            if fn_lower == "not" {
                                let state = p.state();
                                match p.next() {
                                    // :not(:focus) etc. — state pseudo, always true
                                    Ok(&Token::Colon) => {
                                        if let Ok(Token::Ident(ref name)) = p.next() {
                                            let pseudo = name.to_string().to_lowercase();
                                            if matches!(pseudo.as_str(),
                                                "hover" | "focus" | "active" | "visited"
                                                | "focus-within" | "focus-visible") {
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
                                    _ => { p.reset(&state); }
                                }
                            }
                            while p.next().is_ok() {}
                            Ok(())
                        });
                        if inner_is_simple_negation {
                            // Treat as always-true: don't skip selector.
                        } else {
                            match fn_lower.as_str() {
                                "nth-child" | "nth-of-type" | "nth-last-child"
                                | "nth-last-of-type" | "is" | "where" | "has"
                                | "lang" | "dir" | "state" => {
                                    skip_selector = true;
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            // Handle attribute selectors [attr], [attr=val], [attr~=val], etc.
            Ok(&Token::SquareBracketBlock) => {
                let attr_sel: Result<(String, Option<String>), ParseError<'_, ()>> = parser.parse_nested_block(|p| {
                    let attr_name = match p.next() {
                        Ok(Token::Ident(ref name)) => name.to_string(),
                        _ => { while p.next().is_ok() {} return Ok(("".into(), None)); }
                    };
                    // Check for operator + value
                    match p.next() {
                        Ok(Token::Delim('=')) => {
                            // [attr=val]
                            match p.next() {
                                Ok(Token::Ident(ref v)) => Ok((attr_name, Some(v.to_string()))),
                                Ok(Token::QuotedString(ref v)) => Ok((attr_name, Some(v.to_string()))),
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
        0 => Err(parser.new_basic_unexpected_token_error(Token::Ident("".into())).into()),
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
            Ok(Token::Ident(_)) | Ok(Token::Delim('.')) | Ok(Token::IDHash(_))
            | Ok(Token::Delim('*')) | Ok(&Token::Colon) | Ok(&Token::SquareBracketBlock) => {
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

fn parse_declaration<'i>(parser: &mut Parser<'i, '_>) -> Result<Declaration, ParseError<'i, ()>> {
    let property = match parser.next() {
        Ok(Token::Ident(name)) => name.to_string().to_lowercase(),
        _ => {
            return Err(parser.new_basic_unexpected_token_error(Token::Ident("".into())).into());
        }
    };

    parser.expect_colon()?;

    let mut value = parse_value(parser, &property)?;

    // For box model shorthands, collect up to 4 values
    if matches!(property.as_str(), "margin" | "padding" | "border-width" | "border-radius"
        | "border" | "border-top" | "border-right" | "border-bottom" | "border-left") {
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
            if parser.try_parse(|p| -> Result<(), ParseError<'i, ()>> {
                match p.next() {
                    Ok(Token::Delim('/')) => Ok(()),
                    _ => Err(p.new_basic_unexpected_token_error(Token::Delim('/')).into()),
                }
            }).is_ok() {
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
            if parser.try_parse(|p| -> Result<(), ParseError<'i, ()>> {
                match p.next() {
                    Ok(Token::Delim('/')) => Ok(()),
                    _ => Err(p.new_basic_unexpected_token_error(Token::Delim('/')).into()),
                }
            }).is_ok() {
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
    if matches!(property.as_str(), "grid-template-columns" | "grid-template-rows" | "grid-template-areas") {
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
        if vals.len() >= 3 {
            value = CssValue::List(vals);
        } else if vals.len() == 2 {
            value = CssValue::List(vals);
        }
        // Single value already in `value`
    }

    // Skip any remaining value tokens (e.g. "Verdana, Geneva, sans-serif" for font-family)
    // Stop at semicolon, !important, or end of block
    let important = loop {
        let state = parser.state();
        match parser.next() {
            Ok(Token::Semicolon) => break false,
            Ok(Token::Delim('!')) => {
                let is_important = parser.try_parse(|p| p.expect_ident_matching("important")).is_ok();
                let _ = parser.try_parse(|p| p.expect_semicolon());
                break is_important;
            }
            Err(_) => break false, // end of block
            _ => continue, // skip extra value tokens
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
            value,
            ref unit,
            ..
        }) => {
            let u = match unit.as_ref() {
                "px" => LengthUnit::Px,
                "em" => LengthUnit::Em,
                "rem" => LengthUnit::Rem,
                "pt" => LengthUnit::Pt,
                "vw" => LengthUnit::Vw,
                "vh" => LengthUnit::Vh,
                "fr" => LengthUnit::Fr,
                _ => LengthUnit::Px,
            };
            Ok(CssValue::Length(value, u))
        }
        Ok(&Token::Percentage { unit_value, .. }) => {
            Ok(CssValue::Percentage(unit_value * 100.0))
        }
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
                    let parsed = parser.parse_nested_block(|p| -> Result<(String, Option<String>), ParseError<'i, ()>> {
                        let var_name = match p.next() {
                            Ok(Token::Ident(ref name)) => name.to_string(),
                            _ => String::new(),
                        };
                        // Check for comma-separated fallback
                        let fallback = if p.try_parse(|p| p.expect_comma()).is_ok() {
                            // Collect remaining tokens as the fallback value
                            let mut fb = String::new();
                            while let Ok(tok) = p.next() {
                                match tok {
                                    Token::Ident(ref s) => { if !fb.is_empty() { fb.push(' '); } fb.push_str(s); }
                                    Token::Number { value: ref v, .. } => { if !fb.is_empty() { fb.push(' '); } fb.push_str(&format!("{}", v)); }
                                    Token::Percentage { unit_value, .. } => { if !fb.is_empty() { fb.push(' '); } fb.push_str(&format!("{}%", unit_value * 100.0)); }
                                    Token::Dimension { value: ref v, ref unit, .. } => { if !fb.is_empty() { fb.push(' '); } fb.push_str(&format!("{}{}", v, unit)); }
                                    Token::Hash(ref h) => { fb.push('#'); fb.push_str(h); }
                                    Token::UnquotedUrl(ref u) => { fb.push_str(u); }
                                    Token::Comma => { fb.push_str(", "); }
                                    Token::WhiteSpace(_) => { fb.push(' '); }
                                    _ => { /* skip unknown tokens */ }
                                }
                            }
                            Some(fb)
                        } else {
                            None
                        };
                        Ok((var_name, fallback))
                    })?;
                    // Return as a special keyword: var(--name) or var(--name,fallback)
                    let encoded = match parsed {
                        (name, Some(fb)) => format!("var({},{})", name, fb),
                        (name, None) => format!("var({})", name),
                    };
                    Ok(CssValue::Keyword(encoded))
                }
                "repeat" => {
                    // repeat(count | auto-fill | auto-fit, track-size...) -> expand into a List
                    let vals = parser.parse_nested_block(|p| -> Result<Vec<CssValue>, ParseError<'i, ()>> {
                        // Parse the count (integer or auto-fill/auto-fit)
                        let mut auto_fill = false;
                        let count = match p.next() {
                            Ok(&Token::Number { int_value: Some(n), .. }) => n as usize,
                            Ok(&Token::Ident(ref kw)) if kw.eq_ignore_ascii_case("auto-fill") || kw.eq_ignore_ascii_case("auto-fit") => {
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
                            let min_px = track_vals.iter().find_map(|v| match v {
                                CssValue::List(inner) if inner.len() >= 3 => {
                                    // minmax(min, max) — use min
                                    match &inner[1] {
                                        CssValue::Length(px, _) => Some(*px),
                                        _ => None,
                                    }
                                }
                                CssValue::Length(px, _) => Some(*px),
                                _ => None,
                            }).unwrap_or(200.0);
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
                    })?;
                    Ok(CssValue::List(vals))
                }
                "minmax" => {
                    // minmax(min, max) -> store as a keyword "minmax(min,max)"
                    let result = parser.parse_nested_block(|p| -> Result<CssValue, ParseError<'i, ()>> {
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
                "linear-gradient" | "radial-gradient" | "repeating-linear-gradient" => {
                    // Extract the first color from the gradient for background approximation
                    let color = parser.parse_nested_block(|p| -> Result<CssColor, ParseError<'i, ()>> {
                        // Try to find any color in the gradient args
                        loop {
                            let state = p.state();
                            match p.next() {
                                Ok(Token::Hash(ref h)) | Ok(Token::IDHash(ref h)) => {
                                    if let Some(c) = parse_hex_color(&h.to_string()) {
                                        // Consume rest
                                        while p.next().is_ok() {}
                                        return Ok(c);
                                    }
                                }
                                Ok(Token::Ident(ref name)) => {
                                    if let Some(c) = named_color(&name.to_string().to_lowercase()) {
                                        while p.next().is_ok() {}
                                        return Ok(c);
                                    }
                                }
                                Ok(Token::Function(ref fn_name)) if fn_name.eq_ignore_ascii_case("rgb") || fn_name.eq_ignore_ascii_case("rgba") => {
                                    if let Ok(c) = p.parse_nested_block(|p2| parse_rgb_function(p2)) {
                                        while p.next().is_ok() {}
                                        return Ok(c);
                                    }
                                }
                                Err(_) => break,
                                _ => continue,
                            }
                        }
                        Err(p.new_basic_unexpected_token_error(Token::Ident("".into())).into())
                    });
                    match color {
                        Ok(c) => Ok(CssValue::Color(c)),
                        Err(_) => Ok(CssValue::Keyword(fname)),
                    }
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
            Err(parser.new_basic_unexpected_token_error(Token::Ident("".into())).into())
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
            if let Some(c) = parse_hex_color(&h.to_string()) {
                Ok(c)
            } else {
                parser.reset(&state);
                Err(parser.new_basic_unexpected_token_error(Token::Ident("".into())).into())
            }
        }
        Ok(Token::Ident(ref name)) => {
            if let Some(c) = named_color(&name.to_string().to_lowercase()) {
                Ok(c)
            } else {
                parser.reset(&state);
                Err(parser.new_basic_unexpected_token_error(Token::Ident("".into())).into())
            }
        }
        Ok(Token::Function(ref name)) if name.eq_ignore_ascii_case("rgb") || name.eq_ignore_ascii_case("rgba") => {
            parser.parse_nested_block(|p| parse_rgb_function(p))
        }
        _ => {
            parser.reset(&state);
            Err(parser.new_basic_unexpected_token_error(Token::Ident("".into())).into())
        }
    }
}

fn parse_rgb_function<'i>(parser: &mut Parser<'i, '_>) -> Result<CssColor, ParseError<'i, ()>> {
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
fn parse_hsl_function<'i>(parser: &mut Parser<'i, '_>) -> Result<CssColor, ParseError<'i, ()>> {
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
        assert_eq!(
            parse_hex_color("f00"),
            Some(CssColor::from_rgb(255, 0, 0))
        );
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
        el.attributes
            .insert("id".to_string(), "main".to_string());

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
        assert!(matches!(&stylesheet.rules[0].selectors[0], Selector::Descendant(..)));
        // .a > .b should be a Child selector
        assert!(matches!(&stylesheet.rules[1].selectors[0], Selector::Child(..)));
    }

    #[test]
    fn test_descendant_selector_matching() {
        use incognidium_dom::{Document, NodeData, TextData};
        // Build: <div class="outer"><p class="inner">text</p></div>
        let mut doc = Document::new();
        let html = doc.add_node(0, NodeData::Element(ElementData::new("html")));
        let mut outer = ElementData::new("div");
        outer.attributes.insert("class".to_string(), "outer".to_string());
        let outer_id = doc.add_node(html, NodeData::Element(outer));
        let mut inner = ElementData::new("p");
        inner.attributes.insert("class".to_string(), "inner".to_string());
        let inner_id = doc.add_node(outer_id, NodeData::Element(inner));

        let sel = Selector::Descendant(
            Box::new(Selector::Class("outer".into())),
            Box::new(Selector::Class("inner".into())),
        );
        let inner_el = if let NodeData::Element(ref e) = doc.node(inner_id).data { e } else { panic!() };
        assert!(sel.matches(inner_el, &doc, inner_id));
        // outer should NOT match (it's the ancestor, not the descendant)
        let outer_el = if let NodeData::Element(ref e) = doc.node(outer_id).data { e } else { panic!() };
        assert!(!sel.matches(outer_el, &doc, outer_id));
    }

    #[test]
    fn test_inline_style_multivalue_padding() {
        // Wikipedia uses: padding:0 0.9em 0 0; width:300px;
        let decls = parse_inline_style("padding:0 0.9em 0 0; width:300px;");
        eprintln!("Parsed inline decls: {:?}", decls);
        let has_width = decls.iter().any(|d| d.property == "width");
        assert!(has_width, "width:300px not found after multi-value padding. Got: {:?}", decls);
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
        assert!(stylesheet.rules.iter().any(|r|
            r.selectors.iter().any(|s| matches!(s, Selector::Tag(t) if t == "p"))
        ));
    }

    #[test]
    fn test_font_family_doesnt_break_subsequent_decls() {
        let css = "td { font-family:Verdana, Geneva, sans-serif; font-size:10pt; color:#828282; }";
        let stylesheet = parse_css(css);
        assert_eq!(stylesheet.rules.len(), 1);
        let rule = &stylesheet.rules[0];
        eprintln!("Declarations: {:?}", rule.declarations);
        // Should have 3 declarations: font-family, font-size, color
        assert!(rule.declarations.len() >= 3,
            "Expected >= 3 declarations, got {}: {:?}", rule.declarations.len(), rule.declarations);
        // font-size should be 10pt
        let fs = rule.declarations.iter().find(|d| d.property == "font-size").expect("font-size missing");
        assert!(matches!(fs.value, CssValue::Length(10.0, LengthUnit::Pt)), "font-size value: {:?}", fs.value);
        // color should be #828282
        let col = rule.declarations.iter().find(|d| d.property == "color").expect("color missing");
        assert!(matches!(col.value, CssValue::Color(CssColor { r: 0x82, g: 0x82, b: 0x82, a: 0xff })),
            "color value: {:?}", col.value);
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
        assert!(stylesheet.rules.len() >= 1, "Should parse rule inside @media");
        assert!(stylesheet.rules[0].declarations.iter().any(|d| d.property == "display"),
            "Should have display declaration");
    }
}
