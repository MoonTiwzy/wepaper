//! The daemon goes when it is told to, and gives up when it is stuck.
//!
//! Everything the daemon does it does in one loop: it draws, answers its socket and
//! sees that it was signalled there. A loop caught in a call that never returns (a
//! driver, a player, a library waiting on another process) does none of that: the
//! wallpaper stands, its sounds play on, `quit` gets no answer and SIGTERM sets a flag
//! nobody reads. A thread of its own looks at the loop from outside:
//!
//! - told to stop (a signal, `quit`), the daemon has [`GRACE`] to stop the proper way,
//!   and is gone after it either way;
//! - at work on one round of the loop for [`STUCK`], it exits with [`STUCK_STATUS`]:
//!   whoever started it can start it again, which nobody can do with one that hangs.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How long a daemon told to stop may take over it.
pub const GRACE: Duration = Duration::from_secs(3);
/// How long one round of the loop may take: loading the largest scene is seconds.
pub const STUCK: Duration = Duration::from_secs(30);
/// The exit status of a daemon that gave up on a stuck loop (EX_SOFTWARE).
pub const STUCK_STATUS: i32 = 70;

/// How often the loop is looked at.
const EVERY: Duration = Duration::from_millis(200);

/// When the loop's current round began, in milliseconds since the watch started, plus
/// one; zero while the loop waits for something to do.
static WORKING: AtomicU64 = AtomicU64::new(0);
/// When the daemon was told to stop, counted the same way; zero until it is.
static LEAVING: AtomicU64 = AtomicU64::new(0);

/// What the watch makes of the loop.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Fine,
    /// Told to stop and still here after [`GRACE`].
    Overdue,
    /// One round of the loop took [`STUCK`].
    Stuck,
}

/// `working` and `leaving` are how long ago the round began and the daemon was told to
/// stop. Leaving comes first: a daemon told to stop while stuck is overdue, not stuck.
fn verdict(working: Option<Duration>, leaving: Option<Duration>) -> Verdict {
    if leaving.is_some_and(|since| since >= GRACE) {
        Verdict::Overdue
    } else if working.is_some_and(|since| since >= STUCK) {
        Verdict::Stuck
    } else {
        Verdict::Fine
    }
}

fn now(start: Instant) -> u64 {
    start.elapsed().as_millis() as u64 + 1
}

fn since(mark: &AtomicU64, start: Instant) -> Option<Duration> {
    match mark.load(Ordering::Relaxed) {
        0 => None,
        at => Some(Duration::from_millis(now(start).saturating_sub(at))),
    }
}

/// The daemon's side of the watch.
pub struct Watch {
    start: Instant,
}

impl Watch {
    /// Starts watching. `signalled` is what SIGINT and SIGTERM set.
    pub fn start(signalled: &'static AtomicBool) -> Self {
        let start = Instant::now();
        let spawned = std::thread::Builder::new().name("wepaper-watch".into()).spawn(move || {
            loop {
                std::thread::sleep(EVERY);
                if signalled.load(Ordering::Relaxed) {
                    let _ = LEAVING.compare_exchange(0, now(start), Ordering::Relaxed, Ordering::Relaxed);
                }
                match verdict(since(&WORKING, start), since(&LEAVING, start)) {
                    Verdict::Fine => {}
                    Verdict::Overdue => {
                        log::warn!("not stopped {} s after being told to: going at once", GRACE.as_secs());
                        // SAFETY: ends the process without running anything of it: what
                        // would run is what does not return.
                        unsafe { libc::_exit(0) };
                    }
                    Verdict::Stuck => {
                        log::error!("stuck for {} s: giving up", STUCK.as_secs());
                        // SAFETY: as above.
                        unsafe { libc::_exit(STUCK_STATUS) };
                    }
                }
            }
        });
        if let Err(why) = spawned {
            log::warn!("nothing watches the daemon: {why}");
        }
        Self { start }
    }

    /// A round of the loop begins.
    pub fn working(&self) {
        WORKING.store(now(self.start), Ordering::Relaxed);
    }

    /// The loop waits for something to do.
    pub fn resting(&self) {
        WORKING.store(0, Ordering::Relaxed);
    }

    /// The daemon was told to stop: what is left to do has [`GRACE`].
    pub fn leaving(&self) {
        let _ = LEAVING.compare_exchange(0, now(self.start), Ordering::Relaxed, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn a_loop_that_waits_or_works_briefly_is_fine() {
        assert_eq!(verdict(None, None), Verdict::Fine);
        assert_eq!(verdict(Some(5 * SECOND), None), Verdict::Fine);
    }

    #[test]
    fn a_round_that_takes_half_a_minute_is_stuck() {
        assert_eq!(verdict(Some(STUCK - SECOND), None), Verdict::Fine);
        assert_eq!(verdict(Some(STUCK), None), Verdict::Stuck);
    }

    #[test]
    fn a_daemon_told_to_stop_has_a_few_seconds() {
        assert_eq!(verdict(None, Some(GRACE - SECOND)), Verdict::Fine);
        assert_eq!(verdict(None, Some(GRACE)), Verdict::Overdue);
    }

    #[test]
    fn a_stuck_daemon_told_to_stop_goes_as_one_told_to_stop() {
        // Its status is that of a daemon that stopped, not of one that failed.
        assert_eq!(verdict(Some(2 * STUCK), Some(GRACE)), Verdict::Overdue);
        // And it does not wait out the half minute first.
        assert_eq!(verdict(Some(SECOND), Some(GRACE)), Verdict::Overdue);
    }
}
