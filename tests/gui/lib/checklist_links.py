#!/usr/bin/env python3
"""Keeps new checklist rows from shipping without acceptance cases (tests/gui/README.md, 「新功能的验收用例」).
Used by selftest.sh. Python 3.9 compatible.

  checklist_links.py CHECKLIST CASES_DIR

Every section of docs/compat-checklist.md (`## X. …`, one capital letter) that is not in LEGACY must have a
「用例」 column, and each of its rows must, in that column:
  - link to a case file ([ID](../tests/gui/cases/<sec>/<ID>.md), relative to the checklist) that exists and
    whose `checklist:` header names the row, or
  - say 手动 / 真实 claude / 真实 codex followed by the reason in parentheses: 手动（访达「放回原处」）.
    真实 … still needs a linked case file, with `requires: real-claude` / `real-codex`; a linked case that
    requires real-* or manual must carry the matching marker (the reader sees why it is not automatic).
And every case file's `checklist:` ID (other than —) must be a row of the checklist.
"""

import os
import re
import sys

# Sections from before the acceptance cases existed, checked by hand only. Migrating a section (adding its
# 用例 column and cases) removes it from this list.
LEGACY = ("A", "B", "C", "D", "E", "F", "G")

MARKERS = {"手动": "manual", "真实 claude": "real-claude", "真实 codex": "real-codex"}
LINK = re.compile(r"\[[^\]]*\]\(([^)]+\.md)\)")
MARKER = re.compile(r"(手动|真实 claude|真实 codex)\s*(（([^）]*)）|\(([^)]*)\))?")


def cells(line):
    """The cells of a Markdown table row (`\\|` inside a cell does not split it)."""
    parts = re.split(r"(?<!\\)\|", line.strip())
    return [c.strip() for c in parts[1:-1]]


def case_header(path):
    """(requires, checklist) from a case file's header lines."""
    found = {}
    with open(path, encoding="utf-8") as f:
        for line in f:
            m = re.match(r"(requires|checklist):\s*(\S+)", line)
            if m and m.group(1) not in found:
                found[m.group(1)] = m.group(2)
            if line.startswith("## "):
                break
    return found.get("requires"), found.get("checklist")


def check_cell(rid, cell, doc_dir):
    errors = []
    links = LINK.findall(cell)
    markers = MARKER.findall(cell)
    requires = None
    for target in links:
        path = os.path.normpath(os.path.join(doc_dir, target))
        if not os.path.isfile(path):
            errors.append("%s: the 用例 column links to %s, which does not exist" % (rid, target))
            continue
        requires, names = case_header(path)
        if names != rid:
            errors.append("%s: %s is the case of %s (its checklist: header), not of %s" % (rid, target, names, rid))
    for word, _, full, ascii_ in markers:
        if not (full or ascii_).strip():
            errors.append("%s: 「%s」 needs the reason in parentheses, e.g. %s（…为什么不能自动…）" % (rid, word, word))
    said = [MARKERS[w] for w, _, _, _ in markers]
    if not links and not said:
        errors.append("%s: no case (link one under tests/gui/cases/, or say 手动（原因）)" % rid)
    for kind in ("real-claude", "real-codex"):
        word = [w for w, k in MARKERS.items() if k == kind][0]
        if kind in said and requires != kind:
            errors.append("%s: 「%s」 needs a linked case file with requires: %s" % (rid, word, kind))
    if requires in ("real-claude", "real-codex", "manual") and requires not in said:
        word = [w for w, k in MARKERS.items() if k == requires][0]
        errors.append("%s: the linked case requires %s; say %s（原因） in the 用例 column" % (rid, requires, word))
    return errors


def check(doc, cases):
    with open(doc, encoding="utf-8") as f:
        text = f.read()
    doc_dir = os.path.dirname(os.path.abspath(doc))
    errors = []
    all_rows = set()
    for m in re.finditer(r"^## ([A-Z])\. .*?(?=^## |\Z)", text, re.S | re.M):
        sec, part = m.group(1), m.group(0)
        rows = [l for l in part.splitlines() if re.match(r"\| %s\d+ \|" % sec, l)]
        all_rows.update(cells(l)[0] for l in rows)
        if sec in LEGACY:
            continue
        header = next((cells(l) for l in part.splitlines() if l.startswith("| #")), [])
        if "用例" not in header:
            errors.append("section %s: no 用例 column (every section after %s needs one)" % (sec, LEGACY[-1]))
            continue
        col = header.index("用例")
        if not rows:
            errors.append("section %s: no rows" % sec)
        for line in rows:
            row = cells(line)
            cell = row[col] if col < len(row) else ""
            errors.extend(check_cell(row[0], cell, doc_dir))
    for sub in sorted(os.listdir(cases)) if os.path.isdir(cases) else []:
        d = os.path.join(cases, sub)
        for name in sorted(os.listdir(d)) if os.path.isdir(d) else []:
            if not name.endswith(".md"):
                continue
            _, names = case_header(os.path.join(d, name))
            if names and names not in ("—", "-") and names not in all_rows:
                errors.append("%s/%s: checklist: %s has no row in the checklist" % (sub, name, names))
    return errors


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.stderr.write(__doc__)
        sys.exit(2)
    errs = check(sys.argv[1], sys.argv[2])
    for e in errs:
        sys.stderr.write("checklist: %s\n" % e)
    sys.exit(1 if errs else 0)
