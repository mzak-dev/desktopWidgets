//! The semantic diff of two dump texts: what moved, what was reworded, what changed colour.
//!
//! Both texts (the blessed baseline and this run's `SceneDump::to_text`) are read back into
//! nodes keyed by their full key, so a change is named by the node it is on, not by a line
//! number. Each change has a class: `geometry` (rect, clip, scroll, line count, radius,
//! shape sizes), `text` (the string, size, weight, face, wrap; for an image its picture and
//! fit), `colour` (fill, border, shadow, text colour, opacity, tint), `hit` (what a click or a
//! drop does), `flag` (a layout flag that appeared or went), `structure` (a node added or
//! removed) and `env` (the pins and environment named in the header). Node keys are
//! index-based, so a sibling inserted early renumbers the ones after it: when more than half
//! of a subtree's nodes changed it is shown as one plain line diff (names ignored, so the
//! renumbering itself is not a change) instead of a wall of per-key lines.
//!
//! Nothing here looks at the machine: the diff is a function of the two texts. The caller
//! compares the font hashes of the two headers first; a diff across font sets would report
//! text metrics as geometry.

use std::collections::{HashMap, HashSet};

use super::dump::n;

/// What kind of change a diff line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Geometry,
    Text,
    Colour,
    Hit,
    Flag,
    Structure,
    Env,
}

/// How many changes of each class a diff found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub geometry: usize,
    pub text: usize,
    pub colour: usize,
    pub hit: usize,
    pub flags_added: usize,
    pub flags_removed: usize,
    pub structure_added: usize,
    pub structure_removed: usize,
    pub env: usize,
}

impl Counts {
    pub fn total(&self) -> usize {
        self.geometry + self.text + self.colour + self.hit + self.flags_added + self.flags_removed + self.structure_added + self.structure_removed + self.env
    }

    /// Something changed, and all of it is colour (what a palette tweak does to every scene).
    pub fn only_colour(&self) -> bool {
        self.colour > 0 && self.total() == self.colour
    }

    /// `geometry 2, text 1, colour 0, hit 0, flags +0 -0`, then `structure` and `env` when
    /// they are not zero.
    pub fn summary(&self) -> String {
        let mut s = format!("geometry {}, text {}, colour {}, hit {}, flags +{} -{}", self.geometry, self.text, self.colour, self.hit, self.flags_added, self.flags_removed);
        if self.structure_added + self.structure_removed > 0 {
            s.push_str(&format!(", structure +{} -{}", self.structure_added, self.structure_removed));
        }
        if self.env > 0 {
            s.push_str(&format!(", env {}", self.env));
        }
        s
    }
}

/// The result of comparing two dumps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diff {
    pub counts: Counts,
    /// One line per change, indented two spaces; empty when the dumps are the same.
    pub lines: Vec<String>,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.counts.total() == 0
    }
}

/// One node line of a dump, read back.
#[derive(Clone, Debug)]
struct PNode {
    key: String,
    depth: usize,
    name: String,
    kind: String,
    rect: [f64; 4],
    /// Everything after the rect: `name=value` pairs and bare words, in order; the text of a
    /// text node, its size, weight and colour are named `text`, `size`, `weight`, `colour`.
    attrs: Vec<(String, String)>,
    /// The line as written, indentation and all.
    line: String,
}

impl PNode {
    /// The line after the name: what stays the same when a sibling insert renumbers the node.
    fn rest(&self) -> &str {
        self.line.trim_start().split_once(' ').map_or("", |(_, r)| r)
    }
}

#[derive(Debug, Default)]
struct Parsed {
    /// Header fields: `scene`, `target`, `card`, ... `hermetic`, `pins`, ... `fonts`.
    header: Vec<(String, String)>,
    nodes: Vec<PNode>,
    /// `size`, then `scroll <key> <axis>` with the view and content sizes.
    content: Vec<(String, String)>,
    /// Key and the rest of the line split on two spaces: rect, clip, act...
    hits: Vec<(String, Vec<String>)>,
    draw: String,
    flags: Vec<(String, String, String)>,
}

/// Splits on whitespace, keeping a quoted string (`"a b"`, with `\"` inside) in one piece
/// together with what is glued to it.
fn tokens(s: &str) -> Vec<String> {
    let (mut out, mut cur) = (Vec::new(), String::new());
    let (mut quoted, mut esc, mut any) = (false, false, false);
    for c in s.chars() {
        if quoted {
            cur.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                quoted = false;
            }
        } else if c.is_whitespace() {
            if any {
                out.push(std::mem::take(&mut cur));
                any = false;
            }
        } else {
            if c == '"' {
                quoted = true;
            }
            cur.push(c);
            any = true;
        }
    }
    if any {
        out.push(cur);
    }
    out
}

fn unquote(s: &str) -> String {
    s.strip_prefix('"').and_then(|r| r.strip_suffix('"')).unwrap_or(s).to_string()
}

fn is_flag(t: &str) -> bool {
    t.len() > 1 && t.chars().all(|c| c.is_ascii_uppercase() || c == '-')
}

fn pair(a: &str, sep: char) -> Option<(f64, f64)> {
    let (x, y) = a.split_once(sep)?;
    Some((x.parse().ok()?, y.parse().ok()?))
}

fn parse_node(line: &str, stack: &mut Vec<String>, seen: &mut HashMap<String, usize>) -> Result<PNode, String> {
    let body = line.trim_start();
    let depth = (line.len() - body.len()) / 2;
    let toks = tokens(body);
    let bad = || format!("not a node line of a dump: `{line}`");
    if toks.len() < 4 {
        return Err(bad());
    }
    let ((x, y), (w, h)) = (pair(&toks[2], ',').ok_or_else(bad)?, pair(&toks[3], 'x').ok_or_else(bad)?);
    let (name, kind) = (toks[0].clone(), toks[1].clone());
    let text_node = kind == "text";
    let mut custom = None;
    let mut attrs: Vec<(String, String)> = Vec::new();
    for t in &toks[4..] {
        if let Some(v) = t.strip_prefix("key=") {
            custom = Some(unquote(v));
        } else if text_node && t.starts_with('"') {
            attrs.push(("text".into(), t.clone()));
        } else if text_node && t.strip_suffix("px").is_some_and(|v| v.parse::<f64>().is_ok()) {
            attrs.push(("size".into(), t.clone()));
        } else if text_node && t.strip_prefix('w').is_some_and(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit())) {
            attrs.push(("weight".into(), t.clone()));
        } else if text_node && t.starts_with('#') {
            attrs.push(("colour".into(), t.clone()));
        } else if text_node && ["left", "center", "right"].contains(&t.as_str()) {
            attrs.push(("align".into(), t.clone()));
        } else if text_node && ["wrap", "nowrap"].contains(&t.as_str()) {
            attrs.push(("wrap".into(), t.clone()));
        } else if let Some((k, v)) = t.split_once('=') {
            attrs.push((k.to_string(), v.to_string()));
        } else if is_flag(t) {
            // flags are compared from the trailer
        } else {
            attrs.push((t.clone(), String::new()));
        }
    }
    stack.truncate(depth);
    let mut key = custom.unwrap_or_else(|| stack.last().map_or_else(|| name.clone(), |p| format!("{p}/{name}")));
    let count = seen.entry(key.clone()).or_insert(0);
    *count += 1;
    if *count > 1 {
        key = format!("{key}#{count}");
    }
    stack.push(key.clone());
    Ok(PNode { key, depth, name, kind, rect: [x, y, w, h], attrs, line: line.to_string() })
}

fn parse(text: &str) -> Result<Parsed, String> {
    let mut p = Parsed::default();
    let mut section = "";
    let (mut stack, mut seen) = (Vec::new(), HashMap::new());
    for line in text.lines() {
        if line.is_empty() || line.starts_with("note ") || line.starts_with("wayfinder-dump") {
            continue;
        }
        if let Some(name) = line.strip_prefix("--- ") {
            section = match name.split_whitespace().next().unwrap_or("") {
                "content" => "content",
                "hits" => "hits",
                "draw" => "draw",
                "flags" => "flags",
                other => return Err(format!("unknown section `--- {other}` in a dump")),
            };
            continue;
        }
        match section {
            "" if line.starts_with("scene ") => {
                for (i, part) in line.split("   ").enumerate() {
                    match part.split_once(' ') {
                        Some((k, v)) if i > 0 || k == "scene" => p.header.push((k.to_string(), v.trim().to_string())),
                        _ => {}
                    }
                }
            }
            "" if line.starts_with("env ") => {
                let rest = &line[4..];
                let at = rest.find(" pins=").ok_or("the env line of a dump has no pins=")?;
                p.header.push(("hermetic".into(), rest[..at].to_string()));
                for t in rest[at + 1..].split_whitespace() {
                    if let Some((k, v)) = t.split_once('=') {
                        p.header.push((k.to_string(), v.to_string()));
                    }
                }
            }
            "" => p.nodes.push(parse_node(line, &mut stack, &mut seen)?),
            "content" => {
                let rest = line.strip_prefix("content ").ok_or("the content line of a dump")?;
                let (size, scrolls) = rest.split_once("   scrolls: ").unwrap_or((rest, "none"));
                p.content.push(("size".into(), size.trim().to_string()));
                if scrolls != "none" {
                    for s in scrolls.split("; ") {
                        let mut t = s.splitn(3, ' ');
                        let (key, axis, rest) = (t.next().unwrap_or(""), t.next().unwrap_or(""), t.next().unwrap_or(""));
                        p.content.push((format!("scroll {key} {axis}"), rest.to_string()));
                    }
                }
            }
            "hits" => {
                let (key, rest) = line.split_once("  ").unwrap_or((line, ""));
                p.hits.push((key.to_string(), rest.split("  ").map(str::to_string).collect()));
            }
            "draw" => p.draw = line.to_string(),
            "flags" => {
                let (flag, rest) = line.split_once(' ').unwrap_or((line, ""));
                let (key, detail) = rest.trim_start().split_once("  ").unwrap_or((rest.trim(), ""));
                p.flags.push((flag.to_string(), key.to_string(), detail.to_string()));
            }
            _ => {}
        }
    }
    if p.header.is_empty() {
        return Err("not a dump: no `scene` line".into());
    }
    Ok(p)
}

fn class_of(kind: &str, name: &str, value: &str) -> Class {
    if kind.starts_with("shape") {
        return if value.starts_with('#') { Class::Colour } else { Class::Geometry };
    }
    match name {
        "fill" | "grad" | "border" | "shadow" | "hover" | "op" | "tint" | "colour" => Class::Colour,
        "act" | "drop" | "slide" | "hit" => Class::Hit,
        "text" | "size" | "weight" | "want" | "got" | "align" | "wrap" | "caret" | "lh" | "tofu" | "id" | "natural" | "fit" | "shown" | "feather" | "fade" | "play" | "frame" | "ready" | "resident" => Class::Text,
        _ => Class::Geometry,
    }
}

fn rect_text(r: [f64; 4]) -> String {
    format!("{},{} {}x{}", n(r[0]), n(r[1]), n(r[2]), n(r[3]))
}

/// "moved down 6, taller by 4": how a rect changed, in words.
fn rect_note(a: [f64; 4], b: [f64; 4]) -> String {
    let mut parts = Vec::new();
    let by = |v: f64| n(v.abs());
    let (dx, dy, dw, dh) = (b[0] - a[0], b[1] - a[1], b[2] - a[2], b[3] - a[3]);
    if dx.abs() >= 0.005 {
        parts.push(format!("moved {} {}", if dx > 0.0 { "right" } else { "left" }, by(dx)));
    }
    if dy.abs() >= 0.005 {
        parts.push(format!("moved {} {}", if dy > 0.0 { "down" } else { "up" }, by(dy)));
    }
    if dw.abs() >= 0.005 {
        parts.push(format!("{} by {}", if dw > 0.0 { "wider" } else { "narrower" }, by(dw)));
    }
    if dh.abs() >= 0.005 {
        parts.push(format!("{} by {}", if dh > 0.0 { "taller" } else { "shorter" }, by(dh)));
    }
    parts.join(", ")
}

#[derive(Default)]
struct Out {
    counts: Counts,
    lines: Vec<String>,
}

impl Out {
    fn put(&mut self, class: Class, line: String) {
        match class {
            Class::Geometry => self.counts.geometry += 1,
            Class::Text => self.counts.text += 1,
            Class::Colour => self.counts.colour += 1,
            Class::Hit => self.counts.hit += 1,
            Class::Env => self.counts.env += 1,
            Class::Flag | Class::Structure => unreachable!("counted by their own methods"),
        }
        self.lines.push(format!("  {line}"));
    }

    fn added(&mut self, line: String) {
        self.counts.structure_added += 1;
        self.lines.push(format!("  + {line}"));
    }

    fn removed(&mut self, line: String) {
        self.counts.structure_removed += 1;
        self.lines.push(format!("  - {line}"));
    }
}

fn node_changes(a: &PNode, b: &PNode, out: &mut Out) {
    let key = &b.key;
    if a.kind != b.kind {
        out.removed(format!("{key}  {}", a.rest()));
        out.added(format!("{key}  {}", b.rest()));
        return;
    }
    if a.rect != b.rect {
        let note = rect_note(a.rect, b.rect);
        out.put(Class::Geometry, format!("~ {key}  rect  {} -> {}{}", rect_text(a.rect), rect_text(b.rect), if note.is_empty() { String::new() } else { format!("   ({note})") }));
    }
    let mut names: Vec<&str> = Vec::new();
    for (k, _) in a.attrs.iter().chain(&b.attrs) {
        if !names.contains(&k.as_str()) {
            names.push(k);
        }
    }
    for name in names {
        fn get<'a>(n: &'a PNode, name: &str) -> Option<&'a str> {
            n.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
        }
        let (old, new) = (get(a, name), get(b, name));
        if old == new {
            continue;
        }
        let show =|v: Option<&str>| match v {
            None => "(none)".to_string(),
            Some("") => "on".to_string(),
            Some(v) => v.to_string(),
        };
        let class = class_of(&b.kind, name, new.or(old).unwrap_or(""));
        out.put(class, format!("~ {key}  {name}  {} -> {}", show(old), show(new)));
    }
}

/// Keys of the subtree rooted at `root` (itself included).
fn under(root: &str, key: &str) -> bool {
    key == root || (key.len() > root.len() && key.starts_with(root) && key.as_bytes()[root.len()] == b'/')
}

/// Which lines of `a` and `b` a longest common subsequence keeps: `(i, j)` pairs.
fn lcs(a: &[String], b: &[String]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    if n == 0 || m == 0 || n.saturating_mul(m) > 4_000_000 {
        return Vec::new();
    }
    let mut t = vec![0u32; (n + 1) * (m + 1)];
    let w = m + 1;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i * w + j] = if a[i] == b[j] { t[(i + 1) * w + j + 1] + 1 } else { t[(i + 1) * w + j].max(t[i * w + j + 1]) };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if t[(i + 1) * w + j] >= t[i * w + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// The nodes of the subtree rooted at `root`, in order.
fn subtree<'a>(nodes: &'a [PNode], root: &str) -> Vec<&'a PNode> {
    nodes.iter().filter(|q| under(root, &q.key)).collect()
}

/// A subtree as a plain line diff: the lines compare by depth and everything after the
/// name, so nodes that only moved to another index are the same line.
fn subtree_diff(root: &str, old: &[&PNode], new: &[&PNode], out: &mut Out) {
    let norm = |p: &&PNode| format!("{} {}", p.depth, p.rest());
    let (a, b): (Vec<String>, Vec<String>) = (old.iter().map(norm).collect(), new.iter().map(norm).collect());
    let keep = lcs(&a, &b);
    let (mut i, mut j) = (0, 0);
    let mut lines = Vec::new();
    let (mut gone, mut came) = (0, 0);
    for (ki, kj) in keep.iter().copied().chain([(a.len(), b.len())]) {
        while i < ki {
            lines.push(format!("    - {}", old[i].line.trim_end()));
            gone += 1;
            i += 1;
        }
        while j < kj {
            lines.push(format!("    + {}", new[j].line.trim_end()));
            came += 1;
            j += 1;
        }
        i += 1;
        j += 1;
    }
    out.lines.push(format!("  @@ {root}  rewritten: {gone} lines gone, {came} new (shown as a line diff; renumbered nodes are not changes)"));
    out.counts.structure_removed += gone;
    out.counts.structure_added += came;
    out.lines.extend(lines);
}

fn header_diff(a: &Parsed, b: &Parsed, out: &mut Out) {
    let mut names: Vec<&str> = Vec::new();
    for (k, _) in a.header.iter().chain(&b.header) {
        if k != "scene" && !names.contains(&k.as_str()) {
            names.push(k);
        }
    }
    for name in names {
        let get = |p: &Parsed| p.header.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()).unwrap_or_else(|| "(none)".into());
        let (old, new) = (get(a), get(b));
        if old == new {
            continue;
        }
        let class = match name {
            "card" | "window" | "scale" => Class::Geometry,
            "target" => Class::Structure,
            _ => Class::Env,
        };
        let line = format!("~ (scene)  {name}  {old} -> {new}");
        if class == Class::Structure {
            out.counts.structure_removed += 1;
            out.counts.structure_added += 1;
            out.lines.push(format!("  {line}"));
        } else {
            out.put(class, line);
        }
    }
}

/// Compares a baseline with a new dump, both as the text of `SceneDump::to_text`.
pub fn diff(old: &str, new: &str) -> Result<Diff, String> {
    let (a, b) = (parse(old).map_err(|e| format!("baseline: {e}"))?, parse(new).map_err(|e| format!("this run: {e}"))?);
    let mut out = Out::default();
    header_diff(&a, &b, &mut out);

    let old_at: HashMap<&str, usize> = a.nodes.iter().enumerate().map(|(i, p)| (p.key.as_str(), i)).collect();
    let new_at: HashMap<&str, usize> = b.nodes.iter().enumerate().map(|(i, p)| (p.key.as_str(), i)).collect();
    let touched = |key: &str| match (old_at.get(key), new_at.get(key)) {
        // a node on both sides under the same key, kind and text is the same node, however far
        // it moved or whatever colour it took (a padding or palette change moves or paints a
        // whole subtree and that is not a renumbering); a different kind or text under the
        // same key means the indices shifted
        (Some(&i), Some(&j)) => {
            let text = |p: &PNode| p.attrs.iter().find(|(k, _)| k == "text").map(|(_, v)| v.clone());
            let (x, y) = (&a.nodes[i], &b.nodes[j]);
            x.kind != y.kind || text(x) != text(y)
        }
        _ => true,
    };

    // how much of each subtree changed: a node counts for every ancestor key that exists
    let mut total: HashMap<&str, (usize, usize)> = HashMap::new();
    let mut all: Vec<&str> = b.nodes.iter().map(|p| p.key.as_str()).collect();
    all.extend(a.nodes.iter().map(|p| p.key.as_str()).filter(|k| !new_at.contains_key(k)));
    for key in &all {
        let t = touched(key);
        for (at, _) in key.match_indices('/') {
            let anc = &key[..at];
            if old_at.contains_key(anc) && new_at.contains_key(anc) {
                let e = total.entry(anc).or_default();
                e.0 += 1;
                e.1 += usize::from(t);
            }
        }
    }
    let mut roots: Vec<&str> = total.iter().filter(|(_, (count, hit))| *count >= 8 && hit * 2 > *count).map(|(k, _)| *k).collect();
    roots.sort_by_key(|k| (k.matches('/').count(), *k));
    let mut collapsed: Vec<&str> = Vec::new();
    for r in roots {
        if !collapsed.iter().any(|c| under(c, r)) {
            collapsed.push(r);
        }
    }

    let covered = |key: &str| collapsed.iter().find(|c| under(c, key)).copied();
    for p in &b.nodes {
        if let Some(root) = covered(&p.key) {
            if root == p.key {
                subtree_diff(root, &subtree(&a.nodes, root), &subtree(&b.nodes, root), &mut out);
            }
            continue;
        }
        match old_at.get(p.key.as_str()) {
            Some(&i) => node_changes(&a.nodes[i], p, &mut out),
            None => out.added(format!("{}  {}", p.key, p.rest())),
        }
    }
    for p in &a.nodes {
        if !new_at.contains_key(p.key.as_str()) && covered(&p.key).is_none() {
            out.removed(format!("{}  {}", p.key, p.rest()));
        }
    }

    // trailers
    if a.content != b.content {
        let names: Vec<&String> = a.content.iter().chain(&b.content).map(|(k, _)| k).collect();
        let mut seen: HashSet<&String> = HashSet::new();
        for name in names.into_iter().filter(|k| seen.insert(k)) {
            let get = |p: &Parsed| p.content.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()).unwrap_or_else(|| "(none)".into());
            let (old, new) = (get(&a), get(&b));
            if old != new {
                out.put(Class::Geometry, format!("~ (content)  {name}  {old} -> {new}"));
            }
        }
    }
    if collapsed.is_empty() {
        let clip_of = |h: &(String, Vec<String>)| h.1.iter().find(|p| p.starts_with("clip")).cloned().unwrap_or_default();
        let old_hits: HashMap<&str, &(String, Vec<String>)> = a.hits.iter().map(|h| (h.0.as_str(), h)).collect();
        for h in &b.hits {
            if let Some(o) = old_hits.get(h.0.as_str()) {
                let (c0, c1) = (clip_of(o), clip_of(h));
                if c0 != c1 {
                    out.put(Class::Hit, format!("~ {}  hit {c0} -> {c1}", h.0));
                }
            }
        }
        let common = |hits: &[(String, Vec<String>)], other: &[(String, Vec<String>)]| hits.iter().filter(|h| other.iter().any(|o| o.0 == h.0)).map(|h| h.0.clone()).collect::<Vec<_>>();
        if common(&a.hits, &b.hits) != common(&b.hits, &a.hits) {
            out.put(Class::Hit, "~ (hits)  the order of the hit regions changed (the later one wins a click)".into());
        }
    }
    let flag_key = |f: &(String, String, String)| (f.0.clone(), f.1.clone());
    for f in &b.flags {
        match a.flags.iter().find(|o| flag_key(o) == flag_key(f)) {
            None => {
                out.counts.flags_added += 1;
                out.lines.push(format!("  + {}  {}  {}", f.0, f.1, f.2));
            }
            Some(o) if o.2 != f.2 => {
                out.counts.flags_added += 1;
                out.counts.flags_removed += 1;
                out.lines.push(format!("  ~ {}  {}  {} -> {}", f.0, f.1, o.2, f.2));
            }
            Some(_) => {}
        }
    }
    for f in &a.flags {
        if !b.flags.iter().any(|o| flag_key(o) == flag_key(f)) {
            out.counts.flags_removed += 1;
            out.lines.push(format!("  - {}  {}  {}", f.0, f.1, f.2));
        }
    }
    if a.draw != b.draw && out.counts.total() == 0 {
        out.counts.structure_added += 1;
        out.counts.structure_removed += 1;
        out.lines.push(format!("  ~ (draw)  {} -> {}", a.draw, b.draw));
    }
    // the texts differ in a way no class above names (a hit region's rect, a draw count): never
    // report "no change" for different text, show the plain line diff
    if out.counts.total() == 0 && old.trim() != new.trim() {
        let (x, y): (Vec<String>, Vec<String>) = (old.lines().map(str::to_string).collect(), new.lines().map(str::to_string).collect());
        let keep = lcs(&x, &y);
        let (mut i, mut j) = (0, 0);
        out.lines.push("  @@ (dump)  differs outside the classes above; as a line diff".into());
        for (ki, kj) in keep.iter().copied().chain([(x.len(), y.len())]) {
            while i < ki {
                out.counts.structure_removed += 1;
                out.lines.push(format!("    - {}", x[i]));
                i += 1;
            }
            while j < kj {
                out.counts.structure_added += 1;
                out.lines.push(format!("    + {}", y[j]));
                j += 1;
            }
            i += 1;
            j += 1;
        }
    }
    Ok(Diff { counts: out.counts, lines: out.lines })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::dump::{Kind, SceneDump, fixtures};

    fn text(d: &SceneDump) -> String {
        d.to_text()
    }

    fn base() -> SceneDump {
        let mut d = fixtures::tiny_dump(&["Thu 15 Jan", "10:10"]);
        d.nodes[1].rect = [60.0, 150.0, 90.0, 18.0];
        d
    }

    #[test]
    fn the_same_dump_has_no_diff_and_a_dump_reads_back() {
        let d = base();
        let t = text(&d);
        let diff = diff(&t, &t).unwrap();
        assert!(diff.is_empty() && diff.lines.is_empty(), "{diff:?}");
        let p = parse(&t).unwrap();
        assert_eq!(p.nodes.iter().map(|n| n.key.as_str()).collect::<Vec<_>>(), ["w", "w/0", "w/1"]);
        assert_eq!(p.nodes[1].rect, [60.0, 150.0, 90.0, 18.0]);
        assert_eq!(p.header.iter().find(|(k, _)| k == "card").map(|(_, v)| v.as_str()), Some("10x10"));
        assert_eq!(p.hits[0].0, "w/c/b");
    }

    #[test]
    fn a_moved_node_is_named_by_its_key_and_classed_as_geometry() {
        let (a, mut b) = (base(), base());
        b.nodes[1].rect[1] = 156.0;
        b.nodes[2].rect[2] = 120.0;
        let d = diff(&text(&a), &text(&b)).unwrap();
        assert_eq!((d.counts.geometry, d.counts.total()), (2, 2), "{d:?}");
        assert_eq!(d.lines[0], "  ~ w/0  rect  60,150 90x18 -> 60,156 90x18   (moved down 6)");
        assert!(d.lines[1].contains("w/1  rect") && d.lines[1].ends_with("(wider by 110)"), "{:?}", d.lines);
        assert!(d.counts.summary().starts_with("geometry 2, text 0, colour 0, hit 0, flags +0 -0"));
    }

    #[test]
    fn text_colour_and_hit_changes_are_classed_apart() {
        let (a, mut b) = (base(), base());
        if let Kind::Text(t) = &mut b.nodes[1].kind {
            t.text = "Thursday 15 Jan".into();
            t.color = "#ffffff80".into();
            t.size = 14.0;
        }
        b.nodes[2].fill = Some("#1c2333e6".into());
        b.nodes[2].act = Some("tog:x".into());
        let d = diff(&text(&a), &text(&b)).unwrap();
        assert_eq!((d.counts.text, d.counts.colour, d.counts.hit, d.counts.geometry), (2, 2, 1, 0), "{d:?}");
        assert!(d.lines.iter().any(|l| l == "  ~ w/0  text  \"Thu 15 Jan\" -> \"Thursday 15 Jan\""), "{:?}", d.lines);
        assert!(d.lines.iter().any(|l| l == "  ~ w/1  fill  (none) -> #1c2333e6"), "{:?}", d.lines);
        assert!(d.lines.iter().any(|l| l == "  ~ w/1  act  (none) -> \"tog:x\""), "{:?}", d.lines);
        assert!(!d.counts.only_colour());
    }

    #[test]
    fn a_colour_only_change_says_so() {
        let (a, mut b) = (base(), base());
        b.nodes[0].fill = Some("#00000040".into());
        b.nodes[2].op = Some(0.5);
        let d = diff(&text(&a), &text(&b)).unwrap();
        assert_eq!((d.counts.colour, d.counts.total(), d.counts.only_colour()), (2, 2, true), "{d:?}");
    }

    #[test]
    fn nodes_added_and_removed_and_flags_that_come_and_go_are_counted() {
        let (a, mut b) = (base(), base());
        b.nodes.push(fixtures::text("w/2", 1, "Alarm off"));
        b.nodes.remove(2);
        b.nodes[1].flags = vec!["TRUNCATED".into()];
        b.flags = vec![crate::scene::dump::FlagRow { flag: "TRUNCATED".into(), key: "w/0".into(), detail: "nat 149 > box 90".into() }];
        let d = diff(&text(&a), &text(&b)).unwrap();
        assert_eq!((d.counts.structure_added, d.counts.structure_removed, d.counts.flags_added, d.counts.flags_removed), (1, 1, 1, 0), "{d:?}");
        assert!(d.lines.iter().any(|l| l.starts_with("  + w/2  text 0,0 10x10 \"Alarm off\"")), "{:?}", d.lines);
        assert!(d.lines.iter().any(|l| l.starts_with("  - w/1  text")), "{:?}", d.lines);
        assert!(d.lines.iter().any(|l| l == "  + TRUNCATED  w/0  nat 149 > box 90"), "{:?}", d.lines);
        let back = diff(&text(&b), &text(&a)).unwrap();
        assert_eq!((back.counts.flags_added, back.counts.flags_removed), (0, 1));
    }

    #[test]
    fn a_header_difference_is_an_environment_change_not_a_node_change() {
        let (a, mut b) = (base(), base());
        b.header.palette = "Aurora".into();
        b.header.card = [10.0, 12.0];
        let d = diff(&text(&a), &text(&b)).unwrap();
        assert_eq!((d.counts.env, d.counts.geometry, d.counts.total()), (1, 1, 2), "{d:?}");
        assert!(d.lines.contains(&"  ~ (scene)  palette  default -> Aurora".to_string()), "{:?}", d.lines);
    }

    #[test]
    fn a_sibling_inserted_early_is_one_line_diff_not_a_wall_of_renumbered_keys() {
        let words: Vec<String> = (0..12).map(|i| format!("word {i}")).collect();
        let refs: Vec<&str> = words.iter().map(String::as_str).collect();
        let a = fixtures::tiny_dump(&refs);
        let mut more = vec!["new first"];
        more.extend(&refs);
        let b = fixtures::tiny_dump(&more);
        let d = diff(&text(&a), &text(&b)).unwrap();
        assert_eq!((d.counts.structure_added, d.counts.structure_removed), (1, 0), "only the inserted line is a change: {d:?}");
        assert_eq!(d.counts.text, 0, "no per-key text lines");
        assert!(d.lines[0].starts_with("  @@ w  rewritten: 0 lines gone, 1 new"), "{:?}", d.lines);
        assert!(d.lines[1].starts_with("    + ") && d.lines[1].contains("\"new first\""), "{:?}", d.lines);
        assert_eq!(d.lines.len(), 2);
    }

    #[test]
    fn a_whole_subtree_that_moved_is_per_key_geometry_not_a_line_diff() {
        // a padding change moves every node of a card by the same amount: that is what
        // the diff should name, node by node
        let words: Vec<String> = (0..12).map(|i| format!("word {i}")).collect();
        let refs: Vec<&str> = words.iter().map(String::as_str).collect();
        let (a, mut b) = (fixtures::tiny_dump(&refs), fixtures::tiny_dump(&refs));
        b.nodes.iter_mut().skip(1).for_each(|n| n.rect[0] += 8.0);
        let d = diff(&text(&a), &text(&b)).unwrap();
        assert_eq!((d.counts.geometry, d.counts.structure_added, d.counts.structure_removed), (12, 0, 0), "{d:?}");
        assert!(d.lines[0].starts_with("  ~ w/0  rect") && d.lines[0].ends_with("(moved right 8)"), "{:?}", d.lines);
    }

    #[test]
    fn quoted_text_with_spaces_and_escapes_survives_the_round_trip() {
        let (a, mut b) = (fixtures::tiny_dump(&["a \"quoted\" b"]), fixtures::tiny_dump(&["a \"quoted\" b"]));
        assert!(diff(&text(&a), &text(&b)).unwrap().is_empty());
        if let Kind::Text(t) = &mut b.nodes[1].kind {
            t.text = "a \"quoted\" c".into();
        }
        let d = diff(&text(&a), &text(&b)).unwrap();
        assert_eq!(d.lines, ["  ~ w/0  text  \"a \\\"quoted\\\" b\" -> \"a \\\"quoted\\\" c\""]);
    }

    #[test]
    fn something_that_is_not_a_dump_is_an_error_naming_the_side() {
        let t = text(&base());
        assert!(diff("nonsense", &t).unwrap_err().starts_with("baseline: "));
        assert!(diff(&t, "wayfinder-dump 1\n").unwrap_err().starts_with("this run: "));
    }
}
