//! Shared logic for incognidium-shell and its binaries.

#[cfg(feature = "v8-engine")]
pub mod v8_dom;

#[cfg(feature = "boa-engine")]
pub mod boa_dom;

use std::collections::HashMap;
use std::path::Path;

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::ImageEncoder;
use tiny_skia::Pixmap;

use incognidium_dom::{Document, NodeData};
use incognidium_html::parse_html;
use incognidium_net::{fetch_url, resolve_url};
use incognidium_paint::ImageData;
use incognidium_style::{CalcExpression, CalcValue};
use incognidium_style::{CssColor, Display, SizeValue, StyleMap};

/// A script to execute, with its source code and a label for error messages.
pub struct ScriptEntry {
    pub source: String,
    pub origin: String,
}

/// Domains/pages where JS execution reliably crashes the engine or strips all
/// useful server-rendered content. Returning an empty script list for these URLs
/// lets the renderer fall back to the static DOM, which is still useful for QA.
fn should_disable_js_for_url(base_url: &str) -> bool {
    let lower = base_url.to_ascii_lowercase();
    lower.contains("scholar.google.com")
}

/// Look for a `<meta http-equiv="refresh" content="...;url=...">` directive
/// in the raw HTML body. This is the standard server-side/noscript redirect
/// fallback used by sites such as ruby-lang.org, whose meta tag sits inside a
/// `<noscript>` block that the HTML parser treats as raw text. Returns the
/// resolved target URL if one refresh directive is found and it points to a
/// different URL.
pub fn meta_refresh_target(html: &str, base_url: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut search_from = 0usize;
    let mut found: Option<String> = None;

    while let Some(tag_start) = lower[search_from..].find("<meta") {
        let abs_start = search_from + tag_start;
        let tag_end = lower[abs_start..]
            .find('>')
            .map(|i| abs_start + i + 1)
            .unwrap_or(lower.len());
        let tag = &lower[abs_start..tag_end];

        if tag.contains("http-equiv")
            && tag.contains("refresh")
            && !tag.contains("http-equiv=\"expires\"")
        {
            // Find the original-case content attribute in the same tag.
            let original_tag = &html[abs_start..tag_end.min(html.len())];
            if let Some(content_start) = original_tag.to_ascii_lowercase().find("content=") {
                let after = &original_tag[content_start + 8..];
                // content="..." or content='...'
                let quote = after.chars().next()?;
                let end = 1 + after[1..].find(quote)?;
                let content = &after[1..end];
                let lower_content = content.to_ascii_lowercase();
                if let Some(idx) = lower_content.find("url=") {
                    let url_part = content[idx + 4..]
                        .trim()
                        .trim_matches(|c: char| c == '"' || c == '\'');
                    if !url_part.is_empty() {
                        if let Ok(resolved) = resolve_url(base_url, url_part) {
                            if resolved != base_url {
                                if found.is_some() {
                                    // Multiple conflicting refresh directives:
                                    // don't guess.
                                    return None;
                                }
                                found = Some(resolved);
                            }
                        }
                    }
                }
            }
        }
        search_from = tag_end;
    }

    found
}

/// Collect scripts from the DOM in document order, handling both inline and
/// external `<script src="...">` tags.
///
/// - Skips `type="module"` scripts (ES modules not supported)
/// - Limits external script fetches to 20
/// - Maintains document order for execution
pub fn collect_scripts(doc: &incognidium_dom::Document, base_url: &str) -> Vec<ScriptEntry> {
    if should_disable_js_for_url(base_url) {
        return Vec::new();
    }
    const MAX_EXTERNAL_SCRIPTS: usize = 20;
    // Domains that provide ads, tracking, or consent widgets. Skipping them
    // cuts network/JS overhead on heavy news/commerce sites without affecting
    // primary content.
    const BLOCKED_SCRIPT_HOSTS: [&str; 24] = [
        "google-analytics.com",
        "googletagmanager.com",
        "googletagservices.com",
        "googlesyndication.com",
        "googleadservices.com",
        "doubleclick.net",
        "doubleverify.com",
        "amazon-adsystem.com",
        "adsystem.amazon.com",
        "facebook.net",
        "connect.facebook.net",
        "platform.twitter.com",
        "twitter.com",
        "ads-twitter.com",
        "cookielaw.org",
        "onetrust.com",
        "newrelic.com",
        "js-agent.newrelic.com",
        "adsafeprotected.com",
        "moatads.com",
        "outbrain.com",
        "taboola.com",
        "scorecardresearch.com",
        "quantserve.com",
    ];

    fn host_matches_any(url: &str, hosts: &[&str]) -> bool {
        url.parse::<url::Url>()
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_lowercase()))
            .map(|h| hosts.iter().any(|blocked| h.ends_with(blocked)))
            .unwrap_or(false)
    }

    let mut scripts = Vec::new();
    let mut external_count = 0usize;

    for node in &doc.nodes {
        if let incognidium_dom::NodeData::Element(ref el) = node.data {
            if el.tag_name == "script" {
                // Skip non-executable script types
                if let Some(script_type) = el.get_attr("type") {
                    let st = script_type.to_lowercase();
                    if st == "module"
                        || st == "application/json"
                        || st == "application/ld+json"
                        || st == "text/template"
                        || st == "text/html"
                        || st == "importmap"
                        || st == "speculationrules"
                    {
                        continue;
                    }
                }

                if let Some(src) = el.get_attr("src") {
                    // External script
                    if external_count >= MAX_EXTERNAL_SCRIPTS {
                        continue;
                    }
                    let resolved = match resolve_url(base_url, src) {
                        Ok(u) => u,
                        Err(e) => {
                            eprintln!("Failed to resolve script URL {src}: {e}");
                            continue;
                        }
                    };
                    if host_matches_any(&resolved, &BLOCKED_SCRIPT_HOSTS) {
                        eprintln!("Skipping blocked script {resolved}");
                        external_count += 1;
                        continue;
                    }
                    match fetch_url(&resolved) {
                        Ok(resp) => {
                            if !resp.body.is_empty() {
                                scripts.push(ScriptEntry {
                                    source: resp.body,
                                    origin: resolved,
                                });
                            }
                            external_count += 1;
                        }
                        Err(e) => {
                            eprintln!("Failed to fetch script {resolved}: {e}");
                            external_count += 1;
                        }
                    }
                } else {
                    // Inline script
                    let mut text = String::new();
                    for &child_id in &node.children {
                        if let incognidium_dom::NodeData::Text(ref t) = doc.nodes[child_id].data {
                            text.push_str(&t.content);
                        }
                    }
                    if !text.is_empty() {
                        scripts.push(ScriptEntry {
                            source: text,
                            origin: format!("inline <script> in {}", base_url),
                        });
                    }
                }
            }
        }
    }
    scripts
}

/// Execute scripts using whichever JS engine is enabled at build time.
/// With `v8-engine` (default): fast, runs real framework bundles.
/// With `boa-engine`: pure Rust, no Google code, slower.
/// Env `INCOGNIDIUM_JS=off` skips JS entirely.
pub fn execute_scripts_on_doc(
    doc: incognidium_dom::Document,
    scripts: &[ScriptEntry],
    _image_cache: &mut HashMap<String, ImageData>,
) -> incognidium_dom::Document {
    if std::env::var("INCOGNIDIUM_JS").ok().as_deref() == Some("off") {
        return doc;
    }
    #[cfg(feature = "v8-engine")]
    {
        v8_dom::execute_scripts_v8(doc, scripts)
    }
    #[cfg(all(feature = "boa-engine", not(feature = "v8-engine")))]
    {
        return boa_dom::execute_scripts_boa(doc, scripts);
    }
    #[cfg(not(any(feature = "v8-engine", feature = "boa-engine")))]
    {
        let _ = scripts;
        doc
    }
}

/// Strip dark mode styles from CSS text.
///
/// Sites like Wikipedia ship both light and dark variable sets. The dark set
/// can arrive inside `prefers-color-scheme: dark` media queries or in plain
/// rules keyed off a night theme class (e.g. `html.skin-theme-clientpref-night`).
/// Because our renderer does not report a real color-scheme preference, those
/// blocks can end up matching and turning the page black. Removing them leaves
/// the light/default styles intact.
pub fn strip_dark_mode_media_queries(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut i = 0usize;
    let bytes = css.as_bytes();
    let len = bytes.len();

    fn is_night_selector(sel: &str) -> bool {
        let lower = sel.to_ascii_lowercase();
        lower.contains("skin-theme-clientpref-night")
            || lower.contains("-night") && lower.contains("theme")
    }

    while i < len {
        // Find next @media
        if let Some(at_pos) = css[i..].find("@media") {
            let at_pos = i + at_pos;
            out.push_str(&css[i..at_pos]);

            // Find the opening brace
            let after_at = at_pos + 6;
            let open = css[after_at..].find('{').map(|p| after_at + p);
            if let Some(open_pos) = open {
                let prelude = css[after_at..open_pos].to_ascii_lowercase();
                // Check if this media query is for dark color scheme
                let is_dark_media =
                    prelude.contains("prefers-color-scheme") && prelude.contains("dark");
                if is_dark_media {
                    // Skip this block by brace counting
                    let mut depth = 1usize;
                    let mut j = open_pos + 1;
                    while j < len && depth > 0 {
                        match bytes[j] {
                            b'{' => depth += 1,
                            b'}' => depth -= 1,
                            _ => {}
                        }
                        j += 1;
                    }
                    i = j;
                } else {
                    // Keep the block
                    out.push_str(&css[at_pos..=open_pos]);
                    i = open_pos + 1;
                }
            } else {
                // Malformed: no opening brace; keep rest
                out.push_str(&css[at_pos..]);
                break;
            }
        } else {
            out.push_str(&css[i..]);
            break;
        }
    }

    // Second pass: strip top-level rules whose selector is a night theme class.
    // This handles `html.skin-theme-clientpref-night { ... }` and similar.
    let mut final_out = String::with_capacity(out.len());
    let mut k = 0usize;
    while k < out.len() {
        // Find next top-level opening brace. We look for the first '{' after k
        // that is not inside a string/comment is sufficient for this stripper.
        if let Some(open_pos) = out[k..].find('{') {
            let open_pos = k + open_pos;
            // Walk back to find the start of the selector
            let sel_start = {
                let mut s = open_pos;
                while s > k {
                    if out.as_bytes()[s - 1] == b'}' || out.as_bytes()[s - 1] == b'{' {
                        break;
                    }
                    s -= 1;
                }
                s
            };
            let selector = &out[sel_start..open_pos];
            if is_night_selector(selector) {
                // Skip this rule
                let mut depth = 1usize;
                let mut j = open_pos + 1;
                while j < out.len() && depth > 0 {
                    match out.as_bytes()[j] {
                        b'{' => depth += 1,
                        b'}' => depth -= 1,
                        _ => {}
                    }
                    j += 1;
                }
                k = j;
            } else {
                final_out.push_str(&out[k..=open_pos]);
                k = open_pos + 1;
            }
        } else {
            final_out.push_str(&out[k..]);
            break;
        }
    }

    final_out
}

/// Trim excess items inside Brightspot list modules.
///
/// Brightspot pages (e.g. AP News) render the full list of articles in the
/// server HTML, then rely on JS/custom elements to hide everything past
/// `data-max-number-of-posts` and show a "Load more" button. Our headless
/// renderer does not run that behavior, so all items get laid out and inflate
/// the page height. This helper limits any list module that declares
/// `data-max-number-of-posts` to the number of posts it declares, matching the
/// visible state in a real browser.
///
/// Only children of `.PageList-items` with class `.PageList-items-item` are
/// trimmed; non-item siblings such as the load-more button are preserved.
pub fn trim_bsp_list_loadmore(doc: &mut Document) {
    fn find_items_container(
        doc: &Document,
        root: incognidium_dom::NodeId,
    ) -> Option<incognidium_dom::NodeId> {
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            let node = &doc.nodes[id];
            if let NodeData::Element(el) = &node.data {
                if el.classes().contains(&"PageList-items") {
                    return Some(id);
                }
            }
            stack.extend(node.children.iter().copied());
        }
        None
    }

    fn trim_node(doc: &mut Document, id: incognidium_dom::NodeId) {
        let maybe_max: Option<usize> = {
            let node = &doc.nodes[id];
            if let NodeData::Element(el) = &node.data {
                el.get_attr("data-max-number-of-posts")
                    .and_then(|s| s.parse().ok())
            } else {
                None
            }
        };

        if let Some(max_items) = maybe_max {
            if let Some(items_id) = find_items_container(doc, id) {
                let items_node = &doc.nodes[items_id];
                let mut kept = 0usize;
                let to_remove: Vec<incognidium_dom::NodeId> = items_node
                    .children
                    .iter()
                    .filter(|&&cid| {
                        if let NodeData::Element(el) = &doc.nodes[cid].data {
                            if el.classes().contains(&"PageList-items-item") {
                                kept += 1;
                                kept > max_items
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    })
                    .copied()
                    .collect();
                if !to_remove.is_empty() {
                    let set: std::collections::HashSet<incognidium_dom::NodeId> =
                        to_remove.iter().copied().collect();
                    let items_node = &mut doc.nodes[items_id];
                    items_node.children.retain(|cid| !set.contains(cid));
                }
            }
        }

        let child_ids: Vec<incognidium_dom::NodeId> = doc.nodes[id].children.clone();
        for cid in child_ids {
            trim_node(doc, cid);
        }
    }

    trim_node(doc, 0);
}

/// Check whether a node or any of its descendants contributes visible content.
///
/// Used by the placeholder trimmers to decide whether an element that looks
/// like a wrapper is actually empty (no text, no images/media, no form
/// controls, and no meaningful accessibility text).
fn has_visible_content(doc: &Document, id: incognidium_dom::NodeId) -> bool {
    let node = &doc.nodes[id];
    match &node.data {
        incognidium_dom::NodeData::Text(t) => !t.content.trim().is_empty(),
        incognidium_dom::NodeData::Element(el) => {
            if matches!(
                el.tag_name.as_str(),
                "img"
                    | "picture"
                    | "video"
                    | "audio"
                    | "svg"
                    | "canvas"
                    | "iframe"
                    | "object"
                    | "embed"
                    | "input"
                    | "textarea"
                    | "select"
                    | "button"
            ) {
                return true;
            }
            for attr in ["alt", "aria-label", "title", "placeholder"] {
                if let Some(v) = el.get_attr(attr) {
                    if !v.trim().is_empty() {
                        return true;
                    }
                }
            }
            node.children
                .iter()
                .any(|&cid| has_visible_content(doc, cid))
        }
        _ => false,
    }
}

/// Remove empty placeholder containers that real browsers hide or fill with ads.
///
/// Many news/commerce pages include ad slots, tracking widgets, and CMS
/// placeholder boxes (e.g. `markupbox`, `ad-slot`, `dfp-ad`, `adsbygoogle`,
/// `taboola`, `outbrain`) in the server HTML. Without the site's ad/tracking JS
/// these boxes have no visible content, but they still occupy CSS-generated
/// height (padding, min-height, margins). This helper drops any such subtree
/// that contains no real content: no text, no images, no media, no form controls,
/// and no meaningful accessibility text. Visible placeholders (e.g. a footer
/// logo inside a `markupbox`) are preserved.
///
/// It also removes subtrees marked `aria-hidden="true"` when they have no
/// visible content, which is common for off-screen/hidden ad slots.
pub fn remove_empty_placeholders(doc: &mut Document) {
    fn is_placeholder(el: &incognidium_dom::ElementData) -> bool {
        // Inline SVGs are visual replaced elements even when `aria-hidden`.
        // Removing them as "placeholders" strips logos and icons from the page.
        if el.tag_name == "svg" {
            return false;
        }
        let classes: std::collections::HashSet<&str> = el.classes().into_iter().collect();
        const PLACEHOLDER_CLASSES: [&str; 22] = [
            "markupbox",
            "ad",
            "ads",
            "ad-slot",
            "ad__placeholder",
            "ad-placeholder",
            "ad-container",
            "dfp-ad",
            "adsbygoogle",
            "taboola",
            "outbrain",
            "hidden",
            "d-none",
            "invisible",
            "sr-only",
            "usa-sr-only",
            "visually-hidden",
            "screen-reader-text",
            "skip-links",
            "skiplink",
            "skip-to-main",
            "wpds-c-iSKIAI",
        ];
        if classes.iter().any(|c| PLACEHOLDER_CLASSES.contains(c)) {
            return true;
        }
        if let Some(v) = el.get_attr("aria-hidden") {
            if v == "true" {
                return true;
            }
        }
        // NYTimes ad slots use data-testid="StandardAd" and render as empty
        // placeholders when the ad/tracking scripts are blocked.
        if let Some(v) = el.get_attr("data-testid") {
            if v == "StandardAd" {
                return true;
            }
        }
        false
    }

    let mut to_remove: Vec<incognidium_dom::NodeId> = Vec::new();
    let mut collapsed_ad_wrappers: std::collections::HashSet<incognidium_dom::NodeId> =
        std::collections::HashSet::new();

    for id in 0..doc.nodes.len() {
        if let incognidium_dom::NodeData::Element(el) = &doc.nodes[id].data {
            // Accessibility-only skip links contain visible text but should still be
            // removed because they are positioned off-screen and pollute extracted text.
            if is_placeholder(el) {
                to_remove.push(id);

                // NYTimes ad slots are wrapped in a full-bleed container
                // (`.css-1q58nbc`/`.css-ibybby`) with a large min-height and gray
                // background. Remove the placeholder contents but keep the shell
                // so it still renders as the thin gray band a real browser shows
                // when the ad is blocked.
                if el.get_attr("data-testid") == Some("StandardAd") {
                    if let Some(parent_id) = doc.nodes[id].parent {
                        if let NodeData::Element(parent_el) = &doc.nodes[parent_id].data {
                            if !collapsed_ad_wrappers.contains(&parent_id) {
                                collapsed_ad_wrappers.insert(parent_id);
                                // Mark every current child of the wrapper for
                                // removal so the wrapper ends up empty.
                                for &cid in &doc.nodes[parent_id].children {
                                    to_remove.push(cid);
                                }
                                if let NodeData::Element(parent_el_mut) =
                                    &mut doc.node_mut(parent_id).data
                                {
                                    parent_el_mut.attributes.insert(
                                        "data-incog-ad-collapsed".to_string(),
                                        "1".to_string(),
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if to_remove.is_empty() && collapsed_ad_wrappers.is_empty() {
        return;
    }

    let remove_set: std::collections::HashSet<incognidium_dom::NodeId> =
        to_remove.iter().copied().collect();

    for id in to_remove {
        if let Some(parent_id) = doc.nodes[id].parent {
            let parent = &mut doc.nodes[parent_id];
            parent.children.retain(|cid| !remove_set.contains(cid));
        }
    }

    if !collapsed_ad_wrappers.is_empty() {
        // Collapse the kept NYTimes ad wrappers to their padding/border only.
        // `!important` is required because the original min-height rules use
        // higher-specificity class selectors.
        if let Some(html_id) = doc.document_element() {
            let head_id = doc.nodes[html_id].children.iter().copied().find(|&id| {
                matches!(
                    &doc.nodes[id].data,
                    NodeData::Element(ref e) if e.tag_name == "head"
                )
            });
            if let Some(head_id) = head_id {
                let style_el = doc.add_node(
                    head_id,
                    NodeData::Element(incognidium_dom::ElementData::new("style")),
                );
                doc.add_node(
                    style_el,
                    NodeData::Text(incognidium_dom::TextData {
                        content: "[data-incog-ad-collapsed]{min-height:0 !important;padding-bottom:0 !important}".to_string(),
                    }),
                );
            }
        }
    }
}

/// NBC News multi-storyline packages contain article-card wrappers such as
/// `.headline-item-container` and `.headline-container-small`. Some of these
/// wrappers end up with no visible content because JS hides them or because
/// lazy-loaded media is not present. The empty wrapper still participates in
/// the column-flex `multi-item-container` layout and expands to thousands of
/// pixels of whitespace. Remove the empty wrappers so the rail collapses to
/// its real content.
pub fn trim_nbc_empty_headline_placeholders(doc: &mut Document, base_url: &str) {
    if !base_url.to_ascii_lowercase().contains("nbcnews.com") {
        return;
    }

    let mut to_remove: Vec<incognidium_dom::NodeId> = Vec::new();
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            if el.tag_name != "div" {
                continue;
            }
            let classes: std::collections::HashSet<&str> = el.classes().into_iter().collect();
            if !(classes.contains("headline-item-container")
                || classes.contains("headline-container-small"))
            {
                continue;
            }
            if !has_visible_content(doc, id) {
                to_remove.push(id);
            }
        }
    }

    if to_remove.is_empty() {
        return;
    }

    let remove_set: std::collections::HashSet<incognidium_dom::NodeId> =
        to_remove.iter().copied().collect();
    for id in to_remove {
        if let Some(parent_id) = doc.nodes[id].parent {
            doc.nodes[parent_id]
                .children
                .retain(|cid| !remove_set.contains(cid));
        }
    }
}

/// Trim horizontally-snapping carousels to their declared visible count.
///
/// Some sites (e.g. The Register) render large collections of article cards
/// inside `<ul class="scroll-container snap-container-x count_N">`. The CSS
/// is meant to show only `N` cards at a time and scroll the rest horizontally.
/// Our layout engine does not implement overflow scroll / snap, so every
/// `.scroll-item` gets laid out vertically, producing enormous link farms.
/// This helper keeps the first `N` `.scroll-item` children of each such
/// container and removes the rest, matching the visible state in a real
/// browser.
/// Trim WordPress "continue reading" excerpts on the Stratechery homepage.
///
/// The site server-renders the full text of paywalled posts inside
/// `.entry-content.is-style-continue-reading` blocks. The visible state in a
/// real browser keeps only the first few children (hero image, intro paragraph,
/// and a "Continue reading" CTA) and hides the rest with a CSS/JS truncation
/// pattern. Our engine does not implement `:has()` or the dynamic `max-height`
/// behaviour, so every full article gets laid out and the homepage becomes
/// ~75 kpx tall. This helper keeps the first 4 top-level element children of
/// each `.entry-content.is-style-continue-reading` on the Stratechery domain.
pub fn trim_stratechery_continue_reading(doc: &mut Document, base_url: &str) {
    let is_stratechery = base_url.to_ascii_lowercase().contains("stratechery.com");
    if !is_stratechery {
        return;
    }

    let mut removals: Vec<(incognidium_dom::NodeId, Vec<incognidium_dom::NodeId>)> = Vec::new();

    for id in 0..doc.nodes.len() {
        if let incognidium_dom::NodeData::Element(el) = &doc.nodes[id].data {
            let classes: std::collections::HashSet<&str> = el.classes().into_iter().collect();
            if !(el.tag_name == "div"
                && classes.contains("entry-content")
                && classes.contains("is-style-continue-reading"))
            {
                continue;
            }

            let children = doc.nodes[id].children.clone();
            let element_children: Vec<incognidium_dom::NodeId> = children
                .into_iter()
                .filter(|&cid| matches!(doc.nodes[cid].data, incognidium_dom::NodeData::Element(_)))
                .collect();

            const KEEP: usize = 4;
            if element_children.len() > KEEP {
                let to_remove: Vec<incognidium_dom::NodeId> =
                    element_children.into_iter().skip(KEEP).collect();
                if !to_remove.is_empty() {
                    removals.push((id, to_remove));
                }
            }
        }
    }

    for (parent_id, to_remove) in removals {
        let set: std::collections::HashSet<incognidium_dom::NodeId> =
            to_remove.iter().copied().collect();
        doc.nodes[parent_id]
            .children
            .retain(|cid| !set.contains(cid));
    }
}

/// AP News serves full PageList sections on its homepage; real browsers and
/// the site's own CSS only surface a handful of stories per list. Trimming
/// each `PageList-items` (or `PageList-trending-items`) container to the first
/// few items keeps the render representative without ballooning to 3× the QA
/// height.
pub fn trim_apnews_pagelist_items(doc: &mut Document, base_url: &str) {
    let is_apnews = base_url.to_ascii_lowercase().contains("apnews.com");
    if !is_apnews {
        return;
    }

    const KEEP: usize = 4;

    let mut parent_map: std::collections::HashMap<
        incognidium_dom::NodeId,
        incognidium_dom::NodeId,
    > = std::collections::HashMap::new();
    for id in 0..doc.nodes.len() {
        for &cid in &doc.nodes[id].children {
            parent_map.insert(cid, id);
        }
    }

    let mut removals: Vec<(incognidium_dom::NodeId, Vec<incognidium_dom::NodeId>)> = Vec::new();

    for id in 0..doc.nodes.len() {
        if let incognidium_dom::NodeData::Element(el) = &doc.nodes[id].data {
            let class_str = el.classes().join(" ");
            let is_item_container = class_str.contains("PageList")
                && class_str.contains("items")
                && !class_str.contains("items-item");
            if !is_item_container {
                continue;
            }

            let children = doc.nodes[id].children.clone();
            let mut kept = 0usize;
            let to_remove: Vec<incognidium_dom::NodeId> = children
                .iter()
                .filter(|&&cid| {
                    if let incognidium_dom::NodeData::Element(child_el) = &doc.nodes[cid].data {
                        let child_class = child_el.classes().join(" ");
                        if child_class.contains("PageList") && child_class.contains("items-item") {
                            kept += 1;
                            return kept > KEEP;
                        }
                    }
                    false
                })
                .copied()
                .collect();
            if !to_remove.is_empty() {
                removals.push((id, to_remove));
            }
        }
    }

    for (parent_id, to_remove) in removals {
        let set: std::collections::HashSet<incognidium_dom::NodeId> =
            to_remove.iter().copied().collect();
        doc.nodes[parent_id]
            .children
            .retain(|cid| !set.contains(cid));
    }
}

/// AP News renders its mobile/off-canvas hamburger menu server-side and toggles
/// it with JS/CSS transforms.  In the headless renderer the menu is positioned
/// off-screen but still contributes duplicate nav links to text extraction and
/// layout.  Remove it on desktop-width renders so the visible top nav is the only
/// nav that appears.
pub fn trim_apnews_hamburger(doc: &mut Document, base_url: &str) {
    if !base_url.to_ascii_lowercase().contains("apnews.com") {
        return;
    }

    let mut to_remove: Vec<incognidium_dom::NodeId> = Vec::new();
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            let class_str = el.classes().join(" ");
            if class_str.contains("HamburgerNavigation")
                || class_str.contains("Page-header-hamburger-menu-content")
            {
                to_remove.push(id);
            }
        }
    }
    if to_remove.is_empty() {
        return;
    }

    let set: std::collections::HashSet<incognidium_dom::NodeId> =
        to_remove.iter().copied().collect();
    for id in 0..doc.nodes.len() {
        doc.nodes[id].children.retain(|cid| !set.contains(cid));
    }
}

/// Fox News server-renders far more items than the visible surface: the
/// `Must-Watch Videos` playlist is a horizontal scrollable that expands vertically
/// in the headless renderer, and the right-rail `section-bucket-container` repeats
/// many topic sections with long article lists.  Trim those to a representative
/// subset so the render stays a usable QA screenshot instead of a 40 kpx tall page.
pub fn trim_foxnews_collections(doc: &mut Document, base_url: &str) {
    let is_foxnews = base_url.to_ascii_lowercase().contains("foxnews.com");
    if !is_foxnews {
        return;
    }

    let mut to_remove: Vec<incognidium_dom::NodeId> = Vec::new();
    let mut list_removals: Vec<(incognidium_dom::NodeId, Vec<incognidium_dom::NodeId>)> =
        Vec::new();

    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            let class_str = el.classes().join(" ");

            // The video-playlist cannot be rendered as a horizontal rail here and
            // blows up the page height, so drop the whole section.
            if class_str.contains("collection-video-playlist") {
                to_remove.push(id);
                continue;
            }

            // The desktop right-rail columns overflow the 1024px viewport and stack
            // vertically in the headless renderer, producing a 100kpx tall page.
            // They are not visible in the QA screenshot, so remove them entirely.
            if class_str.contains("section-bucket-container")
                || class_str.contains("region-content-sidebar-secondary")
                || class_str.contains("collection game-hub")
                || class_str.contains("collection-fox-nation")
                || class_str.contains("collection-features-faces")
            {
                to_remove.push(id);
                continue;
            }

            // Trim long article lists inside topic buckets and load-more sections.
            let is_article_list = class_str.contains("article-list");
            let is_video_items = class_str.contains("video-items");
            let is_load_more = class_str.contains("has-load-more");
            if !is_article_list && !is_video_items && !is_load_more {
                continue;
            }

            let children = doc.nodes[id].children.clone();
            let mut kept = 0usize;
            let keep = if is_video_items { 1 } else { 3 };
            let to_remove_children: Vec<incognidium_dom::NodeId> = children
                .iter()
                .filter(|&&cid| {
                    if let NodeData::Element(child_el) = &doc.nodes[cid].data {
                        let child_class = child_el.classes().join(" ");
                        if child_class.contains("article") || child_class.contains("list-container")
                        {
                            kept += 1;
                            return kept > keep;
                        }
                    }
                    false
                })
                .copied()
                .collect();
            if !to_remove_children.is_empty() {
                list_removals.push((id, to_remove_children));
            }
        }
    }

    if !to_remove.is_empty() {
        let set: std::collections::HashSet<incognidium_dom::NodeId> =
            to_remove.iter().copied().collect();
        for id in 0..doc.nodes.len() {
            doc.nodes[id].children.retain(|cid| !set.contains(cid));
        }
    }

    for (parent_id, to_remove_children) in list_removals {
        let set: std::collections::HashSet<incognidium_dom::NodeId> =
            to_remove_children.iter().copied().collect();
        doc.nodes[parent_id]
            .children
            .retain(|cid| !set.contains(cid));
    }
}

/// Metacritic uses horizontal `global-carousel_content-scrollable` rows. Without
/// support for `overflow-x: auto`, all cards stack vertically. Keep roughly one
/// row's worth of cards per carousel (desktop-columns from the inline style, or
/// a safe default).
pub fn trim_metacritic_carousel_items(doc: &mut Document, base_url: &str) {
    let is_metacritic = base_url.to_ascii_lowercase().contains("metacritic.com");
    if !is_metacritic {
        return;
    }

    let mut removals: Vec<(incognidium_dom::NodeId, Vec<incognidium_dom::NodeId>)> = Vec::new();

    for id in 0..doc.nodes.len() {
        if let incognidium_dom::NodeData::Element(el) = &doc.nodes[id].data {
            let class_str = el.classes().join(" ");
            if !class_str.contains("carousel_content-scrollable") {
                continue;
            }

            let keep: usize = el
                .get_attr("style")
                .and_then(|style| {
                    // Parse `--desktop-columns: N;` out of the inline style.
                    let needle = "--desktop-columns";
                    let start = style.find(needle)? + needle.len();
                    let rest = &style[start..];
                    let rest = rest.trim_start();
                    if !rest.starts_with(':') {
                        return None;
                    }
                    let rest = rest[1..].trim_start();
                    let end = rest
                        .find(|c: char| c == ';' || c.is_whitespace())
                        .unwrap_or(rest.len());
                    rest[..end].parse::<usize>().ok()
                })
                .map(|n| n.max(1))
                .unwrap_or(6);

            let children = doc.nodes[id].children.clone();
            let mut kept = 0usize;
            let to_remove: Vec<incognidium_dom::NodeId> = children
                .iter()
                .filter(|&&cid| {
                    if matches!(doc.nodes[cid].data, incognidium_dom::NodeData::Element(_)) {
                        kept += 1;
                        return kept > keep;
                    }
                    false
                })
                .copied()
                .collect();
            if !to_remove.is_empty() {
                removals.push((id, to_remove));
            }
        }
    }

    for (parent_id, to_remove) in removals {
        let set: std::collections::HashSet<incognidium_dom::NodeId> =
            to_remove.iter().copied().collect();
        doc.nodes[parent_id]
            .children
            .retain(|cid| !set.contains(cid));
    }
}

/// Kottke.org's homepage includes a long chronological list of `.post` entries.
/// Individual article pages only have one, so trimming only when we see many
/// posts keeps the homepage compact without hurting article views.
pub fn trim_kottke_posts(doc: &mut Document, base_url: &str) {
    let is_kottke = base_url.to_ascii_lowercase().contains("kottke.org");
    if !is_kottke {
        return;
    }

    const KEEP: usize = 20;

    let mut parent_map: std::collections::HashMap<
        incognidium_dom::NodeId,
        incognidium_dom::NodeId,
    > = std::collections::HashMap::new();
    for id in 0..doc.nodes.len() {
        for &cid in &doc.nodes[id].children {
            parent_map.insert(cid, id);
        }
    }

    let post_ids: Vec<incognidium_dom::NodeId> = (0..doc.nodes.len())
        .filter(|&id| {
            if let incognidium_dom::NodeData::Element(el) = &doc.nodes[id].data {
                el.tag_name == "div" && el.classes().contains(&"post")
            } else {
                false
            }
        })
        .collect();

    if post_ids.len() <= KEEP {
        return;
    }

    let mut removals: std::collections::HashMap<
        incognidium_dom::NodeId,
        Vec<incognidium_dom::NodeId>,
    > = std::collections::HashMap::new();
    for &post_id in post_ids.iter().skip(KEEP) {
        if let Some(&parent_id) = parent_map.get(&post_id) {
            removals.entry(parent_id).or_default().push(post_id);
        }
    }

    for (parent_id, to_remove) in removals {
        let set: std::collections::HashSet<incognidium_dom::NodeId> =
            to_remove.iter().copied().collect();
        doc.nodes[parent_id]
            .children
            .retain(|cid| !set.contains(cid));
    }
}

/// The Intercept's homepage server-renders every article card inside
/// `<main>` (hero, top stories, and many category showcase sections).
/// A real browser shows a much smaller initial set; trimming keeps the
/// homepage compact without affecting individual article pages, which
/// do not contain many `.content-card` elements.
pub fn trim_theintercept_cards(doc: &mut Document, base_url: &str) {
    let lower = base_url.to_ascii_lowercase();
    let is_intercept = lower.contains("theintercept.com") && !lower.contains("/202");
    if !is_intercept {
        return;
    }

    const KEEP: usize = 16;

    let mut parent_map: std::collections::HashMap<
        incognidium_dom::NodeId,
        incognidium_dom::NodeId,
    > = std::collections::HashMap::new();
    for id in 0..doc.nodes.len() {
        for &cid in &doc.nodes[id].children {
            parent_map.insert(cid, id);
        }
    }

    let card_ids: Vec<incognidium_dom::NodeId> = (0..doc.nodes.len())
        .filter(|&id| {
            if let incognidium_dom::NodeData::Element(el) = &doc.nodes[id].data {
                el.tag_name == "article" && el.classes().contains(&"content-card")
            } else {
                false
            }
        })
        .collect();

    if card_ids.len() <= KEEP {
        return;
    }

    let remove_set: std::collections::HashSet<incognidium_dom::NodeId> =
        card_ids.iter().skip(KEEP).copied().collect();

    // Remove excess cards from their immediate parents.
    let mut affected_parents: Vec<incognidium_dom::NodeId> = Vec::new();
    for &card_id in card_ids.iter().skip(KEEP) {
        if let Some(&parent_id) = parent_map.get(&card_id) {
            affected_parents.push(parent_id);
        }
    }
    affected_parents.sort_unstable();
    affected_parents.dedup();

    for &parent_id in &affected_parents {
        doc.nodes[parent_id]
            .children
            .retain(|cid| !remove_set.contains(cid));
    }

    // If a category showcase section no longer contains any content cards,
    // remove the whole section so its heading does not leave empty whitespace.
    for &parent_id in &affected_parents {
        let has_cards = doc.nodes[parent_id].children.iter().any(|&cid| {
            if let incognidium_dom::NodeData::Element(el) = &doc.nodes[cid].data {
                el.tag_name == "article" && el.classes().contains(&"content-card")
            } else {
                false
            }
        });
        if !has_cards {
            if let Some(&section_parent) = parent_map.get(&parent_id) {
                doc.nodes[section_parent]
                    .children
                    .retain(|cid| *cid != parent_id);
            }
        }
    }
}

/// Remove visible cookie / GDPR / consent banners that server-render before the
/// site's consent JS runs. These banners can consume the full viewport and push
/// real content far down the page, hurting both visual diff grades and text
/// extraction. The selector list is intentionally conservative: exact id/class
/// matches for known consent-management providers and generic banner names.
pub fn remove_consent_banners(doc: &mut Document) {
    const BANNER_IDS: [&str; 23] = [
        "cookie-banner",
        "cookie-notice",
        "cookie-consent",
        "gdpr-banner",
        "onetrust-consent-sdk",
        "onetrust-pc-sdk",
        "didomi-consent-popup",
        "qc-cmp2-container",
        "privacy-banner",
        "consent-banner",
        "cc-window",
        "cmp-banner",
        "CybotCookiebotDialog",
        "cookie-law-info-bar",
        "moove-gdpr-info-bar",
        "ginger-banner",
        "wp-gdpr-cookie-notice",
        "cookieControl",
        "osano-cm-dialog",
        "js-cookie-consent",
        "truste-consent-track",
        "sp-cc",
        "gdpr-consent-tool",
    ];
    const BANNER_CLASSES: [&str; 28] = [
        "cookie-banner",
        "cookie-notice",
        "cookie-consent",
        "gdpr-banner",
        "cc-window",
        "onetrust",
        "onetrust-pc-sdk",
        "didomi-consent-popup",
        "didomi-popup",
        "didomi-screen",
        "quantcast-cmp",
        "qc-cmp2-container",
        "privacy-banner",
        "consent-banner",
        "cmp-banner",
        "cookie-settings",
        "CybotCookiebotDialog",
        "cookiebot",
        "cookie-law-info-bar",
        "moove-gdpr-info-bar",
        "ginger-banner",
        "wp-gdpr-cookie-notice",
        "cookieControl",
        "osano-cm-dialog",
        "js-cookie-consent",
        "truste-consent-track",
        "sp-cc",
        "gdpr-consent-tool",
    ];

    let mut parent_map: std::collections::HashMap<
        incognidium_dom::NodeId,
        incognidium_dom::NodeId,
    > = std::collections::HashMap::new();
    for id in 0..doc.nodes.len() {
        for &cid in &doc.nodes[id].children {
            parent_map.insert(cid, id);
        }
    }

    let mut to_remove: Vec<incognidium_dom::NodeId> = Vec::new();
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            let classes: std::collections::HashSet<&str> = el.classes().into_iter().collect();
            let is_banner = BANNER_IDS.iter().any(|id_name| el.id() == Some(*id_name))
                || BANNER_CLASSES
                    .iter()
                    .any(|class_name| classes.contains(*class_name));
            if is_banner {
                to_remove.push(id);
            }
        }
    }

    for banner_id in &to_remove {
        if let Some(&parent_id) = parent_map.get(banner_id) {
            doc.nodes[parent_id]
                .children
                .retain(|cid| *cid != *banner_id);
        }
    }
    if !to_remove.is_empty() {
        eprintln!("Removed {} consent banner(s)", to_remove.len());
    }
}

/// Remove server-rendered "unsupported browser" / "upgrade your browser" banners.
/// Sites such as NBC News and nature.com show these notices when they detect a
/// browser whose capability set they do not recognize. Modern browsers never see
/// them, so stripping them brings the headless render closer to the real user
/// view. The list is intentionally conservative: exact id/class matches or a
/// substring match on the visible text for known banner names.
pub fn remove_unsupported_browser_banners(doc: &mut Document) {
    const BANNER_IDS: [&str; 4] = [
        "browser-upgrade",
        "unsupported-browser",
        "old-browser",
        "no-js-banner",
    ];
    const BANNER_CLASSES: [&str; 8] = [
        "alert-banner",
        "c-grade-c-banner",
        "browser-upgrade",
        "browser-notice",
        "unsupported-browser",
        "old-browser",
        "no-js-banner",
        "unsupported-notice",
    ];
    let mut parent_map: std::collections::HashMap<
        incognidium_dom::NodeId,
        incognidium_dom::NodeId,
    > = std::collections::HashMap::new();
    for id in 0..doc.nodes.len() {
        for &cid in &doc.nodes[id].children {
            parent_map.insert(cid, id);
        }
    }

    let mut to_remove: Vec<incognidium_dom::NodeId> = Vec::new();
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            let classes: std::collections::HashSet<&str> = el.classes().into_iter().collect();
            let is_banner = BANNER_IDS.iter().any(|id_name| el.id() == Some(*id_name))
                || BANNER_CLASSES
                    .iter()
                    .any(|class_name| classes.contains(*class_name));
            if is_banner {
                to_remove.push(id);
            }
        }
    }

    for banner_id in &to_remove {
        if let Some(&parent_id) = parent_map.get(banner_id) {
            doc.nodes[parent_id]
                .children
                .retain(|cid| *cid != *banner_id);
        }
    }
    if !to_remove.is_empty() {
        eprintln!("Removed {} unsupported-browser banner(s)", to_remove.len());
    }
}

/// mdBook sites render the table of contents through a custom element
/// (`<mdbook-sidebar-scrollbox>`) whose `connectedCallback` is defined in
/// `toc.js`. Incognidium does not implement `customElements`, so the sidebar
/// stays empty and the TOC text is lost. This helper fetches the server-provided
/// `toc.html` fallback and injects its chapter list into the scrollbox, then
/// ensures the sidebar is visible so it contributes to the rendered text.
/// Remove US government "Touchpoints" customer-feedback forms and their
/// trigger buttons. Sites such as FDA.gov embed a large satisfaction survey as
/// a hidden modal (`<div class="fba-usa-modal" data-touchpoints-form-id="...">`)
/// that is shown only after the user clicks a feedback button. Without the
/// Touchpoints script to keep it hidden, the modal and its trigger are laid out
/// inline, inflating page height with a long form that real users never see on
/// initial page load.
pub fn remove_touchpoints_forms(doc: &mut Document) {
    const FORM_CLASSES: [&str; 4] = [
        "fba-usa-modal",
        "fba-modal",
        "touchpoints-form-wrapper",
        "touchpoints-inner-form-wrapper",
    ];

    let mut parent_map: std::collections::HashMap<
        incognidium_dom::NodeId,
        incognidium_dom::NodeId,
    > = std::collections::HashMap::new();
    for id in 0..doc.nodes.len() {
        for &cid in &doc.nodes[id].children {
            parent_map.insert(cid, id);
        }
    }

    let mut to_remove: Vec<incognidium_dom::NodeId> = Vec::new();
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            let classes: std::collections::HashSet<&str> = el.classes().into_iter().collect();
            let is_touchpoints_form = FORM_CLASSES
                .iter()
                .any(|class_name| classes.contains(*class_name))
                || el.get_attr("data-touchpoints-form-id").is_some()
                || el.id().map_or(false, |id_name| {
                    id_name.contains("survey-btn") && classes.contains("btn")
                })
                || el
                    .get_attr("aria-controls")
                    .map_or(false, |ac| ac.starts_with("fba-modal"));
            if is_touchpoints_form {
                to_remove.push(id);
            }
        }
    }

    for form_id in &to_remove {
        if let Some(&parent_id) = parent_map.get(form_id) {
            doc.nodes[parent_id].children.retain(|cid| *cid != *form_id);
        }
    }
    if !to_remove.is_empty() {
        eprintln!("Removed {} touchpoints form(s)", to_remove.len());
    }
}

/// Collapse the USWDS "An official website of the United States government"
/// banner (`.usa-banner`) to its header bar. The full explanatory content block
/// (`<div class="usa-banner__content">`) is hidden in real browsers until the
/// user clicks "Here's how you know". Without the USWDS JS to collapse it, the
/// block renders inline and creates a tall, overlapping banner on many `.gov`
/// sites.
pub fn collapse_usa_banner(doc: &mut Document) {
    let mut parent_map: std::collections::HashMap<
        incognidium_dom::NodeId,
        incognidium_dom::NodeId,
    > = std::collections::HashMap::new();
    for id in 0..doc.nodes.len() {
        for &cid in &doc.nodes[id].children {
            parent_map.insert(cid, id);
        }
    }

    let mut to_remove: Vec<incognidium_dom::NodeId> = Vec::new();
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            let classes: std::collections::HashSet<&str> = el.classes().into_iter().collect();
            if classes.contains("usa-banner__content") {
                to_remove.push(id);
            }
        }
    }

    for content_id in &to_remove {
        if let Some(&parent_id) = parent_map.get(content_id) {
            doc.nodes[parent_id]
                .children
                .retain(|cid| *cid != *content_id);
        }
    }
    if !to_remove.is_empty() {
        eprintln!("Collapsed {} USA banner content block(s)", to_remove.len());
    }
}

pub fn trim_mdbook_sidebar(doc: &mut Document, base_url: &str) {
    // Detect mdBook by the custom sidebar element rather than URL, so any
    // mdBook-built site is covered.
    let scrollbox_id = match (0..doc.nodes.len()).find(|&id| {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            el.tag_name == "mdbook-sidebar-scrollbox"
        } else {
            false
        }
    }) {
        Some(id) => id,
        None => return,
    };

    // If JS already populated it (e.g. a future custom-elements implementation),
    // leave it alone.
    if !doc.nodes[scrollbox_id].children.is_empty() {
        return;
    }

    let toc_url = match resolve_url(base_url, "toc.html") {
        Ok(u) => u,
        Err(e) => {
            eprintln!("Failed to resolve mdBook toc.html for {}: {}", base_url, e);
            return;
        }
    };

    let toc_html = match fetch_url(&toc_url) {
        Ok(resp) if resp.status >= 200 && resp.status < 300 => resp.body,
        Ok(resp) => {
            eprintln!(
                "mdBook toc.html returned HTTP {} for {}, skipping sidebar fallback",
                resp.status, toc_url
            );
            return;
        }
        Err(e) => {
            eprintln!("Failed to fetch mdBook toc.html {}: {}", toc_url, e);
            return;
        }
    };

    let toc_doc = parse_html(&toc_html);
    let Some(toc_body) = toc_doc.body() else {
        eprintln!("mdBook toc.html has no <body>: {}", toc_url);
        return;
    };

    // Prefer the actual chapter list; fall back to all body children.
    let source_children: Vec<incognidium_dom::NodeId> = {
        let chapter_list = doc.nodes[toc_body].children.iter().copied().find(|&id| {
            if let NodeData::Element(el) = &toc_doc.nodes[id].data {
                el.tag_name == "ol" && el.classes().contains(&"chapter")
            } else {
                false
            }
        });
        if let Some(list_id) = chapter_list {
            vec![list_id]
        } else {
            toc_doc.nodes[toc_body].children.iter().copied().collect()
        }
    };

    if source_children.is_empty() {
        return;
    }

    // Recursively copy the selected toc nodes into the main document.
    fn copy_subtree(
        src_doc: &Document,
        src_id: incognidium_dom::NodeId,
        dst_doc: &mut Document,
        dst_parent: incognidium_dom::NodeId,
    ) {
        let src_node = &src_doc.nodes[src_id];
        let new_id = match &src_node.data {
            NodeData::Element(el) => {
                let mut new_el = incognidium_dom::ElementData::new(el.tag_name.clone());
                new_el.attributes = el.attributes.clone();
                dst_doc.add_node(dst_parent, NodeData::Element(new_el))
            }
            NodeData::Text(t) => dst_doc.add_node(
                dst_parent,
                NodeData::Text(incognidium_dom::TextData {
                    content: t.content.clone(),
                }),
            ),
            NodeData::Comment(c) => dst_doc.add_node(dst_parent, NodeData::Comment(c.clone())),
            NodeData::Document => return,
        };
        for &child_id in &src_node.children {
            copy_subtree(src_doc, child_id, dst_doc, new_id);
        }
    }

    for &src_id in &source_children {
        copy_subtree(&toc_doc, src_id, doc, scrollbox_id);
    }

    // mdBook's inline visibility script hides the sidebar on viewports narrower
    // than 1080 px. Force it visible so the injected TOC is laid out and
    // extracted as text.
    if let Some(html_id) = doc.document_element() {
        if let NodeData::Element(el) = &mut doc.nodes[html_id].data {
            let classes = el.attributes.entry("class".to_string()).or_default();
            let mut class_list: Vec<&str> = classes.split_whitespace().collect();
            if !class_list.iter().any(|c| *c == "sidebar-visible") {
                class_list.push("sidebar-visible");
                *classes = class_list.join(" ");
            }
        }
    }
    if let Some(nav_id) = doc.get_element_by_id("mdbook-sidebar") {
        if let NodeData::Element(el) = &mut doc.nodes[nav_id].data {
            el.attributes
                .insert("aria-hidden".to_string(), "false".to_string());
            el.attributes.remove("style");
        }
    }

    eprintln!(
        "Populated mdBook sidebar from {} ({} top-level nodes)",
        toc_url,
        source_children.len()
    );
}

pub fn trim_scroll_snap_carousels(doc: &mut Document) {
    fn parse_count(classes: &[&str]) -> Option<usize> {
        for c in classes {
            if let Some(num) = c.strip_prefix("count_") {
                if let Ok(n) = num.parse() {
                    return Some(n);
                }
            }
        }
        None
    }

    fn is_scroll_container(el: &incognidium_dom::ElementData) -> bool {
        let classes: std::collections::HashSet<&str> = el.classes().into_iter().collect();
        classes.contains("scroll-container") && classes.contains("snap-container-x")
    }

    fn is_overflow_container(el: &incognidium_dom::ElementData) -> bool {
        let classes: std::collections::HashSet<&str> = el.classes().into_iter().collect();
        classes.contains("no-scrollbar") && classes.contains("overflow-x-auto")
    }

    fn is_scroll_item(el: &incognidium_dom::ElementData) -> bool {
        el.classes().contains(&"scroll-item")
    }

    fn is_list_item(el: &incognidium_dom::ElementData) -> bool {
        let tag = el.tag_name.as_str();
        tag == "li" || tag == "article"
    }

    fn is_list(el: &incognidium_dom::ElementData) -> bool {
        let tag = el.tag_name.as_str();
        tag == "ul" || tag == "ol"
    }

    let mut removals: Vec<(incognidium_dom::NodeId, Vec<incognidium_dom::NodeId>)> = Vec::new();

    for id in 0..doc.nodes.len() {
        if let incognidium_dom::NodeData::Element(el) = &doc.nodes[id].data {
            let is_scroll = is_scroll_container(el);
            let is_overflow = is_overflow_container(el);
            if !is_scroll && !is_overflow {
                continue;
            }

            let count = if is_overflow {
                // Horizontal overflow carousels (NFL-style Tailwind, The Register
                // scroll-snap containers, etc.): keep the first 4 visible cards.
                4usize
            } else {
                match parse_count(&el.classes()) {
                    Some(n) if n > 0 => n,
                    _ => continue,
                }
            };

            // For The Register-style scroll containers the items are direct
            // children with the scroll-item class.
            if is_scroll {
                let children = doc.nodes[id].children.clone();
                let mut kept = 0usize;
                let to_remove: Vec<incognidium_dom::NodeId> = children
                    .iter()
                    .filter(|&&cid| {
                        if let incognidium_dom::NodeData::Element(child_el) = &doc.nodes[cid].data {
                            if is_scroll_item(child_el) {
                                kept += 1;
                                return kept > count;
                            }
                        }
                        false
                    })
                    .copied()
                    .collect();
                if !to_remove.is_empty() {
                    removals.push((id, to_remove));
                }
                continue;
            }

            // Overflow-x-auto carousels may expose their items directly, or wrap
            // them in a <ul>/<ol> (NFL), or wrap each card in a <div>. Trim the
            // first card-bearing child list/collection we find and leave spacers
            // and decorative wrappers alone.
            let container_children = doc.nodes[id].children.clone();
            let list_child = container_children.iter().find(|&&cid| {
                if let incognidium_dom::NodeData::Element(child_el) = &doc.nodes[cid].data {
                    is_list(child_el)
                } else {
                    false
                }
            });

            if let Some(&list_id) = list_child {
                let list_children = doc.nodes[list_id].children.clone();
                let mut kept = 0usize;
                let to_remove: Vec<incognidium_dom::NodeId> = list_children
                    .iter()
                    .filter(|&&cid| {
                        if let incognidium_dom::NodeData::Element(child_el) = &doc.nodes[cid].data {
                            if is_list_item(child_el) {
                                kept += 1;
                                return kept > count;
                            }
                        }
                        false
                    })
                    .copied()
                    .collect();
                if !to_remove.is_empty() {
                    removals.push((list_id, to_remove));
                }
            } else {
                // No list wrapper; trim direct card-like children. Skip purely
                // decorative spacers (aria-hidden) and non-element nodes.
                let mut kept = 0usize;
                let to_remove: Vec<incognidium_dom::NodeId> = container_children
                    .iter()
                    .filter(|&&cid| {
                        if let incognidium_dom::NodeData::Element(child_el) = &doc.nodes[cid].data {
                            if child_el.get_attr("aria-hidden") == Some("true") {
                                return false;
                            }
                            kept += 1;
                            return kept > count;
                        }
                        false
                    })
                    .copied()
                    .collect();
                if !to_remove.is_empty() {
                    removals.push((id, to_remove));
                }
            }
        }
    }

    for (parent_id, to_remove) in removals {
        let set: std::collections::HashSet<incognidium_dom::NodeId> =
            to_remove.iter().copied().collect();
        doc.nodes[parent_id]
            .children
            .retain(|cid| !set.contains(cid));
    }
}

/// Maximum pixel dimension for rasterized inline SVGs. Icons should stay small;
/// large decorative SVGs are downscaled to keep memory and paint costs sane.
const MAX_INLINE_SVG_DIM: f32 = 512.0;
const MAX_INLINE_SVGS: usize = 100;

fn escape_xml_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn escape_xml_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn serialize_svg_subtree(
    doc: &Document,
    node_id: incognidium_dom::NodeId,
    out: &mut String,
    defs: &HashMap<String, incognidium_dom::NodeId>,
    depth: usize,
) {
    const MAX_SVG_DEPTH: usize = 10;
    let node = &doc.nodes[node_id];
    match &node.data {
        NodeData::Element(el) => {
            // Expand `<use href="#id">` (and the legacy `xlink:href` form) so that
            // SVG sprite icons -- common in menus and dropdown chevrons -- can be
            // rasterized even though usvg does not resolve external/external-by-id
            // references in the HTML DOM.
            if el.tag_name == "use" && depth < MAX_SVG_DEPTH {
                let href = el
                    .attributes
                    .get("href")
                    .or_else(|| el.attributes.get("xlink:href"))
                    .cloned()
                    .unwrap_or_default();
                if let Some(target_id) = href.strip_prefix('#').and_then(|id| defs.get(id)) {
                    if let Some(target_el) = doc.nodes.get(*target_id).and_then(|n| match &n.data {
                        NodeData::Element(ref e) => Some(e),
                        _ => None,
                    }) {
                        let target_node = &doc.nodes[*target_id];
                        let x = el.attributes.get("x").and_then(|s| s.parse::<f32>().ok());
                        let y = el.attributes.get("y").and_then(|s| s.parse::<f32>().ok());
                        let use_transform =
                            el.attributes.get("transform").cloned().unwrap_or_default();

                        // SVG <use> placement is a translate(x,y) followed by the element's
                        // own transform.  Combine them into a single transform on the emitted
                        // wrapper/target so references like
                        // <use href="#wordmark" transform="translate(34)"/> render at the
                        // correct position.
                        let mut transform_parts = Vec::new();
                        if !use_transform.is_empty() {
                            transform_parts.push(use_transform);
                        }
                        match (x, y) {
                            (Some(xv), Some(yv)) => {
                                transform_parts.push(format!("translate({}, {})", xv, yv));
                            }
                            (Some(xv), None) => {
                                transform_parts.push(format!("translate({}, 0)", xv));
                            }
                            (None, Some(yv)) => {
                                transform_parts.push(format!("translate(0, {})", yv));
                            }
                            _ => {}
                        }

                        if target_el.tag_name == "symbol" {
                            // A <symbol> is a template: render its children inside a
                            // group, applying any use offset/transform.  The surrounding
                            // <svg> supplies the viewport in the common sprite-sheet case.
                            out.push_str("<g");
                            if !transform_parts.is_empty() {
                                out.push_str(" transform=\"");
                                out.push_str(&escape_xml_attr(&transform_parts.join(" ")));
                                out.push('"');
                            }
                            out.push('>');
                            for &child_id in &target_node.children {
                                serialize_svg_subtree(doc, child_id, out, defs, depth + 1);
                            }
                            out.push_str("</g>");
                        } else {
                            // For a direct shape/group reference, emit the referenced
                            // element, merging the <use>'s placement into a transform
                            // while preserving the target's own transform (applied last
                            // to the target's children).
                            let target_transform = target_el
                                .attributes
                                .get("transform")
                                .cloned()
                                .unwrap_or_default();
                            if !target_transform.is_empty() {
                                transform_parts.push(target_transform);
                            }
                            let combined_transform = transform_parts.join(" ");

                            out.push('<');
                            out.push_str(&target_el.tag_name);
                            for (k, v) in &target_el.attributes {
                                if k == "id" || k == "transform" {
                                    continue;
                                }
                                out.push(' ');
                                out.push_str(k);
                                out.push_str("=\"");
                                out.push_str(&escape_xml_attr(v));
                                out.push('"');
                            }
                            if !combined_transform.is_empty() {
                                out.push_str(" transform=\"");
                                out.push_str(&escape_xml_attr(&combined_transform));
                                out.push('"');
                            }
                            if target_node.children.is_empty() {
                                out.push_str("/>");
                            } else {
                                out.push('>');
                                for &child_id in &target_node.children {
                                    serialize_svg_subtree(doc, child_id, out, defs, depth + 1);
                                }
                                out.push_str("</");
                                out.push_str(&target_el.tag_name);
                                out.push('>');
                            }
                        }
                        return;
                    }
                }
            }

            out.push('<');
            out.push_str(&el.tag_name);
            for (k, v) in &el.attributes {
                out.push(' ');
                out.push_str(k);
                out.push_str("=\"");
                out.push_str(&escape_xml_attr(v));
                out.push('"');
            }
            // Inline SVGs in HTML often omit the SVG namespace, but usvg needs it
            // to parse the standalone XML document we build here.
            if el.tag_name == "svg" && !el.attributes.contains_key("xmlns") {
                out.push_str(" xmlns=\"http://www.w3.org/2000/svg\"");
            }
            if node.children.is_empty() {
                // SVG elements like <line>, <path>, <circle> are typically empty in source;
                // write them as self-closing for compact XML.
                out.push_str("/>");
            } else {
                out.push('>');
                for &child_id in &node.children {
                    serialize_svg_subtree(doc, child_id, out, defs, depth + 1);
                }
                out.push_str("</");
                out.push_str(&el.tag_name);
                out.push('>');
            }
        }
        NodeData::Text(t) => {
            out.push_str(&escape_xml_text(&t.content));
        }
        NodeData::Comment(c) => {
            out.push_str("<!--");
            out.push_str(c);
            out.push_str("-->");
        }
        _ => {}
    }
}

fn css_color_to_svg(color: CssColor) -> String {
    if color.a == 255 {
        format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
    } else {
        let a = color.a as f32 / 255.0;
        format!("rgba({}, {}, {}, {})", color.r, color.g, color.b, a)
    }
}

fn render_svg_xml(svg: &str, current_color: CssColor) -> Option<ImageData> {
    // Inline SVGs frequently use `currentColor` for strokes/fills so they match
    // the surrounding text color. usvg alone cannot resolve CSS `currentColor`,
    // so substitute the computed (or default) color before rasterizing.
    let color_str = css_color_to_svg(current_color);
    let svg = svg.replace("currentColor", &color_str);
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_str(&svg, &opt).ok()?;
    let size = tree.size();
    let intrinsic_w = size.width();
    let intrinsic_h = size.height();
    if intrinsic_w <= 0.0 || intrinsic_h <= 0.0 {
        return None;
    }
    let scale = if intrinsic_w > MAX_INLINE_SVG_DIM || intrinsic_h > MAX_INLINE_SVG_DIM {
        MAX_INLINE_SVG_DIM / intrinsic_w.max(intrinsic_h)
    } else {
        1.0
    };
    let w = (intrinsic_w * scale).ceil().max(1.0) as u32;
    let h = (intrinsic_h * scale).ceil().max(1.0) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    let transform = tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    // tiny-skia uses premultiplied BGRA; convert to straight RGBA.
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for px in pixmap.pixels() {
        let a = px.alpha();
        if a == 0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            let inv = 255.0 / a as f32;
            out.push(((px.red() as f32 * inv).min(255.0)) as u8);
            out.push(((px.green() as f32 * inv).min(255.0)) as u8);
            out.push(((px.blue() as f32 * inv).min(255.0)) as u8);
            out.push(a);
        }
    }
    Some(ImageData {
        pixels: out,
        width: w,
        height: h,
    })
}

/// Rasterize inline `<svg>` elements and turn them into `<img>` placeholders
/// that reference the raster in `image_cache`. This lets the existing layout
/// and paint pipelines render icon menus, logos, and other simple inline SVGs
/// without needing a full SVG layout implementation.
/// Resolve a CSS `<length> | <percentage> | calc()` size to pixels so that
/// inline SVG placeholders are rasterized at the same dimensions the author
/// intended, even when the resolved value is not stored as `SizeValue::Px`.
/// Viewport and containing-block sizes are approximated here because layout
/// has not run yet; for the common case of `rem`/`em`/`px` calc() expressions
/// used by icon fonts this is exact.
fn resolve_size_for_svg(
    value: &SizeValue,
    font_size: f32,
    viewport_width: f32,
    viewport_height: f32,
    is_height: bool,
) -> Option<f32> {
    fn calc_value_to_px(
        val: &CalcValue,
        font_size: f32,
        viewport_width: f32,
        viewport_height: f32,
        is_height: bool,
    ) -> f32 {
        let pct_basis = if is_height {
            viewport_height
        } else {
            viewport_width
        };
        match val {
            CalcValue::Px(v) => *v,
            CalcValue::Percent(p) => p / 100.0 * pct_basis,
            CalcValue::Em(e) => e * font_size,
            CalcValue::Rem(r) => r * 16.0,
            CalcValue::Vw(v) => v * viewport_width / 100.0,
            CalcValue::Vh(v) => v * viewport_height / 100.0,
            CalcValue::Cqw(v) => v * viewport_width / 100.0,
            CalcValue::Cqh(v) => v * viewport_height / 100.0,
            CalcValue::Cqi(v) => v * viewport_width / 100.0,
            CalcValue::Cqb(v) => v * viewport_height / 100.0,
            CalcValue::Cqmin(v) => v * viewport_width.min(viewport_height) / 100.0,
            CalcValue::Cqmax(v) => v * viewport_width.max(viewport_height) / 100.0,
        }
    }

    fn calc_expr_to_px(
        expr: &CalcExpression,
        font_size: f32,
        viewport_width: f32,
        viewport_height: f32,
        is_height: bool,
    ) -> f32 {
        match expr {
            CalcExpression::Value(v) => {
                calc_value_to_px(v, font_size, viewport_width, viewport_height, is_height)
            }
            CalcExpression::Add(a, b) => {
                calc_expr_to_px(a, font_size, viewport_width, viewport_height, is_height)
                    + calc_expr_to_px(b, font_size, viewport_width, viewport_height, is_height)
            }
            CalcExpression::Subtract(a, b) => {
                calc_expr_to_px(a, font_size, viewport_width, viewport_height, is_height)
                    - calc_expr_to_px(b, font_size, viewport_width, viewport_height, is_height)
            }
            CalcExpression::Multiply(a, b) => {
                calc_expr_to_px(a, font_size, viewport_width, viewport_height, is_height)
                    * calc_expr_to_px(b, font_size, viewport_width, viewport_height, is_height)
            }
            CalcExpression::Divide(a, b) => {
                let denom =
                    calc_expr_to_px(b, font_size, viewport_width, viewport_height, is_height);
                if denom == 0.0 {
                    0.0
                } else {
                    calc_expr_to_px(a, font_size, viewport_width, viewport_height, is_height)
                        / denom
                }
            }
        }
    }

    let pct_basis = if is_height {
        viewport_height
    } else {
        viewport_width
    };
    match value {
        SizeValue::Px(v) => Some(*v),
        SizeValue::Percent(p) => Some(p / 100.0 * pct_basis),
        SizeValue::Calc(expr) => Some(calc_expr_to_px(
            expr,
            font_size,
            viewport_width,
            viewport_height,
            is_height,
        )),
        SizeValue::Min(vals) => vals
            .iter()
            .map(|v| calc_value_to_px(v, font_size, viewport_width, viewport_height, is_height))
            .reduce(f32::min),
        SizeValue::Max(vals) => vals
            .iter()
            .map(|v| calc_value_to_px(v, font_size, viewport_width, viewport_height, is_height))
            .reduce(f32::max),
        SizeValue::Clamp { min, val, max } => {
            let min_px =
                calc_value_to_px(min, font_size, viewport_width, viewport_height, is_height);
            let val_px =
                calc_value_to_px(val, font_size, viewport_width, viewport_height, is_height);
            let max_px =
                calc_value_to_px(max, font_size, viewport_width, viewport_height, is_height);
            Some(val_px.clamp(min_px, max_px))
        }
        _ => None,
    }
}

pub fn rasterize_inline_svgs(
    doc: &mut Document,
    image_cache: &mut HashMap<String, ImageData>,
    mut styles: Option<&mut StyleMap>,
    viewport_width: f32,
    viewport_height: f32,
) {
    let svg_ids: Vec<incognidium_dom::NodeId> = doc
        .nodes
        .iter()
        .filter_map(|n| {
            if let NodeData::Element(el) = &n.data {
                if el.tag_name == "svg" {
                    return Some(n.id);
                }
            }
            None
        })
        .collect();

    // Build a global map of `id`-attributed nodes so that SVG `<use href="#id">`
    // references can be expanded while serializing each SVG.  This covers both
    // symbols defined inside the same SVG and icons kept in a hidden sprite sheet
    // elsewhere in the document.
    let mut svg_defs: HashMap<String, incognidium_dom::NodeId> = HashMap::new();
    for n in &doc.nodes {
        if let NodeData::Element(el) = &n.data {
            if let Some(id) = el.attributes.get("id") {
                svg_defs.insert(id.clone(), n.id);
            }
        }
    }

    let mut count = 0usize;
    for id in svg_ids {
        if count >= MAX_INLINE_SVGS {
            break;
        }
        let mut svg_xml = String::new();
        serialize_svg_subtree(doc, id, &mut svg_xml, &svg_defs, 0);
        if svg_xml.is_empty() {
            continue;
        }
        let current_color = styles
            .as_ref()
            .and_then(|s| s.get(&id))
            .map(|s| s.color)
            .unwrap_or(CssColor::BLACK);
        let (parent_id, parent_info) = {
            let node = &doc.nodes[id];
            let info = node
                .parent
                .and_then(|pid| doc.nodes.get(pid))
                .and_then(|p| match &p.data {
                    NodeData::Element(ref pe) => {
                        Some(pe.get_attr("class").unwrap_or("").to_string())
                    }
                    _ => None,
                })
                .unwrap_or_default();
            (node.parent, info)
        };
        let Some(img) = render_svg_xml(&svg_xml, current_color) else {
            continue;
        };
        let key = format!("inline-svg:{id}");

        // Detach SVG children first so we can safely mutate the node data next.
        let children = std::mem::take(&mut doc.nodes[id].children);
        for child in children {
            doc.nodes[child].parent = None;
        }

        if let NodeData::Element(ref mut el) = doc.nodes[id].data {
            // Preserve author-specified dimensions if present. CSS widths/heights
            // take precedence over SVG attributes and intrinsic raster size,
            // otherwise icon fonts such as mdBook's Font Awesome icons blow up
            // to the raster's native resolution (e.g. 512x512).
            let svg_width_attr = el
                .get_attr("width")
                .and_then(|w| w.trim_end_matches("px").parse::<f32>().ok());
            let svg_height_attr = el
                .get_attr("height")
                .and_then(|h| h.trim_end_matches("px").parse::<f32>().ok());

            let css_width = styles
                .as_ref()
                .and_then(|s| s.get(&id))
                .map(|s| s.width.clone())
                .filter(|w| !matches!(w, SizeValue::Auto | SizeValue::None));
            let css_height = styles
                .as_ref()
                .and_then(|s| s.get(&id))
                .map(|s| s.height.clone())
                .filter(|h| !matches!(h, SizeValue::Auto | SizeValue::None));

            let intrinsic_w = img.width as f32;
            let intrinsic_h = img.height as f32;

            let font_size = styles
                .as_ref()
                .and_then(|s| s.get(&id))
                .map(|s| s.font_size)
                .unwrap_or(16.0);

            let mut width_px = css_width.as_ref().and_then(|w| {
                resolve_size_for_svg(w, font_size, viewport_width, viewport_height, false)
            });
            let mut height_px = css_height.as_ref().and_then(|h| {
                resolve_size_for_svg(h, font_size, viewport_width, viewport_height, true)
            });

            // Only derive a missing dimension from the intrinsic aspect ratio when
            // the known CSS dimension is a definite absolute size. Deriving from a
            // percentage (e.g. `height: 100%` inside a 22 px wrapper) uses the wrong
            // basis before layout has resolved the real containing height, which
            // previously produced absurd placeholder widths such as the NBC News
            // wordmark logo expanding to ~7261 px.
            let is_definite = |v: &SizeValue| {
                matches!(
                    v,
                    SizeValue::Px(_)
                        | SizeValue::Calc(_)
                        | SizeValue::Min(_)
                        | SizeValue::Max(_)
                        | SizeValue::Clamp { .. }
                )
            };
            let definite_width = css_width.as_ref().map_or(false, is_definite);
            let definite_height = css_height.as_ref().map_or(false, is_definite);

            if width_px.is_none() && height_px.is_some() && definite_height && intrinsic_h > 0.0 {
                width_px = Some((height_px.unwrap() / intrinsic_h) * intrinsic_w);
            } else if height_px.is_none()
                && width_px.is_some()
                && definite_width
                && intrinsic_w > 0.0
            {
                height_px = Some((width_px.unwrap() / intrinsic_w) * intrinsic_h);
            }

            let attr_width = if definite_width {
                width_px.or(svg_width_attr).unwrap_or(intrinsic_w)
            } else {
                svg_width_attr.unwrap_or(intrinsic_w)
            }
            .round()
            .max(1.0) as u32;
            let attr_height = if definite_height {
                height_px.or(svg_height_attr).unwrap_or(intrinsic_h)
            } else {
                svg_height_attr.unwrap_or(intrinsic_h)
            }
            .round()
            .max(1.0) as u32;

            let scale_to_parent = css_width.is_none()
                && css_height.is_none()
                && svg_width_attr.is_none()
                && svg_height_attr.is_none()
                && parent_id.is_some()
                && styles.as_ref().map_or(false, |s| {
                    if let Some(ps) = s.get(&parent_id.unwrap()) {
                        !matches!(ps.width, SizeValue::Auto | SizeValue::None)
                            && !matches!(ps.height, SizeValue::Auto | SizeValue::None)
                    } else {
                        false
                    }
                });

            el.tag_name = "img".to_string();
            el.attributes.insert("src".to_string(), key.clone());
            el.attributes
                .insert("width".to_string(), attr_width.to_string());
            el.attributes
                .insert("height".to_string(), attr_height.to_string());
            // Alt text so the placeholder is accessible and visible even if
            // the raster is not in cache.
            if !el.attributes.contains_key("alt") {
                let alt = el.get_attr("aria-label").unwrap_or("").to_string();
                el.attributes.insert("alt".to_string(), alt);
            }
            image_cache.insert(key, img);
            // The element was an <svg> during style resolution, so the computed
            // style likely has the UA `display: none` that hides SVGs. The
            // rasterized placeholder is now an <img>; make sure it participates
            // in layout with its explicit dimensions.
            if let Some(styles) = styles.as_deref_mut() {
                let style = styles.entry(id).or_default();
                style.display = Display::Inline;
                if scale_to_parent {
                    // Width fills the containing block; height follows the SVG's
                    // intrinsic aspect ratio. This matches how browsers render an
                    // SVG with no explicit size inside a sized container.
                    style.width = SizeValue::Percent(100.0);
                    style.height = SizeValue::Auto;
                } else {
                    // CSS math functions (calc/min/max/clamp) are evaluated above
                    // so layout_image does not fall back to the intrinsic raster
                    // dimensions. Percentages and explicit pixel sizes are
                    // preserved so width:100% and author px heights still behave
                    // correctly. When the author gives only one definite absolute
                    // dimension, the other is locked to the derived pixel size. If
                    // a dimension is auto and could not be derived from a definite
                    // sibling, leave it auto so the layout engine uses the
                    // placeholder's intrinsic size and the real containing-block
                    // height instead of a viewport-basis guess.
                    let is_math_fn = |v: &SizeValue| {
                        matches!(
                            v,
                            SizeValue::Calc(_)
                                | SizeValue::Min(_)
                                | SizeValue::Max(_)
                                | SizeValue::Clamp { .. }
                        )
                    };
                    style.width = if css_width.as_ref().map_or(false, is_math_fn) {
                        SizeValue::Px(attr_width as f32)
                    } else if css_width.is_some() {
                        css_width.unwrap()
                    } else if width_px.is_some() {
                        SizeValue::Px(width_px.unwrap())
                    } else {
                        SizeValue::Auto
                    };
                    style.height = if css_height.as_ref().map_or(false, is_math_fn) {
                        SizeValue::Px(attr_height as f32)
                    } else if css_height.is_some() {
                        css_height.unwrap()
                    } else if height_px.is_some() {
                        SizeValue::Px(height_px.unwrap())
                    } else {
                        SizeValue::Auto
                    };
                }
            }
            count += 1;
        }
    }
}

/// Predicate to skip synthetic inline-SVG URLs during normal image fetching.
pub fn is_inline_svg_url(url: &str) -> bool {
    url.starts_with("inline-svg:")
}

/// Encode a tiny-skia pixmap as a PNG using best compression. Smaller than
/// `pixmap.save_png()`/`encode_png()` while remaining lossless, which matters
/// for very long news/article pages in the QA pipeline.
pub fn encode_png_compressed(
    pixmap: &Pixmap,
    writer: impl std::io::Write,
) -> Result<(), Box<dyn std::error::Error>> {
    let encoder = PngEncoder::new_with_quality(writer, CompressionType::Best, FilterType::Adaptive);
    encoder.write_image(
        pixmap.data(),
        pixmap.width(),
        pixmap.height(),
        image::ColorType::Rgba8.into(),
    )?;
    Ok(())
}

/// Convenience wrapper that writes a compressed PNG to disk.
pub fn save_png_compressed(pixmap: &Pixmap, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::File::create(path)?;
    encode_png_compressed(pixmap, file)
}
