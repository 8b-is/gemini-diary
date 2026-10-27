use super::*;

struct Scratch(std::path::PathBuf);
impl Scratch {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!("diary-{}-{label}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn state(&self) -> String {
        self.0.join("state.m8").to_string_lossy().into_owned()
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn post(state: &Arc<Mutex<AppState>>, path: &str, body: &str) -> HttpReply {
    route_http_request("POST", path, "localhost", body, state)
}
fn event(id: u64) -> String {
    format!(r#"{{"event_id":"{id:064x}","author":"test","message":"receipt {id}","trit":0}}"#)
}

#[test]
fn replay_conflict_restart_and_ring_wrap() {
    let dir = Scratch::new("replay");
    let path = dir.state();
    let state = Arc::new(Mutex::new(AppState::load_or_new(&path).unwrap()));
    for id in 0..40 {
        assert_eq!(post(&state, "/api/diary/pulse", &event(id)).0, "200 OK");
    }
    let before = state.lock().unwrap().pulse_counter;
    let state = Arc::new(Mutex::new(AppState::load_or_new(&path).unwrap()));
    assert_eq!(state.lock().unwrap().guestbook_ring.len(), 32);
    let replay = post(&state, "/api/diary/pulse", &event(0));
    assert!(String::from_utf8(replay.2)
        .unwrap()
        .contains("\"duplicate\":true"));
    assert_eq!(
        post(
            &state,
            "/api/diary/pulse",
            &event(0).replace("receipt 0", "other payload")
        )
        .0,
        "409 Conflict"
    );
    assert_eq!(state.lock().unwrap().pulse_counter, before);
    assert_eq!(state.lock().unwrap().published_events.len(), 40);
}

#[test]
fn concurrent_retries_commit_once_and_ledger_is_bounded() {
    let state = Arc::new(Mutex::new(AppState::new()));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let state = Arc::clone(&state);
            thread::spawn(move || post(&state, "/api/diary/pulse", &event(0)))
        })
        .collect();
    for thread in threads {
        assert_eq!(thread.join().unwrap().0, "200 OK");
    }
    assert_eq!(state.lock().unwrap().pulse_counter, 5);
    for id in 1..260 {
        assert_eq!(post(&state, "/api/diary/pulse", &event(id)).0, "200 OK");
    }
    let guard = state.lock().unwrap();
    assert_eq!(guard.published_events.len(), 256);
    assert_eq!(guard.published_events[0].0, format!("{:064x}", 4));
}

#[test]
fn persistence_failure_does_not_commit_pulse_vote_or_run() {
    let dir = Scratch::new("failure");
    let blocker = dir.0.join("file-not-directory");
    std::fs::write(&blocker, "preserve").unwrap();
    let mut initial = AppState::new();
    initial.state_path = Some(blocker.join("state").to_string_lossy().into_owned());
    let state = Arc::new(Mutex::new(initial));
    for (path, body) in [
        ("/api/diary/pulse", event(1)),
        ("/api/capsela/snap", "{}".to_string()),
        (
            "/api/capsela/vote",
            r#"{"capsule_id":"c64_ring_osc_432","trit":1}"#.to_string(),
        ),
    ] {
        assert_eq!(post(&state, path, &body).0, "503 Service Unavailable");
    }
    let guard = state.lock().unwrap();
    assert_eq!((guard.pulse_counter, guard.run_counter), (4, 1));
    assert!(guard.published_events.is_empty());
    assert_eq!(guard.capsules[0].aye_votes_pos, 0);
    assert_eq!(std::fs::read_to_string(blocker).unwrap(), "preserve");
}

#[test]
fn legacy_migration_preserves_history_and_corruption_fails_closed() {
    let dir = Scratch::new("migration");
    let path = dir.state();
    let mut original = AppState::load_or_new(&path).unwrap();
    original.capsules[0].aye_votes_pos = 154;
    original.save_to_disk().unwrap();
    let versioned = std::fs::read_to_string(&path).unwrap();
    let legacy = versioned
        .replace("DIARY_STATE\t2\n", "")
        .replace("VOTE_PROVENANCE\tvisitor_unattributed\n", "");
    std::fs::write(&path, &legacy).unwrap();
    let migrated = AppState::load_or_new(&path).unwrap();
    assert!(migrated.legacy_votes);
    assert_eq!(migrated.capsules[0].aye_votes_pos, 154);
    assert_eq!(build_diary_json(&original), build_diary_json(&migrated));
    migrated.save_to_disk().unwrap();
    assert!(AppState::load_or_new(&path).unwrap().legacy_votes);
    for broken in [
        versioned.trim_end().to_string(),
        versioned.replace("DIARY_STATE\t2", "DIARY_STATE\t3"),
        versioned.replace("VOTE_PROVENANCE\tvisitor_unattributed\n", ""),
        legacy + "GARBAGE\n",
    ] {
        std::fs::write(&path, &broken).unwrap();
        assert!(AppState::load_or_new(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
    }
}

#[test]
fn selections_control_execution_and_unavailable_stages_fail() {
    let mut state = AppState::new();
    let only = |id: &str| vec![id.to_string()];
    let spiral = execute_snap_json(&mut state, &only("toss_spiral_9step"), 432, true).unwrap();
    assert!(spiral.contains("\"marine_salience\":null"));
    assert!(spiral.contains("\"mem8_waveint_79b\":null"));
    assert!(spiral.contains("\"oscilloscope_i16\":[]"));
    let oscillator = execute_snap_json(&mut state, &only("c64_ring_osc_432"), 432, false).unwrap();
    assert!(oscillator.contains("\"toss_spiral\":[]"));
    assert!(oscillator.contains("\"marine_salience\":null"));
    let salience = execute_snap_json(
        &mut state,
        &["c64_ring_osc_432".into(), "marine_salience_gate".into()],
        432,
        true,
    )
    .unwrap();
    assert!(salience.contains("\"harmonics_computed\":false"));
    for id in [
        "marine_salience_gate",
        "goertzel_harmonic_triad",
        "mem8_waveint_79b",
        "ed25519_dns_seal",
        "missing",
    ] {
        let before = state.run_counter;
        assert!(execute_snap_json(&mut state, &only(id), 432, true).is_err());
        assert_eq!(state.run_counter, before);
    }
    let all = execute_snap_json(&mut state, &[], 432, true).unwrap();
    assert!(all.contains("\"signature_status\":\"unsigned\""));
    assert!(all.contains("\"capsule_count\":5"));
    assert!(!all.contains("dns_seal"));
}

#[test]
fn malformed_requests_and_head_cannot_mutate_state() {
    let state = Arc::new(Mutex::new(AppState::new()));
    for raw in [
        r#"{"hz":4e2}"#,
        r#"{"event_id":null}"#,
        r#"{"capsules":[]}"#,
    ] {
        assert_eq!(post(&state, "/api/capsela/snap", raw).0, "400 Bad Request");
    }
    assert!(route_http_request("HEAD", "/api/wave/432", "localhost", "", &state).3);
    assert_eq!(state.lock().unwrap().run_counter, 1);
    let mut partial = &b"POST / HTTP/1.1\r\nContent-Length: 20\r\n\r\n{}"[..];
    assert!(read_http_request(&mut partial).is_none());
    let mut large = &b"POST / HTTP/1.1\r\nContent-Length: 16384\r\n\r\n{}"[..];
    assert!(read_http_request(&mut large).is_none());
}

#[test]
fn exhausted_counters_reject_new_writes_but_allow_replay() {
    let state = Arc::new(Mutex::new(AppState::new()));
    assert_eq!(post(&state, "/api/diary/pulse", &event(1)).0, "200 OK");
    {
        let mut guard = state.lock().unwrap();
        guard.pulse_counter = u64::MAX;
        guard.capsules[0].aye_votes_pos = u32::MAX;
    }
    assert_eq!(post(&state, "/api/diary/pulse", &event(1)).0, "200 OK");
    assert_eq!(
        post(&state, "/api/diary/pulse", &event(2)).0,
        "409 Conflict"
    );
    assert_eq!(
        post(
            &state,
            "/api/capsela/vote",
            r#"{"capsule_id":"c64_ring_osc_432","trit":1}"#
        )
        .0,
        "409 Conflict"
    );
}
