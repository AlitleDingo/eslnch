use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const HELP_BANNER: &str = r"
 ___  ___  _      _  _    ___  _  _
| __|/ __|| |    | \| |  / __|| || |
| _| \__ \| |__  | .` | | (__ | __ |
|___||___/|____| |_|\_|  \___||_||_|
  fast app launcher";

const HELP_EXAMPLES: &str = "\
Examples:
  eslnch                    open the interactive menu
  eslnch blender            launch blender (name, prefix or unique match)
  eslnch scan               detect installed apps
  eslnch add code \"code --new-window\"
  eslnch remove blender";

#[derive(Parser)]
#[command(
    name = "eslnch",
    version,
    about = "Launch apps fast. Run with no arguments for the menu.",
    before_help = HELP_BANNER,
    after_help = HELP_EXAMPLES
)]
#[command(args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// App to launch (name, prefix, or unique substring)
    app: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Detect installed apps and register them (never overwrites existing entries)
    Scan,
    /// List registered apps
    Ls,
    /// Add or update an app
    Add { name: String, path: String },
    /// Remove an app
    Remove { name: String },
    /// Launch an app (useful if its name clashes with a subcommand)
    Run { name: String },
}

#[derive(Serialize, Deserialize, Default)]
struct Config {
    #[serde(default)]
    apps: BTreeMap<String, String>,
}

fn config_path() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("could not determine config directory")?
        .join("eslnch")
        .join("config.toml"))
}

fn load() -> Result<Config> {
    let p = config_path()?;
    if !p.exists() {
        return Ok(Config::default());
    }
    let text = fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", p.display()))
}

fn save(cfg: &Config) -> Result<()> {
    let p = config_path()?;
    fs::create_dir_all(p.parent().unwrap())?;
    let tmp = p.with_extension("toml.tmp");
    fs::write(&tmp, toml::to_string_pretty(cfg)?)?;
    fs::rename(&tmp, &p)?;
    Ok(())
}

fn norm(name: &str) -> String {
    name.trim().to_lowercase()
}

// ---------- launching ----------

#[cfg(target_os = "macos")]
fn special(target: &str) -> Option<Command> {
    if target.ends_with(".app") || Path::new(target).is_dir() {
        let mut c = Command::new("open");
        c.arg("-a").arg(target);
        return Some(c);
    }
    None
}

#[cfg(windows)]
fn special(target: &str) -> Option<Command> {
    let is_lnk = Path::new(target)
        .extension()
        .map_or(false, |e| e.eq_ignore_ascii_case("lnk"));
    if is_lnk {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]).arg(target);
        return Some(c);
    }
    None
}

#[cfg(not(any(target_os = "macos", windows)))]
fn special(_target: &str) -> Option<Command> {
    None
}

fn build_command(target: &str) -> Result<Command> {
    if let Some(c) = special(target) {
        return Ok(c);
    }
    let p = Path::new(target);
    if p.is_file() {
        return Ok(Command::new(p));
    }
    let parts = shlex::split(target)
        .filter(|v| !v.is_empty())
        .with_context(|| format!("invalid command: {target}"))?;
    let mut c = Command::new(&parts[0]);
    c.args(&parts[1..]);
    Ok(c)
}

#[cfg(unix)]
fn detach(c: &mut Command) {
    use std::os::unix::process::CommandExt;
    // New session: app survives terminal close and Ctrl+C.
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
}

#[cfg(windows)]
fn detach(c: &mut Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    if c.get_program() == "cmd" {
        c.creation_flags(CREATE_NO_WINDOW);
    } else {
        c.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
}

fn launch(name: &str, target: &str) -> Result<()> {
    let mut cmd = build_command(target)?;
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    detach(&mut cmd);
    cmd.spawn()
        .with_context(|| format!("failed to launch '{name}' ({target})"))?;
    Ok(())
}

// ---------- lookup ----------

fn resolve<'a>(cfg: &'a Config, query: &str) -> Result<(&'a String, &'a String)> {
    let q = norm(query);
    if let Some(kv) = cfg.apps.get_key_value(&q) {
        return Ok(kv);
    }
    let prefix: Vec<_> = cfg.apps.iter().filter(|(k, _)| k.starts_with(&q)).collect();
    if prefix.len() == 1 {
        return Ok(prefix[0]);
    }
    let found = if prefix.is_empty() {
        cfg.apps.iter().filter(|(k, _)| k.contains(&q)).collect()
    } else {
        prefix
    };
    match found.len() {
        0 => bail!("no app matching '{query}'. Try `eslnch scan` or `eslnch add <name> <path>`."),
        1 => Ok(found[0]),
        _ => {
            let names: Vec<_> = found.iter().take(10).map(|(k, _)| k.as_str()).collect();
            bail!("'{query}' is ambiguous: {}", names.join(", "))
        }
    }
}

// ---------- scanning ----------

fn alias(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join("-").to_lowercase()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn parse_desktop(path: &Path) -> Option<(String, String)> {
    let text = fs::read_to_string(path).ok()?;
    let (mut name, mut exec, mut ty) = (None, None, None);
    let mut in_entry = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_entry = l == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        if l == "NoDisplay=true" || l == "Hidden=true" {
            return None;
        }
        if let Some(v) = l.strip_prefix("Name=") {
            name.get_or_insert(v.to_string());
        } else if let Some(v) = l.strip_prefix("Exec=") {
            exec.get_or_insert(v.to_string());
        } else if let Some(v) = l.strip_prefix("Type=") {
            ty.get_or_insert(v.to_string());
        }
    }
    if ty.as_deref() != Some("Application") {
        return None;
    }
    let exec = exec?
        .split_whitespace()
        .filter(|t| !(t.len() == 2 && t.starts_with('%')))
        .collect::<Vec<_>>()
        .join(" ");
    if exec.is_empty() {
        return None;
    }
    Some((alias(&name?), exec))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn scan_system() -> Vec<(String, String)> {
    let mut dirs_: Vec<PathBuf> = vec![
        "/usr/share/applications".into(),
        "/usr/local/share/applications".into(),
        "/var/lib/flatpak/exports/share/applications".into(),
        "/var/lib/snapd/desktop/applications".into(),
    ];
    if let Some(h) = dirs::home_dir() {
        dirs_.insert(0, h.join(".local/share/applications"));
        dirs_.push(h.join(".local/share/flatpak/exports/share/applications"));
    }
    let mut out = Vec::new();
    for d in dirs_ {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map_or(false, |x| x == "desktop") {
                if let Some(entry) = parse_desktop(&p) {
                    out.push(entry);
                }
            }
        }
    }
    out
}

#[cfg(target_os = "macos")]
fn scan_system() -> Vec<(String, String)> {
    let mut dirs_: Vec<PathBuf> = vec![
        "/Applications".into(),
        "/Applications/Utilities".into(),
        "/System/Applications".into(),
        "/System/Applications/Utilities".into(),
    ];
    if let Some(h) = dirs::home_dir() {
        dirs_.insert(0, h.join("Applications"));
    }
    let mut out = Vec::new();
    for d in dirs_ {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map_or(false, |x| x == "app") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    out.push((alias(stem), p.to_string_lossy().into_owned()));
                }
            }
        }
    }
    out
}

#[cfg(windows)]
fn walk(d: &Path, out: &mut Vec<(String, String)>) {
    let Ok(rd) = fs::read_dir(d) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().map_or(false, |x| x.eq_ignore_ascii_case("lnk")) {
            if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                let low = stem.to_lowercase();
                if low.contains("uninstall") {
                    continue;
                }
                out.push((alias(stem), p.to_string_lossy().into_owned()));
            }
        }
    }
}

#[cfg(windows)]
fn scan_system() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for var in ["APPDATA", "PROGRAMDATA"] {
        if let Ok(base) = std::env::var(var) {
            walk(
                &Path::new(&base).join(r"Microsoft\Windows\Start Menu\Programs"),
                &mut out,
            );
        }
    }
    out
}

// ---------- commands ----------

fn cmd_ls(cfg: &Config) {
    if cfg.apps.is_empty() {
        println!("No apps registered. Run `eslnch scan` or `eslnch add <name> <path>`.");
        return;
    }
    let w = cfg.apps.keys().map(|k| k.chars().count()).max().unwrap_or(4).max(4);
    println!("{:<w$}  PATH", "NAME", w = w);
    println!("{:<w$}  ----", "----", w = w);
    for (k, v) in &cfg.apps {
        println!("{:<w$}  {}", k, v, w = w);
    }
}

// ---------- interactive menu ----------

const LOGO: [[&str; 4]; 6] = [
    [" ___ ", "| __|", "| _| ", "|___|"],
    [" ___ ", r"/ __|", r"\__ \", r"|___/"],
    [" _    ", "| |   ", "| |__ ", "|____|"],
    [" _  _ ", r"| \| |", "| .` |", r"|_|\_|"],
    ["  ___ ", r" / __|", "| (__ ", r" \___|"],
    [" _  _ ", "| || |", "| __ |", "|_||_|"],
];
const LOGO_COLORS: [Color; 4] = [Color::Cyan, Color::Cyan, Color::Blue, Color::Magenta];

struct TermGuard;

impl TermGuard {
    fn new() -> Result<Self> {
        terminal::enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        Ok(Self)
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

fn score(name: &str, q: &str) -> Option<u32> {
    if q.is_empty() || name.starts_with(q) {
        return Some(0);
    }
    if name.contains(q) {
        return Some(1);
    }
    let mut chars = name.chars();
    for c in q.chars() {
        if !chars.any(|n| n == c) {
            return None;
        }
    }
    Some(2)
}

fn filter(names: &[String], query: &str) -> Vec<usize> {
    let q = query.to_lowercase();
    let mut v: Vec<(u32, usize)> = names
        .iter()
        .enumerate()
        .filter_map(|(i, n)| score(n, &q).map(|s| (s, i)))
        .collect();
    v.sort();
    v.into_iter().map(|(_, i)| i).collect()
}

fn pick(names: &[String]) -> Result<Option<usize>> {
    let _guard = TermGuard::new()?;
    let mut out = io::stdout();
    let (mut query, mut sel, mut top) = (String::new(), 0usize, 0usize);

    loop {
        let hits = filter(names, &query);
        if sel >= hits.len() {
            sel = hits.len().saturating_sub(1);
        }
        let (w, h) = terminal::size()?;
        let (w, h) = (w as usize, h as usize);
        let mut y: u16 = 1;

        queue!(out, MoveTo(0, 0), Clear(ClearType::All))?;

        if h >= 16 && w >= 34 {
            for row in 0..4 {
                let line: String = LOGO.iter().map(|l| l[row]).collect();
                queue!(
                    out,
                    MoveTo(2, y),
                    SetForegroundColor(LOGO_COLORS[row]),
                    SetAttribute(Attribute::Bold),
                    Print(line),
                    SetAttribute(Attribute::Reset),
                    ResetColor
                )?;
                y += 1;
            }
            queue!(
                out,
                MoveTo(2, y),
                SetForegroundColor(Color::DarkGrey),
                Print(format!("fast app launcher  ·  {} apps", names.len())),
                ResetColor
            )?;
            y += 2;
        }

        queue!(
            out,
            MoveTo(2, y),
            SetForegroundColor(Color::Cyan),
            Print("search › "),
            ResetColor,
            Print(&query),
            SetForegroundColor(Color::DarkGrey),
            Print("▌"),
            ResetColor
        )?;
        y += 2;

        let rows_vis = h.saturating_sub(y as usize + 2).max(1);
        let col_w = (w.saturating_sub(4) / 2).max(12);
        let total_rows = (hits.len() + 1) / 2;
        let sel_row = sel / 2;
        if sel_row < top {
            top = sel_row;
        }
        if sel_row >= top + rows_vis {
            top = sel_row + 1 - rows_vis;
        }

        if hits.is_empty() {
            queue!(
                out,
                MoveTo(2, y),
                SetForegroundColor(Color::DarkGrey),
                Print("no matches"),
                ResetColor
            )?;
        }
        for r in top..total_rows.min(top + rows_vis) {
            for c in 0..2 {
                let i = r * 2 + c;
                if i >= hits.len() {
                    break;
                }
                let name: String = names[hits[i]].chars().take(col_w - 4).collect();
                queue!(out, MoveTo((2 + c * col_w) as u16, y))?;
                if i == sel {
                    queue!(
                        out,
                        SetForegroundColor(Color::Cyan),
                        SetAttribute(Attribute::Bold),
                        Print(format!("❯ {name}")),
                        SetAttribute(Attribute::Reset),
                        ResetColor
                    )?;
                } else {
                    queue!(out, Print(format!("  {name}")))?;
                }
            }
            y += 1;
        }

        queue!(
            out,
            MoveTo(2, h.saturating_sub(1) as u16),
            SetForegroundColor(Color::DarkGrey),
            Print("↑↓←→ move · type to search · enter launch · esc quit"),
            ResetColor
        )?;
        out.flush()?;

        if let Event::Key(k) = event::read()? {
            if k.kind == KeyEventKind::Release {
                continue;
            }
            let last = hits.len().saturating_sub(1);
            match k.code {
                KeyCode::Esc => return Ok(None),
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(None)
                }
                KeyCode::Enter => {
                    if let Some(&i) = hits.get(sel) {
                        return Ok(Some(i));
                    }
                }
                KeyCode::Up => sel = sel.saturating_sub(2),
                KeyCode::Down => sel = (sel + 2).min(last),
                KeyCode::Left => sel = sel.saturating_sub(1),
                KeyCode::Right | KeyCode::Tab => sel = (sel + 1).min(last),
                KeyCode::Backspace => {
                    query.pop();
                    sel = 0;
                }
                KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                    query.push(c);
                    sel = 0;
                }
                _ => {}
            }
        }
    }
}

fn cmd_menu(cfg: &Config) -> Result<()> {
    if cfg.apps.is_empty() {
        println!("No apps registered. Run `eslnch scan` or `eslnch add <name> <path>`.");
        return Ok(());
    }
    if !io::stdout().is_terminal() {
        bail!("interactive menu needs a terminal; use `eslnch <name>` instead");
    }
    let names: Vec<String> = cfg.apps.keys().cloned().collect();
    if let Some(i) = pick(&names)? {
        let name = &names[i];
        launch(name, &cfg.apps[name])?;
        println!("Launched {name}");
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut cfg = load()?;

    match cli.cmd {
        Some(Cmd::Scan) => {
            let mut added = 0;
            for (name, target) in scan_system() {
                if name.is_empty() {
                    continue;
                }
                if !cfg.apps.contains_key(&name) {
                    cfg.apps.insert(name, target);
                    added += 1;
                }
            }
            save(&cfg)?;
            println!("Added {added} new app(s); {} total registered.", cfg.apps.len());
        }
        Some(Cmd::Ls) => cmd_ls(&cfg),
        Some(Cmd::Add { name, path }) => {
            let name = norm(&name);
            if name.is_empty() {
                bail!("name cannot be empty");
            }
            let first = shlex::split(&path).and_then(|v| v.into_iter().next());
            if !Path::new(&path).exists() && first.map_or(true, |f| !Path::new(&f).exists()) {
                eprintln!("warning: '{path}' not found on disk (saved anyway; may resolve via PATH)");
            }
            let existed = cfg.apps.insert(name.clone(), path).is_some();
            save(&cfg)?;
            println!("{} '{name}'.", if existed { "Updated" } else { "Added" });
        }
        Some(Cmd::Remove { name }) => {
            let name = norm(&name);
            if cfg.apps.remove(&name).is_none() {
                bail!("'{name}' is not registered");
            }
            save(&cfg)?;
            println!("Removed '{name}'.");
        }
        Some(Cmd::Run { name }) => {
            let (n, p) = resolve(&cfg, &name)?;
            launch(n, p)?;
        }
        None => match cli.app {
            Some(q) => {
                let (n, p) = resolve(&cfg, &q)?;
                launch(n, p)?;
            }
            None => cmd_menu(&cfg)?,
        },
    }
    Ok(())
}
