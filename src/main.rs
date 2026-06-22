//! interactsh-tui — browse, query, and timeline interactsh OOB-server interactions.
//!
//! Connection settings (ssh host, remote log path, editor) live in a config file
//! — see `config.example.toml`. CLI flags override the config:
//!   interactsh-tui                 # fetch from the configured host over ssh (gzip)
//!   interactsh-tui --host myalias  # override the ssh host alias
//!   interactsh-tui --config p.toml # use a specific config file
//!   interactsh-tui --file log.jsonl  # read a local jsonl file instead of ssh
//!   interactsh-tui --cached        # use the last cached fetch, no ssh

mod app;
mod config;
mod model;
mod ui;

use std::io::Read;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use ratatui::crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

use app::{App, Mode};
use config::Config;

struct Args {
    host: Option<String>,
    file: Option<PathBuf>,
    cached: bool,
    config: Option<PathBuf>,
}

fn parse_args() -> Result<Args> {
    let mut host = None;
    let mut file = None;
    let mut cached = false;
    let mut config = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--host" => host = Some(it.next().context("--host needs a value")?),
            "--file" => file = Some(PathBuf::from(it.next().context("--file needs a value")?)),
            "--config" => config = Some(PathBuf::from(it.next().context("--config needs a value")?)),
            "--cached" => cached = true,
            "-h" | "--help" => {
                println!(
                    "interactsh-tui — interactsh OOB interaction browser\n\n\
                     Settings come from config.toml (see config.example.toml).\n\n\
                     --host <alias>   ssh host alias (overrides config)\n\
                     --config <path>  use a specific config file\n\
                     --file <path>    read a local jsonl file instead of ssh\n\
                     --cached         reuse the last fetch (no ssh)\n\
                     -h, --help       this help"
                );
                std::process::exit(0);
            }
            other => bail!("unknown argument: {other}"),
        }
    }
    Ok(Args { host, file, cached, config })
}

fn cache_path() -> PathBuf {
    let base = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join(".cache/interactsh-tui/interactions.jsonl")
}

/// Pull the log from the server over ssh, decompressing the gzip stream.
fn fetch_from_server(host: &str, remote_log: &str) -> Result<String> {
    if host.trim().is_empty() {
        bail!(
            "no ssh host configured. Copy config.example.toml to config.toml and set \
             `host`, or pass --host <alias> (or --file <path> for a local log)."
        );
    }
    let out = Command::new("ssh")
        .arg(host)
        .arg(format!("gzip -c {remote_log}"))
        .output()
        .context("failed to spawn ssh")?;
    if !out.status.success() {
        bail!(
            "ssh {host} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let mut s = String::new();
    GzDecoder::new(&out.stdout[..])
        .read_to_string(&mut s)
        .context("failed to gunzip remote log")?;
    // Best-effort cache so --cached works offline next time.
    let p = cache_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&p, &s);
    Ok(s)
}

fn load_data(args: &Args, host: &str, remote_log: &str) -> Result<String> {
    if let Some(f) = &args.file {
        return std::fs::read_to_string(f).with_context(|| format!("reading {}", f.display()));
    }
    if args.cached {
        let p = cache_path();
        return std::fs::read_to_string(&p)
            .with_context(|| format!("no cache at {} (run once without --cached)", p.display()));
    }
    fetch_from_server(host, remote_log)
}

fn main() -> Result<()> {
    let args = parse_args()?;
    let (cfg, cfg_path) = Config::load(args.config.as_deref())?;
    // CLI --host overrides the configured host.
    let host = args.host.clone().unwrap_or_else(|| cfg.host.clone());

    if let Some(p) = &cfg_path {
        eprintln!("config: {}", p.display());
    } else if args.file.is_none() && !args.cached {
        eprintln!("config: none found — using defaults (see config.example.toml)");
    }
    eprintln!("loading interactions…");
    let data = load_data(&args, &host, &cfg.remote_log)?;
    let interactions = model::parse_all(&data);
    if interactions.is_empty() {
        eprintln!("warning: no interactions parsed");
    }

    // Auto-refresh pulls over ssh, so disable it in the explicitly-offline modes.
    let refresh_secs = if args.file.is_some() || args.cached {
        0
    } else {
        cfg.refresh_secs
    };
    let mut app = App::new(
        host,
        cfg.remote_log.clone(),
        cfg.editor.clone(),
        refresh_secs,
        interactions,
    );
    let mut terminal = ratatui::init();
    let res = run(&mut terminal, &mut app);
    ratatui::restore();
    res
}

/// Type sent from a background fetch thread to the UI loop: Ok(jsonl) or Err(msg).
type FetchResult = std::result::Result<String, String>;

/// Spawn a background ssh fetch; the result arrives on `tx` so the UI never blocks.
fn spawn_fetch(tx: &mpsc::Sender<FetchResult>, host: &str, remote_log: &str) {
    let tx = tx.clone();
    let host = host.to_string();
    let remote_log = remote_log.to_string();
    thread::spawn(move || {
        let _ = tx.send(fetch_from_server(&host, &remote_log).map_err(|e| format!("{e:#}")));
    });
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    // Auto-clear a transient status message after a moment.
    let mut status_set: Option<Instant> = None;
    let mut dirty = true; // redraw only when state changed, not on a fixed clock

    // Background-refresh plumbing. `fetching` guards against overlapping fetches;
    // `last_fetch` paces the auto interval (and is reset on completion/failure so a
    // failed fetch waits a full interval instead of hammering the server).
    let (tx, rx) = mpsc::channel::<FetchResult>();
    let host = app.host.clone();
    let remote_log = app.remote_log.clone();
    let auto = Duration::from_secs(app.refresh_secs);
    let mut last_fetch = Instant::now();
    let mut fetching = false;

    loop {
        if dirty {
            terminal.draw(|f| ui::render(f, app))?;
            dirty = false;
        }

        // Kick off an auto-refresh when the interval has elapsed.
        if app.refresh_secs > 0 && !fetching && last_fetch.elapsed() >= auto {
            spawn_fetch(&tx, &host, &remote_log);
            fetching = true;
            app.status = "auto-refreshing…".into();
            status_set = Some(Instant::now());
            dirty = true;
        }

        // Apply a completed fetch (from either an auto-refresh or the `r` key).
        match rx.try_recv() {
            Ok(Ok(data)) => {
                let n = app.reload(&data);
                fetching = false;
                last_fetch = Instant::now();
                app.status = format!("updated — {n} interactions");
                status_set = Some(Instant::now());
                dirty = true;
            }
            Ok(Err(e)) => {
                fetching = false;
                last_fetch = Instant::now();
                app.status = format!("refresh failed: {e}");
                status_set = Some(Instant::now());
                dirty = true;
            }
            Err(_) => {} // nothing ready
        }

        // Wake periodically so background results land and the status line clears.
        if !event::poll(Duration::from_millis(250))? {
            if let Some(t) = status_set {
                if t.elapsed() > Duration::from_secs(4) {
                    app.status.clear();
                    status_set = None;
                    dirty = true;
                }
            }
            continue;
        }

        let ev = event::read()?;
        if matches!(ev, Event::Resize(_, _)) {
            dirty = true;
            continue;
        }
        let Event::Key(key) = ev else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        // Any handled key produces a redraw.
        dirty = true;

        // Help overlay: any key closes it.
        if app.show_help {
            app.show_help = false;
            continue;
        }

        if app.mode == Mode::Editing {
            match key.code {
                KeyCode::Esc => {
                    app.mode = Mode::Normal;
                }
                KeyCode::Enter => {
                    app.mode = Mode::Normal;
                    app.recompute();
                    app.select_first(); // jump to the newest match (top)
                }
                KeyCode::Backspace => {
                    app.query.pop();
                    app.recompute();
                }
                KeyCode::Char(c) => {
                    app.query.push(c);
                    app.recompute();
                }
                _ => {}
            }
            continue;
        }

        // Normal mode.
        match key.code {
            KeyCode::Char('q') => app.should_quit = true,
            KeyCode::Esc => app.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.should_quit = true
            }
            KeyCode::Char('?') => app.show_help = true,
            KeyCode::Char('/') => {
                app.mode = Mode::Editing;
                app.status.clear();
            }
            KeyCode::Char('p') => app.cycle_proto(),
            KeyCode::Char('s') => app.toggle_grouping(),
            KeyCode::Char('e') => {
                if let Err(e) = open_in_editor(terminal, app) {
                    app.status = format!("editor failed: {e}");
                }
                status_set = Some(Instant::now());
            }
            KeyCode::Char('t') => app.toggle_view(),
            KeyCode::Char('j') | KeyCode::Down => app.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => app.move_selection(-1),
            KeyCode::Char('g') | KeyCode::Home => app.select_first(),
            KeyCode::Char('G') | KeyCode::End => app.select_last(),
            KeyCode::Char('J') | KeyCode::PageDown => app.scroll_detail(5),
            KeyCode::Char('K') | KeyCode::PageUp => app.scroll_detail(-5),
            KeyCode::Char('r') => {
                if !fetching {
                    spawn_fetch(&tx, &host, &remote_log);
                    fetching = true;
                    app.status = format!("refreshing from {}…", app.host);
                    status_set = Some(Instant::now());
                }
            }
            _ => {}
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

/// Write the selected interaction (or whole group) to a temp file and open it in
/// `$EDITOR` (default `nvim`), suspending the TUI for the duration.
fn open_in_editor(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    let Some(text) = app.export_selected() else {
        app.status = "nothing selected to open".into();
        return Ok(());
    };
    let it = app.selected_interaction().expect("selection exists");
    let stamp = it.timestamp.format("%Y%m%dT%H%M%S");
    let path = std::env::temp_dir().join(format!("oob-{}-{}.txt", it.protocol, stamp));
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;

    // Editor precedence: config `editor` > $EDITOR > nvim. The value may carry
    // args (e.g. "code -w"): program = first token, rest precede the path.
    let editor = app
        .editor
        .clone()
        .or_else(|| std::env::var("EDITOR").ok())
        .unwrap_or_else(|| "nvim".into());
    let mut parts = editor.split_whitespace();
    let prog = parts.next().unwrap_or("nvim");

    // Drop out of the TUI so the editor owns the real terminal.
    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen)?;

    let status = Command::new(prog)
        .args(parts)
        .arg(&path)
        .status()
        .with_context(|| format!("launching editor '{prog}'"));

    // Restore the TUI regardless of how the editor exited.
    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    terminal.clear()?;

    let status = status?;
    app.status = if status.success() {
        format!("opened {}", path.display())
    } else {
        format!("editor exited with {status}")
    };
    Ok(())
}
