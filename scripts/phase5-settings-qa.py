import argparse
import json
import sqlite3
from pathlib import Path


parser = argparse.ArgumentParser()
parser.add_argument("database")
parser.add_argument("--mode", choices=("floating", "sidebar", "desktop"))
parser.add_argument("--presentation", choices=("collapsed", "expanded"))
parser.add_argument("--x", type=int)
parser.add_argument("--y", type=int)
parser.add_argument("--width", type=int)
parser.add_argument("--height", type=int)
parser.add_argument("--background", choices=("glass", "solid", "gradient", "image", "wallpaper"))
parser.add_argument("--always-on-top", choices=("true", "false"))
parser.add_argument("--backup")
parser.add_argument("--restore")
parser.add_argument("--transparent-css", action="store_true")
parser.add_argument("--quiet", action="store_true")
args = parser.parse_args()

connection = sqlite3.connect(args.database)


def every_profile(settings):
    """The appearance value(s) carried by a settings document.

    v1 stores one `appearanceSettings`; per-mode profiles store three. Seeding a
    background for QA means the same thing either way: the product shows it.
    """
    if "appearanceProfiles" in settings:
        return list(settings["appearanceProfiles"].values())
    return [settings["appearanceSettings"]]


def set_background(settings, background):
    for appearance in every_profile(settings):
        appearance["backgroundType"] = background


if args.restore:
    backup_path = Path(args.restore)
    raw_settings = backup_path.read_text(encoding="utf-8")
    connection.execute(
        "UPDATE app_settings SET value = ? WHERE key = 'product_settings'",
        (raw_settings,),
    )
    connection.commit()
    connection.close()
    backup_path.unlink()
    print("Restored exact product_settings and removed the temporary backup")
    raise SystemExit(0)

row = connection.execute(
    "SELECT value FROM app_settings WHERE key = 'product_settings'"
).fetchone()
if row is None:
    raise SystemExit("product_settings was not found")
raw_settings = row[0]
settings = json.loads(raw_settings)
if not args.quiet:
    print(json.dumps(settings, indent=2, ensure_ascii=False))
if args.backup:
    backup_path = Path(args.backup)
    if backup_path.exists():
        raise SystemExit(f"backup already exists: {backup_path}")
    backup_path.write_text(raw_settings, encoding="utf-8")

if args.mode or args.presentation or args.x is not None or args.y is not None or args.width or args.height or args.background or args.always_on_top or args.transparent_css:
    if args.mode:
        settings["mode"] = args.mode
    if args.presentation:
        settings["floatingPresentation"] = args.presentation
    if args.x is not None:
        settings["x"] = args.x
    if args.y is not None:
        settings["y"] = args.y
    if args.width:
        settings["width"] = args.width
    if args.height:
        settings["height"] = args.height
    if args.background:
        set_background(settings, args.background)
    if args.always_on_top:
        settings["alwaysOnTop"] = args.always_on_top == "true"
    if args.transparent_css:
        for appearance in every_profile(settings):
            appearance["backgroundType"] = "glass"
            appearance["glassTintOpacity"] = 0
            appearance["blurPx"] = 0
            appearance["overlayStrength"] = 0
            appearance["backgroundOpacity"] = 0
    connection.execute(
        "UPDATE app_settings SET value = ? WHERE key = 'product_settings'",
        (json.dumps(settings, separators=(",", ":"), ensure_ascii=False),),
    )
    connection.commit()
connection.close()
