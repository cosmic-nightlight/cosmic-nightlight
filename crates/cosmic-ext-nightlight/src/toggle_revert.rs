// SPDX-License-Identifier: MPL-2.0

//! Putting a toggle back when its apply doesn't land.
//!
//! A toggle stores its override straight away and applies it in the
//! background, and when the apply fails (the password prompt dismissed, most
//! often) the window puts the override back. Clicks don't wait for each other,
//! though. A second click while the first apply is still out, behind an open
//! password prompt say, is a second toggle in flight, and the override to put
//! back is then not simply the one each toggle found.
//!
//! Say the tint is off and the user turns it on, then off again while the
//! prompt for the first is up, then cancels the prompt. Both applies fail: the
//! first was refused, and the second is answered by that same refusal. Each
//! putting back what it found left the toggle on, the value the second click
//! found, over a screen that never changed.
//!
//! So [`Toggles`] treats the clicks made while any apply is out as one burst.
//! Only the burst's last apply may put anything back, and what it puts back is
//! the last override known to have reached the screen: the one from before the
//! burst, or the latest a toggle in it applied.

use crate::config::Override;

/// The toggles one window has in flight.
#[derive(Debug, Default)]
pub struct Toggles {
    /// Toggles whose apply hasn't answered yet.
    in_flight: u32,
    /// The last override known to describe the screen. Set when a burst starts
    /// and moved on by each apply in it that lands.
    landed: Option<Override>,
}

impl Toggles {
    /// A toggle is about to replace `current` and queue its apply.
    pub fn started(&mut self, current: Override) {
        if self.in_flight == 0 {
            self.landed = Some(current);
        }
        self.in_flight += 1;
    }

    /// The apply for a toggle that stored `wrote` has answered. `landed` is
    /// `None` when it never ran because a newer request took its place, which
    /// says nothing about the screen either way.
    ///
    /// Returns the override to put back, if any.
    pub fn finished(&mut self, wrote: Override, landed: Option<bool>) -> Option<Override> {
        self.in_flight = self.in_flight.saturating_sub(1);
        if landed == Some(true) {
            self.landed = Some(wrote);
        }
        if self.in_flight > 0 {
            return None;
        }
        let known = self.landed.take();
        if landed == Some(false) {
            known
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lone_failure_puts_back_what_it_found() {
        let mut toggles = Toggles::default();
        toggles.started(Override::Auto);
        assert_eq!(
            toggles.finished(Override::On, Some(false)),
            Some(Override::Auto)
        );
    }

    #[test]
    fn a_lone_success_puts_nothing_back() {
        let mut toggles = Toggles::default();
        toggles.started(Override::Auto);
        assert_eq!(toggles.finished(Override::On, Some(true)), None);
    }

    /// The case from the module docs: on, then off during the prompt, then the
    /// prompt cancelled. The toggle must end up off, as the screen is.
    #[test]
    fn a_refused_burst_puts_back_what_was_there_before_it() {
        let mut toggles = Toggles::default();
        toggles.started(Override::Auto);
        toggles.started(Override::On);
        assert_eq!(toggles.finished(Override::On, Some(false)), None);
        assert_eq!(
            toggles.finished(Override::Auto, Some(false)),
            Some(Override::Auto)
        );
    }

    /// The first prompt answered and the second cancelled. The screen shows the
    /// first toggle's change, so that is what the toggle goes back to.
    #[test]
    fn a_failure_after_a_success_puts_back_the_success() {
        let mut toggles = Toggles::default();
        toggles.started(Override::Auto);
        toggles.started(Override::On);
        assert_eq!(toggles.finished(Override::On, Some(true)), None);
        assert_eq!(
            toggles.finished(Override::Auto, Some(false)),
            Some(Override::On)
        );
    }

    /// An apply that never ran showed nothing, so it moves nothing on.
    #[test]
    fn a_superseded_apply_is_not_taken_as_landed() {
        let mut toggles = Toggles::default();
        toggles.started(Override::Auto);
        toggles.started(Override::On);
        toggles.started(Override::Auto);
        assert_eq!(toggles.finished(Override::Auto, None), None);
        assert_eq!(toggles.finished(Override::On, Some(false)), None);
        assert_eq!(
            toggles.finished(Override::On, Some(false)),
            Some(Override::Auto)
        );
    }

    /// Once a burst is over, the next one starts from what is stored then.
    #[test]
    fn each_burst_starts_afresh() {
        let mut toggles = Toggles::default();
        toggles.started(Override::Auto);
        assert_eq!(toggles.finished(Override::On, Some(true)), None);
        toggles.started(Override::On);
        assert_eq!(
            toggles.finished(Override::Off, Some(false)),
            Some(Override::On)
        );
    }
}
