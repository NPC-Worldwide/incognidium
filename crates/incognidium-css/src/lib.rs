use cssparser::{ParseError, Parser, ParserInput, Token};
use incognidium_dom::{Document, ElementData, NodeData, NodeId};

/// A parsed CSS stylesheet.
#[derive(Debug, Default, Clone)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
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
    /// Compound: tag.class, tag#id, .class1.class2
    Compound(Vec<Selector>),
    /// Descendant: `.foo .bar` — bar inside foo (any depth)
    Descendant(Box<Selector>, Box<Selector>),
    /// Child: `.foo > .bar` — bar is direct child of foo
    Child(Box<Selector>, Box<Selector>),
}

impl Selector {
    /// Compute specificity as (id_count, class_count, tag_count).
    pub fn specificity(&self) -> (u32, u32, u32) {
        match self {
            Selector::Universal => (0, 0, 0),
            Selector::Tag(_) => (0, 0, 1),
            Selector::Class(_) => (0, 1, 0),
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
            Selector::Descendant(ancestor, descendant) | Selector::Child(ancestor, descendant) => {
                let a = ancestor.specificity();
                let d = descendant.specificity();
                (a.0 + d.0, a.1 + d.1, a.2 + d.2)
            }
        }
    }

    /// Check if this simple selector matches an element (no ancestor check).
    pub fn matches_element(&self, element: &ElementData) -> bool {
        match self {
            Selector::Universal => true,
            Selector::Tag(tag) => element.tag_name == *tag,
            Selector::Class(class) => element.classes().contains(&class.as_str()),
            Selector::Id(id) => element.id() == Some(id.as_str()),
            Selector::Compound(parts) => parts.iter().all(|p| p.matches_element(element)),
            // For descendant/child, only check the rightmost part
            Selector::Descendant(_, descendant) => descendant.matches_element(element),
            Selector::Child(_, child) => child.matches_element(element),
        }
    }

    /// Check if this selector matches a given element in the document context.
    pub fn matches(&self, element: &ElementData, doc: &Document, node_id: NodeId) -> bool {
        match self {
            Selector::Universal => true,
            Selector::Tag(tag) => element.tag_name == *tag,
            Selector::Class(class) => element.classes().contains(&class.as_str()),
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
                // Check direct parent
                if let Some(parent_id) = doc.node(node_id).parent {
                    if let NodeData::Element(ref parent_el) = doc.node(parent_id).data {
                        return parent_sel.matches(parent_el, doc, parent_id);
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
    pub fn to_px(&self, parent_font_size: f32) -> Option<f32> {
        match self {
            CssValue::Length(v, LengthUnit::Px) => Some(*v),
            CssValue::Length(v, LengthUnit::Em) => Some(*v * parent_font_size),
            CssValue::Length(v, LengthUnit::Rem) => Some(*v * 16.0), // root em = 16px default
            CssValue::Length(v, LengthUnit::Pt) => Some(*v * 4.0 / 3.0),
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
        // Handle at-rules (@media, @import, @keyframes, etc.) by skipping them
        let state = parser.state();
        match parser.next() {
            Ok(Token::AtKeyword(_)) => {
                skip_at_rule(&mut parser);
                continue;
            }
            _ => parser.reset(&state),
        }

        if let Ok(rule) = parse_rule(&mut parser) {
            stylesheet.rules.push(rule);
        } else {
            // Skip one token on error; if it's a CurlyBracketBlock the
            // block contents are auto-skipped by cssparser on the next next()
            let _ = parser.next();
        }
    }

    stylesheet
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
            // Handle pseudo-classes/elements — skip but don't break
            Ok(&Token::Colon) => {
                match parser.next_including_whitespace() {
                    Ok(&Token::Colon) => { let _ = parser.next_including_whitespace(); }
                    Ok(Token::Ident(_)) => {}
                    Ok(Token::Function(_)) => {
                        let _: Result<(), ParseError<'_, ()>> = parser.parse_nested_block(|p| {
                            while p.next().is_ok() {}
                            Ok(())
                        });
                    }
                    _ => {}
                }
            }
            // Handle attribute selectors [attr]
            Ok(&Token::SquareBracketBlock) => {
                let _: Result<(), ParseError<'_, ()>> = parser.parse_nested_block(|p| {
                    while p.next().is_ok() {}
                    Ok(())
                });
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
            Ok(Token::Delim('+')) | Ok(Token::Delim('~')) => {
                // Sibling combinators — we don't support matching these,
                // but parse the next selector to stay in sync.
                // Treat as descendant (overly broad but better than nothing).
                if let Ok(next) = parse_simple_selector(parser) {
                    result = Selector::Descendant(Box::new(result), Box::new(next));
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

    let value = parse_value(parser, &property)?;

    let important = parser
        .try_parse(|p| {
            p.expect_delim('!')?;
            p.expect_ident_matching("important")
        })
        .is_ok();

    // Consume optional semicolon
    let _ = parser.try_parse(|p| p.expect_semicolon());

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
            let keyword = kw.to_string().to_lowercase();
            match keyword.as_str() {
                "auto" => Ok(CssValue::Auto),
                "none" => Ok(CssValue::None),
                "inherit" => Ok(CssValue::Inherit),
                _ => {
                    // Check if it's a named color
                    if is_color_property(property) {
                        if let Some(color) = named_color(&keyword) {
                            return Ok(CssValue::Color(color));
                        }
                    }
                    Ok(CssValue::Keyword(keyword))
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
                _ => LengthUnit::Px,
            };
            Ok(CssValue::Length(value, u))
        }
        Ok(&Token::Percentage { unit_value, .. }) => {
            Ok(CssValue::Percentage(unit_value * 100.0))
        }
        Ok(&Token::Number { value, .. }) => Ok(CssValue::Number(value)),
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
}
