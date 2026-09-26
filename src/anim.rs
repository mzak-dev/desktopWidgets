//! Declarative transitions (ADR-004). The engine owns the clock, so "is
//! anything animating" is a question the redraw scheduler can ask.

use std::collections::HashMap;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Ease {
    Linear,
    In,
    #[default]
    Out,
    InOut,
    /// Overshoots, then settles.
    Back,
}

impl Ease {
    pub fn parse(s: &str) -> Ease {
        match s {
            "linear" => Ease::Linear,
            "in" => Ease::In,
            "in-out" | "inout" => Ease::InOut,
            "back" | "bounce" => Ease::Back,
            _ => Ease::Out,
        }
    }

    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Ease::Linear => t,
            Ease::In => t * t * t,
            Ease::Out => 1.0 - (1.0 - t).powi(3),
            Ease::InOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
            Ease::Back => {
                let (c1, c3) = (1.70158, 2.70158);
                1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Tween {
    from: [f32; 4],
    to: [f32; 4],
    start: Instant,
    delay: f32,
    dur: f32,
    ease: Ease,
}

impl Tween {
    fn at(&self, now: Instant) -> [f32; 4] {
        let ms = now.saturating_duration_since(self.start).as_secs_f32() * 1000.0 - self.delay;
        if ms <= 0.0 {
            return self.from;
        }
        let k = self.ease.apply((ms / self.dur.max(1.0)).min(1.0));
        std::array::from_fn(|i| self.from[i] + (self.to[i] - self.from[i]) * k)
    }

    fn done(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.start).as_secs_f32() * 1000.0 >= self.delay + self.dur
    }
}

/// The `anim-speed` style token as a multiplier on every duration; 0 means no animation.
pub fn duration_factor(speed: &str) -> f32 {
    match speed {
        "off" => 0.0,
        "fast" => 0.6,
        "relaxed" => 1.6,
        _ => 1.0,
    }
}

/// A picture an `image` draws: its id, and its own size for its own fit.
#[derive(Clone, Debug, PartialEq)]
pub struct Pic {
    pub id: String,
    pub size: (f32, f32),
}

/// What a crossfading `image` draws: the picture underneath at full opacity, and while a new
/// one comes in, that one on top at this much of it. An opaque cover never dims halfway.
pub type FadeDraw = (Pic, Option<(Pic, f32)>);

/// A crossfading `image`: the picture it settled on, and the one coming in with when its fade
/// began (`None` while it is still loading on another thread).
#[derive(Clone, Debug, PartialEq)]
pub struct Crossfade {
    under: Pic,
    over: Option<(Pic, Option<Instant>)>,
    ms: f32,
}

impl Crossfade {
    /// Steps `state` toward `target`, which has loaded when `ready`, at `now`: the old picture
    /// stays until the new one is ready, then the new one fades in over `ms`; 0 swaps at once.
    pub fn step(state: &mut Option<Crossfade>, target: Pic, ready: bool, ms: f32, now: Instant) -> FadeDraw {
        let c = match state {
            Some(c) if ms > 0.0 => c,
            _ => {
                *state = Some(Crossfade { under: target.clone(), over: None, ms });
                return (target, None);
            }
        };
        c.ms = ms;
        if c.under.id == target.id {
            // back to the settled picture (or its size arrived): nothing fades
            c.under = target;
            c.over = None;
            return (c.under.clone(), None);
        }
        let began = c.over.as_ref().filter(|(p, _)| p.id == target.id).and_then(|(_, b)| *b);
        let start = began.or(ready.then_some(now));
        c.over = Some((target.clone(), start));
        let Some(t0) = start else { return (c.under.clone(), None) };
        let t = now.saturating_duration_since(t0).as_secs_f32() * 1000.0 / ms;
        if t >= 1.0 {
            c.under = target;
            c.over = None;
            return (c.under.clone(), None);
        }
        (c.under.clone(), Some((target, t.max(0.0))))
    }

    fn fading(&self, now: Instant) -> bool {
        self.over.as_ref().and_then(|(_, b)| *b).is_some_and(|t0| now.saturating_duration_since(t0).as_secs_f32() * 1000.0 < self.ms)
    }
}

pub struct Anim {
    map: HashMap<(String, &'static str), (Tween, u64)>,
    /// Crossfading images by node key, and the frame each was last drawn.
    fades: HashMap<String, (Option<Crossfade>, u64)>,
    frame: u64,
    /// Scales every duration and delay (`duration_factor` of `anim-speed`); 0 jumps to the target.
    pub duration_factor: f32,
}

impl Default for Anim {
    fn default() -> Self {
        Self { map: HashMap::new(), fades: HashMap::new(), frame: 0, duration_factor: 1.0 }
    }
}

impl Anim {
    pub fn begin_frame(&mut self) {
        self.frame += 1;
    }

    /// Drop tweens whose node vanished, so a re-appearing node replays its
    /// enter animation.
    pub fn end_frame(&mut self) {
        let f = self.frame;
        self.map.retain(|_, (_, seen)| *seen == f);
        self.fades.retain(|_, (_, seen)| *seen == f);
    }

    /// What the `image` at `key` draws this frame: `target`, faded in over `ms` (times the
    /// duration factor) from the picture it showed before.
    pub fn crossfade(&mut self, key: &str, target: Pic, ready: bool, ms: u32, now: Instant) -> FadeDraw {
        let ms = ms as f32 * self.duration_factor;
        let frame = self.frame;
        let e = self.fades.entry(key.to_string()).or_insert((None, frame));
        e.1 = frame;
        Crossfade::step(&mut e.0, target, ready, ms, now)
    }

    /// Current animated value of `(key, prop)` heading to `target`. With
    /// `ms == 0` and no `from`, the value is simply the target. `from` seeds an
    /// enter animation the first time the key is seen.
    #[allow(clippy::too_many_arguments)]
    pub fn value(
        &mut self,
        key: &str,
        prop: &'static str,
        target: [f32; 4],
        ms: u32,
        ease: Ease,
        delay_ms: u32,
        from: Option<[f32; 4]>,
        now: Instant,
    ) -> [f32; 4] {
        let id = (key.to_string(), prop);
        let ms = (ms as f32 * self.duration_factor).round() as u32;
        let delay_ms = (delay_ms as f32 * self.duration_factor).round() as u32;
        if ms == 0 {
            self.map.remove(&id);
            return target;
        }
        let frame = self.frame;
        let e = self.map.entry(id).or_insert_with(|| {
            let tw = Tween {
                from: from.unwrap_or(target),
                to: target,
                start: now,
                delay: delay_ms as f32,
                dur: ms as f32,
                ease,
            };
            (tw, frame)
        });
        e.1 = frame;
        if e.0.to != target {
            // Retarget from wherever we are right now, so hover in/out never jumps.
            let cur = e.0.at(now);
            e.0 = Tween { from: cur, to: target, start: now, delay: 0.0, dur: ms as f32, ease };
        }
        e.0.at(now)
    }

    /// A laid-out value (a node's rect) that glides when it jumps by more than `jump`
    /// in one frame, as on a reflow, and tracks smaller steps directly, as during a
    /// live resize. The first sight of a key is never animated.
    pub fn follow(&mut self, key: &str, prop: &'static str, target: [f32; 4], ms: u32, jump: f32, now: Instant) -> [f32; 4] {
        let ms = ms as f32 * self.duration_factor;
        let frame = self.frame;
        let settled = |v: [f32; 4]| Tween { from: v, to: v, start: now, delay: 0.0, dur: 0.0, ease: Ease::Out };
        let e = self.map.entry((key.to_string(), prop)).or_insert_with(|| (settled(target), frame));
        e.1 = frame;
        if e.0.to != target {
            let cur = e.0.at(now);
            let jumped = (0..4).any(|k| (target[k] - cur[k]).abs() > jump);
            e.0 = if ms >= 1.0 && (jumped || !e.0.done(now)) { Tween { from: cur, to: target, start: now, delay: 0.0, dur: ms, ease: Ease::Out } } else { settled(target) };
        }
        e.0.at(now)
    }

    pub fn animating(&self, now: Instant) -> bool {
        self.map.values().any(|(t, _)| !t.done(now)) || self.fades.values().any(|(c, _)| c.as_ref().is_some_and(|c| c.fading(now)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn follow_glides_on_a_jump_and_tracks_a_creep() {
        let t0 = Instant::now();
        let mut a = Anim::default();
        a.begin_frame();
        assert_eq!(a.follow("k", "rect", [0.0, 0.0, 50.0, 50.0], 200, 6.0, t0), [0.0, 0.0, 50.0, 50.0], "first sight: no animation");
        assert!(!a.animating(t0));
        a.begin_frame();
        assert_eq!(a.follow("k", "rect", [3.0, 0.0, 50.0, 50.0], 200, 6.0, t0), [3.0, 0.0, 50.0, 50.0], "a small step (a live resize) is followed directly");
        assert!(!a.animating(t0));
        a.begin_frame();
        let mid = a.follow("k", "rect", [103.0, 0.0, 50.0, 50.0], 200, 6.0, t0);
        assert_eq!(mid[0], 3.0, "a reflow jump starts where it was");
        assert!(a.animating(t0));
        let t1 = t0 + Duration::from_millis(100);
        let mid = a.follow("k", "rect", [103.0, 0.0, 50.0, 50.0], 200, 6.0, t1);
        assert!(mid[0] > 53.0 && mid[0] < 103.0, "{mid:?}");
        let end = t0 + Duration::from_millis(250);
        assert_eq!(a.follow("k", "rect", [103.0, 0.0, 50.0, 50.0], 200, 6.0, end)[0], 103.0);
        assert!(!a.animating(end));

        let mut off = Anim { duration_factor: 0.0, ..Default::default() };
        off.begin_frame();
        off.follow("k", "rect", [0.0; 4], 200, 6.0, t0);
        assert_eq!(off.follow("k", "rect", [100.0, 0.0, 0.0, 0.0], 200, 6.0, t0)[0], 100.0, "animations off: it jumps");
    }

    #[test]
    fn duration_factor_stretches_or_skips_animation() {
        let t0 = Instant::now();
        let mut a = Anim { duration_factor: 1.6, ..Default::default() };
        a.begin_frame();
        a.value("k", "x", [1.0; 4], 100, Ease::Linear, 0, Some([0.0; 4]), t0);
        let v = a.value("k", "x", [1.0; 4], 100, Ease::Linear, 0, None, t0 + Duration::from_millis(80));
        assert!((v[0] - 0.5).abs() < 1e-3, "80 of 160 ms: {v:?}");
        let mut off = Anim { duration_factor: 0.0, ..Default::default() };
        off.begin_frame();
        assert_eq!(off.value("k", "x", [1.0; 4], 100, Ease::Linear, 0, Some([0.0; 4]), t0), [1.0; 4]);
        assert!(!off.animating(t0));
    }

    #[test]
    fn a_crossfade_waits_for_the_new_picture_then_fades_it_in_over_the_old() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let pic = |id: &str| Pic { id: id.into(), size: (64.0, 64.0) };
        let mut c = None;
        assert_eq!(Crossfade::step(&mut c, pic("a"), true, 300.0, t0), (pic("a"), None), "the first picture just shows");
        assert_eq!(Crossfade::step(&mut c, pic("b"), false, 300.0, t0), (pic("a"), None), "switch: the old one stays while the new one loads");
        assert_eq!(Crossfade::step(&mut c, pic("b"), true, 300.0, t0 + ms(50)), (pic("a"), Some((pic("b"), 0.0))), "loaded: the fade starts");
        let (under, over) = Crossfade::step(&mut c, pic("b"), true, 300.0, t0 + ms(200));
        assert_eq!((under, over.as_ref().map(|o| &o.0)), (pic("a"), Some(&pic("b"))), "fading: the new one over the old");
        assert!((over.unwrap().1 - 0.5).abs() < 1e-3);
        assert_eq!(Crossfade::step(&mut c, pic("b"), true, 300.0, t0 + ms(400)), (pic("b"), None), "done: the old one goes");
        assert_eq!(Crossfade::step(&mut c, pic("c"), false, 0.0, t0 + ms(400)), (pic("c"), None), "no fade: it swaps at once");
    }

    #[test]
    fn a_crossfade_keeps_frames_coming_and_respects_the_animation_speed() {
        let t0 = Instant::now();
        let pic = |id: &str| Pic { id: id.into(), size: (64.0, 64.0) };
        let mut a = Anim::default();
        a.begin_frame();
        a.crossfade("cover", pic("a"), true, 300, t0);
        a.crossfade("cover", pic("b"), true, 300, t0);
        assert!(a.animating(t0), "a fade needs frames");
        a.begin_frame();
        a.end_frame();
        assert!(!a.animating(t0), "a cover no longer drawn is forgotten");
        let mut off = Anim { duration_factor: 0.0, ..Default::default() };
        off.begin_frame();
        off.crossfade("cover", pic("a"), true, 300, t0);
        assert_eq!(off.crossfade("cover", pic("b"), true, 300, t0), (pic("b"), None), "animations off: instant");
    }

    #[test]
    fn anim_speed_names_map_to_duration_factors() {
        assert_eq!((duration_factor("off"), duration_factor("fast"), duration_factor("normal"), duration_factor("relaxed"), duration_factor("?")), (0.0, 0.6, 1.0, 1.6, 1.0));
    }

    #[test]
    fn idle_when_nothing_moves_and_animating_while_a_tween_runs() {
        let mut a = Anim::default();
        let t0 = Instant::now();
        a.begin_frame();
        let v = a.value("k", "op", [1.0; 4], 0, Ease::Out, 0, None, t0);
        assert_eq!(v, [1.0; 4]);
        assert!(!a.animating(t0), "a zero-duration value must not keep the scheduler awake");

        a.begin_frame();
        a.value("k", "op", [1.0, 0.0, 0.0, 0.0], 100, Ease::Linear, 0, Some([0.0; 4]), t0);
        assert!(a.animating(t0));
        let mid = a.value("k", "op", [1.0, 0.0, 0.0, 0.0], 100, Ease::Linear, 0, None, t0 + Duration::from_millis(50));
        assert!((mid[0] - 0.5).abs() < 1e-3, "{mid:?}");
        let end = t0 + Duration::from_millis(150);
        assert_eq!(a.value("k", "op", [1.0, 0.0, 0.0, 0.0], 100, Ease::Linear, 0, None, end)[0], 1.0);
        assert!(!a.animating(end));
    }

    #[test]
    fn retarget_continues_from_the_current_value() {
        let mut a = Anim::default();
        let t0 = Instant::now();
        a.begin_frame();
        a.value("k", "x", [10.0; 4], 100, Ease::Linear, 0, Some([0.0; 4]), t0);
        let t1 = t0 + Duration::from_millis(50);
        let v = a.value("k", "x", [0.0; 4], 100, Ease::Linear, 0, None, t1);
        assert!((v[0] - 5.0).abs() < 1e-3, "reversal must start at 5, not jump: {v:?}");
    }

    #[test]
    fn vanished_nodes_are_forgotten() {
        let mut a = Anim::default();
        let t0 = Instant::now();
        a.begin_frame();
        a.value("gone", "x", [1.0; 4], 100, Ease::Out, 0, Some([0.0; 4]), t0);
        a.end_frame();
        a.begin_frame(); // "gone" not touched this frame
        a.end_frame();
        assert!(!a.animating(t0));
    }
}
