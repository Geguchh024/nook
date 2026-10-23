//! A small, reusable spring-physics animator.
//!
//! Springs are integrated with fixed sub-steps (semi-implicit Euler), so the
//! motion is frame-rate independent: a 60 Hz and a 144 Hz display trace the
//! same curve.

/// Physical parameters of a damped harmonic oscillator.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringParams {
    pub stiffness: f32,
    pub damping: f32,
    pub mass: f32,
}

impl SpringParams {
    /// Critically damped spring: the fastest motion that never overshoots.
    pub fn critical(stiffness: f32) -> Self {
        let mass = 1.0;
        Self {
            stiffness,
            damping: 2.0 * (stiffness * mass).sqrt(),
            mass,
        }
    }

    /// Under-damped spring that overshoots its target by `overshoot`
    /// (a fraction, e.g. `0.04` for 4%) when released from rest.
    pub fn with_overshoot(stiffness: f32, overshoot: f32) -> Self {
        let mass = 1.0;
        let zeta = damping_ratio_for_overshoot(overshoot);
        Self {
            stiffness,
            damping: 2.0 * zeta * (stiffness * mass).sqrt(),
            mass,
        }
    }

    /// The default UI spring: critically damped and fairly snappy.
    pub fn snappy() -> Self {
        Self::critical(420.0)
    }

    /// Shell expand/collapse: about 4% overshoot.
    pub fn shell() -> Self {
        Self::with_overshoot(260.0, 0.04)
    }

    /// Damping ratio (zeta). 1.0 is critical, below 1.0 overshoots.
    pub fn damping_ratio(&self) -> f32 {
        self.damping / (2.0 * (self.stiffness * self.mass).sqrt())
    }
}

impl Default for SpringParams {
    fn default() -> Self {
        Self::snappy()
    }
}

/// Damping ratio that yields the given peak overshoot for a step response.
pub fn damping_ratio_for_overshoot(overshoot: f32) -> f32 {
    if overshoot <= 0.0 {
        return 1.0;
    }
    let ln = overshoot.min(0.99).ln();
    let pi = std::f32::consts::PI;
    (-ln / (pi * pi + ln * ln).sqrt()).clamp(0.0, 1.0)
}

const SUBSTEP: f32 = 1.0 / 480.0;
const MAX_DT: f32 = 1.0 / 15.0;
const REST_DISTANCE: f32 = 1e-3;
const REST_VELOCITY: f32 = 1e-2;

/// A one-dimensional spring that chases `target`.
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub value: f32,
    pub velocity: f32,
    pub target: f32,
    pub params: SpringParams,
}

impl Spring {
    pub fn new(value: f32, params: SpringParams) -> Self {
        Self {
            value,
            velocity: 0.0,
            target: value,
            params,
        }
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Jumps to `value` with no motion.
    pub fn snap(&mut self, value: f32) {
        self.value = value;
        self.target = value;
        self.velocity = 0.0;
    }

    /// Advances the simulation by `dt` seconds. Large gaps (the app was idle)
    /// are clamped so a spring never "teleports" through its whole motion in
    /// one hidden frame. Returns `true` while still moving.
    pub fn step(&mut self, dt: f32) -> bool {
        if self.is_settled() {
            self.value = self.target;
            self.velocity = 0.0;
            return false;
        }
        let mut remaining = dt.clamp(0.0, MAX_DT);
        let SpringParams {
            stiffness,
            damping,
            mass,
        } = self.params;
        let mass = mass.max(1e-4);
        while remaining > 0.0 {
            let h = remaining.min(SUBSTEP);
            let force = -stiffness * (self.value - self.target) - damping * self.velocity;
            self.velocity += force / mass * h;
            self.value += self.velocity * h;
            remaining -= h;
        }
        if self.is_settled() {
            self.value = self.target;
            self.velocity = 0.0;
            false
        } else {
            true
        }
    }

    pub fn is_settled(&self) -> bool {
        (self.value - self.target).abs() < REST_DISTANCE && self.velocity.abs() < REST_VELOCITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(spring: &mut Spring, seconds: f32, dt: f32) -> (f32, f32) {
        let mut peak = spring.value;
        let mut t = 0.0;
        while t < seconds {
            spring.step(dt);
            peak = peak.max(spring.value);
            t += dt;
        }
        (spring.value, peak)
    }

    #[test]
    fn critical_spring_settles_without_overshoot() {
        let mut s = Spring::new(0.0, SpringParams::critical(300.0));
        s.set_target(1.0);
        let (end, peak) = run(&mut s, 2.0, 1.0 / 60.0);
        assert!((end - 1.0).abs() < 1e-3, "end = {end}");
        assert!(peak <= 1.0 + 1e-4, "peak = {peak}");
        assert!(s.is_settled());
    }

    #[test]
    fn overshoot_spring_overshoots_about_four_percent() {
        let mut s = Spring::new(0.0, SpringParams::with_overshoot(260.0, 0.04));
        s.set_target(1.0);
        let (end, peak) = run(&mut s, 3.0, 1.0 / 240.0);
        assert!((end - 1.0).abs() < 1e-3);
        assert!((peak - 1.04).abs() < 0.006, "peak = {peak}");
    }

    #[test]
    fn frame_rate_independent() {
        let mut a = Spring::new(0.0, SpringParams::shell());
        let mut b = a;
        a.set_target(1.0);
        b.set_target(1.0);
        run(&mut a, 0.1, 1.0 / 60.0);
        run(&mut b, 0.1, 1.0 / 144.0);
        // Frame boundaries do not line up exactly, so allow a small slack.
        assert!(
            (a.value - b.value).abs() < 0.03,
            "{} vs {}",
            a.value,
            b.value
        );
    }

    #[test]
    fn huge_dt_is_clamped() {
        let mut s = Spring::new(0.0, SpringParams::snappy());
        s.set_target(100.0);
        s.step(10.0);
        assert!(s.value < 100.0);
        assert!(s.value.is_finite());
    }

    #[test]
    fn settled_spring_reports_idle() {
        let mut s = Spring::new(5.0, SpringParams::snappy());
        assert!(!s.step(1.0 / 60.0));
        s.snap(2.0);
        assert!(!s.step(1.0 / 60.0));
        assert_eq!(s.value, 2.0);
    }

    #[test]
    fn damping_ratio_round_trip() {
        assert!((damping_ratio_for_overshoot(0.04) - 0.7156).abs() < 1e-3);
        assert_eq!(damping_ratio_for_overshoot(0.0), 1.0);
        let p = SpringParams::critical(100.0);
        assert!((p.damping_ratio() - 1.0).abs() < 1e-6);
    }
}
