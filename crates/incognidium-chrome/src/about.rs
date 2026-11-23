use crate::persistence::{Bookmark, HistoryEntry, Settings, UserData};

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn build_local_page(
    url: &str,
    user_data: &UserData,
    page_source: Option<&str>,
) -> Option<String> {
    if url == "about:home" || url.starts_with("about:home?") {
        return Some(build_home_page(
            &user_data.settings,
            &user_data.bookmarks,
            &user_data.history,
        ));
    }
    if url == "about:about" {
        return Some(build_about_page());
    }
    if url == "about:settings" || url.starts_with("about:settings?") {
        return Some(build_settings_page(&user_data.settings));
    }
    if url == "about:bookmarks" {
        return Some(build_bookmarks_page(&user_data.bookmarks));
    }
    if url == "about:history" {
        return Some(build_history_page(&user_data.history));
    }
    if url == "about:source" {
        return Some(build_source_page(page_source.unwrap_or("")));
    }
    if url == "about:blank" {
        return Some("<html><body></body></html>".to_string());
    }
    None
}

pub fn build_home_page(
    settings: &Settings,
    bookmarks: &[Bookmark],
    history: &[HistoryEntry],
) -> String {
    let _search = html_escape(&settings.search_engine_url);
    let _home = html_escape(&settings.homepage);
    let mut bm_items = String::new();
    for bm in bookmarks.iter().rev().take(10) {
        bm_items.push_str(&format!(
            r#"<li><a href="{}">{}</a></li>"#,
            html_escape(&bm.url),
            html_escape(&bm.title)
        ));
    }
    let mut hist_items = String::new();
    for h in history.iter().rev().take(10) {
        hist_items.push_str(&format!(
            r#"<li><a href="{}">{}</a></li>"#,
            html_escape(&h.url),
            html_escape(&h.title)
        ));
    }

    format!(
        r#"<!DOCTYPE html>
<html>
<head><title>New Tab - Incognidium</title>
<style>
body {{ font-family: system-ui, sans-serif; background: #f5f5f5; color: #222; margin: 0; }}
.container {{ max-width: 720px; margin: 80px auto; text-align: center; padding: 0 16px; }}
h1 {{ font-weight: 400; }}
.search-box {{ display: flex; justify-content: center; margin: 32px 0; }}
input[type="search"] {{ width: 100%; max-width: 480px; padding: 12px 16px; font-size: 16px; border: 1px solid #ccc; border-radius: 24px; }}
input[type="submit"] {{ margin-left: 8px; padding: 12px 20px; border: none; border-radius: 24px; background: #333; color: #fff; cursor: pointer; }}
.columns {{ display: flex; gap: 32px; text-align: left; justify-content: center; flex-wrap: wrap; }}
.column {{ min-width: 220px; }}
ul {{ list-style: none; padding: 0; }}
li {{ margin: 8px 0; }}
a {{ color: #1155cc; text-decoration: none; }}
a:hover {{ text-decoration: underline; }}
</style>
</head>
<body>
<div class="container">
<h1>Incognidium</h1>
<form class="search-box" method="GET" action="about:home/search">
<input type="search" name="q" placeholder="Search or enter address" autofocus />
<input type="submit" value="Search" />
</form>
<div class="columns">
<div class="column">
<h2>Bookmarks</h2>
<ul>{}</ul>
</div>
<div class="column">
<h2>History</h2>
<ul>{}</ul>
</div>
</div>
</div>
</body>
</html>"#,
        if bm_items.is_empty() {
            "<li><em>No bookmarks yet.</em></li>".to_string()
        } else {
            bm_items
        },
        if hist_items.is_empty() {
            "<li><em>No history yet.</em></li>".to_string()
        } else {
            hist_items
        },
    )
}

pub fn build_about_page() -> String {
    r#"<!DOCTYPE html>
<html><head><title>About Incognidium</title></head>
<body style="font-family:system-ui,sans-serif;max-width:720px;margin:40px auto;padding:0 16px;">
<h1>Incognidium</h1>
<p>A lightweight web browser built from scratch in Rust.</p>
<p><a href="https://github.com/npcww/incognidium">github.com/npcww/incognidium</a></p>
</body></html>"#
        .to_string()
}

pub fn build_settings_page(settings: &Settings) -> String {
    let search = html_escape(&settings.search_engine_url);
    let home = html_escape(&settings.homepage);
    format!(
        r#"<!DOCTYPE html>
<html>
<head><title>Settings - Incognidium</title>
<style>
body {{ font-size: 16px; color: #222; background: #f9f9f9; }}
.container {{ max-width: 720px; margin: 40px auto; padding: 24px; background: #fff; border: 1px solid #ddd; border-radius: 8px; }}
h1 {{ font-size: 28px; color: #1a1a2e; }}
.form-row {{ margin-bottom: 16px; }}
label {{ display: block; font-weight: bold; margin-bottom: 6px; color: #333; }}
input[type="text"] {{ width: 100%; box-sizing: border-box; padding: 8px; font-size: 14px; border: 1px solid #ccc; border-radius: 4px; }}
button {{ padding: 8px 16px; font-size: 14px; background: #2277ee; color: #fff; border: none; border-radius: 4px; cursor: pointer; }}
.note {{ color: #666; font-size: 13px; margin-top: 8px; }}
</style>
</head>
<body>
<div class="container">
<h1>Settings</h1>
<form method="GET" action="about:settings/set">
<div class="form-row">
<label for="search_engine_url">Search engine URL</label>
<input type="text" id="search_engine_url" name="search_engine_url" value="{search}">
<p class="note">Use %s as a placeholder for the query text.</p>
</div>
<div class="form-row">
<label for="homepage">Homepage</label>
<input type="text" id="homepage" name="homepage" value="{home}">
<p class="note">URL to load on startup. Use about:home for the built-in start page.</p>
</div>
<button type="submit">Save settings</button>
</form>
</div>
</body>
</html>"#
    )
}

pub fn build_bookmarks_page(bookmarks: &[Bookmark]) -> String {
    let rows: String = bookmarks
        .iter()
        .rev()
        .map(|b| {
            let title = html_escape(&b.title);
            let url = html_escape(&b.url);
            format!(
                r#"<li class="bookmark"><a href="{url}">{title}</a><span class="url">{url}</span></li>"#,
            )
        })
        .collect();
    let count = bookmarks.len();
    format!(
        r#"<!DOCTYPE html>
<html>
<head><title>Bookmarks - Incognidium</title>
<style>
body {{ font-size: 16px; color: #222; background: #f9f9f9; }}
.container {{ max-width: 720px; margin: 40px auto; padding: 24px; background: #fff; border: 1px solid #ddd; border-radius: 8px; }}
h1 {{ font-size: 28px; color: #1a1a2e; }}
ul {{ list-style: none; padding: 0; }}
li.bookmark {{ margin-bottom: 14px; border-bottom: 1px solid #eee; padding-bottom: 10px; }}
a {{ font-size: 16px; color: #2277ee; text-decoration: none; }}
a:hover {{ text-decoration: underline; }}
.url {{ display: block; font-size: 12px; color: #666; margin-top: 2px; }}
.empty {{ color: #666; font-style: italic; }}
</style>
</head>
<body>
<div class="container">
<h1>Bookmarks ({count})</h1>
{list}
</div>
</body></html>"#,
        count = count,
        list = if rows.is_empty() {
            r#"<p class="empty">No bookmarks yet. Open the menu and choose Add Bookmark.</p>"#
                .to_string()
        } else {
            format!("<ul>{}</ul>", rows)
        }
    )
}

pub fn build_history_page(history: &[HistoryEntry]) -> String {
    let rows: String = history
        .iter()
        .rev()
        .map(|e| {
            let url = html_escape(&e.url);
            let title = html_escape(&e.title);
            let when = {
                let secs = e.visited_at as i64;
                let days = secs / 86400;
                let rem = secs % 86400;
                let hh = rem / 3600;
                let mm = (rem % 3600) / 60;
                format!("day {days} {hh:02}:{mm:02}")
            };
            format!(
                r#"<li class="history"><a href="{url}">{title}</a><span class="meta">{when} — {url}</span></li>"#,
            )
        })
        .collect();
    let count = history.len();
    format!(
        r#"<!DOCTYPE html>
<html>
<head><title>History - Incognidium</title>
<style>
body {{ font-size: 16px; color: #222; background: #f9f9f9; }}
.container {{ max-width: 720px; margin: 40px auto; padding: 24px; background: #fff; border: 1px solid #ddd; border-radius: 8px; }}
h1 {{ font-size: 28px; color: #1a1a2e; }}
ul {{ list-style: none; padding: 0; }}
li.history {{ margin-bottom: 14px; border-bottom: 1px solid #eee; padding-bottom: 10px; }}
a {{ font-size: 16px; color: #2277ee; text-decoration: none; }}
a:hover {{ text-decoration: underline; }}
.meta {{ display: block; font-size: 12px; color: #666; margin-top: 2px; }}
.empty {{ color: #666; font-style: italic; }}
</style>
</head>
<body>
<div class="container">
<h1>History ({count})</h1>
{list}
</div>
</body></html>"#,
        count = count,
        list = if rows.is_empty() {
            r#"<p class="empty">No history yet. Start browsing to populate your history.</p>"#
                .to_string()
        } else {
            format!("<ul>{}</ul>", rows)
        }
    )
}

pub fn build_source_page(source: &str) -> String {
    let escaped = html_escape(source);
    format!(
        r#"<!DOCTYPE html>
<html>
<head><title>Page Source - Incognidium</title>
<style>
body {{ font-size: 13px; color: #222; background: #fff; margin: 0; padding: 16px; font-family: monospace; }}
pre {{ white-space: pre-wrap; word-break: break-all; margin: 0; }}
</style>
</head><body><pre>{escaped}</pre></body></html>"#
    )
}
