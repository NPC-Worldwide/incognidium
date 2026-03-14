//! Shared logic for incognidium-shell and its binaries.

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
                // Skip type="module" -- we can't handle ES modules
                if let Some(script_type) = el.get_attr("type") {
                    if script_type.eq_ignore_ascii_case("module") {
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

/// Execute scripts using Boa (full ES2024 JS engine).
/// Returns the modified Document.
pub fn execute_scripts_on_doc(
    doc: incognidium_dom::Document,
    scripts: &[ScriptEntry],
    _image_cache: &mut HashMap<String, ImageData>,
) -> incognidium_dom::Document {
    boa_dom::execute_scripts_boa(doc, scripts)
}
