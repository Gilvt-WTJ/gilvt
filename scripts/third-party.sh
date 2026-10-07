#!/bin/bash
# Regenerates THIRD_PARTY_LICENSES.md (name / version / SPDX license of every Rust dependency) from
# `cargo metadata`, limited to what ships: the crates `cargo tree -e normal,build` reaches (all targets), so
# dev-dependencies and the features they switch on (gpui's test-support) are left out. Fails when a dependency has no license field or a copyleft license that needs review.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

shipped="$(cargo tree --workspace -e normal,build --target all --locked --prefix none --format '{p}' 2>/dev/null)"
if [ -z "$shipped" ]; then
  echo "third-party.sh: cargo tree listed no crates (run it by hand to see why); THIRD_PARTY_LICENSES.md left unchanged" >&2
  exit 1
fi

cargo metadata --format-version 1 --locked 2>/dev/null | SHIPPED="$shipped" python3 -c '
import collections, json, os, sys
m = json.load(sys.stdin)
ws = {p["id"] for p in m["packages"] if p["source"] is None}
shipped = {tuple(l.split()[:2]) for l in os.environ["SHIPPED"].splitlines() if l.strip()}
shipped = {(n, v.lstrip("v")) for n, v in shipped}
rows = sorted({(p["name"], p["version"], p.get("license") or "UNKNOWN") for p in m["packages"]
               if p["id"] not in ws and (p["name"], p["version"]) in shipped})
c = collections.Counter(r[2] for r in rows)
out = ["# Third-party licenses", "",
       "Generated from `cargo metadata` (shipped Rust dependencies only; run `scripts/third-party.sh` to refresh). "
       "Full license texts ship inside each crate; `crates/gilvt-mermaid/assets/mermaid-LICENSE` covers the bundled mermaid.js; `crates/gilvt-theme/themes/LICENSE` (MIT, iTerm2-Color-Schemes) covers the built-in themes.",
       "", "## Summary", "", "| License | Crates |", "|---|---|"]
out += [f"| {l} | {n} |" for l, n in c.most_common()]
out += ["", "## Crates", "", "| Crate | Version | License |", "|---|---|---|"]
out += [f"| {n} | {v} | {l} |" for n, v, l in rows]
open("THIRD_PARTY_LICENSES.md", "w").write("\n".join(out) + "\n")
bad = [r for r in rows if r[2] == "UNKNOWN" or ("GPL" in r[2] and "OR" not in r[2])]
if bad:
    sys.exit("needs license review: " + ", ".join(f"{n} {v} ({l})" for n, v, l in bad))
'
echo "wrote THIRD_PARTY_LICENSES.md"
