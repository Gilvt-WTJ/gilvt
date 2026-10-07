#!/usr/bin/env bash
# Re-imports the built-in themes (Ghostty format) from iTerm2-Color-Schemes at a pinned commit.
# Usage: scripts/update-themes.sh [COMMIT]
set -euo pipefail
commit="${1:-31756e77934045c66b28f59463ddb2870606d7ca}"
here="$(cd "$(dirname "$0")/.." && pwd)"
dest="$here/crates/gilvt-theme/themes"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "https://codeload.github.com/mbadolato/iTerm2-Color-Schemes/tar.gz/$commit" | tar -xz -C "$tmp"
src="$(echo "$tmp"/iTerm2-Color-Schemes-*)"
rm -rf "$dest/ghostty"
mkdir -p "$dest/ghostty"
cp "$src"/ghostty/* "$dest/ghostty/"
cp "$src/LICENSE" "$dest/LICENSE"
cat > "$dest/SOURCE.md" <<EOF
# Built-in themes

Copied from https://github.com/mbadolato/iTerm2-Color-Schemes (directory \`ghostty/\`) at commit
\`$commit\` by \`scripts/update-themes.sh\`. License: MIT (\`LICENSE\` in this directory).
EOF
echo "$(ls "$dest/ghostty" | wc -l | tr -d ' ') themes imported at $commit"
