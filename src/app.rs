//! Application state: the loaded interactions, the active filter, grouping, and
//! view state. Navigation is always over `groups` — with grouping off, each group
//! is a single interaction; with it on, identical requests collapse into one.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::model::Interaction;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProtoFilter {
    All,
    Http,
    Dns,
}

impl ProtoFilter {
    pub fn label(self) -> &'static str {
        match self {
            ProtoFilter::All => "ALL",
            ProtoFilter::Http => "HTTP",
            ProtoFilter::Dns => "DNS",
        }
    }
    pub fn next(self) -> Self {
        match self {
            ProtoFilter::All => ProtoFilter::Http,
            ProtoFilter::Http => ProtoFilter::Dns,
            ProtoFilter::Dns => ProtoFilter::All,
        }
    }
    fn accepts(self, proto: &str) -> bool {
        match self {
            ProtoFilter::All => true,
            ProtoFilter::Http => proto == "http",
            ProtoFilter::Dns => proto == "dns",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    List,
    Timeline,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Editing,
}

/// A navigable row: one or more interactions (indices into `App.all`) that share a
/// group signature. Indices are time-ascending; the representative is the newest.
pub struct Group {
    pub indices: Vec<usize>,
}

impl Group {
    pub fn count(&self) -> usize {
        self.indices.len()
    }
    pub fn rep<'a>(&self, all: &'a [Interaction]) -> &'a Interaction {
        &all[*self.indices.last().unwrap()]
    }
    pub fn first<'a>(&self, all: &'a [Interaction]) -> &'a Interaction {
        &all[self.indices[0]]
    }
    /// Distinct source IPs in the group, in first-seen order.
    pub fn sources(&self, all: &[Interaction]) -> Vec<String> {
        let mut seen = Vec::new();
        for &i in &self.indices {
            let r = &all[i].remote;
            if !seen.iter().any(|s| s == r) {
                seen.push(r.clone());
            }
        }
        seen
    }
}

pub struct App {
    /// Short label for the data source (ssh host or local file name), shown in the header.
    pub source: String,
    /// Configured editor override; `None` falls back to $EDITOR then `nvim`.
    pub editor: Option<String>,
    /// Auto-refresh interval in seconds (0 = disabled). Driven by the run loop.
    pub refresh_secs: u64,
    pub all: Vec<Interaction>,
    /// Flat filtered indices (time-sorted) — used by the timeline (activity volume).
    pub filtered: Vec<usize>,
    /// Navigable rows built from `filtered`, honoring the grouping toggle.
    pub groups: Vec<Group>,
    pub selected: usize, // index into `groups`
    pub query: String,
    pub proto: ProtoFilter,
    pub grouping: bool,
    pub view: View,
    pub mode: Mode,
    pub detail_scroll: u16,
    pub show_help: bool,
    pub status: String,
    pub should_quit: bool,
}

impl App {
    pub fn new(
        source: String,
        editor: Option<String>,
        refresh_secs: u64,
        all: Vec<Interaction>,
    ) -> Self {
        let mut app = App {
            source,
            editor,
            refresh_secs,
            all,
            filtered: Vec::new(),
            groups: Vec::new(),
            selected: 0,
            query: String::new(),
            proto: ProtoFilter::All,
            grouping: false,
            view: View::List,
            mode: Mode::Normal,
            detail_scroll: 0,
            show_help: false,
            status: String::new(),
            should_quit: false,
        };
        app.recompute();
        app.select_first(); // land on the most recent (top of the list)
        app
    }

    /// Rebuild `filtered` and `groups` from `all` using filter + grouping state.
    /// `groups` is ordered newest-first so the most recent activity sits at the top.
    pub fn recompute(&mut self) {
        let needle = self.query.to_lowercase();
        self.filtered = self
            .all
            .iter()
            .enumerate()
            .filter(|(_, it)| self.proto.accepts(&it.protocol) && it.matches(&needle))
            .map(|(i, _)| i)
            .collect();

        self.groups = if self.grouping {
            self.build_groups()
        } else {
            self.filtered.iter().map(|&i| Group { indices: vec![i] }).collect()
        };
        // Both branches build oldest-first (matching `filtered`); flip so the
        // newest group/interaction is index 0 (top). `filtered` stays ascending
        // for the timeline, which depends on first()=earliest, last()=latest.
        self.groups.reverse();

        if self.selected >= self.groups.len() {
            self.selected = self.groups.len().saturating_sub(1);
        }
        self.detail_scroll = 0;
    }

    /// Replace all interactions with a freshly parsed set (used by refresh),
    /// preserving the user's place: stay pinned to the top if already there,
    /// otherwise re-select whatever interaction was selected before. Returns the
    /// new interaction count.
    pub fn reload(&mut self, data: &str) -> usize {
        let was_top = self.selected == 0;
        let key = self
            .selected_interaction()
            .map(|it| (it.timestamp, it.full_id.clone()));
        self.all = crate::model::parse_all(data);
        self.recompute();
        self.restore_selection(was_top, key);
        self.all.len()
    }

    fn restore_selection(&mut self, was_top: bool, key: Option<(DateTime<Utc>, String)>) {
        if was_top || self.groups.is_empty() {
            self.select_first();
            return;
        }
        if let Some((ts, fid)) = key {
            // The log is append-only, so the previously selected interaction still
            // exists; find whichever group now holds it.
            if let Some(gi) = self.groups.iter().position(|g| {
                g.indices
                    .iter()
                    .any(|&i| self.all[i].timestamp == ts && self.all[i].full_id == fid)
            }) {
                self.selected = gi;
                self.detail_scroll = 0;
                return;
            }
        }
        self.select_first();
    }

    /// Collapse `filtered` by group signature, ordered oldest-first here (the
    /// caller reverses to newest-first).
    fn build_groups(&self) -> Vec<Group> {
        let mut by_sig: HashMap<String, usize> = HashMap::new();
        let mut groups: Vec<Group> = Vec::new();
        for &i in &self.filtered {
            let sig = self.all[i].group_signature();
            match by_sig.get(&sig) {
                Some(&g) => groups[g].indices.push(i),
                None => {
                    by_sig.insert(sig, groups.len());
                    groups.push(Group { indices: vec![i] });
                }
            }
        }
        // `filtered` is time-ascending, so each group's last index is its newest.
        groups.sort_by_key(|g| self.all[*g.indices.last().unwrap()].timestamp);
        groups
    }

    pub fn selected_group(&self) -> Option<&Group> {
        self.groups.get(self.selected)
    }

    pub fn selected_interaction(&self) -> Option<&Interaction> {
        self.selected_group().map(|g| g.rep(&self.all))
    }

    pub fn filtered_items(&self) -> impl Iterator<Item = &Interaction> {
        self.filtered.iter().map(move |&i| &self.all[i])
    }

    pub fn move_selection(&mut self, delta: i64) {
        if self.groups.is_empty() {
            return;
        }
        let len = self.groups.len() as i64;
        let s = (self.selected as i64 + delta).clamp(0, len - 1);
        self.selected = s as usize;
        self.detail_scroll = 0;
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
        self.detail_scroll = 0;
    }

    pub fn select_last(&mut self) {
        self.selected = self.groups.len().saturating_sub(1);
        self.detail_scroll = 0;
    }

    pub fn scroll_detail(&mut self, delta: i32) {
        self.detail_scroll = (self.detail_scroll as i32 + delta).max(0) as u16;
    }

    pub fn cycle_proto(&mut self) {
        self.proto = self.proto.next();
        self.recompute();
    }

    #[cfg(test)]
    fn rep_summary(&self) -> &str {
        &self.selected_interaction().unwrap().summary
    }

    pub fn toggle_view(&mut self) {
        self.view = match self.view {
            View::List => View::Timeline,
            View::Timeline => View::List,
        };
    }

    pub fn toggle_grouping(&mut self) {
        self.grouping = !self.grouping;
        self.recompute();
        self.select_first(); // newest is at the top
    }

    /// Render the current selection as a self-contained text document for `$EDITOR`.
    /// Includes the full request/response (never truncated) and, for a collapsed
    /// group, every occurrence's time + source.
    pub fn export_selected(&self) -> Option<String> {
        let g = self.selected_group()?;
        let rep = g.rep(&self.all);
        let mut out = String::new();
        out.push_str("# interactsh-tui interaction export\n\n");
        out.push_str(&format!("protocol : {}\n", rep.protocol.to_uppercase()));
        if let Some(q) = &rep.qtype {
            if !q.is_empty() {
                out.push_str(&format!("q-type   : {q}\n"));
            }
        }
        out.push_str(&format!("full-id  : {}\n", rep.full_id));

        if g.count() > 1 {
            let first = g.first(&self.all);
            let srcs = g.sources(&self.all);
            out.push_str(&format!(
                "group    : {} identical occurrences\n",
                g.count()
            ));
            out.push_str(&format!(
                "seen     : {} -> {}\n",
                first.timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
                rep.timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
            ));
            out.push_str(&format!("sources  : {}\n", srcs.join(", ")));
            out.push_str("\n## occurrences (time  source)\n");
            for &i in &g.indices {
                let it = &self.all[i];
                out.push_str(&format!(
                    "  {}  {}\n",
                    it.timestamp.format("%Y-%m-%d %H:%M:%S%.3f"),
                    it.remote
                ));
            }
            out.push_str("\n(request below is the newest occurrence; the rest are identical)\n");
        } else {
            out.push_str(&format!("source   : {}\n", rep.remote));
            out.push_str(&format!(
                "time     : {}\n",
                rep.timestamp.format("%Y-%m-%d %H:%M:%S%.3f UTC")
            ));
        }

        out.push_str("\n===== RAW REQUEST =====\n");
        out.push_str(rep.raw_request.replace('\r', "").trim_end());
        out.push_str("\n\n===== RAW RESPONSE =====\n");
        out.push_str(rep.raw_response.replace('\r', "").trim_end());
        out.push('\n');
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::parse_all;

    fn http(ts: &str, path: &str) -> String {
        format!(
            r#"{{"protocol":"http","full-id":"{path}","raw-request":"GET {path} HTTP/1.1\r\n\r\n","raw-response":"","remote-address":"1.1.1.1","timestamp":"{ts}"}}"#
        )
    }

    fn app_from(lines: &[String]) -> App {
        App::new("test".into(), None, 0, parse_all(&lines.join("\n")))
    }

    #[test]
    fn newest_is_first_and_selected_by_default() {
        let app = app_from(&[
            http("2026-06-20T10:00:00Z", "/old"),
            http("2026-06-20T12:00:00Z", "/new"),
        ]);
        assert_eq!(app.groups.len(), 2);
        assert_eq!(app.selected, 0, "selection defaults to the top");
        assert_eq!(app.rep_summary(), "GET /new", "top row is newest");
        assert_eq!(app.groups.last().unwrap().rep(&app.all).summary, "GET /old");
    }

    #[test]
    fn reload_at_top_follows_the_newest() {
        let mut app = app_from(&[http("2026-06-20T10:00:00Z", "/a")]);
        assert_eq!(app.selected, 0);
        let data = [
            http("2026-06-20T10:00:00Z", "/a"),
            http("2026-06-20T11:00:00Z", "/b"),
        ]
        .join("\n");
        app.reload(&data);
        assert_eq!(app.selected, 0);
        assert_eq!(app.rep_summary(), "GET /b", "stays pinned to newest at top");
    }

    #[test]
    fn reload_preserves_a_non_top_selection() {
        let mut app = app_from(&[
            http("2026-06-20T10:00:00Z", "/a"), // oldest -> bottom
            http("2026-06-20T11:00:00Z", "/b"),
            http("2026-06-20T12:00:00Z", "/c"), // newest -> top
        ]);
        app.select_last(); // select the oldest, /a
        assert_eq!(app.rep_summary(), "GET /a");
        let data = [
            http("2026-06-20T10:00:00Z", "/a"),
            http("2026-06-20T11:00:00Z", "/b"),
            http("2026-06-20T12:00:00Z", "/c"),
            http("2026-06-20T13:00:00Z", "/d"), // brand-new newest
        ]
        .join("\n");
        app.reload(&data);
        assert_eq!(app.rep_summary(), "GET /a", "selection follows the interaction");
        assert_ne!(app.selected, 0, "not yanked back to the top");
    }
}
