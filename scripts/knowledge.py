#!/usr/bin/env python3
"""
knowledge.py

File-based knowledge memory helper for Incognidium.

Mirrors npcpy's knowledge-graph and memory-lifecycle tables, but stores the
knowledge graph in a hidden .knowledge.yaml file in the project root and the
approved memory files as
Markdown documents in:
    ~/.claude/projects/-home-caug-incognidium/memory/

Each Markdown memory file uses the structured format required by Claude's
project memory system:

    ---
    name: <kebab-case-slug>
    description: <one-line summary>
    metadata:
      node_type: memory
      type: project
      originSessionId: <uuid>
    ---

    # Title

    Body...

    **Why:** ...
    **How to apply:** ...

Usage:
    python scripts/knowledge.py read
    python scripts/knowledge.py add-memory <name> <description> <fact_statement>
        [--body TEXT] [--why TEXT] [--how-to-apply TEXT]
        [--concept CONCEPT]... [--origin manual|organic|dream]
    python scripts/knowledge.py add-pending <initial_memory>
        [--name NAME] [--description DESC]
    python scripts/knowledge.py approve-pending <index>
        [--final-memory TEXT] [--name NAME] [--description DESC]
        [--concept CONCEPT]... [--body TEXT] [--why TEXT] [--how-to-apply TEXT]
    python scripts/knowledge.py add-fact <statement>
        [--source-text TEXT] [--type TYPE] [--origin ORIGIN]
    python scripts/knowledge.py add-concept <name> [--description DESC] [--origin ORIGIN]
    python scripts/knowledge.py link-fact-concept <fact_statement> <concept_name>
    python scripts/knowledge.py link-fact-fact <statement1> <statement2>
    python scripts/knowledge.py link-concept-concept <concept1> <concept2>
    python scripts/knowledge.py backfill
"""

import argparse
import datetime
import os
import re
import sys
import uuid

import yaml


REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
KNOWLEDGE_PATH = os.path.join(REPO, ".knowledge.yaml")
MEMORY_DIR = os.path.expanduser("~/.claude/projects/-home-caug-incognidium/memory")
MEMORY_INDEX = os.path.join(MEMORY_DIR, "MEMORY.md")


def _now_iso():
    return datetime.datetime.now().isoformat()


def _today():
    return datetime.date.today().isoformat()


def _slugify(name):
    s = name.lower().strip()
    s = re.sub(r"[^a-z0-9\s-]+", "", s)
    s = re.sub(r"\s+", "-", s)
    s = re.sub(r"-+", "-", s)
    return s.strip("-")[:80]


def load_knowledge():
    if not os.path.exists(KNOWLEDGE_PATH):
        return {
            "metadata": {
                "team_name": "incognidium",
                "npc_name": "browser_team",
                "directory_path": REPO,
                "memory_directory": MEMORY_DIR,
                "source": ".knowledge.yaml",
                "version": 3,
                "last_updated": _now_iso(),
            },
            "generation": 0,
            "memory_lifecycle": [],
            "facts": [],
            "concepts": [],
            "fact_to_concept_links": {},
            "fact_to_fact_links": [],
            "concept_links": [],
        }
    with open(KNOWLEDGE_PATH, "r", encoding="utf-8") as f:
        data = yaml.safe_load(f) or {}
    return data


def save_knowledge(data):
    data["metadata"]["last_updated"] = _now_iso()
    with open(KNOWLEDGE_PATH, "w", encoding="utf-8") as f:
        yaml.dump(
            data,
            f,
            sort_keys=False,
            default_flow_style=False,
            allow_unicode=True,
            width=120,
        )


def _fact_index(data, statement):
    for i, f in enumerate(data.get("facts", [])):
        if f.get("statement") == statement:
            return i
    return None


def _concept_index(data, name):
    for i, c in enumerate(data.get("concepts", [])):
        if c.get("name") == name:
            return i
    return None


def _ensure_memory_dir():
    os.makedirs(MEMORY_DIR, exist_ok=True)


def _update_memory_index(name, description):
    _ensure_memory_dir()
    filename = f"{_slugify(name)}.md"
    line = f"- [{description}]({filename}) — {description[:80]}"
    if os.path.exists(MEMORY_INDEX):
        with open(MEMORY_INDEX, "r", encoding="utf-8") as f:
            existing = f.read()
    else:
        existing = ""
    # avoid duplicates
    if filename in existing:
        return
    with open(MEMORY_INDEX, "a", encoding="utf-8") as f:
        if existing and not existing.endswith("\n"):
            f.write("\n")
        f.write(line + "\n")


def _write_memory_file(name, description, body=None, why=None, how_to_apply=None, origin_session_id=None):
    _ensure_memory_dir()
    slug = _slugify(name)
    filename = f"{slug}.md"
    path = os.path.join(MEMORY_DIR, filename)

    if not body:
        body = description
    if not why:
        why = "Documented during an improvement loop so future iterations remember the lesson."
    if not how_to_apply:
        how_to_apply = "Review this memory before starting related work."
    if not origin_session_id:
        origin_session_id = str(uuid.uuid4())

    title = name.replace("-", " ").replace("_", " ").title()

    content = f"""---
name: {slug}
description: {description}
metadata:
  node_type: memory
  type: project
  originSessionId: {origin_session_id}
---

# {title}

{body}

**Why:** {why}
**How to apply:** {how_to_apply}
"""
    with open(path, "w", encoding="utf-8") as f:
        f.write(content)
    _update_memory_index(name, description)
    return filename


def _add_fact(data, statement, source_text="", type_="explicit", origin="manual",
              memory_file=None, generation=None):
    if generation is None:
        generation = data.get("generation", 0)
    idx = _fact_index(data, statement)
    if idx is not None:
        fact = data["facts"][idx]
        if memory_file and not fact.get("memory_file"):
            fact["memory_file"] = memory_file
        if source_text and not fact.get("source_text"):
            fact["source_text"] = source_text
        return fact
    fact = {
        "statement": statement,
        "source_text": source_text or "",
        "type": type_ or "explicit",
        "generation": generation,
        "origin": origin or "manual",
        "memory_file": memory_file,
    }
    data["facts"].append(fact)
    return fact


def _add_concept(data, name, description="", origin="manual", generation=None):
    if generation is None:
        generation = data.get("generation", 0)
    idx = _concept_index(data, name)
    if idx is not None:
        return data["concepts"][idx]
    concept = {
        "name": name,
        "generation": generation,
        "origin": origin or "manual",
        "description": description or "",
    }
    data["concepts"].append(concept)
    return concept


def _link_fact_concept(data, statement, concept_name):
    links = data.setdefault("fact_to_concept_links", {})
    lst = links.setdefault(statement, [])
    if concept_name not in lst:
        lst.append(concept_name)


def cmd_read(_args):
    data = load_knowledge()
    print(yaml.dump(data, sort_keys=False, default_flow_style=False, allow_unicode=True, width=120))


def cmd_add_memory(args):
    data = load_knowledge()
    filename = _write_memory_file(
        args.name,
        args.description,
        body=args.body,
        why=args.why,
        how_to_apply=args.how_to_apply,
    )
    _add_fact(
        data,
        args.fact_statement,
        source_text=f"Approved memory: {args.description}",
        type_="explicit",
        origin=args.origin,
        memory_file=filename,
        generation=data.get("generation", 0),
    )
    for concept_name in args.concept or []:
        _add_concept(data, concept_name)
        _link_fact_concept(data, args.fact_statement, concept_name)
    save_knowledge(data)
    print(f"Memory written to {filename} and linked in knowledge graph.")


def cmd_add_pending(args):
    data = load_knowledge()
    entry = {
        "id": f"pending-{uuid.uuid4().hex[:8]}",
        "timestamp": _now_iso(),
        "initial_memory": args.initial_memory,
        "final_memory": None,
        "status": "pending_approval",
        "name": args.name,
        "description": args.description,
    }
    data.setdefault("memory_lifecycle", []).append(entry)
    save_knowledge(data)
    print(f"Pending memory added (index {len(data['memory_lifecycle']) - 1}).")


def cmd_approve_pending(args):
    data = load_knowledge()
    lifecycle = data.get("memory_lifecycle", [])
    if args.index < 0 or args.index >= len(lifecycle):
        print("Invalid pending index.", file=sys.stderr)
        sys.exit(1)
    item = lifecycle.pop(args.index)
    final = args.final_memory or item.get("initial_memory", "")
    item["final_memory"] = final
    item["status"] = "human-approved"

    name = args.name or item.get("name") or _slugify(final)[:40]
    description = args.description or item.get("description") or final[:100]
    filename = _write_memory_file(
        name,
        description,
        body=args.body or final,
        why=args.why,
        how_to_apply=args.how_to_apply,
    )
    fact_statement = final[:200]
    _add_fact(
        data,
        fact_statement,
        source_text=f"Approved from pending memory {item.get('id')}",
        type_="explicit",
        origin="manual",
        memory_file=filename,
        generation=data.get("generation", 0),
    )
    for concept_name in args.concept or []:
        _add_concept(data, concept_name)
        _link_fact_concept(data, fact_statement, concept_name)
    data.setdefault("memory_lifecycle", []).append(item)
    save_knowledge(data)
    print(f"Approved pending memory and wrote {filename}.")


def cmd_add_fact(args):
    data = load_knowledge()
    _add_fact(
        data,
        args.statement,
        source_text=args.source_text or "",
        type_=args.type or "explicit",
        origin=args.origin or "manual",
    )
    save_knowledge(data)
    print("Fact added.")


def cmd_add_concept(args):
    data = load_knowledge()
    _add_concept(
        data,
        args.name,
        description=args.description or "",
        origin=args.origin or "manual",
    )
    save_knowledge(data)
    print("Concept added.")


def cmd_link_fact_concept(args):
    data = load_knowledge()
    if _fact_index(data, args.statement) is None:
        print(f"Fact not found: {args.statement}", file=sys.stderr)
        sys.exit(1)
    if _concept_index(data, args.concept_name) is None:
        print(f"Concept not found: {args.concept_name}", file=sys.stderr)
        sys.exit(1)
    _link_fact_concept(data, args.statement, args.concept_name)
    save_knowledge(data)
    print("Fact linked to concept.")


def cmd_link_fact_fact(args):
    data = load_knowledge()
    if _fact_index(data, args.statement1) is None:
        print(f"Fact not found: {args.statement1}", file=sys.stderr)
        sys.exit(1)
    if _fact_index(data, args.statement2) is None:
        print(f"Fact not found: {args.statement2}", file=sys.stderr)
        sys.exit(1)
    pair = [args.statement1, args.statement2]
    if pair not in data.get("fact_to_fact_links", []):
        data.setdefault("fact_to_fact_links", []).append(pair)
    save_knowledge(data)
    print("Facts linked.")


def cmd_link_concept_concept(args):
    data = load_knowledge()
    if _concept_index(data, args.concept1) is None:
        print(f"Concept not found: {args.concept1}", file=sys.stderr)
        sys.exit(1)
    if _concept_index(data, args.concept2) is None:
        print(f"Concept not found: {args.concept2}", file=sys.stderr)
        sys.exit(1)
    pair = [args.concept1, args.concept2]
    if pair not in data.get("concept_links", []):
        data.setdefault("concept_links", []).append(pair)
    save_knowledge(data)
    print("Concepts linked.")


def cmd_backfill(_args):
    data = load_knowledge()
    _ensure_memory_dir()
    existing_statements = {f.get("statement") for f in data.get("facts", [])}
    count = 0
    for entry in os.listdir(MEMORY_DIR):
        if not entry.endswith(".md") or entry == "MEMORY.md":
            continue
        path = os.path.join(MEMORY_DIR, entry)
        with open(path, "r", encoding="utf-8") as f:
            content = f.read()
        m = re.search(r"description:\s*(.+)", content)
        description = m.group(1).strip() if m else entry[:-3]
        statement = description
        if statement in existing_statements:
            continue
        _add_fact(
            data,
            statement,
            source_text=f"Backfilled from memory file {entry}",
            type_="explicit",
            origin="manual",
            memory_file=entry,
        )
        count += 1
    save_knowledge(data)
    print(f"Backfilled {count} facts from memory files.")


def main():
    parser = argparse.ArgumentParser(description="Incognidium file-based knowledge memory")
    sub = parser.add_subparsers(dest="command", required=True)

    p_read = sub.add_parser("read", help="Print knowledge.yaml")
    p_read.set_defaults(func=cmd_read)
    p_back = sub.add_parser("backfill", help="Backfill facts from existing Markdown memory files")
    p_back.set_defaults(func=cmd_backfill)

    p_mem = sub.add_parser("add-memory", help="Create a Markdown memory file and add it as a fact")
    p_mem.add_argument("name")
    p_mem.add_argument("description")
    p_mem.add_argument("fact_statement")
    p_mem.add_argument("--body", default="")
    p_mem.add_argument("--why", default="")
    p_mem.add_argument("--how-to-apply", default="")
    p_mem.add_argument("--concept", action="append", help="Concept to link; created if missing")
    p_mem.add_argument("--origin", default="manual")
    p_mem.set_defaults(func=cmd_add_memory)

    p_pen = sub.add_parser("add-pending", help="Add a pending memory lifecycle entry")
    p_pen.add_argument("initial_memory")
    p_pen.add_argument("--name", default=None)
    p_pen.add_argument("--description", default=None)
    p_pen.set_defaults(func=cmd_add_pending)

    p_app = sub.add_parser("approve-pending", help="Approve a pending memory and write a Markdown file")
    p_app.add_argument("index", type=int)
    p_app.add_argument("--final-memory", default=None)
    p_app.add_argument("--name", default=None)
    p_app.add_argument("--description", default=None)
    p_app.add_argument("--body", default=None)
    p_app.add_argument("--why", default=None)
    p_app.add_argument("--how-to-apply", default=None)
    p_app.add_argument("--concept", action="append")
    p_app.set_defaults(func=cmd_approve_pending)

    p_fact = sub.add_parser("add-fact", help="Add a fact without creating a memory file")
    p_fact.add_argument("statement")
    p_fact.add_argument("--source-text", default="")
    p_fact.add_argument("--type", default="explicit")
    p_fact.add_argument("--origin", default="manual")
    p_fact.set_defaults(func=cmd_add_fact)

    p_con = sub.add_parser("add-concept", help="Add a concept")
    p_con.add_argument("name")
    p_con.add_argument("--description", default="")
    p_con.add_argument("--origin", default="manual")
    p_con.set_defaults(func=cmd_add_concept)

    p_lfc = sub.add_parser("link-fact-concept", help="Link a fact to a concept")
    p_lfc.add_argument("statement")
    p_lfc.add_argument("concept_name")
    p_lfc.set_defaults(func=cmd_link_fact_concept)

    p_lff = sub.add_parser("link-fact-fact", help="Link two facts")
    p_lff.add_argument("statement1")
    p_lff.add_argument("statement2")
    p_lff.set_defaults(func=cmd_link_fact_fact)

    p_lcc = sub.add_parser("link-concept-concept", help="Link two concepts")
    p_lcc.add_argument("concept1")
    p_lcc.add_argument("concept2")
    p_lcc.set_defaults(func=cmd_link_concept_concept)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
