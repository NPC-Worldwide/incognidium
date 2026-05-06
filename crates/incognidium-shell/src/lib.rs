//! Shared logic for incognidium-shell and its binaries.

#[cfg(feature = "v8-engine")]
pub mod v8_dom;

#[cfg(feature = "boa-engine")]
pub mod boa_dom;

use std::collections::HashMap;

use incognidium_net::{fetch_url, resolve_url};
use incognidium_paint::ImageData;

/// A script to execute, with its source code and a label for error messages.
pub struct ScriptEntry {
    pub source: String,
    pub origin: String,
}

/// Collect scripts from the DOM in document order, handling both inline and
/// external `<script src="...">` tags.
///
/// - Skips `type="module"` scripts (ES modules not supported)
/// - Limits external script fetches to 20
/// - Maintains document order for execution
pub fn collect_scripts(doc: &incognidium_dom::Document, base_url: &str) -> Vec<ScriptEntry> {
    const MAX_EXTERNAL_SCRIPTS: usize = 20;
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
