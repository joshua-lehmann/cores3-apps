//! Countdown and tilt recognition for the standalone timer.

/// Preset durations in milliseconds.
pub const PRESETS: [u64; 6] = [30_000, 60_000, 120_000, 300_000, 600_000, 1_500_000];

/// Countdown driven by caller-supplied monotonic milliseconds.
#[derive(Debug)]
pub struct Countdown {
    /// Selected preset index.
    pub preset: usize,
    /// Time retained when stopped.
    remaining: u64,
    /// Configured duration retained across reset.
    duration: u64,
    /// Monotonic expiration while running.
    deadline: Option<u64>,
    /// Completion remains visible until reset or a new start.
    pub finished: bool,
}

impl Default for Countdown {
    fn default() -> Self {
        Self {
            preset: 0,
            remaining: PRESETS[0],
            duration: PRESETS[0],
            deadline: None,
            finished: false,
        }
    }
}
impl Countdown {
    /// Remaining milliseconds at `now`.
    pub fn remaining(&self, now: u64) -> u64 {
        self.deadline
            .map_or(self.remaining, |end| end.saturating_sub(now))
    }
    /// Whether the countdown is running.
    pub fn running(&self) -> bool {
        self.deadline.is_some()
    }
    /// Advance the clock and return true exactly once on completion.
    pub fn update(&mut self, now: u64) -> bool {
        if self.deadline.is_some_and(|end| now >= end) {
            self.deadline = None;
            self.remaining = 0;
            self.finished = true;
            true
        } else {
            false
        }
    }
    /// Start or pause, preserving elapsed time.
    pub fn toggle(&mut self, now: u64) {
        self.update(now);
        if self.running() {
            self.remaining = self.remaining(now);
            self.deadline = None;
        } else {
            if self.remaining == 0 {
                self.remaining = self.duration;
            }
            self.deadline = Some(now.saturating_add(self.remaining));
            self.finished = false;
        }
    }
    /// Reset the current preset.
    pub fn reset(&mut self) {
        self.deadline = None;
        self.remaining = self.duration;
        self.finished = false;
    }
    /// Select an adjacent preset while stopped.
    pub fn select(&mut self, direction: i8) {
        if !self.running() {
            self.preset = (self.preset as i32 + direction.signum() as i32)
                .rem_euclid(PRESETS.len() as i32) as usize;
            self.duration = PRESETS[self.preset];
            self.reset();
        }
    }
    /// Configured duration in milliseconds.
    pub fn duration(&self) -> u64 {
        self.duration
    }
    /// Adjust a stopped timer in ten-second increments, bounded to 10s–99min.
    pub fn adjust(&mut self, direction: i8) {
        if !self.running() {
            self.duration = step_duration(self.duration, direction);
            self.reset();
        }
    }
}

/// Apply one bounded ten-second adjustment.
pub fn step_duration(duration: u64, direction: i8) -> u64 {
    (duration as i64 + i64::from(direction.signum()) * 10_000).clamp(10_000, 5_940_000) as u64
}

/// Hold-repeat state; callers cancel on release or leaving the button.
#[derive(Default)]
pub struct HoldRepeat {
    /// Direction, press timestamp and next repeat deadline.
    held: Option<(i8, u64, u64)>,
}
impl HoldRepeat {
    /// Start a hold after applying the initial tap.
    pub fn press(&mut self, direction: i8, now: u64) {
        self.held = Some((direction, now, now.saturating_add(500)));
    }
    /// Current button direction, for cancelling when a finger leaves it.
    pub fn direction(&self) -> Option<i8> {
        self.held.map(|h| h.0)
    }
    /// Stop repeating immediately.
    pub fn cancel(&mut self) {
        self.held = None;
    }
    /// Produce at most one step per UI iteration, accelerating after two seconds.
    pub fn update(&mut self, now: u64) -> Option<i8> {
        let (direction, since, next) = self.held?;
        if now < next {
            return None;
        }
        let delay = if now.saturating_sub(since) >= 2_000 {
            80
        } else {
            200
        };
        self.held = Some((direction, since, now.saturating_add(delay)));
        Some(direction)
    }
}

/// Recognizes a sustained tilt after returning to level.
#[derive(Default)]
pub struct Tilt {
    /// A level reading has enabled the next gesture.
    armed: bool,
    /// Direction and start time of the current tilt.
    candidate: Option<(i8, u64)>,
}
impl Tilt {
    /// Disarm when samples are unavailable or selection is disabled.
    pub fn clear(&mut self) {
        self.armed = false;
        self.candidate = None;
    }
    /// Process gravity in screen coordinates and return a preset direction.
    pub fn sample(&mut self, down: [f32; 3], now: u64) -> Option<i8> {
        if down[1].abs() < 0.20 && down[2] > 0.7 {
            self.armed = true;
            self.candidate = None;
            return None;
        }
        if !self.armed {
            return None;
        }
        let direction = if down[2] > 0.3 && down[1] > 0.55 {
            1
        } else if down[2] > 0.3 && down[1] < -0.55 {
            -1
        } else {
            0
        };
        if direction == 0 {
            self.candidate = None;
            return None;
        }
        match self.candidate {
            Some((previous, since))
                if previous == direction && now.saturating_sub(since) >= 250 =>
            {
                self.clear();
                Some(direction)
            }
            Some((previous, _)) if previous == direction => None,
            _ => {
                self.candidate = Some((direction, now));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn countdown_pause_resume_and_completion() {
        let mut t = Countdown::default();
        t.toggle(100);
        assert_eq!(t.remaining(1100), 29_000);
        t.toggle(1100);
        assert_eq!(t.remaining(9000), 29_000);
        t.toggle(9000);
        assert!(!t.update(37_999));
        assert!(t.update(38_000));
        assert!(!t.update(40_000));
        assert!(t.finished);
        t.toggle(40_000);
        assert_eq!(t.remaining(40_000), 30_000);
    }
    #[test]
    fn selection_wraps_and_is_blocked_while_running() {
        let mut t = Countdown::default();
        t.select(-1);
        assert_eq!(t.preset, 5);
        t.toggle(0);
        t.select(1);
        assert_eq!(t.preset, 5);
        t.reset();
        t.select(1);
        assert_eq!(t.preset, 0);
    }
    #[test]
    fn tilt_requires_level_and_sustained_threshold_and_rearms() {
        let mut g = Tilt::default();
        let level = [0.0, 0.0, 1.0];
        let right = [0.0, 0.6, 0.8];
        assert_eq!(g.sample(right, 0), None);
        g.sample(level, 10);
        assert_eq!(g.sample(right, 20), None);
        assert_eq!(g.sample(right, 269), None);
        assert_eq!(g.sample(right, 270), Some(1));
        assert_eq!(g.sample(right, 1000), None);
        g.sample(level, 1010);
        g.sample(right, 1020);
        assert_eq!(g.sample([0.0, 0.3, 0.9], 1200), None);
        assert_eq!(g.sample(right, 1300), None);
    }
}

/// Two alternating countdown phases, driven by monotonic milliseconds.
#[derive(Debug)]
pub struct Intervals {
    /// Duration of each phase in milliseconds.
    durations: [u64; 2],
    /// Currently active phase, zero or one.
    phase: usize,
    /// Milliseconds left while stopped.
    remaining: u64,
    /// Deadline while running.
    deadline: Option<u64>,
    /// Number of full work and rest pairs requested.
    rounds: u8,
    /// Number of pairs completed.
    completed_rounds: u8,
    /// The final rest phase has ended.
    finished: bool,
}

impl Default for Intervals {
    fn default() -> Self {
        Self {
            durations: [30_000, 20_000],
            phase: 0,
            remaining: 30_000,
            deadline: None,
            rounds: 5,
            completed_rounds: 0,
            finished: false,
        }
    }
}
impl Intervals {
    /// The active phase index.
    pub fn phase(&self) -> usize {
        self.phase
    }
    /// The configured duration of a phase.
    pub fn duration(&self, phase: usize) -> u64 {
        self.durations[phase]
    }
    /// Whether the timer is running.
    pub fn running(&self) -> bool {
        self.deadline.is_some()
    }
    /// Remaining milliseconds at the supplied time.
    pub fn remaining(&self, now: u64) -> u64 {
        self.deadline
            .map_or(self.remaining, |end| end.saturating_sub(now))
    }
    /// Configured number of work/rest pairs.
    pub fn rounds(&self) -> u8 {
        self.rounds
    }
    /// Current round, one-based, including after completion.
    pub fn round(&self) -> u8 {
        (self.completed_rounds + 1).min(self.rounds)
    }
    /// Whether all configured rounds have finished.
    pub fn finished(&self) -> bool {
        self.finished
    }
    /// Set the number of rounds while stopped, resetting the session.
    pub fn set_rounds(&mut self, rounds: u8) {
        if !self.running() && (1..=99).contains(&rounds) {
            self.rounds = rounds;
            self.reset();
        }
    }
    /// Replace a phase duration while stopped. Zero durations are ignored.
    pub fn configure(&mut self, phase: usize, duration: u64) {
        if !self.running() && phase < 2 && duration > 0 {
            self.durations[phase] = duration;
            self.reset();
        }
    }
    /// Return to the start of phase one.
    pub fn reset(&mut self) {
        self.phase = 0;
        self.remaining = self.durations[0];
        self.deadline = None;
        self.completed_rounds = 0;
        self.finished = false;
    }
    /// Start or pause at the supplied time.
    pub fn toggle(&mut self, now: u64) {
        self.update(now);
        if self.running() {
            self.remaining = self.remaining(now);
            self.deadline = None;
        } else {
            if self.finished {
                self.reset();
            }
            self.deadline = Some(now.saturating_add(self.remaining));
        }
    }
    /// Advance through elapsed phases; return whether at least one ended.
    pub fn update(&mut self, now: u64) -> bool {
        let Some(mut end) = self.deadline else {
            return false;
        };
        if now < end {
            return false;
        }
        loop {
            if self.phase == 1 {
                self.completed_rounds += 1;
                if self.completed_rounds >= self.rounds {
                    self.deadline = None;
                    self.remaining = 0;
                    self.finished = true;
                    return true;
                }
            }
            self.phase ^= 1;
            end = end.saturating_add(self.durations[self.phase]);
            if now < end {
                break;
            }
        }
        self.deadline = Some(end);
        true
    }
}

#[cfg(test)]
mod interval_tests {
    use super::Intervals;
    #[test]
    fn alternates_without_drift_and_handles_long_delays() {
        let mut t = Intervals::default();
        t.toggle(100);
        assert!(t.update(30_100));
        assert_eq!(t.phase(), 1);
        assert_eq!(t.remaining(30_100), 20_000);
        assert!(t.update(125_100));
        assert_eq!(t.phase(), 0);
        assert_eq!(t.remaining(125_100), 5_000);
        assert!(!t.update(125_101));
    }
    #[test]
    fn completes_after_exact_rounds_and_restarts() {
        let mut t = Intervals::default();
        t.set_rounds(2);
        t.toggle(0);
        assert!(t.update(30_000));
        assert_eq!(t.round(), 1);
        assert!(t.update(50_000));
        assert_eq!(t.round(), 2);
        assert!(t.update(100_000));
        assert!(t.finished());
        assert!(!t.running());
        assert_eq!(t.remaining(200_000), 0);
        assert!(!t.update(200_000));
        t.toggle(200_000);
        assert!(!t.finished());
        assert_eq!(t.round(), 1);
    }
    #[test]
    fn pause_configuration_and_reset() {
        let mut t = Intervals::default();
        t.configure(1, 10_000);
        t.toggle(0);
        t.configure(1, 5_000);
        assert_eq!(t.duration(1), 10_000);
        t.toggle(10_000);
        assert_eq!(t.remaining(99_000), 20_000);
        t.toggle(99_000);
        assert!(t.update(119_000));
        assert_eq!(t.phase(), 1);
        t.reset();
        assert_eq!(t.phase(), 0);
        assert_eq!(t.remaining(0), 30_000);
    }
}

#[cfg(test)]
mod adjustment_tests {
    use super::*;
    #[test]
    fn custom_duration_survives_reset_and_running_cannot_change_it() {
        let mut timer = Countdown::default();
        timer.adjust(1);
        assert_eq!(timer.duration(), 40_000);
        timer.toggle(0);
        timer.adjust(-1);
        assert_eq!(timer.duration(), 40_000);
        timer.reset();
        assert_eq!(timer.remaining(0), 40_000);
        assert_eq!(step_duration(10_000, -1), 10_000);
        assert_eq!(step_duration(5_940_000, 1), 5_940_000);
    }
    #[test]
    fn hold_delays_accelerates_and_cancels_without_catchup() {
        let mut hold = HoldRepeat::default();
        hold.press(1, 0);
        assert_eq!(hold.update(499), None);
        assert_eq!(hold.update(500), Some(1));
        assert_eq!(hold.update(699), None);
        assert_eq!(hold.update(2000), Some(1));
        assert_eq!(hold.update(2079), None);
        assert_eq!(hold.update(2080), Some(1));
        hold.cancel();
        assert_eq!(hold.update(9999), None);
    }
}
