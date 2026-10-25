#!/usr/bin/env python3
"""constellation.py — one harness across every surface of the constellation.

    python3 e2e/constellation.py              # the table
    python3 e2e/constellation.py --json       # machine-readable
    python3 e2e/constellation.py --quick      # skip the slow subprocess checks
    python3 e2e/constellation.py --local-only # do not touch the public internet

Exit code is 0 only when no *required* check fails. Advisory checks are reported
and never gate: a dead edge worker three time zones away is information, not a
broken build.

Why this lives in gemini-diary: the diary is the constellation's only surface that
already proves its own invariants (`float_count: 0`, `waveint_frame_bytes: 79`,
`residual_drift_pq: 0/1`). Putting the harness next to the surface with the
strictest contract means the checks are written in the presence of a standard.

Standard library only. No floating point in the output: counts are integers,
latencies are integer milliseconds, and ratios are printed as p/q.
"""

from __future__ import annotations

import argparse
import concurrent.futures as cf
import json
import os
import pathlib
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request

HOME = pathlib.Path.home()
WS = HOME / "workspace" / "peterlodri-sec"
DIARY = WS / "gemini-diary"
VAKED = WS / "vaked"
FIRST_SPARK = WS / "first-spark"
SPHEREPOP = WS / "spherepop-toss"
WA = WS / "wa-stream"
POCCO = WS / "pocoo.vaked.dev"
SIGNS = WS / "music-quant-signs" / "signs"

OK, WARN, FAIL, SKIP = "ok", "warn", "fail", "skip"

RESULTS: list[dict] = []


def record(section: str, name: str, status: str, detail: str = "", required: bool = True) -> None:
    RESULTS.append({"section": section, "name": name, "status": status,
                    "detail": detail, "required": required})


def http(url: str, timeout: int = 12) -> tuple[int | str, bytes, int]:
    t0 = time.monotonic()
    try:
        req = urllib.request.Request(url, headers={"User-Agent": "constellation-e2e/1"})
        with urllib.request.urlopen(req, timeout=timeout) as r:
            body = r.read()
        return r.status, body, int((time.monotonic() - t0) * 1000)
    except urllib.error.HTTPError as e:
        return e.code, b"", int((time.monotonic() - t0) * 1000)
    except Exception as e:  # noqa: BLE001 - the whole point is to classify failures
        return type(e).__name__, b"", int((time.monotonic() - t0) * 1000)


def run(cmd: list[str], cwd: pathlib.Path | None = None, timeout: int = 300) -> tuple[int, str]:
    try:
        p = subprocess.run(cmd, cwd=str(cwd) if cwd else None, capture_output=True,
                           text=True, timeout=timeout)
        return p.returncode, (p.stdout + p.stderr).strip()
    except FileNotFoundError:
        return 127, f"{cmd[0]}: not found"
    except subprocess.TimeoutExpired:
        return 124, f"{' '.join(cmd)}: timed out after {timeout}s"


# --------------------------------------------------------------------------
# sections
# --------------------------------------------------------------------------

def check_diary(local_only: bool) -> None:
    """The host surface: its own invariants, locally and on all three portals."""
    binp = DIARY / "target" / "release" / "gemini-diary"
    if binp.exists():
        record("diary", "release binary present", OK, f"{binp.stat().st_size} bytes")
    else:
        record("diary", "release binary present", WARN, "not built; run cargo build --release")

    bind = os.environ.get("DIARY_BIND", "127.0.0.1:8811")
    st, body, ms = http(f"http://{bind}/health", timeout=4)
    if st == 200:
        try:
            j = json.loads(body)
            record("diary", "local /health", OK, f"{ms}ms")
            record("diary", "float_count == 0", OK if j.get("float_count") == 0 else FAIL,
                   str(j.get("float_count")))
            record("diary", "waveint_frame_bytes == 79",
                   OK if j.get("waveint_frame_bytes") == 79 else FAIL,
                   str(j.get("waveint_frame_bytes")))
        except Exception:
            record("diary", "local /health", FAIL, "non-JSON body")
        st, body, ms = http(f"http://{bind}/api/toss", timeout=6)
        if st == 200:
            try:
                j = json.loads(body)
                steps = len(j.get("steps") or [])
                record("diary", "TOSS spiral closes to [0,0]",
                       OK if steps == 9 and j.get("residual_drift_pq") == "0/1" else FAIL,
                       f"{steps} steps, drift {j.get('residual_drift_pq')}")
            except Exception:
                record("diary", "TOSS spiral closes to [0,0]", FAIL, "unparseable")
        else:
            record("diary", "/api/toss", WARN, str(st), required=False)
        st, body, _ = http(f"http://{bind}/api/wave/wav", timeout=8)
        record("diary", "wav is a real RIFF",
               OK if st == 200 and body[:4] == b"RIFF" else FAIL,
               f"{len(body)} bytes")
    else:
        record("diary", "local /health", SKIP, f"{st} (start it: BIND_ADDR={bind} ./target/release/gemini-diary)")

    if local_only:
        record("diary", "public portals", SKIP, "--local-only", required=False)
        return
    for host in ("https://gemini-diary.8b.is", "https://g.8b.is", "https://gemini.g8t.is"):
        st, body, ms = http(host + "/health", timeout=15)
        ok = st == 200 and b'"float_count":0' in body.replace(b" ", b"")
        record("diary", f"portal {host.split('//')[1]}", OK if ok else FAIL, f"{st} {ms}ms")


def check_ports() -> None:
    """Who owns the ports the constellation shares. Three services want 8788."""
    wanted = {8787: "wa webhook", 8788: "wa raw duplex (canonical) · diary default · memnet",
              8789: "mux http", 8790: "diary webhook shift", 8797: "wa raw duplex (actual)",
              8811: "diary (local run)", 8080: "ollama", 4222: "nats"}

    def owner(port: int) -> str:
        rc, out = run(["lsof", "-nP", f"-iTCP:{port}", "-sTCP:LISTEN"], timeout=10)
        lines = [l for l in out.splitlines()[1:] if l.strip()]
        return lines[0].split()[0] if lines else ""

    for port, what in sorted(wanted.items()):
        who = owner(port)
        if port == 8797:
            record("ports", f"{port} {what}", OK if who == "node" else FAIL, who or "nobody")
        elif port == 8788:
            # memnet squatting here is the documented failure that silenced the
            # stream for three days; it is a warning, not a hard fail, because the
            # stream now lives on 8797.
            record("ports", f"{port} {what}", WARN if who and who != "node" else OK,
                   who or "free", required=False)
        else:
            record("ports", f"{port} {what}", OK if who else WARN, who or "free", required=False)


def check_stream() -> None:
    rc, out = run(["pgrep", "-fl", "node sidecar.ts"])
    record("stream", "sidecar running", OK if rc == 0 else FAIL,
           out.splitlines()[0] if out else "not running")
    rc, out = run(["pgrep", "-fl", "music-read.ts --watch"])
    n = len([l for l in out.splitlines() if l.strip()])
    record("stream", "one watcher (not two)", OK if n in (1, 2) else WARN,
           f"{n} process line(s)", required=False)
    log = WA / "music-read.log"
    if log.exists():
        tail = "\n".join(log.read_text(errors="replace").splitlines()[-40:])
        record("stream", "watcher attached to the duplex",
               OK if "attached to" in tail else WARN,
               tail.strip().splitlines()[-1][:80] if tail.strip() else "")
    rc, out = run(["launchctl", "list"], timeout=15)
    loaded = [l for l in out.splitlines() if "wastream" in l]
    record("stream", "launchd owns the pair", OK if len(loaded) == 2 else WARN,
           f"{len(loaded)} agent(s)", required=False)


def check_music_lane() -> None:
    if SIGNS.exists():
        n = len(list(SIGNS.glob("*.json")))
        record("music", "quant signs", OK, f"{n} files")
    else:
        record("music", "quant signs", FAIL, "signs directory missing")
    if POCCO.exists():
        posts = sorted((POCCO / "posts").glob("*.md"))
        record("music", "music-lane posts", OK, f"{len(posts)} posts")
    rc, out = run(["hf", "auth", "whoami"], timeout=30)
    record("music", "hf identity", OK if rc == 0 else WARN, out.splitlines()[0] if out else "", required=False)


def check_edge(local_only: bool) -> None:
    if local_only:
        record("edge", "worker fleet", SKIP, "--local-only", required=False)
        return
    probe_file = VAKED / "examples" / "wip-catalog" / "fleet-probe.json"
    hosts = [r["host"] for r in json.loads(probe_file.read_text())] if probe_file.exists() else []
    if not hosts:
        record("edge", "worker fleet", SKIP, "no probe roster")
        return

    def probe(h: str) -> int | str:
        st, _, _ = http("https://" + h + "/", timeout=15)
        return st

    with cf.ThreadPoolExecutor(max_workers=18) as ex:
        sts = list(ex.map(probe, hosts))
    alive = sum(1 for s in sts if s == 200)
    dead = [h for h, s in zip(hosts, sts) if s != 200]
    # 17/18 was the measured baseline; anything at or above two thirds is healthy.
    record("edge", f"workers answering ({alive}/{len(hosts)})",
           OK if alive * 3 >= len(hosts) * 2 else WARN, ", ".join(dead[:4]), required=False)


def check_vaked(quick: bool) -> None:
    fixtures = [
        (VAKED / "examples" / "teamlab" / "teamlab-toss-recipe.vaked", 0),
        (VAKED / "examples" / "teamlab" / "teamlab-borderless-field.vaked", 0),
        (VAKED / "examples" / "spherepop" / "spherepop-lifecycle.vaked", 0),
        (VAKED / "examples" / "spherepop" / "spherepop-shortcut.vaked", 0),
        (VAKED / "examples" / "spherepop" / "spherepop-refusal-leak.vaked", 1),
        (VAKED / "examples" / "wip-catalog" / "wip-catalog.vaked", 0),
        (VAKED / "examples" / "first-spark" / "first-spark.vaked", 0),
        (VAKED / "examples" / "first-spark" / "first-spark-debt-leak.vaked", 1),
    ]
    if quick:
        record("vaked", "fixtures", SKIP, "--quick", required=False)
        return
    for path, expect in fixtures:
        if not path.exists():
            record("vaked", path.name, WARN, "missing", required=False)
            continue
        rc, out = run([sys.executable, "-m", "vakedc", "check", "--json", str(path)],
                      cwd=VAKED, timeout=120)
        # stderr is appended by run(), so decode the first JSON value only:
        # raw_decode stops at the closing brace and ignores whatever follows.
        try:
            j, _ = json.JSONDecoder().raw_decode(out[out.index("{"):])
            n = len(j.get("diagnostics") or [])
        except Exception:
            n = -1
        record("vaked", f"{path.name} → {expect} diagnostic(s)",
               OK if n == expect else FAIL, f"got {n}")


def check_suites(quick: bool) -> None:
    if quick:
        record("suites", "python + cargo", SKIP, "--quick", required=False)
        return
    for name, cwd, cmd in (
        ("first-spark tests", FIRST_SPARK, [sys.executable, "-m", "unittest", "discover", "-s", "tests"]),
        ("spherepop-toss tests", SPHEREPOP, [sys.executable, "-m", "unittest", "discover", "-s", "tests"]),
    ):
        if not cwd.exists():
            record("suites", name, SKIP, "project not present", required=False)
            continue
        rc, out = run(cmd, cwd=cwd, timeout=180)
        last = out.strip().splitlines()[-1] if out.strip() else ""
        record("suites", name, OK if rc == 0 else FAIL, last[:70])


def check_tailnet() -> None:
    rc, out = run(["tailscale", "status"], timeout=20)
    if rc != 0:
        record("tailnet", "reachable peers", SKIP, "tailscale not responding", required=False)
        return
    peers = [l for l in out.splitlines()[1:] if l.startswith("100.")]
    online = [l for l in peers if "offline" not in l]
    record("tailnet", "reachable peers", OK if online else WARN,
           f"{len(online)} of {len(peers)}", required=False)


def check_repos() -> None:
    """Every repo this constellation claims should be at 0/0 against its upstream."""
    repos = ["vaked", "pocoo.vaked.dev", "wa-stream", "first-spark", "spherepop-toss",
             "kompress-ultra", "gemini-diary"]
    for r in repos:
        p = WS / r
        if not (p / ".git").exists():
            record("repos", r, SKIP, "not present", required=False)
            continue
        run(["git", "fetch", "-q", "origin"], cwd=p, timeout=60)
        rc, out = run(["git", "rev-list", "--left-right", "--count", "origin/main...HEAD"], cwd=p, timeout=30)
        if rc != 0:
            record("repos", r, WARN, "no origin/main", required=False)
            continue
        behind, ahead = (out.split() + ["0", "0"])[:2]
        ok = behind == "0" and ahead == "0"
        record("repos", r, OK if ok else WARN, f"{ahead} ahead, {behind} behind", required=False)


# --------------------------------------------------------------------------
# report
# --------------------------------------------------------------------------

GLYPH = {OK: " ok ", WARN: "warn", FAIL: "FAIL", SKIP: "skip"}


def report(as_json: bool) -> int:
    required_failures = [r for r in RESULTS if r["required"] and r["status"] == FAIL]
    if as_json:
        print(json.dumps({
            "checks": RESULTS,
            "counts": {s: sum(1 for r in RESULTS if r["status"] == s) for s in (OK, WARN, FAIL, SKIP)},
            "required_failures": len(required_failures),
            "exit": 1 if required_failures else 0,
        }, indent=2))
        return 1 if required_failures else 0

    print()
    print("constellation e2e")
    print("=" * 78)
    section = None
    for r in RESULTS:
        if r["section"] != section:
            section = r["section"]
            print(f"\n  {section}")
        flag = "" if r["required"] else " (advisory)"
        detail = f"  ·  {r['detail']}" if r["detail"] else ""
        print(f"    [{GLYPH[r['status']]}] {r['name']}{flag}{detail}")
    counts = {s: sum(1 for r in RESULTS if r["status"] == s) for s in (OK, WARN, FAIL, SKIP)}
    print()
    print("-" * 78)
    print(f"  {counts[OK]} ok · {counts[WARN]} warn · {counts[FAIL]} fail · {counts[SKIP]} skipped")
    print(f"  required failures: {len(required_failures)}")
    print("=" * 78)
    return 1 if required_failures else 0


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="one harness across the constellation")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--quick", action="store_true", help="skip subprocess-heavy checks")
    ap.add_argument("--local-only", action="store_true", help="do not touch the internet")
    args = ap.parse_args(argv)

    check_diary(args.local_only)
    check_ports()
    check_stream()
    check_music_lane()
    check_edge(args.local_only)
    check_vaked(args.quick)
    check_suites(args.quick)
    check_tailnet()
    check_repos()
    return report(args.json)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
