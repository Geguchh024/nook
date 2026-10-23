"""Refreshes the Claude Code skill's copied files and packs it for the website.

    python skills/pack.py

- copies sdk/python/nook.py and sdk/node/nook.js into nook-plugin/assets/
- regenerates nook-plugin/references/icons.txt from the plugin host's icon table
- writes site/public/downloads/nook-plugin-skill.zip (a nook-plugin/ folder inside)

Run it after changing the SDKs, the icon table or anything in nook-plugin/.
"""

import re
import shutil
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SKILL = ROOT / "skills" / "nook-plugin"
ZIP = ROOT / "site" / "public" / "downloads" / "nook-plugin-skill.zip"


def main():
    shutil.copyfile(ROOT / "sdk" / "python" / "nook.py", SKILL / "assets" / "nook.py")
    shutil.copyfile(ROOT / "sdk" / "node" / "nook.js", SKILL / "assets" / "nook.js")

    table = (ROOT / "crates" / "modules" / "src" / "plugins" / "icon_names.rs").read_text(encoding="utf-8")
    icons = re.findall(r'^\s*\("([a-z0-9-]+)",', table, re.M)
    (SKILL / "references" / "icons.txt").write_text("\n".join(icons) + "\n", encoding="utf-8", newline="\n")

    ZIP.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(ZIP, "w", zipfile.ZIP_DEFLATED) as z:
        for f in sorted(SKILL.rglob("*")):
            if f.is_file() and "__pycache__" not in f.parts:
                z.write(f, Path("nook-plugin") / f.relative_to(SKILL))
    print(f"{len(icons)} icons; wrote {ZIP.relative_to(ROOT)} ({ZIP.stat().st_size // 1024} KB)")


if __name__ == "__main__":
    main()
