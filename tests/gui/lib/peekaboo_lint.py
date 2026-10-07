#!/usr/bin/env python3
"""Lint for tests/gui (run by selftest.sh): every Peekaboo command in the scripts passes --no-remote.

Peekaboo 4.5 hands a call to a running Peekaboo daemon / Bridge host when there is one. That host is its own
TCC-responsible process, often without Screen Recording or Accessibility even when the terminal has both, so
every call made through it fails. --no-remote keeps the call in the caller, under the terminal's grants.

  peekaboo_lint.py DIR    checks DIR/*.sh (but selftest.sh) and DIR/lib/*.py; prints each offending line,
                          exit 1 when there is one

In shell files a call is the word `peekaboo` at the start of a command (after a blank, ; & | ( ! or `), not
inside a "…" string unless it starts a $( … ) there, not after `command -v`; the rest of its line must hold
--no-remote. In Python files an argv list naming "peekaboo" must hold "--no-remote" on the same line.
Compatible with the system /usr/bin/python3 (3.9).
"""

import glob
import os
import re
import sys

CALL = re.compile(r"(?:^|(?<=[\s;&|(!`]))peekaboo(?=\s)")
PY_ARGV = re.compile(r"""["']peekaboo["']\s*,""")


def in_string(before):
    """Is the text after `before` inside a double-quoted shell string (unescaped quotes counted)?"""
    return (before.count('"') - before.count('\\"')) % 2 == 1


def shell_offences(path):
    out = []
    with open(path, encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            if line.lstrip().startswith("#"):
                continue
            for m in CALL.finditer(line):
                before = line[:m.start()]
                if in_string(before) and not before.endswith("$("):
                    continue
                if before.rstrip().endswith("command -v"):
                    continue
                if "--no-remote" not in line[m.end():]:
                    out.append("%s:%d: %s" % (path, n, line.strip()))
                    break
    return out


def python_offences(path):
    out = []
    with open(path, encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            if PY_ARGV.search(line) and "--no-remote" not in line:
                out.append("%s:%d: %s" % (path, n, line.strip()))
    return out


def main(argv):
    if len(argv) != 2:
        sys.stderr.write(__doc__)
        return 2
    here = argv[1]
    bad = []
    for path in sorted(glob.glob(os.path.join(here, "*.sh"))):
        if os.path.basename(path) != "selftest.sh":
            bad += shell_offences(path)
    for path in sorted(glob.glob(os.path.join(here, "lib", "*.py"))):
        bad += python_offences(path)
    for b in bad:
        print(b)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
