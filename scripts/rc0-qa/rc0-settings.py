#!/usr/bin/env python3
"""RC-0 QA helper: read/patch/restore the persisted product settings row.

Used only to seed the window mode and floating presentation before a restart so
the product's own startup path builds the Orb. `floatingPresentation` is exactly
what the real `floating.collapse` action persists, so the state under test is the
state the product produces.

Uses a parameterized SQLite statement: shelling out to the sqlite3 CLI loses the
JSON's double quotes to PowerShell argument re-parsing.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sqlite3
import sys


def db_path() -> str:
    roaming = os.environ.get("APPDATA")
    if not roaming:
        sys.exit("APPDATA is not set")
    return os.path.join(roaming, "net.alanfloyd.desktop", "alan-desktop.sqlite3")


def read_settings(conn: sqlite3.Connection) -> str:
    row = conn.execute(
        "select value from app_settings where key = 'product_settings'"
    ).fetchone()
    if row is None:
        sys.exit("no product_settings row")
    if not row[0]:
        sys.exit("product_settings row is empty")
    return row[0]


def write_settings(conn: sqlite3.Connection, value: str) -> None:
    conn.execute(
        "update app_settings set value = ? where key = 'product_settings'", (value,)
    )
    conn.commit()


def patch(current: str, pairs: dict[str, str]) -> str:
    """Replace only the named JSON keys, preserving key order and formatting."""
    parsed = json.loads(current)  # validate before touching anything
    for key, raw in pairs.items():
        if key not in parsed:
            sys.exit(f"key {key!r} is not present in the persisted settings")
        value: object
        if raw == "null":
            value = None
        elif raw in ("true", "false"):
            value = raw == "true"
        elif re.fullmatch(r"-?\d+", raw):
            value = int(raw)
        else:
            value = raw
        # Keep the existing payload shape: erase the old token, insert the new.
        old = json.dumps(parsed[key])
        pattern = re.compile(r'("' + re.escape(key) + r'":)' + re.escape(old))
        replaced, count = pattern.subn(
            lambda m: m.group(1) + json.dumps(value), current, count=1
        )
        if count != 1:
            sys.exit(f"could not locate the serialized form of {key!r}")
        current = replaced
    json.loads(current)  # validate the result
    return current


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=["show", "patch", "restore"])
    parser.add_argument("--set", dest="pairs", default="")
    parser.add_argument("--save-to", default="")
    parser.add_argument("--restore-from", default="")
    args = parser.parse_args()

    conn = sqlite3.connect(db_path())
    try:
        if args.action == "show":
            print(read_settings(conn))
            return

        if args.action == "restore":
            with open(args.restore_from, encoding="utf-8") as handle:
                value = handle.read().strip()
            json.loads(value)
            write_settings(conn, value)
            print(f"restored from {args.restore_from}")
            print(read_settings(conn))
            return

        current = read_settings(conn)
        if args.save_to:
            with open(args.save_to, "w", encoding="utf-8") as handle:
                handle.write(current)
            print(f"saved -> {args.save_to}")

        pairs: dict[str, str] = {}
        for item in args.pairs.split(","):
            if not item.strip():
                continue
            key, _, value = item.partition("=")
            pairs[key.strip()] = value.strip()

        write_settings(conn, patch(current, pairs))
        print(f"patched: {args.pairs}")
        print(read_settings(conn))
    finally:
        conn.close()


if __name__ == "__main__":
    main()
