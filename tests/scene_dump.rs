//! `wayfinder scene` end to end, as separate processes: no window and no GPU device is
//! created (a dump needs none). The expected dumps are for fixtures whose layout does not
//! depend on the machine's fonts; the font hash in the header is masked. A scene with text is
//! exact on the machine and font set it was dumped on, so those are only compared with
//! themselves across two runs.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn exe() -> &'static str {
    env!("CARGO_BIN_EXE_wayfinder")
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wf-scene-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn scene(args: &[&str]) -> Output {
    Command::new(exe()).arg("scene").args(args).current_dir(repo()).output().expect("the wayfinder binary runs")
}

fn text(o: &[u8]) -> String {
    String::from_utf8_lossy(o).replace("\r\n", "\n")
}

/// A dump with the font hash of its header masked.
fn masked(dump: &str) -> String {
    let at = dump.find("fonts=system:").map(|i| i + "fonts=system:".len());
    match at {
        Some(i) => {
            let end = dump[i..].find(|c: char| !c.is_ascii_hexdigit()).map_or(dump.len(), |e| i + e);
            format!("{}*{}", &dump[..i], &dump[end..])
        }
        None => dump.to_string(),
    }
}

fn read(path: &Path) -> String {
    text(&std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
}

#[test]
fn a_fixture_scenes_dump_equals_its_checked_in_expected_file() {
    let out = temp("expected");
    let o = scene(&["dump", "fixtures/plain", "fixtures/clock", "--out", out.to_str().unwrap(), "-q"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    for name in ["plain", "clock"] {
        let got = masked(&read(&out.join("fixtures").join(format!("{name}.dump.txt"))));
        let want = read(&repo().join("tests").join("scene_expected").join(format!("{name}.dump.txt")));
        assert_eq!(got, want, "fixtures/{name}: if the change is meant, replace tests/scene_expected/{name}.dump.txt with the new dump (font hash masked as `*`)");
    }
    assert!(read(&out.join("summary.txt")).contains("2 scenes, 2 ok"), "the summary file holds what the console prints");
    let last: serde_json::Value = serde_json::from_str(&read(&out.join("last.json"))).unwrap();
    assert_eq!((last["format"].as_u64(), last["exit"].as_u64(), last["hermetic"].as_bool(), last["scenes"].as_array().map(Vec::len)), (Some(1), Some(0), Some(true), Some(2)));
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn two_separate_processes_give_identical_dumps_and_records() {
    let (a, b) = (temp("run-a"), temp("run-b"));
    for out in [&a, &b] {
        let o = scene(&["dump", "fixtures", "--out", out.to_str().unwrap(), "-q"]);
        assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    }
    let mut names: Vec<String> = Vec::new();
    for e in std::fs::read_dir(a.join("fixtures")).unwrap() {
        names.push(e.unwrap().file_name().to_string_lossy().into_owned());
    }
    assert!(names.iter().filter(|n| n.ends_with(".dump.txt")).count() >= 6, "the Tier sweep, the text and the text-free fixtures: {names:?}");
    for n in &names {
        assert_eq!(read(&a.join("fixtures").join(n)), read(&b.join("fixtures").join(n)), "{n}");
    }
    assert_eq!(read(&a.join("last.json")), read(&b.join("last.json")), "the run id is a hash of the dumps");
    let _ = (std::fs::remove_dir_all(a), std::fs::remove_dir_all(b));
}

#[test]
fn a_tier_sweep_dumps_one_scene_per_tier() {
    let o = scene(&["list", "fixtures/monitor-tiers"]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(text(&o.stdout).lines().collect::<Vec<_>>(), ["fixtures/monitor-tiers@compact", "fixtures/monitor-tiers@normal", "fixtures/monitor-tiers@large"]);
    let o = scene(&["list", "--sets"]);
    assert!(text(&o.stdout).contains("fixtures   6 scenes"), "{}", text(&o.stdout));
}

#[test]
fn a_view_and_the_json_twin_come_from_the_same_dump() {
    let out = temp("views");
    let o = scene(&["dump", "fixtures/digital-clock", "--view", "texts", "--out", out.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let shown = text(&o.stdout);
    assert!(shown.contains("\"15:42\"") && shown.contains("\"Sunday, 8 March\""), "{shown}");
    let o = scene(&["dump", "fixtures/digital-clock", "--find", "Sunday", "--out", out.to_str().unwrap(), "-q"]);
    assert_eq!(o.status.code(), Some(0));
    let o = scene(&["dump", "fixtures/digital-clock", "--format", "json", "--out", out.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let j: serde_json::Value = serde_json::from_str(&text(&o.stdout)).expect("stdout is one JSON document");
    let texts: Vec<&str> = j["nodes"].as_array().unwrap().iter().filter(|n| n["kind"] == "text").filter_map(|n| n["text"].as_str()).collect();
    assert_eq!((texts, j["format"].as_u64(), j["header"]["id"].as_str()), (vec!["15:42", "Sunday, 8 March"], Some(1), Some("fixtures/digital-clock")));
    let _ = std::fs::remove_dir_all(out);
}

fn write_scene(dir: &Path, name: &str, body: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, body).unwrap();
    p
}

#[test]
fn an_unknown_scene_key_is_exit_2_with_a_suggestion_and_nothing_is_run() {
    let dir = temp("badkey").join("scenes");
    let f = write_scene(&dir, "bad.scene.toml", "format = 1\n[widget]\nid = \"clock\"\nsizee = \"180x180\"\n");
    let out = dir.join("out");
    let o = scene(&["dump", f.to_str().unwrap(), "--out", out.to_str().unwrap()]);
    let err = text(&o.stderr);
    assert_eq!(o.status.code(), Some(2), "{err}");
    assert!(err.contains("unknown key `widget.sizee`") && err.contains("did you mean `size`") && err.contains("nothing was run"), "{err}");
    assert!(!out.exists(), "a bad file writes nothing");
    assert_eq!(scene(&["list", "--root", dir.to_str().unwrap()]).status.code(), Some(2), "list reads the files too");
    let o = scene(&["dump", "x", "--nope"]);
    assert_eq!(o.status.code(), Some(2));
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

#[test]
fn a_failed_expectation_is_exit_1_and_a_missing_scene_is_exit_2() {
    let dir = temp("expect").join("scenes");
    let f = write_scene(&dir, "e.scene.toml", "format = 1\n[widget]\nid = \"clock\"\n[expect]\ntext = [\"nothing like this\"]\n");
    let out = dir.join("out");
    let o = scene(&["dump", f.to_str().unwrap(), "--out", out.to_str().unwrap(), "-q"]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o.stderr));
    assert!(read(&out.join("summary.txt")).contains("EXPECT"), "{}", read(&out.join("summary.txt")));
    let o = scene(&["dump", "fixtures/clok", "--out", out.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(2));
    assert!(text(&o.stderr).contains("did you mean `fixtures/clock`"), "{}", text(&o.stderr));
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

#[test]
fn an_unwritable_output_folder_is_exit_3() {
    let dir = temp("infra");
    let blocker = write_scene(&dir, "file", "not a folder");
    let o = scene(&["dump", "fixtures/plain", "--out", blocker.join("sub").to_str().unwrap(), "-q"]);
    assert_eq!(o.status.code(), Some(3), "{}", text(&o.stderr));
    let _ = std::fs::remove_dir_all(dir);
}

/// A scenes folder with a widget whose text is 20 px wide on one line: it can only be cut off,
/// whatever the machine's fonts are.
fn narrow(name: &str, expect: &str) -> (PathBuf, PathBuf) {
    let dir = temp(name).join("scenes");
    write_scene(&dir.join("widget"), "narrow.toml", "name = 'Narrow'\nsize = [120, 60]\nmin_size = [48, 48]\n[root]\npadding = 8\n  [[root.children]]\n  type = 'text'\n  text = 'Wednesday 23 September'\n  size = 14\n  width = 20\n");
    let f = write_scene(&dir, "narrow.scene.toml", &format!("format = 1\n[widget]\nfile = \"widget/narrow.toml\"\n[expect]\n{expect}\n"));
    (dir, f)
}

#[test]
fn check_exits_1_for_an_error_level_flag_and_keeps_the_dump_with_the_flag_in_it() {
    let (dir, f) = narrow("check-error", "flags = \"error\"");
    let out = dir.join("out");
    let o = scene(&["check", f.to_str().unwrap(), "--out", out.to_str().unwrap()]);
    let shown = text(&o.stdout);
    assert_eq!(o.status.code(), Some(1), "{shown}{}", text(&o.stderr));
    assert!(shown.contains("narrow (120x60): TRUNCATED narrow-1/0") && shown.contains("> box 20"), "{shown}");
    assert!(shown.contains("1 scene, 0 ok, 1 need a look. exit 1"), "{shown}");
    assert!(read(&out.join("summary.txt")).contains("FLAGS"), "{}", read(&out.join("summary.txt")));
    let dump = read(&out.join("narrow.dump.txt"));
    assert!(dump.contains("--- flags (1)\nTRUNCATED  narrow-1/0") && dump.lines().any(|l| l.contains("\"Wednesday 23 September\"") && l.ends_with(" TRUNCATED")), "the trailer and the node line both carry it: {dump}");
    let o = scene(&["dump", f.to_str().unwrap(), "--view", "flags", "--out", out.to_str().unwrap(), "-q"]);
    assert_eq!(o.status.code(), Some(1), "dump judges flags the same way");
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

#[test]
fn a_warn_level_flag_is_shown_and_passes_and_an_ignored_or_allowed_one_is_quiet() {
    for (name, expect, shows) in [("check-warn", "flags = \"warn\"", true), ("check-ignore", "flags = \"ignore\"", false), ("check-allow", "flags = \"error\"\nallow = [\"TRUNCATED\", \"OVERFLOW-X\"]", false)] {
        let (dir, f) = narrow(name, expect);
        let out = dir.join("out");
        let o = scene(&["check", f.to_str().unwrap(), "--out", out.to_str().unwrap()]);
        let shown = text(&o.stdout);
        assert_eq!(o.status.code(), Some(0), "{name}: {shown}{}", text(&o.stderr));
        assert_eq!(shown.contains("warning: TRUNCATED"), shows, "{name}: {shown}");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }
}

#[test]
fn check_of_a_clean_set_is_exit_0_and_a_bad_scene_is_exit_2() {
    let out = temp("check-clean");
    let o = scene(&["check", "fixtures/plain", "fixtures/clock", "fits/digital_clock", "--out", out.to_str().unwrap(), "-q"]);
    let shown = text(&o.stdout);
    assert_eq!(o.status.code(), Some(0), "{shown}{}", text(&o.stderr));
    assert!(shown.contains("36 scenes, 36 ok, 0 need a look. exit 0"), "the digital clock's 34 fit cases and two fixtures: {shown}");
    let last: serde_json::Value = serde_json::from_str(&read(&out.join("last.json"))).unwrap();
    assert_eq!((last["exit"].as_u64(), last["scenes"].as_array().map(Vec::len)), (Some(0), Some(36)));
    let o = scene(&["check", "fits/digital_clok", "--out", out.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(2));
    assert!(text(&o.stderr).contains("did you mean `fits/digital_clock`"), "{}", text(&o.stderr));
    let _ = std::fs::remove_dir_all(out);
}

/// A scenes folder holding the text-free `plain` widget and a scene of it, so a baseline of it
/// does not depend on the machine's fonts. Returns the scenes folder and the widget file.
fn plain_set(name: &str) -> (PathBuf, PathBuf) {
    let dir = temp(name).join("scenes");
    let widget = write_scene(&dir.join("widget"), "plain.toml", &read(&repo().join("scenes/fixtures/widget/plain.toml")));
    write_scene(&dir, "plain.scene.toml", "format = 1\n[widget]\nfile = \"widget/plain.toml\"\nsize = \"180x100\"\n");
    (dir, widget)
}

fn baseline_files(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                todo.push(p);
            } else if p.extension().is_some_and(|x| x == "txt") {
                out.push((p.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/"), read(&p)));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn bless_needs_a_reason_then_diff_names_a_padding_change_by_node_key_and_exits_1() {
    let (dir, widget) = plain_set("bless-diff");
    let root = dir.to_str().unwrap();
    let id = "plain";

    // no baseline yet: a finding, not a pass
    let o = scene(&["diff", id, "--root", root, "--no-pixels", "-q"]);
    assert_eq!(o.status.code(), Some(1), "{}{}", text(&o.stdout), text(&o.stderr));
    assert!(text(&o.stdout).contains("NEW"), "{}", text(&o.stdout));

    // bless refuses without a reason, and writes nothing
    let o = scene(&["bless", id, "--root", root]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o.stderr));
    assert!(text(&o.stderr).contains("--reason"), "{}", text(&o.stderr));
    assert!(!dir.join("baselines").exists());

    let o = scene(&["bless", id, "--root", root, "--reason", "first baseline"]);
    assert_eq!(o.status.code(), Some(0), "{}{}", text(&o.stdout), text(&o.stderr));
    let files = baseline_files(&dir.join("baselines"));
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(files[0].0.ends_with("/plain.dump.txt") && files[0].1.contains("fonts=system:"), "the baseline is filed under its font hash: {files:?}");
    let log = std::fs::read_dir(dir.join("baselines")).unwrap().flatten().map(|e| e.path().join("bless.log")).find(|p| p.is_file()).expect("a bless.log");
    assert!(read(&log).contains("first baseline"), "{}", read(&log));

    // a second process finds the same dump: nothing changed
    let o = scene(&["diff", id, "--root", root, "--no-pixels"]);
    assert_eq!(o.status.code(), Some(0), "{}{}", text(&o.stdout), text(&o.stderr));

    // change a padding: geometry, by node key, exit 1
    let changed = read(&widget).replace("padding = 10", "padding = 18");
    assert_ne!(changed, read(&widget));
    std::fs::write(&widget, changed).unwrap();
    let o = scene(&["diff", id, "--root", root, "--no-pixels"]);
    let shown = text(&o.stdout);
    assert_eq!(o.status.code(), Some(1), "{shown}{}", text(&o.stderr));
    assert!(shown.contains("plain  DUMP  geometry ") && shown.contains("plain-1/0  rect  30,30 40x80 -> 38,38 40x64") && shown.contains("moved right 8"), "{shown}");
    assert!(!shown.contains("geometry 0"), "{shown}");

    // blessing it needs a reason again, and records the change
    let o = scene(&["bless", id, "--root", root, "--reason", "wider padding"]);
    assert_eq!(o.status.code(), Some(0), "{}{}", text(&o.stdout), text(&o.stderr));
    assert!(read(&log).contains("wider padding"));
    assert_eq!(scene(&["diff", id, "--root", root, "--no-pixels", "-q"]).status.code(), Some(0));
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

#[test]
fn baselines_made_by_two_separate_processes_are_byte_identical() {
    let (dir, _) = plain_set("bless-twice");
    let root = dir.to_str().unwrap();
    let (a, b) = (dir.join("a"), dir.join("b"));
    for to in [&a, &b] {
        let o = scene(&["bless", "plain", "--root", root, "--baselines", to.to_str().unwrap(), "--reason", "twice"]);
        assert_eq!(o.status.code(), Some(0), "{}{}", text(&o.stdout), text(&o.stderr));
    }
    assert_eq!(baseline_files(&a), baseline_files(&b));
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

#[test]
fn a_baseline_made_with_other_fonts_is_never_compared() {
    let (dir, _) = plain_set("bless-fonts");
    let root = dir.to_str().unwrap();
    assert_eq!(scene(&["bless", "plain", "--root", root, "--reason", "here"]).status.code(), Some(0));
    // pretend the baselines came from another machine: move them under another font hash
    let base = dir.join("baselines");
    let mine = std::fs::read_dir(&base).unwrap().flatten().map(|e| e.path()).find(|p| p.is_dir()).unwrap();
    let theirs = base.join("system-00000000");
    std::fs::rename(&mine, &theirs).unwrap();
    let file = theirs.join("plain.dump.txt");
    let from = read(&file);
    let at = from.find("fonts=system:").unwrap() + "fonts=system:".len();
    let end = from[at..].find(|c: char| !c.is_ascii_hexdigit()).map_or(from.len(), |e| at + e);
    std::fs::write(&file, format!("{}00000000{}", &from[..at], &from[end..])).unwrap();
    let o = scene(&["diff", "plain", "--root", root, "--no-pixels"]);
    let (shown, err) = (text(&o.stdout), text(&o.stderr));
    assert_eq!(o.status.code(), Some(3), "{shown}{err}");
    assert!(shown.contains("FONTS") && err.contains("font set") && !shown.contains("geometry"), "{shown}{err}");
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}

#[test]
fn bless_refuses_a_flagged_run_and_writes_nothing() {
    let (dir, _f) = narrow("bless-flagged", "flags = \"error\"");
    let root = dir.to_str().unwrap();
    let o = scene(&["bless", "narrow", "--root", root, "--reason", "should not be written"]);
    let err = text(&o.stderr);
    assert_eq!(o.status.code(), Some(1), "{}{err}", text(&o.stdout));
    assert!(err.contains("refused narrow") && err.contains("nothing was blessed"), "{err}");
    assert!(!dir.join("baselines").exists());
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}
