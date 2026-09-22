//! The runtime state of one Instance: its window and render target, its
//! `state.*`, hover, frame, schedule and in-flight drag or size tween.
//! Index-aligned with `Workspace::instances`.

use super::*;

pub(super) struct Drag {
    pub(super) handle: Handle,
    pub(super) cursor0: (i32, i32),
    pub(super) rect0: Rect,
    pub(super) before: Rect,
}

pub(super) struct SizeTween {
    pub(super) from: Rect,
    pub(super) to: Rect,
    pub(super) start: Instant,
}

/// Runtime state of one Instance, index-aligned with `Workspace::instances`.
pub(super) struct Instance {
    pub(super) window: Option<Arc<Window>>,
    /// Whether DWM blur is currently switched on for this window.
    pub(super) blur: bool,
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
            blur: false,
            want: None,
            raised: false,
            error: None,
            widget_error: None,
        }
    }
}
