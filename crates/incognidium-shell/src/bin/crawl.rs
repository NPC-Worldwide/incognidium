/// incognidium crawl — archive the web for training data
///
/// Crawls a list of URLs, renders them, extracts text, and saves to
/// ~/.incognidium/archive/ in JSONL format suitable for ML training.
///
/// Usage:
///   incognidium-crawl                    # crawl default sites
///   incognidium-crawl --sites news       # crawl news sites only
///   incognidium-crawl --url https://...  # crawl a single URL
///   incognidium-crawl --history          # show crawl history
///   incognidium-crawl --stats            # show corpus stats

use std::collections::HashMap;
use std::io::Write;

use incognidium_css::parse_css;
use incognidium_html::parse_html;
use incognidium_layout::{flatten_layout, layout_with_images, ImageSizes};
use incognidium_net::{fetch_url, resolve_url};
use incognidium_paint::ImageData;
use incognidium_style::resolve_styles;

use incognidium_shell::{collect_scripts, execute_scripts_on_doc};

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let base_dir = format!("{}/.incognidium", home);
    let archive_dir = format!("{}/archive", base_dir);
    let screenshots_dir = format!("{}/screenshots", base_dir);

    std::fs::create_dir_all(&archive_dir).ok();
    std::fs::create_dir_all(&screenshots_dir).ok();

    // --history: show crawl history
    if args.iter().any(|a| a == "--history") {
        show_history(&archive_dir);
        return;
    }

    // --stats: show corpus stats
    if args.iter().any(|a| a == "--stats") {
        show_stats(&archive_dir);
        return;
    }

    // Determine which sites to crawl
    let urls = if let Some(pos) = args.iter().position(|a| a == "--url") {
        let url = args.get(pos + 1).expect("--url requires a URL");
        vec![(slug_from_url(url), url.clone())]
    } else {
        let category = args.iter().position(|a| a == "--sites")
            .and_then(|i| args.get(i + 1))
            .map(|s| s.as_str())
            .unwrap_or("all");
        get_sites(category)
    };

    let date = chrono_date();
    let time = chrono_time();
    let corpus_path = format!("{}/crawl_{}.jsonl", archive_dir, date);
    let day_screenshots = format!("{}/{}", screenshots_dir, date);
    std::fs::create_dir_all(&day_screenshots).ok();

    eprintln!("Incognidium Crawl — {}", date);
    eprintln!("Sites: {}", urls.len());
    eprintln!("Corpus: {}", corpus_path);
    eprintln!();

    let mut corpus_file = std::fs::OpenOptions::new()
        .create(true).append(true)
        .open(&corpus_path)
        .expect("open corpus file");

    let mut total_chars = 0usize;
    let mut total_lines = 0usize;
    let mut success = 0usize;

    for (name, url) in &urls {
        eprint!("  {:<20} ", name);

        match crawl_page(url) {
            Ok(page) => {
                let line_count = page.text.lines().count();
                let char_count = page.text.len();
                total_chars += char_count;
                total_lines += line_count;
                success += 1;

                // Save screenshot
                let png_path = format!("{}/{}_{}.png", day_screenshots, name, time);
                if let Some(ref pixmap) = page.pixmap_data {
                    std::fs::write(&png_path, pixmap).ok();
                }

                // Write JSONL record
                let record = serde_json::json!({
                    "url": url,
                    "name": name,
                    "timestamp": format!("{}T{}", date, time),
                    "date": date,
                    "title": page.title,
                    "text": page.text,
                    "text_lines": line_count,
                    "text_chars": char_count,
                    "dom_nodes": page.dom_nodes,
                    "css_bytes": page.css_bytes,
                    "text_boxes": page.text_boxes,
                    "flat_boxes": page.flat_boxes,
                    "has_screenshot": page.pixmap_data.is_some(),
                });
                writeln!(corpus_file, "{}", record).ok();

                eprintln!("{:>5} lines  {:>7} chars  ✓", line_count, char_count);
            }
            Err(e) => {
                eprintln!("FAILED: {}", e);
            }
        }
    }

    eprintln!();
    eprintln!("Done: {}/{} sites, {} lines, {} chars",
        success, urls.len(), total_lines, total_chars);
    eprintln!("Corpus: {}", corpus_path);
    eprintln!("Screenshots: {}/", day_screenshots);
}

struct CrawledPage {
    title: String,
    text: String,
    dom_nodes: usize,
    css_bytes: usize,
    text_boxes: usize,
    flat_boxes: usize,
    pixmap_data: Option<Vec<u8>>,
}

fn crawl_page(url: &str) -> Result<CrawledPage, String> {
    let resp = fetch_url(url).map_err(|e| format!("fetch: {}", e))?;
    let doc = parse_html(&resp.body);
    let dom_nodes = doc.nodes.len();

    // JS execution
    let mut image_cache: HashMap<String, ImageData> = HashMap::new();
    let scripts = collect_scripts(&doc, url);
    let doc = if !scripts.is_empty() {
        execute_scripts_on_doc(doc, &scripts, &mut image_cache)
    } else {
        doc
    };

    // Extract title
    let title = extract_title(&doc);

    // External CSS
    let mut css_text = fetch_external_css_for_doc(&doc, url);
    let css_bytes = css_text.len();
    css_text.push_str(&doc.collect_style_text());

    let stylesheet = parse_css(&css_text);
    let styles = resolve_styles(&doc, &stylesheet, 1024.0, 768.0);

    let image_sizes = ImageSizes::new();
    let layout_root = layout_with_images(&doc, &styles, 1024.0, 20000.0, &image_sizes);
    let flat_boxes = flatten_layout(&layout_root, 0.0, 0.0, &styles);

    // Extract text
    let mut text_items: Vec<(f32, f32, String)> = Vec::new();
    let mut text_box_count = 0usize;
    for fbox in &flat_boxes {
        if let Some(ref t) = fbox.text {
            let trimmed = t.trim();
            if !trimmed.is_empty() {
                text_items.push((fbox.y, fbox.x, trimmed.to_string()));
                text_box_count += 1;
            }
        }
    }
    text_items.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.partial_cmp(&b.1).unwrap()));

    // Merge into lines
    let mut lines: Vec<String> = Vec::new();
    let mut current_line = String::new();
    let mut last_y: f32 = -100.0;
    for (y, _x, text) in &text_items {
        if (y - last_y).abs() > 4.0 {
            if !current_line.is_empty() {
                lines.push(std::mem::take(&mut current_line));
            }
        } else if !current_line.is_empty() {
            current_line.push(' ');
        }
        current_line.push_str(text);
        last_y = *y;
    }
    if !current_line.is_empty() {
        lines.push(current_line);
    }

    // Render screenshot
    let content_height = flat_boxes.iter()
        .map(|b| (b.y + b.height) as u32)
        .max().unwrap_or(768).max(200) + 20;
    let render_height = content_height.min(2000);
    let pixmap = incognidium_paint::paint_with_images(
        &flat_boxes, &styles, 1024, render_height, &image_cache);
    let png_data = pixmap.encode_png().ok();

    Ok(CrawledPage {
        title,
        text: lines.join("\n"),
        dom_nodes,
        css_bytes,
        text_boxes: text_box_count,
        flat_boxes: flat_boxes.len(),
        pixmap_data: png_data,
    })
}

fn extract_title(doc: &incognidium_dom::Document) -> String {
    for node in &doc.nodes {
        if let incognidium_dom::NodeData::Element(ref el) = node.data {
            if el.tag_name == "title" {
                // Get first text child
                for &child_id in &node.children {
                    if let incognidium_dom::NodeData::Text(ref t) = doc.nodes[child_id].data {
                        return t.content.trim().to_string();
                    }
                }
            }
        }
    }
    String::new()
}

fn fetch_external_css_for_doc(doc: &incognidium_dom::Document, base_url: &str) -> String {
    let mut css = String::new();
    let mut fetched = 0usize;
    for node in &doc.nodes {
        if fetched >= 10 { break; }
        if let incognidium_dom::NodeData::Element(ref el) = node.data {
            if el.tag_name == "link" {
                let is_ss = el.get_attr("rel")
                    .map(|r| r.eq_ignore_ascii_case("stylesheet")).unwrap_or(false);
                if is_ss {
                    if let Some(href) = el.get_attr("href") {
                        if let Ok(resolved) = resolve_url(base_url, href) {
                            if let Ok(resp) = fetch_url(&resolved) {
                                if resp.body.len() <= 256 * 1024 {
                                    css.push_str(&resp.body);
                                    css.push('\n');
                                    fetched += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    css
}

fn slug_from_url(url: &str) -> String {
    url.replace("https://", "").replace("http://", "")
        .replace("www.", "")
        .split('/').next().unwrap_or("unknown")
        .replace('.', "_")
}

fn chrono_date() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap();
    let secs = now.as_secs();
    // Simple date calculation
    let days = secs / 86400;
    let mut y = 1970i64;
    let mut remaining = days as i64;
    loop {
        let days_in_year = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 366 } else { 365 };
        if remaining < days_in_year { break; }
        remaining -= days_in_year;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let month_days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 0;
    for (i, &d) in month_days.iter().enumerate() {
        if remaining < d as i64 { m = i + 1; break; }
        remaining -= d as i64;
    }
    format!("{:04}-{:02}-{:02}", y, m, remaining + 1)
}

fn chrono_time() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap();
    let secs = now.as_secs() % 86400;
    format!("{:02}{:02}{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
}

fn get_sites(category: &str) -> Vec<(String, String)> {
    // Try to load from sites.txt first
    let sites_file = std::path::Path::new("sites.txt");
    if sites_file.exists() {
        if let Ok(content) = std::fs::read_to_string(sites_file) {
            let mut sites = Vec::new();
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') { continue; }
                let parts: Vec<&str> = line.split('|').collect();
                if parts.len() >= 2 {
                    let name = parts[0];
                    let url = parts[1];
                    let cat = if parts.len() > 2 { parts[2] } else { "other" };
                    if category == "all" || cat == category {
                        sites.push((name.to_string(), url.to_string()));
                    }
                }
            }
            if !sites.is_empty() {
                return sites;
            }
        }
    }

    // Fallback: hardcoded sites
    let news = vec![
        ("hn", "https://news.ycombinator.com"),
        ("cnn_lite", "https://lite.cnn.com"),
        ("npr", "https://text.npr.org"),
        ("bbc", "https://www.bbc.com"),
        ("reuters", "https://www.reuters.com"),
        ("ap_news", "https://apnews.com"),
        ("guardian", "https://www.theguardian.com"),
        ("aljazeera", "https://www.aljazeera.com"),
        ("nytimes", "https://www.nytimes.com"),
    ];
    let tech = vec![
        ("hn", "https://news.ycombinator.com"),
        ("lobsters", "https://lobste.rs"),
        ("slashdot", "https://slashdot.org"),
        ("ars", "https://arstechnica.com"),
        ("github", "https://github.com"),
        ("mdn", "https://developer.mozilla.org/en-US/docs/Web/HTML"),
    ];
    let reference = vec![
        ("wikipedia", "https://en.wikipedia.org/wiki/Main_Page"),
        ("wiki_rust", "https://en.wikipedia.org/wiki/Rust_(programming_language)"),
        ("wiki_linux", "https://en.wikipedia.org/wiki/Linux"),
        ("wiki_python", "https://en.wikipedia.org/wiki/Python_(programming_language)"),
        ("wiki_html", "https://en.wikipedia.org/wiki/HTML"),
        ("python_docs", "https://docs.python.org/3/"),
        ("rust_book", "https://doc.rust-lang.org/book/"),
    ];
    let blogs = vec![
        ("paulgraham", "http://www.paulgraham.com"),
        ("daringfireball", "https://daringfireball.net"),
        ("dan_luu", "https://danluu.com"),
        ("joel_on_sw", "https://www.joelonsoftware.com"),
        ("kottke", "https://kottke.org"),
        ("gwern", "https://gwern.net"),
    ];
    let minimal = vec![
        ("duckduckgo", "https://duckduckgo.com"),
        ("lite_ddg", "https://lite.duckduckgo.com/lite"),
        ("wiby", "https://wiby.me"),
        ("info_cern", "http://info.cern.ch"),
        ("textfiles", "http://textfiles.com"),
        ("craigslist", "https://www.craigslist.org"),
        ("archive_org", "https://archive.org"),
    ];

    let selected: Vec<(&str, &str)> = match category {
        "news" => news,
        "tech" => tech,
        "reference" | "ref" => reference,
        "blogs" => blogs,
        "minimal" | "min" => minimal,
        _ => {
            let mut all = Vec::new();
            all.extend_from_slice(&news);
            all.extend_from_slice(&tech);
            all.extend_from_slice(&reference);
            all.extend_from_slice(&blogs);
            all.extend_from_slice(&minimal);
            // Dedup by name
            let mut seen = std::collections::HashSet::new();
            all.retain(|(name, _)| seen.insert(*name));
            all
        }
    };

    selected.into_iter()
        .map(|(n, u)| (n.to_string(), u.to_string()))
        .collect()
}

fn show_history(archive_dir: &str) {
    let mut entries: Vec<_> = std::fs::read_dir(archive_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("crawl_"))
        .collect();
    entries.sort_by_key(|e| e.file_name().to_string_lossy().to_string());

    println!("Crawl History:");
    println!("{:<15} {:>8} {:>10}", "Date", "Records", "Size");
    println!("{}", "-".repeat(35));
    for entry in &entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let date = name.strip_prefix("crawl_").and_then(|s| s.strip_suffix(".jsonl"))
            .unwrap_or(&name);
        let meta = entry.metadata().ok();
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let records = std::fs::read_to_string(entry.path()).ok()
            .map(|s| s.lines().count()).unwrap_or(0);
        println!("{:<15} {:>8} {:>9}K", date, records, size / 1024);
    }
}

fn show_stats(archive_dir: &str) {
    let mut total_records = 0usize;
    let mut total_bytes = 0u64;
    let mut total_text_chars = 0usize;
    let mut dates = 0usize;

    if let Ok(entries) = std::fs::read_dir(archive_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            if entry.file_name().to_string_lossy().starts_with("crawl_") {
                dates += 1;
                if let Ok(meta) = entry.metadata() {
                    total_bytes += meta.len();
                }
                if let Ok(content) = std::fs::read_to_string(entry.path()) {
                    for line in content.lines() {
                        total_records += 1;
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                            total_text_chars += v["text_chars"].as_u64().unwrap_or(0) as usize;
                        }
                    }
                }
            }
        }
    }

    println!("Incognidium Corpus Stats");
    println!("{}", "=".repeat(30));
    println!("Days crawled:    {}", dates);
    println!("Total records:   {}", total_records);
    println!("Corpus size:     {}K", total_bytes / 1024);
    println!("Total text:      {} chars", total_text_chars);
    println!("Archive:         {}/", archive_dir);
}
