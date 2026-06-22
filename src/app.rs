//! Application state: the loaded interactions, the active filter, grouping, and
//! view state. Navigation is always over `groups` — with grouping off, each group
//! is a single interaction; with it on, identical requests collapse into one.

use std::collections::HashMap;

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
    pub host: String,
    pub remote_log: String,
    /// Configured editor override; `None` falls back to $EDITOR then `nvim`.
    pub editor: Option<String>,
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
    pub fn new(host: String, remote_log: String, editor: Option<String>, all: Vec<Interaction>) -> Self {
        let mut app = App {
            host,
            remote_log,
            editor,
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
        app.select_last(); // land on the most recent
        app
    }

    /// Rebuild `filtered` and `groups` from `all` using filter + grouping state.
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

        if self.selected >= self.groups.len() {
            self.selected = self.groups.len().saturating_sub(1);
        }
        self.detail_scroll = 0;
    }

    /// Collapse `filtered` by group signature; order groups by last-seen (newest
    /// activity at the bottom, matching the time-sorted list and `G`).
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

    pub fn toggle_view(&mut self) {
        self.view = match self.view {
            View::List => View::Timeline,
            View::Timeline => View::List,
        };
    }

    pub fn toggle_grouping(&mut self) {
        self.grouping = !self.grouping;
        self.recompute();
        self.select_last();
    }

    /// Render the current selection as a self-contained text document for `$EDITOR`.
    /// Includes the full request/response (never truncated) and, for a collapsed
    /// group, every occurrence's time + source.
    pub fn export_selected(&self) -> Option<String> {
        let g = self.selected_group()?;
        let rep = g.rep(&self.all);
        let mut out = String::new();
        out.push_str("# oob-tui interaction export\n\n");
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
