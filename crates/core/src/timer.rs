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
                self.remaining = PRESETS[self.preset];
            }
            self.deadline = Some(now.saturating_add(self.remaining));
            self.finished = false;
        }
    }
    /// Reset the current preset.
    pub fn reset(&mut self) {
        self.deadline = None;
        self.remaining = PRESETS[self.preset];
        self.finished = false;
    }
    /// Select an adjacent preset while stopped.
    pub fn select(&mut self, direction: i8) {
        if !self.running() {
            self.preset = (self.preset as i32 + direction.signum() as i32)
                .rem_euclid(PRESETS.len() as i32) as usize;
            self.reset();
        }
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
