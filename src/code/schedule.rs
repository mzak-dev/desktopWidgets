//! When a Code Source's worker calls its module: a pure state machine, handed the time.
//! Only Instances that asked for a value are sampled, and with nothing due there is no
//! wake-up at all (ADR-0004).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

pub const MIN_REFRESH: Duration = Duration::from_secs(1);
/// After a sample that used the network.
pub const NET_MIN_REFRESH: Duration = Duration::from_secs(60);
pub const MAX_REFRESH: Duration = Duration::from_secs(24 * 3600);
/// When the module reports its own error.
pub const ERROR_REFRESH: Duration = Duration::from_secs(30);
/// The next call waits at least this many times the CPU the last one took.
pub const CPU_FACTOR: u32 = 100;
pub const BACKOFF_BASE: Duration = Duration::from_secs(10);
pub const BACKOFF_MAX: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Debug, PartialEq)]
pub enum Job {
    Sample(String),
    Act { instance: String, verb: String, arg: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resample {
    This,
    All,
    Nothing,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    /// A value; `refresh: None` means sample again only after an act or a param change.
    Sampled { refresh: Option<Duration>, used_net: bool, cpu: Duration },
    /// The module answered with its own error.
    Errored { refresh: Option<Duration>, used_net: bool, cpu: Duration },
    /// It trapped or ran out of fuel.
    Faulted { cpu: Duration },
    Acted { resample: Resample, cpu: Duration },
}

#[derive(Debug)]
struct Slot {
    params: String,
    due: Option<Instant>,
}

#[derive(Debug, Default)]
pub struct Schedule {
    slots: BTreeMap<String, Slot>,
    acts: VecDeque<(String, String, String)>,
    /// Nothing runs before this, after a fault.
    hold_until: Option<Instant>,
    faults: u32,
}

fn backoff(faults: u32) -> Duration {
    BACKOFF_BASE.saturating_mul(1u32 << faults.saturating_sub(1).min(16)).min(BACKOFF_MAX)
}

fn clamp_refresh(r: Duration, used_net: bool) -> Duration {
    let floor = if used_net { NET_MIN_REFRESH } else { MIN_REFRESH };
    r.clamp(floor, MAX_REFRESH)
}

impl Schedule {
    /// An Instance wants a value for `params` (JSON). Due at once when it is new or its
    /// params changed; returns whether that happened.
    pub fn need(&mut self, instance: &str, params: &str, now: Instant) -> bool {
        match self.slots.get_mut(instance) {
            Some(s) if s.params == params => false,
            Some(s) => {
                s.params = params.to_string();
                s.due = Some(now);
                true
            }
            None => {
                self.slots.insert(instance.to_string(), Slot { params: params.to_string(), due: Some(now) });
                true
            }
        }
    }

    pub fn act(&mut self, instance: &str, verb: &str, arg: &str) {
        self.acts.push_back((instance.to_string(), verb.to_string(), arg.to_string()));
    }

    /// Forgets Instances that are gone, so they stop costing anything.
    pub fn retain(&mut self, live: &BTreeSet<String>) {
        self.slots.retain(|id, _| live.contains(id));
        self.acts.retain(|(id, _, _)| live.contains(id));
    }

    pub fn params(&self, instance: &str) -> Option<&str> {
        self.slots.get(instance).map(|s| s.params.as_str())
    }

    fn held(&self, now: Instant) -> bool {
        self.hold_until.is_some_and(|t| now < t)
    }

    /// The next call to make now, actions first.
    pub fn next(&mut self, now: Instant) -> Option<Job> {
        if self.held(now) {
            return None;
        }
        if let Some((instance, verb, arg)) = self.acts.pop_front() {
            return Some(Job::Act { instance, verb, arg });
        }
        let (id, _) = self.slots.iter().filter_map(|(id, s)| s.due.filter(|d| *d <= now).map(|d| (id, d))).min_by_key(|(_, d)| *d)?;
        let id = id.clone();
        self.slots.get_mut(&id).expect("just found").due = None; // running; `done` sets the next one
        Some(Job::Sample(id))
    }

    pub fn done(&mut self, instance: &str, outcome: Outcome, now: Instant) {
        let stretch = |cpu: Duration| cpu.saturating_mul(CPU_FACTOR);
        let due = |r: Duration, cpu: Duration| Some(now + r.max(stretch(cpu)));
        match outcome {
            Outcome::Faulted { cpu } => {
                self.faults += 1;
                let until = now + backoff(self.faults).max(stretch(cpu));
                self.hold_until = Some(until);
                if let Some(s) = self.slots.get_mut(instance) {
                    s.due = Some(until);
                }
                return;
            }
            Outcome::Sampled { refresh, used_net, cpu } => {
                if let Some(s) = self.slots.get_mut(instance) {
                    s.due = refresh.and_then(|r| due(clamp_refresh(r, used_net), cpu));
                }
            }
            Outcome::Errored { refresh, used_net, cpu } => {
                if let Some(s) = self.slots.get_mut(instance) {
                    s.due = due(clamp_refresh(refresh.unwrap_or(ERROR_REFRESH).max(ERROR_REFRESH), used_net), cpu);
                }
            }
            Outcome::Acted { resample, cpu } => {
                let at = Some(now + stretch(cpu).min(MIN_REFRESH));
                match resample {
                    Resample::This => {
                        if let Some(s) = self.slots.get_mut(instance) {
                            s.due = at;
                        }
                    }
                    Resample::All => self.slots.values_mut().for_each(|s| s.due = at),
                    Resample::Nothing => {}
                }
            }
        }
        self.faults = 0;
        self.hold_until = None;
    }

    /// When to call `next` again; `None` means only a new message can make anything due.
    pub fn next_wake(&self, now: Instant) -> Option<Instant> {
        let earliest = if self.acts.is_empty() { self.slots.values().filter_map(|s| s.due).min()? } else { now };
        Some(match self.hold_until {
            Some(t) => earliest.max(t),
            None => earliest,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(refresh: Option<u64>) -> Outcome {
        Outcome::Sampled { refresh: refresh.map(Duration::from_secs), used_net: false, cpu: Duration::ZERO }
    }

    #[test]
    fn a_new_instance_is_due_at_once_then_at_its_refresh() {
        let (mut s, t0) = (Schedule::default(), Instant::now());
        assert!(s.need("w-1", "{}", t0));
        assert!(!s.need("w-1", "{}", t0), "asking again changes nothing");
        assert_eq!(s.next(t0), Some(Job::Sample("w-1".into())));
        assert_eq!(s.next(t0), None, "running, not due twice");
        s.done("w-1", ok(Some(5)), t0);
        assert_eq!(s.next_wake(t0), Some(t0 + Duration::from_secs(5)));
        assert_eq!(s.next(t0 + Duration::from_secs(4)), None);
        assert_eq!(s.next(t0 + Duration::from_secs(5)), Some(Job::Sample("w-1".into())));
        assert!(s.need("w-1", r#"{"city":"Oslo"}"#, t0), "new params: due again");
    }

    #[test]
    fn no_refresh_means_sampled_once() {
        let (mut s, t0) = (Schedule::default(), Instant::now());
        s.need("w-1", "{}", t0);
        s.next(t0);
        s.done("w-1", ok(None), t0);
        assert_eq!(s.next_wake(t0), None);
    }

    #[test]
    fn an_idle_schedule_never_wakes() {
        let t0 = Instant::now();
        assert_eq!(Schedule::default().next_wake(t0), None);
        let mut s = Schedule::default();
        s.need("w-1", "{}", t0);
        s.retain(&BTreeSet::new());
        assert_eq!(s.next_wake(t0), None);
    }

    #[test]
    fn refresh_is_clamped_and_network_samples_wait_a_minute() {
        let (mut s, t0) = (Schedule::default(), Instant::now());
        s.need("w-1", "{}", t0);
        s.next(t0);
        s.done("w-1", Outcome::Sampled { refresh: Some(Duration::from_millis(10)), used_net: false, cpu: Duration::ZERO }, t0);
        assert_eq!(s.next_wake(t0), Some(t0 + MIN_REFRESH));
        s.next(t0 + MIN_REFRESH);
        s.done("w-1", Outcome::Sampled { refresh: Some(Duration::from_secs(5)), used_net: true, cpu: Duration::ZERO }, t0);
        assert_eq!(s.next_wake(t0), Some(t0 + NET_MIN_REFRESH));
        s.next(t0 + NET_MIN_REFRESH);
        s.done("w-1", ok(Some(10 * 24 * 3600)), t0);
        assert_eq!(s.next_wake(t0), Some(t0 + MAX_REFRESH));
    }

    #[test]
    fn a_slow_call_stretches_its_next_refresh() {
        let (mut s, t0) = (Schedule::default(), Instant::now());
        s.need("w-1", "{}", t0);
        s.next(t0);
        s.done("w-1", Outcome::Sampled { refresh: Some(Duration::from_secs(1)), used_net: false, cpu: Duration::from_millis(50) }, t0);
        assert_eq!(s.next_wake(t0), Some(t0 + Duration::from_secs(5)), "50 ms of CPU: at most 1% of a core");
    }

    #[test]
    fn faults_back_off_and_reset_on_success() {
        let (mut s, t0) = (Schedule::default(), Instant::now());
        s.need("a", "{}", t0);
        s.need("b", "{}", t0);
        s.next(t0);
        s.done("a", Outcome::Faulted { cpu: Duration::ZERO }, t0);
        assert_eq!(s.next(t0), None, "nothing runs while held, not even b");
        assert_eq!(s.next_wake(t0), Some(t0 + BACKOFF_BASE));
        let t1 = t0 + BACKOFF_BASE;
        let job = s.next(t1).unwrap();
        let id = match job {
            Job::Sample(id) => id,
            other => panic!("{other:?}"),
        };
        s.done(&id, Outcome::Faulted { cpu: Duration::ZERO }, t1);
        assert_eq!(s.next_wake(t1), Some(t1 + BACKOFF_BASE * 2), "doubles");
        for _ in 0..20 {
            s.faults += 1;
        }
        assert_eq!(backoff(s.faults), BACKOFF_MAX);
        let t2 = t1 + BACKOFF_MAX;
        let Some(Job::Sample(id)) = s.next(t2) else { panic!() };
        s.done(&id, ok(Some(5)), t2);
        assert_eq!((s.faults, s.hold_until), (0, None));
    }

    #[test]
    fn a_module_error_is_retried_after_half_a_minute() {
        let (mut s, t0) = (Schedule::default(), Instant::now());
        s.need("w-1", "{}", t0);
        s.next(t0);
        s.done("w-1", Outcome::Errored { refresh: Some(Duration::from_secs(1)), used_net: false, cpu: Duration::ZERO }, t0);
        assert_eq!(s.next_wake(t0), Some(t0 + ERROR_REFRESH));
    }

    #[test]
    fn acts_run_before_due_samples() {
        let (mut s, t0) = (Schedule::default(), Instant::now());
        s.need("a", "{}", t0);
        s.need("b", "{}", t0);
        s.act("b", "refresh", "");
        assert_eq!(s.next(t0), Some(Job::Act { instance: "b".into(), verb: "refresh".into(), arg: String::new() }));
        s.next(t0);
        s.next(t0);
        s.done("a", ok(Some(600)), t0);
        s.done("b", ok(Some(600)), t0);
        s.act("b", "refresh", "");
        s.next(t0);
        s.done("b", Outcome::Acted { resample: Resample::This, cpu: Duration::ZERO }, t0);
        assert_eq!(s.next(t0), Some(Job::Sample("b".into())), "an act resamples its own Instance");
        assert_eq!(s.next(t0), None, "and not a");
        s.act("a", "reset", "");
        s.next(t0);
        s.done("a", Outcome::Acted { resample: Resample::All, cpu: Duration::ZERO }, t0);
        assert!(s.next(t0).is_some() && s.next(t0).is_some());
    }

    #[test]
    fn retain_forgets_removed_instances() {
        let (mut s, t0) = (Schedule::default(), Instant::now());
        s.need("a", "{}", t0);
        s.need("b", "{}", t0);
        s.act("b", "x", "");
        s.retain(&BTreeSet::from(["a".to_string()]));
        assert_eq!(s.next(t0), Some(Job::Sample("a".into())));
        assert_eq!((s.next(t0), s.params("b")), (None, None));
    }
}
