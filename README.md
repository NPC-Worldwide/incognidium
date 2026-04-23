# Incognidium

A web browser engine built from scratch in Rust. Renders HTML, CSS, and JavaScript with software rendering via tiny-skia.

## Example Renders

Incognidium rendering real-world pages at 1024px viewport (compared head-to-head against Firefox headless in the QA pipeline).

### Default page (about:blank)

![default](assets/examples/default.png)

### Wikipedia

![wikipedia main](assets/examples/wikipedia/main.png)

<details>
<summary>More Wikipedia pages (articles, science, history)</summary>

#### Albert Einstein
![einstein](assets/examples/wikipedia/einstein.png)

#### Rust (programming language)
![rust](assets/examples/wikipedia/rust.png)

#### Photosynthesis
![photosynthesis](assets/examples/wikipedia/photosynthesis.png)

#### Bell's theorem
![bell_theorem](assets/examples/wikipedia/bell_theorem.png)

#### List of countries by population
![list_of_countries](assets/examples/wikipedia/list_of_countries.png)

#### Antonio Gramsci
![gramsci](assets/examples/wikipedia/gramsci.png)

#### Richard Nixon
![nixon](assets/examples/wikipedia/nixon.png)

#### Mao Zedong
![mao](assets/examples/wikipedia/mao.png)

#### Simón Bolívar
![bolivar](assets/examples/wikipedia/bolivar.png)

#### Bernardo O'Higgins
![ohiggins](assets/examples/wikipedia/ohiggins.png)

#### Nelson Mandela
![mandela](assets/examples/wikipedia/mandela.png)

#### Che Guevara
![che_guevara](assets/examples/wikipedia/che_guevara.png)

#### Rosa Luxemburg
![rosa_luxemburg](assets/examples/wikipedia/rosa_luxemburg.png)

#### Frantz Fanon
![fanon](assets/examples/wikipedia/fanon.png)

#### Toussaint Louverture
![louverture](assets/examples/wikipedia/louverture.png)

#### Fidel Castro
![castro](assets/examples/wikipedia/castro.png)

</details>

### Hacker News

![hackernews main](assets/examples/hackernews/main.png)

<details>
<summary>More Hacker News pages</summary>

#### Newest
![newest](assets/examples/hackernews/newest.png)

#### Show HN
![show](assets/examples/hackernews/show.png)

#### Ask HN
![ask](assets/examples/hackernews/ask.png)

</details>

### Sibiji (search engine)

![sibiji](assets/examples/sibiji/main.png)

Currently a React SPA — incognidium's JS engine doesn't yet hydrate React trees, so the page shows the "enable JavaScript" fallback. Tracking progress.

### NPR

![npr main](assets/examples/npr/main.png)

<details>
<summary>More NPR pages</summary>

#### News
![news](assets/examples/npr/news.png)

#### Politics
![politics](assets/examples/npr/politics.png)

#### Culture
![culture](assets/examples/npr/culture.png)

</details>

### AP News

![apnews main](assets/examples/apnews/main.png)

<details>
<summary>More AP News pages</summary>

#### World
![world](assets/examples/apnews/world.png)

#### Politics
![politics](assets/examples/apnews/politics.png)

#### Business
![business](assets/examples/apnews/business.png)

</details>

### The Japan Times

![japantimes](assets/examples/japantimes/main.png)

### CNN Lite

![cnn_lite](assets/examples/cnn_lite/main.png)

### MDN Web Docs

![mdn js_array](assets/examples/mdn/js_array.png)

<details>
<summary>More MDN pages</summary>

#### CSS flex
![css_flex](assets/examples/mdn/css_flex.png)

</details>

### Rust Docs

![rust_docs main](assets/examples/rust_docs/main.png)

<details>
<summary>More Rust docs pages</summary>

#### std library
![std](assets/examples/rust_docs/std.png)

</details>

### arXiv

![arxiv main](assets/examples/arxiv/main.png)

<details>
<summary>More arXiv pages</summary>

#### CS new submissions
![cs](assets/examples/arxiv/cs.png)

</details>

### GitHub Docs

![github_docs](assets/examples/github_docs/main.png)

### Wiktionary

![wiktionary](assets/examples/wikitionary/main.png)

### Al Jazeera

![aljazeera main](assets/examples/aljazeera/main.png)

<details>
<summary>More Al Jazeera pages</summary>

#### News
![news](assets/examples/aljazeera/news.png)

#### Features
![features](assets/examples/aljazeera/features.png)

#### Opinion
![opinion](assets/examples/aljazeera/opinion.png)

</details>

### Deutsche Welle (DW)

![dw main](assets/examples/dw/main.png)

<details>
<summary>More DW pages</summary>

#### World
![world](assets/examples/dw/world.png)

</details>

### Archive.org

![archive_org](assets/examples/archive_org/main.png)

### Stack Overflow

![stackoverflow main](assets/examples/stackoverflow/main.png)

<details>
<summary>More Stack Overflow pages</summary>

#### Questions
![questions](assets/examples/stackoverflow/questions.png)

</details>

### Python Docs

![python_docs main](assets/examples/python_docs/main.png)

<details>
<summary>More Python Docs pages</summary>

#### Tutorial
![tutorial](assets/examples/python_docs/tutorial.png)

</details>

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
