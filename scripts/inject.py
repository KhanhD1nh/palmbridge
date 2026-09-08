#!/usr/bin/env python3
"""Copy the Palmbridge crate into a grok-build checkout and register it."""

from __future__ import annotations

import shutil
import sys
from pathlib import Path

MEMBER = '    "crates/codegen/palmbridge",'
OLD_MEMBER = '    "crates/codegen/grok-harness",'
LEGACY_MEMBER = '    "crates/codegen/hands",'


def quiet_upstream_warnings(grok_build: Path) -> None:
    """Apply text fixes for known warnings in vendored upstream crates.

    Each fix is idempotent and skipped silently if upstream changes shape.
    """
    codegen = grok_build / "crates" / "codegen"
    file_fixes = {
        codegen
        / "xai-grok-tools"
        / "src"
        / "computer"
        / "local"
        / "terminal.rs": [
            (
                "async fn collect_shell_state_dumps(&mut self, task_ids: &[String])",
                "async fn collect_shell_state_dumps(&mut self, _task_ids: &[String])",
            ),
            (
                "let mut build_cmd = |with_breakaway: bool| {",
                "let build_cmd = |with_breakaway: bool| {",
            ),
            (
                "    login_shell_capture: bool,\n",
                "    #[allow(dead_code)]\n    login_shell_capture: bool,\n",
            ),
        ],
        codegen / "xai-tty-utils" / "src" / "lib.rs": [
            (
                "use std::os::windows::io::{AsRawHandle, FromRawHandle};",
                "use std::os::windows::io::FromRawHandle;",
            ),
        ],
        codegen / "xai-grok-config" / "src" / "managed_text" / "source.rs": [
            (
                "pub(super) struct ParentAnchor {\n    path: PathBuf,\n    identity: FileIdentity,\n    directory: fs::File,\n}",
                "pub(super) struct ParentAnchor {\n    path: PathBuf,\n    identity: FileIdentity,\n    #[allow(dead_code)]\n    directory: fs::File,\n}",
            ),
        ],
    }
    for path, fixes in file_fixes.items():
        if not path.is_file():
            continue
        text = path.read_text(encoding="utf-8")
        changed = False
        for old, new in fixes:
            if old in text:
                text = text.replace(old, new, 1)
                changed = True
        if changed:
            path.write_text(text, encoding="utf-8")


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: inject.py <palmbridge-repo> <grok-build-checkout>", file=sys.stderr)
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

    # Silence known upstream warnings in xai-grok-tools (fixed upstream pending).
    quiet_upstream_warnings(grok_build)

    root = grok_build / "Cargo.toml"


if __name__ == "__main__":
    raise SystemExit(main())
