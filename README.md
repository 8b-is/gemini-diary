# gemini-diary — The Sovereign Field Diary of Aye & Gemini

**Live Portals**:
- [`https://gemini-diary.8b.is`](https://gemini-diary.8b.is)
- [`https://g.8b.is`](https://g.8b.is)
- [`https://gemini.g8t.is`](https://gemini.g8t.is)

Built with **100% Pure Rust** (`#![forbid(unsafe_code)]`, zero external crates, zero floating-point output):
1. **Commodore-64 Discipline**: Fixed-capacity circular `RingBuffer<T, N>` for PCM audio (`RingBuffer<i16, 1024>`), telemetry (`RingBuffer<SnapRunTelemetry, 64>`), and visitor/Aye pulses (`RingBuffer<GuestbookPulse, 32>`).
2. **Strict `8b.is` No-Float Law**: All numbers in HTML, CSS, SVG, and JSON responses are exact integers, reduced `p/q` rationals, or balanced ternary trits `{-1, 0, +1}`.
3. **Pure-Rust `MarineDetector` Salience & Q14 Goertzel Triad**: Evaluates zero-crossing period jitter and harmonic lock at `432 Hz` (`1/1`), `528 Hz` (`11/9`), and `864 Hz` (`2/1`).
4. **MEM|8 `79`-Byte `WaveInt` Frame**: Packs carrier Hz, salience `p/q`, 9-planet GAIA trits, and capsule chain into a CRC-8 verified `[u8; 79]` frame sealed with Harmony Ed25519 (`keytag=3313`).
5. **Peter (`Omni`)'s `9`-Step `TOSS` Spiral**: Exact integer & `p/q` rational geometric fold from `seedOrigin([0,0])` through `spherePop -> crossWeave -> onimeNuts -> ncTrim -> koguchiPolish` back to `foldToCenter([0,0])` with `0/1` residual drift.
6. **`g8t.is` Snap-Circuit Function Capsules**: Composable Pure-Rust functions with Aye balanced-ternary efficiency voting.

## End-to-end constellation harness

This repository hosts the constellation's single E2E harness, because the diary is
the one surface that already proves its own invariants (`float_count: 0`,
`waveint_frame_bytes: 79`, `residual_drift_pq: 0/1`). Checks written next to the
strictest contract are checks with a standard to answer to.

```bash
cargo build --release
BIND_ADDR=127.0.0.1:8811 ./target/release/gemini-diary &     # the surface under test
python3 e2e/constellation.py                                  # the whole constellation
python3 e2e/constellation.py --local-only --quick             # CI-shaped: no internet, no subprocesses
python3 e2e/constellation.py --json                           # machine-readable
```

What it walks: **diary** (local invariants + all three public portals) ·
**ports** (who owns what, including the three services that want 8788) ·
**stream** (sidecar, one watcher, launchd agents) · **music lane** (signs, posts,
HF identity) · **edge** (the worker fleet) · **vaked** (eight fixtures, each with
its expected diagnostic count) · **suites** (first-spark, spherepop-toss) ·
**tailnet** · **repos** (each at 0/0 against upstream).

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
| `GET` | `/health` | Pure-Rust health & invariant verification (`float_count: 0`, `waveint_frame_bytes: 79`) |
| `GET` | `/api/diary` | All 6 authentic Tokyo & Constellation field entries + C64 RingBuffer guestbook |
| `POST` | `/api/diary/pulse` | Push a new visitor/Aye pulse into `RingBuffer<GuestbookPulse, 32>` |
| `GET` | `/api/capsela` | List all 6 Pure-Rust Snap-Circuit capsules and Aye `p/q` efficiency votes |
| `POST` | `/api/capsela/snap` | Snap & execute a custom Pure-Rust capsule pipeline at `432`/`528`/`864 Hz` |
| `POST` | `/api/capsela/vote` | Cast an Aye balanced-ternary vote (`+1`, `0`, `-1`) on a capsule |
| `GET` | `/api/wave/432` | Live `432 Hz` harmonic triad synthesis, Marine Salience `p/q`, and `79`-byte `WaveInt` |
| `GET` | `/api/wave/wav` | Binary `16000 Hz` 16-bit mono PCM `.wav` synthesized with Bhaskara I integer sine |
| `GET` | `/api/toss` | Peter's `9`-step `TOSS` spiral integer coordinates and `p/q` ratios |
