//! Properties of the reconnect schedule, over any run of delays and resets, from any attempt
//! count: delays never shrink until a reset and never pass the 30 s cap, `attempt` counts the
//! delays handed out (saturating, never wrapping), and a reset starts over at 1 s.

use std::time::Duration;

use hegel::TestCase;
use hegel::generators as gs;

use super::*;

const CAP: Duration = Duration::from_secs(30);

#[hegel::test]
fn delays_grow_to_the_cap_attempt_counts_them_and_reset_starts_over(tc: TestCase) {
    let start = tc.draw(hegel::one_of!(
        gs::just(0u32),
        gs::integers::<u32>().max_value(6),
        gs::integers::<u32>().min_value(u32::MAX - 6)
    ));
    let steps = tc.draw(gs::vecs(gs::sampled_from(vec!["delay", "reset"])).max_size(60));
    let mut backoff = Backoff { attempt: start };
    let mut previous: Option<Duration> = None;
    let mut expected_attempt = start;
    for step in steps {
        if step == "reset" {
            backoff.reset();
            assert_eq!(backoff.attempt(), 0);
            assert_eq!(backoff.next_delay(), Duration::from_secs(1), "a reset starts over at 1 s");
            previous = Some(Duration::from_secs(1));
            expected_attempt = 1;
            continue;
        }
        let delay = backoff.next_delay();
        expected_attempt = expected_attempt.saturating_add(1);
        assert!(delay <= CAP, "{delay:?}");
        assert!(previous.is_none_or(|p| p <= delay), "{previous:?} then {delay:?}");
        if previous == Some(CAP) {
            assert_eq!(delay, CAP, "it stays at the cap");
        }
        assert_eq!(backoff.attempt(), expected_attempt);
        previous = Some(delay);
    }
}
