#!/usr/bin/env python3
"""Bounded, restart-safe maintenance reporting. Python standard library only.

All times are integer UTC seconds. CI evidence is fetched independently; legacy
benchmark claims are never promoted to measured improvements by this publisher.
"""

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
import time
import urllib.request

MAX_BYTES = 1_048_576
HISTORY_LIMIT = 256


def reject_number(value):
    raise ValueError("non-integer JSON number: " + value)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON key: " + key)
        result[key] = value
    return result


def decode(raw):
    if len(raw) > MAX_BYTES:
        raise ValueError("JSON size limit exceeded")
    return json.loads(raw, parse_float=reject_number, parse_constant=reject_number,
                      object_pairs_hook=unique_object)


def read_json(path):
    with Path(path).open("rb") as source:
        return decode(source.read(MAX_BYTES + 1))


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise ValueError("unexpected HTTP redirect")


def http_json(url, method="GET", payload=None):
    data = None if payload is None else json.dumps(payload).encode()
    request = urllib.request.Request(url, data=data, method=method, headers={
        "Content-Type": "application/json", "User-Agent": "diary-publisher/2",
    })
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    with opener.open(request, timeout=10) as response:
        return decode(response.read(MAX_BYTES + 1))


def atomic_write(path, raw):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=".publisher-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as output:
            output.write(raw)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def load_state(path):
    try:
        state = read_json(path)
    except FileNotFoundError:
        return {"version": 1, "published": []}
    if not isinstance(state, dict):
        raise ValueError("invalid publisher state; preserve it for recovery")
    keys = state.get("published")
    if (state.get("version") != 1 or not isinstance(keys, list)
            or len(keys) > HISTORY_LIMIT
            or any(not isinstance(key, str) or not re.fullmatch(r"[0-9a-f]{64}", key) for key in keys)
            or len(set(keys)) != len(keys)):
        raise ValueError("invalid publisher state; preserve it for recovery")
    return state


def uint(value, name):
    if type(value) is not int or not 0 <= value <= (1 << 63) - 1:
        raise ValueError("invalid integer " + name)
    return value


def github_json(endpoint):
    result = subprocess.run(
        ["gh", "api", "--hostname", "github.com", endpoint],
        capture_output=True, check=True, timeout=20,
    )
    return decode(result.stdout)


def check_evidence(record, policies, api=github_json):
    """Require a changed full commit and every explicitly configured CI job."""
    repo = record["repo_name"]
    sha = record.get("git_commit_after", "")
    required = policies.get(repo, [])
    if (not isinstance(required, list) or any(not isinstance(name, str) or not name for name in required)
            or len(set(required)) != len(required)):
        raise ValueError("invalid required CI policy")
    missing = {"verification": "unverified", "detail": "No configured CI evidence for this result.",
               "commit_url": "", "checks_url": "", "check_urls": []}
    if not isinstance(sha, str) or not re.fullmatch(r"[0-9a-f]{40}", sha):
        return dict(missing, detail="Source has no full commit ID.")
    commit_url = f"https://github.com/8b-is/{repo}/commit/{sha}"
    missing.update(commit_url=commit_url, checks_url=commit_url + "/checks")
    before = record.get("git_commit_before", "")
    if (not isinstance(before, str) or not re.fullmatch(r"[0-9a-f]{40}", before)
            or sha == before or record.get("applied_to_repo") is not True):
        return dict(missing, detail="Source does not identify a changed, applied commit.")
    if not required:
        return missing
    commit = api(f"repos/8b-is/{repo}/commits/{sha}")
    if commit.get("sha") != sha:
        raise ValueError("GitHub commit identity mismatch")
    runs = api(f"repos/8b-is/{repo}/commits/{sha}/check-runs?filter=latest&per_page=100")
    if uint(runs.get("total_count"), "total_count") > 100:
        return dict(missing, detail="CI response exceeds the bounded check window.")
    if not isinstance(runs.get("check_runs"), list) or len(runs["check_runs"]) != runs["total_count"]:
        raise ValueError("incomplete CI response")
    links = []
    for name in required:
        matches = [run for run in runs.get("check_runs", [])
                   if run.get("name") == name and run.get("head_sha") == sha
                   and run.get("app", {}).get("slug") == "github-actions"]
        if len(matches) != 1:
            return dict(missing, detail=f"Required GitHub Actions check missing or ambiguous: {name}")
        run = matches[0]
        if run.get("status") != "completed":
            return dict(missing, verification="pending", detail=f"CI pending: {name}")
        if run.get("conclusion") != "success":
            return dict(missing, verification="failed", detail=f"CI did not pass: {name}")
        url = run.get("details_url", "")
        if not url.startswith(f"https://github.com/8b-is/{repo}/actions/runs/"):
            raise ValueError("unexpected CI log URL")
        links.append(url)
    return dict(missing, verification="ci_passed", check_urls=links,
                detail="Configured GitHub Actions checks passed: " + ", ".join(required)
                + ". This does not verify a benchmark speedup or deployment.")


def observe(data, config, now, verifier=check_evidence):
    snapshot = {
        "schema_version": 2, "observed_at_secs": now, "source_timestamp_secs": 0,
        "cycle_id": 0, "repo": "", "symbol": "", "verification": "unverified",
        "state": "waiting", "detail": "No completed result reported by the source.",
        "commit_url": "", "checks_url": "", "check_urls": [],
        "source_url": config["source_url"], "event_id": "",
    }
    if not isinstance(data, dict) or data.get("ok") is not True:
        raise ValueError("source did not return ok=true")
    records = data.get("recent_records")
    if not isinstance(records, list):
        raise ValueError("source records are missing")
    if not records:
        return snapshot
    record = records[0]
    if not isinstance(record, dict):
        raise ValueError("invalid source record")
    timestamp = uint(record.get("timestamp_secs"), "timestamp_secs")
    cycle = uint(record.get("cycle_id"), "cycle_id")
    repo = record.get("repo_name", "")
    symbol = record.get("target_symbol", "")
    if (not timestamp or timestamp > now or not cycle
            or not isinstance(repo, str) or not re.fullmatch(r"[A-Za-z0-9_.-]{1,100}", repo)
            or repo in (".", "..") or not isinstance(symbol, str) or len(symbol.encode()) > 256):
        raise ValueError("invalid result identity or timestamp")
    snapshot.update(source_timestamp_secs=timestamp, cycle_id=cycle, repo=repo, symbol=symbol)
    if now - timestamp > 600:
        snapshot.update(state="stale", detail="Source result is older than 600 seconds; no announcement.")
        return snapshot
    snapshot.update(verifier(record, config["required_checks"]))
    snapshot["state"] = snapshot["verification"]
    if snapshot["verification"] == "ci_passed":
        # One announcement per repository commit, even if the cycle counter changes.
        identity = repo + ":" + record["git_commit_after"]
        snapshot["event_id"] = hashlib.sha256(identity.encode()).hexdigest()
    return snapshot


def status_bytes(snapshot):
    lines = ["DIARY_MAINTENANCE\t1"]
    for key in ("observed_at_secs", "source_timestamp_secs", "cycle_id"):
        lines.append(f"{key}\t{uint(snapshot[key], key)}")
    for key in ("repo", "symbol", "verification", "state", "detail", "commit_url", "checks_url", "source_url"):
        lines.append(key + "\t" + snapshot[key].encode().hex())
    return ("\n".join(lines) + "\n").encode()


def markdown(snapshot):
    # These are observations and linked CI receipts, never generated success prose.
    return ("# Agent maintenance observation\n\n"
            f"State: **{snapshot['state']}** · verification: **{snapshot['verification']}**\n\n"
            f"Cycle: {snapshot['cycle_id']} · repository: `{snapshot['repo']}`\n\n"
            f"Observed UTC seconds: {snapshot['observed_at_secs']} · "
            f"source UTC seconds: {snapshot['source_timestamp_secs']}\n\n"
            f"{snapshot['detail']}\n\n"
            f"Commit: {snapshot['commit_url'] or 'unavailable'}\n\n"
            f"Checks and logs: {snapshot['checks_url'] or 'unavailable'}\n")


def export(snapshot, config):
    """Atomic files on the already configured diary host; no public write API."""
    directory = config["remote_public_dir"]
    files = {
        directory + "/next-level.json": json.dumps(snapshot).encode().hex(),
        directory + "/next-level.md": markdown(snapshot).encode().hex(),
        config["remote_status_path"]: status_bytes(snapshot).hex(),
    }
    program = """import json, os, pathlib, sys, tempfile
for name, encoded in json.load(sys.stdin).items():
    path = pathlib.Path(name)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix='.publisher-', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as output:
            output.write(bytes.fromhex(encoded))
            output.flush()
            os.fsync(output.fileno())
            os.fchmod(output.fileno(), 0o644)
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try: os.fsync(directory)
        finally: os.close(directory)
    finally:
        if os.path.exists(temporary): os.unlink(temporary)
"""
    subprocess.run(
        ["ssh", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes", "-o", "ConnectTimeout=10",
         config["remote_host"], "python3 -c " + shlex.quote(program)],
        input=json.dumps(files).encode(), capture_output=True, check=True, timeout=30,
    )


def update_nibble(snapshot, base, request=http_json):
    slug = "g8t-next-level-live"
    existing = request(base + "/nibbles/slug/" + slug)
    payload = {"title": "Agent maintenance: " + snapshot["state"], "content": markdown(snapshot),
               "category": "next-level", "priority": 98, "is_active": True}
    if existing.get("success") and existing.get("nibble"):
        result = request(base + "/nibbles/" + str(existing["nibble"]["id"]), "PUT", payload)
    else:
        payload.update(slug=slug, author="g8t maintenance publisher")
        result = request(base + "/nibbles", "POST", payload)
    if result.get("success") is not True:
        raise ValueError("nibble update was not acknowledged")


def publish_event(snapshot, state, config, request=http_json):
    key = snapshot["event_id"]
    if snapshot["state"] != "ci_passed" or not key:
        return "withheld"
    if key in state["published"]:
        return "already_published"
    # Detailed commit and CI URLs live in /api/maintenance, outside the 96-byte note.
    commit = snapshot["commit_url"].rsplit("/", 1)[-1]
    message = f"CI: {snapshot['repo']}@{commit}; logs"
    if len(message.encode()) > 96:
        raise ValueError("commit reference exceeds the diary note capacity")
    response = request(config["diary_url"] + "/api/diary/pulse", "POST", {
        "author": "g8t maintenance", "message": message, "hz": 432, "trit": 0, "event_id": key,
    })
    if response.get("ok") is not True:
        raise ValueError("diary did not acknowledge the event")
    next_state = dict(state, published=(state["published"] + [key])[-HISTORY_LIMIT:])
    atomic_write(Path(config["state_dir"]) / "state.json", json.dumps(next_state).encode())
    state.update(next_state)
    return "already_delivered" if response.get("duplicate") is True else "published"


def cycle(config):
    root = Path(config["state_dir"])
    root.mkdir(parents=True, exist_ok=True)
    with (root / "writer.lock").open("a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            print('{"ok":true,"status":"writer_busy"}')
            return 0
        now = time.time_ns() // 1_000_000_000
        errors = []
        state = None
        try:
            state = load_state(root / "state.json")
            snapshot = observe(http_json(config["source_url"]), config, now)
        except Exception as error:
            snapshot = observe({"ok": True, "recent_records": []}, config, now)
            snapshot.update(state="error", detail="Source/evidence check failed: " + str(error)[:400])
            errors.append(snapshot["detail"])
        if (snapshot["state"] == "ci_passed"
                and time.time_ns() // 1_000_000_000 - snapshot["source_timestamp_secs"] > 600):
            snapshot.update(state="stale", detail="Source result aged out during evidence verification; no announcement.")
        atomic_write(root / "observation.json", json.dumps(snapshot).encode())
        action = "withheld"
        try:
            export(snapshot, config)
            if state is not None:
                action = publish_event(snapshot, state, config)
        except Exception as error:
            errors.append("Delivery: " + str(error)[:400])
        for base in config.get("nibble_urls", []):
            try:
                update_nibble(snapshot, base)
            except Exception as error:
                errors.append("Site update: " + str(error)[:400])
        receipt = {"ok": not errors, "observed_at_secs": now, "cycle_id": snapshot["cycle_id"],
                   "state": snapshot["state"], "action": action, "errors": errors}
        atomic_write(root / "receipt.json", json.dumps(receipt).encode())
        print(json.dumps(receipt))
        return 1 if errors else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", default="/etc/g8t-nextlevel-publisher.json")
    args = parser.parse_args()
    try:
        return cycle(read_json(args.config))
    except Exception as error:
        print(json.dumps({"ok": False, "status": "blocked", "error": str(error)[:400]}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
