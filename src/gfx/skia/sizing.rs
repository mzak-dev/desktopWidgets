//! How big a window's swapchain is, from ADR-0006: sized in buckets with slack, so a
//! live drag reconfigures a handful of times and not on every frame. Everything is
//! drawn from the top-left and the window clips the rest, so nothing is scaled.

use std::time::Duration;

pub const BUCKET: u32 = 128;
pub const MAX_SIDE: u32 = 8192;

/// How long a swapchain must sit far too big before it shrinks.
const SHRINK_AFTER: Duration = Duration::from_millis(600);

pub fn bucketed(v: u32) -> u32 {
    (v.div_ceil(BUCKET) * BUCKET + BUCKET).min(MAX_SIDE)
}

/// The size to reconfigure to, if `view` needs one: at once when the window outgrows
/// `cur`, and only after a while when `cur` is 4 buckets or more too big.
pub fn plan(cur: (u32, u32), view: (u32, u32), since_last: Duration) -> Option<(u32, u32)> {
    let (w, h) = (view.0.max(1), view.1.max(1));
    let (cw, ch) = cur;
    let grow = w > cw || h > ch;
    let shrink = cw >= w + 4 * BUCKET && ch >= h + 4 * BUCKET && since_last > SHRINK_AFTER;
    if !(grow || shrink) {
        return None;
    }
    let nw = if w > cw || shrink { bucketed(w) } else { cw };
    let nh = if h > ch || shrink { bucketed(h) } else { ch };
    Some((nw, nh))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONG: Duration = Duration::from_secs(5);

    #[test]
    fn a_size_gets_a_bucket_of_slack_and_never_exceeds_the_cap() {
        assert_eq!(bucketed(1), 256);
        assert_eq!(bucketed(128), 256);
        assert_eq!(bucketed(129), 384);
        assert_eq!(bucketed(100_000), MAX_SIDE);
    }

    #[test]
    fn a_window_inside_its_swapchain_changes_nothing() {
        assert_eq!(plan((512, 512), (500, 300), LONG), None);
        assert_eq!(plan((512, 512), (512, 512), Duration::ZERO), None);
    }

    #[test]
    fn outgrowing_it_reconfigures_at_once_and_only_the_side_that_grew() {
        assert_eq!(plan((512, 512), (600, 300), Duration::ZERO), Some((768, 512)));
        assert_eq!(plan((512, 512), (300, 600), Duration::ZERO), Some((512, 768)));
    }

    #[test]
    fn shrinking_waits_until_it_is_far_too_big_and_has_been_for_a_while() {
        assert_eq!(plan((2048, 2048), (500, 500), Duration::from_millis(100)), None, "too soon");
        assert_eq!(plan((2048, 2048), (500, 500), LONG), Some((640, 640)));
        assert_eq!(plan((1024, 1024), (600, 600), LONG), None, "not far enough over");
        assert_eq!(plan((2048, 640), (500, 500), LONG), None, "only one side is far over");
    }

    #[test]
    fn a_zero_window_is_treated_as_one_pixel() {
        assert_eq!(plan((512, 512), (0, 0), Duration::ZERO), None);
    }
}
