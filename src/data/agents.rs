//! Live sessions of coding agents: Claude Code, GitHub Copilot CLI and Antigravity CLI, read
//! from the files each tool already writes under the home folder. Read-only.
//!
//! Param `provider`: `claude` (default), `copilot`, `antigravity` or `all`.
//! Fields: `items` (name, cwd, tool, state, label, working, done, age), `count`, `working`,
//! `provider`, `note`. `done` is an agent that stopped working within the last minute.
//!
//! The files are read at most about once a second, and only while a widget reads `agents`:
//! every second while an agent works (its age counts up), every ten seconds otherwise.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{Cadence, DataSource, SourceCx};
use crate::value::Value;

/// A file touched this recently means the agent is mid-turn.
const WORKING_WINDOW_MS: i64 = 20_000;
/// Older than this is not shown: Claude's session file stays behind after a crash.
const STALE_MS: i64 = 12 * 3600 * 1000;
/// Antigravity has no live marker: a conversation stays listed this long after its last write.
const ANTIGRAVITY_SHOWN_MS: i64 = 3600 * 1000;
/// Idle for less than this: it has just finished, worth a look.
const DONE_MS: i64 = 60_000;
/// Widgets reading the same tool within this long share one look at its files.
const FRESH: Duration = Duration::from_millis(900);

pub const PROVIDERS: &[&str] = &["claude", "copilot", "antigravity"];

#[derive(Clone, Debug, PartialEq)]
pub struct Agent {
    pub name: String,
    pub cwd: String,
    /// Which tool runs it, as people call it.
    pub tool: &'static str,
    pub working: bool,
    pub updated_ms: i64,
}

fn leaf(path: &str) -> String {
    path.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().unwrap_or(path).to_string()
}

pub fn age_label(now: i64, then: i64) -> String {
    let s = (now - then).max(0) / 1000;
    match s {
        0..=4 => "now".into(),
        5..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        _ => format!("{}h", s / 3600),
    }
}

pub fn is_working(updated_ms: i64, now: i64) -> bool {
    now - updated_ms <= WORKING_WINDOW_MS
}

fn modified_ms(p: &Path) -> i64 {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64)
}

fn entries(dir: &Path) -> Vec<std::fs::DirEntry> {
    std::fs::read_dir(dir).map(|rd| rd.filter_map(|e| e.ok()).collect()).unwrap_or_default()
}

/// `~/.claude/sessions/<pid>.json`: `status` is "busy" or "idle".
pub fn parse_claude(json: &str, now: i64) -> Option<Agent> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let cwd = v["cwd"].as_str().unwrap_or("").to_string();
    let updated = v["updatedAt"].as_i64().unwrap_or(0);
    if now - updated > STALE_MS {
        return None;
    }
    let name = v["name"].as_str().filter(|n| !n.is_empty()).map(String::from).unwrap_or_else(|| leaf(&cwd));
    Some(Agent { name, cwd, tool: "Claude Code", working: v["status"].as_str() == Some("busy"), updated_ms: updated })
}

/// `~/.copilot/session-state/<id>/workspace.yaml`: flat `key: value` lines.
pub fn yaml_field<'a>(yaml: &'a str, key: &str) -> Option<&'a str> {
    yaml.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(':')).map(str::trim)
}

fn claude(home: &Path, now: i64) -> Vec<Agent> {
    entries(&home.join(".claude").join("sessions"))
        .iter()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .filter_map(|e| parse_claude(&std::fs::read_to_string(e.path()).ok()?, now))
        .collect()
}

fn copilot(home: &Path, now: i64) -> Vec<Agent> {
    entries(&home.join(".copilot").join("session-state"))
        .iter()
        .filter(|d| d.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|d| {
            let base = d.path();
            // a lock file exists only while a Copilot process holds the session
            if !entries(&base).iter().any(|f| f.file_name().to_string_lossy().starts_with("inuse.")) {
                return None;
            }
            let yaml = std::fs::read_to_string(base.join("workspace.yaml")).unwrap_or_default();
            let cwd = yaml_field(&yaml, "cwd").unwrap_or("").to_string();
            let name = yaml_field(&yaml, "name").filter(|n| !n.is_empty()).map(String::from).unwrap_or_else(|| leaf(&cwd));
            let updated = modified_ms(&base.join("events.jsonl"));
            // a crashed Copilot leaves its lock file behind for good
            if now - updated > STALE_MS {
                return None;
            }
            Some(Agent { name, cwd, tool: "Copilot CLI", working: is_working(updated, now), updated_ms: updated })
        })
        .collect()
}

/// One SQLite file per conversation (plus `-wal` / `-shm`): the newest write of the three.
fn antigravity(home: &Path, now: i64) -> Vec<Agent> {
    let mut latest: BTreeMap<String, i64> = BTreeMap::new();
    for e in entries(&home.join(".gemini").join("antigravity-cli").join("conversations")) {
        let file = e.file_name().to_string_lossy().into_owned();
        let Some((id, _)) = file.split_once(".db") else { continue };
        let t = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64);
        let at = latest.entry(id.to_string()).or_insert(0);
        *at = (*at).max(t);
    }
    latest
        .into_iter()
        .filter(|(_, t)| now - t <= ANTIGRAVITY_SHOWN_MS)
        .map(|(id, t)| Agent { name: format!("session {}", &id[..id.len().min(8)]), cwd: String::new(), tool: "Antigravity CLI", working: is_working(t, now), updated_ms: t })
        .collect()
}

/// The sessions of `provider` (or of every tool, for `all`), working first, then the latest.
pub fn sessions(provider: &str, home: &Path, now: i64) -> Vec<Agent> {
    let mut found: Vec<Agent> = match provider {
        "copilot" => copilot(home, now),
        "antigravity" => antigravity(home, now),
        "all" => [claude(home, now), copilot(home, now), antigravity(home, now)].concat(),
        _ => claude(home, now),
    };
    found.sort_by(|a, b| b.working.cmp(&a.working).then(b.updated_ms.cmp(&a.updated_ms)));
    found
}

pub fn to_value(provider: &str, agents: &[Agent], now: i64) -> Value {
    let items = agents
        .iter()
        .map(|a| {
            let done = !a.working && now - a.updated_ms < DONE_MS;
            Value::obj([
                ("name", a.name.as_str().into()),
                ("cwd", a.cwd.as_str().into()),
                ("tool", a.tool.into()),
                ("state", (if a.working { "working" } else if done { "done" } else { "idle" }).into()),
                ("label", (if a.working { "Working" } else if done { "Done" } else { "Idle" }).into()),
                ("working", a.working.into()),
                ("done", done.into()),
                ("age", age_label(now, a.updated_ms).into()),
            ])
        })
        .collect();
    let guessed = agents.iter().any(|a| a.tool == "Antigravity CLI") || provider == "antigravity";
    Value::obj([
        ("items", Value::List(items)),
        ("count", (agents.len() as i32).into()),
        ("working", (agents.iter().filter(|a| a.working).count() as i32).into()),
        ("provider", provider.into()),
        ("note", (if guessed { "Antigravity activity is inferred from file writes" } else { "" }).into()),
    ])
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

fn provider_of(cx: &SourceCx) -> String {
    match cx.params.get("provider").map(|v| v.to_string()) {
        Some(p) if p == "all" || PROVIDERS.contains(&p.as_str()) => p,
        _ => "claude".into(),
    }
}

#[derive(Default)]
pub struct Agents {
    /// The home folder; `None` reads `USERPROFILE`.
    home: Option<PathBuf>,
    /// Per provider: when its files were last read, and what they said.
    seen: Mutex<HashMap<String, (Instant, Vec<Agent>)>>,
}

impl Agents {
    pub fn at(home: impl Into<PathBuf>) -> Agents {
        Agents { home: Some(home.into()), ..Default::default() }
    }

    fn look(&self, provider: &str) -> Vec<Agent> {
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, list)) = seen.get(provider).filter(|(at, _)| at.elapsed() < FRESH) {
            return list.clone();
        }
        let Some(home) = self.home.clone().or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from)) else { return vec![] };
        let list = sessions(provider, &home, now_ms());
        seen.insert(provider.to_string(), (Instant::now(), list.clone()));
        list
    }
}

impl DataSource for Agents {
    fn name(&self) -> &str {
        "agents"
    }

    fn value(&self, cx: &SourceCx) -> Value {
        let provider = provider_of(cx);
        to_value(&provider, &self.look(&provider), now_ms())
    }

    fn cadence(&self, _field: &str, cx: &SourceCx) -> Option<Cadence> {
        let busy = self.seen.lock().unwrap_or_else(|e| e.into_inner()).get(&provider_of(cx)).is_some_and(|(_, l)| l.iter().any(|a| a.working));
        Some(if busy { Cadence::Second } else { Cadence::TenSecond })
    }

    fn invalidate(&self) {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::InstanceCfg;

    fn home(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wf-agents-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn claude_busy_is_working_and_the_name_falls_back_to_the_folder() {
        let now = 1_000_000_000;
        let a = parse_claude(&format!(r#"{{"cwd":"C:\\dev\\app","status":"busy","updatedAt":{now}}}"#), now).unwrap();
        assert_eq!((a.name.as_str(), a.working, a.tool), ("app", true, "Claude Code"));
        let b = parse_claude(&format!(r#"{{"name":"Fix bug","status":"idle","updatedAt":{now}}}"#), now).unwrap();
        assert_eq!((b.name.as_str(), b.working), ("Fix bug", false));
        assert!(parse_claude(r#"{"status":"busy","updatedAt":1}"#, 48 * 3600 * 1000).is_none(), "a day old is hidden");
    }

    #[test]
    fn helpers() {
        assert_eq!(yaml_field("id: x\ncwd: c:\\a\nname: My session\n", "name"), Some("My session"));
        assert_eq!(yaml_field("id: x", "name"), None);
        assert_eq!((age_label(100_000, 100_000), age_label(200_000, 100_000), age_label(7_300_000, 0)), ("now".to_string(), "1m".to_string(), "2h".to_string()));
        assert!(is_working(100_000, 110_000) && !is_working(100_000, 200_000));
    }

    #[test]
    fn sessions_are_read_from_home_busy_first() {
        let now = 1_000_000_000_000i64;
        let h = home("claude");
        let dir = h.join(".claude").join("sessions");
        std::fs::create_dir_all(&dir).unwrap();
        for (i, (st, t)) in [("idle", now), ("busy", now - 5000)].iter().enumerate() {
            std::fs::write(dir.join(format!("{i}.json")), format!(r#"{{"name":"s{i}","status":"{st}","updatedAt":{t}}}"#)).unwrap();
        }
        std::fs::write(dir.join("notes.txt"), "not a session").unwrap();
        let found = sessions("claude", &h, now);
        assert_eq!(found.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["s1", "s0"], "busy first");
        let v = to_value("claude", &found, now);
        assert_eq!((v.get("count"), v.get("working"), v.get("note")), (Some(&Value::Num(2.0)), Some(&Value::Num(1.0)), Some(&Value::Str(String::new()))));
        let Some(Value::List(items)) = v.get("items") else { panic!("no items") };
        assert_eq!(items[1].get("label"), Some(&Value::Str("Done".into())), "idle a moment ago: just finished");
        let later = to_value("claude", &found, now + DONE_MS);
        let Some(Value::List(items)) = later.get("items") else { panic!("no items") };
        assert_eq!(items[1].get("label"), Some(&Value::Str("Idle".into())));
        std::fs::remove_dir_all(&h).ok();
    }

    #[test]
    fn a_copilot_lock_left_by_a_crash_is_not_a_live_session() {
        let h = home("copilot");
        let s = h.join(".copilot").join("session-state").join("abc");
        std::fs::create_dir_all(&s).unwrap();
        std::fs::write(s.join("inuse.123.lock"), "").unwrap();
        std::fs::write(s.join("workspace.yaml"), "name: Live\ncwd: c:\\x\n").unwrap();
        std::fs::write(s.join("events.jsonl"), "{}").unwrap();
        let now = now_ms();
        let live = sessions("copilot", &h, now);
        assert_eq!(live.iter().map(|a| (a.name.as_str(), a.working)).collect::<Vec<_>>(), [("Live", true)], "just written: working");
        assert!(sessions("copilot", &h, now + STALE_MS + 60_000).is_empty(), "a lock with no writes for half a day is a crash");
        std::fs::remove_dir_all(&h).ok();
    }

    #[test]
    fn all_lists_every_tool_and_a_missing_tool_is_an_empty_list() {
        let h = home("all");
        let now = now_ms();
        let dir = h.join(".claude").join("sessions");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("1.json"), format!(r#"{{"name":"c","status":"idle","updatedAt":{now}}}"#)).unwrap();
        let conv = h.join(".gemini").join("antigravity-cli").join("conversations");
        std::fs::create_dir_all(&conv).unwrap();
        std::fs::write(conv.join("0123456789.db"), "").unwrap();
        std::fs::write(conv.join("0123456789.db-wal"), "").unwrap();
        let all = sessions("all", &h, now);
        assert_eq!(all.iter().map(|a| a.tool).collect::<Vec<_>>(), ["Antigravity CLI", "Claude Code"], "the one just written works");
        assert_eq!(all[0].name, "session 01234567", "one conversation, however many files");
        assert!(sessions("copilot", &h, now).is_empty());
        assert_ne!(to_value("all", &all, now).get("note"), Some(&Value::Str(String::new())));
        std::fs::remove_dir_all(&h).ok();
    }

    #[test]
    fn it_ticks_every_second_only_while_an_agent_works() {
        let h = home("cadence");
        let dir = h.join(".claude").join("sessions");
        std::fs::create_dir_all(&dir).unwrap();
        let src = Agents::at(&h);
        let cfg = InstanceCfg { id: "agent_status-1".into(), ..Default::default() };
        let params = cfg.params_map();
        let cx = SourceCx { cfg: &cfg, params: &params, tm: crate::data::now_local(), icon_pack: "Default" };
        assert_eq!(src.value(&cx).get("count"), Some(&Value::Num(0.0)));
        assert_eq!(src.cadence("items", &cx), Some(Cadence::TenSecond));
        std::fs::write(dir.join("1.json"), format!(r#"{{"name":"c","status":"busy","updatedAt":{}}}"#, now_ms())).unwrap();
        assert_eq!(src.value(&cx).get("count"), Some(&Value::Num(0.0)), "read at most about once a second");
        src.invalidate();
        assert_eq!(src.value(&cx).get("working"), Some(&Value::Num(1.0)));
        assert_eq!(src.cadence("items", &cx), Some(Cadence::Second));
        std::fs::remove_dir_all(&h).ok();
    }
}
