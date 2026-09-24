//! The runtime state of each Instance, index-aligned with `Workspace::instances`.

use super::*;

pub(super) struct Drag {
    pub(super) handle: Handle,
    pub(super) cursor0: (i32, i32),
    pub(super) rect0: Rect,
    pub(super) before: Rect,
    /// `snap_offset` at the last update: a change means a snap caught, let go or stepped.
    pub(super) snapped: [i32; 4],
}

/// A window moving or resizing from one rect to another (expand, snap, undo).
pub(super) struct SizeTween {
    pub(super) from: Rect,
    pub(super) to: Rect,
    pub(super) start: Instant,
    pub(super) secs: f32,
}

/// Seconds, before `anim-speed`.
pub(super) const EXPAND_SECS: f32 = 0.20;
pub(super) const GLIDE_SECS: f32 = 0.12;

/// How far snapping moved each edge (left, top, right, bottom) from where the cursor alone put it.
pub(super) fn snap_offset(raw: Rect, snapped: Rect) -> [i32; 4] {
    [snapped.x - raw.x, snapped.y - raw.y, snapped.right() - raw.right(), snapped.bottom() - raw.bottom()]
}

impl SizeTween {
    fn progress(&self, now: Instant) -> f32 {
        (now.saturating_duration_since(self.start).as_secs_f32() / self.secs.max(1e-3)).clamp(0.0, 1.0)
    }

    pub(super) fn rect_at(&self, now: Instant) -> Rect {
        let k = Ease::Out.apply(self.progress(now));
        let l = |a: i32, b: i32| a + ((b - a) as f32 * k).round() as i32;
        let (f, to) = (self.from, self.to);
        Rect::new(l(f.x, to.x), l(f.y, to.y), l(f.w, to.w).max(1), l(f.h, to.h).max(1))
    }

    pub(super) fn finished_at(&self, now: Instant) -> bool {
        self.progress(now) >= 1.0
    }
}

pub(super) const ANIMATION_FRAME: Duration = Duration::from_micros(16_667);

pub(super) struct Instance {
    pub(super) window: Option<Arc<Window>>,
    /// The DWM corner preference while blurred, `None` while not.
    pub(super) blur_applied: Option<i32>,
    pub(super) target: Option<Target>,
    pub(super) state: BTreeMap<String, Value>,
    pub(super) anim: Anim,
    pub(super) ov_anim: Anim,
    pub(super) hover: Option<String>,
    pub(super) frame: Option<Frame>,
    pub(super) deps: BTreeSet<String>,
    pub(super) next_tick: Option<Instant>,
    pub(super) redraw: bool,
    pub(super) requested: bool,
    pub(super) animating: bool,
    pub(super) last_render: Instant,
    pub(super) drag: Option<Drag>,
    pub(super) grab: Option<Handle>,
    pub(super) mouse: (f32, f32),
    pub(super) tween: Option<SizeTween>,
    pub(super) want: Option<Rect>,
    /// The Widget's last `[expand]`, window units: which axes the window's size belongs to.
    pub(super) expand: Option<ExpandInfo>,
    pub(super) raised: bool,
    pub(super) error: Option<String>,
    pub(super) widget_error: Option<String>,
}

impl Instance {
    pub(super) fn new() -> Self {
        Self {
            window: None,
            target: None,
            state: BTreeMap::new(),
            anim: Anim::default(),
            ov_anim: Anim::default(),
            hover: None,
            frame: None,
            deps: BTreeSet::new(),
            next_tick: None,
            redraw: true,
            requested: false,
            animating: false,
            last_render: Instant::now(),
            drag: None,
            grab: None,
            mouse: (-1.0, -1.0),
            tween: None,
            blur_applied: None,
            want: None,
            expand: None,
            raised: false,
            error: None,
            widget_error: None,
        }
    }

    pub(super) fn needs_frame(&self, now: Instant) -> bool {
        self.redraw || self.next_tick.is_some_and(|t| t <= now) || (self.animating && now.duration_since(self.last_render) >= ANIMATION_FRAME)
    }

    pub(super) fn next_wake(&self) -> Option<Instant> {
        if self.animating { Some(self.last_render + ANIMATION_FRAME) } else { self.next_tick }
    }
}

/// The size to store after a move or resize. While an expand is active the window
/// shows the expand's size on the axes it sets, so those keep the stored size.
pub(super) fn base_size_after_edit(window: (f32, f32), stored: (f32, f32), expand: Option<ExpandInfo>) -> (f32, f32) {
    match expand.filter(|e| e.active) {
        Some(e) => (if e.width.is_some() { stored.0 } else { window.0 }, if e.height.is_some() { stored.1 } else { window.1 }),
        None => window,
    }
}

/// Grows away from the nearest work-area edge and stays inside it.
pub(super) fn expand_target(expand: Option<ExpandInfo>, stored: (f32, f32), collapsed: Rect, work: (i32, i32, u32, u32), scale: f64) -> Rect {
    let Some(e) = expand.filter(|e| e.active) else { return collapsed };
    let (tw, th) = (e.width.unwrap_or(stored.0), e.height.unwrap_or(stored.1));
    let (w, h) = (((tw as f64 * scale).round() as i32).min(work.2 as i32), ((th as f64 * scale).round() as i32).min(work.3 as i32));
    let (wl, wt, wr, wb) = (work.0, work.1, work.0 + work.2 as i32, work.1 + work.3 as i32);
    let x = if collapsed.x + w > wr { (collapsed.right() - w).max(wl) } else { collapsed.x };
    let y = if collapsed.y + h > wb { (collapsed.bottom() - h).max(wt) } else { collapsed.y };
    Rect::new(x, y, w, h)
}

/// The scroll region a wheel turn moves, and by how much. A plain wheel over a strip that
/// only scrolls sideways moves it sideways.
pub(super) fn wheel_target<'a>(frame: &'a crate::ui::Frame, (mx, my): (f32, f32), dx: f32, dy: f32) -> Option<(&'a crate::ui::ScrollInfo, f32)> {
    let under = |horizontal: bool| frame.scrolls.iter().find(|s| s.horizontal == horizontal && frame.rect_of(&s.key).is_some_and(|[x, y, w, h]| mx >= x && mx < x + w && my >= y && my < y + h));
    match (under(false), under(true)) {
        (Some(v), _) if dy != 0.0 => Some((v, dy)),
        (_, Some(h)) if dx != 0.0 => Some((h, dx)),
        (None, Some(h)) => Some((h, dy)),
        _ => None,
    }
}

pub(super) fn scrolled_offset(cur: f32, dy: f32, view_h: f32, content_h: f32) -> Option<f32> {
    let next = (cur - dy).clamp(0.0, (content_h - view_h).max(0.0));
    ((next - cur).abs() > 0.01).then_some(next)
}

#[derive(Debug, PartialEq)]
pub(super) enum VerbOutcome {
    Redraw,
    Launch(String),
    OpenSettings,
    Nothing,
    Unknown,
}

/// `toggle` also scrolls back to the top.
/// A `set` value: numbers and `true`/`false` keep their type, so `set index 2` compares
/// equal to `2`; quotes keep text as written (`set code '007'`).
pub(super) fn typed(v: &str) -> Value {
    let t = v.trim();
    let quoted = |q: char| t.strip_prefix(q).and_then(|s| s.strip_suffix(q)).filter(|_| t.len() >= 2);
    if let Some(s) = quoted('\'').or_else(|| quoted('"')) {
        return Value::Str(s.to_string());
    }
    match t {
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        _ => {}
    }
    let numeric = t.chars().any(|c| c.is_ascii_digit()) && t.chars().all(|c| c.is_ascii_digit() || "+-.eE".contains(c));
    match t.parse::<f64>() {
        Ok(n) if numeric && n.is_finite() => Value::Num(n),
        _ => Value::Str(v.to_string()),
    }
}

pub(super) fn engine_action(state: &mut BTreeMap<String, Value>, verb: &str, rest: &str) -> VerbOutcome {
    match verb {
        "launch" => VerbOutcome::Launch(rest.trim().to_string()),
        "toggle" => {
            let cur = state.get(rest).is_some_and(|v| v.truthy());
            state.insert(rest.to_string(), Value::Bool(!cur));
            state.insert("scroll".into(), Value::Num(0.0));
            VerbOutcome::Redraw
        }
        "set" => match rest.split_once(' ') {
            Some((name, val)) => {
                state.insert(name.to_string(), typed(val));
                VerbOutcome::Redraw
            }
            None => VerbOutcome::Nothing,
        },
        "settings" => VerbOutcome::OpenSettings,
        _ => VerbOutcome::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_glide_takes_its_own_duration_and_lands_on_the_target() {
        let t0 = Instant::now();
        let tw = SizeTween { from: Rect::new(0, 0, 100, 100), to: Rect::new(100, 0, 100, 100), start: t0, secs: 0.1 };
        assert!(tw.rect_at(t0 + Duration::from_millis(50)).x > 50, "ease-out is past halfway at half time");
        assert!(!tw.finished_at(t0 + Duration::from_millis(90)));
        assert_eq!(tw.rect_at(t0 + Duration::from_millis(100)), Rect::new(100, 0, 100, 100));
        assert!(tw.finished_at(t0 + Duration::from_millis(100)));
    }

    #[test]
    fn snap_offset_is_zero_until_a_guide_or_the_grid_moves_an_edge() {
        let raw = Rect::new(96, 40, 200, 100);
        assert_eq!(snap_offset(raw, raw), [0; 4]);
        assert_eq!(snap_offset(raw, Rect::new(100, 40, 200, 100)), [4, 0, 4, 0], "a move snaps both x edges");
        assert_eq!(snap_offset(raw, Rect::new(96, 40, 204, 100)), [0, 0, 4, 0], "a resize snaps one edge");
    }

    #[test]
    fn editing_an_expanded_widget_keeps_its_base_size_on_the_expanded_axes() {
        // a collapsed drawer: `[expand]` sets only the height (44), the open size is 300x220
        let collapsed = Some(ExpandInfo { active: true, width: None, height: Some(44.0) });
        assert_eq!(base_size_after_edit((300.0, 44.0), (300.0, 220.0), collapsed), (300.0, 220.0), "moving it keeps the open height");
        assert_eq!(base_size_after_edit((360.0, 44.0), (300.0, 220.0), collapsed), (360.0, 220.0), "a width change still counts");
        let open = Some(ExpandInfo { active: false, width: None, height: Some(44.0) });
        assert_eq!(base_size_after_edit((360.0, 250.0), (300.0, 220.0), open), (360.0, 250.0));
        assert_eq!(base_size_after_edit((360.0, 250.0), (300.0, 220.0), None), (360.0, 250.0));
    }

    #[test]
    fn an_idle_instance_is_never_due_and_wakes_only_for_its_next_tick() {
        let now = Instant::now();
        let mut i = Instance::new();
        i.redraw = false;
        assert!(!i.needs_frame(now + Duration::from_secs(3600)));
        assert_eq!(i.next_wake(), None, "nothing bound changes with time: sleep until an event");
        i.next_tick = Some(now + Duration::from_millis(500));
        assert!(!i.needs_frame(now) && i.needs_frame(now + Duration::from_millis(500)));
        assert_eq!(i.next_wake(), i.next_tick);
    }

    #[test]
    fn an_animating_instance_is_due_once_per_frame() {
        let mut i = Instance::new();
        i.redraw = false;
        i.animating = true;
        let t0 = i.last_render;
        assert!(!i.needs_frame(t0 + Duration::from_millis(10)));
        assert!(i.needs_frame(t0 + ANIMATION_FRAME));
        assert_eq!(i.next_wake(), Some(t0 + ANIMATION_FRAME));
    }

    #[test]
    fn a_size_tween_eases_to_its_target_and_ends() {
        let start = Instant::now();
        let tw = SizeTween { from: Rect::new(0, 0, 100, 100), to: Rect::new(0, 0, 300, 200), start, secs: EXPAND_SECS };
        let half = start + Duration::from_millis(100);
        let mid = tw.rect_at(half);
        assert!(!tw.finished_at(half) && mid.w > 200 && mid.w < 300, "ease-out is past halfway at half time: {mid:?}");
        let end = start + Duration::from_millis(250);
        assert_eq!((tw.rect_at(end), tw.finished_at(end)), (tw.to, true));
    }

    #[test]
    fn expanding_grows_away_from_the_near_edge_and_stays_in_the_work_area() {
        let work = (0, 0, 1000, 800);
        let open = |w, h| Some(ExpandInfo { active: true, width: Some(w), height: Some(h) });
        let at = |x, y| Rect::new(x, y, 100, 100);
        assert_eq!(expand_target(open(300.0, 200.0), (100.0, 100.0), at(100, 100), work, 1.0), Rect::new(100, 100, 300, 200), "room to grow right and down");
        assert_eq!(expand_target(open(300.0, 200.0), (100.0, 100.0), at(850, 650), work, 1.0), Rect::new(650, 550, 300, 200), "near the corner: grows left and up");
        assert_eq!(expand_target(open(3000.0, 200.0), (100.0, 100.0), at(100, 100), work, 1.0).w, 1000, "never wider than the work area");
        assert_eq!(expand_target(open(300.0, 200.0), (100.0, 100.0), at(100, 100), work, 1.5), Rect::new(100, 100, 450, 300), "logical sizes scale");
        let closed = Some(ExpandInfo { active: false, width: Some(300.0), height: None });
        assert_eq!(expand_target(closed, (100.0, 100.0), at(100, 100), work, 1.0), at(100, 100));
    }

    #[test]
    fn scrolling_clamps_to_the_content_and_ignores_no_ops() {
        assert_eq!(scrolled_offset(0.0, -48.0, 100.0, 300.0), Some(48.0));
        assert_eq!(scrolled_offset(180.0, -48.0, 100.0, 300.0), Some(200.0), "stops at the end");
        assert_eq!(scrolled_offset(0.0, 48.0, 100.0, 300.0), None, "already at the top");
        assert_eq!(scrolled_offset(0.0, -48.0, 300.0, 100.0), None, "content fits: nothing to scroll");
    }

    #[test]
    fn the_wheel_moves_the_region_under_the_cursor_along_its_axis() {
        use crate::ui::{Frame, ScrollInfo};
        let mut f = Frame::default();
        f.rects.push(("list".into(), [0.0, 0.0, 100.0, 100.0]));
        f.rects.push(("strip".into(), [0.0, 100.0, 100.0, 40.0]));
        f.scrolls.push(ScrollInfo { key: "list".into(), view: 100.0, content: 300.0, horizontal: false });
        f.scrolls.push(ScrollInfo { key: "strip".into(), view: 100.0, content: 500.0, horizontal: true });
        let hit = |at: (f32, f32), dx: f32, dy: f32| wheel_target(&f, at, dx, dy).map(|(s, d)| (s.key.clone(), d));
        assert_eq!(hit((10.0, 10.0), 0.0, -48.0), Some(("list".into(), -48.0)));
        assert_eq!(hit((10.0, 110.0), 0.0, -48.0), Some(("strip".into(), -48.0)), "a plain wheel over a sideways strip moves it sideways");
        assert_eq!(hit((10.0, 110.0), -30.0, 0.0), Some(("strip".into(), -30.0)));
        assert_eq!(hit((10.0, 10.0), -30.0, 0.0), None, "a sideways wheel over a list that only scrolls down");
        assert_eq!(hit((10.0, 200.0), 0.0, -48.0), None);
    }

    #[test]
    fn engine_verbs_change_state_and_say_what_else_to_do() {
        let mut st = BTreeMap::from([("scroll".to_string(), Value::Num(40.0))]);
        assert_eq!(engine_action(&mut st, "toggle", "expanded"), VerbOutcome::Redraw);
        assert_eq!((st.get("expanded"), st.get("scroll")), (Some(&Value::Bool(true)), Some(&Value::Num(0.0))), "toggling scrolls back to the top");
        engine_action(&mut st, "toggle", "expanded");
        assert_eq!(st.get("expanded"), Some(&Value::Bool(false)));
        assert_eq!(engine_action(&mut st, "set", "tab news"), VerbOutcome::Redraw);
        assert_eq!(st.get("tab"), Some(&Value::Str("news".into())));
        engine_action(&mut st, "set", "index 2");
        assert_eq!(st.get("index"), Some(&Value::Num(2.0)), "a number, so `state.index == 2` holds");
        engine_action(&mut st, "set", "on true");
        assert_eq!(st.get("on"), Some(&Value::Bool(true)));
        engine_action(&mut st, "set", "code '007'");
        assert_eq!(st.get("code"), Some(&Value::Str("007".into())), "quotes keep text");
        engine_action(&mut st, "set", "title 2 cats");
        assert_eq!(st.get("title"), Some(&Value::Str("2 cats".into())));
        assert_eq!((typed("-1.5"), typed("1e3"), typed("inf"), typed("1.2.3"), typed("'")), (Value::Num(-1.5), Value::Num(1000.0), Value::Str("inf".into()), Value::Str("1.2.3".into()), Value::Str("'".into())));
        assert_eq!(engine_action(&mut st, "launch", " C:\\app.exe "), VerbOutcome::Launch("C:\\app.exe".into()));
        assert_eq!(engine_action(&mut st, "settings", ""), VerbOutcome::OpenSettings);
        assert_eq!(engine_action(&mut st, "add_app", ""), VerbOutcome::Unknown, "a Widget's own verb is not the engine's");
    }
}
