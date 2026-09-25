#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
# SPDX-License-Identifier: Apache-2.0
"""Regenerate docs/STDLIB.md from `cig --json language` so the reference can never drift."""
import json, subprocess, sys, pathlib
root = pathlib.Path(__file__).resolve().parent.parent
cig = sys.argv[1] if len(sys.argv) > 1 else str(root / "target/release/cig")
data = json.loads(subprocess.check_output([cig, "--json", "language"]))
out = ["# Standard library reference", "",
       f"Generated from `cig --json language` (CigScript {data['version']}) by `scripts/gen-stdlib-doc.py`. Do not edit by hand.", "",
       "Functions marked **burn** change the world and can only be called inside a `burn { }` block. "
       "`read` functions look at the world without changing it; `pure` functions touch nothing.", "",
       "Methods are called on a value (`\"x\".upper()`, `list.map(f)`, `map.has(\"k\")`); the receiver is not counted in the arity column.", ""]
for group in data["library"]:
    out.append(f"## {group['group']}"); out.append("")
    out.append("| function | args | effect |"); out.append("|---|---|---|")
    for f in group["functions"]:
        lo, hi = f["min_args"], f["max_args"]
        is_method = group["group"].endswith("methods")
        if is_method:
            lo -= 1
            hi = None if hi is None else hi - 1
        arity = f"{lo}" if hi == lo else (f"{lo}+" if hi is None else f"{lo}-{hi}")
        eff = "**burn**" if f["needs_burn"] else f["effect"]
        name = f["name"].split(".", 1)[1] if is_method else f["name"]
        out.append(f"| `{name}` | {arity} | {eff} |")
    out.append("")
(root / "docs/STDLIB.md").write_text("\n".join(out))
print("wrote docs/STDLIB.md")
