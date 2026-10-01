//! The Scene Dump: what a settled frame looks like as data. One `SceneDump` struct feeds the
//! line text (what is read and diffed: one node per line, pre-order, numbers to 0.01, colours
//! as u8 hex) and its JSON twin (for tools). Nothing in it needs a GPU; the node facts come
//! from the UI trace (`Frame.nodes`), the text and image facts from the engines that made them.
//!
//! Fonts are the machine's (there are no bundled ones), so a dump is exact on the machine and
//! font set it was made on: the header carries a hash of the font environment, and a reader
//! comparing two dumps should compare it first.

use serde::{Serialize, Serializer, ser::SerializeMap};
use serde_json::Value as Json;

use crate::render::{Settled, env};
use crate::textspec::TextAlign;
use crate::ui::{Frame, Placed, PlacedKind};

/// The dump format, in the first line of a dump.
pub const FORMAT: u32 = 1;
/// A text run longer than this is cut in the text form (the JSON twin keeps it whole).
const TEXT_CUT: usize = 120;
/// A clip edge this far out means nothing clips.
const UNBOUNDED: f32 = 1.0e5;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SceneDump {
    pub format: u32,
    pub header: Header,
    pub nodes: Vec<Node>,
    pub content: ContentInfo,
    pub hits: Vec<HitInfo>,
    pub draw: Vec<LayerCount>,
    pub flags: Vec<FlagRow>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Header {
    pub id: String,
    pub target: String,
    pub card: [f64; 2],
    pub window: [f64; 2],
    pub scale: f64,
    /// The virtual time of the described frame, ms after T0.
    pub at_ms: u64,
    pub hermetic: bool,
    pub not_hermetic_because: Vec<String>,
    /// A hash of every pin, so two dumps made under different pins say so.
    pub pins: String,
    pub now: String,
    pub zone: String,
    pub locale: String,
    pub anim: String,
    pub palette: String,
    /// `system:` and a hash of the machine's font faces.
    pub fonts: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Node {
    pub key: String,
    /// The last `/`-segment of the key.
    pub name: String,
    pub depth: u32,
    #[serde(flatten)]
    pub kind: Kind,
    /// Logical px in the window, to 0.01.
    pub rect: [f64; 4],
    #[serde(skip_serializing_if = "is_zero")]
    pub layer: usize,
    /// Effective opacity, when not 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub op: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grad: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub border: Option<(f64, String)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    /// Blur, dy and colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadow: Option<(f64, f64, String)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hover: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enter: Option<String>,
    /// The clip this node imposes on its children, `[x, y, w, h]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip: Option<[f64; 4]>,
    /// Offset and content size along each scrolling axis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scroll_y: Option<[f64; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scroll_x: Option<[f64; 2]>,
    #[serde(skip_serializing_if = "is_false")]
    pub hit: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub act: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drop: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slide: Option<String>,
    /// Layout flags, upper case (TRUNCATED, CLIPPED...).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
    /// The key is not `parent key + "/" + name`: the text form prints it.
    #[serde(skip)]
    pub custom_key: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Kind {
    Box,
    Text(TextFacts),
    Image(ImageFacts),
    Shape {
        shape: String,
        #[serde(serialize_with = "as_map")]
        attrs: Vec<(String, String)>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TextFacts {
    pub text: String,
    pub size: f64,
    pub weight: u16,
    /// The requested family; empty = the theme's.
    pub want: String,
    /// The faces the shaper used (several = a fallback happened); empty when never shaped.
    pub got: Vec<String>,
    pub color: String,
    pub align: String,
    pub wrap: bool,
    pub lines: Option<usize>,
    /// Natural (unwrapped) size.
    pub nat: Option<[f64; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caret: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ImageFacts {
    pub id: String,
    /// The picture's own size, once it is resident.
    pub natural: Option<[u32; 2]>,
    pub fit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feather: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fade: Option<u32>,
    pub play: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<u32>,
    /// The widget built it with its size known.
    pub ready: bool,
    /// The Image Store holds it.
    pub resident: bool,
    /// The ids drawn, when they are not just `id` (a fade shows two).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shown: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ContentInfo {
    pub size: [f64; 2],
    pub scrolls: Vec<ScrollRow>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ScrollRow {
    pub key: String,
    pub axis: String,
    pub view: f64,
    pub content: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HitInfo {
    pub key: String,
    pub rect: [f64; 4],
    /// `[x0, y0, x1, y1]`; none when nothing clips it.
    pub clip: Option<[f64; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub act: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drop: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slide: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LayerCount {
    pub layer: usize,
    pub shapes: usize,
    pub images: usize,
    pub texts: usize,
}

/// One layout flag (`TRUNCATED`, `OUTSIDE`...), naming its node.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FlagRow {
    pub flag: String,
    pub key: String,
    pub detail: String,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn as_map<S: Serializer>(v: &[(String, String)], s: S) -> Result<S::Ok, S::Error> {
    let mut m = s.serialize_map(Some(v.len()))?;
    for (k, x) in v {
        m.serialize_entry(k, x)?;
    }
    m.end()
}

/// `v` to 0.01, as the value a dump holds (never `-0`).
pub fn r2(v: f32) -> f64 {
    let r = (f64::from(v) * 100.0).round() / 100.0;
    if r == 0.0 { 0.0 } else { r }
}

/// A rounded number as text: `12`, `12.5`, `12.25`.
pub fn n(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn rect4(r: [f32; 4]) -> [f64; 4] {
    r.map(r2)
}

fn unbounded(c: &[f32; 4]) -> bool {
    c.iter().any(|v| v.abs() >= UNBOUNDED)
}

/// A quoted string with `\"`, `\\` and `\n` escaped.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn quote_cut(s: &str) -> String {
    let len = s.chars().count();
    if len <= TEXT_CUT {
        return quote(s);
    }
    let head: String = s.chars().take(TEXT_CUT).collect();
    format!("{}...({} more)", quote(&head), len - TEXT_CUT)
}

/// A value the text form may print bare: no space, quote or `=` in it.
fn bare(s: &str) -> String {
    if !s.is_empty() && !s.chars().any(|c| c.is_whitespace() || c == '"' || c == '=') { s.to_string() } else { quote(s) }
}

fn node(p: &Placed, parent_key: Option<&str>, s: &Settled) -> Node {
    let name = p.key.rsplit('/').next().unwrap_or(&p.key).to_string();
    let custom_key = match parent_key {
        Some(pk) => p.key != format!("{pk}/{name}") && p.key != name,
        None => false,
    };
    let kind = match &p.kind {
        PlacedKind::Box => Kind::Box,
        PlacedKind::Text { spec, color, run } => Kind::Text(TextFacts {
            text: spec.text.clone(),
            size: r2(spec.size),
            weight: spec.weight,
            want: spec.family.clone(),
            got: run.as_ref().map(|r| r.faces.clone()).unwrap_or_default(),
            color: color.to_hex(),
            align: match spec.align {
                TextAlign::Left => "left",
                TextAlign::Center => "center",
                TextAlign::Right => "right",
            }
            .into(),
            wrap: spec.wrap,
            lines: run.as_ref().map(|r| r.lines),
            nat: run.as_ref().map(|r| [r2(r.natural_w), r2(r.natural_h)]),
            caret: spec.caret,
            line_height: ((spec.line_height - 1.25).abs() > 1e-6).then(|| r2(spec.line_height)),
        }),
        PlacedKind::Image { spec, shown } => Kind::Image(ImageFacts {
            natural: s.images.size(&spec.id).map(|(w, h)| [w, h]),
            resident: s.images.size(&spec.id).is_some(),
            fit: format!("{:?}", spec.fit).to_lowercase(),
            tint: spec.tint.map(|c| c.to_hex()),
            feather: (spec.feather > 0.0).then(|| r2(spec.feather)),
            fade: (spec.fade > 0).then_some(spec.fade),
            play: spec.play,
            frame: spec.frame,
            ready: spec.ready,
            shown: if shown.len() == 1 && shown[0] == spec.id { vec![] } else { shown.clone() },
            id: spec.id.clone(),
        }),
        PlacedKind::Shape { name, attrs } => Kind::Shape { shape: (*name).to_string(), attrs: attrs.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect() },
    };
    let hover = {
        let mut parts = Vec::new();
        if let Some(c) = p.hover.fill {
            parts.push(format!("fill:{}", c.to_hex()));
        }
        if let Some(c) = p.hover.border_color {
            parts.push(format!("border:{}", c.to_hex()));
        }
        if let Some(o) = p.hover.opacity {
            parts.push(format!("op:{}", n(r2(o))));
        }
        if let Some(c) = p.hover.text_color {
            parts.push(format!("tc:{}", c.to_hex()));
        }
        (!parts.is_empty()).then(|| parts.join(","))
    };
    let enter = p.enter.map(|e| format!("{}ms/dy{}{}", e.ms, n(r2(e.dy)), if e.delay > 0 { format!("@{}ms", e.delay) } else { String::new() }));
    let clip = p.imposes.map(|[x0, y0, x1, y1]| rect4([x0, y0, x1 - x0, y1 - y0]));
    let axis = |a: Option<crate::ui::ScrollAxis>| a.map(|a| [r2(a.offset), r2(a.content)]);
    Node {
        key: p.key.clone(),
        name,
        depth: p.depth,
        kind,
        rect: rect4(p.rect),
        layer: p.layer,
        op: Some(r2(p.opacity)).filter(|o| *o != 1.0),
        fill: (p.look.fill.0[3] > 0.0).then(|| p.look.fill.to_hex()),
        grad: p.look.gradient_bottom.map(|c| c.to_hex()),
        border: (p.look.border > 0.0).then(|| (r2(p.look.border), p.look.border_color.to_hex())),
        radius: (p.look.radius > 0.0).then(|| r2(p.look.radius)),
        shadow: p.look.shadow.map(|sh| (r2(sh.blur), r2(sh.dy), sh.color.to_hex())),
        hover,
        enter,
        clip,
        scroll_y: axis(p.scroll_y),
        scroll_x: axis(p.scroll_x),
        hit: p.hit,
        act: p.action.clone(),
        drop: p.on_drop.clone(),
        slide: p.on_slide.clone(),
        flags: Vec::new(),
        custom_key,
    }
}

/// The dump of a settled frame. `settle` must have been called with the trace on.
pub fn build(id: &str, s: &Settled) -> SceneDump {
    let frame: &Frame = &s.prepared.frame;
    assert!(frame.nodes.len() == frame.rects.len(), "the frame was laid out without the trace");
    let mut keys: Vec<&str> = Vec::new();
    let mut nodes = Vec::with_capacity(frame.nodes.len());
    for p in &frame.nodes {
        keys.truncate(p.depth as usize);
        nodes.push(node(p, keys.last().copied(), s));
        keys.push(&p.key);
    }
    let pins = s.pins.entries();
    let entry = |k: &str| pins.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str().map_or_else(|| v.to_string(), str::to_string)).unwrap_or_default();
    let faces = s.text.font_faces();
    let facts = env::Facts { widget: &s.content.id, size: (0, 0), pins: &s.pins, real: &s.real, installed: s.content.installed, roots: &s.content.roots, faces: &faces, adapter: None, deps: &s.prepared.deps, code_sources: &s.code_sources, rounds: s.rounds };
    let because = env::leaks(&facts);
    let pins_json = serde_json::to_string(&pins).unwrap_or_default();
    SceneDump {
        format: FORMAT,
        header: Header {
            id: id.to_string(),
            target: format!("widget {}", s.content.id),
            card: [r2(s.card_size.0), r2(s.card_size.1)],
            window: [r2(s.window.0), r2(s.window.1)],
            scale: r2(s.pins.scale),
            at_ms: s.at.as_millis() as u64,
            hermetic: because.is_empty(),
            not_hermetic_because: because,
            pins: env::fnv([pins_json.as_bytes()])[..6].to_string(),
            now: entry("now"),
            zone: entry("zone"),
            locale: entry("locale"),
            anim: s.pins.anim.clone().unwrap_or_else(|| "theme".into()),
            palette: if s.pins.palette.is_empty() { "default".into() } else { s.pins.palette.clone() },
            fonts: format!("system:{}", &env::fnv(&faces)[..8]),
        },
        nodes,
        content: ContentInfo {
            size: [r2(frame.content_size.0), r2(frame.content_size.1)],
            scrolls: frame.scrolls.iter().map(|sc| ScrollRow { key: sc.key.clone(), axis: if sc.horizontal { "x" } else { "y" }.into(), view: r2(sc.view), content: r2(sc.content) }).collect(),
        },
        hits: frame
            .hits
            .iter()
            .map(|h| HitInfo { key: h.key.clone(), rect: rect4(h.rect), clip: (!unbounded(&h.clip)).then(|| rect4(h.clip)), act: h.action.clone(), drop: h.on_drop.clone(), slide: h.on_slide.clone() })
            .collect(),
        draw: frame.list.layers.iter().enumerate().map(|(layer, l)| LayerCount { layer, shapes: l.shapes.len(), images: l.images.len(), texts: l.texts.len() }).collect(),
        flags: Vec::new(),
    }
}

impl Node {
    /// The body line of the text form, without indentation. `force_key` prints the key even
    /// when the parent implies it (the top of an `--under` subtree).
    pub fn line(&self, force_key: bool) -> String {
        let [x, y, w, h] = self.rect;
        let kind = match &self.kind {
            Kind::Box => "box".to_string(),
            Kind::Text(_) => "text".to_string(),
            Kind::Image(_) => "image".to_string(),
            Kind::Shape { shape, .. } => format!("shape:{shape}"),
        };
        let mut out = format!("{} {kind} {},{} {}x{}", self.name, n(x), n(y), n(w), n(h));
        let mut put = |s: String| {
            out.push(' ');
            out.push_str(&s);
        };
        if self.custom_key || (force_key && self.key != self.name) {
            put(format!("key={}", bare(&self.key)));
        }
        if self.layer > 0 {
            put(format!("layer={}", self.layer));
        }
        if let Some(o) = self.op {
            put(format!("op={}", n(o)));
        }
        if let Some(f) = &self.fill {
            put(format!("fill={f}"));
        }
        if let Some(g) = &self.grad {
            put(format!("grad={g}"));
        }
        if let Some((w, c)) = &self.border {
            put(format!("border={}:{c}", n(*w)));
        }
        if let Some(r) = self.radius {
            put(format!("radius={}", n(r)));
        }
        if let Some((b, dy, c)) = &self.shadow {
            put(format!("shadow={}/{}:{c}", n(*b), n(*dy)));
        }
        if let Some(hv) = &self.hover {
            put(format!("hover={hv}"));
        }
        if let Some(e) = &self.enter {
            put(format!("enter={e}"));
        }
        if let Some([cx, cy, cw, ch]) = self.clip {
            put(format!("clip=[{},{},{},{}]", n(cx), n(cy), n(cw), n(ch)));
        }
        let scroll: Vec<String> = [("y", self.scroll_y), ("x", self.scroll_x)].iter().filter_map(|(a, v)| v.map(|[o, c]| format!("{a}:{}/{}", n(o), n(c)))).collect();
        if !scroll.is_empty() {
            put(format!("scroll={}", scroll.join(",")));
        }
        if self.hit {
            put("hit".into());
        }
        for (k, v) in [("act", &self.act), ("drop", &self.drop), ("slide", &self.slide)] {
            if let Some(v) = v {
                put(format!("{k}={}", quote(v)));
            }
        }
        match &self.kind {
            Kind::Box => {}
            Kind::Text(t) => {
                put(quote_cut(&t.text));
                put(format!("{}px", n(t.size)));
                put(format!("w{}", t.weight));
                put(format!("want={}", if t.want.is_empty() { "theme".to_string() } else { quote(&t.want) }));
                put(format!("got={}", if t.got.is_empty() { "none".to_string() } else { quote(&t.got.join("+")) }));
                put(t.color.clone());
                put(t.align.clone());
                put(if t.wrap { "wrap" } else { "nowrap" }.into());
                if let Some(l) = t.lines {
                    put(format!("lines={l}"));
                }
                if let Some([nw, nh]) = t.nat {
                    put(format!("nat={}x{}", n(nw), n(nh)));
                }
                if let Some(c) = t.caret {
                    put(format!("caret={c}"));
                }
                if let Some(lh) = t.line_height {
                    put(format!("lh={}", n(lh)));
                }
            }
            Kind::Image(i) => {
                put(format!("id={}", bare(&i.id)));
                if let Some([nw, nh]) = i.natural {
                    put(format!("natural={nw}x{nh}"));
                }
                put(format!("fit={}", i.fit));
                if let Some(t) = &i.tint {
                    put(format!("tint={t}"));
                }
                if let Some(f) = i.feather {
                    put(format!("feather={}", n(f)));
                }
                if let Some(f) = i.fade {
                    put(format!("fade={f}"));
                }
                if i.play {
                    put("play".into());
                }
                if let Some(f) = i.frame {
                    put(format!("frame={f}"));
                }
                if !i.shown.is_empty() {
                    put(format!("shown={}", quote(&i.shown.join("+"))));
                }
                put(format!("ready={}", i.ready));
                put(format!("resident={}", i.resident));
            }
            Kind::Shape { attrs, .. } => {
                for (k, v) in attrs {
                    put(format!("{k}={}", bare(v)));
                }
            }
        }
        for f in &self.flags {
            put(f.clone());
        }
        out
    }
}

impl HitInfo {
    pub fn line(&self) -> String {
        let [x, y, w, h] = self.rect;
        let mut out = format!("{}  [{},{},{}x{}]", self.key, n(x), n(y), n(w), n(h));
        match self.clip {
            Some([a, b, c, d]) => out.push_str(&format!("  clip [{},{},{},{}]", n(a), n(b), n(c), n(d))),
            None => out.push_str("  clip none"),
        }
        for (k, v) in [("act", &self.act), ("drop", &self.drop), ("slide", &self.slide)] {
            if let Some(v) = v {
                out.push_str(&format!("  {k}={}", quote(v)));
            }
        }
        out
    }
}

impl SceneDump {
    /// The first lines: format, scene, environment and the note about fonts.
    pub fn header_text(&self) -> String {
        let h = &self.header;
        let env = if h.hermetic { "hermetic".to_string() } else { format!("NOT-HERMETIC ({})", h.not_hermetic_because.join("; ")) };
        format!(
            "wayfinder-dump {}\nscene {}   target {}   card {}x{}   window {}x{}   scale {}   at {}ms\nenv {env} pins={} now={} zone={} locale={} anim={} palette={} fonts={}\nnote fonts=system: text metrics are this machine's; a dump is exact on the same machine and font set only\n",
            self.format,
            h.id,
            h.target,
            n(h.card[0]),
            n(h.card[1]),
            n(h.window[0]),
            n(h.window[1]),
            n(h.scale),
            h.at_ms,
            h.pins,
            h.now,
            h.zone,
            h.locale,
            h.anim,
            h.palette,
            h.fonts
        )
    }

    pub fn content_text(&self) -> String {
        let scrolls = if self.content.scrolls.is_empty() { "none".to_string() } else { self.content.scrolls.iter().map(|s| format!("{} {} view={} content={}", s.key, s.axis, n(s.view), n(s.content))).collect::<Vec<_>>().join("; ") };
        format!("--- content\ncontent {}x{}   scrolls: {scrolls}\n", n(self.content.size[0]), n(self.content.size[1]))
    }

    pub fn hits_text(&self) -> String {
        let mut out = String::from("--- hits\n");
        self.hits.iter().for_each(|h| out.push_str(&format!("{}\n", h.line())));
        out
    }

    pub fn draw_text(&self) -> String {
        let cells: Vec<String> = self.draw.iter().map(|l| format!("layer{} shapes={} images={} texts={}", l.layer, l.shapes, l.images, l.texts)).collect();
        format!("--- draw\n{}\n", cells.join("   "))
    }

    pub fn flags_text(&self) -> String {
        let mut out = format!("--- flags ({})\n", self.flags.len());
        for f in &self.flags {
            out.push_str(&format!("{:<10} {}  {}\n", f.flag, f.key, f.detail));
        }
        out
    }

    /// The whole text form: what a baseline holds.
    pub fn to_text(&self) -> String {
        let mut out = self.header_text();
        for nd in &self.nodes {
            out.push_str(&"  ".repeat(nd.depth as usize));
            out.push_str(&nd.line(false));
            out.push('\n');
        }
        out.push_str(&self.content_text());
        out.push_str(&self.hits_text());
        out.push_str(&self.draw_text());
        out.push_str(&self.flags_text());
        out
    }

    pub fn to_json(&self) -> Json {
        serde_json::to_value(self).expect("a dump serialises")
    }

    /// Every text run, in order, with its node key: the cheapest read of the copy.
    pub fn texts(&self) -> Vec<(&str, &str)> {
        self.nodes.iter().filter_map(|nd| if let Kind::Text(t) = &nd.kind { Some((nd.key.as_str(), t.text.as_str())) } else { None }).collect()
    }

    /// The fonts hash line of a dump text (`system:...`), for comparing two dumps' environments.
    pub fn fonts_of(text: &str) -> Option<&str> {
        text.lines().find(|l| l.starts_with("env ")).and_then(|l| l.split_whitespace().find_map(|w| w.strip_prefix("fonts=")))
    }
}

/// Small dumps for the tests of this module's neighbours.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub fn plain(key: &str, depth: u32) -> Node {
        Node {
            key: key.into(),
            name: key.rsplit('/').next().unwrap().into(),
            depth,
            kind: Kind::Box,
            rect: [0.0, 0.0, 10.0, 10.0],
            layer: 0,
            op: None,
            fill: None,
            grad: None,
            border: None,
            radius: None,
            shadow: None,
            hover: None,
            enter: None,
            clip: None,
            scroll_y: None,
            scroll_x: None,
            hit: false,
            act: None,
            drop: None,
            slide: None,
            flags: vec![],
            custom_key: false,
        }
    }

    pub fn text(key: &str, depth: u32, s: &str) -> Node {
        Node { kind: Kind::Text(TextFacts { text: s.into(), size: 12.0, weight: 400, want: String::new(), got: vec![], color: "#ffffff".into(), align: "left".into(), wrap: false, lines: Some(1), nat: None, caret: None, line_height: None }), ..plain(key, depth) }
    }

    pub fn dump(nodes: Vec<Node>) -> SceneDump {
        let header = Header { id: "s".into(), target: "widget t".into(), card: [10.0, 10.0], window: [20.0, 20.0], scale: 1.0, at_ms: 2000, hermetic: true, not_hermetic_because: vec![], pins: "abcdef".into(), now: "2026-01-15T10:10:30".into(), zone: "UTC".into(), locale: "en-US".into(), anim: "theme".into(), palette: "default".into(), fonts: "system:0".into() };
        let hits = vec![HitInfo { key: "w/c/b".into(), rect: [1.0, 2.0, 3.0, 4.0], clip: None, act: Some("x".into()), drop: None, slide: None }];
        SceneDump { format: 1, header, nodes, content: ContentInfo { size: [20.0, 20.0], scrolls: vec![] }, hits, draw: vec![LayerCount { layer: 0, shapes: 1, images: 0, texts: 1 }], flags: vec![] }
    }

    /// A dump whose only nodes are a window box and one text run per string.
    pub fn tiny_dump(texts: &[&str]) -> SceneDump {
        let mut nodes = vec![plain("w", 0)];
        nodes.extend(texts.iter().enumerate().map(|(i, t)| text(&format!("w/{i}"), 1, t)));
        dump(nodes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_print_to_a_hundredth_shortest_and_never_negative_zero() {
        let got: Vec<String> = [12.0, 12.5, 12.254, 12.256, -0.001, 0.0].map(|v| n(r2(v))).to_vec();
        assert_eq!(got, ["12", "12.5", "12.25", "12.26", "0", "0"]);
        assert_eq!(r2(63.4000001), 63.4);
    }

    #[test]
    fn text_is_quoted_with_escapes_and_cut_in_the_text_form() {
        assert_eq!(quote("a \"b\" \\ c\nd"), "\"a \\\"b\\\" \\\\ c\\nd\"");
        let long = "x".repeat(130);
        assert_eq!(quote_cut(&long), format!("\"{}\"...(10 more)", "x".repeat(120)));
        assert_eq!(quote_cut("short"), "\"short\"");
        assert_eq!((bare("a/b.png"), bare("a b"), bare("")), ("a/b.png".to_string(), "\"a b\"".to_string(), "\"\"".to_string()));
    }

    fn text_node(text: &str) -> Node {
        Node {
            key: "w/c/date".into(),
            name: "date".into(),
            depth: 2,
            kind: Kind::Text(TextFacts { text: text.into(), size: 12.0, weight: 400, want: String::new(), got: vec!["Open Sans".into()], color: "#ffffffb0".into(), align: "center".into(), wrap: false, lines: Some(1), nat: Some([71.0, 16.0]), caret: None, line_height: None }),
            rect: [60.0, 150.0, 90.0, 18.0],
            layer: 0,
            op: None,
            fill: None,
            grad: None,
            border: None,
            radius: None,
            shadow: None,
            hover: None,
            enter: None,
            clip: None,
            scroll_y: None,
            scroll_x: None,
            hit: false,
            act: None,
            drop: None,
            slide: None,
            flags: vec![],
            custom_key: false,
        }
    }

    #[test]
    fn a_text_line_has_the_grammar_of_the_spec() {
        assert_eq!(text_node("Thu 15 Jan").line(false), "date text 60,150 90x18 \"Thu 15 Jan\" 12px w400 want=theme got=\"Open Sans\" #ffffffb0 center nowrap lines=1 nat=71x16");
        let mut t = text_node("x");
        t.custom_key = true;
        t.key = "weird".into();
        t.op = Some(0.35);
        t.layer = 1;
        t.flags = vec!["TRUNCATED".into()];
        assert_eq!(t.line(false), "date text 60,150 90x18 key=weird layer=1 op=0.35 \"x\" 12px w400 want=theme got=\"Open Sans\" #ffffffb0 center nowrap lines=1 nat=71x16 TRUNCATED");
    }

    #[test]
    fn a_box_line_prints_only_what_is_not_default_in_the_fixed_order() {
        let mut b = text_node("");
        b.kind = Kind::Box;
        b.name = "btn".into();
        b.rect = [82.0, 164.0, 46.0, 22.5];
        b.fill = Some("#1c2333e6".into());
        b.border = Some((1.0, "#ffffff30".into()));
        b.radius = Some(28.0);
        b.shadow = Some((18.0, 6.0, "#00000080".into()));
        b.hover = Some("fill:#ffffff22".into());
        b.enter = Some("300ms/dy8".into());
        b.clip = Some([15.0, 15.0, 180.0, 180.0]);
        b.scroll_y = Some([0.0, 340.0]);
        b.hit = true;
        b.act = Some("tog:clock-1|hour24".into());
        assert_eq!(b.line(false), "btn box 82,164 46x22.5 fill=#1c2333e6 border=1:#ffffff30 radius=28 shadow=18/6:#00000080 hover=fill:#ffffff22 enter=300ms/dy8 clip=[15,15,180,180] scroll=y:0/340 hit act=\"tog:clock-1|hour24\"");
    }

    #[test]
    fn the_json_twin_names_the_kind_and_skips_defaults() {
        let j = serde_json::to_value(text_node("hi")).unwrap();
        assert_eq!((j["kind"].as_str(), j["text"].as_str(), j["rect"][2].as_f64(), j["nat"][0].as_f64()), (Some("text"), Some("hi"), Some(90.0), Some(71.0)));
        assert!(j.get("fill").is_none() && j.get("hit").is_none() && j.get("layer").is_none());
        let shape = Node { kind: Kind::Shape { shape: "ticks".into(), attrs: vec![("count".into(), "60".into()), ("len".into(), "5".into())] }, ..text_node("") };
        let j = serde_json::to_value(shape).unwrap();
        assert_eq!((j["kind"].as_str(), j["shape"].as_str(), j["attrs"]["count"].as_str()), (Some("shape"), Some("ticks"), Some("60")));
    }
}
