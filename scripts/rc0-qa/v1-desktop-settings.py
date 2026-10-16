"""Back up / restore the product_settings row around v1 Desktop usability QA."""

import json
import os
import sqlite3
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]

APP_DIR = os.path.join(os.environ["APPDATA"], "net.alanfloyd.desktop")
DB = os.path.join(APP_DIR, "alan-desktop.sqlite3")
EVIDENCE = REPO_ROOT / ".rc0-evidence" / "v1-desktop"


def connect(path: str):
    return sqlite3.connect(path)


def open_readonly():
    return sqlite3.connect("file:" + DB + "?mode=ro", uri=True)


def backup() -> None:
    os.makedirs(EVIDENCE, exist_ok=True)
    dst = os.path.join(EVIDENCE, "alan-desktop.pre-v1-backup.sqlite3")
    src = connect(DB)
    out = connect(dst)
    src.backup(out)
    out.close()
    src.close()
    row = read_row()
    with open(
        os.path.join(EVIDENCE, "product_settings.pre-v1.json"), "w", encoding="utf-8"
    ) as handle:
        handle.write(row)
    parsed = json.loads(row)
    print(
        "backup ok ->",
        dst,
        "| mode =",
        parsed.get("mode"),
        "| renderingBackend =",
        parsed.get("renderingBackend"),
    )


def read_row() -> str:
    conn = open_readonly()
    row = conn.execute(
        "select value from app_settings where key='product_settings'"
    ).fetchone()[0]
    conn.close()
    return row


def set_keys(**updates) -> None:
    row = read_row()
    parsed = json.loads(row)
    before = {key: parsed.get(key) for key in updates}
    parsed.update(updates)
    conn = connect(DB)
    conn.execute(
        "update app_settings set value = ? where key = 'product_settings'",
        (json.dumps(parsed, separators=(",", ":")),),
    )
    conn.commit()
    conn.close()
    print("set", updates, "(was", before, ")")


def show() -> None:
    parsed = json.loads(read_row())
    keys = [
        "mode",
        "renderingBackend",
        "locked",
        "floatingPresentation",
        "desktopX",
        "desktopY",
        "desktopWidth",
        "desktopHeight",
    ]
    print({key: parsed.get(key) for key in keys})


def restore() -> None:
    path = os.path.join(EVIDENCE, "product_settings.pre-v1.json")
    with open(path, encoding="utf-8") as handle:
        row = handle.read()
    conn = connect(DB)
    conn.execute(
        "update app_settings set value = ? where key = 'product_settings'", (row,)
    )
    conn.commit()
    conn.close()
    print("restored product_settings from", path)
    show()


def write_from(path: str) -> None:
    with open(path, encoding="utf-8") as handle:
        row = handle.read()
    parsed = json.loads(row)
    conn = connect(DB)
    conn.execute(
        "update app_settings set value = ? where key = 'product_settings'", (row,)
    )
    conn.commit()
    conn.close()
    print(
        "wrote product_settings from",
        path,
        "| mode =",
        parsed.get("mode"),
        "| presentation =",
        parsed.get("floatingPresentation"),
    )


if __name__ == "__main__":
    command = sys.argv[1] if len(sys.argv) > 1 else "show"
    if command == "backup":
        backup()
    elif command == "show":
        show()
    elif command == "restore":
        restore()
    elif command == "write":
        write_from(sys.argv[2])
    else:
        updates = dict(pair.split("=", 1) for pair in sys.argv[2:])
        coerced = {}
        for key, value in updates.items():
            if value in ("true", "false"):
                coerced[key] = value == "true"
            elif value == "null":
                coerced[key] = None
            elif value.lstrip("-").isdigit():
                coerced[key] = int(value)
            else:
                coerced[key] = value
        set_keys(**coerced)
