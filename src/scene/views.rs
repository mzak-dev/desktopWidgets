//! Ways to read a `SceneDump` without reading all of it: `outline` (the console default, at
//! most 200 lines), `full` (what a baseline holds), `texts`, `hits` and `flags`, narrowed by
//! `--under KEY`, `--depth N` and `--find TEXT`.

use super::dump::{Kind, Node, SceneDump, n, quote};
use crate::suggest::suggest;

/// The outline stops after this many lines.
pub const OUTLINE_CAP: usize = 200;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    #[default]
    Outline,
    Full,
    Texts,
    Hits,
    Flags,
}

impl View {
    pub fn parse(s: &str) -> Result<View, String> {
        match s {
            "outline" => Ok(View::Outline),
            "full" => Ok(View::Full),
            "texts" => Ok(View::Texts),
            "hits" => Ok(View::Hits),
            "flags" => Ok(View::Flags),
            other => Err(format!("unknown view `{other}`{} (the views are outline, full, texts, hits, flags)", suggest(other, &[&["outline", "full", "texts", "hits", "flags"]]))),
        }
    }
}

/// What to show of a dump.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub view: View,
    /// Only the subtree under the node with this key.
    pub under: Option<String>,
    /// Only this many levels below the top of what is shown.
    pub depth: Option<u32>,
    /// Only nodes whose line holds this text (case does not matter).
    pub find: Option<String>,
}

impl Query {
    fn narrowed(&self) -> bool {
        self.under.is_some() || self.depth.is_some() || self.find.is_some()
    }
}

/// The nodes a query selects, with the depth of the top of the selection.
fn select<'a>(d: &'a SceneDump, q: &Query) -> Result<(Vec<&'a Node>, u32), String> {
    let (mut nodes, base): (Vec<&Node>, u32) = match &q.under {
        None => (d.nodes.iter().collect(), 0),
        Some(key) => {
            let Some(at) = d.nodes.iter().position(|nd| nd.key == *key) else {
                let keys: Vec<&str> = d.nodes.iter().map(|nd| nd.key.as_str()).collect();
                return Err(format!("--under: no node with the key `{key}`{}; `scene dump --view full` lists the keys", suggest(key, &[&keys])));
            };
            let top = d.nodes[at].depth;
            (d.nodes[at..].iter().take_while(|nd| std::ptr::eq(*nd, &d.nodes[at]) || nd.depth > top).collect(), top)
        }
    };
    if let Some(max) = q.depth {
        nodes.retain(|nd| nd.depth - base <= max);
    }
    if let Some(needle) = &q.find {
        let needle = needle.to_lowercase();
        nodes.retain(|nd| nd.line(true).to_lowercase().contains(&needle));
    }
    Ok((nodes, base))
}

fn informative(nd: &Node) -> bool {
    !matches!(nd.kind, Kind::Box) || nd.hit || nd.act.is_some() || nd.drop.is_some() || nd.slide.is_some() || !nd.flags.is_empty() || nd.clip.is_some() || nd.scroll_y.is_some() || nd.scroll_x.is_some()
}

fn indent(nd: &Node, base: u32) -> String {
    "  ".repeat((nd.depth - base) as usize)
}

/// The dump as `q` asks to see it.
pub fn render(d: &SceneDump, q: &Query) -> Result<String, String> {
    let (nodes, base) = select(d, q)?;
    let mut out = String::new();
    match q.view {
        View::Full => {
            out.push_str(&d.header_text());
            for nd in &nodes {
                let top = (q.under.is_some() && nd.depth == base) || q.find.is_some();
                out.push_str(&format!("{}{}\n", indent(nd, base), nd.line(top)));
            }
            if !q.narrowed() {
                out.push_str(&d.content_text());
                out.push_str(&d.hits_text());
                out.push_str(&d.draw_text());
                out.push_str(&d.flags_text());
            }
        }
        View::Outline => {
            // a search lists every match, plain boxes included
            let all = q.find.is_some();
            let shows = |nd: &Node| all || informative(nd) || (q.under.is_some() && nd.depth == base);
            out.push_str(&d.header_text());
            let (mut lines, mut i) = (0, 0);
            while i < nodes.len() {
                if lines == OUTLINE_CAP {
                    out.push_str(&format!("... {} more nodes; use --under KEY or --find TEXT\n", nodes.len() - i));
                    break;
                }
                let nd = nodes[i];
                if shows(nd) {
                    let top = (q.under.is_some() && nd.depth == base) || all;
                    out.push_str(&format!("{}{}\n", indent(nd, base), nd.line(top)));
                    i += 1;
                } else {
                    let run = nodes[i..].iter().take_while(|x| !shows(x)).count();
                    out.push_str(&format!("{}..({run})\n", indent(nd, base)));
                    i += run;
                }
                lines += 1;
            }
            out.push_str(&d.flags_text());
        }
        View::Texts => {
            for nd in &nodes {
                if let Kind::Text(t) = &nd.kind {
                    let [x, y, w, h] = nd.rect;
                    out.push_str(&format!("{}  [{},{},{}x{}]  {}\n", nd.key, n(x), n(y), n(w), n(h), quote(&t.text)));
                }
            }
        }
        View::Hits => {
            let shown: std::collections::HashSet<&str> = nodes.iter().map(|nd| nd.key.as_str()).collect();
            for h in d.hits.iter().filter(|h| !q.narrowed() || shown.contains(h.key.as_str())) {
                out.push_str(&format!("{}\n", h.line()));
            }
        }
        View::Flags => out.push_str(&d.flags_text()),
    }
    Ok(out)
}

/// The dump as JSON, with its nodes and hits narrowed by `q` (the view itself only matters to
/// the text form).
pub fn render_json(d: &SceneDump, q: &Query) -> Result<serde_json::Value, String> {
    let mut d = d.clone();
    if q.narrowed() {
        let (nodes, _) = select(&d, q)?;
        let keep: std::collections::HashSet<String> = nodes.iter().map(|nd| nd.key.clone()).collect();
        d.nodes.retain(|nd| keep.contains(&nd.key));
        d.hits.retain(|h| keep.contains(&h.key));
    }
    Ok(d.to_json())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::dump::fixtures::{dump, plain, text};

    fn sample() -> SceneDump {
        let mut b = plain("w/c/b", 2);
        b.hit = true;
        b.act = Some("x".into());
        dump(vec![plain("w", 0), plain("w/c", 1), b, plain("w/c/p1", 2), plain("w/c/p2", 2), text("w/c/p2/t", 3, "Hello World"), plain("w/c/p3", 2), text("w/d", 1, "Other")])
    }

    #[test]
    fn the_outline_collapses_runs_of_plain_boxes_and_keeps_what_carries_information() {
        let out = render(&sample(), &Query::default()).unwrap();
        let body: Vec<&str> = out.lines().skip(4).collect();
        assert_eq!(body[0], "..(2)");
        assert!(body[1].starts_with("    b box") && body[1].contains("hit act=\"x\""), "{body:?}");
        assert_eq!(body[2], "    ..(2)", "p1 and p2 are plain and follow each other");
        assert!(body[3].starts_with("      t text") && body[3].contains("\"Hello World\""), "{body:?}");
        assert_eq!(body[4], "    ..(1)");
        assert!(body[5].starts_with("  d text"));
        assert_eq!(body[6], "--- flags (0)");
    }

    #[test]
    fn the_outline_stops_at_its_cap_and_says_how_to_read_the_rest() {
        let mut nodes = vec![plain("w", 0)];
        nodes.extend((0..300).map(|i| text(&format!("w/{i}"), 1, "x")));
        let out = render(&dump(nodes), &Query::default()).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.iter().filter(|l| l.contains(" text 0,0")).count(), OUTLINE_CAP - 1, "the window box takes the first line");
        assert!(out.contains("... 101 more nodes; use --under KEY or --find TEXT"), "{out}");
    }

    #[test]
    fn full_is_header_body_and_every_trailer_in_order() {
        let out = render(&sample(), &Query { view: View::Full, ..Default::default() }).unwrap();
        let marks: Vec<&str> = out.lines().filter(|l| l.starts_with("--- ") || l.starts_with("wayfinder-dump")).collect();
        assert_eq!(marks, ["wayfinder-dump 1", "--- content", "--- hits", "--- draw", "--- flags (0)"]);
        assert_eq!(out, sample().to_text(), "the full view is the text form");
    }

    #[test]
    fn under_shows_one_subtree_from_its_own_top_and_names_the_key_it_was_given() {
        let q = Query { view: View::Full, under: Some("w/c/p2".into()), ..Default::default() };
        let out = render(&sample(), &q).unwrap();
        let body: Vec<&str> = out.lines().skip(4).collect();
        assert_eq!(body.len(), 2, "{body:?}");
        assert!(body[0].starts_with("p2 box") && body[0].contains("key=w/c/p2"), "{body:?}");
        assert!(body[1].starts_with("  t text"), "{body:?}");
        let outline = render(&sample(), &Query { under: Some("w/c/p2".into()), ..Default::default() }).unwrap();
        assert!(outline.lines().nth(4).unwrap().starts_with("p2 box"), "the top of what was asked for is never folded away: {outline}");
        let e = render(&sample(), &Query { under: Some("w/c/p".into()), ..Default::default() }).unwrap_err();
        assert!(e.contains("no node with the key `w/c/p`") && e.contains("did you mean"), "{e}");
    }

    #[test]
    fn depth_limits_levels_below_the_top_and_find_matches_lines_case_blind() {
        let d = sample();
        let out = render(&d, &Query { view: View::Full, depth: Some(1), ..Default::default() }).unwrap();
        assert!(out.contains("c box") && !out.contains("p1 box") && !out.contains("b box"));
        let out = render(&d, &Query { view: View::Full, under: Some("w/c".into()), depth: Some(1), ..Default::default() }).unwrap();
        assert!(out.contains("p1 box") && !out.contains("t text"), "{out}");
        let out = render(&d, &Query { find: Some("hello".into()), ..Default::default() }).unwrap();
        let found: Vec<&str> = out.lines().skip(4).collect();
        assert!(found.len() == 2 && found[0].contains("\"Hello World\"") && found[0].contains("key=w/c/p2/t"), "{found:?}");
    }

    #[test]
    fn texts_hits_and_flags_views_are_just_their_lines() {
        let d = sample();
        assert_eq!(render(&d, &Query { view: View::Texts, ..Default::default() }).unwrap(), "w/c/p2/t  [0,0,10x10]  \"Hello World\"\nw/d  [0,0,10x10]  \"Other\"\n");
        assert_eq!(render(&d, &Query { view: View::Hits, ..Default::default() }).unwrap(), "w/c/b  [1,2,3x4]  clip none  act=\"x\"\n");
        assert_eq!(render(&d, &Query { view: View::Flags, ..Default::default() }).unwrap(), "--- flags (0)\n");
        assert_eq!(render(&d, &Query { view: View::Texts, under: Some("w/d".into()), ..Default::default() }).unwrap(), "w/d  [0,0,10x10]  \"Other\"\n");
        assert_eq!(render(&d, &Query { view: View::Hits, find: Some("nothing".into()), ..Default::default() }).unwrap(), "");
        assert!(View::parse("outlne").unwrap_err().contains("did you mean `outline`"));
    }
}
