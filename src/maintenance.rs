//! Read the publisher's bounded, versioned local observation file.
//! Freshness is recomputed on every request, including after publisher failure.

use std::collections::BTreeMap;
use std::io::Read;

const MAX_BYTES: u64 = 8192;
const MAX_AGE_SECS: u64 = 600;

fn parse(raw: &str, now: u64) -> Result<String, &'static str> {
    let mut lines = raw.lines();
    if lines.next() != Some("DIARY_MAINTENANCE\t1") || !raw.ends_with('\n') {
        return Err("Invalid maintenance observation version or incomplete file");
    }
    let mut fields = BTreeMap::new();
    for line in lines {
        let (key, value) = line.split_once('\t').ok_or("Malformed observation field")?;
        if fields.insert(key, value).is_some() {
            return Err("Duplicate observation field");
        }
    }
    if fields.len() != 11 {
        return Err("Unexpected observation field count");
    }
    let number = |key| -> Result<u64, &'static str> {
        let value = fields.get(key).ok_or("Missing observation number")?;
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err("Invalid observation integer");
        }
        value.parse().map_err(|_| "Observation integer overflow")
    };
    let text = |key| -> Result<String, &'static str> {
        let raw = fields.get(key).ok_or("Missing observation text")?;
        let bytes = crate::hex_decode(raw).ok_or("Invalid observation hex")?;
        String::from_utf8(bytes).map_err(|_| "Invalid observation UTF-8")
    };
    let observed = number("observed_at_secs")?;
    let timestamp = number("source_timestamp_secs")?;
    let cycle = number("cycle_id")?;
    if observed == 0 || observed > now || timestamp > observed {
        return Err("Observation has an invalid or future timestamp");
    }
    let verification = text("verification")?;
    if !matches!(
        verification.as_str(),
        "unverified" | "pending" | "failed" | "ci_passed"
    ) {
        return Err("Unknown verification state");
    }
    let reported_state = text("state")?;
    if !matches!(
        reported_state.as_str(),
        "waiting" | "error" | "stale" | "unverified" | "pending" | "failed" | "ci_passed"
    ) {
        return Err("Unknown reporting state");
    }
    if match reported_state.as_str() {
        "waiting" | "error" => timestamp != 0 || cycle != 0 || verification != "unverified",
        "stale" => timestamp == 0 || cycle == 0,
        state => timestamp == 0 || cycle == 0 || state != verification,
    } {
        return Err("Inconsistent observation state");
    }
    if verification == "ci_passed" {
        let repo = text("repo")?;
        let commit = text("commit_url")?;
        let prefix = format!("https://github.com/8b-is/{repo}/commit/");
        let sha = commit
            .strip_prefix(&prefix)
            .ok_or("Invalid commit evidence URL")?;
        if repo.is_empty()
            || !repo
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
            || sha.len() != 40
            || !sha
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || text("checks_url")? != format!("{commit}/checks")
        {
            return Err("Invalid CI evidence references");
        }
    }
    let publisher_age = now - observed;
    let result_age = if timestamp == 0 {
        None
    } else {
        Some(now - timestamp)
    };
    let state = if publisher_age > MAX_AGE_SECS {
        "publisher_stale"
    } else if result_age.is_some_and(|age| age > MAX_AGE_SECS) {
        "stale"
    } else {
        &reported_state
    };
    let mut pairs = vec![
        "\"ok\":true".to_string(),
        "\"schema_version\":2".to_string(),
        format!("\"state\":\"{state}\""),
        format!("\"verification\":\"{verification}\""),
        format!("\"cycle_id\":\"{cycle}\""),
        format!("\"observed_at_secs\":{observed}"),
        format!("\"source_timestamp_secs\":{timestamp}"),
        format!("\"publisher_age_secs\":{publisher_age}"),
        format!(
            "\"result_age_secs\":{}",
            result_age.map_or_else(|| "null".to_string(), |n| n.to_string())
        ),
        format!("\"max_age_secs\":{MAX_AGE_SECS}"),
    ];
    for key in [
        "repo",
        "symbol",
        "detail",
        "commit_url",
        "checks_url",
        "source_url",
    ] {
        pairs.push(format!("\"{key}\":\"{}\"", crate::json_escape(&text(key)?)));
    }
    Ok(format!("{{{}}}", pairs.join(",")))
}

pub fn build_json(path: &str, now: u64) -> String {
    let read = || -> Result<String, &'static str> {
        let file = std::fs::File::open(path).map_err(|_| "No readable publisher observation")?;
        let mut raw = String::new();
        file.take(MAX_BYTES + 1)
            .read_to_string(&mut raw)
            .map_err(|_| "Cannot read publisher observation")?;
        if raw.len() as u64 > MAX_BYTES {
            return Err("Publisher observation exceeds size limit");
        }
        parse(&raw, now)
    };
    read().unwrap_or_else(|error| format!(
        "{{\"ok\":false,\"state\":\"unavailable\",\"verification\":\"unverified\",\"detail\":\"{}\"}}",
        crate::json_escape(error)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation() -> String {
        let mut raw = "DIARY_MAINTENANCE\t1\nobserved_at_secs\t1000\nsource_timestamp_secs\t900\ncycle_id\t42\n".to_string();
        for (key, value) in [
            ("repo", "aurora"),
            ("symbol", "task"),
            ("verification", "ci_passed"),
            ("state", "ci_passed"),
            ("detail", "GitHub checks passed"),
            ("commit_url", "https://github.com/8b-is/aurora/commit/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
            ("checks_url", "https://github.com/8b-is/aurora/commit/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/checks"),
            ("source_url", "http://127.0.0.1:8790"),
        ] {
            raw.push_str(&format!("{key}\t{}\n", crate::hex_encode(value.as_bytes())));
        }
        raw
    }

    #[test]
    fn ages_out_results_and_stopped_publishers() {
        let raw = observation();
        assert!(parse(&raw, 1000)
            .unwrap()
            .contains("\"state\":\"ci_passed\""));
        assert!(parse(&raw, 1501).unwrap().contains("\"state\":\"stale\""));
        assert!(parse(&raw, 1601)
            .unwrap()
            .contains("\"state\":\"publisher_stale\""));
        assert!(parse(&raw, 999).is_err());
        assert!(parse(&raw.replace("\t900\n", "\t1001\n"), 1001).is_err());
    }

    #[test]
    fn rejects_corrupt_partial_and_duplicate_fields() {
        let raw = observation();
        assert!(parse(raw.trim_end(), 1000).is_err());
        assert!(parse(&(raw.clone() + "state\t00\n"), 1000).is_err());
        assert!(parse(&raw.replace("cycle_id\t42", "cycle_id\t4e2"), 1000).is_err());
        assert!(parse(&raw.replace("repo\t6175726F7261", "repo\tzz"), 1000).is_err());
        assert!(parse(
            &raw.replace("source_timestamp_secs\t900", "source_timestamp_secs\t0"),
            1000
        )
        .is_err());
        assert!(parse(
            &raw.replace(
                &crate::hex_encode(b"ci_passed"),
                &crate::hex_encode(b"failed")
            ),
            1000
        )
        .is_ok());
        assert!(parse(
            &raw.replace(
                "verification\t63695F706173736564",
                "verification\t6661696C6564"
            ),
            1000
        )
        .is_err());
        assert!(parse(
            &raw.replace("DIARY_MAINTENANCE\t1", "DIARY_MAINTENANCE\t2"),
            1000
        )
        .is_err());
    }
}
