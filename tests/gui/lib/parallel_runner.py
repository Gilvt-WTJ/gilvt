#!/usr/bin/env python3
"""Case-level scheduler for run.sh --jobs.

Each child run owns a worker TMPDIR, so sandbox.sh's current-session link, app socket, HOME and cached
Swift tools never overlap. The parent is the only process that updates the aggregate evidence bundle.
"""

import argparse
import concurrent.futures
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

import evidence


def now_ms():
    return int(time.time() * 1000)


def read_schedule(path):
    rows = []
    with open(path, encoding="utf-8") as f:
        for raw in f:
            fields = raw.rstrip("\n").split("\t")
            if len(fields) != 3:
                raise ValueError("bad schedule row: %r" % raw.rstrip("\n"))
            rows.append({"id": fields[0], "mode": fields[1], "reason": fields[2]})
    return rows


def parse_args(argv):
    parser = argparse.ArgumentParser()
    parser.add_argument("--run", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--jobs", required=True, type=int)
    parser.add_argument("--app", default="")
    parser.add_argument("--foreground", action="store_true")
    parser.add_argument("--real", action="store_true")
    parser.add_argument("schedule")
    return parser.parse_args(argv)


def main(argv):
    args = parse_args(argv)
    if args.jobs < 2:
        raise SystemExit("parallel_runner: --jobs must be at least 2")
    out = Path(args.out)
    rows = read_schedule(args.schedule)
    evidence.command_parallel([str(out), str(args.jobs)])

    worker_root = out / ".workers"
    # Darwin's sockaddr_un path is limited to 104 bytes. Keeping runtime files under an arbitrary
    # --out path can make the per-process debug socket too long before the app even opens a window.
    tmp_root = Path(tempfile.mkdtemp(prefix="gilvt-gui-workers-", dir="/tmp"))
    worker_root.mkdir()
    active = {}
    active_lock = threading.Lock()
    interrupted = threading.Event()

    def stop(_signum, _frame):
        interrupted.set()
        with active_lock:
            processes = list(active.values())
        for process in processes:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except OSError:
                pass

    signal.signal(signal.SIGINT, stop)
    signal.signal(signal.SIGTERM, stop)

    def run_one(row, worker, execution_mode):
        case_id = row["id"]
        child_out = worker_root / case_id
        worker_tmp = tmp_root / worker
        worker_tmp.mkdir(parents=True, exist_ok=True)
        cmd = [args.run, "--jobs", "1", "--out", str(child_out)]
        if args.app:
            cmd += ["--app", args.app]
        if args.foreground:
            cmd.append("--foreground")
        if args.real:
            cmd.append("--real")
        cmd.append(case_id)
        env = os.environ.copy()
        env.update({
            "GILVT_GUI_PARALLEL_WORKER": worker,
            "GILVT_GUI_NO_CLIPBOARD_GUARD": "1",
            "TMPDIR": str(worker_tmp) + "/",
        })
        if interrupted.is_set():
            return row, worker, execution_mode, None, ""
        try:
            process = subprocess.Popen(
                cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=env,
                start_new_session=True)
        except OSError as error:
            return row, worker, execution_mode, 127, "could not start worker: %s\n" % error
        with active_lock:
            active[process.pid] = process
        try:
            output, _ = process.communicate()
        finally:
            with active_lock:
                active.pop(process.pid, None)
        return row, worker, execution_mode, process.returncode, output

    completed = {}

    def remember(result):
        row, worker, execution_mode, returncode, output = result
        completed[row["id"]] = (worker, execution_mode, returncode, output)

    gates = [row for row in rows if row["mode"] == "gate"]
    parallel = [row for row in rows if row["mode"] == "parallel"]
    serial = [row for row in rows if row["mode"] == "serial"]
    skipped = [row for row in rows if row["mode"] == "skip"]
    for row in skipped:
        evidence.command_case_skip([str(out), row["id"], row["reason"], evidence.utc_now(), str(now_ms())])

    gate_failed = False
    for row in gates:
        result = run_one(row, "gate", "gate")
        remember(result)
        child = worker_root / row["id"] / evidence.RESULT
        if result[3] != 0 or not child.is_file():
            gate_failed = True
            break
        child_result = evidence.load_result(worker_root / row["id"])
        if child_result["cases"][0]["status"] != "passed":
            gate_failed = True
            break

    if not gate_failed and not interrupted.is_set() and parallel:
        buckets = [[] for _ in range(min(args.jobs, len(parallel)))]
        for index, row in enumerate(parallel):
            buckets[index % len(buckets)].append(row)

        def run_bucket(index, bucket):
            worker = "worker-%d" % (index + 1)
            results = []
            for row in bucket:
                if interrupted.is_set():
                    break
                results.append(run_one(row, worker, "parallel"))
            return results

        sys.stderr.write("run: parallel phase: %d case(s), %d worker(s); serial phase: %d case(s)\n" %
                         (len(parallel), len(buckets), len(serial)))
        sys.stderr.flush()
        with concurrent.futures.ThreadPoolExecutor(max_workers=len(buckets)) as pool:
            futures = [pool.submit(run_bucket, index, bucket) for index, bucket in enumerate(buckets)]
            for future in concurrent.futures.as_completed(futures):
                for result in future.result():
                    remember(result)

    if not gate_failed and not interrupted.is_set():
        for row in serial:
            if interrupted.is_set():
                break
            remember(run_one(row, "serial", "serial"))

    stop_reason = None
    if interrupted.is_set():
        stop_reason = "parallel runner interrupted"
    elif gate_failed:
        stop_reason = "S0 gate failed"

    for row in rows:
        case_id = row["id"]
        if row["mode"] == "skip":
            continue
        record = completed.get(case_id)
        if record is None:
            evidence.command_case_not_run([str(out), case_id, stop_reason or "worker did not run", "harness"])
            continue
        worker, execution_mode, returncode, output = record
        child_out = worker_root / case_id
        if (child_out / evidence.RESULT).is_file():
            evidence.command_merge_case([str(out), str(child_out), worker, execution_mode])
            continue

        case_dir = out / "cases" / case_id
        case_dir.mkdir(parents=True, exist_ok=True)
        log = case_dir / "case.log"
        log.write_text(output or "worker produced no output\n", encoding="utf-8")
        started = evidence.utc_now()
        stamp = str(now_ms())
        evidence.command_case_start([str(out), case_id, started, stamp,
                                     "cases/%s/case.log" % case_id, "cases/%s" % case_id])
        evidence.command_case_end([str(out), case_id, "failed", "harness",
                                   "worker exited %s without result.json" % returncode,
                                   evidence.utc_now(), str(now_ms()), "0", "",
                                   "cases/%s/case.log" % case_id])

    shutil.rmtree(worker_root, ignore_errors=True)
    shutil.rmtree(tmp_root, ignore_errors=True)
    evidence.command_finalize([str(out), evidence.utc_now(), str(now_ms()),
                               "true" if interrupted.is_set() else "false"])
    result = evidence.load_result(out)
    for case in result["cases"]:
        status = case["status"]
        if status == "passed":
            suffix = " (%d skipped)" % case["skipped_steps"] if case["skipped_steps"] else ""
            print("%s PASS%s" % (case["id"], suffix))
        elif status == "failed":
            reason = case.get("reason") or "unknown"
            print("%s %s" % (case["id"], reason if reason.startswith("FAIL(") else "FAIL(%s)" % reason))
        elif status == "skipped":
            print("%s SKIP(%s)" % (case["id"], case.get("reason") or "unknown"))
        else:
            print("%s NOT_RUN(%s)" % (case["id"], case.get("reason") or status))
    summary = result["summary"]
    not_run = ", %d not run" % summary["not_run"] if summary["not_run"] else ""
    print("summary: %d passed, %d failed, %d skipped%s (%d cases); logs in %s" % (
        summary["passed"], summary["failed"], summary["skipped"], not_run, summary["total"], out))
    if interrupted.is_set():
        return 130
    return 1 if summary["failed"] or summary["interrupted"] else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
