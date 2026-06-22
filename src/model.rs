//! Parsing interactsh `interactions.jsonl` records into a typed, sorted timeline.

use chrono::{DateTime, Utc};
use serde_json::Value;

/// One OOB interaction as logged by the interactsh notifier.
#[derive(Clone, Debug)]
pub struct Interaction {
    pub timestamp: DateTime<Utc>,
    pub protocol: String,    // "http", "dns", "smtp", ...
    pub remote: String,      // source IP
    pub full_id: String,     // queried sub-domain / correlation id
    pub qtype: Option<String>, // DNS query type, when protocol == dns
    pub raw_request: String,
    pub raw_response: String,
    /// One-line human summary derived from the request (method+path, dns query, ...).
    pub summary: String,
}

impl Interaction {
    fn from_value(v: &Value) -> Option<Self> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let ts_raw = v.get("timestamp").and_then(Value::as_str)?;
        let timestamp = DateTime::parse_from_rfc3339(ts_raw).ok()?.with_timezone(&Utc);

        let protocol = {
            let p = s("protocol");
            if p.is_empty() { "?".into() } else { p.to_lowercase() }
        };
        let qtype = v.get("q-type").and_then(Value::as_str).map(str::to_string);
        let raw_request = s("raw-request");
        let full_id = {
            let f = s("full-id");
            if f.is_empty() { s("unique-id") } else { f }
        };
        let summary = summarize(&protocol, qtype.as_deref(), &raw_request, &full_id);

        Some(Interaction {
            timestamp,
            protocol,
            remote: s("remote-address"),
            full_id,
            qtype,
            raw_request,
            raw_response: s("raw-response"),
            summary,
        })
    }

    /// Identity used by smart grouping: interactions sharing a signature differ
    /// only in time (and server-generated response nonces), so they collapse into
    /// one navigable group. For HTTP/etc. that's protocol+source+raw request; for
    /// DNS it's source+query-type+sub-domain (DNS raw requests carry volatile ids).
    pub fn group_signature(&self) -> String {
        const SEP: char = '\u{1}';
        match self.protocol.as_str() {
            "dns" => format!(
                "dns{SEP}{}{SEP}{}{SEP}{}",
                self.remote,
                self.qtype.as_deref().unwrap_or(""),
                self.full_id
            ),
            _ => format!("{}{SEP}{}{SEP}{}", self.protocol, self.remote, self.raw_request),
        }
    }

    /// Lowercased haystack used for free-text query matching.
    pub fn matches(&self, needle_lower: &str) -> bool {
        if needle_lower.is_empty() {
            return true;
        }
        self.summary.to_lowercase().contains(needle_lower)
            || self.remote.to_lowercase().contains(needle_lower)
            || self.protocol.contains(needle_lower)
            || self.full_id.to_lowercase().contains(needle_lower)
            || self.raw_request.to_lowercase().contains(needle_lower)
            || self.raw_response.to_lowercase().contains(needle_lower)
    }
}

fn summarize(proto: &str, qtype: Option<&str>, raw_request: &str, full_id: &str) -> String {
    match proto {
        "dns" => format!("{} {}", qtype.unwrap_or("?"), full_id),
        _ => {
            // HTTP/SMTP/... first request line, e.g. "GET /path HTTP/1.1" -> "GET /path".
            let first = raw_request.lines().next().unwrap_or("").trim();
            if first.is_empty() {
                full_id.to_string()
            } else {
                let mut parts = first.split_whitespace();
                match (parts.next(), parts.next()) {
                    (Some(a), Some(b)) => format!("{a} {b}"),
                    (Some(a), None) => a.to_string(),
                    _ => first.to_string(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HTTP: &str = r#"{"protocol":"http","unique-id":"abc","full-id":"abc.oob.example.com","raw-request":"GET /steal?x=1 HTTP/1.1\r\nHost: h\r\n\r\n","raw-response":"HTTP/1.1 200 OK\r\n\r\nhi","remote-address":"1.2.3.4","timestamp":"2026-06-14T20:59:01.662306966Z"}"#;
    const DNS: &str = r#"{"protocol":"dns","q-type":"A","unique-id":"xyz","full-id":"xyz.oob.example.com","raw-request":";; QUESTION","raw-response":"","remote-address":"8.8.8.8","timestamp":"2026-06-14T21:00:00Z"}"#;

    #[test]
    fn parses_and_sorts() {
        // DNS line first in input but earlier... no, HTTP is earlier; feed reversed to test sort.
        let data = format!("{DNS}\n{HTTP}\n\n");
        let v = parse_all(&data);
        assert_eq!(v.len(), 2);
        // sorted ascending: HTTP (20:59) before DNS (21:00)
        assert_eq!(v[0].protocol, "http");
        assert_eq!(v[0].summary, "GET /steal?x=1");
        assert_eq!(v[1].protocol, "dns");
        assert_eq!(v[1].summary, "A xyz.oob.example.com");
    }

    #[test]
    fn matching_is_case_insensitive_and_searches_raw() {
        let v = parse_all(HTTP);
        let it = &v[0];
        assert!(it.matches("steal"));
        assert!(it.matches("1.2.3.4"));
        assert!(it.matches("get /steal"));
        assert!(!it.matches("nonexistent"));
        assert!(it.matches("")); // empty needle matches all
    }

    #[test]
    fn identical_requests_share_a_signature() {
        // Two HTTP hits, identical request, different time + response nonce + ts.
        let a = HTTP;
        let b = HTTP.replace("20:59:01.662306966", "21:05:00.000000000").replace("hi", "HI");
        let v = parse_all(&format!("{a}\n{b}"));
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].group_signature(), v[1].group_signature());
        // A different path must NOT collapse with them.
        let c = HTTP.replace("/steal?x=1", "/other");
        let w = parse_all(c.as_str());
        assert_ne!(v[0].group_signature(), w[0].group_signature());
        // Different source IP is a different group too.
        let d = HTTP.replace("1.2.3.4", "9.9.9.9");
        let x = parse_all(d.as_str());
        assert_ne!(v[0].group_signature(), x[0].group_signature());
    }

    #[test]
    fn bad_lines_are_skipped() {
        let data = format!("not json\n{HTTP}\n{{\"partial\":true}}\n");
        let v = parse_all(&data);
        assert_eq!(v.len(), 1); // only the valid, timestamped record
    }
}

/// Parse every JSONL line and return interactions sorted ascending by time.
pub fn parse_all(data: &str) -> Vec<Interaction> {
    let mut items: Vec<Interaction> = data
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() {
                return None;
            }
            let v: Value = serde_json::from_str(l).ok()?;
            Interaction::from_value(&v)
        })
        .collect();
    items.sort_by_key(|i| i.timestamp);
    items
}
