//! Pružinové animácie (semi-implicitný Euler s pevným krokom).

#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub x: f32,
    pub v: f32,
    pub target: f32,
    pub k: f32, // tuhosť
    pub c: f32, // tlmenie
}

impl Spring {
    pub fn new(x: f32) -> Spring {
        // mierne podkritické tlmenie = jemný „dynamic island“ presah
        Spring { x, v: 0.0, target: x, k: 260.0, c: 24.0 }
    }
    pub fn with(x: f32, k: f32, c: f32) -> Spring {
        Spring { x, v: 0.0, target: x, k, c }
    }
    pub fn set(&mut self, t: f32) {
        self.target = t;
    }
    pub fn snap(&mut self, t: f32) {
        self.x = t;
        self.v = 0.0;
        self.target = t;
    }
    pub fn settled(&self) -> bool {
        (self.x - self.target).abs() < 0.05 && self.v.abs() < 0.05
    }
    pub fn step(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, 0.1);
        let h: f32 = 1.0 / 240.0;
        let mut t: f32 = 0.0;
        while t < dt {
            let s = h.min(dt - t);
            let a = self.k * (self.target - self.x) - self.c * self.v;
            self.v += a * s;
            self.x += self.v * s;
            t += s;
        }
        if self.settled() {
            self.x = self.target;
            self.v = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn converges() {
        let mut s = Spring::new(0.0);
        s.set(100.0);
        for _ in 0..240 {
            s.step(1.0 / 60.0);
        }
        assert!(s.settled());
        assert_eq!(s.x, 100.0);
    }
    #[test]
    fn stable_with_big_dt() {
        let mut s = Spring::new(0.0);
        s.set(10.0);
        s.step(5.0);
        assert!(s.x.is_finite() && s.x.abs() < 100.0);
    }
}
