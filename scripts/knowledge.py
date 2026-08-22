#!/usr/bin/env python3
"""
knowledge.py

Thin CLI wrapper around npcpy.memory.knowledge_store.KnowledgeStore.

It does NOT create Markdown memory files or touch any external memory directory.
The source of truth is the directory-local `.knowledge.yaml`, and all read/write
operations are delegated to the real npcpy implementation.

Usage:
    python scripts/knowledge.py read [--directory DIR]
    python scripts/knowledge.py add-memory <final_memory>
        [--directory DIR]
        [--initial-memory TEXT] [--status STATUS]
        [--npc NPC] [--team TEAM] [--model MODEL] [--provider PROVIDER]
    python scripts/knowledge.py add-pending <initial_memory> [--directory DIR]
    python scripts/knowledge.py approve-pending <index>
        [--directory DIR] [--final-memory TEXT]
    python scripts/knowledge.py add-link <from_id> <to_id>
        [--directory DIR] --relation RELATION
"""

import argparse
import os
import sys

import yaml

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NPC = "browser_team"
TEAM = "incognidium"

# Use the real npcpy KnowledgeStore implementation instead of duplicating logic.
NPCPY_ROOT = os.path.expanduser("~/npcpy")
if NPCPY_ROOT not in sys.path:
    sys.path.insert(0, NPCPY_ROOT)

from npcpy.memory.knowledge_store import KnowledgeStore  # noqa: E402


def _store(directory: str) -> KnowledgeStore:
    return KnowledgeStore(directory)


def _add_directory_arg(parser):
    parser.add_argument(
        "--directory",
        default=REPO,
        help="Directory containing the .knowledge.yaml to edit (default: repo root)",
    )


def cmd_read(args):
    store = _store(args.directory)
    data = store.load()
    print(
        yaml.dump(
            data,
            sort_keys=False,
            default_flow_style=False,
            allow_unicode=True,
            width=120,
        )
    )


def cmd_add_memory(args):
    store = _store(args.directory)
    mem_id = store.append_memory(
        message_id="",
        conversation_id="",
        npc=args.npc or NPC,
        team=args.team or TEAM,
        directory_path=args.directory,
        initial_memory=args.initial_memory or args.final_memory,
        final_memory=args.final_memory,
        status=args.status or "human-approved",
        model=args.model or "",
        provider=args.provider or "",
        source_type="manual",
        source_id="",
    )
    print(f"Memory {mem_id} added.")


def cmd_add_pending(args):
    store = _store(args.directory)
    mem_id = store.append_memory(
        message_id="",
        conversation_id="",
        npc=NPC,
        team=TEAM,
        directory_path=args.directory,
        initial_memory=args.initial_memory,
        final_memory=None,
        status="pending_approval",
        model="",
        provider="",
        source_type="manual",
        source_id="",
    )
    print(f"Pending memory {mem_id} added.")


def cmd_approve_pending(args):
    store = _store(args.directory)
    pending = store.get_pending_memories()
    if args.index < 0 or args.index >= len(pending):
        print("Invalid pending index.", file=sys.stderr)
        sys.exit(1)
    mem = pending[args.index]
    final = args.final_memory or mem.get("initial_memory", "")
    store.update_memory(mem.get("id"), "human-approved", final_memory=final)
    print(f"Memory {mem['id']} approved.")


def cmd_add_link(args):
    store = _store(args.directory)
    link_id = store.append_link(args.from_id, args.to_id, args.relation, agent=NPC)
    print(f"Link {link_id} added.")


def main():
    parser = argparse.ArgumentParser(description="Manage .knowledge.yaml")
    sub = parser.add_subparsers(dest="command", required=True)

    read_cmd = sub.add_parser("read", help="Print .knowledge.yaml")
    _add_directory_arg(read_cmd)
    read_cmd.set_defaults(func=cmd_read)

    add_mem = sub.add_parser("add-memory", help="Add an approved memory")
    _add_directory_arg(add_mem)
    add_mem.add_argument("final_memory", help="Final approved memory text")
    add_mem.add_argument("--initial-memory", default=None, help="Raw extracted text")
    add_mem.add_argument("--status", default="human-approved", help="Memory status")
    add_mem.add_argument("--npc", default=NPC, help="NPC name")
    add_mem.add_argument("--team", default=TEAM, help="Team name")
    add_mem.add_argument("--model", default=None, help="Extracting model")
    add_mem.add_argument("--provider", default=None, help="Extracting provider")
    add_mem.set_defaults(func=cmd_add_memory)

    add_pend = sub.add_parser("add-pending", help="Add a pending memory")
    _add_directory_arg(add_pend)
    add_pend.add_argument("initial_memory", help="Raw memory text")
    add_pend.set_defaults(func=cmd_add_pending)

    approve = sub.add_parser("approve-pending", help="Approve a pending memory by index")
    _add_directory_arg(approve)
    approve.add_argument("index", type=int, help="Pending memory index")
    approve.add_argument("--final-memory", default=None, help="Edited final memory")
    approve.set_defaults(func=cmd_approve_pending)

    link = sub.add_parser("add-link", help="Link two memory IDs")
    _add_directory_arg(link)
    link.add_argument("from_id", help="Source memory ID")
    link.add_argument("to_id", help="Target memory ID")
    link.add_argument("--relation", required=True, help="Relation label")
    link.set_defaults(func=cmd_add_link)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
