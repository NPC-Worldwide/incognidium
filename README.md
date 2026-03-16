# Incognidium

A web browser engine built from scratch in Rust. Renders HTML, CSS, and JavaScript with software rendering via tiny-skia.

## Quick Start

```bash
# Desktop browser
cargo run --release -p incognidium-shell -- https://news.ycombinator.com

# Render a page to PNG
cargo run --release --bin render_to_png -- https://lite.cnn.com /tmp/output.png

# Render with text extraction
cargo run --release --bin render_to_png -- https://lite.cnn.com /tmp/output.png --text /tmp/output.txt

# Crawl sites for training data
cargo run --release --bin incognidium-crawl -- --sites news
```

## Install

```bash
# Build .deb package (Linux)
cargo install cargo-deb
cargo deb -p incognidium-shell
sudo dpkg -i target/debian/incognidium_*.deb

# Then run from anywhere
incognidium https://en.wikipedia.org
```

## Architecture

```
crates/
  incognidium-dom/     # DOM tree (arena-based)
  incognidium-html/    # HTML5 parser (html5ever)
  incognidium-css/     # CSS parser (cssparser) + selector matching
  incognidium-style/   # Style resolution, cascade, inheritance
  incognidium-layout/  # Box layout: block, inline, flex, table-as-flex
  incognidium-paint/   # Pixel rendering (tiny-skia) + TTF text
  incognidium-net/     # HTTP fetching (reqwest + rustls)
  incognidium-shell/   # Browser shell, JS engine (Boa), windowing
  incognidium-devtools/ # MCP devtools bridge
```

## Rendering Scorecard

Tested against Firefox headless on 20 sites:

| Grade | Sites |
|-------|-------|
| GREAT (diff < 25) | Hacker News, CNN Lite, NPR, Lobsters, Dan Luu |
| OK (diff 25-50) | Wikipedia, arxiv, Weather.gov, AP News, NYTimes |
| Rendering | TechCrunch, BBC, Slashdot, Nature, Ars Technica |

### CSS Support
- External stylesheets, inline styles, `<style>` blocks
- `@media` queries with viewport-aware min/max-width filtering
- `flex` shorthand, `flex-direction`, `flex-grow/shrink/basis`
- Multi-value `margin`/`padding` shorthands (1-4 values)
- `border` shorthand with width + style + color
- `box-sizing: border-box`
- `!important`, specificity-based cascade
- Pseudo-class filtering (skip `:visited`/`:hover`/`:focus`)
- `text-align`, `text-transform`, `text-decoration`
- `opacity`, `visibility`, `overflow`, `white-space`
- `position: fixed` elements skipped
- ~120 named CSS colors, `rgb()`/`rgba()`, hex colors
- `em`/`rem`/`pt`/`vw`/`vh` units

### JavaScript (Boa ES2024)
- DOM: `getElementById`, `querySelector`, `createElement`, `getElementsBy*`
- Window: `setTimeout`, `setInterval`, `requestAnimationFrame`, `fetch`
- Observers: `MutationObserver`, `IntersectionObserver`, `ResizeObserver`
- Storage: `localStorage`, `sessionStorage`
- 50+ DOM constructor stubs (`HTMLElement`, `Element`, `Node`, etc.)
- `getComputedStyle`, `matchMedia` with viewport awareness
- Script limits: 256KB per script, 1MB total per page

### Layout
- Block flow (vertical stacking)
- Inline flow (horizontal with word-wrap)
- Flexbox (row/column, grow/shrink/basis, justify/align)
- Table-as-flex (tr = flex row, td = flex items)
- `max-width` centering, `text-align` for inline content
- List bullet markers (ul/ol)
- Image loading and sizing

## Web Crawler

Archive the web for training data:

```bash
# Crawl news sites
incognidium-crawl --sites news

# Crawl all 40+ sites
incognidium-crawl

# Single URL
incognidium-crawl --url https://arxiv.org/list/cs.AI/recent

# View history
incognidium-crawl --history

# Corpus stats
incognidium-crawl --stats
```

Archive saved to `~/.incognidium/archive/` as JSONL (one record per page per crawl).

## NPC Team

The `npc_team/` directory contains an NPC team for automated testing:

```bash
# From npcsh
@renderer /render_batch iterations=3
@renderer /render_compare url="https://arxiv.org" name="arxiv"
```

## License

MIT
