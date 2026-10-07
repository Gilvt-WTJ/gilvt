#!/usr/bin/env python3
"""Build a reviewable evidence bundle for tests/gui/run.sh.

The runner calls this command after each lifecycle event. result.json is updated atomically so a
partially completed run remains inspectable. `finalize` derives the human-readable report, JUnit,
and a content manifest from that canonical result.

Compatible with the macOS system Python 3.9.
"""

import hashlib
import html
import json
import os
import platform
import shutil
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from xml.etree import ElementTree


SCHEMA_VERSION = 1
RESULT = "result.json"


class Usage(Exception):
    pass


def utc_now():
    return datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def atomic_json(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=path.name + ".", dir=str(path.parent))
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump(value, f, ensure_ascii=False, indent=2, sort_keys=True)
            f.write("\n")
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def load_result(out):
    path = Path(out) / RESULT
    try:
        with path.open(encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError) as e:
        raise Usage("cannot read %s: %s" % (path, e))


def save_result(out, result):
    atomic_json(Path(out) / RESULT, result)


def parse_bool(value):
    if value == "true":
        return True
    if value == "false":
        return False
    raise Usage("expected true or false, got %r" % value)


def integer(value, name):
    try:
        return int(value)
    except ValueError:
        raise Usage("%s must be an integer, got %r" % (name, value))


def optional_path(value):
    return value or None


def case_by_id(result, case_id):
    hits = [case for case in result["cases"] if case["id"] == case_id]
    if len(hits) != 1:
        raise Usage("result has %d cases named %s" % (len(hits), case_id))
    return hits[0]


def command_init(args):
    if len(args) != 10:
        raise Usage("init OUT RUN_ID COMMIT DIRTY STARTED_AT START_MS APP FOREGROUND REAL CASES_TSV")
    out, run_id, commit, dirty, started_at, started_ms, app, foreground, real, cases_tsv = args
    cases = []
    try:
        with open(cases_tsv, encoding="utf-8") as f:
            for raw in f:
                raw = raw.rstrip("\n")
                if not raw:
                    continue
                fields = raw.split("\t")
                if len(fields) != 5:
                    raise Usage("bad cases TSV row: %r" % raw)
                case_id, source, requires, foreground_steps, steps = fields
                cases.append({
                    "id": case_id,
                    "source": source,
                    "requires": requires,
                    "foreground_steps": integer(foreground_steps, "foreground_steps"),
                    "declared_steps": integer(steps, "steps"),
                    "status": "pending",
                    "classification": None,
                    "reason": None,
                    "started_at": None,
                    "ended_at": None,
                    "duration_ms": None,
                    "skipped_steps": 0,
                    "log": None,
                    "failure_state": None,
                    "artifact_dir": None,
                    "steps": [],
                })
    except OSError as e:
        raise Usage("cannot read %s: %s" % (cases_tsv, e))
    result = {
        "schema_version": SCHEMA_VERSION,
        "run": {
            "id": run_id,
            "commit": commit,
            "dirty": parse_bool(dirty),
            "started_at": started_at,
            "started_ms": integer(started_ms, "started_ms"),
            "ended_at": None,
            "duration_ms": None,
            "app": app or None,
            "foreground_authorized": parse_bool(foreground),
            "real_enabled": parse_bool(real),
            "interrupted": False,
            "complete": False,
        },
        "summary": {
            "passed": 0,
            "failed": 0,
            "skipped": 0,
            "interrupted": 0,
            "not_run": len(cases),
            "total": len(cases),
        },
        "cases": cases,
    }
    save_result(out, result)
    atomic_json(Path(out) / "environment.json", {
        "schema_version": 1,
        "platform": platform.platform(),
        "python": platform.python_version(),
        "app": app or None,
    })


def command_case_start(args):
    if len(args) != 6:
        raise Usage("case-start OUT ID STARTED_AT START_MS LOG ARTIFACT_DIR")
    out, case_id, started_at, started_ms, log, artifact_dir = args
    result = load_result(out)
    case = case_by_id(result, case_id)
    case.update({
        "status": "running",
        "started_at": started_at,
        "started_ms": integer(started_ms, "started_ms"),
        "log": optional_path(log),
        "artifact_dir": optional_path(artifact_dir),
    })
    save_result(out, result)


def command_case_skip(args):
    if len(args) != 5:
        raise Usage("case-skip OUT ID REASON ENDED_AT ENDED_MS")
    out, case_id, reason, ended_at, ended_ms = args
    result = load_result(out)
    case = case_by_id(result, case_id)
    case.update({
        "status": "skipped",
        "classification": "manual" if reason == "manual" else reason,
        "reason": reason,
        "ended_at": ended_at,
        "ended_ms": integer(ended_ms, "ended_ms"),
        "duration_ms": 0,
    })
    save_result(out, result)


def command_case_not_run(args):
    if len(args) != 4:
        raise Usage("case-not-run OUT ID REASON CLASSIFICATION")
    out, case_id, reason, classification = args
    result = load_result(out)
    case = case_by_id(result, case_id)
    case.update({
        "status": "not_run",
        "classification": classification or None,
        "reason": reason,
    })
    save_result(out, result)


def command_parallel(args):
    if len(args) != 2:
        raise Usage("parallel OUT JOBS")
    out, jobs = args
    result = load_result(out)
    result["run"]["jobs"] = integer(jobs, "jobs")
    result["run"]["execution"] = "parallel"
    save_result(out, result)


def command_merge_case(args):
    if len(args) != 4:
        raise Usage("merge-case OUT CHILD_OUT WORKER EXECUTION_MODE")
    out, child_out, worker, execution_mode = args
    parent = load_result(out)
    child = load_result(child_out)
    if len(child["cases"]) != 1:
        raise Usage("child result must contain exactly one case")
    child_case = child["cases"][0]
    parent_case = case_by_id(parent, child_case["id"])
    case_id = child_case["id"]

    source_dir = Path(child_out) / "cases" / case_id
    target_dir = Path(out) / "cases" / case_id
    if source_dir.exists():
        if target_dir.exists():
            shutil.rmtree(target_dir)
        target_dir.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(source_dir, target_dir, symlinks=True)

    selection = {key: parent_case[key] for key in (
        "id", "source", "requires", "foreground_steps", "declared_steps")
    }
    parent_case.clear()
    parent_case.update(child_case)
    parent_case.update(selection)
    parent_case["worker"] = worker
    parent_case["execution_mode"] = execution_mode
    save_result(out, parent)

    links = {
        case_id: "cases/%s" % case_id,
        case_id + ".log": "cases/%s/case.log" % case_id,
    }
    if parent_case.get("failure_state"):
        links[case_id + ".state.json"] = parent_case["failure_state"]
    for name, target in links.items():
        path = Path(out) / name
        if not path.exists() and not path.is_symlink():
            path.symlink_to(target)


def command_step(args):
    if len(args) != 18:
        raise Usage(
            "step OUT ID N ACTION FOREGROUND LINE STARTED_AT START_MS ENDED_AT ENDED_MS RC STATUS "
            "OUTPUT BEFORE_STATE AFTER_STATE BEFORE_SHOT AFTER_SHOT NOTE"
        )
    (out, case_id, number, action, foreground, line, started_at, started_ms, ended_at, ended_ms, rc,
     status, output, before_state, after_state, before_shot, after_shot, note) = args
    result = load_result(out)
    case = case_by_id(result, case_id)
    number = integer(number, "step number")
    if any(step["number"] == number for step in case["steps"]):
        raise Usage("case %s already has step %d" % (case_id, number))
    start = integer(started_ms, "started_ms")
    end = integer(ended_ms, "ended_ms")
    case["steps"].append({
        "number": number,
        "declared_action": line,
        "action": action,
        "foreground": parse_bool(foreground),
        "status": status,
        "exit_code": integer(rc, "exit code"),
        "started_at": started_at,
        "ended_at": ended_at,
        "duration_ms": max(0, end - start),
        "evidence": {
            "output": optional_path(output),
            "before_state": optional_path(before_state),
            "after_state": optional_path(after_state),
            "before_screenshot": optional_path(before_shot),
            "after_screenshot": optional_path(after_shot),
            "note": note or None,
        },
    })
    save_result(out, result)


def command_case_end(args):
    if len(args) != 10:
        raise Usage(
            "case-end OUT ID STATUS CLASSIFICATION REASON ENDED_AT ENDED_MS SKIPPED_STEPS FAILURE_STATE LOG"
        )
    (out, case_id, status, classification, reason, ended_at, ended_ms, skipped_steps,
     failure_state, log) = args
    result = load_result(out)
    case = case_by_id(result, case_id)
    end = integer(ended_ms, "ended_ms")
    start = case.get("started_ms") or end
    case.update({
        "status": status,
        "classification": classification or None,
        "reason": reason or None,
        "ended_at": ended_at,
        "ended_ms": end,
        "duration_ms": max(0, end - start),
        "skipped_steps": integer(skipped_steps, "skipped_steps"),
        "failure_state": optional_path(failure_state),
        "log": optional_path(log),
    })
    save_result(out, result)


def summarize(result):
    counts = {"passed": 0, "failed": 0, "skipped": 0, "interrupted": 0, "not_run": 0}
    for case in result["cases"]:
        status = case["status"]
        if status == "running":
            status = "interrupted"
            case["status"] = status
            case["classification"] = "harness"
            case["reason"] = case.get("reason") or "runner interrupted"
        if status == "pending":
            status = "not_run"
            case["status"] = status
        if status not in counts:
            raise Usage("unknown case status %r" % status)
        counts[status] += 1
    counts["total"] = len(result["cases"])
    result["summary"] = counts


def markdown_report(result):
    run = result["run"]
    summary = result["summary"]
    lines = [
        "# gilvt GUI acceptance %s" % run["id"],
        "",
        "- Commit: `%s`%s" % (run["commit"], " (dirty)" if run["dirty"] else ""),
        "- Started: `%s`" % run["started_at"],
        "- Duration: `%s ms`" % run["duration_ms"],
        "- App: `%s`" % (run["app"] or "default"),
        "- Foreground authorized: `%s`" % str(run["foreground_authorized"]).lower(),
        "- Real agents enabled: `%s`" % str(run["real_enabled"]).lower(),
        "- Jobs: `%s`" % run.get("jobs", 1),
        "",
        "## Summary",
        "",
        "Passed %d | Failed %d | Skipped %d | Interrupted %d | Not run %d" % (
            summary["passed"], summary["failed"], summary["skipped"], summary["interrupted"],
            summary["not_run"]),
        "",
        "| Case | Requirement | Result | Classification | Duration | Evidence |",
        "|---|---|---|---|---:|---|",
    ]
    for case in result["cases"]:
        evidence = case.get("artifact_dir") or ""
        if evidence:
            evidence = "[%s](%s)" % (evidence, evidence)
        reason = case.get("reason") or ""
        result_text = case["status"] + ((": " + reason) if reason else "")
        duration = "" if case.get("duration_ms") is None else "%d ms" % case["duration_ms"]
        lines.append("| `%s` | `%s` | %s | %s | %s | %s |" % (
            case["id"], case["requires"], result_text.replace("|", "\\|"),
            case.get("classification") or "", duration, evidence))
    failed = [case for case in result["cases"] if case["status"] in ("failed", "interrupted")]
    if failed:
        lines += ["", "## Failures", ""]
        for case in failed:
            lines += ["### %s" % case["id"], "", "- Reason: `%s`" % (case.get("reason") or case["status"])]
            if case.get("failure_state"):
                lines.append("- State: [%s](%s)" % (case["failure_state"], case["failure_state"]))
            if case.get("log"):
                lines.append("- Log: [%s](%s)" % (case["log"], case["log"]))
            lines.append("")
    lines += ["## Integrity", "", "See `manifest.json` and `manifest.sha256` for the content-addressed evidence index.", ""]
    return "\n".join(lines)


def html_report(result):
    run = result["run"]
    rows = []
    details = []
    for case in result["cases"]:
        rows.append(
            "<tr><td><a href=\"#case-%s\">%s</a></td><td>%s</td><td>%s</td><td>%s</td><td>%s</td></tr>" % (
                html.escape(case["id"]), html.escape(case["id"]), html.escape(case["requires"]),
                html.escape(case["status"]), html.escape(case.get("classification") or ""),
                html.escape(case.get("reason") or "")))
        step_rows = []
        for step in case["steps"]:
            links = []
            for label, key in (("output", "output"), ("before state", "before_state"), ("after state", "after_state"),
                               ("before screenshot", "before_screenshot"), ("after screenshot", "after_screenshot")):
                value = step["evidence"].get(key)
                if value:
                    links.append('<a href="%s">%s</a>' % (html.escape(value), label))
            step_rows.append(
                "<tr><td>%d</td><td><code>%s</code></td><td>%s</td><td>%d</td><td>%s</td></tr>" % (
                    step["number"], html.escape(step["declared_action"]), html.escape(step["status"]),
                    step["duration_ms"], " | ".join(links)))
        details.append(
            '<section id="case-%s"><h2>%s: %s</h2><p>%s</p><table><thead><tr><th>#</th><th>Action</th>'
            '<th>Result</th><th>ms</th><th>Evidence</th></tr></thead><tbody>%s</tbody></table></section>' % (
                html.escape(case["id"]), html.escape(case["id"]), html.escape(case["status"]),
                html.escape(case.get("reason") or ""), "".join(step_rows)))
    return """<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width">
<title>gilvt acceptance {run_id}</title>
<style>
body{{font:15px/1.5 ui-sans-serif,system-ui,sans-serif;max-width:1200px;margin:40px auto;padding:0 24px;color:#17201b;background:#f5f2e9}}
h1,h2{{font-family:ui-serif,Georgia,serif}}table{{border-collapse:collapse;width:100%;background:#fff}}
th,td{{border:1px solid #c8c2b4;padding:8px;text-align:left;vertical-align:top}}th{{background:#e8e2d4}}
code{{white-space:pre-wrap}}section{{margin-top:36px}}a{{color:#075f5b}}
</style></head><body>
<h1>gilvt GUI acceptance</h1><p><strong>Run:</strong> {run_id}<br><strong>Commit:</strong> {commit}<br>
<strong>Started:</strong> {started}<br><strong>Duration:</strong> {duration} ms</p>
<table><thead><tr><th>Case</th><th>Requirement</th><th>Result</th><th>Classification</th><th>Reason</th></tr></thead>
<tbody>{rows}</tbody></table>{details}</body></html>
""".format(run_id=html.escape(run["id"]), commit=html.escape(run["commit"]),
           started=html.escape(run["started_at"]), duration=run["duration_ms"], rows="".join(rows),
           details="".join(details))


def junit_report(result):
    summary = result["summary"]
    suite = ElementTree.Element("testsuite", {
        "name": "gilvt-gui-acceptance",
        "tests": str(summary["total"]),
        "failures": str(summary["failed"] + summary["interrupted"]),
        "skipped": str(summary["skipped"] + summary["not_run"]),
        "time": "%.3f" % ((result["run"]["duration_ms"] or 0) / 1000.0),
    })
    for case in result["cases"]:
        node = ElementTree.SubElement(suite, "testcase", {
            "classname": "gilvt.gui.%s" % case["requires"],
            "name": case["id"],
            "time": "%.3f" % ((case.get("duration_ms") or 0) / 1000.0),
        })
        if case["status"] in ("failed", "interrupted"):
            failure = ElementTree.SubElement(node, "failure", {
                "type": case.get("classification") or "unknown",
                "message": case.get("reason") or case["status"],
            })
            failure.text = case.get("reason") or case["status"]
        elif case["status"] in ("skipped", "not_run"):
            ElementTree.SubElement(node, "skipped", {"message": case.get("reason") or case["status"]})
    xml = ElementTree.tostring(suite, encoding="unicode")
    return '<?xml version="1.0" encoding="UTF-8"?>\n' + xml + "\n"


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        while True:
            block = f.read(1024 * 1024)
            if not block:
                break
            digest.update(block)
    return digest.hexdigest()


def write_text(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as f:
        f.write(value)


def write_manifest(out):
    root = Path(out)
    excluded = {"manifest.json", "manifest.sha256"}
    files = []
    for path in sorted(p for p in root.rglob("*") if p.is_file() and not p.is_symlink()):
        rel = str(path.relative_to(root))
        if rel in excluded:
            continue
        files.append({"path": rel, "bytes": path.stat().st_size, "sha256": sha256(path)})
    manifest_path = root / "manifest.json"
    atomic_json(manifest_path, {"schema_version": 1, "generated_at": utc_now(), "files": files})
    write_text(root / "manifest.sha256", "%s  manifest.json\n" % sha256(manifest_path))


def command_finalize(args):
    if len(args) != 4:
        raise Usage("finalize OUT ENDED_AT ENDED_MS INTERRUPTED")
    out, ended_at, ended_ms, interrupted = args
    result = load_result(out)
    end = integer(ended_ms, "ended_ms")
    result["run"].update({
        "ended_at": ended_at,
        "duration_ms": max(0, end - result["run"]["started_ms"]),
        "interrupted": parse_bool(interrupted),
        "complete": not parse_bool(interrupted),
    })
    summarize(result)
    save_result(out, result)
    write_text(Path(out) / "summary.md", markdown_report(result))
    write_text(Path(out) / "report.html", html_report(result))
    write_text(Path(out) / "junit.xml", junit_report(result))
    write_manifest(out)


COMMANDS = {
    "init": command_init,
    "case-start": command_case_start,
    "case-skip": command_case_skip,
    "case-not-run": command_case_not_run,
    "step": command_step,
    "case-end": command_case_end,
    "parallel": command_parallel,
    "merge-case": command_merge_case,
    "finalize": command_finalize,
}


def main(argv):
    if len(argv) < 2 or argv[1] not in COMMANDS:
        raise Usage("usage: evidence.py <%s> ..." % "|".join(sorted(COMMANDS)))
    COMMANDS[argv[1]](argv[2:])


if __name__ == "__main__":
    try:
        main(sys.argv)
    except Usage as e:
        sys.stderr.write("evidence: %s\n" % e)
        sys.exit(2)
    except (OSError, ValueError, KeyError) as e:
        sys.stderr.write("evidence: %s\n" % e)
        sys.exit(1)
