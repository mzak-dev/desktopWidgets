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

pub struct Anim {
    map: HashMap<(String, &'static str), (Tween, u64)>,
    frame: u64,
    /// Scales every duration and delay (`duration_factor` of `anim-speed`); 0 jumps to the target.
    pub duration_factor: f32,
}

impl Default for Anim {
    fn default() -> Self {
        Self { map: HashMap::new(), frame: 0, duration_factor: 1.0 }
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

    pub fn animating(&self, now: Instant) -> bool {
        self.map.values().any(|(t, _)| !t.done(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
