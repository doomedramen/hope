#!/usr/bin/env python3
"""Regenerate crates/hope-server/data/oui.tsv, the MAC vendor table the server embeds.

    python3 -I scripts/update-oui.py [URL-or-path of oui-data index.json]

Source: the IEEE MA-L, MA-M and MA-S registries as published by the oui-data package
(https://github.com/silverwind/oui-data, BSD-2-Clause; see crates/hope-server/data/OUI-NOTICE).
Each output line is `PREFIX<TAB>Vendor`, PREFIX being 6, 7 or 9 upper-case hex digits (24, 28
or 36 bits); the server tries the longest prefix first. Vendor names keep only the first line
of the registry entry, without legal-form suffixes, so the console can show them as is.
"""
import json
import re
import sys
import urllib.request
from pathlib import Path

DEFAULT = "https://raw.githubusercontent.com/silverwind/oui-data/master/index.json"
OUT = Path(__file__).resolve().parent.parent / "crates/hope-server/data/oui.tsv"
SUFFIX = re.compile(
    r"[\s,.]+(co\.?,?\s*ltd\.?|ltd\.?|limited|inc\.?|incorporated|gmbh(\s*&\s*co\.?\s*kg)?|ag|kg|"
    r"corporation|corp\.?|company|co\.?|llc|l\.l\.c\.|s\.?a\.?|s\.?a\.?s\.?|s\.?r\.?l\.?|b\.?v\.?|"
    r"n\.?v\.?|a/s|ab|oy|pty|pte|plc|sp\.?\s*z\s*o\.?o\.?|technologies|technology|electronics?)$",
    re.IGNORECASE,
)


def tidy(name: str) -> str:
    name = " ".join(name.split()).strip(" ,.")
    previous = None
    while previous != name:
        previous = name
        name = SUFFIX.sub("", name).strip(" ,.")
    words = [w.capitalize() if w.isupper() and len(w) > 4 and w.isalpha() else w for w in name.split()]
    return " ".join(words) or previous


def main() -> None:
    source = sys.argv[1] if len(sys.argv) > 1 else DEFAULT
    if "://" in source:
        with urllib.request.urlopen(source, timeout=120) as response:
            data = json.load(response)
    else:
        data = json.loads(Path(source).read_text())
    rows = []
    for prefix, entry in data.items():
        prefix = prefix.upper()
        if len(prefix) not in (6, 7, 9) or any(c not in "0123456789ABCDEF" for c in prefix):
            continue
        vendor = tidy(entry.split("\n", 1)[0])
        if vendor and "\t" not in vendor:
            rows.append(f"{prefix}\t{vendor}")
    rows.sort()
    OUT.write_text("\n".join(rows) + "\n")
    print(f"wrote {len(rows)} prefixes to {OUT}")


if __name__ == "__main__":
    main()
