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
use std::sync::Arc;

use incognidium_css::CssValue;

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

/// Fix AOL/Yahoo CSS subgrid declarations that the headless renderer cannot
/// layout, causing article cards to stack in a single full-width column.
///
/// The AOL homepage uses `.grid-cols-subgrid` (`grid-template-columns: subgrid`)
/// inside a 12-column outer grid. Because subgrid is not implemented, the track
/// list ends up empty and every card becomes full-width, which both squashes
/// headlines into narrow columns and makes the page thousands of pixels taller
/// than the real browser view. Replacing the class with `grid-cols-12` restores
/// the intended multi-column layout.
pub fn fix_aol_yahoo_subgrid(doc: &mut Document, base_url: &str) {
    let lower = base_url.to_ascii_lowercase();
    let is_aol_yahoo = lower.contains("aol.com") || lower.contains("yahoo.com");
    if !is_aol_yahoo {
        return;
    }

    let mut changed = 0usize;
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &mut doc.node_mut(id).data {
            let cls = el.classes();
            if cls.iter().any(|c| *c == "grid-cols-subgrid") {
                let new_classes: Vec<&str> = cls
                    .into_iter()
                    .map(|c| {
                        if c == "grid-cols-subgrid" {
                            "grid-cols-12"
                        } else {
                            c
                        }
                    })
                    .collect();
                el.attributes
                    .insert("class".to_string(), new_classes.join(" "));
                changed += 1;
            }
        }
    }

    if changed > 0 {
        eprintln!("Fixed {} AOL/Yahoo subgrid container(s)", changed);
    }
}

/// Remove AOL/Yahoo ad containers that would otherwise reserve hundreds of
/// pixels of empty vertical space.
///
/// AOL and Yahoo homepages render ad slots (`m-gam`, `m-gam__container`) and an
/// ad-detection element (`m-ad-blocker`) in the server HTML. Real browsers fill
/// these with ads, but in the headless renderer the slots contain only an
/// "Advertisement" label and still apply `min-height: 600px` / `height: 600px`
/// rules. This helper drops those subtrees entirely, and also removes the
/// `.m-banner--bannerAd` wrapper that contains the top-center ad slot so the
/// fixed-height banner shell does not push content down.
pub fn remove_aol_yahoo_ad_slots(doc: &mut Document) {
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
            let is_ad_slot = classes.contains("m-gam")
                || classes.contains("m-gam__container")
                || classes.contains("m-gam__placeholder")
                || classes.contains("m-ad-blocker")
                || classes.contains("m-banner--bannerAd");
            if is_ad_slot {
                to_remove.push(id);
            }
        }
    }

    if to_remove.is_empty() {
        return;
    }

    // Walk ancestors of removed ad slots and also remove ancestor nodes that
    // only existed to hold the ad slot, such as `.m-banner--bannerAd`'s inner
    // wrappers.  Stop at the first ancestor that has other children.
    let mut remove_set: std::collections::HashSet<incognidium_dom::NodeId> =
        to_remove.iter().copied().collect();
    let mut changed = true;
    while changed {
        changed = false;
        let mut new_orphans: Vec<incognidium_dom::NodeId> = Vec::new();
        for id in 0..doc.nodes.len() {
            if remove_set.contains(&id) {
                continue;
            }
            if let NodeData::Element(el) = &doc.nodes[id].data {
                // An ancestor becomes removable if all of its children are already
                // marked for removal and the element itself is ad-related.
                let classes: std::collections::HashSet<&str> = el.classes().into_iter().collect();
                let is_ad_related = classes.contains("m-banner")
                    || classes.contains("m-banner__inner")
                    || classes.contains("m-banner__inner--container");
                if !is_ad_related || doc.nodes[id].children.is_empty() {
                    continue;
                }
                if doc.nodes[id]
                    .children
                    .iter()
                    .all(|cid| remove_set.contains(cid))
                {
                    new_orphans.push(id);
                }
            }
        }
        for id in new_orphans {
            if remove_set.insert(id) {
                changed = true;
            }
        }
    }

    // Detach every node in the removal set from its parent.  We iterate over all
    // parents and drop removed children in one pass.
    for id in 0..doc.nodes.len() {
        if let Some(&parent_id) = parent_map.get(&id) {
            doc.nodes[parent_id]
                .children
                .retain(|cid| !remove_set.contains(cid));
        }
    }

    eprintln!("Removed {} AOL/Yahoo ad slot(s)", to_remove.len());
}

/// Remove AdChoices (`#adchoicesBtn`) icons that are appended to `<body>` by ad
/// scripts. Without the script that positions and sizes them, Incognidium's SVG
/// rasterization uses the icon's huge viewBox and the element covers the top-left
/// of the page. Drop the icon and its wrapper when it has no other visible
/// siblings.
pub fn remove_adchoices_overlays(doc: &mut Document) {
    let mut parent_map: std::collections::HashMap<
        incognidium_dom::NodeId,
        incognidium_dom::NodeId,
    > = std::collections::HashMap::new();
    for id in 0..doc.nodes.len() {
        for &cid in &doc.nodes[id].children {
            parent_map.insert(cid, id);
        }
    }

    let mut to_remove: std::collections::HashSet<incognidium_dom::NodeId> =
        std::collections::HashSet::new();
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            if el.get_attr("id").map(|v| v.trim()) == Some("adchoicesBtn") {
                to_remove.insert(id);
                if let Some(&parent_id) = parent_map.get(&id) {
                    if let NodeData::Element(parent_el) = &doc.nodes[parent_id].data {
                        if parent_el.tag_name == "div"
                            && doc.nodes[parent_id]
                                .children
                                .iter()
                                .filter(|&&cid| matches!(doc.nodes[cid].data, NodeData::Element(_)))
                                .count()
                                <= 1
                        {
                            to_remove.insert(parent_id);
                        }
                    }
                }
            }
        }
    }

    if to_remove.is_empty() {
        return;
    }

    for id in 0..doc.nodes.len() {
        if let Some(&parent_id) = parent_map.get(&id) {
            doc.nodes[parent_id]
                .children
                .retain(|cid| !to_remove.contains(cid));
        }
    }

    eprintln!("Removed {} AdChoices overlay node(s)", to_remove.len());
}

/// Remove Yahoo's server-rendered stream skeleton cards.
///
/// Yahoo's homepage server-renders the "For You" feed as `<li>` items whose
/// image slot uses `yahoo-nebula-ad-placeholder-image` and whose headline/body
/// lines are empty `bg-tertiary` bars. Incognidium's JS engine does not fetch the
/// real feed JSON, so these skeletons paint as a column of gray placeholder
/// cards. This helper removes the skeleton `<li>` items (and any top-level
/// `cls-stream-placeholder` rail) so the static render shows the real
/// server-rendered chrome instead of a wall of empty placeholders.
pub fn trim_yahoo_stream_skeletons(doc: &mut Document, base_url: &str) {
    let lower = base_url.to_ascii_lowercase();
    if !lower.contains("yahoo.com") {
        return;
    }

    let mut to_remove: Vec<incognidium_dom::NodeId> = Vec::new();

    // Remove feed list items that contain the Yahoo placeholder image class.
    // These are the skeleton cards in the "For You" stream.
    fn subtree_has_class(doc: &Document, root: incognidium_dom::NodeId, target: &str) -> bool {
        for &cid in &doc.nodes[root].children {
            if let NodeData::Element(el) = &doc.nodes[cid].data {
                if el.classes().contains(&target) {
                    return true;
                }
            }
            if subtree_has_class(doc, cid, target) {
                return true;
            }
        }
        false
    }

    // A Yahoo feed skeleton card uses the placeholder image plus empty
    // `bg-tertiary` bars where the headline/body will appear. The real
    // server-rendered hero card uses the same `yahoo-nebula-ad-placeholder-image`
    // class for its image slot, so we keep any `<li>` that has real text content.
    fn subtree_has_text(doc: &Document, root: incognidium_dom::NodeId) -> bool {
        for &cid in &doc.nodes[root].children {
            match &doc.nodes[cid].data {
                NodeData::Text(t) if !t.content.trim().is_empty() => return true,
                NodeData::Element(el) => {
                    if let Some(alt) = el.get_attr("alt") {
                        if !alt.trim().is_empty() {
                            return true;
                        }
                    }
                    if let Some(aria) = el.get_attr("aria-label") {
                        if !aria.trim().is_empty() {
                            return true;
                        }
                    }
                }
                _ => {}
            }
            if subtree_has_text(doc, cid) {
                return true;
            }
        }
        false
    }

    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            if el.tag_name != "li" {
                continue;
            }
            if subtree_has_class(doc, id, "yahoo-nebula-ad-placeholder-image")
                && !subtree_has_text(doc, id)
            {
                to_remove.push(id);
            }
        }
    }

    // Remove the top-level stream placeholder rail if present.
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            let classes: std::collections::HashSet<&str> = el.classes().into_iter().collect();
            if classes
                .iter()
                .any(|c| c.starts_with("cls-stream-placeholder"))
            {
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

    eprintln!("Removed {} Yahoo stream skeleton card(s)", remove_set.len());
}

/// Switch Wikipedia's root `client-nojs` class to `client-js".
///
/// Wikipedia's Vector skin uses the `client-nojs` class when JavaScript is
/// unavailable and relies on it to keep collapsible sections (navboxes,
/// reference columns, motto lists) expanded. Real browsers with JS enabled
/// use `client-js` and the corresponding CSS rules collapse those sections,
/// producing a much shorter and more usable page. Incognidium's JS engine does
/// not run the startup script that flips this class, so we flip it explicitly
/// before style resolution. This also makes `.client-js`-qualified CSS rules
/// for hiding menus, dropdowns, and other JS-only chrome take effect.
pub fn fix_wikipedia_client_nojs(doc: &mut Document, base_url: &str) {
    let lower = base_url.to_ascii_lowercase();
    let is_wiki = lower.contains("wikipedia.org") || lower.contains("wikimedia.org");
    if !is_wiki {
        return;
    }

    let mut changed = 0usize;
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &mut doc.node_mut(id).data {
            let tag = el.tag_name.as_str();
            if tag != "html" && tag != "body" {
                continue;
            }
            let cls = el.classes();
            if !cls.iter().any(|c| *c == "client-nojs") {
                continue;
            }
            let new_classes: Vec<&str> = cls
                .into_iter()
                .map(|c| if c == "client-nojs" { "client-js" } else { c })
                .collect();
            el.attributes
                .insert("class".to_string(), new_classes.join(" "));
            changed += 1;
        }
    }

    if changed > 0 {
        eprintln!(
            "Switched {} Wikipedia root element(s) from client-nojs to client-js",
            changed
        );
    }
}

/// Strip lazy-image skeleton wrappers that would otherwise paint as large gray
/// blocks when their images are not loaded.
///
/// Sites such as AOL and Yahoo use Tailwind-style wrappers like
/// `<div class="w-full aspect-[16/9] bg-gray-100 animate-pulse"><img loading="lazy" ...></div>`.
/// The `onload` handler that removes the skeleton classes never fires in the
/// headless renderer, so every article card reserves a 500+px gray rectangle.
/// This helper removes the skeleton background and aspect-ratio classes from
/// such wrappers and turns the contained `<img>` into `loading="eager"`, so the
/// wrapper either renders the loaded image or collapses cleanly.
pub fn strip_lazy_image_skeletons(doc: &mut Document) {
    let mut to_strip: Vec<(incognidium_dom::NodeId, String)> = Vec::new();

    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            let cls = el.classes();
            let has_skeleton_bg = cls.iter().any(|c| {
                c.starts_with("bg-gray-")
                    || c.starts_with("bg-slate-")
                    || c.starts_with("bg-zinc-")
                    || c.starts_with("bg-neutral-")
                    || c.starts_with("bg-stone-")
                    || c.starts_with("bg-carbon-")
            });
            // Tailwind animation utilities used for skeleton pulses. Match both the
            // base class and prefixed variants (`motion-safe:animate-pulse`, etc.).
            let has_pulse = cls.iter().any(|c| {
                *c == "animate-pulse"
                    || c.ends_with(":animate-pulse")
                    || *c == "animate-ping"
                    || c.ends_with(":animate-ping")
                    || *c == "animate-bounce"
                    || c.ends_with(":animate-bounce")
            });
            let has_shimmer = cls.iter().any(|c| c.starts_with("shimmer_"));
            if !has_skeleton_bg && !has_pulse && !has_shimmer {
                continue;
            }
            // Only strip if the wrapper contains a lazy <img>.
            let has_lazy_img = doc.nodes[id].children.iter().any(|&cid| {
                if let NodeData::Element(child_el) = &doc.nodes[cid].data {
                    child_el.tag_name == "img" && child_el.get_attr("loading") == Some("lazy")
                } else {
                    false
                }
            });
            if !has_lazy_img {
                continue;
            }
            let new_classes: Vec<&str> = cls
                .into_iter()
                .filter(|c| {
                    !c.starts_with("aspect-")
                        && !c.starts_with("bg-gray-")
                        && !c.starts_with("bg-slate-")
                        && !c.starts_with("bg-zinc-")
                        && !c.starts_with("bg-neutral-")
                        && !c.starts_with("bg-stone-")
                        && !c.starts_with("bg-carbon-")
                        && !c.starts_with("shimmer_")
                        && !(*c == "animate-pulse"
                            || (*c).ends_with(":animate-pulse")
                            || *c == "animate-ping"
                            || (*c).ends_with(":animate-ping")
                            || *c == "animate-bounce"
                            || (*c).ends_with(":animate-bounce"))
                })
                .collect();
            to_strip.push((id, new_classes.join(" ")));
        }
    }

    let stripped_count = to_strip.len();
    let wrapper_ids: std::collections::HashSet<incognidium_dom::NodeId> =
        to_strip.iter().map(|(id, _)| *id).collect();

    for (id, new_classes) in to_strip {
        if let NodeData::Element(el) = &mut doc.node_mut(id).data {
            if new_classes.is_empty() {
                el.attributes.remove("class");
            } else {
                el.attributes.insert("class".to_string(), new_classes);
            }
        }
    }

    // Convert only the lazy images inside the stripped skeleton wrappers to eager,
    // so we don't blow through the global image-fetch cap for every lazy image
    // on unrelated sites.
    let mut eager_count = 0usize;
    let mut stack: Vec<incognidium_dom::NodeId> = wrapper_ids.iter().copied().collect();
    while let Some(id) = stack.pop() {
        if let NodeData::Element(el) = &mut doc.node_mut(id).data {
            if el.tag_name == "img" && el.get_attr("loading") == Some("lazy") {
                el.attributes
                    .insert("loading".to_string(), "eager".to_string());
                eager_count += 1;
            }
        }
        for &cid in &doc.nodes[id].children {
            stack.push(cid);
        }
    }

    if stripped_count > 0 || eager_count > 0 {
        eprintln!(
            "Stripped {} lazy-image skeleton(s), converted {} image(s) to eager",
            stripped_count, eager_count
        );
    }
}

/// Strip inline `background-color` styles from image wrappers that act as lazy-load
/// placeholders (e.g. Washington Post's `<a style="background-color:var(--wpds-colors-gray400)">`).
///
/// When scripting is enabled the site's JS removes these placeholders after the
/// image loads, but in the headless renderer the inline style persists and paints
/// a gray box behind/around the image.
pub fn strip_inline_bg_placeholders(doc: &mut Document) {
    let mut stripped = 0usize;
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            let Some(style) = el.get_attr("style") else {
                continue;
            };
            if !style.to_lowercase().contains("background-color") {
                continue;
            }
            // Only strip if this element directly contains an <img> with a src.
            let has_img_with_src = doc.nodes[id].children.iter().any(|&cid| {
                if let NodeData::Element(child_el) = &doc.nodes[cid].data {
                    child_el.tag_name == "img" && child_el.get_attr("src").is_some()
                } else {
                    false
                }
            });
            if !has_img_with_src {
                continue;
            }
            // Remove the background-color declaration from the inline style.
            let cleaned = remove_bg_color_from_style(style);
            if cleaned != style {
                if let NodeData::Element(el_mut) = &mut doc.node_mut(id).data {
                    if cleaned.trim().is_empty() {
                        el_mut.attributes.remove("style");
                    } else {
                        el_mut.attributes.insert("style".to_string(), cleaned);
                    }
                    stripped += 1;
                }
            }
        }
    }
    if stripped > 0 {
        eprintln!(
            "Stripped {} inline background-color placeholder(s)",
            stripped
        );
    }
}

/// Remove BBC's no-script image placeholders.
///
/// BBC article cards ship a pair of `<img>` elements: a real image with eager
/// sources and an absolute-positioned fallback that loads
/// `grey-placeholder.png` and carries the `hide-when-no-script` class. The
/// fallback is meant to be hidden by a JS-driven `display:none` rule; in our
/// no-JS renderer the rule that targets it also contains an unsupported
/// substring attribute selector, so the whole selector list is discarded and
/// the grey placeholder overlays the real photograph. Drop the placeholder
/// images from the DOM so the real images are visible.
pub fn strip_bbc_no_script_placeholders(doc: &mut Document, base_url: &str) {
    if !base_url.contains("bbc.com") {
        return;
    }

    let mut to_remove: Vec<incognidium_dom::NodeId> = Vec::new();
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            if el.tag_name == "img" {
                if let Some(src) = el.get_attr("src") {
                    if src.contains("/grey-placeholder.png") {
                        to_remove.push(id);
                    }
                }
            }
        }
    }

    if to_remove.is_empty() {
        return;
    }

    let remove_set: std::collections::HashSet<incognidium_dom::NodeId> =
        to_remove.iter().copied().collect();
    for id in 0..doc.nodes.len() {
        doc.nodes[id]
            .children
            .retain(|cid| !remove_set.contains(cid));
    }

    eprintln!(
        "Removed {} BBC no-script placeholder image(s)",
        to_remove.len()
    );
}

/// Remove Business Insider's lazy-loading placeholder article cards.
///
/// BI's "Latest", "Featured", "Videos", "Markets", etc. feeds are
/// server-rendered with empty `<article class="tout ... as-placeholder">`
/// skeletons. The cards contain the generic tagline
/// "Business Insider tells the innovative stories you want to know" and a
/// loading background that should be replaced by real stories once JS runs.
/// Without JS they stay in the DOM and render as repeated blue/grey boxes that
/// pollute both the screenshot and the extracted text. Drop them before layout.
pub fn remove_business_insider_placeholders(doc: &mut Document, base_url: &str) {
    if !base_url.contains("businessinsider.com") {
        return;
    }

    let mut to_remove: Vec<incognidium_dom::NodeId> = Vec::new();
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            if el.tag_name == "article" {
                if let Some(class) = el.get_attr("class") {
                    if class.contains("as-placeholder") {
                        to_remove.push(id);
                    }
                }
            }
        }
    }

    if to_remove.is_empty() {
        return;
    }

    let remove_set: std::collections::HashSet<incognidium_dom::NodeId> =
        to_remove.iter().copied().collect();
    for id in 0..doc.nodes.len() {
        doc.nodes[id]
            .children
            .retain(|cid| !remove_set.contains(cid));
    }

    eprintln!(
        "Removed {} Business Insider placeholder article(s)",
        to_remove.len()
    );
}

/// The Verge's Next.js image cards duplicate each article title: the visible
/// `<h2>` heading and the paired `<img data-nimg="fill" alt="...">` both
/// contain the same text. Incognidium renders the `alt` text as an extra text
/// box on top of/below the image, so every card shows the title twice. Empty
/// the `alt` attribute on Verge images before layout; the real heading still
/// provides the title.
/// Strip `alt` attributes from images whose captions already contain the same
/// text. NPR renders each photo with an `<img alt="...">` and a following
/// `<div class="credit-caption"><div class="caption">...</div></div>` that
/// repeats the exact description. Incognidium lays out both as text boxes, so
/// every caption appears twice. Empty the `alt` on images that sit inside a
/// `.credit-caption` sibling structure (or any figure where the alt is mirrored
/// by a visible caption) so only the caption remains.
pub fn strip_duplicate_img_alt_text(doc: &mut Document, base_url: &str) {
    if !base_url.contains("npr.org") && !base_url.contains("theverge.com") {
        return;
    }

    let mut stripped = 0usize;
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            if el.tag_name != "img" {
                continue;
            }
            let alt = el.get_attr("alt").unwrap_or("");
            if alt.is_empty() {
                continue;
            }
            // Walk ancestors to find a figure/picture wrapper, then inspect siblings
            // for a caption element whose text duplicates the alt.
            let mut cur = doc.nodes[id].parent;
            let mut found_caption_dup = false;
            while let Some(pid) = cur {
                if let NodeData::Element(parent_el) = &doc.nodes[pid].data {
                    // For NPR, the immediate wrapper is usually a <div> or <figure>
                    // containing the <picture>/<img> and the .credit-caption div.
                    for &cid in &doc.nodes[pid].children {
                        if cid == id {
                            continue;
                        }
                        if let NodeData::Element(sib) = &doc.nodes[cid].data {
                            if sib
                                .get_attr("class")
                                .unwrap_or("")
                                .contains("credit-caption")
                                || sib.tag_name == "figcaption"
                            {
                                if subtree_contains_text(doc, cid, alt) {
                                    found_caption_dup = true;
                                    break;
                                }
                            }
                        }
                    }
                    // Also accept a figcaption child anywhere inside the figure.
                    if !found_caption_dup {
                        for &cid in &doc.nodes[pid].children {
                            if cid == id {
                                continue;
                            }
                            found_caption_dup = subtree_has_figcaption_with_text(doc, cid, alt);
                            if found_caption_dup {
                                break;
                            }
                        }
                    }
                    if found_caption_dup
                        || parent_el.tag_name == "figure"
                        || parent_el
                            .get_attr("class")
                            .unwrap_or("")
                            .contains("credit-caption")
                    {
                        // Stop walking once we've inspected the likely figure wrapper.
                        break;
                    }
                }
                cur = doc.nodes[pid].parent;
            }
            if found_caption_dup {
                if let NodeData::Element(el_mut) = &mut doc.node_mut(id).data {
                    el_mut.attributes.insert("alt".to_string(), "".to_string());
                    stripped += 1;
                }
            }
        }
    }

    if stripped > 0 {
        eprintln!("Stripped {} duplicate image alt attribute(s)", stripped);
    }
}

fn subtree_contains_text(doc: &Document, node_id: incognidium_dom::NodeId, text: &str) -> bool {
    if let NodeData::Text(t) = &doc.nodes[node_id].data {
        if t.content.trim().contains(text) {
            return true;
        }
    }
    for &cid in &doc.nodes[node_id].children {
        if subtree_contains_text(doc, cid, text) {
            return true;
        }
    }
    false
}

fn subtree_has_figcaption_with_text(
    doc: &Document,
    node_id: incognidium_dom::NodeId,
    text: &str,
) -> bool {
    if let NodeData::Element(el) = &doc.nodes[node_id].data {
        if el.tag_name == "figcaption" && subtree_contains_text(doc, node_id, text) {
            return true;
        }
    }
    for &cid in &doc.nodes[node_id].children {
        if subtree_has_figcaption_with_text(doc, cid, text) {
            return true;
        }
    }
    false
}

/// Fix Next.js `data-nimg="fill"` images that are hidden by CSS classes
/// (e.g. `._1ismqjf{display:none}`) because the JS that toggles visibility
/// never runs in the headless renderer.
pub fn fix_nextjs_fill_images(doc: &mut Document) {
    let mut fixed = 0usize;
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            if el.tag_name != "img" {
                continue;
            }
            if el.get_attr("data-nimg") != Some("fill") {
                continue;
            }
            let has_src = el.get_attr("src").map(|s| !s.is_empty()).unwrap_or(false);
            if !has_src {
                continue;
            }
            // Ensure the image is visible by adding display:block to inline style.
            let style = el.get_attr("style").unwrap_or("");
            let lower = style.to_lowercase();
            if lower.contains("display:none") {
                // Replace display:none with display:block
                let new_style = lower
                    .split(';')
                    .map(|s| {
                        let t = s.trim();
                        if t.starts_with("display:none") {
                            "display: block"
                        } else {
                            t
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                if let NodeData::Element(el_mut) = &mut doc.node_mut(id).data {
                    el_mut.attributes.insert("style".to_string(), new_style);
                    fixed += 1;
                }
            } else if !lower.contains("display:") {
                // No display property at all - add display:block
                let new_style = if style.trim().is_empty() {
                    "display: block".to_string()
                } else {
                    format!("{}; display: block", style.trim_end_matches(';'))
                };
                if let NodeData::Element(el_mut) = &mut doc.node_mut(id).data {
                    el_mut.attributes.insert("style".to_string(), new_style);
                    fixed += 1;
                }
            }
        }
    }
    if fixed > 0 {
        eprintln!("Fixed {} Next.js data-nimg='fill' image(s)", fixed);
    }
}

/// NYTimes renders article images as a lazy-loading placeholder (`css-dzl7b5`)
/// paired with a real image (`css-122y91a`). The real image is hidden by CSS
/// `display:none` and the placeholder is invisible (`opacity:0`). Without the
/// browser's intersection-observer logic the real image never gets revealed.
///
/// In some render variations the real image (`css-122y91a`) is absent and the
/// placeholder (`css-dzl7b5`) is promoted from a `<noscript>` fallback.  In that
/// case the placeholder now carries a real `src` and must be kept visible.
pub fn fix_nytimes_lazy_images(doc: &mut Document, base_url: &str) {
    let is_nytimes = base_url.to_ascii_lowercase().contains("nytimes.com");
    if !is_nytimes {
        return;
    }

    enum Action {
        SetStyle(String), // css-dzl7b5 with src  -> set inline style
        RemoveClass,      // css-122y91a with src  -> strip the class
        RemoveNode,       // css-dzl7b5 no src     -> delete from tree
    }

    let mut actions: Vec<(incognidium_dom::NodeId, Action)> = Vec::new();

    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            if el.tag_name != "img" {
                continue;
            }
            let class = el.get_attr("class").unwrap_or("");
            let has_src = el.get_attr("src").map(|s| !s.is_empty()).unwrap_or(false);

            if class.contains("css-dzl7b5") {
                if has_src {
                    let style = el.get_attr("style").unwrap_or("");
                    let new_style = if style.trim().is_empty() {
                        "display: block; opacity: 1".to_string()
                    } else {
                        format!(
                            "{}; display: block; opacity: 1",
                            style.trim_end_matches(';')
                        )
                    };
                    actions.push((id, Action::SetStyle(new_style)));
                } else {
                    actions.push((id, Action::RemoveNode));
                }
            } else if class.contains("css-122y91a") && has_src {
                // This is the `<noscript>` fallback image that was promoted to the
                // sibling `<img class="css-dzl7b5">`. Keeping it creates a duplicate
                // image box, so remove the fallback node entirely.
                actions.push((id, Action::RemoveNode));
            } else if class.contains("css-1ii2lp6") && has_src {
                // Author thumbnails start at opacity:0 for lazy fade-in – force visible.
                let style = el.get_attr("style").unwrap_or("");
                let new_style = if style.trim().is_empty() {
                    "opacity: 1".to_string()
                } else {
                    format!("{}; opacity: 1", style.trim_end_matches(';'))
                };
                actions.push((id, Action::SetStyle(new_style)));
            }
        }
    }

    let mut fixed = 0usize;
    let mut to_remove: Vec<incognidium_dom::NodeId> = Vec::new();

    for (id, action) in actions {
        match action {
            Action::SetStyle(new_style) => {
                if let NodeData::Element(el_mut) = &mut doc.node_mut(id).data {
                    el_mut.attributes.insert("style".to_string(), new_style);
                    fixed += 1;
                }
            }
            Action::RemoveClass => {
                if let NodeData::Element(el_mut) = &mut doc.node_mut(id).data {
                    let class = el_mut.get_attr("class").unwrap_or("");
                    let new_class = class
                        .split_whitespace()
                        .filter(|c| *c != "css-122y91a")
                        .collect::<Vec<_>>()
                        .join(" ");
                    if new_class.is_empty() {
                        el_mut.attributes.remove("class");
                    } else {
                        el_mut.attributes.insert("class".to_string(), new_class);
                    }
                    fixed += 1;
                }
            }
            Action::RemoveNode => {
                to_remove.push(id);
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

    if fixed > 0 || !to_remove.is_empty() {
        eprintln!(
            "Fixed {} NYTimes lazy image(s), removed {} placeholder(s)",
            fixed,
            to_remove.len()
        );
    }
}

/// Remove `background-color: ...;` (and variants) from a CSS style string.
fn remove_bg_color_from_style(style: &str) -> String {
    let mut result = String::new();
    for decl in style.split(';') {
        let trimmed = decl.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_lowercase();
        if lower.starts_with("background-color") {
            continue;
        }
        if !result.is_empty() {
            result.push_str("; ");
        }
        result.push_str(trimmed);
    }
    result
}

/// Promote lazy-loaded `<img>` sources so they render without JS.
///
/// Many themes/plugins set `src` to a 1x1 transparent placeholder and put the
/// real URL in `data-src`, hiding the image with `.lazyload[data-src]{display:none}`.
/// Without JS the lazy loader never swaps the attributes, so article thumbnails and
/// hero images stay invisible. This helper copies `data-src` to `src`, copies
/// `data-srcset` to `srcset`, flips the lazy class to `lazyloaded`, and forces
/// `loading="eager"` so the image is fetched and laid out like a normal `<img>`.
pub fn promote_lazy_image_sources(doc: &mut Document) {
    let mut promoted = 0usize;
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &mut doc.node_mut(id).data {
            if el.tag_name != "img" {
                continue;
            }
            // Read all attributes into owned locals before mutating the element.
            let class = el.get_attr("class").unwrap_or("").to_string();
            let src = el.get_attr("src").unwrap_or("").to_string();
            let data_src = el.get_attr("data-src").map(String::from);
            let data_default_src = el.get_attr("data-default-src").map(String::from);
            let data_srcset = el.get_attr("data-srcset").map(String::from);
            let loading = el.get_attr("loading").map(String::from);
            let has_srcset = el.get_attr("srcset").is_some();

            // Use the real source when the current src is missing or looks like a
            // tiny placeholder (data URI, about:blank, or a known transparent GIF
            // such as Fox News's clear-16x9.gif).
            let src_is_placeholder = src.is_empty()
                || src.starts_with("data:")
                || src == "about:blank"
                || src == "about:srcdoc"
                || src.ends_with("clear-16x9.gif")
                || src.ends_with("blank.gif")
                || src.ends_with("spacer.gif")
                || src.ends_with("transparent.gif");

            // Some lazy-load libraries (e.g. WIRED's responsive-image) use data-src
            // without the standard lazyload/lazyloading class names. Promote any
            // image that has a data-src and a missing/placeholder src.
            // ESPN uses data-default-src for its lazy-loaded article images.
            let should_promote =
                (data_src.is_some() || data_default_src.is_some()) && src_is_placeholder;

            let classes: Vec<&str> = class.split_whitespace().collect();
            let has_lazy_class = classes
                .iter()
                .any(|c| *c == "lazyload" || *c == "lazyloading");

            // CSS-module-style lazy classes (e.g. The Atlantic's Image_lazy__hYWHV)
            // hide the image with opacity:0 until JS adds a loaded class.
            let has_css_module_lazy = classes
                .iter()
                .any(|c| c.contains("_lazy__") || c.contains("-lazy-"));
            let has_lazy_loading = loading.as_deref() == Some("lazy");

            if !should_promote && !has_lazy_class && !has_css_module_lazy {
                continue;
            }

            // Prefer data-src over data-default-src when both are present.
            let real_src = data_src.or(data_default_src);
            if let Some(real) = real_src {
                if src_is_placeholder {
                    el.attributes.insert("src".to_string(), real);
                    promoted += 1;
                }
            }
            if let Some(realset) = data_srcset {
                if !has_srcset {
                    el.attributes.insert("srcset".to_string(), realset);
                }
            }

            // Swap lazy classes to lazyloaded so the `.lazyload[data-src]{display:none}`
            // rule no longer matches and the image becomes visible.
            // Also strip CSS-module lazy classes that set opacity:0.
            let new_classes: Vec<&str> = classes
                .iter()
                .filter(|c| !c.contains("_lazy__") && !c.contains("-lazy-"))
                .map(|c| match *c {
                    "lazyload" | "lazyloading" => "lazyloaded",
                    _ => *c,
                })
                .collect();
            let new_class = new_classes.join(" ");
            if new_class.is_empty() {
                el.attributes.remove("class");
            } else {
                el.attributes.insert("class".to_string(), new_class);
            }

            // Ensure the image is fetched even if it was originally below the fold.
            if has_lazy_loading {
                el.attributes
                    .insert("loading".to_string(), "eager".to_string());
            }
        }
    }
    if promoted > 0 {
        eprintln!("Promoted {} lazy image source(s) to src", promoted);
    }
}

/// Remove hidden Bootstrap-style dropdown menus that contain login/account UI.
///
/// Sites such as phys.org server-render the account dropdown menu inside
/// `<div class="dropdown">` with `aria-expanded="false"`. The real menu is
/// hidden by `pointer-events: none` and `opacity: 0` until the parent gets the
/// `.show` class. Incognidium's layout/paint pipeline does not fully suppress
/// `pointer-events:none` and applies opacity only to backgrounds, so the login
/// form ("Science X Account", email, password, "Sign In") is rendered inline.
///
/// This helper removes any `.dropdown-menu` (or element with `role="menu"`) that
/// is inside a `.dropdown` without `.show` and whose text matches common
/// login/account keywords. This matches the real browser initial view while
/// preserving the account icon trigger.
pub fn remove_hidden_login_dropdowns(doc: &mut Document, base_url: &str) {
    let lower = base_url.to_ascii_lowercase();
    let is_target = lower.contains("phys.org")
        || lower.contains("sciencex.com")
        || lower.contains("medicalxpress.com")
        || lower.contains("techxplore.com");
    if !is_target {
        return;
    }

    const LOGIN_KEYWORDS: [&str; 8] = [
        "sign in",
        "sign up",
        "log in",
        "login",
        "password",
        "email",
        "account",
        "forgot password",
    ];

    fn collect_text(doc: &Document, id: incognidium_dom::NodeId, out: &mut String) {
        let node = &doc.nodes[id];
        match &node.data {
            incognidium_dom::NodeData::Text(t) => {
                out.push_str(&t.content);
                out.push(' ');
            }
            incognidium_dom::NodeData::Element(el) => {
                if matches!(el.tag_name.as_str(), "script" | "style" | "noscript") {
                    return;
                }
                for attr in ["placeholder", "aria-label", "title"] {
                    if let Some(v) = el.get_attr(attr) {
                        out.push_str(v);
                        out.push(' ');
                    }
                }
                for &cid in &node.children {
                    collect_text(doc, cid, out);
                }
            }
            _ => {}
        }
    }

    fn is_hidden_dropdown(parent_id: incognidium_dom::NodeId, doc: &Document) -> bool {
        let node = &doc.nodes[parent_id];
        if let incognidium_dom::NodeData::Element(el) = &node.data {
            let classes: std::collections::HashSet<&str> = el.classes().into_iter().collect();
            if classes.contains("dropdown") && !classes.contains("show") {
                return true;
            }
        }
        // Also remove menus whose immediate toggle has aria-expanded="false".
        if let incognidium_dom::NodeData::Element(parent_el) = &node.data {
            for &cid in &node.children {
                if let incognidium_dom::NodeData::Element(el) = &doc.nodes[cid].data {
                    if el.tag_name == "a" && el.get_attr("data-toggle") == Some("dropdown") {
                        if el.get_attr("aria-expanded") == Some("false") {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    let mut to_remove: Vec<incognidium_dom::NodeId> = Vec::new();
    for id in 0..doc.nodes.len() {
        if let incognidium_dom::NodeData::Element(el) = &doc.nodes[id].data {
            let classes: std::collections::HashSet<&str> = el.classes().into_iter().collect();
            let is_menu = classes.contains("dropdown-menu") || el.get_attr("role") == Some("menu");
            if !is_menu {
                continue;
            }
            let parent_id = match doc.nodes[id].parent {
                Some(p) => p,
                None => continue,
            };
            if !is_hidden_dropdown(parent_id, doc) {
                continue;
            }
            let mut text = String::new();
            collect_text(doc, id, &mut text);
            let text_lower = text.to_ascii_lowercase();
            if LOGIN_KEYWORDS.iter().any(|kw| text_lower.contains(kw)) {
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
    eprintln!("Removed {} hidden login dropdown menu(s)", remove_set.len());
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
    // Precompute which nodes contain a visual replaced element (image, video,
    // SVG, canvas, etc.) somewhere in their subtree. Accessibility-only
    // wrappers are often marked `aria-hidden="true"`, but many of those wrappers
    // carry real visual content such as article cover images. Don't strip them.
    let mut has_visual_descendant: Vec<bool> = vec![false; doc.nodes.len()];
    for id in (0..doc.nodes.len()).rev() {
        if let incognidium_dom::NodeData::Element(ref el) = doc.nodes[id].data {
            if matches!(
                el.tag_name.as_str(),
                "img"
                    | "svg"
                    | "video"
                    | "audio"
                    | "canvas"
                    | "picture"
                    | "iframe"
                    | "object"
                    | "embed"
            ) {
                has_visual_descendant[id] = true;
            }
        }
        if has_visual_descendant[id] {
            if let Some(parent_id) = doc.nodes[id].parent {
                has_visual_descendant[parent_id] = true;
            }
        }
    }

    fn has_ancestor_with_class(
        doc: &incognidium_dom::Document,
        id: incognidium_dom::NodeId,
        target: &str,
    ) -> bool {
        let mut cur = doc.nodes[id].parent;
        while let Some(pid) = cur {
            if let incognidium_dom::NodeData::Element(parent_el) = &doc.nodes[pid].data {
                if parent_el
                    .get_attr("class")
                    .unwrap_or("")
                    .split_whitespace()
                    .any(|c| c == target)
                {
                    return true;
                }
            }
            cur = doc.nodes[pid].parent;
        }
        false
    }

    fn is_placeholder(
        el: &incognidium_dom::ElementData,
        has_visual_descendant: bool,
        doc: &incognidium_dom::Document,
        node_id: incognidium_dom::NodeId,
    ) -> bool {
        // Inline SVGs are visual replaced elements even when `aria-hidden`.
        // Removing them as "placeholders" strips logos and icons from the page.
        if el.tag_name == "svg" {
            return false;
        }
        let classes: std::collections::HashSet<&str> = el.classes().into_iter().collect();
        const PLACEHOLDER_CLASSES: [&str; 38] = [
            "markupbox",
            "ad",
            "ads",
            "ad-slot",
            "ad__placeholder",
            "ad-placeholder",
            "ad-container",
            "ad-wrapper",
            "ad-wrap",
            "ad-unit",
            "advertisement",
            "sponsored",
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
            // Generic skeleton/loading indicators found on GCS sites (e.g. Target,
            // Yahoo, CNET, The Register). Real browsers either replace these with
            // content via JS or hide them; in the headless renderer they often
            // render as empty blocks or spinner text.
            "placeholder",
            "skeleton",
            "shimmer",
            "loading",
            "loader",
            "spinner",
            // Tailwind utility for fully transparent/invisible elements. Real
            // browsers still keep them in the accessibility tree, but in a static
            // screenshot they contribute no visual content and often leave empty
            // boxes or off-screen wrappers (e.g. collapsed nav dropdowns).
            "opacity-0",
            // Site-specific skeleton/placeholder classes observed in recent GCS
            // snapshots.
            "yahoo-nebula-ad-placeholder-image",
            "styles_ndsPlaceholder__XOx9j",
            "styles_tilePlaceholderContainer__kSpvO",
            "wp-block-zd-newsletter-cta__loading-text",
        ];
        // Tailwind/Bootstrap-style responsive display pattern: an element that is
        // `hidden` by default but shown at a breakpoint (e.g. `hidden lg:flex`)
        // is real content, not a placeholder. Keep it so the layout matches the
        // responsive breakpoint the stylesheet applies at.
        if classes.contains("hidden") {
            const RESPONSIVE_BREAKPOINTS: [&str; 5] = ["sm:", "md:", "lg:", "xl:", "xxl:"];
            const RESPONSIVE_DISPLAYS: [&str; 8] = [
                "flex",
                "inline-flex",
                "block",
                "grid",
                "inline",
                "inline-block",
                "table",
                "contents",
            ];
            let has_responsive_display = classes.iter().any(|c| {
                RESPONSIVE_BREAKPOINTS.iter().any(|bp| {
                    c.strip_prefix(bp)
                        .map(|rest| RESPONSIVE_DISPLAYS.contains(&rest))
                        .unwrap_or(false)
                })
            });
            if has_responsive_display {
                return false;
            }
        }

        if classes.iter().any(|c| PLACEHOLDER_CLASSES.contains(c)) {
            return true;
        }
        // Styled-components / CSS-module hashed ad/skeleton classes (e.g. BBC's
        // `AdSlot-styles__AdSlotContainerStyled-sc-...`) embed the placeholder
        // token inside a longer class name. Match those tokens case-insensitively
        // so the exact list above does not need to enumerate every hashed variant.
        const PLACEHOLDER_SUBSTRINGS: [&str; 8] = [
            "adslot",
            "adsbygoogle",
            "dfp-ad",
            "taboola",
            "outbrain",
            "advertisement",
            // Guardian and other publishers wrap blocked ad slots in containers
            // like `.top-banner-ad-container` and `.ad-slot-container`.
            // Match the hyphenated tokens so these empty wrappers are removed.
            "ad-container",
            "ad-slot",
        ];
        if classes.iter().any(|c| {
            let c = c.to_ascii_lowercase();
            PLACEHOLDER_SUBSTRINGS.iter().any(|p| c.contains(p))
        }) {
            // The Guardian's top-of-page banner ad container is server-rendered
            // with a CSS pseudo-element placeholder. Even when the inner ad slot is
            // empty, the container itself is meant to be visible so the rest of
            // the header aligns with the browser reference. Keep it.
            let is_top_banner = classes
                .iter()
                .any(|c| c.to_ascii_lowercase().contains("top-banner"))
                || has_ancestor_with_class(doc, node_id, "top-banner-ad-container");
            if is_top_banner {
                return false;
            }
            return true;
        }
        // CSS-module hashed skeleton classes (e.g. Yahoo's `shimmer_shimmer__GgM0s`)
        // are not matched by the exact list above. Treat any class that starts with a
        // known skeleton/placeholder prefix as a placeholder.
        const PLACEHOLDER_PREFIXES: [&str; 2] = ["shimmer_", "cls-stream-placeholder"];
        if classes
            .iter()
            .any(|c| PLACEHOLDER_PREFIXES.iter().any(|p| c.starts_with(p)))
        {
            return true;
        }
        if let Some(v) = el.get_attr("aria-hidden") {
            if v == "true" && !has_visual_descendant {
                return true;
            }
        }
        // NYTimes ad slots use data-testid="StandardAd" and render as empty
        // placeholders when the ad/tracking scripts are blocked. Other publishers
        // use similar test-id patterns for ad slots. Keep this list ad-specific:
        // generic names like "loading" or "placeholder" can appear on real content
        // containers (e.g. Walmart's homepage section).
        if let Some(v) = el.get_attr("data-testid") {
            if v == "StandardAd" || v == "ad" || v == "ad-slot" || v == "advertisement" {
                return true;
            }
        }
        // Walmart's server-rendered homepage contains a large "loading home page"
        // section full of skeleton placeholders below the real content. The generic
        // "loading" substring check was too broad, but this exact label is a true
        // placeholder.
        if let Some(v) = el.get_attr("aria-label") {
            if v.eq_ignore_ascii_case("loading home page") {
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
            if is_placeholder(el, has_visual_descendant[id], doc, id) {
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

            // Remove the standalone game hub; keep article lists and the main
            // right-rail column that holds the bulk of the homepage article cards.
            if class_str.contains("collection game-hub") {
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
            let keep = if is_video_items {
                1
            } else if is_load_more {
                6
            } else {
                8
            };
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

    // mdBook's real desktop layout shows the sidebar at the left and offsets
    // the page wrapper. The headless renderer does not fully support the CSS
    // custom properties (--sidebar-width) and :checked-sibling selectors that
    // mdBook uses, so apply explicit inline overrides after injecting the TOC.
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
            el.attributes.insert(
                "style".to_string(),
                "transform:none;width:200px".to_string(),
            );
        }
    }
    if let Some(toggle_id) = doc.get_element_by_id("mdbook-sidebar-toggle-anchor") {
        if let NodeData::Element(el) = &mut doc.nodes[toggle_id].data {
            // Drop the "hidden" class so the generic placeholder trimmer does not
            // remove this real toggle. Keep it visually hidden with an inline style.
            el.attributes.remove("class");
            el.attributes
                .insert("style".to_string(), "display:none".to_string());
        }
    }
    if let Some(page_wrapper_id) = (0..doc.nodes.len()).find(|&id| {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            el.tag_name == "div" && el.classes().contains(&"page-wrapper")
        } else {
            false
        }
    }) {
        if let NodeData::Element(el) = &mut doc.nodes[page_wrapper_id].data {
            el.attributes.insert(
                "style".to_string(),
                "transform:none;margin-left:200px".to_string(),
            );
        }
    }

    // The fixed-position chapter-navigation arrows are JS/fixed-layout chrome.
    // Without fixed positioning they flow as large blocks at the end of the
    // static render and push content around. Hide them; inline Next/Previous
    // links and the sidebar TOC still allow navigation.
    for id in 0..doc.nodes.len() {
        if let NodeData::Element(el) = &doc.nodes[id].data {
            if el.tag_name == "a"
                && (el.classes().contains(&"nav-chapters")
                    || el.classes().contains(&"mobile-nav-chapters"))
            {
                if let NodeData::Element(el) = &mut doc.nodes[id].data {
                    el.attributes
                        .insert("style".to_string(), "display:none".to_string());
                }
            }
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
    external_defs: &HashMap<String, (String, Option<String>)>,
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
                                serialize_svg_subtree(
                                    doc,
                                    child_id,
                                    out,
                                    defs,
                                    external_defs,
                                    depth + 1,
                                );
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
                                    serialize_svg_subtree(
                                        doc,
                                        child_id,
                                        out,
                                        defs,
                                        external_defs,
                                        depth + 1,
                                    );
                                }
                                out.push_str("</");
                                out.push_str(&target_el.tag_name);
                                out.push('>');
                            }
                        }
                        return;
                    }
                }

                // Expand `<use href="url#id">` references that point at external SVG
                // sprite sheets.  The sprite is fetched once, the referenced symbol is
                // extracted and cached as a standalone SVG fragment, and inlined here.
                if let Some((fragment, viewbox)) = external_defs.get(&href) {
                    let x = el.attributes.get("x").and_then(|s| s.parse::<f32>().ok());
                    let y = el.attributes.get("y").and_then(|s| s.parse::<f32>().ok());
                    let width = el.attributes.get("width").cloned();
                    let height = el.attributes.get("height").cloned();
                    let use_transform = el.attributes.get("transform").cloned().unwrap_or_default();

                    // Emit a nested SVG viewport that preserves the referenced
                    // symbol's viewBox so scaling/aspect ratio matches the real
                    // browser.  The original `<use>` x/y/width/height become the
                    // nested SVG's geometry; if width/height are absent we let the
                    // nested SVG fill the referencing viewport.
                    out.push_str("<svg");
                    if let Some(xv) = x {
                        out.push_str(" x=\"");
                        out.push_str(&escape_xml_attr(&format!("{}", xv)));
                        out.push('"');
                    }
                    if let Some(yv) = y {
                        out.push_str(" y=\"");
                        out.push_str(&escape_xml_attr(&format!("{}", yv)));
                        out.push('"');
                    }
                    if let Some(w) = &width {
                        out.push_str(" width=\"");
                        out.push_str(&escape_xml_attr(w));
                        out.push('"');
                    }
                    if let Some(h) = &height {
                        out.push_str(" height=\"");
                        out.push_str(&escape_xml_attr(h));
                        out.push('"');
                    }
                    if let Some(vb) = viewbox {
                        out.push_str(" viewBox=\"");
                        out.push_str(&escape_xml_attr(vb));
                        out.push('"');
                        out.push_str(" preserveAspectRatio=\"xMidYMid meet\"");
                    }
                    if !use_transform.is_empty() {
                        out.push_str(" transform=\"");
                        out.push_str(&escape_xml_attr(&use_transform));
                        out.push('"');
                    }
                    out.push_str(" xmlns=\"http://www.w3.org/2000/svg\">");
                    out.push_str(fragment);
                    out.push_str("</svg>");
                    return;
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
                    serialize_svg_subtree(doc, child_id, out, defs, external_defs, depth + 1);
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

/// Inject presentation attributes (fill, stroke, etc.) into the root `<svg>`
/// tag of a serialized SVG fragment.  This ensures computed CSS styles that
/// apply to the inline `<svg>` element in the HTML document are visible to
/// `usvg` when it rasterizes the standalone XML document.
fn inject_svg_presentation_attrs(svg: &mut String, attrs: &[(String, String)]) {
    if attrs.is_empty() {
        return;
    }
    let Some(svg_start) = svg.find("<svg") else {
        return;
    };
    let after_tag = &svg[svg_start + 4..];
    // Find the closing `>` of the opening `<svg …>` tag.
    let Some(close_idx) = after_tag.find('>') else {
        return;
    };
    let tag_end = svg_start + 4 + close_idx;
    let tag_content = &svg[svg_start..tag_end];

    // Strip any existing occurrences of the attributes we are about to inject
    // so that the new values take precedence (XML parsers use the first match).
    let mut cleaned_tag = tag_content.to_string();
    for (k, _) in attrs {
        // Remove ` name="value"` or ` name='value'` variants.
        // This is a simple heuristic sufficient for serialized SVG.
        let pattern_space = format!(" {}=\"", k);
        while let Some(start) = cleaned_tag.find(&pattern_space) {
            let after_eq = start + pattern_space.len();
            if let Some(end) = cleaned_tag[after_eq..].find('"') {
                cleaned_tag.drain(start..after_eq + end + 1);
            } else {
                break;
            }
        }
        let pattern_space_s = format!(" {}='", k);
        while let Some(start) = cleaned_tag.find(&pattern_space_s) {
            let after_eq = start + pattern_space_s.len();
            if let Some(end) = cleaned_tag[after_eq..].find('\'') {
                cleaned_tag.drain(start..after_eq + end + 1);
            } else {
                break;
            }
        }
    }

    let mut injection = String::new();
    for (k, v) in attrs {
        injection.push(' ');
        injection.push_str(k);
        injection.push_str("=\"");
        injection.push_str(&escape_xml_attr(v));
        injection.push('"');
    }
    // Replace the old tag content with the cleaned one + new attributes.
    let before = svg[..svg_start].to_string();
    let after = svg[tag_end..].to_string();
    *svg = format!("{}{}{}", before, cleaned_tag, injection);
    svg.push_str(&after);
}

/// Convert a resolved CSS value to a string suitable for an SVG attribute.
/// This is intentionally minimal: inline SVGs mostly need colors, lengths,
/// percentages, and keywords.
fn css_value_to_svg_string(value: &CssValue) -> String {
    match value {
        CssValue::Color(c) => css_color_to_svg(*c),
        CssValue::Keyword(k) => k.clone(),
        CssValue::Length(n, u) => {
            let unit = match u {
                incognidium_css::LengthUnit::Px => "px",
                incognidium_css::LengthUnit::Em => "em",
                incognidium_css::LengthUnit::Rem => "rem",
                incognidium_css::LengthUnit::Pt => "pt",
                incognidium_css::LengthUnit::Percent => "%",
                incognidium_css::LengthUnit::Vw => "vw",
                incognidium_css::LengthUnit::Vh => "vh",
                incognidium_css::LengthUnit::Fr => "fr",
                incognidium_css::LengthUnit::Vmin => "vmin",
                incognidium_css::LengthUnit::Vmax => "vmax",
                incognidium_css::LengthUnit::Ex => "ex",
                incognidium_css::LengthUnit::Ch => "ch",
                incognidium_css::LengthUnit::Cap => "cap",
                incognidium_css::LengthUnit::Cm => "cm",
                incognidium_css::LengthUnit::Mm => "mm",
                incognidium_css::LengthUnit::In => "in",
                incognidium_css::LengthUnit::Pc => "pc",
            };
            format!("{}{}", n, unit)
        }
        CssValue::Percentage(p) => format!("{}%", p),
        CssValue::Number(n) => format!("{}", n),
        CssValue::Auto => "auto".to_string(),
        _ => {
            // Fallback: try to emit a sensible string. For complex values this
            // will not be SVG-valid, but it preserves the original token text
            // better than silently dropping it.
            format!("{:?}", value)
        }
    }
}

/// Resolve CSS `var(--name)` references inside a serialized SVG, using the
/// element's inherited custom properties. `fallback` is used when a variable is
/// not defined. This fixes inline SVGs that set `fill="var(--icon-color)"`
/// before `usvg` rasterizes them.
fn resolve_css_vars_in_svg(
    svg: &str,
    vars: &std::collections::HashMap<String, CssValue>,
) -> String {
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(start) = rest.find("var(") {
        out.push_str(&rest[..start]);
        let inner_start = start + 4;
        if inner_start >= rest.len() {
            // Malformed; keep the remainder unchanged.
            out.push_str(&rest[start..]);
            break;
        }
        // Find the matching ')' honoring nested parentheses.
        let mut depth = 1;
        let mut inner_end = None;
        for (i, c) in rest[inner_start..].char_indices() {
            if c == '(' {
                depth += 1;
            } else if c == ')' {
                depth -= 1;
                if depth == 0 {
                    inner_end = Some(inner_start + i);
                    break;
                }
            }
        }
        let Some(inner_end) = inner_end else {
            // No closing paren; keep remainder.
            out.push_str(&rest[start..]);
            break;
        };
        let inner = &rest[inner_start..inner_end];
        let mut parts = inner.splitn(2, ',');
        let name = parts.next().unwrap_or("").trim();
        let fallback = parts.next().map(|s| s.trim());
        let replacement = vars.get(name).map(|v| css_value_to_svg_string(v));
        if let Some(repl) = replacement {
            out.push_str(&repl);
        } else if let Some(fb) = fallback {
            out.push_str(fb);
        } else {
            // Variable undefined and no fallback: leave the var() call in place
            // so the SVG still contains a recognizable token.
            out.push_str(&rest[start..=inner_end]);
        }
        rest = &rest[inner_end + 1..];
    }
    out.push_str(rest);
    out
}

fn render_svg_xml(
    svg: &str,
    current_color: CssColor,
    vars: Option<&std::collections::HashMap<String, CssValue>>,
    target_width: Option<f32>,
    target_height: Option<f32>,
) -> Option<ImageData> {
    // Inline SVGs frequently use `currentColor` for strokes/fills so they match
    // the surrounding text color. usvg alone cannot resolve CSS `currentColor`,
    // so substitute the computed (or default) color before rasterizing.
    let color_str = css_color_to_svg(current_color);
    let mut svg = svg.replace("currentColor", &color_str);
    if let Some(vars) = vars {
        svg = resolve_css_vars_in_svg(&svg, vars);
    }
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_str(&svg, &opt).ok()?;
    let size = tree.size();
    let intrinsic_w = size.width();
    let intrinsic_h = size.height();
    if intrinsic_w <= 0.0 || intrinsic_h <= 0.0 {
        return None;
    }

    // Raster at the CSS-resolved box size when one is provided; otherwise use
    // the SVG's intrinsic size. This prevents inline SVG logos from being
    // rendered at viewBox-unit scale and then down-scaled during paint, which
    // made them appear oversized in flex headers.
    let target_w = target_width.unwrap_or(intrinsic_w).max(1.0);
    let target_h = target_height.unwrap_or(intrinsic_h).max(1.0);
    let max_target_dim = target_w.max(target_h);
    let cap = if max_target_dim > MAX_INLINE_SVG_DIM {
        MAX_INLINE_SVG_DIM / max_target_dim
    } else {
        1.0
    };
    let render_w = (target_w * cap).ceil().max(1.0);
    let render_h = (target_h * cap).ceil().max(1.0);
    let scale_x = render_w / intrinsic_w;
    let scale_y = render_h / intrinsic_h;

    let w = render_w as u32;
    let h = render_h as u32;
    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    let transform = tiny_skia::Transform::from_scale(scale_x, scale_y);
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
            CalcValue::Cap(v) => v * font_size * 0.7,
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

/// Collect external SVG sprite references of the form `<use href="url#id">`.
/// Returns a list of `(sprite_url, symbol_id, use_node_id)` tuples so the caller
/// can fetch each unique sprite once and extract the referenced symbols.
fn collect_external_svg_use_refs(doc: &Document) -> Vec<(String, String, incognidium_dom::NodeId)> {
    let mut refs = Vec::new();
    for n in &doc.nodes {
        if let NodeData::Element(el) = &n.data {
            if el.tag_name != "use" {
                continue;
            }
            let href = el
                .attributes
                .get("href")
                .or_else(|| el.attributes.get("xlink:href"))
                .cloned()
                .unwrap_or_default();
            if href.starts_with('#') || !href.contains('#') {
                continue;
            }
            let hash_idx = href.rfind('#').unwrap_or(href.len());
            let sprite_url = href[..hash_idx].to_string();
            let symbol_id = href[hash_idx + 1..].to_string();
            if sprite_url.is_empty() || symbol_id.is_empty() {
                continue;
            }
            refs.push((sprite_url, symbol_id, n.id));
        }
    }
    refs
}

/// Serialize the children of an external SVG `<symbol>` node.  The wrapper is
/// dropped because it is a template, but its `viewBox` is returned so the caller
/// can create a correctly scaled nested SVG viewport in place of the original
/// `<use>`.
fn serialize_external_svg_symbol(
    ext_doc: &Document,
    symbol_id: &str,
) -> Option<(String, Option<String>)> {
    let symbol_id = symbol_id.to_string();
    let symbol_node_id = ext_doc.nodes.iter().find_map(|n| {
        if let NodeData::Element(el) = &n.data {
            if el.tag_name == "symbol" && el.attributes.get("id") == Some(&symbol_id) {
                return Some(n.id);
            }
        }
        None
    })?;
    let mut out = String::new();
    let symbol_node = &ext_doc.nodes[symbol_node_id];
    let viewbox = if let NodeData::Element(el) = &symbol_node.data {
        el.attributes.get("viewBox").cloned()
    } else {
        None
    };
    for &child_id in &symbol_node.children {
        serialize_svg_subtree(
            ext_doc,
            child_id,
            &mut out,
            &HashMap::new(),
            &HashMap::new(),
            0,
        );
    }
    Some((out, viewbox))
}

pub fn rasterize_inline_svgs(
    doc: &mut Document,
    image_cache: &mut HashMap<String, ImageData>,
    mut styles: Option<&mut StyleMap>,
    viewport_width: f32,
    viewport_height: f32,
    base_url: Option<&str>,
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

    // Fetch external SVG sprite sheets referenced by `<use href="url#id">` and
    // cache the serialized symbol fragments.  Each unique sprite URL is fetched
    // once; failures are logged and the corresponding <use> remains empty.
    let mut external_defs: HashMap<String, (String, Option<String>)> = HashMap::new();
    let external_refs = collect_external_svg_use_refs(doc);
    let mut fetched_sprites: HashMap<String, Document> = HashMap::new();
    if let Some(base) = base_url {
        for (sprite_url, symbol_id, _use_id) in external_refs {
            let full_url = match resolve_url(base, &sprite_url) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!(
                        "Failed to resolve external SVG sprite {}: {}",
                        sprite_url, e
                    );
                    continue;
                }
            };
            let ext_doc = match fetched_sprites.entry(full_url.clone()) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::hash_map::Entry::Vacant(e) => {
                    let resp = match fetch_url(&full_url) {
                        Ok(r) => r,
                        Err(err) => {
                            eprintln!("Failed to fetch external SVG sprite {}: {}", full_url, err);
                            continue;
                        }
                    };
                    if resp.status < 200 || resp.status >= 300 {
                        eprintln!(
                            "External SVG sprite {} returned HTTP {}",
                            full_url, resp.status
                        );
                        continue;
                    }
                    e.insert(parse_html(&resp.body))
                }
            };
            let href = format!("{}#{}", sprite_url, symbol_id);
            if external_defs.contains_key(&href) {
                continue;
            }
            match serialize_external_svg_symbol(ext_doc, &symbol_id) {
                Some((fragment, viewbox)) => {
                    external_defs.insert(href, (fragment, viewbox));
                }
                None => {
                    eprintln!(
                        "External SVG sprite {} has no symbol #{}",
                        full_url, symbol_id
                    );
                }
            }
        }
    }

    let mut count = 0usize;
    for id in svg_ids {
        if count >= MAX_INLINE_SVGS {
            break;
        }
        let mut svg_xml = String::new();
        serialize_svg_subtree(doc, id, &mut svg_xml, &svg_defs, &external_defs, 0);
        if svg_xml.is_empty() {
            continue;
        }
        // Inline SVGs frequently rely on CSS `fill` / `stroke` set by author rules
        // (e.g. `.logo { fill: #2c0022; }`).  The computed values are not present
        // in the serialized element attributes, so usvg would default to black.
        // Inject them as presentation attributes on the root `<svg>` so the
        // rasterizer can see them.
        let mut svg_attrs: Vec<(String, String)> = Vec::new();
        if let Some(s) = styles.as_ref().and_then(|s| s.get(&id)) {
            if let Some(fill) = s.fill {
                svg_attrs.push(("fill".into(), css_color_to_svg(fill)));
            }
            if let Some(stroke) = s.stroke {
                svg_attrs.push(("stroke".into(), css_color_to_svg(stroke)));
            }
            if s.stroke_width > 0.0 {
                svg_attrs.push(("stroke-width".into(), format!("{}", s.stroke_width)));
            }
        }
        inject_svg_presentation_attrs(&mut svg_xml, &svg_attrs);

        let current_color = styles
            .as_ref()
            .and_then(|s| s.get(&id))
            .map(|s| s.color)
            .unwrap_or(CssColor::BLACK);
        let parent_id = doc.nodes[id].parent;
        let custom_props = styles
            .as_ref()
            .and_then(|s| s.get(&id))
            .map(|s| s.custom_properties.as_ref());
        let Some(mut img) = render_svg_xml(&svg_xml, current_color, custom_props, None, None)
        else {
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

            // Re-raster at the resolved placeholder size so the cached bitmap matches
            // the CSS box the layout engine will use.  The first render above only
            // served to discover the SVG's intrinsic dimensions for the fallback
            // case; the final bitmap is produced here.
            if let Some(target_img) = render_svg_xml(
                &svg_xml,
                current_color,
                custom_props,
                Some(attr_width as f32),
                Some(attr_height as f32),
            ) {
                img = target_img;
            }

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
                // Preserve the resolved dimensions as an inline style so that after
                // re-resolving styles the placeholder is not resized by author rules
                // that target `img` differently than the original `svg` (e.g.
                // `img,svg,video { width:100% }`). Inline styles have the highest
                // cascade origin and override those generic rules.
                let inline_width = match style.width {
                    SizeValue::Px(v) => format!("width:{}px", v),
                    SizeValue::Percent(p) => format!("width:{}%", p),
                    _ => "width:auto".to_string(),
                };
                let inline_height = match style.height {
                    SizeValue::Px(v) => format!("height:{}px", v),
                    SizeValue::Percent(p) => format!("height:{}%", p),
                    _ => "height:auto".to_string(),
                };
                let existing_style = el.get_attr("style").unwrap_or("").to_string();
                let mut new_style = existing_style;
                if !new_style.is_empty() && !new_style.ends_with(';') {
                    new_style.push(';');
                }
                if !new_style.is_empty() {
                    new_style.push(' ');
                }
                new_style.push_str(&inline_width);
                new_style.push(';');
                new_style.push(' ');
                new_style.push_str(&inline_height);
                new_style.push(';');
                el.attributes.insert("style".to_string(), new_style);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remove_empty_placeholders_keeps_hidden_responsive() {
        // Tailwind responsive pattern: `hidden lg:flex` should not be treated as a
        // placeholder, because the stylesheet makes it visible at the viewport width
        // used for comparisons. This is the pattern Ars Technica's hero uses for its
        // left-hand story list.
        let html = r#"<!doctype html>
<html><body>
<div class="mx-auto flex flex-col flex-nowrap sm:max-w-2xl lg:max-w-6xl lg:flex-row-reverse">
  <div class="lg:w-1/2"><article>Right</article></div>
  <div class="hidden flex-col lg:flex lg:w-1/2 lg:pr-12">
    <article>Left</article>
  </div>
</div>
</body></html>
"#;
        let mut doc = parse_html(html);
        remove_empty_placeholders(&mut doc);
        let outer = doc
            .body()
            .and_then(|body| {
                doc.node(body)
                    .children
                    .iter()
                    .find(|id| {
                        matches!(&doc.node(**id).data,
                            NodeData::Element(ref e) if e.tag_name == "div"
                        )
                    })
                    .copied()
            })
            .unwrap();
        let element_children: Vec<_> = doc
            .node(outer)
            .children
            .iter()
            .filter(|id| matches!(&doc.node(**id).data, NodeData::Element(_)))
            .copied()
            .collect();
        assert_eq!(
            element_children.len(),
            2,
            "responsive `hidden lg:flex` container must not be removed as a placeholder"
        );
    }

    #[test]
    fn test_remove_empty_placeholders_removes_plain_hidden() {
        // A plain `hidden` element with no responsive display override is still a
        // placeholder and should be removed.
        let html = r#"<!doctype html>
<html><body>
<div class="hidden">This should be removed</div>
<div class="visible">Keep me</div>
</body></html>
"#;
        let mut doc = parse_html(html);
        remove_empty_placeholders(&mut doc);
        let body = doc.body().unwrap();
        let remaining: Vec<_> = doc
            .node(body)
            .children
            .iter()
            .filter(|id| matches!(&doc.node(**id).data, NodeData::Element(_)))
            .collect();
        assert_eq!(remaining.len(), 1);
        let kept_id = *remaining[0];
        if let NodeData::Element(ref e) = doc.node(kept_id).data {
            assert_eq!(e.get_attr("class"), Some("visible"));
        } else {
            panic!("expected the visible div to remain");
        }
    }

    #[test]
    fn test_remove_empty_placeholders_removes_styled_components_ad_slot() {
        // BBC's ad slots use a styled-components class like
        // `AdSlot-styles__AdSlotContainerStyled-sc-...` with no visible content.
        // The placeholder detector should catch the `AdSlot` token even though it
        // is embedded in a longer hashed class name.
        let html = r#"<!doctype html>
<html><body>
<div class="AdSlot-styles__AdSlotContainerStyled-sc-4b576bed-0 jQxGIx"></div>
<div class="real-article">Keep me</div>
</body></html>
"#;
        let mut doc = parse_html(html);
        remove_empty_placeholders(&mut doc);
        let body = doc.body().unwrap();
        let remaining: Vec<_> = doc
            .node(body)
            .children
            .iter()
            .filter(|id| matches!(&doc.node(**id).data, NodeData::Element(_)))
            .collect();
        assert_eq!(remaining.len(), 1);
        let kept_id = *remaining[0];
        if let NodeData::Element(ref e) = doc.node(kept_id).data {
            assert_eq!(e.get_attr("class"), Some("real-article"));
        } else {
            panic!("expected the real article to remain");
        }
    }

    #[test]
    fn test_remove_empty_placeholders_keeps_aria_hidden_image_wrapper() {
        // Al Jazeera article cover images live inside an `aria-hidden="true"`
        // wrapper so screen readers ignore the decorative image. The wrapper is
        // real visual content, not a placeholder, and must survive cleanup.
        let html = r#"<!doctype html>
<html><body>
<div class="article-card__image-wrap article-card__featured-image" aria-hidden="true" tabindex="-1">
  <div class="responsive-image">
    <img src="/hero.jpg" alt="An Iranian-made drone" />
  </div>
</div>
<div class="real-article">Keep me</div>
</body></html>
"#;
        let mut doc = parse_html(html);
        remove_empty_placeholders(&mut doc);
        let body = doc.body().unwrap();
        let remaining: Vec<_> = doc
            .node(body)
            .children
            .iter()
            .filter(|id| matches!(&doc.node(**id).data, NodeData::Element(_)))
            .collect();
        assert_eq!(remaining.len(), 2, "aria-hidden image wrapper must be kept");
        let wrapper_id = *remaining[0];
        if let NodeData::Element(ref e) = doc.node(wrapper_id).data {
            assert_eq!(
                e.get_attr("class"),
                Some("article-card__image-wrap article-card__featured-image")
            );
        } else {
            panic!("expected the image wrapper to remain");
        }
    }

    #[test]
    fn test_promote_lazy_image_sources_swaps_data_src_and_class() {
        // SD Times and many WordPress themes hide `.lazyload[data-src]` images with
        // `display:none !important` and rely on JS to swap `data-src` to `src`.
        // Without this promotion the hero/article images never render.
        let html = r#"<!doctype html>
<html><body>
<img class="hero lazyload" src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8/5+hHgAHggJ/PchI7wAAAABJRU5ErkJggg==" data-src="/hero.jpg" width="1024" height="683" loading="lazy" />
<img class="thumb lazyloading" data-src="/thumb.jpg" data-srcset="/thumb-400.jpg 400w, /thumb-800.jpg 800w" />
<img class="normal" src="/existing.jpg" />
</body></html>
"#;
        let mut doc = parse_html(html);
        promote_lazy_image_sources(&mut doc);
        let body = doc.body().unwrap();
        let images: Vec<_> = doc
            .node(body)
            .children
            .iter()
            .filter(|id| {
                matches!(&doc.node(**id).data, NodeData::Element(ref e) if e.tag_name == "img")
            })
            .copied()
            .collect();
        assert_eq!(images.len(), 3);

        let hero = &doc.node(images[0]).data;
        if let NodeData::Element(ref e) = hero {
            assert_eq!(e.get_attr("src"), Some("/hero.jpg"));
            assert_eq!(e.get_attr("class"), Some("hero lazyloaded"));
            assert_eq!(e.get_attr("loading"), Some("eager"));
        } else {
            panic!("expected img element");
        }

        let thumb = &doc.node(images[1]).data;
        if let NodeData::Element(ref e) = thumb {
            assert_eq!(e.get_attr("src"), Some("/thumb.jpg"));
            assert_eq!(
                e.get_attr("srcset"),
                Some("/thumb-400.jpg 400w, /thumb-800.jpg 800w")
            );
            assert_eq!(e.get_attr("class"), Some("thumb lazyloaded"));
        } else {
            panic!("expected img element");
        }

        let normal = &doc.node(images[2]).data;
        if let NodeData::Element(ref e) = normal {
            assert_eq!(e.get_attr("src"), Some("/existing.jpg"));
            assert_eq!(e.get_attr("class"), Some("normal"));
        } else {
            panic!("expected img element");
        }
    }

    #[test]
    fn test_promote_lazy_image_sources_without_lazy_class() {
        // WIRED and other sites use data-src without the standard lazyload class.
        let html = r#"<!doctype html>
<html><body>
<img class="responsive-image__image" data-src="/wired.jpg" width="800" height="600" />
<img class="responsive-image__image" data-src="/wired2.jpg" src="data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7" />
<img class="responsive-image__image" src="/existing.jpg" data-src="/other.jpg" />
</body></html>
"#;
        let mut doc = parse_html(html);
        promote_lazy_image_sources(&mut doc);
        let body = doc.body().unwrap();
        let images: Vec<_> = doc
            .node(body)
            .children
            .iter()
            .filter(|id| {
                matches!(&doc.node(**id).data,
                    NodeData::Element(ref e) if e.tag_name == "img"
                )
            })
            .copied()
            .collect();
        assert_eq!(images.len(), 3);

        // First image: no src, has data-src -> promoted
        let first = &doc.node(images[0]).data;
        if let NodeData::Element(ref e) = first {
            assert_eq!(e.get_attr("src"), Some("/wired.jpg"));
            assert_eq!(e.get_attr("class"), Some("responsive-image__image"));
        } else {
            panic!("expected img element");
        }

        // Second image: placeholder src, has data-src -> promoted
        let second = &doc.node(images[1]).data;
        if let NodeData::Element(ref e) = second {
            assert_eq!(e.get_attr("src"), Some("/wired2.jpg"));
        } else {
            panic!("expected img element");
        }

        // Third image: real src exists -> NOT promoted
        let third = &doc.node(images[2]).data;
        if let NodeData::Element(ref e) = third {
            assert_eq!(e.get_attr("src"), Some("/existing.jpg"));
        } else {
            panic!("expected img element");
        }
    }
}
