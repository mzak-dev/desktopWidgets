//! Module-graph check: parses production `crate::<module>` imports into a graph and
//! asserts (1) it has no import cycle and (2) every import points down or across the layer
//! table (research/07-cycle-breaking.md). A new edge that closes a cycle or points up a
//! layer fails the test. Test code (`#[cfg(test)]` items) is not counted.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Target layers (research/07-cycle-breaking.md): a module may import only modules on a
/// lower or equal layer.
const LAYERS: &[(&str, u8)] = &[
    ("anim", 0), ("color", 0), ("dialog", 0), ("draw", 0), ("images", 0), ("monitor", 0), ("native", 0), ("net", 0), ("suggest", 0), ("textspec", 0), ("value", 0),
    ("ambient", 1), ("expr", 1), ("meta", 1), ("platform", 1), ("shortcut", 1), ("text", 1), ("thumbs", 1),
    ("data", 2), ("gfx", 2), ("theme", 2),
    ("code", 3), ("elements", 3), ("icons", 3), ("workspace", 3),
    ("ui", 4),
    ("edit", 5), ("format", 5),
    ("card", 6),
    ("widgets", 7),
    ("content", 8),
    ("plugins", 9),
    ("cli", 10), ("settings", 10),
    ("app", 11),
];

type Edges = BTreeMap<(String, String), Vec<String>>;

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

fn keep(c: char) -> char {
    if c == '\n' { '\n' } else { ' ' }
}

/// Blanks comments and string/char literal contents (keeping newlines), so braces and
/// `crate::` inside them are ignored and line numbers stay right.
fn blank(src: &str) -> String {
    let b: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let n = b.get(i + 1).copied().unwrap_or('\0');
        if c == '/' && n == '/' {
            while i < b.len() && b[i] != '\n' {
                out.push(' ');
                i += 1;
            }
        } else if c == '/' && n == '*' {
            let mut depth = 0;
            while i < b.len() {
                if b[i] == '/' && b.get(i + 1) == Some(&'*') {
                    depth += 1;
                    out.push_str("  ");
                    i += 2;
                } else if b[i] == '*' && b.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    out.push_str("  ");
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(keep(b[i]));
                    i += 1;
                }
            }
        } else if c == 'r' && (n == '"' || n == '#') && (i == 0 || !(b[i - 1].is_alphanumeric() || b[i - 1] == '_')) {
            let mut j = i + 1;
            let mut hashes = 0;
            while b.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if b.get(j) != Some(&'"') {
                out.push(c);
                i += 1;
                continue;
            }
            j += 1;
            let close: Vec<char> = std::iter::once('"').chain(std::iter::repeat('#').take(hashes)).collect();
            while j < b.len() && !(b[j..].len() >= close.len() && b[j..j + close.len()] == close[..]) {
                j += 1;
            }
            let end = (j + close.len()).min(b.len());
            out.extend(b[i..end].iter().map(|&c| keep(c)));
            i = end;
        } else if c == '"' {
            out.push(' ');
            i += 1;
            while i < b.len() && b[i] != '"' {
                if b[i] == '\\' && i + 1 < b.len() {
                    out.push(keep(b[i]));
                    i += 1;
                }
                out.push(keep(b[i]));
                i += 1;
            }
            out.push(' ');
            i += 1;
        } else if c == '\'' {
            // A char literal ('x', '\n', '{') closes within a few chars; a lifetime does not.
            let close = if n == '\\' {
                b[i + 2..].iter().take(10).position(|&x| x == '\'').map(|p| i + 2 + p)
            } else if b.get(i + 2) == Some(&'\'') {
                Some(i + 2)
            } else {
                None
            };
            match close {
                Some(e) => {
                    out.extend(b[i..=e].iter().map(|&c| keep(c)));
                    i = e + 1;
                }
                None => {
                    out.push(c);
                    i += 1;
                }
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Removes every `#[cfg(test)]` item from blanked source (to its `;` or matching `}`),
/// returning the production text and the names of `#[cfg(test)] mod x;` files to skip.
fn strip_tests(text: &str) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut skipped = Vec::new();
    let mut rest = text;
    let marker = "#[cfg(test)]";
    while let Some(at) = rest.find(marker) {
        out.push_str(&rest[..at]);
        let after = &rest[at + marker.len()..];
        // The item ends at the first `;` at depth 0 or at the `}` closing the first `{`.
        let mut depth = 0i32;
        let mut end = after.len();
        for (k, ch) in after.char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = k + 1;
                        break;
                    }
                }
                ';' if depth == 0 => {
                    end = k + 1;
                    break;
                }
                _ => {}
            }
        }
        let item = &after[..end];
        out.extend(item.chars().map(keep));
        let words: Vec<&str> = item.split_whitespace().collect();
        if let Some(p) = words.iter().position(|w| *w == "mod") {
            if let Some(name) = words.get(p + 1).and_then(|w| w.strip_suffix(';')) {
                skipped.push(name.to_string());
            }
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    (out, skipped)
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Every `crate::x` and `crate::{x, y::z}` target in `text`, with its line.
fn crate_targets(text: &str) -> Vec<(String, usize)> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(p) = text[from..].find("crate::") {
        let at = from + p;
        from = at + 7;
        if text[..at].chars().next_back().is_some_and(|c| is_ident(c) || c == '$') {
            continue;
        }
        let line = text[..at].matches('\n').count() + 1;
        let rest = &text[at + 7..];
        if let Some(group) = rest.strip_prefix('{') {
            // Top-level names of a brace group: split on commas at depth 0.
            let mut depth = 0;
            let mut cur = String::new();
            let mut names = Vec::new();
            for ch in group.chars() {
                match ch {
                    '{' => {
                        depth += 1;
                        cur.push(ch);
                    }
                    '}' if depth == 0 => break,
                    '}' => {
                        depth -= 1;
                        cur.push(ch);
                    }
                    ',' if depth == 0 => names.push(std::mem::take(&mut cur)),
                    _ => cur.push(ch),
                }
            }
            names.push(cur);
            for n in names {
                let name: String = n.trim().chars().take_while(|&c| is_ident(c)).collect();
                if !name.is_empty() {
                    found.push((name, line));
                }
            }
        } else {
            let name: String = rest.chars().take_while(|&c| is_ident(c)).collect();
            if !name.is_empty() {
                found.push((name, line));
            }
        }
    }
    found
}

/// The module graph: (from, to) -> the `file:line` sites of the import.
fn graph() -> Edges {
    let src = src_dir();
    let mut files = Vec::new();
    rs_files(&src, &mut files);
    let lib = fs::read_to_string(src.join("lib.rs")).unwrap();
    let modules: BTreeSet<String> = lib
        .lines()
        .filter_map(|l| l.trim().strip_prefix("pub mod "))
        .filter_map(|l| l.strip_suffix(';'))
        .map(String::from)
        .collect();
    assert!(modules.len() > 20, "parsed too few modules from lib.rs: {modules:?}");

    // Files declared by `#[cfg(test)] mod x;` are test-only.
    let mut test_files: BTreeSet<PathBuf> = BTreeSet::new();
    let mut prod: Vec<(PathBuf, String)> = Vec::new();
    for f in &files {
        let (text, skipped) = strip_tests(&blank(&fs::read_to_string(f).unwrap()));
        let dir = if f.file_name().is_some_and(|n| n == "mod.rs" || n == "lib.rs") {
            f.parent().unwrap().to_path_buf()
        } else {
            f.with_extension("")
        };
        for s in skipped {
            test_files.insert(dir.join(format!("{s}.rs")));
            test_files.insert(dir.join(&s).join("mod.rs"));
        }
        prod.push((f.clone(), text));
    }

    let mut edges = Edges::new();
    for (f, text) in prod {
        if test_files.contains(&f) {
            continue;
        }
        let rel = f.strip_prefix(&src).unwrap();
        let first = rel.components().next().unwrap().as_os_str().to_string_lossy().into_owned();
        let from = first.trim_end_matches(".rs").to_string();
        if !modules.contains(&from) {
            continue; // lib.rs, main.rs, this file
        }
        for (to, line) in crate_targets(&text) {
            if to != from && modules.contains(&to) {
                let site = format!("{}:{line}", rel.display());
                edges.entry((from.clone(), to)).or_default().push(site);
            }
        }
    }
    edges
}

/// Strongly connected components with more than one node (Tarjan).
fn cycles(edges: &Edges) -> Vec<BTreeSet<String>> {
    struct T<'a> {
        adj: &'a BTreeMap<&'a str, Vec<&'a str>>,
        index: BTreeMap<&'a str, usize>,
        low: BTreeMap<&'a str, usize>,
        stack: Vec<&'a str>,
        on: BTreeSet<&'a str>,
        next: usize,
        out: Vec<BTreeSet<String>>,
    }
    fn go<'a>(t: &mut T<'a>, v: &'a str) {
        t.index.insert(v, t.next);
        t.low.insert(v, t.next);
        t.next += 1;
        t.stack.push(v);
        t.on.insert(v);
        for &w in t.adj.get(v).map(Vec::as_slice).unwrap_or(&[]) {
            if !t.index.contains_key(w) {
                go(t, w);
                let l = t.low[v].min(t.low[w]);
                t.low.insert(v, l);
            } else if t.on.contains(w) {
                let l = t.low[v].min(t.index[w]);
                t.low.insert(v, l);
            }
        }
        if t.low[v] == t.index[v] {
            let mut comp = BTreeSet::new();
            loop {
                let w = t.stack.pop().unwrap();
                t.on.remove(w);
                comp.insert(w.to_string());
                if w == v {
                    break;
                }
            }
            if comp.len() > 1 {
                t.out.push(comp);
            }
        }
    }
    let mut adj: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (a, b) in edges.keys() {
        adj.entry(a).or_default().push(b);
        adj.entry(b).or_default();
    }
    let mut t = T {
        adj: &adj,
        index: BTreeMap::new(),
        low: BTreeMap::new(),
        stack: vec![],
        on: BTreeSet::new(),
        next: 0,
        out: vec![],
    };
    for &v in adj.keys() {
        if !t.index.contains_key(v) {
            go(&mut t, v);
        }
    }
    t.out
}

fn describe(edges: &Edges, pairs: impl Iterator<Item = (String, String)>) -> String {
    pairs
        .map(|p| format!("  {} -> {}  ({})", p.0, p.1, edges.get(&p).map(|s| s.join(", ")).unwrap_or_default()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn module_graph_has_no_import_cycle() {
    let edges = graph();
    let sccs = cycles(&edges);
    let tying: Vec<_> = edges.keys().filter(|(a, b)| sccs.iter().any(|c| c.contains(a) && c.contains(b))).cloned().collect();
    assert!(sccs.is_empty(), "modules import each other in a cycle: {sccs:?}. Edges inside it:
{}", describe(&edges, tying.into_iter()));
}

#[test]
fn every_import_points_down_the_layer_table() {
    let edges = graph();
    let layer: BTreeMap<&str, u8> = LAYERS.iter().copied().collect();
    let mut up = BTreeSet::new();
    for (a, b) in edges.keys() {
        match (layer.get(a.as_str()), layer.get(b.as_str())) {
            (Some(la), Some(lb)) => {
                if lb > la {
                    up.insert((a.clone(), b.clone()));
                }
            }
            _ => panic!("module {a} or {b} is missing from LAYERS in src/module_graph.rs"),
        }
    }
    assert!(up.is_empty(), "imports point up the layer table:
{}", describe(&edges, up.into_iter()));
}

#[cfg(test)]
mod parser_tests {
    use super::*;

    #[test]
    fn finds_plain_grouped_and_nested_targets() {
        let t = "use crate::a::B;\nuse crate::{c, d::{E, F}, g};\nlet x = crate::h::i();\nuse super::crate_x;";
        let names: Vec<_> = crate_targets(&blank(t)).into_iter().map(|(n, l)| format!("{n}@{l}")).collect();
        assert_eq!(names, ["a@1", "c@2", "d@2", "g@2", "h@3"]);
    }

    #[test]
    fn ignores_comments_strings_and_test_items() {
        let t = "// crate::x\nlet s = \"crate::y {\";\nuse crate::keep;\n#[cfg(test)]\nmod tests { use crate::gone; }\nuse crate::after;";
        let (text, _) = strip_tests(&blank(t));
        let names: Vec<_> = crate_targets(&text).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["keep", "after"]);
    }
}
