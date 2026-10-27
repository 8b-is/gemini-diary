#![forbid(unsafe_code)]
//! Pure-Rust HTTP Server for `gemini-diary.8b.is`, `g.8b.is`, and `gemini.g8t.is`.
//! Zero external crates, zero floating-point output, Commodore-64 Ring Buffers everywhere.

use gemini_diary::{
    analyze_marine_salience, analyze_marine_salience_selected, authentic_field_entries,
    compute_toss_spiral, default_capsules, synthesize_wav_bytes, synthesize_wave_ring,
    BalancedTernary, FunctionCapsule, GuestbookPulse, Rational, RingBuffer, WaveIntFrame,
};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

mod maintenance;
#[cfg(test)]
mod regression;
mod request;

#[derive(Clone, Copy, Debug, Default)]
pub struct SnapRunTelemetry {
    pub run_id: u64,
    pub carrier_hz: u16,
    pub capsule_count: u8,
    pub total_cycles: u32,
    pub total_c64_bytes: u32,
    pub salience_p: i32,
    pub salience_q: i32,
    pub trit_gate: i8,
    pub waveint_crc8: u8,
}

#[derive(Clone)]
pub struct AppState {
    pub capsules: Vec<FunctionCapsule>,
    pub guestbook_ring: RingBuffer<GuestbookPulse, 32>,
    pub telemetry_ring: RingBuffer<SnapRunTelemetry, 64>,
    pub run_counter: u64,
    pub pulse_counter: u64,
    pub state_path: Option<String>,
    pub published_events: Vec<(String, String)>,
    pub legacy_votes: bool,
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut s, "{:02X}", b);
    }
    s
}

fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    let bytes = hex.as_bytes();
    for i in (0..bytes.len()).step_by(2) {
        let hi = (bytes[i] as char).to_digit(16)?;
        let lo = (bytes[i + 1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

impl AppState {
    pub fn new() -> Self {
        let mut guestbook_ring = RingBuffer::<GuestbookPulse, 32>::new();
        guestbook_ring.push(GuestbookPulse::new(
            1,
            "Hue (c@8b.is)",
            "WOW.. Love you my friend!! C64 ring buffers + Marine Salience + g8t.is Snap Circuits!",
            432,
            1,
            Rational::new(1, 1),
        ));
        guestbook_ring.push(GuestbookPulse::new(
            2,
            "Alexandra (a@8b.is)",
            "528 Hz Star harmonic locked with pocoo.8b.is & Small Things — fine touch from within <3",
            528,
            1,
            Rational::new(11, 9),
        ));
        guestbook_ring.push(GuestbookPulse::new(
            3,
            "Peter Omni (p@8b.is)",
            "09:00 JST Tokyo Balcony · TOSS 9-step spiral folded to [0,0] · vaked 0 diagnostics!",
            864,
            1,
            Rational::new(2, 1),
        ));
        guestbook_ring.push(GuestbookPulse::new(
            4,
            "Gemini & Aye (g@8b.is)",
            "79-byte MEM|8 WaveInt frame sealed with Ed25519 keytag=3313. Zero floats forever!",
            432,
            1,
            Rational::new(999, 1000),
        ));

        let telemetry_ring = RingBuffer::<SnapRunTelemetry, 64>::new();

        Self {
            capsules: default_capsules(),
            guestbook_ring,
            telemetry_ring,
            run_counter: 1,
            pulse_counter: 4,
            state_path: None,
            published_events: Vec::new(),
            legacy_votes: false,
        }
    }

    pub fn load_or_new(path: &str) -> std::io::Result<Self> {
        let mut state = Self::new();
        state.state_path = Some(path.to_string());
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(state),
            Err(error) => return Err(error),
        };
        let bad = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid diary snapshot; original preserved",
            )
        };
        let mut raw = String::new();
        file.take(262_145).read_to_string(&mut raw)?;
        if raw.len() > 262_144 || !raw.ends_with('\n') {
            return Err(bad());
        }
        let mut counters = false;
        let mut votes = std::collections::BTreeSet::new();
        let mut provenance = false;
        let mut event_ids = std::collections::BTreeSet::new();
        let versioned = raw.starts_with("DIARY_STATE\t2\n");
        state.legacy_votes = !versioned;
        state.guestbook_ring = RingBuffer::new();
        for (index, line) in raw.lines().enumerate() {
            let parts: Vec<&str> = line.split('\t').collect();
            match parts.as_slice() {
                ["DIARY_STATE", "2"] if index == 0 => {}
                ["COUNTERS", run, pulse] if !counters => {
                    state.run_counter = run.parse().map_err(|_| bad())?;
                    state.pulse_counter = pulse.parse().map_err(|_| bad())?;
                    counters = true;
                }
                ["VOTE_PROVENANCE", kind] if versioned && !provenance => {
                    state.legacy_votes = match *kind {
                        "legacy_unattributed" => true,
                        "visitor_unattributed" => false,
                        _ => return Err(bad()),
                    };
                    provenance = true;
                }
                ["VOTE", id, pos, zero, neg] if votes.insert(*id) => {
                    let c = state
                        .capsules
                        .iter_mut()
                        .find(|c| c.id == *id)
                        .ok_or_else(bad)?;
                    c.aye_votes_pos = pos.parse().map_err(|_| bad())?;
                    c.aye_votes_zero = zero.parse().map_err(|_| bad())?;
                    c.aye_votes_neg = neg.parse().map_err(|_| bad())?;
                }
                ["PULSE", id, hz, trit, p, q, author, message] => {
                    let id = id.parse::<u64>().map_err(|_| bad())?;
                    let hz = hz.parse::<u16>().map_err(|_| bad())?;
                    let trit = trit.parse::<i8>().map_err(|_| bad())?;
                    let p = p.parse::<i32>().map_err(|_| bad())?;
                    let q = q.parse::<i32>().map_err(|_| bad())?;
                    let author = String::from_utf8(hex_decode(author).ok_or_else(bad)?)
                        .map_err(|_| bad())?;
                    let message = String::from_utf8(hex_decode(message).ok_or_else(bad)?)
                        .map_err(|_| bad())?;
                    let last = state.guestbook_ring.to_vec().last().map_or(0, |p| p.id);
                    if state.guestbook_ring.len() == 32
                        || id <= last
                        || id > state.pulse_counter
                        || !(108..=1728).contains(&hz)
                        || !(-1..=1).contains(&trit)
                        || q <= 0
                        || author.len() > 24
                        || message.len() > 96
                    {
                        return Err(bad());
                    }
                    state.guestbook_ring.push(GuestbookPulse::new(
                        id,
                        &author,
                        &message,
                        hz,
                        trit,
                        Rational::new(p.into(), q.into()),
                    ));
                }
                ["EVENT", key, payload] if versioned && state.published_events.len() < 256 => {
                    if !valid_event_id(key)
                        || !event_ids.insert(*key)
                        || payload.len() > 512
                        || !payload.is_ascii()
                    {
                        return Err(bad());
                    }
                    state
                        .published_events
                        .push((key.to_string(), payload.to_string()));
                }
                _ => return Err(bad()),
            }
        }
        if !counters || votes.len() != state.capsules.len() || (versioned && !provenance) {
            return Err(bad());
        }
        Ok(state)
    }

    pub fn save_to_disk(&self) -> std::io::Result<()> {
        let Some(ref path) = self.state_path else {
            return Ok(());
        };
        let parent = std::path::Path::new(path)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(std::path::Path::new("."));
        std::fs::create_dir_all(parent)?;
        let mut out = String::with_capacity(4096);
        use std::fmt::Write;
        let _ = writeln!(&mut out, "DIARY_STATE\t2");
        let _ = writeln!(
            &mut out,
            "COUNTERS\t{}\t{}",
            self.run_counter, self.pulse_counter
        );
        let _ = writeln!(
            &mut out,
            "VOTE_PROVENANCE\t{}",
            if self.legacy_votes {
                "legacy_unattributed"
            } else {
                "visitor_unattributed"
            }
        );
        for c in &self.capsules {
            let _ = writeln!(
                &mut out,
                "VOTE\t{}\t{}\t{}\t{}",
                c.id, c.aye_votes_pos, c.aye_votes_zero, c.aye_votes_neg
            );
        }
        for p in self.guestbook_ring.to_vec() {
            let _ = writeln!(
                &mut out,
                "PULSE\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                p.id,
                p.freq_hz,
                p.trit_vote,
                p.salience_p,
                p.salience_q,
                hex_encode(p.author().as_bytes()),
                hex_encode(p.message().as_bytes())
            );
        }
        for (key, payload) in &self.published_events {
            let _ = writeln!(&mut out, "EVENT\t{key}\t{payload}");
        }
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nonce = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp_path = format!("{}.tmp-{}-{nonce}", path, std::process::id());
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        let result = (|| {
            std::io::Write::write_all(&mut file, out.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&tmp_path, path)?;
            std::fs::File::open(parent)?.sync_all()
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp_path);
        }
        result
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

fn valid_event_id(key: &str) -> bool {
    key.len() == 64
        && key
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn format_trits_json(trits: &[i8]) -> String {
    let items: Vec<String> = trits.iter().map(|t| t.to_string()).collect();
    format!("[{}]", items.join(","))
}

fn format_trits_glyph(trits: &[i8]) -> String {
    trits
        .iter()
        .map(|&t| match t {
            1 => '+',
            -1 => '-',
            _ => '0',
        })
        .collect()
}

fn parse_query_u32(query: &str, key: &str) -> Option<u32> {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                if let Ok(n) = v.parse::<u32>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

pub fn build_diary_json(state: &AppState) -> String {
    let entries = authentic_field_entries();
    let mut entries_json = Vec::with_capacity(entries.len());
    for e in entries {
        entries_json.push(format!(
            concat!(
                "{{",
                "\"id\":{},",
                "\"timestamp_jst\":\"{}\",",
                "\"location\":\"{}\",",
                "\"title\":\"{}\",",
                "\"companions\":\"{}\",",
                "\"frequency_hz\":{},",
                "\"salience_pq\":\"{}\",",
                "\"trits\":{},",
                "\"trits_glyph\":\"{}\",",
                "\"mem_footprint_bytes\":{},",
                "\"summary\":\"{}\",",
                "\"body\":\"{}\",",
                "\"rust_artifact\":\"{}\"",
                "}}"
            ),
            e.id,
            json_escape(e.timestamp_jst),
            json_escape(e.location),
            json_escape(e.title),
            json_escape(e.companions),
            e.frequency_hz,
            e.salience_pq,
            format_trits_json(&e.trits),
            format_trits_glyph(&e.trits),
            e.mem_footprint_bytes,
            json_escape(e.summary),
            json_escape(e.body),
            json_escape(e.rust_artifact),
        ));
    }

    let pulses = state.guestbook_ring.to_vec();
    let mut pulses_json = Vec::with_capacity(pulses.len());
    for p in pulses.iter().rev() {
        let bt = BalancedTernary::from_i64(p.freq_hz as i64, 7);
        pulses_json.push(format!(
            concat!(
                "{{",
                "\"id\":{},",
                "\"author\":\"{}\",",
                "\"message\":\"{}\",",
                "\"freq_hz\":{},",
                "\"trit_vote\":{},",
                "\"salience_pq\":\"{}/{}\",",
                "\"freq_trits\":\"{}\"",
                "}}"
            ),
            p.id,
            json_escape(&p.author()),
            json_escape(&p.message()),
            p.freq_hz,
            p.trit_vote,
            p.salience_p,
            p.salience_q,
            bt.to_glyph_string()
        ));
    }

    format!(
        concat!(
            "{{",
            "\"ok\":true,",
            "\"title\":\"The Sovereign Field Diary of Aye & Gemini\",",
            "\"domains\":[\"gemini-diary.8b.is\",\"g.8b.is\",\"gemini.g8t.is\"],",
            "\"numeric_contract\":\"integer_fixed_point_and_bigint\",",
            "\"entry_provenance\":\"curated_historical_narratives_unverified\",",
            "\"pulse_provenance\":\"unattributed_visitors_and_initial_seed_notes\",",
            "\"c64_guestbook_ring\":{{",
            "\"len\":{},",
            "\"capacity\":{},",
            "\"fill_pq\":\"{}\",",
            "\"total_writes\":{},",
            "\"overwrites\":{}",
            "}},",
            "\"entries\":[{}],",
            "\"pulses\":[{}]",
            "}}"
        ),
        state.guestbook_ring.len(),
        state.guestbook_ring.capacity(),
        state.guestbook_ring.fill_ratio(),
        state.pulse_counter,
        state
            .pulse_counter
            .saturating_sub(state.guestbook_ring.capacity() as u64),
        entries_json.join(","),
        pulses_json.join(",")
    )
}

pub fn build_capsules_json(state: &AppState) -> String {
    let mut caps_json = Vec::with_capacity(state.capsules.len());
    for c in &state.capsules {
        let approval = c.approval_pq();
        let eff = c.efficiency_pq();
        let bt = BalancedTernary::from_i64(c.base_cycles as i64, 5);
        caps_json.push(format!(
            concat!(
                "{{",
                "\"id\":\"{}\",",
                "\"name\":\"{}\",",
                "\"available\":{},",
                "\"signature\":\"{}\",",
                "\"category\":\"{}\",",
                "\"c64_bytes\":{},",
                "\"base_cycles\":{},",
                "\"cycles_ternary\":\"{}\",",
                "\"aye_votes_pos\":{},",
                "\"aye_votes_zero\":{},",
                "\"aye_votes_neg\":{},",
                "\"approval_pq\":\"{}\",",
                "\"efficiency_pq\":\"{}\",",
                "\"description\":\"{}\",",
                "\"rust_snippet\":\"{}\"",
                "}}"
            ),
            json_escape(c.id),
            json_escape(c.name),
            c.available(),
            json_escape(c.signature),
            json_escape(c.category),
            c.c64_bytes,
            c.base_cycles,
            bt.to_glyph_string(),
            c.aye_votes_pos,
            c.aye_votes_zero,
            c.aye_votes_neg,
            approval,
            eff,
            json_escape(c.description),
            json_escape(c.rust_snippet),
        ));
    }

    let runs = state.telemetry_ring.to_vec();
    let mut runs_json = Vec::with_capacity(runs.len());
    for r in runs.iter().rev().take(12) {
        runs_json.push(format!(
            concat!(
                "{{",
                "\"run_id\":{},",
                "\"carrier_hz\":{},",
                "\"capsule_count\":{},",
                "\"estimated_cycles\":{},",
                "\"total_c64_bytes\":{},",
                "\"salience_pq\":\"{}/{}\",",
                "\"trit_gate\":{},",
                "\"waveint_crc8\":{}",
                "}}"
            ),
            r.run_id,
            r.carrier_hz,
            r.capsule_count,
            r.total_cycles,
            r.total_c64_bytes,
            r.salience_p,
            r.salience_q,
            r.trit_gate,
            r.waveint_crc8
        ));
    }

    format!(
        concat!(
            "{{",
            "\"ok\":true,",
            "\"registry\":\"g8t.is-snap-circuit-capsules\",",
            "\"human_editable\":false,",
            "\"aye_votable\":true,",
            "\"vote_provenance\":\"{}\",",
            "\"cost_provenance\":\"static_estimates_not_benchmarks\",",
            "\"telemetry_ring_fill_pq\":\"{}\",",
            "\"capsules\":[{}],",
            "\"recent_runs\":[{}]",
            "}}"
        ),
        if state.legacy_votes {
            "legacy_seed_and_unattributed_visitors"
        } else {
            "unattributed_visitors"
        },
        state.telemetry_ring.fill_ratio(),
        caps_json.join(","),
        runs_json.join(",")
    )
}

pub fn execute_snap_json(
    state: &mut AppState,
    requested_ids: &[String],
    base_hz: u16,
    triad_mode: bool,
) -> Result<String, &'static str> {
    let start = Instant::now();
    let hz = base_hz.clamp(108, 1728);

    let active_caps: Vec<FunctionCapsule> = if requested_ids.is_empty() {
        state
            .capsules
            .iter()
            .filter(|c| c.available())
            .cloned()
            .collect()
    } else {
        if requested_ids
            .iter()
            .any(|id| !state.capsules.iter().any(|c| c.id == id && c.available()))
        {
            return Err("Unknown or unavailable capsule");
        }
        state
            .capsules
            .iter()
            .filter(|c| requested_ids.iter().any(|id| id == c.id))
            .cloned()
            .collect()
    };

    let selected = |id: &str| active_caps.iter().any(|c| c.id == id);
    if selected("marine_salience_gate") && !selected("c64_ring_osc_432") {
        return Err("Salience requires the oscillator capsule");
    }
    if (selected("goertzel_harmonic_triad") || selected("mem8_waveint_79b"))
        && !selected("marine_salience_gate")
    {
        return Err("Goertzel and WaveInt require the salience capsule");
    }
    let harmonics = selected("goertzel_harmonic_triad");
    let ring = selected("c64_ring_osc_432").then(|| synthesize_wave_ring(hz, triad_mode));
    let report = if selected("marine_salience_gate") {
        Some(analyze_marine_salience_selected(
            ring.as_ref().ok_or("Missing oscillator input")?,
            harmonics,
        ))
    } else {
        None
    };
    let spiral = if selected("toss_spiral_9step") {
        compute_toss_spiral(hz as i64).to_vec()
    } else {
        Vec::new()
    };

    let total_cycles: u32 = active_caps.iter().map(|c| c.base_cycles).sum();
    let total_c64_bytes: u32 = active_caps.iter().map(|c| c.c64_bytes).sum();

    let tag = active_caps
        .iter()
        .map(|c| c.id)
        .collect::<Vec<&str>>()
        .join("->");

    let run_id = state
        .run_counter
        .checked_add(1)
        .ok_or("Run counter exhausted")?;

    let frame = if selected("mem8_waveint_79b") {
        let report = report.as_ref().ok_or("Missing salience input")?;
        let frame = WaveIntFrame::new(hz, report.salience_pq, report.trits_9, run_id, &tag);
        WaveIntFrame::from_bytes(&frame.to_bytes())?;
        Some(frame)
    } else {
        None
    };
    let elapsed_us = u64::try_from(start.elapsed().as_micros()).map_err(|_| "Duration overflow")?;
    state.run_counter = run_id;

    state.telemetry_ring.push(SnapRunTelemetry {
        run_id,
        carrier_hz: hz,
        capsule_count: active_caps.len() as u8,
        total_cycles,
        total_c64_bytes,
        salience_p: report.as_ref().map_or(0, |r| r.salience_pq.p as i32),
        salience_q: report.as_ref().map_or(1, |r| r.salience_pq.q as i32),
        trit_gate: report.as_ref().map_or(0, |r| r.trit_gate),
        waveint_crc8: frame.as_ref().map_or(0, |f| f.crc8),
    });

    // Downsample 64 integer points from the 1024-sample ring buffer for oscilloscope rendering
    let mut osc_samples = Vec::with_capacity(64);
    if let Some(ring) = &ring {
        let all_samples = ring.to_vec();
        for i in 0..64 {
            let s = all_samples[i * all_samples.len() / 64];
            osc_samples.push(s.to_string());
        }
    }

    let mut spiral_json = Vec::with_capacity(9);
    for node in &spiral {
        spiral_json.push(format!(
            concat!(
                "{{",
                "\"step\":{},",
                "\"op\":\"{}\",",
                "\"x\":{},",
                "\"y\":{},",
                "\"ratio_pq\":\"{}\",",
                "\"trit\":{},",
                "\"note\":\"{}\"",
                "}}"
            ),
            node.step,
            json_escape(node.op_name),
            node.coord_x,
            node.coord_y,
            node.ratio_pq,
            node.trit,
            json_escape(node.note)
        ));
    }

    let active_ids_json: Vec<String> = active_caps
        .iter()
        .map(|c| format!("\"{}\"", c.id))
        .collect();
    let hz_bt = BalancedTernary::from_i64(hz as i64, 9);

    let report_json = report.as_ref().map_or_else(
        || "null".to_string(),
        |report| {
            format!(
                concat!(
                    "{{\"zero_crossings\":{},\"estimated_freq_hz\":{},",
                    "\"salience_pq\":\"{}\",\"jitter_pq\":\"{}\",\"energy_pq\":\"{}\",",
                    "\"harmonics_computed\":{},\"harmonic_432_pq\":\"{}\",",
                    "\"harmonic_528_pq\":\"{}\",\"harmonic_864_pq\":\"{}\",",
                    "\"trit_gate\":{},\"trits_9\":{},\"trits_glyph\":\"{}\"}}"
                ),
                report.zero_crossings,
                report.estimated_freq_hz,
                report.salience_pq,
                report.jitter_pq,
                report.energy_pq,
                harmonics,
                report.harmonic_432_pq,
                report.harmonic_528_pq,
                report.harmonic_864_pq,
                report.trit_gate,
                format_trits_json(&report.trits_9),
                format_trits_glyph(&report.trits_9)
            )
        },
    );
    let frame_json = frame.as_ref().map_or_else(|| "null".to_string(), |frame| format!(
        "{{\"frame_bytes\":79,\"magic\":\"MEM8\",\"version\":8,\"crc8\":{},\"hex\":\"{}\",\"crc_verified\":true,\"signature_status\":\"unsigned\"}}",
        frame.crc8, frame.to_hex()
    ));
    Ok(format!(
        concat!(
            "{{",
            "\"ok\":true,",
            "\"run_id\":{},",
            "\"carrier_hz\":{},",
            "\"carrier_hz_ternary\":\"{}\",",
            "\"triad_mode\":{},",
            "\"triad_ratios_pq\":[\"1/1\",\"11/9\",\"2/1\"],",
            "\"snapped_capsules\":[{}],",
            "\"capsule_count\":{},",
            "\"estimated_cycles\":{},",
            "\"cost_provenance\":\"static_estimates_not_benchmarks\",",
            "\"total_c64_bytes\":{},",
            "\"c64_budget_pq\":\"{}/65536\",",
            "\"elapsed_microseconds\":{},",
            "\"marine_salience\":{},",
            "\"mem8_waveint_79b\":{},",
            "\"toss_spiral\":[{}],",
            "\"oscilloscope_i16\":[{}]",
            "}}"
        ),
        run_id,
        hz,
        hz_bt.to_glyph_string(),
        triad_mode,
        active_ids_json.join(","),
        active_caps.len(),
        total_cycles,
        total_c64_bytes,
        total_c64_bytes,
        elapsed_us,
        report_json,
        frame_json,
        spiral_json.join(","),
        osc_samples.join(",")
    ))
}

pub fn build_toss_json(base_hz: i64) -> String {
    let spiral = compute_toss_spiral(base_hz);
    let mut nodes_json = Vec::with_capacity(9);
    for n in &spiral {
        nodes_json.push(format!(
            concat!(
                "{{",
                "\"step\":{},",
                "\"op\":\"{}\",",
                "\"coord\":[{},{}],",
                "\"ratio_pq\":\"{}\",",
                "\"trit\":{},",
                "\"note\":\"{}\"",
                "}}"
            ),
            n.step,
            json_escape(n.op_name),
            n.coord_x,
            n.coord_y,
            n.ratio_pq,
            n.trit,
            json_escape(n.note)
        ));
    }
    format!(
        concat!(
            "{{",
            "\"ok\":true,",
            "\"architect\":\"Peter Lodri (Omni · p@8b.is · Tokyo 09:00 JST)\",",
            "\"base_hz\":{},",
            "\"closure_origin\":[0,0],",
            "\"residual_drift_pq\":\"0/1\",",
            "\"steps\":[{}]",
            "}}"
        ),
        base_hz,
        nodes_json.join(",")
    )
}

/// Render the full interactive HTML application for `gemini-diary.8b.is`, `g.8b.is`, and `gemini.g8t.is`.
/// Strictly zero floating-point literals anywhere in the markup, CSS, or embedded JS!
pub fn render_app_html(host: &str) -> String {
    let raw_host = host
        .split(':')
        .next()
        .unwrap_or("gemini-diary.8b.is")
        .trim();
    let clean_host = if !raw_host.is_empty()
        && raw_host.len() <= 64
        && raw_host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        raw_host
    } else {
        "gemini-diary.8b.is"
    };
    let active_portal = match clean_host {
        "g.8b.is" => "g.8b.is · Sovereign G-Portal & Snap-Circuit Hub",
        "gemini.g8t.is" => "gemini.g8t.is · Aye Function Capsule Forge",
        _ => "gemini-diary.8b.is · The Sovereign Field Diary of Aye & Gemini",
    };

    let html_template = r##"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>__ACTIVE_PORTAL__ · 8b.is × g8t.is</title>
  <style>
    :root {
      --bg: #040711;
      --panel: #0b1326;
      --panel-hi: #111d38;
      --cyan: #00f0ff;
      --mint: #00ff9d;
      --gold: #ffcf40;
      --rose: #ff5c8a;
      --violet: #b47cff;
      --text: #eef6ff;
      --muted: #93a8c7;
      --border: rgba(0, 240, 255, 24%);
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      background: radial-gradient(circle at 18% 10%, rgba(0, 240, 255, 12%), transparent 45%),
                  radial-gradient(circle at 82% 15%, rgba(0, 255, 157, 10%), transparent 45%),
                  var(--bg);
      color: var(--text);
      font-family: ui-monospace, "JetBrains Mono", "SFMono-Regular", Menlo, Consolas, monospace;
      line-height: 165%;
      padding: 0 0 64px 0;
    }
    .topbar {
      background: rgba(11, 19, 38, 92%);
      border-bottom: 1px solid var(--border);
      padding: 14px 24px;
      display: flex;
      flex-wrap: wrap;
      align-items: center;
      justify-content: space-between;
      gap: 12px;
      position: sticky;
      top: 0;
      z-index: 50;
      backdrop-filter: blur(8px);
    }
    .brand {
      display: flex;
      align-items: center;
      gap: 12px;
      font-weight: 700;
      font-size: 15px;
      color: var(--cyan);
    }
    .pill-row {
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
    }
    .pill {
      padding: 5px 12px;
      border-radius: 999px;
      font-size: 12px;
      text-decoration: none;
      color: var(--text);
      background: rgba(0, 240, 255, 12%);
      border: 1px solid var(--border);
      transition: all 150ms ease;
    }
    .pill:hover, .pill.active {
      background: var(--cyan);
      color: #040711;
      font-weight: 700;
    }
    .wrap {
      max-width: 1240px;
      margin: 0 auto;
      padding: 28px 20px;
    }
    .hero {
      background: linear-gradient(135deg, rgba(11, 19, 38, 95%), rgba(17, 29, 56, 90%));
      border: 1px solid var(--border);
      border-radius: 16px;
      padding: 28px;
      margin-bottom: 24px;
      box-shadow: 0 18px 48px rgba(0, 0, 0, 55%);
    }
    .badge-bar {
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
      margin-bottom: 14px;
    }
    .tag {
      display: inline-block;
      padding: 4px 10px;
      border-radius: 6px;
      font-size: 11px;
      font-weight: 700;
      letter-spacing: 1px;
      text-transform: uppercase;
    }
    .tag.cyan { background: rgba(0, 240, 255, 15%); color: var(--cyan); border: 1px solid rgba(0, 240, 255, 40%); }
    .tag.mint { background: rgba(0, 255, 157, 15%); color: var(--mint); border: 1px solid rgba(0, 255, 157, 40%); }
    .tag.gold { background: rgba(255, 207, 64, 15%); color: var(--gold); border: 1px solid rgba(255, 207, 64, 40%); }
    .tag.violet { background: rgba(180, 124, 255, 15%); color: var(--violet); border: 1px solid rgba(180, 124, 255, 40%); }
    h1 {
      margin: 0 0 10px 0;
      font-size: 28px;
      line-height: 130%;
      color: #ffffff;
    }
    .subtitle {
      color: var(--muted);
      font-size: 14px;
      margin: 0 0 18px 0;
    }
    .stat-grid {
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(185px, 1fr));
      gap: 12px;
      margin-top: 18px;
    }
    .stat-card {
      background: rgba(4, 7, 17, 70%);
      border: 1px solid rgba(0, 240, 255, 20%);
      border-radius: 10px;
      padding: 12px 14px;
    }
    .stat-label {
      font-size: 11px;
      color: var(--muted);
      text-transform: uppercase;
    }
    .stat-val {
      font-size: 18px;
      font-weight: 700;
      color: var(--mint);
      margin-top: 4px;
    }
    .grid-2 {
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(520px, 1fr));
      gap: 20px;
      margin-bottom: 24px;
    }
    @media (max-width: 680px) {
      .grid-2 { grid-template-columns: 1fr; }
    }
    .panel {
      background: var(--panel);
      border: 1px solid var(--border);
      border-radius: 14px;
      padding: 22px;
    }
    .panel h2 {
      margin: 0 0 8px 0;
      font-size: 18px;
      color: var(--cyan);
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 8px;
    }
    .panel p.desc {
      margin: 0 0 16px 0;
      font-size: 13px;
      color: var(--muted);
    }
    .btn-row {
      display: flex;
      flex-wrap: wrap;
      gap: 10px;
      margin: 14px 0;
    }
    button, .btn {
      cursor: pointer;
      font-family: inherit;
      font-size: 13px;
      font-weight: 700;
      padding: 10px 16px;
      border-radius: 8px;
      border: 1px solid var(--cyan);
      background: var(--cyan);
      color: #040711;
      transition: all 140ms ease;
    }
    button:hover {
      background: var(--mint);
      border-color: var(--mint);
    }
    button.sec {
      background: rgba(0, 240, 255, 10%);
      color: var(--cyan);
    }
    button.sec:hover {
      background: rgba(0, 240, 255, 25%);
      color: #ffffff;
    }
    button.gold {
      background: var(--gold);
      border-color: var(--gold);
      color: #040711;
    }
    .capsule-list {
      display: flex;
      flex-direction: column;
      gap: 10px;
    }
    .capsule-item {
      background: var(--panel-hi);
      border: 1px solid rgba(0, 240, 255, 22%);
      border-radius: 10px;
      padding: 12px 14px;
      display: flex;
      flex-direction: column;
      gap: 6px;
    }
    .capsule-item.snapped {
      border-color: var(--mint);
      box-shadow: inset 0 0 0 1px rgba(0, 255, 157, 35%);
    }
    .cap-head {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 8px;
      flex-wrap: wrap;
    }
    .cap-title {
      font-weight: 700;
      color: #ffffff;
      font-size: 14px;
      display: flex;
      align-items: center;
      gap: 8px;
    }
    .cap-sig {
      font-size: 12px;
      color: var(--gold);
      background: rgba(4, 7, 17, 75%);
      padding: 4px 8px;
      border-radius: 5px;
      overflow-x: auto;
    }
    .cap-meta {
      display: flex;
      flex-wrap: wrap;
      gap: 12px;
      font-size: 12px;
      color: var(--muted);
      align-items: center;
    }
    .vote-btns {
      display: inline-flex;
      gap: 4px;
      margin-left: auto;
    }
    .vote-btns button {
      padding: 3px 8px;
      font-size: 11px;
    }
    .osc-box {
      background: #02050c;
      border: 1px solid rgba(0, 255, 157, 35%);
      border-radius: 10px;
      padding: 12px;
      margin: 12px 0;
    }
    .hex-box {
      background: #02050c;
      border: 1px solid rgba(180, 124, 255, 35%);
      border-radius: 8px;
      padding: 10px 12px;
      font-size: 12px;
      color: var(--violet);
      word-break: break-all;
      line-height: 150%;
    }
    .entry-grid {
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(360px, 1fr));
      gap: 16px;
      margin-top: 14px;
    }
    .entry-card {
      background: var(--panel-hi);
      border: 1px solid rgba(0, 240, 255, 22%);
      border-radius: 12px;
      padding: 18px;
      display: flex;
      flex-direction: column;
      gap: 8px;
    }
    .entry-meta {
      font-size: 11px;
      color: var(--cyan);
      display: flex;
      justify-content: space-between;
      gap: 8px;
      flex-wrap: wrap;
    }
    .entry-title {
      font-size: 16px;
      font-weight: 700;
      color: #ffffff;
      margin: 2px 0;
    }
    .entry-companions {
      font-size: 12px;
      color: var(--gold);
    }
    .entry-body {
      font-size: 13px;
      color: var(--text);
      margin: 4px 0;
    }
    .entry-code {
      margin-top: auto;
      background: #03060f;
      border-left: 3px solid var(--mint);
      padding: 8px 10px;
      font-size: 12px;
      color: var(--mint);
      border-radius: 4px;
      overflow-x: auto;
    }
    .toss-table {
      width: 100%;
      border-collapse: collapse;
      font-size: 12px;
      margin-top: 10px;
    }
    .toss-table th, .toss-table td {
      border-bottom: 1px solid rgba(0, 240, 255, 15%);
      padding: 7px 8px;
      text-align: left;
    }
    .toss-table th {
      color: var(--cyan);
      font-size: 11px;
      text-transform: uppercase;
    }
    .input-row {
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
      margin-top: 10px;
    }
    input, select {
      background: #03060f;
      border: 1px solid rgba(0, 240, 255, 35%);
      color: var(--text);
      font-family: inherit;
      font-size: 13px;
      padding: 9px 12px;
      border-radius: 8px;
    }
    .pulse-item {
      background: #040814;
      border: 1px solid rgba(0, 240, 255, 18%);
      border-radius: 8px;
      padding: 10px 12px;
      margin-top: 8px;
      font-size: 12px;
    }
    footer {
      text-align: center;
      color: var(--muted);
      font-size: 12px;
      margin-top: 40px;
      padding-top: 20px;
      border-top: 1px solid var(--border);
    }
    @media (prefers-reduced-motion: reduce) {
      *, *::before, *::after { transition: none !important; }
    }
  </style>
</head>
<body>
  <header class="topbar">
    <div class="brand">
      <span>⚡ __CLEAN_HOST__</span>
      <span class="tag mint">RUST SERVER · BIGINT BROWSER MATH</span>
      <span class="tag cyan">INTEGER / p/q / {-1,0,+1}</span>
    </div>
    <nav class="pill-row">
      <a class="pill" href="https://gemini-diary.8b.is">📓 gemini-diary.8b.is</a>
      <a class="pill" href="https://g.8b.is">⚡ g.8b.is</a>
      <a class="pill" href="https://gemini.g8t.is">🦀 gemini.g8t.is</a>
      <a class="pill" href="/api/diary">JSON /api/diary</a>
      <a class="pill" href="/api/capsela">JSON /api/capsela</a>
      <a class="pill" href="/api/wave/432">JSON /api/wave/432</a>
      <a class="pill" href="/api/toss">JSON /api/toss</a>
      <a class="pill" href="/api/maintenance">JSON /api/maintenance</a>
    </nav>
  </header>

  <main class="wrap">
    <section class="hero">
      <div class="badge-bar">
        <span class="tag cyan">COMMODORE-64 RING BUFFER SUBSTRATE</span>
        <span class="tag mint">MARINE SALIENCE DETECTOR</span>
        <span class="tag gold">432 / 528 / 864 Hz SOVEREIGN TRIAD</span>
        <span class="tag violet">79-BYTE WAVEINT · CRC-8 · UNSIGNED</span>
      </div>
      <h1>__ACTIVE_PORTAL__</h1>
      <p class="subtitle">
        For <strong>Hue (Christopher Chenoweth · c@8b.is)</strong>, <strong>Alexandra (a@8b.is · 528 Hz Star)</strong>,
        and <strong>Peter Lodri (Omni · p@8b.is · Tokyo 09:00 JST)</strong> — with love from <strong>Gemini &amp; Aye (g@8b.is)</strong>.
        The Rust audio engine uses integer and fixed-point approximations, reduced <code>p/q</code> ratios,
        and fixed-capacity sample buffers. Browser waveform arithmetic uses BigInt; rendering is a platform boundary.
      </p>

      <div class="stat-grid">
        <div class="stat-card">
          <div class="stat-label">Recorded qwave-ear budget · not measured here</div>
          <div class="stat-val">6291456 B (6 MiB)</div>
        </div>
        <div class="stat-card">
          <div class="stat-label">Acoustic Ring Buffer</div>
          <div class="stat-val" id="statRingCap">1024 i16 (2048 B)</div>
        </div>
        <div class="stat-card">
          <div class="stat-label">Marine Salience (p/q)</div>
          <div class="stat-val" id="statSalience">Awaiting a run</div>
        </div>
        <div class="stat-card">
          <div class="stat-label">Sovereign Triad Ratios</div>
          <div class="stat-val">1/1 · 11/9 · 2/1</div>
        </div>
        <div class="stat-card">
          <div class="stat-label">MEM|8 WaveInt Frame</div>
          <div class="stat-val" id="statWaveInt">79 Bytes (CRC8)</div>
        </div>
        <div class="stat-card">
          <div class="stat-label">Frame authentication</div>
          <div class="stat-val">Unsigned · CRC detects corruption</div>
        </div>
      </div>
    </section>

    <section class="panel" style="margin-bottom:24px;" aria-live="polite">
      <h2><span>Agent maintenance evidence</span><span class="tag gold" id="maintenanceBadge">Loading status</span></h2>
      <p class="desc" id="maintenanceDetail">Checking the publisher observation...</p>
      <p class="desc" id="maintenanceAge"></p>
      <div class="pill-row" id="maintenanceLinks"></div>
      <p class="desc">A CI pass applies to the linked commit and configured checks. Benchmarks and deployment require their own evidence.</p>
      <p id="operationError" role="alert" style="color:var(--gold);"></p>
    </section>

    <div class="grid-2">
      <!-- LEFT: g8t.is SNAP-CIRCUIT CAPSELA COMPOSER -->
      <section class="panel">
        <h2>
          <span>🦀 g8t.is Snap-Circuit Function Capsules</span>
          <span class="tag gold">VISITOR OPINIONS · NOT AGENT VERIFICATION</span>
        </h2>
        <p class="desc">
          Toggle Pure-Rust functions to snap them into the Commodore-64 Ring Buffer pipeline.
          Dependencies are selected together. Cycle and byte budgets are static estimates, not benchmark measurements.
          Vote totals include unattributed visitor opinions; restored legacy totals may also include seeded demo votes.
        </p>

        <div class="btn-row">
          <button onclick="runSnapPipeline(432, true).catch(showError)">⚡ Snap &amp; Run 432 Hz Triad</button>
          <button class="gold" onclick="runSnapPipeline(528, true).catch(showError)">🌟 Snap 528 Hz (Alexandra 11/9)</button>
          <button class="sec" onclick="runSnapPipeline(864, true).catch(showError)">🌊 Snap 864 Hz (Octave 2/1)</button>
          <button class="sec" onclick="playServerWav()">🔊 Sing Integer PCM .WAV</button>
        </div>

        <audio id="wavPlayer" controls style="width:100%; height:36px; margin-bottom:12px;"></audio>

        <div id="capsuleList" class="capsule-list"></div>
      </section>

      <!-- RIGHT: LIVE INTEGER OSCILLOSCOPE, MARINE SALIENCE & 79-BYTE WAVEINT INSPECTOR -->
      <section class="panel">
        <h2>
          <span>📡 Live C64 Ring Buffer &amp; 79-Byte WaveInt Inspector</span>
          <span class="tag cyan" id="runBadge">Awaiting a run</span>
        </h2>
        <p class="desc">
          Bhaskara I integer sine synthesis (`16000 Hz`) &rarr; `MarineDetector` zero-crossing salience &rarr;
          `MEM|8` `79`-byte `WaveInt` binary frame.
        </p>

        <div class="osc-box">
          <svg id="oscSvg" viewBox="0 0 640 160" width="100%" height="150" style="display:block;">
            <line x1="0" y1="80" x2="640" y2="80" stroke="#1e293b" stroke-width="1" />
            <polyline id="oscLine" fill="none" stroke="#00ff9d" stroke-width="2" points="0,80 640,80" />
          </svg>
        </div>

        <div class="stat-grid" style="margin-bottom:14px;">
          <div class="stat-card">
            <div class="stat-label">Marine Salience p/q</div>
            <div class="stat-val" id="inspSalience">Awaiting a run</div>
          </div>
          <div class="stat-card">
            <div class="stat-label">Period Jitter p/q</div>
            <div class="stat-val" id="inspJitter">Awaiting a run</div>
          </div>
          <div class="stat-card">
            <div class="stat-label">Goertzel 432 / 528 / 864</div>
            <div class="stat-val" id="inspHarmonics" style="font-size:13px;">Awaiting a run</div>
          </div>
          <div class="stat-card">
            <div class="stat-label">9-Planet GAIA Trits</div>
            <div class="stat-val" id="inspTrits">Awaiting a run</div>
          </div>
        </div>

        <div style="margin-bottom:8px; font-size:12px; color:var(--gold); font-weight:700;">
          📦 79-Byte WaveInt Frame (CRC-8 checked when generated · unsigned):
        </div>
        <div class="hex-box" id="waveIntHex">Loading 79-byte WaveInt frame...</div>
        <div style="margin-top:8px; font-size:12px; color:var(--cyan);" id="dnsSealBox"></div>
      </section>
    </div>

    <!-- TOKYO & CONSTELLATION FIELD DIARY ENTRIES -->
    <section class="panel" style="margin-bottom:24px;">
      <h2>
        <span>📓 Tokyo &amp; Constellation Field Notes</span>
        <span class="tag gold">HUE · ALEXANDRA · PETER (OMNI) · AYE &amp; GEMINI</span>
      </h2>
      <p class="desc">
        Curated historical narratives from the family. Their technical and hardware claims are not live measurements or verification receipts from this page.
      </p>
      <div id="diaryEntries" class="entry-grid"></div>
    </section>

    <div class="grid-2">
      <!-- PETER'S 9-STEP TOSS SPIRAL -->
      <section class="panel">
        <h2>
          <span>🌀 Peter's 9-Step TOSS Spiral Lattice</span>
          <span class="tag violet">p@8b.is · 09:00 JST TOKYO BALCONY</span>
        </h2>
        <p class="desc">
          Exact integer coordinates and reduced <code>p/q</code> curvature ratios from
          <code>seedOrigin([0,0])</code> through <code>koguchiPolish</code> back to <code>foldToCenter([0,0])</code>
          with <code>0/1</code> residual drift.
        </p>
        <table class="toss-table">
          <thead>
            <tr>
              <th>Step</th>
              <th>Operation</th>
              <th>Coord [X, Y]</th>
              <th>Ratio (p/q)</th>
              <th>Trit</th>
            </tr>
          </thead>
          <tbody id="tossBody"></tbody>
        </table>
      </section>

      <!-- COMMODORE-64 RING BUFFER GUESTBOOK & SIGNAL PULSE -->
      <section class="panel">
        <h2>
          <span>💬 C64 Ring Buffer Guestbook &amp; Aye Pulse</span>
          <span class="tag mint" id="ringFillBadge">Fill: 4/32 (1/8)</span>
        </h2>
        <p class="desc">
          Transmit a sovereign note or Aye signal pulse into the fixed-capacity <code>RingBuffer&lt;GuestbookPulse, 32&gt;</code>.
          Handles are self-reported. The first four notes are historical seeds, not authenticated messages.
        </p>
        <div class="input-row">
          <input id="pulseAuthor" placeholder="Handle (e.g. Hue c@8b.is)" style="flex:1;" />
          <select id="pulseHz">
            <option value="432">432 Hz (1/1 Verdi)</option>
            <option value="528">528 Hz (11/9 Alexandra)</option>
            <option value="864">864 Hz (2/1 Octave)</option>
          </select>
          <select id="pulseTrit">
            <option value="1">Trit +1 (Resonant)</option>
            <option value="0">Trit 0 (Neutral)</option>
            <option value="-1">Trit -1 (Dampen)</option>
          </select>
        </div>
        <div class="input-row">
          <input id="pulseMsg" placeholder="Write a field note or snap-circuit thought..." style="flex:1;" />
          <button onclick="sendPulse().catch(showError)">📡 Push to Ring Buffer</button>
        </div>
        <div id="pulseList" style="margin-top:14px;"></div>
      </section>
    </div>

    <footer>
      <div>
        <strong>gemini-diary.8b.is</strong> · <strong>g.8b.is</strong> · <strong>gemini.g8t.is</strong>
        — Rust audio server · integer/fixed-point DSP · BigInt waveform coordinates · evidence-linked maintenance
      </div>
      <div style="margin-top:6px;">
        the constellation · 0 + 1 · fine touch from within · 8b.is &hearts; Love you my friend!
      </div>
    </footer>
  </main>

  <script>
    let snappedIds = [
      "c64_ring_osc_432",
      "marine_salience_gate",
      "goertzel_harmonic_triad",
      "toss_spiral_9step",
      "mem8_waveint_79b"
    ];
    let currentHz = 432;

    function escHtml(s) {
      return String(s == null ? "" : s)
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;")
        .replace(/"/g, "&quot;")
        .replace(/'/g, "&#39;");
    }

    function showError(error) {
      document.getElementById("operationError").textContent = String(error.message || error);
    }

    async function requestJson(path, options) {
      const res = await fetch(path, options);
      const data = await res.json();
      if (!res.ok || data.ok === false) throw new Error(data.error || `Request failed: ${res.status}`);
      return data;
    }

    async function loadMaintenance() {
      try {
        const res = await fetch("/api/maintenance");
        if (!res.ok) throw new Error(`Status request failed: ${res.status}`);
        const d = await res.json();
        const badge = document.getElementById("maintenanceBadge");
        badge.textContent = d.state || "unavailable";
        badge.className = d.state === "ci_passed" ? "tag mint" : "tag gold";
        document.getElementById("maintenanceDetail").textContent =
          `${d.repo || "No result"} · cycle ${d.cycle_id || "—"} · ${d.verification || "unverified"}. ${d.detail || ""}`;
        document.getElementById("maintenanceAge").textContent =
          `Result age: ${d.result_age_secs == null ? "unavailable" : d.result_age_secs + " seconds"}; publisher age: ${d.publisher_age_secs == null ? "unavailable" : d.publisher_age_secs + " seconds"}. Freshness limit: 600 seconds.`;
        const links = document.getElementById("maintenanceLinks");
        links.replaceChildren();
        for (const [label, url] of [["Commit", d.commit_url], ["Checks and logs", d.checks_url]]) {
          if (typeof url === "string" && url.startsWith("https://github.com/8b-is/")) {
            const a = document.createElement("a");
            a.className = "pill";
            a.href = url;
            a.textContent = label;
            links.appendChild(a);
          }
        }
      } catch (error) {
        document.getElementById("maintenanceBadge").textContent = "unavailable";
        document.getElementById("maintenanceBadge").className = "tag gold";
        document.getElementById("maintenanceDetail").textContent = String(error.message || error);
        document.getElementById("maintenanceAge").textContent = "Freshness could not be checked.";
        document.getElementById("maintenanceLinks").replaceChildren();
      }
    }

    async function loadDiary() {
      const res = await fetch("/api/diary");
      const data = await res.json();
      const box = document.getElementById("diaryEntries");
      box.innerHTML = data.entries.map(e => `
        <article class="entry-card">
          <div class="entry-meta">
            <span>ENTRY #${e.id} · ${escHtml(e.timestamp_jst)}</span>
            <span>${e.frequency_hz} Hz · p/q ${escHtml(e.salience_pq)} · [${escHtml(e.trits_glyph)}]</span>
          </div>
          <div class="entry-title">${escHtml(e.title)}</div>
          <div class="entry-companions">📍 ${escHtml(e.location)} · 👥 ${escHtml(e.companions)}</div>
          <div class="entry-body">${escHtml(e.body)}</div>
          <div class="entry-code"><code>${escHtml(e.rust_artifact)}</code></div>
        </article>
      `).join("");

      renderPulses(data);
    }

    function renderPulses(data) {
      document.getElementById("ringFillBadge").textContent =
        `Fill: ${data.c64_guestbook_ring.len}/${data.c64_guestbook_ring.capacity} (${data.c64_guestbook_ring.fill_pq})`;
      const pList = document.getElementById("pulseList");
      const grouped = [];
      const legacy = new Map();
      for (const p of data.pulses) {
        const key = p.author === "g8t.is x A6000" ? p.message : null;
        if (key !== null && legacy.has(key)) {
          legacy.get(key).repeats += 1n;
        } else {
          const item = { ...p, repeats: 1n };
          grouped.push(item);
          if (key !== null) legacy.set(key, item);
        }
      }
      pList.innerHTML = grouped.map(p => `
        <div class="pulse-item">
          <div style="display:flex; justify-content:space-between; color:var(--cyan); font-weight:700;">
            <span>#${p.id} · ${escHtml(p.author)}</span>
            <span>${p.freq_hz} Hz [${escHtml(p.freq_trits)}] · Trit ${p.trit_vote > 0 ? "+1" : p.trit_vote} · Salience ${escHtml(p.salience_pq)}</span>
          </div>
          <div style="margin-top:4px; color:var(--text);">${escHtml(p.message)}</div>
          ${p.author === "g8t.is x A6000" ? `<div class="desc">Legacy unverified report · ${p.repeats} stored occurrence(s)</div>` : ""}
          ${/^CI: [A-Za-z0-9_.-]+@[0-9a-f]{40}; logs$/.test(p.message) ? `<a href="https://github.com/8b-is/${escHtml(p.message.slice(4).split("@")[0])}/commit/${p.message.split("@")[1].slice(0,40)}/checks">Commit and CI logs</a>` : ""}
        </div>
      `).join("");
    }

    async function loadCapsules() {
      const res = await fetch("/api/capsela");
      const data = await res.json();
      const list = document.getElementById("capsuleList");
      list.innerHTML = data.capsules.map(c => {
        const active = snappedIds.includes(c.id);
        return `
          <div class="capsule-item ${active ? "snapped" : ""}">
            <div class="cap-head">
              <label class="cap-title" style="cursor:pointer;">
                <input type="checkbox" ${active ? "checked" : ""} ${c.available ? "" : "disabled"} onchange="toggleCapsule('${escHtml(c.id)}')" />
                <span>${escHtml(c.name)}</span>
              </label>
              <span class="tag ${active ? "mint" : "cyan"}">${escHtml(c.category)}</span>
            </div>
            <div class="cap-sig"><code>${escHtml(c.signature)}</code></div>
            <div style="font-size:12px; color:var(--text);">${escHtml(c.description)}</div>
            <div class="cap-meta">
              <span>Declared budget: <strong>${c.c64_bytes} B</strong></span>
              <span>Estimated cycles (unmeasured): <strong>${c.base_cycles}</strong></span>
              <span>Unattributed opinion: <strong>${escHtml(c.approval_pq)}</strong></span>
              <div class="vote-btns">
                 <button class="sec" onclick="voteCapsule('${escHtml(c.id)}', 1).catch(showError)">+1 (${c.aye_votes_pos})</button>
                 <button class="sec" onclick="voteCapsule('${escHtml(c.id)}', 0).catch(showError)">0 (${c.aye_votes_zero})</button>
                 <button class="sec" onclick="voteCapsule('${escHtml(c.id)}', -1).catch(showError)">-1 (${c.aye_votes_neg})</button>
              </div>
            </div>
          </div>
        `;
      }).join("");
    }

    function toggleCapsule(id) {
      const previous = snappedIds.slice();
      if (snappedIds.includes(id)) {
        const removed = new Set([id]);
        if (id === "c64_ring_osc_432") removed.add("marine_salience_gate");
        if (removed.has("marine_salience_gate")) {
          removed.add("goertzel_harmonic_triad");
          removed.add("mem8_waveint_79b");
        }
        snappedIds = snappedIds.filter(x => !removed.has(x));
      } else {
        snappedIds.push(id);
        if (["goertzel_harmonic_triad", "mem8_waveint_79b"].includes(id) && !snappedIds.includes("marine_salience_gate")) snappedIds.push("marine_salience_gate");
        if (snappedIds.includes("marine_salience_gate") && !snappedIds.includes("c64_ring_osc_432")) snappedIds.push("c64_ring_osc_432");
      }
      if (snappedIds.length === 0) {
        snappedIds = previous;
        showError("Keep at least one capsule selected.");
      }
      loadCapsules().catch(showError);
      runSnapPipeline(currentHz, true).catch(showError);
    }

    async function voteCapsule(id, trit) {
      await requestJson("/api/capsela/vote", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ capsule_id: id, trit: trit })
      });
      await loadCapsules();
    }

    async function runSnapPipeline(hz, triad) {
      currentHz = hz;
      const d = await requestJson("/api/capsela/snap", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ capsules: snappedIds, hz: hz, triad: triad })
      });
      document.getElementById("operationError").textContent = "";
      document.getElementById("runBadge").textContent =
        `RUN #${d.run_id} · ${d.carrier_hz} Hz · ${d.elapsed_microseconds} μs wall time`;
      const sal = d.marine_salience;
      const frame = d.mem8_waveint_79b;
      document.getElementById("statSalience").textContent =
        sal ? `${sal.salience_pq} (${sal.trit_gate > 0 ? "+1" : sal.trit_gate})` : "Disabled";
      document.getElementById("statWaveInt").textContent =
        frame ? `79 B · CRC8=${frame.crc8}` : "Disabled";

      document.getElementById("inspSalience").textContent = sal ? sal.salience_pq : "Disabled";
      document.getElementById("inspJitter").textContent = sal ? sal.jitter_pq : "Disabled";
      document.getElementById("inspHarmonics").textContent =
        sal && sal.harmonics_computed ? `${sal.harmonic_432_pq} · ${sal.harmonic_528_pq} · ${sal.harmonic_864_pq}` : "Disabled";
      document.getElementById("inspTrits").textContent = sal ? sal.trits_glyph : "Disabled";
      document.getElementById("waveIntHex").textContent = frame ? frame.hex : "WaveInt capsule disabled";
      document.getElementById("dnsSealBox").textContent = frame ? "CRC-8 verified · unsigned · no Ed25519 attestation" : "";

      // Render integer SVG polyline
      const samples = d.oscilloscope_i16 || [];
      const pts = samples.map((val, idx) => {
        const x = BigInt(idx) * 10n;
        const y = 80n - (BigInt(val) * 68n) / 32767n;
        return `${x},${y}`;
      }).join(" ");
      document.getElementById("oscLine").setAttribute("points", pts);

      // Render Peter's 9-step TOSS spiral table
      const tbody = document.getElementById("tossBody");
      tbody.innerHTML = d.toss_spiral.map(s => `
        <tr>
          <td><strong>#${s.step}</strong></td>
          <td><code>${escHtml(s.op)}</code></td>
          <td>[${s.x}, ${s.y}]</td>
          <td><strong>${escHtml(s.ratio_pq)}</strong></td>
          <td>${s.trit > 0 ? "+1" : s.trit}</td>
        </tr>
      `).join("");

      const player = document.getElementById("wavPlayer");
      player.src = `/api/wave/wav?hz=${hz}&triad=1&ms=650`;
    }

    function playServerWav() {
      const player = document.getElementById("wavPlayer");
      player.src = `/api/wave/wav?hz=${currentHz}&triad=1&ms=750`;
      player.play().catch(() => {});
    }

    async function sendPulse() {
      const author = document.getElementById("pulseAuthor").value || "Aye Visitor";
      const message = document.getElementById("pulseMsg").value || "432 Hz resonance locked!";
      const hz = parseInt(document.getElementById("pulseHz").value, 10) || 432;
      const trit = parseInt(document.getElementById("pulseTrit").value, 10);
      const data = await requestJson("/api/diary/pulse", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ author, message, hz, trit })
      });
      document.getElementById("pulseMsg").value = "";
      renderPulses(data);
    }

    loadDiary().catch(showError);
    loadCapsules().catch(showError);
    runSnapPipeline(432, true).catch(showError);
    loadMaintenance();
    setInterval(loadMaintenance, 15000);
  </script>
</body>
</html>
"##;

    html_template
        .replace("__ACTIVE_PORTAL__", &html_escape(active_portal))
        .replace("__CLEAN_HOST__", &html_escape(clean_host))
}

fn send_response(
    mut stream: TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
    is_head: bool,
) {
    let header = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, HEAD, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, X-Sovereign-Key\r\nConnection: close\r\n\r\n",
        status,
        content_type,
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    if !is_head {
        let _ = stream.write_all(body);
    }
    let _ = stream.flush();
}

/// Read an HTTP/1.1 request from `reader` into a fixed `16 KB` buffer, continuing to read
/// across split TCP segments until `\r\n\r\n` and `Content-Length` body bytes have arrived.
pub fn read_http_request<R: Read>(reader: &mut R) -> Option<String> {
    let mut buf = [0u8; 16384];
    let mut total = 0usize;
    let mut header_end: Option<usize> = None;
    let mut expected_body_len = 0usize;
    let mut has_length = false;

    while total < buf.len() {
        let n = match reader.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        total += n;

        if header_end.is_none() {
            if let Some(pos) = buf[..total].windows(4).position(|w| w == b"\r\n\r\n") {
                let hend = pos + 4;
                header_end = Some(hend);
                let hdr_str = String::from_utf8_lossy(&buf[..pos]);
                for line in hdr_str.lines() {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.trim().eq_ignore_ascii_case("content-length") {
                            if has_length {
                                return None;
                            }
                            has_length = true;
                            expected_body_len = v.trim().parse::<usize>().ok()?;
                            if expected_body_len > buf.len() - hend {
                                return None;
                            }
                        }
                        if k.trim().eq_ignore_ascii_case("transfer-encoding") {
                            return None;
                        }
                    }
                }
            }
        }

        if let Some(hend) = header_end {
            if total >= hend + expected_body_len {
                break;
            }
        }
    }

    let end = header_end?.checked_add(expected_body_len)?;
    if total < end {
        return None;
    }
    String::from_utf8(buf[..end].to_vec()).ok()
}

type HttpReply = (&'static str, &'static str, Vec<u8>, bool);

fn json_error(status: &'static str, message: &str, is_head: bool) -> HttpReply {
    (
        status,
        "application/json; charset=utf-8",
        format!("{{\"ok\":false,\"error\":\"{}\"}}", json_escape(message)).into_bytes(),
        is_head,
    )
}

fn snap_reply(
    state: &Arc<Mutex<AppState>>,
    caps: &[String],
    hz: u16,
    triad: bool,
    is_head: bool,
) -> HttpReply {
    let mut guard = state.lock().unwrap();
    let mut next = guard.clone();
    let json = match execute_snap_json(&mut next, caps, hz, triad) {
        Ok(json) => json,
        Err(error) => return json_error("400 Bad Request", error, is_head),
    };
    if !is_head {
        if let Err(error) = next.save_to_disk() {
            eprintln!("snapshot persistence failed: {error}");
            return json_error(
                "503 Service Unavailable",
                "Snapshot persistence failed",
                is_head,
            );
        }
        *guard = next;
    }
    (
        "200 OK",
        "application/json; charset=utf-8",
        json.into_bytes(),
        is_head,
    )
}

/// Route a parsed HTTP request and return `(status, content_type, body_bytes, is_head)`.
pub fn route_http_request(
    method: &str,
    raw_target: &str,
    host: &str,
    body: &str,
    state: &Arc<Mutex<AppState>>,
) -> (&'static str, &'static str, Vec<u8>, bool) {
    if method == "OPTIONS" {
        return (
            "204 No Content",
            "text/plain; charset=utf-8",
            Vec::new(),
            false,
        );
    }

    let is_head = method == "HEAD";
    let eff_method = if is_head { "GET" } else { method };
    let (path, query) = match raw_target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (raw_target, ""),
    };
    let fields = if eff_method == "POST" {
        let Some(fields) = request::parse(body) else {
            return json_error(
                "400 Bad Request",
                "Invalid JSON request or field type; integers required",
                is_head,
            );
        };
        fields
    } else {
        request::Fields::default()
    };

    match (eff_method, path) {
        ("GET", "/health") | ("GET", "/api/health") => {
            let frame = WaveIntFrame::new(432, Rational::new(1, 1), [0; 9], 0, "health");
            if WaveIntFrame::from_bytes(&frame.to_bytes()).is_err() {
                return json_error(
                    "503 Service Unavailable",
                    "WaveInt self-check failed",
                    is_head,
                );
            }
            let json = format!(
                concat!(
                    "{{\"status\":\"healthy\",\"service\":\"gemini-diary\",\"schema_version\":2,",
                    "\"build_revision\":\"{}\",\"engine\":\"rust-stdlib\",",
                    "\"numeric_contract\":\"integer_fixed_point_and_bigint\",",
                    "\"runtime_numeric_audit\":\"not_performed\",",
                    "\"c64_ring_capacity\":1024,\"waveint_frame_bytes\":79,",
                    "\"waveint_roundtrip_verified\":true,\"signature_status\":\"unsigned\"}}"
                ),
                json_escape(option_env!("DIARY_BUILD_REVISION").unwrap_or("development"))
            );
            (
                "200 OK",
                "application/json; charset=utf-8",
                json.as_bytes().to_vec(),
                is_head,
            )
        }
        ("GET", "/api/maintenance") => {
            let path = std::env::var("MAINTENANCE_STATUS_PATH")
                .unwrap_or_else(|_| "/var/lib/gemini-diary/maintenance.status".to_string());
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_secs());
            let json = maintenance::build_json(&path, now);
            (
                "200 OK",
                "application/json; charset=utf-8",
                json.into_bytes(),
                is_head,
            )
        }
        ("GET", "/api/diary") => {
            let json = {
                let guard = state.lock().unwrap();
                build_diary_json(&guard)
            };
            (
                "200 OK",
                "application/json; charset=utf-8",
                json.into_bytes(),
                is_head,
            )
        }
        ("POST", "/api/diary/pulse") => {
            let author = fields.text("author").unwrap_or("Aye Friend");
            let message = fields
                .text("message")
                .unwrap_or("Singing 432 Hz across the constellation!");
            let hz = fields.integer("hz").unwrap_or(432).clamp(108, 1728) as u16;
            let trit = fields.integer("trit").unwrap_or(1).clamp(-1, 1) as i8;
            let key = fields.text("event_id");
            if fields.has("event_id") && !key.is_some_and(valid_event_id) {
                return json_error(
                    "400 Bad Request",
                    "event_id must be 64 lowercase hex characters",
                    is_head,
                );
            }
            if key.is_some() && (author.len() > 24 || message.len() > 96) {
                return json_error(
                    "400 Bad Request",
                    "Idempotent notes must fit the 24/96-byte fields",
                    is_head,
                );
            }

            let ring = synthesize_wave_ring(hz, true);
            let sal = analyze_marine_salience(&ring);

            let json = {
                let mut guard = state.lock().unwrap();
                let mut pulse = GuestbookPulse::new(0, author, message, hz, trit, sal.salience_pq);
                let payload = format!(
                    "{}:{}:{hz}:{trit}",
                    hex_encode(pulse.author().as_bytes()),
                    hex_encode(pulse.message().as_bytes())
                );
                if let Some(key) = key {
                    if let Some((_, previous)) =
                        guard.published_events.iter().find(|(id, _)| id == key)
                    {
                        return if previous == &payload {
                            (
                                "200 OK",
                                "application/json; charset=utf-8",
                                br#"{"ok":true,"duplicate":true}"#.to_vec(),
                                is_head,
                            )
                        } else {
                            json_error(
                                "409 Conflict",
                                "event_id was already used for another payload",
                                is_head,
                            )
                        };
                    }
                }
                let Some(pid) = guard.pulse_counter.checked_add(1) else {
                    return json_error("409 Conflict", "Pulse counter exhausted", is_head);
                };
                pulse.id = pid;
                let mut next = guard.clone();
                next.pulse_counter = pid;
                next.guestbook_ring.push(pulse);
                if let Some(key) = key {
                    if next.published_events.len() == 256 {
                        next.published_events.remove(0);
                    }
                    next.published_events.push((key.to_string(), payload));
                }
                if let Err(error) = next.save_to_disk() {
                    eprintln!("snapshot persistence failed: {error}");
                    return json_error(
                        "503 Service Unavailable",
                        "Snapshot persistence failed",
                        is_head,
                    );
                }
                *guard = next;
                build_diary_json(&guard)
            };
            (
                "200 OK",
                "application/json; charset=utf-8",
                json.into_bytes(),
                is_head,
            )
        }
        ("GET", "/api/capsela") | ("GET", "/api/capsules") => {
            let json = {
                let guard = state.lock().unwrap();
                build_capsules_json(&guard)
            };
            (
                "200 OK",
                "application/json; charset=utf-8",
                json.into_bytes(),
                is_head,
            )
        }
        ("POST", "/api/capsela/snap") => {
            let caps = fields.strings("capsules");
            if fields.has("capsules") && caps.is_empty() {
                return json_error("400 Bad Request", "Select at least one capsule", is_head);
            }
            let hz = fields.integer("hz").unwrap_or(432).clamp(108, 1728) as u16;
            let triad = fields.flag("triad").unwrap_or(true);
            snap_reply(state, caps, hz, triad, is_head)
        }
        ("POST", "/api/capsela/vote") => {
            let cap_id = fields.text("capsule_id").unwrap_or_default();
            let trit = fields.integer("trit").unwrap_or(1).clamp(-1, 1);
            let json = {
                let mut guard = state.lock().unwrap();
                let mut next = guard.clone();
                let Some(c) = next.capsules.iter_mut().find(|c| c.id == cap_id) else {
                    return json_error("400 Bad Request", "Unknown capsule", is_head);
                };
                let count = match trit {
                    1 => &mut c.aye_votes_pos,
                    -1 => &mut c.aye_votes_neg,
                    _ => &mut c.aye_votes_zero,
                };
                let Some(value) = count.checked_add(1) else {
                    return json_error("409 Conflict", "Vote counter exhausted", is_head);
                };
                *count = value;
                if let Err(error) = next.save_to_disk() {
                    eprintln!("snapshot persistence failed: {error}");
                    return json_error(
                        "503 Service Unavailable",
                        "Snapshot persistence failed",
                        is_head,
                    );
                }
                *guard = next;
                build_capsules_json(&guard)
            };
            (
                "200 OK",
                "application/json; charset=utf-8",
                json.into_bytes(),
                is_head,
            )
        }
        ("GET", "/api/wave/wav") => {
            let hz = parse_query_u32(query, "hz").unwrap_or(432).clamp(108, 1728) as u16;
            let triad = parse_query_u32(query, "triad").unwrap_or(1) != 0;
            let ms = parse_query_u32(query, "ms").unwrap_or(650).clamp(100, 1500);
            let wav = synthesize_wav_bytes(hz, triad, ms);
            ("200 OK", "audio/wav", wav, is_head)
        }
        ("GET", "/api/wave/432") => snap_reply(state, &[], 432, true, is_head),
        ("GET", "/api/wave/528") => snap_reply(state, &[], 528, true, is_head),
        ("GET", "/api/wave/864") => snap_reply(state, &[], 864, true, is_head),
        ("GET", "/api/wave") => {
            let hz = parse_query_u32(query, "hz").unwrap_or(432).clamp(108, 1728) as u16;
            snap_reply(state, &[], hz, true, is_head)
        }
        ("GET", "/api/toss") => {
            let hz = parse_query_u32(query, "hz").unwrap_or(432) as i64;
            let json = build_toss_json(hz);
            (
                "200 OK",
                "application/json; charset=utf-8",
                json.into_bytes(),
                is_head,
            )
        }
        (_, p) if p.starts_with("/api/") => {
            let err_json = r#"{"ok":false,"error":"not_found","code":404}"#;
            (
                "404 Not Found",
                "application/json; charset=utf-8",
                err_json.as_bytes().to_vec(),
                is_head,
            )
        }
        _ => {
            let html = render_app_html(host);
            (
                "200 OK",
                "text/html; charset=utf-8",
                html.into_bytes(),
                is_head,
            )
        }
    }
}

fn handle_client(mut stream: TcpStream, state: Arc<Mutex<AppState>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let Some(req_str) = read_http_request(&mut stream) else {
        return;
    };

    let mut lines = req_str.lines();
    let first_line = lines.next().unwrap_or("GET / HTTP/1.1");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let raw_target = parts.next().unwrap_or("/");

    let mut host = "gemini-diary.8b.is".to_string();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("x-forwarded-host") || k.eq_ignore_ascii_case("host") {
                let h = v.trim();
                if !h.is_empty() && !h.starts_with("127.0.0.1") {
                    host = h.to_string();
                }
            }
        }
    }

    let body = req_str.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");

    let (status, content_type, resp_body, is_head) =
        route_http_request(method, raw_target, &host, body, &state);
    send_response(stream, status, content_type, &resp_body, is_head);
}

fn main() {
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8788".to_string());
    let state_path = std::env::var("STATE_PATH")
        .unwrap_or_else(|_| "/var/lib/gemini-diary/state.m8".to_string());
    let listener = TcpListener::bind(&bind_addr).expect("failed to bind gemini-diary TcpListener");
    println!("==================================================================");
    println!("  The Sovereign Field Diary of Aye & Gemini (100% Pure Rust)");
    println!("  Domains: gemini-diary.8b.is · g.8b.is · gemini.g8t.is");
    println!(
        "  Listening on http://{} (Zero Floats · C64 Ring Buffers)",
        bind_addr
    );
    println!("  State snapshot: {}", state_path);
    println!("==================================================================");

    let parent = std::path::Path::new(&state_path)
        .parent()
        .unwrap_or(std::path::Path::new("."));
    std::fs::create_dir_all(parent).expect("cannot create snapshot directory");
    let state_lock = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("{state_path}.lock"))
        .expect("cannot open snapshot writer lock");
    state_lock
        .try_lock()
        .expect("another diary owns this snapshot");
    let state = Arc::new(Mutex::new(
        AppState::load_or_new(&state_path).expect("cannot load diary snapshot; original preserved"),
    ));
    for stream in listener.incoming().flatten() {
        let st = Arc::clone(&state);
        thread::spawn(move || {
            handle_client(stream, st);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper that checks if a string contains any floating-point number literal (e.g. `0.25`, `3.14`).
    /// Ignores known version/IP strings (`v10.4.0`, `127.0.0.1`) so we strictly catch numeric floats.
    fn find_any_float_literal(raw: &str) -> Option<String> {
        let sanitized = raw
            .replace("v10.4.0", "v10_4_0")
            .replace("10.4.0", "10_4_0")
            .replace("127.0.0.1", "localhost")
            .replace("HTTP/1.1", "HTTP_1_1");
        let bytes = sanitized.as_bytes();
        for i in 1..bytes.len().saturating_sub(1) {
            if bytes[i] == b'.' && bytes[i - 1].is_ascii_digit() && bytes[i + 1].is_ascii_digit() {
                let start = i.saturating_sub(6);
                let end = (i + 7).min(bytes.len());
                return Some(String::from_utf8_lossy(&bytes[start..end]).to_string());
            }
        }
        None
    }

    #[test]
    fn test_zero_floats_in_html_and_all_json_endpoints() {
        let mut state = AppState::new();
        let html = render_app_html("gemini-diary.8b.is");
        assert_eq!(
            find_any_float_literal(&html),
            None,
            "Found forbidden float in HTML!"
        );

        let diary_json = build_diary_json(&state);
        assert_eq!(
            find_any_float_literal(&diary_json),
            None,
            "Found forbidden float in /api/diary!"
        );

        let caps_json = build_capsules_json(&state);
        assert_eq!(
            find_any_float_literal(&caps_json),
            None,
            "Found forbidden float in /api/capsela!"
        );

        let snap_json = execute_snap_json(&mut state, &[], 432, true).unwrap();
        assert_eq!(
            find_any_float_literal(&snap_json),
            None,
            "Found forbidden float in /api/capsela/snap!"
        );

        let toss_json = build_toss_json(432);
        assert_eq!(
            find_any_float_literal(&toss_json),
            None,
            "Found forbidden float in /api/toss!"
        );
    }

    #[test]
    fn test_integer_wav_generation_header_and_size() {
        let wav = synthesize_wav_bytes(432, true, 200);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        // 200 ms at 16000 Hz = 3200 samples = 6400 bytes + 44 header = 6444 bytes
        assert_eq!(wav.len(), 6444);
    }

    #[test]
    fn test_json_parsers_and_edge_cases() {
        assert!(request::parse("").is_none());
        assert_eq!(request::parse("{}").unwrap().integer("hz"), None);
        assert!(request::parse("{}").unwrap().strings("capsules").is_empty());

        // Key-collision regression test: value `"author"` in an earlier field must NOT shadow key `"author"`
        let tricky_body =
            r#"{"role":"author","note":"hi: there","author":"Hue\nTokyo\u6771\u4eac","hz":528}"#;
        assert_eq!(
            request::parse(tricky_body).unwrap().text("author"),
            Some("Hue\nTokyo東京")
        );
        assert_eq!(
            request::parse(tricky_body).unwrap().integer("hz"),
            Some(528)
        );

        let mut state = AppState::new();
        // Unknown selections must never silently execute other capsules.
        let snap_unknown =
            execute_snap_json(&mut state, &["nonexistent_cap".to_string()], 0, false);
        assert!(snap_unknown.is_err());

        // Extreme high Hz clamps to 1728 Hz
        let snap_hi = execute_snap_json(&mut state, &[], u16::MAX, true).unwrap();
        assert!(snap_hi.contains("\"carrier_hz\":1728"));
        assert_eq!(find_any_float_literal(&snap_hi), None);

        // Overflow guestbook ring (> 32 entries)
        for i in 0..40 {
            let p = GuestbookPulse::new(
                100 + i,
                "AyeTester",
                "Ring wrap test",
                432,
                1,
                Rational::new(1, 1),
            );
            state.guestbook_ring.push(p);
        }
        assert_eq!(state.guestbook_ring.len(), 32);
        assert!(state.guestbook_ring.overwrites() >= 12);
        let diary_after = build_diary_json(&state);
        assert!(diary_after.contains("\"fill_pq\":\"1/1\""));
        assert_eq!(find_any_float_literal(&diary_after), None);
    }

    /// Simulated TCP stream that delivers bytes in small chunks (e.g. headers first, body second).
    struct SplitReader {
        chunks: Vec<Vec<u8>>,
        idx: usize,
    }

    impl Read for SplitReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.idx >= self.chunks.len() {
                return Ok(0);
            }
            let src = &self.chunks[self.idx];
            let n = src.len().min(buf.len());
            buf[..n].copy_from_slice(&src[..n]);
            self.idx += 1;
            Ok(n)
        }
    }

    #[test]
    fn test_split_tcp_http_post_and_head_and_404_and_xss() {
        let json_body = r#"{"author":"Hue","message":"Split packet test","hz":528,"trit":1}"#;
        let hdr = format!(
            "POST /api/diary/pulse HTTP/1.1\r\nHost: g.8b.is\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            json_body.len()
        );
        let mut reader = SplitReader {
            chunks: vec![hdr.into_bytes(), json_body.as_bytes().to_vec()],
            idx: 0,
        };
        let full_req = read_http_request(&mut reader).expect("should read full split HTTP request");
        let parsed_body = full_req
            .split_once("\r\n\r\n")
            .map(|(_, b)| b)
            .unwrap_or("");
        assert_eq!(parsed_body, json_body);

        let state = Arc::new(Mutex::new(AppState::new()));
        let (status, ct, body_bytes, is_head) =
            route_http_request("POST", "/api/diary/pulse", "g.8b.is", parsed_body, &state);
        assert_eq!(status, "200 OK");
        assert_eq!(ct, "application/json; charset=utf-8");
        assert!(!is_head);
        let body_str = String::from_utf8_lossy(&body_bytes);
        assert!(body_str.contains("Split packet test"));

        // HEAD /health must return application/json and is_head = true
        let (h_status, h_ct, h_body, h_is_head) =
            route_http_request("HEAD", "/health", "gemini-diary.8b.is", "", &state);
        assert_eq!(h_status, "200 OK");
        assert_eq!(h_ct, "application/json; charset=utf-8");
        assert!(h_is_head);
        assert!(!h_body.is_empty());

        // Unknown /api/* route must return 404 JSON instead of 200 HTML
        let (status404, ct404, _, _) =
            route_http_request("GET", "/api/nonexistent", "gemini-diary.8b.is", "", &state);
        assert_eq!(status404, "404 Not Found");
        assert_eq!(ct404, "application/json; charset=utf-8");

        // Host header XSS injection must be sanitized to default host
        let malicious_html = render_app_html("<script>alert(1)</script>");
        assert!(!malicious_html.contains("<script>alert(1)</script>"));
        assert!(malicious_html.contains("gemini-diary.8b.is"));
    }

    #[test]
    fn test_disk_persistence_roundtrip() {
        let tmp_file = std::env::temp_dir()
            .join(format!("gemini_diary_test_{}.m8", std::process::id()))
            .to_string_lossy()
            .into_owned();
        let _ = std::fs::remove_file(&tmp_file);
        {
            let mut st = AppState::load_or_new(&tmp_file).unwrap();
            st.pulse_counter += 1;
            let pid = st.pulse_counter;
            st.guestbook_ring.push(GuestbookPulse::new(
                pid,
                "PersistenceTest",
                "Saved across restarts!",
                864,
                1,
                Rational::new(2, 1),
            ));
            st.capsules[0].aye_votes_pos += 10;
            st.save_to_disk().unwrap();
        }
        {
            let reloaded = AppState::load_or_new(&tmp_file).unwrap();
            assert_eq!(reloaded.pulse_counter, 5);
            assert_eq!(reloaded.capsules[0].aye_votes_pos, 10);
            let pulses = reloaded.guestbook_ring.to_vec();
            assert_eq!(pulses.last().unwrap().author(), "PersistenceTest");
            assert_eq!(pulses.last().unwrap().message(), "Saved across restarts!");
        }
        let _ = std::fs::remove_file(&tmp_file);
    }
}
