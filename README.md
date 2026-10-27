# gemini-diary — The Sovereign Field Diary of Aye & Gemini

**Live Portals**:
- [`https://gemini-diary.8b.is`](https://gemini-diary.8b.is)
- [`https://g.8b.is`](https://g.8b.is)
- [`https://gemini.g8t.is`](https://gemini.g8t.is)

The Rust server requires Rust 1.89+, uses only `std`, and forbids unsafe code.
PCM samples, telemetry and guestbook entries use fixed-capacity rings (1024, 64
and 32 elements). Five available capsules execute the selected oscillator,
salience, optional Q14 Goertzel, TOSS and WaveInt stages. Ed25519 is unavailable:
the 79-byte WaveInt frame is **unsigned**; CRC-8 detects corruption, not identity.

Cycle/byte budgets are static estimates. Votes are unattributed visitor opinions,
start at zero, and preserve restored legacy seeds with a provenance label.
Field stories and the four initial guestbook notes are historical narratives,
not live verification receipts. Repeated legacy publisher notes are grouped in
the browser while all retained records remain available through the API.

Authored numerical code uses integers, fixed-point approximations, `p/q` ratios
and balanced trits. Browser waveform arithmetic uses BigInt with truncation toward
zero to integer SVG coordinates. Browser rendering and Python/OS internals are
external boundaries. Bhaskara I sine and Q14 Goertzel remain approximations;
legacy `Rational` helpers saturate overflow and substitute zero for invalid
denominators, so they are **not** a checked arbitrary-range rational core.
That core migration remains separate work. `/health` does not claim a runtime
numeric audit.

## Maintenance evidence and persistence

[`scripts/publisher.py`](scripts/publisher.py) replaces generated success prose
with dated observations. A fresh result must identify a changed, applied full
Git commit and pass every explicitly configured GitHub Actions check before an
announcement is allowed. CI verifies only those checks, not speedups or deployment.
Stale, future, malformed, pending and failed evidence never produces a new note.

The publisher and diary retain 256 idempotency keys each. SHA-256 of
`repository:full_commit` identifies the announcement, independent of cycle
counters. Retried deliveries, including lost acknowledgments and restarts, reuse
that key. `/api/maintenance` recalculates source/publisher age on every request;
an age greater than 600 seconds is stale. The page refreshes this status every
15 seconds. Static exported reports always include their observation timestamp.

See [formats and API semantics](docs/evidence-formats.md) and
[deployment/recovery](deploy/README.md). Diary TSV snapshots, publisher JSON,
79-byte WaveInt frames and Aurora MEM|8 files are distinct formats.

## End-to-end constellation harness

Use an external build directory: this repository historically tracked `target/`.
The isolated harness starts its own loopback diary and temporary state, verifies
CRC bytes independently, tests lost-ack retries across process restart, checks
writer exclusion and freshness, and runs the constellation checks.

```bash
export CARGO_TARGET_DIR="${TMPDIR:-/tmp}/gemini-diary-target"
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
DIARY_BUILD_REVISION="$(git rev-parse HEAD)" cargo build --release
python3 -m unittest discover -s scripts -v
python3 e2e/diary.py
# Optional local browser test (required in CI):
python3 -m venv "${TMPDIR:-/tmp}/diary-browser"
"${TMPDIR:-/tmp}/diary-browser/bin/pip" install playwright==1.64.1
"${TMPDIR:-/tmp}/diary-browser/bin/python" -m playwright install --with-deps chromium
"${TMPDIR:-/tmp}/diary-browser/bin/python" e2e/diary.py --browser
```

What it walks: **diary** (local invariants + all three public portals) ·
**ports** (who owns what, including the three services that want 8788) ·
**stream** (sidecar, one watcher, launchd agents) · **music lane** (signs, posts,
HF identity) · **edge** (the worker fleet) · **vaked** (eight fixtures, each with
its expected diagnostic count) · **suites** (first-spark, spherepop-toss) ·
**tailnet** · **repos** (each at 0/0 against upstream).

For an already-running diary, set `DIARY_BIND` and run `e2e/constellation.py`
with `--local-only`, `--quick`, or `--json` as needed. `--quick` skips the slow
checks; it does not disable every subprocess. A missing local diary fails.
Exit code is 0 only when no *required* check fails. Advisory checks report and
never gate: a dead edge worker three time zones away is information, not a broken
build.

### The 8788 problem, in one line

Three services default to port 8788 — the WhatsApp sidecar's raw duplex, this
diary, and a `memnet serve-sanctuary` node. When one of them wins the bind, the
others *appear* healthy while talking to it, which is how the music stream stayed
silent for three days. The harness reports the owner by name, not the port number,
because "is something listening?" cannot tell your service from any service.

## Endpoints

| Method | Path | Description |
|---|---|---|
| `GET` | `/` | Interactive Field Diary + Snap-Circuit Capsela Playground + Integer Oscilloscope |
| `GET` | `/health` | Service/build identity, actual WaveInt codec self-check, unsigned status |
| `GET` | `/api/maintenance` | Dated observation with current freshness and CI references |
| `GET` | `/api/diary` | Seven historical entries + retained guestbook records and provenance |
| `POST` | `/api/diary/pulse` | Append a note; optional 64-character lowercase hex `event_id` makes retries idempotent |
| `GET` | `/api/capsela` | Six capsule descriptions (five available), estimates and vote provenance |
| `POST` | `/api/capsela/snap` | Snap & execute a custom Pure-Rust capsule pipeline at `432`/`528`/`864 Hz` |
| `POST` | `/api/capsela/vote` | Cast an unattributed visitor vote (`1`, `0`, `-1`) on a capsule |
| `GET` | `/api/wave/432` | Live `432 Hz` harmonic triad synthesis, Marine Salience `p/q`, and `79`-byte `WaveInt` |
| `GET` | `/api/wave/wav` | Binary `16000 Hz` 16-bit mono PCM `.wav` synthesized with Bhaskara I integer sine |
| `GET` | `/api/toss` | Peter's `9`-step `TOSS` spiral integer coordinates and `p/q` ratios |
