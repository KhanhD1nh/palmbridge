#!/usr/bin/env python3
"""Copy the Hands crate into a grok-build checkout and register it."""

from __future__ import annotations

import shutil
import sys
from pathlib import Path

MEMBER = '    "crates/codegen/palmbridge",'
OLD_MEMBER = '    "crates/codegen/grok-harness",'
LEGACY_MEMBER = '    "crates/codegen/hands",'


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: inject.py <hands-repo> <grok-build-checkout>", file=sys.stderr)
        return 2
    src_repo = Path(sys.argv[1]).resolve()
    grok_build = Path(sys.argv[2]).resolve()
    crate_src = src_repo / "crate"
    dest = grok_build / "crates" / "codegen" / "palmbridge"
    if not (crate_src / "Cargo.toml").is_file():
        print(f"missing crate at {crate_src}", file=sys.stderr)
        return 1
    old = grok_build / "crates" / "codegen" / "grok-harness"
    legacy_dest = grok_build / "crates" / "codegen" / "hands"
    if legacy_dest.exists():
        shutil.rmtree(legacy_dest)
    if old.exists():
        shutil.rmtree(old)
    if dest.exists():
        shutil.rmtree(dest)
    shutil.copytree(crate_src, dest)

    root = grok_build / "Cargo.toml"
    text = root.read_text()
    if OLD_MEMBER in text:
        text = text.replace(OLD_MEMBER, MEMBER, 1)
        root.write_text(text)
    elif LEGACY_MEMBER in text:
        text = text.replace(LEGACY_MEMBER, MEMBER, 1)
        root.write_text(text)
    elif MEMBER.strip() not in text:
        needle = '    "crates/codegen/xai-grok-tools",'
        if needle not in text:
            print("could not find xai-grok-tools member in grok-build Cargo.toml", file=sys.stderr)
            return 1
        text = text.replace(needle, needle + "\n" + MEMBER, 1)
        root.write_text(text)
    print(f"injected {dest}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
