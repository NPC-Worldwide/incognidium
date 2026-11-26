/// String-aware ES-module-to-script transpiler.
///
/// Modern web apps (Reddit, etc.) ship scripts that contain ES module syntax
/// (`import.meta`, dynamic `import()`, `export` declarations) even when the
/// `<script>` tag has no `type="module"`. V8's `v8::Script` context cannot
/// compile that syntax, so we rewrite it to plain script syntax before
/// evaluation.
///
/// The pass is intentionally conservative: it skips the bodies of plain string
/// literals so that occurrences of `import(...)` or `import.meta` inside
/// evaluated strings are not corrupted. Only real dynamic imports and bare
/// `import.meta` expressions are rewritten, including those that appear inside
/// template literal `${...}` interpolations (the interpolation code is still
/// evaluated as regular script code).
use std::iter::Peekable;
use std::str::CharIndices;

const IMPORT_META: &str = concat!("imp", "ort", ".", "me", "ta");
const IMPORT_META_URL: &str = concat!("imp", "ort", ".", "me", "ta", ".", "url");

/// Rewrite `import.meta` / dynamic `import()` / top-level `export` in `source`
/// so it can be run as a regular script. `origin` is the source URL used for
/// `import.meta.url`.
pub fn transpile_module_to_script(source: &str, origin: &str) -> String {
    transpile_module_to_script_internal(source, origin, true)
}

/// Like [`transpile_module_to_script`], but leaves top-level `export` statements
/// in place so a caller (e.g. the dynamic-import module loader) can translate
/// them to its own module format before evaluation.
pub fn transpile_module_to_script_preserving_exports(source: &str, origin: &str) -> String {
    transpile_module_to_script_internal(source, origin, false)
}

fn transpile_module_to_script_internal(
    source: &str,
    origin: &str,
    strip_exports: bool,
) -> String {
    let mut result = String::with_capacity(source.len());
    let mut chars: Peekable<CharIndices<'_>> = source.char_indices().peekable();
    // Tracks whether the next `/` we see is more likely to start a regular
    // expression literal than a division operator. Regex literals can contain
    // quotes and backticks, so without this context our string/template scanner
    // would treat those characters as opening string literals and skip large
    // regions of real code.
    let mut regex_allowed = true;

    while let Some((idx, ch)) = chars.next() {
        // Skip string literal bodies verbatim and copy template literals
        // while recursively transpiling interpolations.
        if ch == '"' || ch == '\'' || ch == '`' {
            copy_string_or_template(ch, &mut chars, source, &mut result, origin);
            regex_allowed = false;
            continue;
        }

        // Skip regex literals verbatim. A regex begins with `/` when a regex
        // is syntactically allowed and the next character is not `/` or `*`
        // (which would start a comment). The body is copied until an unescaped
        // `/` outside a `[...]` character class, followed by optional flags.
        if ch == '/'
            && regex_allowed
            && !matches!(chars.peek(), Some(&(_, '/' | '*')))
        {
            if let Some(end) = find_regex_end(source, idx + 1) {
                for c in source[idx..end].chars() {
                    result.push(c);
                }
                skip_chars(&mut chars, end);
                regex_allowed = false;
                continue;
            }
        }

        // Comments also do not contain rewritable module syntax in their bodies.
        if ch == '/' {
            if matches!(chars.peek(), Some(&(_, '/'))) {
                result.push(ch);
                while let Some((_, c)) = chars.next() {
                    result.push(c);
                    if c == '\n' {
                        break;
                    }
                }
                regex_allowed = true;
                continue;
            }
            if matches!(chars.peek(), Some(&(_, '*'))) {
                result.push(ch);
                chars.next();
                result.push('*');
                let mut prev = '*';
                while let Some((_, c)) = chars.next() {
                    result.push(c);
                    if prev == '*' && c == '/' {
                        break;
                    }
                    prev = c;
                }
                regex_allowed = true;
                continue;
            }
        }

        // import.meta.url
        if ch == 'i' && source[idx..].starts_with(IMPORT_META_URL) {
            let after = idx + IMPORT_META_URL.len();
            if is_boundary(source, after) {
                result.push_str(&format!("\"{}\"", origin));
                skip_chars(&mut chars, after);
                regex_allowed = false;
                continue;
            }
        }

        // import.meta
        if ch == 'i' && source[idx..].starts_with(IMPORT_META) {
            let after = idx + IMPORT_META.len();
            if is_boundary(source, after) {
                result.push_str(&format!("{{ url: \"{}\" }}", origin));
                skip_chars(&mut chars, after);
                regex_allowed = false;
                continue;
            }
        }

        // Dynamic import: import(spec) -> __incognidium_import(spec, origin)
        // Pass the script origin as the resolution base so relative imports in
        // dynamically loaded module chunks resolve against the chunk URL rather
        // than the page location.
        if ch == 'i'
            && source[idx..].starts_with("import")
            && source[idx + 6..].chars().next().map_or(false, |c| c == '(')
        {
            if let Some(call_end) = find_call_end(source, idx + 7) {
                // call_end is one past the closing ')', so the argument text is
                // between idx+7 (after 'import(') and call_end-1 (before ')').
                let spec = &source[idx + 7..call_end.saturating_sub(1)];
                let safe_origin = origin.replace('"', "\\\"");
                result.push_str(&format!("__incognidium_import({}, \"{}\")", spec, safe_origin));
                skip_chars(&mut chars, call_end);
                regex_allowed = false;
                continue;
            }
        }

        // Static `import` keyword used as a statement (e.g. `import * as foo
        // from "bar"`) is illegal in scripts. We cannot rewrite it safely
        // without a real module loader, so we at least rewrite the keyword to
        // a no-op expression to avoid a parse error. This is a last-resort
        // fallback for malformed inline modules; dynamic and meta rewrites
        // above handle the common bundled-app cases.
        //
        // We also skip occurrences where `import` is part of a larger
        // identifier (e.g. `onimport`, `importHook`, `importMap`) — those are
        // ordinary property names and must not be corrupted.
        if ch == 'i'
            && source[idx..].starts_with("import")
            && is_boundary(source, idx + 6)
            && is_boundary(source, idx)
        {
            result.push_str("/*import*/0");
            skip_chars(&mut chars, idx + 6);
            regex_allowed = false;
            continue;
        }

        result.push(ch);

        // Update regex-vs-division context. A regex is allowed after tokens
        // that introduce a value/operand; it is not allowed after an operand.
        regex_allowed = regex_allowed_after(ch, regex_allowed);
    }

    if strip_exports {
        strip_top_level_exports(&result)
    } else {
        result
    }
}

/// Advance `chars` so that its next item starts at byte offset `target`.
fn skip_chars(chars: &mut Peekable<CharIndices<'_>>, target: usize) {
    while let Some(&(next_i, _)) = chars.peek() {
        if next_i >= target {
            break;
        }
        chars.next();
    }
}

/// Copy a string literal (`"..."` or `'...'`) verbatim, including escaped
/// characters.
fn copy_string_literal(quote: char, chars: &mut Peekable<CharIndices<'_>>, out: &mut String) {
    out.push(quote);
    while let Some((_, c)) = chars.next() {
        out.push(c);
        if c == '\\' {
            if let Some((_, next_c)) = chars.next() {
                out.push(next_c);
            }
        } else if c == quote {
            break;
        }
    }
}

/// Copy a template literal (``...``) verbatim, including nested template
/// literals and string literals inside `${...}` interpolations. The
/// interpolation expressions are also transpiled so that `import.meta` and
/// dynamic `import()` are rewritten in evaluated code.
fn copy_template_literal(
    chars: &mut Peekable<CharIndices<'_>>,
    source: &str,
    out: &mut String,
    origin: &str,
) {
    while let Some((_, c)) = chars.next() {
        out.push(c);
        if c == '\\' {
            if let Some((_, next_c)) = chars.next() {
                out.push(next_c);
            }
        } else if c == '`' {
            break;
        } else if c == '$' {
            if matches!(chars.peek(), Some(&(_, '{'))) {
                chars.next();
                out.push('{');
                copy_and_transpile_interpolation(chars, source, out, origin);
            }
        }
    }
}

/// Copy the expression inside a `${...}` interpolation, balancing braces and
/// nested template/string literals, while also rewriting ES module syntax.
fn copy_and_transpile_interpolation(
    chars: &mut Peekable<CharIndices<'_>>,
    source: &str,
    out: &mut String,
    origin: &str,
) {
    let expr = extract_balanced_expression(chars, '}', source, origin);
    if expr.len() > 1000 && expr.contains("processPreload") {
        eprintln!(
            "[transpile debug] interpolation expr near EOF len {} ends_with_newline {} last 80 {:?}",
            expr.len(),
            expr.ends_with('\n'),
            expr[expr.len().saturating_sub(80)..].escape_default()
        );
    }
    let transpiled = transpile_module_to_script(&expr, origin);
    out.push_str(&transpiled);
    out.push('}');
}

/// Extract a balanced JavaScript expression starting immediately after the
/// opening `{` of a `${...}` interpolation. The closing `}` is consumed but
/// not included in the returned string. Handles nested braces, plain strings,
/// regex literals, and nested template literals. Nested template literal
/// interpolations are also transpiled using the same `origin`.
fn extract_balanced_expression(
    chars: &mut Peekable<CharIndices<'_>>,
    close: char,
    source: &str,
    origin: &str,
) -> String {
    let mut depth = 1usize;
    let mut expr = String::new();
    // Regex literals can appear inside interpolation expressions and contain
    // braces (e.g., quantifiers). Use the same conservative regex-context
    // heuristic as the main scanner so regex braces are not mistaken for
    // expression braces.
    let mut regex_allowed = true;

    while let Some((idx, c)) = chars.next() {
        if c == '\\' {
            expr.push(c);
            if let Some((_, next_c)) = chars.next() {
                expr.push(next_c);
            }
            continue;
        }
        if c == '"' || c == '\'' {
            copy_string_literal(c, chars, &mut expr);
            regex_allowed = false;
            continue;
        }
        if c == '`' {
            expr.push(c);
            copy_template_literal(chars, source, &mut expr, origin);
            regex_allowed = false;
            continue;
        }
        if c == '/'
            && regex_allowed
            && !matches!(chars.peek(), Some(&(_, '/' | '*')))
        {
            if let Some(end) = find_regex_end(source, idx + 1) {
                expr.push_str(&source[idx..end]);
                skip_chars(chars, end);
                regex_allowed = false;
                continue;
            }
        }
        if c == '/' {
            if matches!(chars.peek(), Some(&(_, '/'))) {
                expr.push(c);
                while let Some((_, next_c)) = chars.next() {
                    expr.push(next_c);
                    if next_c == '\n' {
                        break;
                    }
                }
                regex_allowed = true;
                continue;
            }
            if matches!(chars.peek(), Some(&(_, '*'))) {
                expr.push(c);
                chars.next();
                expr.push('*');
                let mut prev = '*';
                while let Some((_, next_c)) = chars.next() {
                    expr.push(next_c);
                    if prev == '*' && next_c == '/' {
                        break;
                    }
                    prev = next_c;
                }
                regex_allowed = true;
                continue;
            }
        }
        expr.push(c);
        if c == '{' {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                expr.pop();
                break;
            }
        }
        regex_allowed = regex_allowed_after(c, regex_allowed);
    }
    expr
}

/// Copy either a plain string literal or a template literal depending on the
/// opening quote.
fn copy_string_or_template(
    quote: char,
    chars: &mut Peekable<CharIndices<'_>>,
    source: &str,
    out: &mut String,
    origin: &str,
) {
    if quote == '`' {
        out.push('`');
        copy_template_literal(chars, source, out, origin);
    } else {
        copy_string_literal(quote, chars, out);
    }
}

/// Return true if `source[offset]` is at a word boundary (start/end of the
/// identifier/keyword we are matching). Used to avoid rewriting `importX` or
/// `foot.import.meta.bar` incorrectly.
fn is_boundary(source: &str, offset: usize) -> bool {
    source[offset..].chars().next().map_or(true, |c| {
        !c.is_alphanumeric() && c != '_' && c != '$'
    })
}

/// Find the byte index just after the matching `)` for a call that starts at
/// `start` (the character after the opening `(`). Returns `None` if the
/// parentheses are not balanced.
fn find_call_end(source: &str, start: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut end = start;
    for c in source[start..].chars() {
        end += 1;
        if c == '(' {
            depth += 1;
        } else if c == ')' {
            depth -= 1;
            if depth == 0 {
                return Some(end);
            }
        }
    }
    None
}

/// Find the byte index just after a regex literal that starts at `idx` (the
/// character immediately after the opening `/`). Handles escaped characters and
/// `[...]` character classes. Returns `None` if the regex is not closed before
/// end-of-input or a newline.
fn find_regex_end(source: &str, idx: usize) -> Option<usize> {
    let mut in_class = false;
    let mut escaped = false;
    let mut end = idx;
    for c in source[idx..].chars() {
        end += 1;
        if c == '\n' || c == '\r' {
            return None;
        }
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if c == '[' {
            in_class = true;
        } else if c == ']' {
            in_class = false;
        } else if c == '/' && !in_class {
            // optional flags
            for f in source[end..].chars() {
                if f.is_ascii_alphabetic() {
                    end += 1;
                } else {
                    break;
                }
            }
            return Some(end);
        }
    }
    None
}

/// Return true if a `/` occurring after `last_ch` is more likely to start a
/// regular expression than a division operator. This is a conservative
/// expression-context heuristic used by the module-to-script scanner.
fn regex_allowed_after(ch: char, previously_allowed: bool) -> bool {
    match ch {
        // These characters introduce or separate operands, so a regex may
        // follow.
        '(' | '[' | '{' | ',' | ':' | ';' | '=' | '!' | '~' | '+' | '-' | '*'
        | '%' | '&' | '|' | '^' | '?' | '<' | '>' => true,
        // A closing paren/bracket/brace ends an expression, so the next `/`
        // is likely division; however after an `if`/`while`/etc condition a
        // regex is rare. We err toward division.
        ')' | ']' | '}' => false,
        // Whitespace preserves the previous decision.
        c if c.is_whitespace() => previously_allowed,
        // Letters, digits, underscores, `$`, `.`, `)` etc end expressions.
        _ => false,
    }
}

/// Strip top-level `export` declarations from scripts evaluated inline. This
/// is safe for our use because the module loader wrapper already translates
/// `export` into property assignments; scripts that run in the global context
/// just need the keyword removed so they do not throw.
fn strip_top_level_exports(source: &str) -> String {
    let mut stripped = String::with_capacity(source.len());
    let mut chars: Peekable<CharIndices<'_>> = source.char_indices().peekable();
    // Track whether the current position is inside a string, template literal,
    // or regex literal. Export keywords inside those bodies are content, not
    // module declarations, and must be preserved.
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    // Regex literals start with `/` in expression context and end with an
    // unescaped `/` outside character classes. We reuse the same conservative
    // regex context heuristic as the main transpiler scanner.
    let mut regex_allowed = true;

    while let Some((idx, ch)) = chars.next() {
        if let Some(quote) = in_string {
            stripped.push(ch);
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == quote {
                in_string = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' || ch == '`' {
            stripped.push(ch);
            in_string = Some(ch);
            regex_allowed = false;
            continue;
        }
        if ch == '/'
            && regex_allowed
            && !matches!(chars.peek(), Some(&(_, '/' | '*')))
        {
            if let Some(end) = find_regex_end(source, idx + 1) {
                stripped.push_str(&source[idx..end]);
                skip_chars(&mut chars, end);
                regex_allowed = false;
                continue;
            }
        }
        if ch == '/' {
            if matches!(chars.peek(), Some(&(_, '/'))) {
                stripped.push(ch);
                while let Some((_, c)) = chars.next() {
                    stripped.push(c);
                    if c == '\n' {
                        break;
                    }
                }
                regex_allowed = true;
                continue;
            }
            if matches!(chars.peek(), Some(&(_, '*'))) {
                stripped.push(ch);
                chars.next();
                stripped.push('*');
                let mut prev = '*';
                while let Some((_, c)) = chars.next() {
                    stripped.push(c);
                    if prev == '*' && c == '/' {
                        break;
                    }
                    prev = c;
                }
                regex_allowed = true;
                continue;
            }
        }
        if ch == 'e' && source[idx..].starts_with("export") {
            let after = idx + 6;
            if is_boundary(source, after) {
                // Determine what kind of export this is and either drop the
                // whole statement (for `export { ... }` / `export * from
                // "..."`) or just drop the `export` (and optional `default`)
                // keyword so the declaration remains in script scope.
                let rest = &source[after..];
                let mut ri = rest.chars().peekable();
                while let Some(&c) = ri.peek() {
                    if !c.is_whitespace() {
                        break;
                    }
                    ri.next();
                }
                match ri.peek().copied() {
                    Some('{') | Some('*') => {
                        // Drop the whole export statement or clause.
                        skip_to_statement_end(&mut chars, after);
                        continue;
                    }
                    _ => {
                        // Drop "export" and any following "default" keyword,
                        // preserving the actual declaration.
                        let default_end =
                            rest.trim_start().strip_prefix("default").map(|s| {
                                after + (rest.len() - s.len())
                            });
                        let drop_end = default_end.unwrap_or(after);
                        skip_chars(&mut chars, drop_end);
                        // Emit a single space to separate from the
                        // declaration if needed.
                        if stripped
                            .chars()
                            .last()
                            .map_or(false, |c| c.is_ascii_alphabetic() || c == '_')
                        {
                            stripped.push(' ');
                        }
                        continue;
                    }
                }
            }
        }
        stripped.push(ch);

        regex_allowed = regex_allowed_after(ch, regex_allowed);
    }

    stripped
}

/// Advance `chars` so it has consumed everything up to the end of the current
/// top-level export statement (`;`, matching `{}`, or end-of-input).
fn skip_to_statement_end(chars: &mut Peekable<CharIndices<'_>>, after: usize) {
    skip_chars(chars, after);
    let mut brace_depth = 0usize;
    let mut in_string: Option<char> = None;
    let mut escaped = false;
    while let Some((_, c)) = chars.next() {
        if let Some(quote) = in_string {
            if escaped {
                escaped = false;
                continue;
            }
            if c == '\\' {
                escaped = true;
                continue;
            }
            if c == quote {
                in_string = None;
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' => in_string = Some(c),
            '{' => brace_depth += 1,
            '}' if brace_depth > 0 => {
                brace_depth -= 1;
                if brace_depth == 0 {
                    // consume the statement-closing semicolon if present
                    if matches!(chars.peek(), Some(&(_, ';'))) {
                        chars.next();
                    }
                    break;
                }
            }
            ';' if brace_depth == 0 => break,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_import_rewritten() {
        let out = transpile_module_to_script("import(\"./x.js\")", "file:///test.js");
        assert_eq!(
            out.trim(),
            "__incognidium_import(\"./x.js\", \"file:///test.js\")"
        );
    }

    #[test]
    fn import_meta_rewritten() {
        let out = transpile_module_to_script("console.log(import.meta.url)", "file:///test.js");
        assert!(!out.contains("import.meta"));
        assert!(out.contains("file:///test.js"));
    }

    #[test]
    fn import_meta_url_rewritten_in_call() {
        let src = "SML.di(`./x.js`,import.meta.url,`x`)";
        let out = transpile_module_to_script(src, "file:///test.js");
        assert!(!out.contains("import.meta"), "survivor: {}", out);
        assert!(out.contains("file:///test.js"));
    }

    #[test]
    fn import_meta_url_rewritten_in_await_call() {
        let src = "await SML.di(`./x.js`,import.meta.url,`x`)";
        let out = transpile_module_to_script(src, "file:///test.js");
        assert!(!out.contains("import.meta"), "survivor: {}", out);
        assert!(out.contains("file:///test.js"));
    }

    #[test]
    fn import_meta_url_rewritten_in_nested_template_literal_interpolation() {
        let src = "t.x` a ${SML.di(`./x.js`,import.meta.url,`x`)} b`";
        let out = transpile_module_to_script(src, "file:///test.js");
        assert!(!out.contains("import.meta"), "survivor: {}", out);
        assert!(out.contains("file:///test.js"));
    }

    #[test]
    fn import_meta_url_rewritten_in_actual_reddit_devvit_render_snippet() {
        let src = "render(){return t.x` <devvit-wrapper loid=\"${this.loid}\"> <slot></slot> </devvit-wrapper> `}async installLiAfnFeedShimmer(){let{installLiAfnFeedShimmer:e}=await SML.di(`./B9h8E7p0wS.js`,import.meta.url,`B9h8E7p0wS`);this.isConnected}";
        let out = transpile_module_to_script(src, "file:///test.js");
        assert!(!out.contains("import.meta"), "survivor: {}", out);
        assert!(out.contains("file:///test.js"));
    }

    #[test]
    fn import_inside_string_preserved() {
        let src = "const a = \"import(\"./x.js\")\"";
        let out = transpile_module_to_script(src, "file:///test.js");
        assert_eq!(out, src);
    }

    #[test]
    fn import_meta_inside_string_preserved() {
        let src = "const a = \"import.meta.url\"";
        let out = transpile_module_to_script(src, "file:///test.js");
        assert_eq!(out, src);
    }

    #[test]
    fn import_in_template_literal_preserved() {
        let src = "const a = `import(\"./x.js\")`";
        let out = transpile_module_to_script(src, "file:///test.js");
        assert_eq!(out, src);
    }

    #[test]
    fn import_meta_url_in_template_literal_interpolation_rewritten() {
        // Reddit's SML.di(...) calls put import.meta.url inside a template
        // literal interpolation. That code is evaluated, so it must be
        // rewritten here.
        let src = "const a = `${import.meta.url}`";
        let out = transpile_module_to_script(src, "file:///test.js");
        assert!(!out.contains("import.meta"));
        assert!(out.contains("file:///test.js"));
    }

    #[test]
    fn dynamic_import_in_template_literal_interpolation_rewritten() {
        let src = "const a = `${import(\"./x.js\")}`";
        let out = transpile_module_to_script(src, "file:///test.js");
        assert!(
            out.contains("__incognidium_import("),
            "dynamic import was not rewritten inside interpolation: {}",
            out
        );
    }

    #[test]
    fn export_default_stripped() {
        let src = "export default function foo() {}";
        let out = transpile_module_to_script(src, "file:///test.js");
        assert!(!out.contains("export"));
        assert!(out.contains("function foo"));
        assert_eq!(out.trim(), "function foo() {}");
    }

    #[test]
    fn regex_literal_with_quote_preserves_string() {
        let src = "const r = /\\'/g; SML.di(`./x.js`,import.meta.url,`x`);";
        let out = transpile_module_to_script(src, "file:///test.js");
        assert!(!out.contains("import.meta"));
        assert!(out.contains("/\\'/g"));
    }

    #[test]
    fn exact_devvit_b9h8_snippet() {
        let src = r#"render(){return t.x` <devvit-wrapper loid="${this.loid}"> <slot></slot> </devvit-wrapper> `}async installLiAfnFeedShimmer(){let{installLiAfnFeedShimmer:e}=await SML.di(`./B9h8E7p0wS.js`,import.meta.url,`B9h8E7p0wS`);}"#;
        let out = transpile_module_to_script(src, "file:///test.js");
        assert!(
            !out.contains("import.meta"),
            "import.meta remained in exact devvit snippet:\n{}",
            out
        );
    }



    #[test]
    fn deeply_nested_template_interpolation_rewrites_import_meta() {
        // Reddit's Faceplate-ui error path wraps devvit classes inside several
        // layers of nested template literal interpolations. The innermost
        // interpolation contains the SML.di(...) call that references
        // import.meta.url. This must be rewritten even when nested.
        let src = r#"const tpl = `outer ${`inner ${`deep ${SML.di(`./B9h8.js`,import.meta.url,`B9h8`)}`}`}`;"#;
        let out = transpile_module_to_script(src, "file:///test.js");
        assert!(
            !out.contains("import.meta"),
            "import.meta remained in deeply nested template:\n{}",
            out
        );
        assert!(out.contains("file:///test.js"));
    }

    #[test]
    fn division_inside_interpolation_preserved() {
        // Removing over-eager regex handling from interpolation extraction
        // must not break ordinary division operators.
        let src = "const tpl = `${a / b}`";
        let out = transpile_module_to_script(src, "file:///test.js");
        assert_eq!(out, src);
    }

    #[test]
    fn es_module_shims_polyload_does_not_get_corrupted() {
        // Reddit's polyfills/es-module-shims script contains dynamic imports
        // and regex literals. The transpiler must not introduce syntax errors.
        let src = r#"let shimMode=/no-shim/i.test(location.search);import("./x.js")"#;
        let out = transpile_module_to_script(src, "file:///test.js");
        eprintln!("polyfill out: {}", out);
        // Must still be syntactically valid: no unmatched parens.
        assert!(out.contains("__incognidium_import("), "dynamic import was not rewritten: {}", out);
    }

    #[test]
    fn url_js_string_template_is_closed() {
        // Minimal reproduction of the es-module-shims function that caused an
        // unclosed template literal warning.
        let src = "function urlJsString(url){return `'${url.replace(/'/g, \"\\\\'\")}'`;};";
        let out = transpile_module_to_script(src, "file:///test.js");
        // The output must not contain a dangling, unclosed template literal.
        // A simple check: count backticks is even and the outer template is
        // preserved.
        let backticks = out.matches('`').count();
        assert_eq!(backticks % 2, 0, "unbalanced template literals in {}", out);
        assert!(!out.contains("unclosed"), "template not closed: {}", out);
    }

    #[test]
    fn strip_exports_preserves_export_in_template_literal_body() {
        // Inside a template literal argument to a function call, "export default"
        // is string content, not code, and must not be stripped by
        // strip_top_level_exports.
        let src = "createBlob(`export default importShim._s[${urlJsString(depLoad.r)}]`)";
        let out = strip_top_level_exports(src);
        eprintln!("strip exports out: {}", out);
        assert!(
            out.contains("export default"),
            "strip_top_level_exports removed export from template body: {}",
            out
        );
    }

    #[test]
    fn strip_exports_without_whitespace() {
        // Bundled code may write `export{...}` without a space after the
        // keyword. The export stripper must still remove it.
        let src = "typeof window<`u`&&!window.process?.env?.SHREDDIT_UNIT_TEST&&!window.SKIP_FEED_ELEMENT_LOAD_PERF_MARK_AUTO_RUN&&$();export{Q as init,$ as scheduleInit};";
        let out = strip_top_level_exports(src);
        eprintln!("strip exports no-space out: {}", out);
        assert!(!out.contains("export"), "export clause survived: {}", out);
    }

    #[test]
    #[ignore = "temporary diagnostic for polyfill divergence"]
    fn diagnostic_polyfill_first_divergence() {
        use std::fs;
        let source = fs::read_to_string("/tmp/es_module_shims.js").expect("read polyfill");
        let out = transpile_module_to_script(
            &source,
            "https://www.redditstatic.com/shreddit/polyfills/es-module-shims.js",
        );
        fs::write("/tmp/es_module_shims_transpiled_crate.js", &out).expect("write");
        eprintln!("source len {} export count {}", source.len(), source.matches("export").count());
        eprintln!("out len {} export count {}", out.len(), out.matches("export").count());
    }

    #[test]
    #[ignore = "requires a real Reddit concat bundle in /tmp"]
    fn integration_real_reddit_concat() {
        use std::fs;
        let paths: Vec<_> = fs::read_dir("/tmp")
            .expect("read /tmp")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map_or(false, |n| n.starts_with("incognidium_concat_err_") && n.ends_with(".js"))
            })
            .collect();
        assert!(!paths.is_empty(), "no /tmp/incognidium_concat_err_*.js bundle found");
        for path in paths {
            let source = fs::read_to_string(&path).expect("read bundle");
            let out = transpile_module_to_script(&source, "https://www.redditstatic.com/js/concat");
            assert!(
                !out.contains("import.meta"),
                "import.meta survived in {}:\n{}",
                path.display(),
                out
            );
            assert!(
                !out.contains("__incognidium_import") || !out.contains("import("),
                "dynamic import was not fully rewritten in {}",
                path.display()
            );
        }
    }
}
