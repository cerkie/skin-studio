//! Spinner behavior for autoplay, following lazer's `Spinner` (spins required, bonus ticks) and the auto generator
//! (the cursor spins fast enough to earn every bonus tick).
use std::f32::consts::TAU;

#[derive(Clone, Copy, Debug)]
pub struct SpinnerInfo {
    pub start: f32,
    pub end: f32,
    pub required: u32,
    pub max_bonus: u32,
    /// Revolutions per second of the autoplay spin.
    pub rps: f32,
}

fn range(od: f32, min: f32, mid: f32, max: f32) -> f32 {
    if od > 5.0 {
        mid + (max - mid) * (od - 5.0) / 5.0
    } else if od < 5.0 {
        mid - (mid - min) * (5.0 - od) / 5.0
    } else {
        mid
    }
}

impl SpinnerInfo {
    pub fn new(od: f32, start: f32, end: f32) -> SpinnerInfo {
        let secs = (end - start) / 1000.0;
        let min_rps = range(od, 90.0, 150.0, 225.0) / 60.0;
        let max_rps = range(od, 250.0, 380.0, 430.0) / 60.0;
        let required = (min_rps * secs + 1e-4) as u32;
        let max_bonus = ((max_rps * secs + 1e-4) as i32 - required as i32 - 2).max(0) as u32;
        SpinnerInfo { start, end, required, max_bonus, rps: max_rps }
    }

    pub fn duration(&self) -> f32 {
        self.end - self.start
    }

    pub fn total_ticks(&self) -> u32 {
        self.max_bonus + self.required + 2
    }

    /// Total rotation in degrees at time `t`.
    pub fn rotation(&self, t: f32) -> f32 {
        (t.clamp(self.start, self.end) - self.start) / 1000.0 * self.rps * 360.0
    }

    pub fn progress_unclamped(&self, t: f32) -> f32 {
        if self.required == 0 {
            return 1.0;
        }
        self.rotation(t) / 360.0 / self.required as f32
    }

    pub fn progress(&self, t: f32) -> f32 {
        self.progress_unclamped(t).clamp(0.0, 1.0)
    }

    /// When the spinner becomes cleared.
    pub fn completed_at(&self) -> f32 {
        if self.required == 0 {
            self.start
        } else {
            self.start + self.required as f32 / self.rps * 1000.0
        }
    }

    pub fn tick_time(&self, i: u32) -> f32 {
        self.start + (i + 1) as f32 / self.total_ticks() as f32 * self.duration()
    }

    /// Whether tick `i` is a bonus tick.
    pub fn is_bonus(&self, i: u32) -> bool {
        i >= self.required + 2
    }

    /// (bonus ticks passed at `t`, bonus score shown, is it the maximum)
    pub fn bonus_at(&self, t: f32) -> (u32, u32, bool) {
        let n = (0..self.total_ticks()).filter(|&i| self.is_bonus(i) && self.tick_time(i) <= t).count() as u32;
        (n, n * 1000, n == self.max_bonus && n > 0)
    }

    pub fn rpm(&self, t: f32) -> f32 {
        if t < self.start || t > self.end {
            0.0
        } else {
            self.rps * 60.0
        }
    }

    /// Angle of the autoplay cursor on its 50-unit circle at `t`, in radians.
    pub fn cursor_angle(&self, t: f32) -> f32 {
        self.rotation(t).to_radians() % TAU
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_spins_like_lazer() {
        // 4 s at OD 8: clear 150..225 rpm -> 195 rpm = 3.25 rps -> 13 spins
        let s = SpinnerInfo::new(8.0, 1000.0, 5000.0);
        assert_eq!(s.required, 13);
        assert!(s.max_bonus > 0);
        assert!(s.progress(5000.0) >= 1.0);
        assert!(s.completed_at() < 5000.0);
        assert!(s.bonus_at(5000.0).0 <= s.max_bonus);
    }
}
