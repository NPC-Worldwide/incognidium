#!/usr/bin/env python3
"""Remove site-specific code and references from the Incognidium engine and QA scripts."""

import os


def remove_reddit_code(repo, log):
    # 1. shell lib.rs
    path = os.path.join(repo, "crates", "incognidium-shell", "src", "lib.rs")
    with open(path) as f:
        src = f.read()

    start = src.find("pub fn reddit_challenge_target")
    if start == -1:
        log.append("reddit_challenge_target already removed from lib.rs")
    else:
        end_marker = "/// Collect scripts from the DOM"
        end = src.find(end_marker, start)
        if end == -1:
            log.append("ERROR: could not find end marker for Reddit functions")
        else:
            src = src[:start] + src[end:]
            log.append("Removed Reddit challenge solver functions from lib.rs")

    test_start = src.find("#[cfg(test)]\nmod reddit_tests {")
    if test_start == -1:
        test_start = src.find("mod reddit_tests {")
    if test_start == -1:
        log.append("reddit_tests module already removed from lib.rs")
    else:
        depth = 0
        i = test_start
        while i < len(src):
            if src[i] == '{':
                depth += 1
            elif src[i] == '}':
                depth -= 1
                if depth == 0:
                    end = i + 1
                    if end < len(src) and src[end] == '\n':
                        end += 1
                    src = src[:test_start] + src[end:]
                    log.append("Removed reddit_tests module from lib.rs")
                    break
            i += 1
        else:
            log.append("ERROR: could not find end of reddit_tests module")

    with open(path, "w") as f:
        f.write(src)

    # 2. render_to_png.rs
    path = os.path.join(repo, "crates", "incognidium-shell", "src", "bin", "render_to_png.rs")
    with open(path) as f:
        src = f.read()
    src = src.replace(
        "    propagate_canvas_background, rasterize_inline_svgs, reddit_challenge_target,\n",
        "    propagate_canvas_background, rasterize_inline_svgs,\n",
    )
    block_start = src.find("    if let Some(target) = reddit_challenge_target(&resp.body, &resp.url) {")
    if block_start == -1:
        log.append("reddit_challenge_target call already removed from render_to_png.rs")
    else:
        depth = 1
        i = src.find('{', block_start) + 1
        while i < len(src):
            if src[i] == '{':
                depth += 1
            elif src[i] == '}':
                depth -= 1
                if depth == 0:
                    end = i + 1
                    while end < len(src) and src[end] == '\n':
                        end += 1
                    src = src[:block_start] + "\n" + src[end:]
                    log.append("Removed reddit_challenge_target call from render_to_png.rs")
                    break
            i += 1
        else:
            log.append("ERROR: could not find end of reddit call block in render_to_png.rs")
    with open(path, "w") as f:
        f.write(src)

    # 3. main.rs
    path = os.path.join(repo, "crates", "incognidium-shell", "src", "main.rs")
    with open(path) as f:
        src = f.read()
    marker = "incognidium_shell::reddit_challenge_target"
    while marker in src:
        call_start = src.find("if let Some(target) =\n                    incognidium_shell::reddit_challenge_target")
        if call_start == -1:
            call_start = src.find("if let Some(target) = incognidium_shell::reddit_challenge_target")
        if call_start == -1:
            log.append("ERROR: could not locate reddit_challenge_target call in main.rs")
            break
        brace = src.find('{', call_start)
        if brace == -1:
            log.append("ERROR: could not find opening brace for reddit call in main.rs")
            break
        depth = 1
        i = brace + 1
        while i < len(src):
            if src[i] == '{':
                depth += 1
            elif src[i] == '}':
                depth -= 1
                if depth == 0:
                    end = i + 1
                    while end < len(src) and src[end] == '\n':
                        end += 1
                    src = src[:call_start] + "\n" + src[end:]
                    log.append("Removed one reddit_challenge_target call from main.rs")
                    break
            i += 1
        else:
            log.append("ERROR: could not find end of reddit call block in main.rs")
            break
    with open(path, "w") as f:
        f.write(src)


def generalize_comments(repo, log):
    path = os.path.join(repo, "crates", "incognidium-layout", "src", "lib.rs")
    with open(path) as f:
        src = f.read()

    replacements = [
        ("// Non-wrapping flex headers (e.g. a news site masthead) rely on",
         "// Non-wrapping flex headers (e.g. a page masthead) rely on"),
        ("// `height: 100%` (e.g. Guardian circular avatars).",
         "// `height: 100%` (e.g. circular avatar wrappers)."),
        ("// their height (e.g. Guardian circular avatars).",
         "// their height (e.g. circular avatar wrappers)."),
        ("// wrapping on list-heavy mobile pages such as CNN Lite.",
         "// wrapping on list-heavy mobile pages."),
        ("// Regression for a common responsive portal pattern: an inline-block",
         "// Regression for a common responsive content grid: an inline-block"),
        ("// percentage-height children. The Guardian opinion cards use this",
         "// percentage-height children. Some column cards use this"),
    ]
    for old, new in replacements:
        if old in src:
            src = src.replace(old, new, 1)
            log.append("Generalized comment: " + old[:60])
        else:
            log.append("Already absent or changed: " + old[:60])

    old = "// Regression for Wikipedia-style lead sections: a floated image inside a"
    new = "// Regression for lead-section layouts: a floated image inside a"
    if old in src:
        src = src.replace(old, new, 1)
        log.append("Generalized Wikipedia lead-section comment in layout.rs")
    else:
        log.append("Wikipedia lead-section comment already absent in layout.rs")

    old2 = "// Regression for Wikipedia infobox captions: <abbr> and similar"
    new2 = "// Regression for infobox captions: <abbr> and similar"
    if old2 in src:
        src = src.replace(old2, new2, 1)
        log.append("Generalized Wikipedia infobox comment in layout.rs")
    else:
        log.append("Wikipedia infobox comment already absent in layout.rs")

    with open(path, "w") as f:
        f.write(src)

    path = os.path.join(repo, "crates", "incognidium-style", "src", "lib.rs")
    with open(path) as f:
        src = f.read()
    old = "// Regression for Wikipedia infobox captions: <abbr>, <q>, <cite>, and"
    new = "// Regression for infobox captions: <abbr>, <q>, <cite>, and"
    if old in src:
        src = src.replace(old, new, 1)
        log.append("Generalized Wikipedia comment in style.rs")
    else:
        log.append("Wikipedia comment already absent in style.rs")
    with open(path, "w") as f:
        f.write(src)


def clean_batch_test(repo, log):
    path = os.path.join(repo, "scripts", "batch_test.py")
    if not os.path.exists(path):
        log.append("scripts/batch_test.py not found; skipping")
        return
    with open(path) as f:
        src = f.read()

    src = src.replace(
        '    parser.add_argument("--quick", "-q", action="store_true", help="Quick mode: 20 key sites")\n',
        '',
    )
    quick_block_start = src.find("    if args.quick:")
    if quick_block_start != -1:
        else_text = "    else:\n        sites = load_sites(category=args.category, limit=args.limit)"
        else_pos = src.find(else_text, quick_block_start)
        if else_pos != -1:
            src = src[:quick_block_start] + "    sites = load_sites(category=args.category, limit=args.limit)\n" + src[else_pos + len(else_text):]
            log.append("Removed --quick hardcoded site list from batch_test.py")
        else:
            log.append("ERROR: could not find else branch for --quick block")
    else:
        log.append("--quick block already absent from batch_test.py")

    diff_start = src.find("        if os.path.exists(ff_path):")
    if diff_start != -1:
        diff_end = src.find('                result["diff_score"] = -1\n', diff_start)
        if diff_end != -1:
            # Remove the if block through the following blank line.
            blank = src.find("\n\n", diff_end)
            if blank != -1:
                src = src[:diff_start] + src[blank + 1:]
                log.append("Removed pixel/RMSE diff scoring from batch_test.py")
            else:
                log.append("ERROR: could not find blank line after diff block")
        else:
            log.append("Pixel diff block already absent from batch_test.py")
    else:
        log.append("Pixel diff block already absent from batch_test.py")

    src = src.replace(
        '        diff_str = f"diff={r.get(\'diff_score\', -1):3d}" if \'diff_score\' in r else ""\n',
        '',
    )
    src = src.replace(
        '        print(f"  {icon} {g:5s}  {name:25s}  text={r[\'text_boxes\']:5d}  {diff_str}  [{i+1}/{len(sites)}]")\n',
        '        print(f"  {icon} {g:5s}  {name:25s}  text={r[\'text_boxes\']:5d}  [{i+1}/{len(sites)}]")\n',
    )

    grade_start = src.find('    tb = result["text_boxes"]')
    if grade_start != -1:
        else_else = '    else:\n        result["grade"] = "POOR"\n'
        else_else_pos = src.find(else_else, grade_start)
        if else_else_pos != -1:
            end = else_else_pos + len(else_else)
            while end < len(src) and src[end] == '\n':
                end += 1
            new_grade = (
                '    tb = result["text_boxes"]\n'
                '    if not result["inc_ok"]:\n'
                '        result["grade"] = "CRASH"\n'
                '    elif tb < 3:\n'
                '        result["grade"] = "EMPTY"\n'
                '    elif tb > 50:\n'
                '        result["grade"] = "GREAT"\n'
                '    elif tb > 10:\n'
                '        result["grade"] = "OK"\n'
                '    else:\n'
                '        result["grade"] = "POOR"\n'
            )
            src = src[:grade_start] + new_grade + src[end:]
            log.append("Removed pixel-based grading thresholds from batch_test.py")
        else:
            log.append("ERROR: could not find end of grade block")
    else:
        log.append("Pixel-based grading already absent from batch_test.py")

    src = src.replace(
        '    print(f"Commit: {safe_run([\'git\', \'rev-parse\', \'--short\', \'HEAD\']).stdout.strip()}")\n',
        '',
    )
    src = src.replace(
        '        "commit": safe_run(["git", "rev-parse", "--short", "HEAD"]).stdout.strip(),\n',
        '        "commit": "unknown",\n',
    )
    log.append("Removed git rev-parse calls from batch_test.py")

    with open(path, "w") as f:
        f.write(src)


def clean_continuous_qa(repo, log):
    path = os.path.join(repo, "scripts", "continuous_qa.jinx")
    if not os.path.exists(path):
        log.append("scripts/continuous_qa.jinx not found; skipping")
        return
    with open(path) as f:
        src = f.read()

    old_sites = '''SITES=(
  "https://lobste.rs"
  "https://news.ycombinator.com"
  "https://example.com"
  "https://wikipedia.org"
)'''
    new_sites = '''# Read URLs from a file passed as the first argument, or default to a
# single generic placeholder so the script never hardcodes real websites.
SITES_FILE="${1:-}"
if [ -n "$SITES_FILE" ] && [ -f "$SITES_FILE" ]; then
  mapfile -t SITES < "$SITES_FILE"
else
  SITES=("https://example.com")
fi'''
    if old_sites in src:
        src = src.replace(old_sites, new_sites, 1)
        log.append("Removed hardcoded site list from scripts/continuous_qa.jinx")
    else:
        log.append("Hardcoded site list already absent from scripts/continuous_qa.jinx")

    with open(path, "w") as f:
        f.write(src)


def main():
    repo = os.getcwd()
    log_file = "/tmp/fix_remove_site_specific_code.log"
    log = []
    remove_reddit_code(repo, log)
    generalize_comments(repo, log)
    clean_batch_test(repo, log)
    clean_continuous_qa(repo, log)
    with open(log_file, "w") as f:
        f.write("PATCHED=1\n")
        f.write("\n".join(log) + "\n")
    print("\n".join(log))


if __name__ == "__main__":
    main()
