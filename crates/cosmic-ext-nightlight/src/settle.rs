// SPDX-License-Identifier: MPL-2.0

//! Saving a slider that was moved without a drag.
//!
//! The windows save a slider's value on `on_release`, but iced's slider only
//! sends that when a mouse or touch drag ends. Ctrl+scroll and the arrow keys
//! (with the pointer over the slider) step it through `on_change` alone, so a
//! value set that way was previewed on the screen and never stored. The next
//! apply put the old one back, and the window went on treating the slider as
//! mid-drag, so it stopped following changes made elsewhere.
//!
//! [`Settle`] fills in the missing release: once a slider has sat still for
//! [`SETTLE`], it is treated as let go. A drag that pauses that long is saved
//! early too, which is harmless, since its release saves the final value again.

use std::time::{Duration, Instant};

use cosmic::app::Task;

/// How long a slider has to sit still before a scroll or key step counts as
/// done. Longer than the gap between key repeats or scroll ticks, so holding
/// a key or spinning the wheel saves once at the end rather than every step.
pub const SETTLE: Duration = Duration::from_millis(500);

/// One slider's timer. At most one is ever running: a step while it runs only
/// moves the deadline, and the timer re-arms itself for the rest of the wait.
#[derive(Debug, Default)]
pub struct Settle {
    /// When the slider last moved, or `None` once that move has been saved.
    last_step: Option<Instant>,
    /// Whether a timer is running, so a step doesn't start a second one.
    armed: bool,
}

/// What to do when a slider's timer goes off.
#[derive(Debug, PartialEq, Eq)]
pub enum Woke {
    /// Saved already, by a release. Nothing to do.
    Idle,
    /// Still for long enough. Save it as if it had been released.
    Settled,
    /// Moved since the timer started. Wait this much longer.
    Wait(Duration),
}

impl Settle {
    /// Notes a step. Returns how long to wait if a timer needs starting.
    pub fn stepped(&mut self, now: Instant) -> Option<Duration> {
        self.last_step = Some(now);
        (!std::mem::replace(&mut self.armed, true)).then_some(SETTLE)
    }

    /// Notes a release, which saves on its own, so a running timer finds
    /// nothing left to do.
    pub fn released(&mut self) {
        self.last_step = None;
    }

    /// Handles the timer going off.
    pub fn woke(&mut self, now: Instant) -> Woke {
        let Some(last_step) = self.last_step else {
            self.armed = false;
            return Woke::Idle;
        };
        let still = now.saturating_duration_since(last_step);
        if still >= SETTLE {
            self.last_step = None;
            self.armed = false;
            Woke::Settled
        } else {
            Woke::Wait(SETTLE - still)
        }
    }
}

/// Sends `message` after `delay`. A plain thread rather than an async timer,
/// because the applet and the settings window run on different executors and
/// only one of them has a timer.
pub fn after<M: Send + 'static>(delay: Duration, message: M) -> Task<M> {
    let (sender, receiver) = cosmic::iced::futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        let _ = sender.send(());
    });
    cosmic::task::future(async move {
        let _ = receiver.await;
        message
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_step_saves_once_the_slider_is_still() {
        let start = Instant::now();
        let mut settle = Settle::default();
        assert_eq!(settle.stepped(start), Some(SETTLE));
        assert_eq!(settle.woke(start + SETTLE), Woke::Settled);
    }

    /// Holding a key or spinning the wheel runs one timer, which waits out
    /// the last step before saving once.
    #[test]
    fn steps_in_a_row_share_one_timer_and_save_after_the_last() {
        let start = Instant::now();
        let mut settle = Settle::default();
        assert_eq!(settle.stepped(start), Some(SETTLE));
        let last = start + Duration::from_millis(300);
        assert_eq!(settle.stepped(last), None);
        assert_eq!(
            settle.woke(start + SETTLE),
            Woke::Wait(Duration::from_millis(300))
        );
        assert_eq!(settle.woke(last + SETTLE), Woke::Settled);
    }

    #[test]
    fn a_release_leaves_the_timer_nothing_to_save() {
        let start = Instant::now();
        let mut settle = Settle::default();
        settle.stepped(start);
        settle.released();
        assert_eq!(settle.woke(start + SETTLE), Woke::Idle);
    }

    /// Once a timer has finished, the next step has to start a new one.
    #[test]
    fn a_step_after_a_save_starts_a_new_timer() {
        let start = Instant::now();
        let mut settle = Settle::default();
        settle.stepped(start);
        settle.released();
        settle.woke(start + SETTLE);
        let later = start + Duration::from_secs(5);
        assert_eq!(settle.stepped(later), Some(SETTLE));
        assert_eq!(settle.woke(later + SETTLE), Woke::Settled);
    }
}
