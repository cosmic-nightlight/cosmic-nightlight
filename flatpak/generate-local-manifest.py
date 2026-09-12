#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0
"""Derive the working-tree Flatpak manifest from the release manifest."""

import json
from pathlib import Path


def main():
    directory = Path(__file__).resolve().parent
    manifest = json.loads((directory / "io.github.cosmic_nightlight.json").read_text())
    module = next(m for m in manifest["modules"] if m["name"] == "cosmic-nightlight")
    module["sources"][0] = {
        "type": "dir",
        "path": "..",
        "skip": [
            ".git",
            "target",
            "flatpak/build",
            "flatpak/.flatpak-builder",
            "flatpak/venv",
        ],
    }
    output = directory / "io.github.cosmic_nightlight.local.json"
    output.write_text(json.dumps(manifest, indent=4) + "\n")
    print(output)


if __name__ == "__main__":
    main()
