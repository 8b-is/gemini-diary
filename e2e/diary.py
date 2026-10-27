#!/usr/bin/env python3
"""Isolated HTTP/restart/publisher contract tests; --browser also runs Chromium.

Uses a fresh temporary state directory and literal loopback. Never posts to a
production diary. DIARY_BINARY can select a compatibility/deployment artifact.
"""
import argparse
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import publisher as p


def port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class Diary:
    def __init__(self, directory):
        self.directory = Path(directory)
        self.port = port()
        self.base = f"http://127.0.0.1:{self.port}"
        self.binary = os.environ.get("DIARY_BINARY", str(Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / "release/gemini-diary"))
        self.env = dict(os.environ, BIND_ADDR=f"127.0.0.1:{self.port}", STATE_PATH=str(self.directory / "state.m8"),
                        MAINTENANCE_STATUS_PATH=str(self.directory / "maintenance.status"))
        self.process = None

    def start(self):
        self.process = subprocess.Popen([self.binary], env=self.env, stdout=subprocess.DEVNULL)
        for _ in range(10):
            if self.process.poll() is not None:
                raise RuntimeError("diary exited during startup")
            try:
                self.get("/health")
                return
            except OSError:
                time.sleep(1)
        raise RuntimeError("diary did not start")

    def stop(self):
        if self.process is not None:
            self.process.terminate()
            self.process.wait(timeout=5)
            self.process = None

    def get(self, path):
        return p.http_json(self.base + path)


def contract(diary):
    health = diary.get("/health")
    assert health["service"] == "gemini-diary" and health["waveint_roundtrip_verified"] is True
    assert health["signature_status"] == "unsigned" and "float_count" not in health
    assert health["runtime_numeric_audit"] == "not_performed"
    initial = diary.get("/api/diary")
    caps = diary.get("/api/capsela")
    assert all(c["aye_votes_pos"] == c["aye_votes_zero"] == c["aye_votes_neg"] == 0 for c in caps["capsules"])
    wave = diary.get("/api/wave/432")
    frame = bytes.fromhex(wave["mem8_waveint_79b"]["hex"])
    assert len(frame) == 79 and frame[:5] == b"MEM8\x08"
    crc = 0x5A  # WaveInt v8's documented initial value, polynomial 0x07.
    for byte in frame[:-1]:
        crc ^= byte
        for _ in range(8):
            crc = ((crc << 1) ^ (7 if crc & 128 else 0)) & 255
    assert crc == frame[-1] == wave["mem8_waveint_79b"]["crc8"]
    assert wave["capsule_count"] == 5 and "dns_seal" not in wave["mem8_waveint_79b"]
    assert len(wave["oscilloscope_i16"]) == 64

    config = {"diary_url": diary.base, "state_dir": str(diary.directory / "publisher")}
    snapshot = {"event_id": "a" * 64, "state": "ci_passed", "repo": "aurora",
                "commit_url": "https://github.com/8b-is/aurora/commit/" + "b" * 40}
    state = {"version": 1, "published": []}

    def lost_ack(*args):
        p.http_json(*args)
        raise TimeoutError("simulated lost acknowledgement after real HTTP delivery")

    try:
        p.publish_event(snapshot, state, config, lost_ack)
        raise AssertionError("expected lost acknowledgement")
    except TimeoutError:
        pass
    diary.stop()
    diary.start()
    assert p.publish_event(snapshot, state, config) == "already_delivered"
    state = p.load_state(Path(config["state_dir"]) / "state.json")
    assert p.publish_event(snapshot, state, config) == "already_published"
    assert diary.get("/api/diary")["c64_guestbook_ring"]["total_writes"] == initial["c64_guestbook_ring"]["total_writes"] + 1
    try:
        p.http_json(diary.base + "/api/diary/pulse", "POST", {"event_id": "a" * 64, "message": "conflict"})
        raise AssertionError("expected conflict")
    except urllib.error.HTTPError as error:
        assert error.code == 409

    second = subprocess.run([diary.binary], env=dict(diary.env, BIND_ADDR=f"127.0.0.1:{port()}"),
                            capture_output=True, timeout=5)
    assert second.returncode != 0 and b"another diary owns this snapshot" in second.stderr
    assert diary.get("/api/maintenance")["state"] == "unavailable"
    now = time.time_ns() // 1_000_000_000
    observation = {"observed_at_secs": now, "source_timestamp_secs": now - 601, "cycle_id": 1774,
                   "repo": "aurora", "symbol": "task", "verification": "unverified", "state": "stale",
                   "detail": "Old result withheld", "commit_url": "", "checks_url": "", "source_url": "http://127.0.0.1/source"}
    status = diary.directory / "maintenance.status"
    p.atomic_write(status, p.status_bytes(observation))
    assert diary.get("/api/maintenance")["state"] == "stale"
    observation["observed_at_secs"] = now - 601
    p.atomic_write(status, p.status_bytes(observation))
    assert diary.get("/api/maintenance")["state"] == "publisher_stale"
    observation.update(observed_at_secs=now, source_timestamp_secs=now + 10)
    p.atomic_write(status, p.status_bytes(observation))
    assert diary.get("/api/maintenance")["state"] == "unavailable"
    observation.update(source_timestamp_secs=now - 601)
    p.atomic_write(status, p.status_bytes(observation))

    for _ in range(2):
        p.http_json(diary.base + "/api/diary/pulse", "POST", {"author": "g8t.is x A6000", "message": "Legacy repeated report"})
    result = subprocess.run([sys.executable, str(ROOT / "e2e/constellation.py"), "--local-only", "--quick"],
                            env=dict(os.environ, DIARY_BIND=f"127.0.0.1:{diary.port}"), capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stdout + result.stderr
    print("HTTP, independent CRC, lost-ack retry, restart, writer lock, freshness, constellation: PASS")


def browser(diary):
    from playwright.sync_api import sync_playwright, expect
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch()
        page = browser.new_page(reduced_motion="reduce")
        errors = []
        page.on("pageerror", lambda error: errors.append(str(error)))
        page.goto(diary.base)
        expect(page.locator("#dnsSealBox")).to_contain_text("unsigned")
        expect(page.locator("#maintenanceBadge")).to_have_text("stale")
        expect(page.locator("#pulseList .pulse-item").filter(has_text="Legacy repeated report")).to_have_count(1)
        expect(page.locator("#pulseList")).to_contain_text("2 stored occurrence(s)")
        assert len(page.locator("#oscLine").get_attribute("points").split()) == 64
        assert all(part.lstrip("-").isdigit() for pair in page.locator("#oscLine").get_attribute("points").split() for part in pair.split(","))
        expect(page.locator('input[onchange*="ed25519_dns_seal"]')).to_be_disabled()
        page.locator('input[onchange*="mem8_waveint_79b"]').uncheck()
        expect(page.locator("#waveIntHex")).to_have_text("WaveInt capsule disabled")
        page.locator('input[onchange*="c64_ring_osc_432"]').uncheck()
        expect(page.locator("#inspSalience")).to_have_text("Disabled")
        expect(page.locator("#oscLine")).to_have_attribute("points", "")
        expect(page.locator("#tossBody tr")).to_have_count(9)
        page.route("**/api/capsela/snap", lambda route: route.fulfill(status=503, content_type="application/json",
                                                                   body='{"ok":false,"error":"test persistence failure"}'))
        page.get_by_role("button", name="⚡ Snap & Run 432 Hz Triad").click()
        expect(page.locator("#operationError")).to_have_text("test persistence failure")
        page.route("**/api/maintenance", lambda route: route.abort())
        page.evaluate("loadMaintenance()")
        expect(page.locator("#maintenanceBadge")).to_have_text("unavailable")
        assert not errors, errors
        browser.close()
    print("Chromium: integer SVG, dependency toggles, unsigned state, legacy grouping, visible errors: PASS")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--browser", action="store_true")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="diary-e2e-") as directory:
        diary = Diary(directory)
        try:
            diary.start()
            contract(diary)
            if args.browser:
                browser(diary)
        finally:
            diary.stop()


if __name__ == "__main__":
    main()
