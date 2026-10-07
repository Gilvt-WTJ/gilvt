#!/usr/bin/env python3
"""Checks that the gilvt-acceptance skill only refers to things that exist: its frontmatter names the
skill, every `drive.sh <action>` / `sandbox.sh <subcommand>` is dispatched by that script, and every
repo path (tests/…, scripts/…, docs/…, crates/…) exists. Used by selftest.sh. Python 3.9 compatible.

  skill_lint.py SKILL_MD REPO
"""

import os
import re
import sys

# A path of this repo; placeholders (<节>, *, $VAR, …) are not checked.
PATH_RE = re.compile(r"(?<![\w./~-])((?:tests|scripts|docs|crates)/[^\s`'\"()，。；、（）「」]+)")
COMMAND_RE = re.compile(r"\b(drive|sandbox)\.sh[ \t]+([^\s`'\"]+)")


def dispatched(script):
    """The words of the script's last top-level `case … in` block (`  a | b)` lines, `*` excluded)."""
    with open(script, encoding="utf-8") as f:
        lines = f.read().splitlines()
    start = max(i for i, l in enumerate(lines) if re.match(r"^case .* in$", l))
    words = set()
    for l in lines[start + 1:]:
        if l.startswith("esac"):
            break
        m = re.match(r"^  ([a-z][a-z-]*(?: \| [a-z][a-z-]*)*)\)", l)
        if m:
            words.update(w.strip() for w in m.group(1).split("|"))
    return words


def lint(skill, repo):
    with open(skill, encoding="utf-8") as f:
        text = f.read()
    errors = []
    front = re.match(r"^---\n(.*?)\n---\n", text, re.S)
    if not front:
        errors.append("no frontmatter")
    else:
        if not re.search(r"^name: gilvt-acceptance$", front.group(1), re.M):
            errors.append("frontmatter: name is not gilvt-acceptance")
        if not re.search(r"^description: \S", front.group(1), re.M):
            errors.append("frontmatter: no description")
    gui = os.path.join(repo, "tests", "gui")
    known = {"drive": dispatched(os.path.join(gui, "drive.sh")), "sandbox": dispatched(os.path.join(gui, "sandbox.sh"))}
    for no, line in enumerate(text.splitlines(), 1):
        for script, word in COMMAND_RE.findall(line):
            # Only words that look like an action; `<动作>`, `…` and options are prose.
            if re.match(r"^[a-z][a-z-]*$", word) and word not in known[script]:
                errors.append("line %d: %s.sh has no action %r" % (no, script, word))
        for path in PATH_RE.findall(line):
            path = path.rstrip(".:,")
            if re.search(r"[<*$…]", path):
                continue
            if not os.path.exists(os.path.join(repo, path)):
                errors.append("line %d: no such file %s" % (no, path))
    return errors


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.stderr.write(__doc__)
        sys.exit(2)
    errs = lint(sys.argv[1], sys.argv[2])
    for e in errs:
        sys.stderr.write("skill: %s\n" % e)
    sys.exit(1 if errs else 0)
