import ast
import copy
import fcntl
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

import publisher as p


class PublisherTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="diary-publisher-")
        self.addCleanup(self.temp.cleanup)
        self.config = {"source_url": "http://127.0.0.1/source", "required_checks": {"aurora": ["rust", "go"]},
                       "diary_url": "https://diary.invalid", "state_dir": self.temp.name}
        self.record = {"cycle_id": 42, "timestamp_secs": 1000, "repo_name": "aurora", "target_symbol": "task",
                       "git_commit_before": "a" * 40, "git_commit_after": "b" * 40, "applied_to_repo": True}
        self.runs = {"total_count": 2, "check_runs": [
            {"name": name, "head_sha": "b" * 40, "app": {"slug": "github-actions"}, "status": "completed",
             "conclusion": "success", "details_url": "https://github.com/8b-is/aurora/actions/runs/1"}
            for name in ("rust", "go")]}

    def api(self, endpoint):
        return self.runs if "check-runs?" in endpoint else {"sha": "b" * 40}

    def verify(self, record=None, policies=None):
        return p.check_evidence(record or self.record, self.config["required_checks"] if policies is None else policies, self.api)

    def observe(self, record=None, now=1100):
        return p.observe({"ok": True, "recent_records": [record or self.record]}, self.config, now,
                         lambda record, policies: self.verify(record, policies))

    def test_fresh_ci_evidence_and_commit_identity(self):
        snapshot = self.observe()
        self.assertEqual(snapshot["state"], "ci_passed")
        self.assertEqual(len(snapshot["event_id"]), 64)
        other_cycle = dict(self.record, cycle_id=43)
        self.assertEqual(snapshot["event_id"], self.observe(other_cycle)["event_id"])
        with self.assertRaises(ValueError):
            p.check_evidence(self.record, self.config["required_checks"], lambda _: {"sha": "c" * 40})

    def test_requires_changed_applied_full_commit_and_explicit_policy(self):
        for change in [{"git_commit_after": "short+bad"}, {"git_commit_before": "b" * 40},
                       {"applied_to_repo": False}, {"git_commit_before": ""}]:
            self.assertEqual(self.verify(dict(self.record, **change))["verification"], "unverified")
        self.assertEqual(self.verify(policies={})["verification"], "unverified")
        with self.assertRaises(ValueError):
            self.verify(policies={"aurora": "rust"})

    def test_check_status_identity_application_and_ambiguity(self):
        original = copy.deepcopy(self.runs)
        for change, expected in [({"status": "in_progress"}, "pending"),
                                  ({"conclusion": "failure"}, "failed"),
                                  ({"conclusion": "skipped"}, "failed"),
                                  ({"head_sha": "c" * 40}, "unverified"),
                                  ({"app": {"slug": "other"}}, "unverified")]:
            self.runs = copy.deepcopy(original)
            self.runs["check_runs"][0].update(change)
            self.assertEqual(self.verify()["verification"], expected)
        self.runs = copy.deepcopy(original)
        self.runs["check_runs"].append(self.runs["check_runs"][0])
        self.runs["total_count"] = 3
        self.assertEqual(self.verify()["verification"], "unverified")
        self.runs["total_count"] = 101
        self.assertEqual(self.verify()["verification"], "unverified")
        self.runs["total_count"] = 2
        with self.assertRaises(ValueError):
            self.verify()

    def test_stale_future_and_malformed_source(self):
        self.assertEqual(self.observe(now=1600)["state"], "ci_passed")
        self.assertEqual(self.observe(now=1601)["state"], "stale")
        for change in [{"timestamp_secs": 1101}, {"timestamp_secs": True}, {"timestamp_secs": -1},
                       {"cycle_id": "42"}, {"repo_name": "../aurora"}, {"target_symbol": "x" * 257}]:
            with self.assertRaises(ValueError):
                self.observe(dict(self.record, **change))
        for data in [{"ok": False}, {"ok": True}, {"ok": True, "recent_records": [None]}]:
            with self.assertRaises(ValueError):
                p.observe(data, self.config, 1100)
        self.assertEqual(p.observe({"ok": True, "recent_records": []}, self.config, 1100)["state"], "waiting")

    def test_json_rejects_floats_duplicate_keys_and_oversize(self):
        for raw in [b'{"n":1.5}', b'{"n":1e3}', b'{"n":NaN}', b'{"x":1,"x":2}', b" " * (p.MAX_BYTES + 1)]:
            with self.assertRaises(ValueError):
                p.decode(raw)

    def test_python_numeric_syntax(self):
        root = Path(__file__).resolve().parents[1]
        for path in [*root.joinpath("scripts").glob("*.py"), *root.joinpath("e2e").glob("*.py")]:
            for node in ast.walk(ast.parse(path.read_text())):
                if isinstance(node, ast.Constant):
                    self.assertNotIn(type(node.value).__name__, ("float", "complex"), str(path))
                if isinstance(node, ast.Call):
                    self.assertNotIn(ast.unparse(node.func),
                                     ("float", "time.time", "time.monotonic", "time.perf_counter"), str(path))
        # Path's overloaded / operator joins paths; numerical divisions use //.

    def test_delivery_restart_dedup_and_failed_ack(self):
        snapshot = self.observe()
        state = p.load_state(Path(self.temp.name) / "state.json")
        request = Mock(return_value={"ok": True})
        self.assertEqual(p.publish_event(snapshot, state, self.config, request), "published")
        reloaded = p.load_state(Path(self.temp.name) / "state.json")
        self.assertEqual(p.publish_event(snapshot, reloaded, self.config, request), "already_published")
        self.assertEqual(request.call_count, 1)
        for status in ["stale", "failed", "pending", "unverified", "waiting", "error"]:
            self.assertEqual(p.publish_event(dict(snapshot, state=status), state, self.config, request), "withheld")
        state = {"version": 1, "published": []}
        with self.assertRaises(ValueError):
            p.publish_event(snapshot, state, self.config, Mock(return_value={"ok": False}))
        self.assertFalse(state["published"])

    def test_lost_ack_or_local_save_retry_uses_same_event_key(self):
        snapshot = self.observe()
        state = {"version": 1, "published": []}
        request = Mock(side_effect=[TimeoutError("ack lost"), {"ok": True, "duplicate": True}])
        with self.assertRaises(TimeoutError):
            p.publish_event(snapshot, state, self.config, request)
        self.assertEqual(p.publish_event(snapshot, state, self.config, request), "already_delivered")
        self.assertEqual(request.call_args_list[0], request.call_args_list[1])
        state = {"version": 1, "published": []}
        with patch.object(p, "atomic_write", side_effect=OSError("disk full")):
            with self.assertRaises(OSError):
                p.publish_event(snapshot, state, self.config, Mock(return_value={"ok": True}))
        self.assertFalse(state["published"])

    def test_corrupt_state_preserved_and_failure_receipt_exported(self):
        path = Path(self.temp.name) / "state.json"
        for raw in [b"broken", b"[]", b'{"version":2,"published":[]}',
                    json.dumps({"version": 1, "published": ["a" * 64] * 2}).encode()]:
            path.write_bytes(raw)
            with self.assertRaises(ValueError):
                p.load_state(path)
        with patch.object(p, "export") as export, patch.object(p, "http_json") as request:
            self.assertEqual(p.cycle(self.config), 1)
            request.assert_not_called()
            self.assertEqual(export.call_args.args[0]["state"], "error")
        self.assertEqual(path.read_bytes(), raw)
        receipt = p.read_json(Path(self.temp.name) / "receipt.json")
        self.assertFalse(receipt["ok"])
        self.assertEqual(receipt["action"], "withheld")

    def test_writer_lock_and_failed_export_never_post(self):
        with (Path(self.temp.name) / "writer.lock").open("a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            with patch.object(p, "http_json") as request:
                self.assertEqual(p.cycle(self.config), 0)
                request.assert_not_called()
        with patch.object(p, "observe", return_value=self.observe()), patch.object(p, "http_json"), \
                patch.object(p, "export", side_effect=OSError("SSH failed")), patch.object(p, "publish_event") as publish:
            self.assertEqual(p.cycle(self.config), 1)
            publish.assert_not_called()


if __name__ == "__main__":
    unittest.main()
