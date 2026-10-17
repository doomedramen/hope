#!/usr/bin/env python3
"""Refresh the small name-to-artwork catalog used by the web UI."""

import json
from pathlib import Path
from urllib.request import Request, urlopen


ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "web/data/icon-catalog.json"
SOURCES = {
    "apps": "https://cdn.jsdelivr.net/gh/selfhst/icons@main/index.json",
    "unifi": "https://static.ui.com/fingerprint/0/devicelist.json",
}


def get_json(url: str):
    request = Request(url, headers={"User-Agent": "hope-icon-catalog-refresh/1.0"})
    with urlopen(request, timeout=30) as response:
        return json.load(response)


apps = [
    {"slug": entry["Reference"], "name": entry["Name"]}
    for entry in get_json(SOURCES["apps"])
    if entry.get("PNG") == "Yes" and entry.get("Reference") and entry.get("Name")
]
unifi_source = get_json(SOURCES["unifi"])["dev_ids"]
unifi = [
    {"id": str(identifier), "name": entry["name"]}
    for identifier, entry in unifi_source.items()
    if entry.get("name")
]
catalog = {
    "sources": SOURCES,
    "apps": sorted(apps, key=lambda entry: entry["slug"]),
    "unifi": sorted(unifi, key=lambda entry: int(entry["id"])),
}
OUTPUT.parent.mkdir(parents=True, exist_ok=True)
OUTPUT.write_text(json.dumps(catalog, ensure_ascii=False, separators=(",", ":")) + "\n")
print(f"Wrote {len(apps)} app icons and {len(unifi)} UniFi fingerprints to {OUTPUT}")
