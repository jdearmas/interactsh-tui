//! All rendering. Pure functions of `&App` + frame.

use chrono::{DateTime, Utc};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap},
    Frame,
};

use crate::app::{App, Mode, View};

const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;

pub fn render(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(0),    // body
            Constraint::Length(1), // query / status bar
        ])
        .split(f.area());

    render_header(f, app, chunks[0]);
    match app.view {
        View::List => render_list_view(f, app, chunks[1]),
        View::Timeline => render_timeline_view(f, app, chunks[1]),
    }
    render_status(f, app, chunks[2]);

    if app.show_help {
        render_help(f, f.area());
    }
}

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let view = match app.view {
        View::List => "list",
        View::Timeline => "timeline",
    };
    let shown = if app.grouping {
        format!(" {} groups / {} shown ", app.groups.len(), app.filtered.len())
    } else {
        format!(" {}/{} shown ", app.filtered.len(), app.all.len())
    };
    let group_span = if app.grouping {
        Span::styled(" group:ON ", Style::default().fg(Color::Black).bg(Color::Red).add_modifier(Modifier::BOLD))
    } else {
        Span::styled(" group:off ", Style::default().fg(DIM))
    };
    let spans = vec![
        Span::styled(" oob-tui ", Style::default().bg(ACCENT).fg(Color::Black).add_modifier(Modifier::BOLD)),
        Span::raw(format!(" {} ", app.host)),
        Span::styled("│", Style::default().fg(DIM)),
        Span::raw(shown),
        Span::styled("│", Style::default().fg(DIM)),
        Span::raw(format!(" proto:{} ", app.proto.label())),
        Span::styled("│", Style::default().fg(DIM)),
        group_span,
        Span::styled("│", Style::default().fg(DIM)),
        Span::raw(format!(" view:{view} ")),
        Span::styled("│", Style::default().fg(DIM)),
        Span::raw(if app.refresh_secs > 0 {
            format!(" auto:{}s ", app.refresh_secs)
        } else {
            " auto:off ".to_string()
        }),
        Span::styled("│", Style::default().fg(DIM)),
        Span::styled(" ? help ", Style::default().fg(DIM)),
    ];
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_list_view(f: &mut Frame, app: &App, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);

    render_table(f, app, cols[0]);
    render_detail(f, app, cols[1]);
}

fn render_table(f: &mut Frame, app: &App, area: Rect) {
    let multiday = spans_multiple_days(app);
    let grouping = app.grouping;
    let rows: Vec<Row> = app
        .groups
        .iter()
        .map(|g| {
            let it = g.rep(&app.all);
            let proto_style = match it.protocol.as_str() {
                "http" => Style::default().fg(Color::Green),
                "dns" => Style::default().fg(Color::Yellow),
                _ => Style::default().fg(Color::Magenta),
            };
            // With grouping on, a >1 source group shows the count of distinct IPs.
            let nsrc = g.sources(&app.all).len();
            let source = if grouping && nsrc > 1 {
                format!("{} +{}", it.remote, nsrc - 1)
            } else {
                it.remote.clone()
            };
            let mut cells = vec![
                Cell::from(fmt_time(it.timestamp, multiday)).style(Style::default().fg(DIM))
            ];
            if grouping {
                let n = g.count();
                let badge = if n > 1 { format!("×{n}") } else { String::new() };
                let style = if n > 1 {
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(DIM)
                };
                cells.push(Cell::from(badge).style(style));
            }
            cells.push(Cell::from(it.protocol.to_uppercase()).style(proto_style));
            cells.push(Cell::from(source));
            cells.push(Cell::from(it.summary.clone()));
            Row::new(cells)
        })
        .collect();

    let tcol = Constraint::Length(if multiday { 15 } else { 8 });
    let (widths, header): (Vec<Constraint>, Row) = if grouping {
        (
            vec![tcol, Constraint::Length(6), Constraint::Length(5), Constraint::Length(18), Constraint::Min(10)],
            Row::new(["last", "count", "proto", "source", "summary"]),
        )
    } else {
        (
            vec![tcol, Constraint::Length(5), Constraint::Length(16), Constraint::Min(10)],
            Row::new(["time", "proto", "source", "summary"]),
        )
    };
    let header = header.style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD));
    let title = if grouping {
        " interactions — grouped, newest first (s to ungroup) "
    } else {
        " interactions — newest first (↑/↓ j/k, g/G) "
    };
    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(DIM))
                .title(title),
        )
        .row_highlight_style(Style::default().bg(ACCENT).fg(Color::Black).add_modifier(Modifier::BOLD))
        .highlight_symbol("▶ ");

    let mut state = TableState::default();
    if !app.filtered.is_empty() {
        state.select(Some(app.selected));
    }
    f.render_stateful_widget(table, area, &mut state);
}

fn render_detail(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DIM))
        .title(" detail (J/K or PgUp/PgDn to scroll) ");

    let Some(it) = app.selected_interaction() else {
        f.render_widget(
            Paragraph::new("no interactions match the current filter").block(block),
            area,
        );
        return;
    };

    let mut lines: Vec<Line> = Vec::new();

    // Grouped selection: lead with the collapse summary.
    if let Some(g) = app.selected_group() {
        if app.grouping && g.count() > 1 {
            let first = g.first(&app.all);
            let srcs = g.sources(&app.all);
            let src_str = if srcs.len() <= 3 {
                srcs.join(", ")
            } else {
                format!("{}, +{} more", srcs[..3].join(", "), srcs.len() - 3)
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" ×{} identical ", g.count()),
                    Style::default().bg(Color::Red).fg(Color::Black).add_modifier(Modifier::BOLD),
                ),
                Span::styled("  (newest shown below)", Style::default().fg(DIM)),
            ]));
            lines.push(Line::from(vec![
                Span::styled("seen       ", Style::default().fg(ACCENT)),
                Span::raw(format!(
                    "{} → {}",
                    first.timestamp.format("%m-%d %H:%M:%S"),
                    it.timestamp.format("%m-%d %H:%M:%S"),
                )),
            ]));
            lines.push(Line::from(vec![
                Span::styled("sources    ", Style::default().fg(ACCENT)),
                Span::raw(src_str),
            ]));
            lines.push(Line::from(""));
        }
    }

    let mut kv = |k: &str, v: String| {
        lines.push(Line::from(vec![
            Span::styled(format!("{k:<11}"), Style::default().fg(ACCENT)),
            Span::raw(v),
        ]));
    };
    kv("protocol", it.protocol.to_uppercase());
    if let Some(q) = &it.qtype {
        if !q.is_empty() {
            kv("q-type", q.clone());
        }
    }
    kv("time", it.timestamp.format("%Y-%m-%d %H:%M:%S%.3f UTC").to_string());
    kv("source", it.remote.clone());
    kv("full-id", it.full_id.clone());

    push_raw(&mut lines, "request", &it.raw_request);
    push_raw(&mut lines, "response", &it.raw_response);

    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((app.detail_scroll, 0));
    f.render_widget(para, area);
}

fn push_raw(lines: &mut Vec<Line>, title: &str, raw: &str) {
    lines.push(Line::from(""));
    lines.push(Line::styled(
        format!("── raw {title} ──"),
        Style::default().fg(DIM).add_modifier(Modifier::BOLD),
    ));
    if raw.trim().is_empty() {
        lines.push(Line::styled("(empty)", Style::default().fg(DIM)));
        return;
    }
    // Show control-free text; full content, never truncated.
    for l in raw.replace('\r', "").lines() {
        lines.push(Line::raw(l.to_string()));
    }
}

fn render_timeline_view(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DIM))
        .title(" timeline (t to return to list) ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.filtered.is_empty() {
        f.render_widget(Paragraph::new("no interactions match the current filter"), inner);
        return;
    }

    let items: Vec<&crate::model::Interaction> = app.filtered_items().collect();
    let t0 = items.first().unwrap().timestamp;
    let t1 = items.last().unwrap().timestamp;
    let span = (t1 - t0).num_seconds().max(1) as f64;

    let n = (inner.width as usize).max(1);
    let mut buckets = vec![0u64; n];
    let mut http = 0u64;
    let mut dns = 0u64;
    let mut other = 0u64;
    for it in &items {
        let frac = (it.timestamp - t0).num_seconds() as f64 / span;
        let idx = ((frac * (n as f64 - 1.0)).round() as usize).min(n - 1);
        buckets[idx] += 1;
        match it.protocol.as_str() {
            "http" => http += 1,
            "dns" => dns += 1,
            _ => other += 1,
        }
    }
    let peak = *buckets.iter().max().unwrap_or(&1).max(&1);

    let bar = spark_line(&buckets, peak);
    let secs_per_col = span / n as f64;

    let mut lines = vec![
        Line::from(vec![
            Span::styled("range  ", Style::default().fg(ACCENT)),
            Span::raw(format!(
                "{} → {}  ({})",
                t0.format("%Y-%m-%d %H:%M"),
                t1.format("%Y-%m-%d %H:%M"),
                human_span(t1.signed_duration_since(t0).num_seconds())
            )),
        ]),
        Line::from(vec![
            Span::styled("count  ", Style::default().fg(ACCENT)),
            Span::raw(format!(
                "{} total   http:{http}  dns:{dns}{}   peak {peak}/col (~{} per col)",
                items.len(),
                if other > 0 { format!("  other:{other}") } else { String::new() },
                human_span(secs_per_col as i64),
            )),
        ]),
        Line::from(""),
        Line::styled(bar, Style::default().fg(ACCENT)),
        Line::styled(axis_line(n), Style::default().fg(DIM)),
        Line::from(vec![
            Span::styled(t0.format("%m-%d %H:%M").to_string(), Style::default().fg(DIM)),
            Span::raw(" ".repeat(n.saturating_sub(22))),
            Span::styled(t1.format("%m-%d %H:%M").to_string(), Style::default().fg(DIM)),
        ]),
    ];
    lines.push(Line::from(""));
    lines.push(Line::styled(
        "newest interactions:",
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    ));
    for it in items.iter().rev().take((inner.height as usize).saturating_sub(9)) {
        lines.push(Line::from(format!(
            "  {}  {:<5}  {:<15}  {}",
            it.timestamp.format("%m-%d %H:%M:%S"),
            it.protocol.to_uppercase(),
            it.remote,
            it.summary,
        )));
    }

    f.render_widget(Paragraph::new(lines), inner);
}

fn render_status(f: &mut Frame, app: &App, area: Rect) {
    let line = if app.mode == Mode::Editing {
        Line::from(vec![
            Span::styled(" query ", Style::default().bg(Color::Yellow).fg(Color::Black)),
            Span::raw(" "),
            Span::raw(&app.query),
            Span::styled("█", Style::default().fg(ACCENT)),
        ])
    } else if !app.status.is_empty() {
        Line::from(Span::styled(format!(" {}", app.status), Style::default().fg(Color::Yellow)))
    } else {
        let q = if app.query.is_empty() {
            "(none)".to_string()
        } else {
            app.query.clone()
        };
        Line::from(vec![
            Span::styled(" / ", Style::default().fg(DIM)),
            Span::raw("query: "),
            Span::styled(q, Style::default().fg(ACCENT)),
            Span::styled(
                "   p:proto  s:group  t:timeline  e:editor  r:refresh  q:quit",
                Style::default().fg(DIM),
            ),
        ])
    };
    f.render_widget(Paragraph::new(line), area);
}

fn render_help(f: &mut Frame, area: Rect) {
    let text = vec![
        Line::styled("  oob-tui — keys", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Line::from(""),
        Line::raw("  ↑/↓  j/k   move selection"),
        Line::raw("  g / G       newest / oldest"),
        Line::raw("  J/K PgDn/Up scroll detail pane"),
        Line::raw("  /           edit text query"),
        Line::raw("  Enter       apply query (in edit)"),
        Line::raw("  Esc         cancel query / close"),
        Line::raw("  p           cycle ALL/HTTP/DNS"),
        Line::raw("  s           smart-group identical reqs"),
        Line::raw("  e           open selection in $EDITOR"),
        Line::raw("  t           toggle timeline view"),
        Line::raw("  r           refresh from server"),
        Line::raw("  ?           toggle this help"),
        Line::raw("  q           quit"),
        Line::from(""),
        Line::styled("  press any key to close", Style::default().fg(DIM)),
    ];
    let w = 56.min(area.width.saturating_sub(4));
    let h = (text.len() as u16 + 2).min(area.height.saturating_sub(2));
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(text).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(ACCENT))
                .title(" help "),
        ),
        rect,
    );
}

// ---- helpers ----

fn spans_multiple_days(app: &App) -> bool {
    match (app.filtered_items().next(), app.filtered_items().last()) {
        (Some(a), Some(b)) => a.timestamp.date_naive() != b.timestamp.date_naive(),
        _ => false,
    }
}

fn fmt_time(t: DateTime<Utc>, multiday: bool) -> String {
    if multiday {
        t.format("%m-%d %H:%M:%S").to_string()
    } else {
        t.format("%H:%M:%S").to_string()
    }
}

/// Map bucket counts to unicode block heights.
fn spark_line(buckets: &[u64], peak: u64) -> String {
    const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    buckets
        .iter()
        .map(|&c| {
            if c == 0 {
                ' '
            } else {
                let lvl = ((c as f64 / peak as f64) * 8.0).ceil() as usize;
                BLOCKS[lvl.clamp(1, 8)]
            }
        })
        .collect()
}

fn axis_line(n: usize) -> String {
    "─".repeat(n)
}

fn human_span(secs: i64) -> String {
    let s = secs.max(0);
    if s < 90 {
        format!("{s}s")
    } else if s < 5400 {
        format!("{}m", s / 60)
    } else if s < 172800 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86400)
    }
}
