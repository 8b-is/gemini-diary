# Evidence and storage contracts

## Diary API

POST bodies are bounded flat JSON objects, with string fields, signed i64 integer
fields (`hz`, `trit`), boolean `triad`, and a string array `capsules`. Duplicate
keys, numeric fractions/exponents, wrong field types, nested objects and trailing
data are rejected. HTTP headers plus body must fit 16384 bytes; incomplete or
oversized requests are dropped. Hz is clamped to 108..1728 and trits to -1..1.

Omitting `capsules` executes all five available stages. An explicit empty array,
unknown/unavailable capsule, or missing dependency returns 400. Salience requires
the oscillator; Goertzel and WaveInt require salience. TOSS is independent.
Disabled salience/frame outputs are `null`; disabled waveform/TOSS arrays are
empty. `estimated_cycles` replaces the old `total_cycles`; static budgets are not
benchmarks. `elapsed_microseconds` measures the computation before persistence,
not HTTP or disk latency. The Ed25519 capsule cannot be selected.

An `event_id` must contain 64 lowercase hex digits. The stored normalized payload
contains author/message bytes and clamped Hz/trit. Identical replay returns
`{"ok":true,"duplicate":true}` without a write; a conflicting payload returns
409. Keyed notes must fit 24 author bytes and 96 message bytes (UTF-8). The key
ledger holds 256 events independently of the 32-note ring. Deduplication is
guaranteed within that retained window, not forever. Handles and keys are not
authentication. Persistence failure returns 503 and leaves memory uncommitted.
HEAD requests do not mutate state.

## Diary snapshot: `DIARY_STATE\t2`

`STATE_PATH` defaults to `/var/lib/gemini-diary/state.m8`. This is UTF-8 TSV,
**not Aurora MEM|8**. Lines end in newline. Maximum input: 262144 bytes.

- Header: `DIARY_STATE<TAB>2`
- One `COUNTERS<TAB>run_u64<TAB>pulse_u64`
- One `VOTE_PROVENANCE<TAB>legacy_unattributed|visitor_unattributed`
- Six `VOTE<TAB>capsule_id<TAB>positive_u32<TAB>neutral_u32<TAB>negative_u32`
- At most 32 ascending `PULSE<TAB>id<TAB>hz<TAB>trit<TAB>p_i32<TAB>q_i32<TAB>author_hex<TAB>message_hex`
- At most 256 `EVENT<TAB>key<TAB>author_hex:message_hex:hz:trit`

Legacy unversioned COUNTERS/VOTE/PULSE snapshots are validated and loaded without
rewriting. The next successful mutation writes v2 and marks votes as legacy.
Invalid, truncated, unreadable or unsupported snapshots stop startup; missing
files start a new diary. The original is never overwritten to make loading work.
The server holds a process-lifetime OS lock on `STATE_PATH.lock`. A mutation
stages a clone, fsyncs a same-directory temporary file, renames it, fsyncs its
directory, then commits memory. Snapshot text has structural validation, no
cryptographic integrity claim. Retain operational backups for recovery.
Telemetry is process-local; persisted pulse/run counters retain their history.

## Publisher state and observations

`scripts/publisher.py` uses Python's standard library plus configured `gh` and
SSH executables. It rejects non-integer/duplicate-key JSON and bounds inputs at
1048576 bytes. Time is integer UTC seconds (`time_ns // 1000000000`).
The oneshot service has a 120-second overall deadline and individual HTTP,
GitHub and SSH timeouts. HTTP proxies and redirects are disabled.

`state_dir` contains:

- `writer.lock`: nonblocking exclusive OS writer lock; a busy run reports waiting.
- `state.json`: `{"version":1,"published":["sha256-event-key",...]}` (max 256).
- `observation.json`: schema 2 source timestamp, observation time, source identity,
  verification, state, commit/check URLs, and explanation.
- `receipt.json`: delivery action, errors and completion result for the last run.

Writes use temporary files, fsync, atomic rename and directory fsync. Invalid
state is preserved, reporting is blocked, and an error observation is exported.
If remote delivery succeeds but its acknowledgment or local save fails, the next
run resends the same key; the diary's ledger resolves the ambiguity.

Only fresh changed/applied full commits with every named GitHub Actions check
completed successfully can be announced. `gh api` checks the commit SHA and each
check's head SHA, GitHub Actions app and log URL. More than 100 checks, ambiguous
names, missing policies and incomplete results withhold publication. This is an
explicit CI-job policy, not a claim that arbitrary repository tests establish
behavioral improvement. GitHub is a publication/evidence boundary, not inference.

## Local observation: `DIARY_MAINTENANCE\t1`

`MAINTENANCE_STATUS_PATH` defaults to
`/var/lib/gemini-diary/maintenance.status`. Maximum: 8192 bytes. Header followed by
exactly 11 unique fields, newline terminated:

- Decimal unsigned `observed_at_secs`, `source_timestamp_secs`, `cycle_id`.
- Hex-encoded UTF-8 `repo`, `symbol`, `verification`, `state`, `detail`,
  `commit_url`, `checks_url`, `source_url`.

The publisher atomically installs this file over the configured SSH connection.
No public status-write endpoint exists. `/api/maintenance` returns `unavailable`
on missing/malformed/future observations; age over 600 seconds yields `stale`
(source) or `publisher_stale` (publisher, taking precedence). Zero source time
means no result. Cycle IDs are strings in this API to avoid browser precision
loss. Responses use `Cache-Control: no-store`.

## WaveInt v8

79 bytes: `MEM8` magic (0..4), version 8 (4), big-endian u16 Hz (5..7),
big-endian i32 numerator/positive denominator (7..15), nine trits encoded as
0/1/2 (15..24), big-endian u64 tick (24..32), zero-padded UTF-8 tag (32..78),
CRC-8 (78). CRC starts at `0x5A`, polynomial `0x07`, MSB first over bytes 0..78,
no final XOR. CRC and trit/denominator checks establish structural integrity,
not sender identity or durable memory recall. No Ed25519 signature is attached.
