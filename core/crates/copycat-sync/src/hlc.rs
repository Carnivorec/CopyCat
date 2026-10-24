use copycat_crypto::DeviceId;

pub const MAX_FORWARD_DRIFT_MS: u64 = 60 * 60 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Hlc {
    pub wall_ms: u64,
    pub counter: u32,
    pub node: DeviceId,
}

impl Hlc {
    pub fn capped(self, phys_ms: u64) -> Self {
        let max = phys_ms.saturating_add(MAX_FORWARD_DRIFT_MS);
        if self.wall_ms > max { Hlc { wall_ms: max, ..self } } else { self }
    }
}

#[derive(Clone, Debug)]
pub struct HlcClock {
    wall_ms: u64,
    counter: u32,
    node: DeviceId,
}

impl HlcClock {
    pub fn new(node: DeviceId) -> Self {
        Self { wall_ms: 0, counter: 0, node }
    }

    pub fn restore(node: DeviceId, wall_ms: u64, counter: u32) -> Self {
        Self { wall_ms, counter, node }
    }

    pub fn last(&self) -> (u64, u32) {
        (self.wall_ms, self.counter)
    }

    pub fn now(&mut self, phys_ms: u64) -> Hlc {
        self.cap(phys_ms);
        if phys_ms > self.wall_ms {
            self.wall_ms = phys_ms;
            self.counter = 0;
        } else {
            self.bump();
        }
        Hlc { wall_ms: self.wall_ms, counter: self.counter, node: self.node }
    }

    pub fn cap(&mut self, phys_ms: u64) {
        let max = phys_ms.saturating_add(MAX_FORWARD_DRIFT_MS);
        if self.wall_ms > max {
            self.wall_ms = max;
        }
    }

    pub fn observe(&mut self, remote: &Hlc, phys_ms: u64) {
        let remote_wall = remote.wall_ms.min(phys_ms.saturating_add(MAX_FORWARD_DRIFT_MS));
        let wall = self.wall_ms.max(remote_wall).max(phys_ms);
        let counter = match (wall == self.wall_ms, wall == remote_wall) {
            (true, true) => self.counter.max(remote.counter),
            (true, false) => self.counter,
            (false, true) => remote.counter,
            (false, false) => 0,
        };
        self.wall_ms = wall;
        self.counter = counter;
    }

    fn bump(&mut self) {
        match self.counter.checked_add(1) {
            Some(c) => self.counter = c,
            None => {
                self.wall_ms += 1;
                self.counter = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(b: u8) -> DeviceId {
        DeviceId([b; 16])
    }

    #[test]
    fn monotonic_even_if_clock_goes_back() {
        let mut c = HlcClock::new(node(1));
        let a = c.now(1000);
        let b = c.now(900);
        let d = c.now(1000);
        assert!(a < b && b < d);
    }

    #[test]
    fn observe_orders_after_remote() {
        let mut a = HlcClock::new(node(1));
        let mut b = HlcClock::new(node(2));
        let t1 = a.now(5000);
        b.observe(&t1, 1000);
        let t2 = b.now(1000);
        assert!(t2 > t1, "因果后发生的事件必须排在后面");
    }

    #[test]
    fn far_future_remote_is_clamped() {
        let mut c = HlcClock::new(node(1));
        let evil = Hlc { wall_ms: u64::MAX / 2, counter: 0, node: node(9) };
        c.observe(&evil, 1000);
        assert!(c.now(1000).wall_ms <= 1000 + MAX_FORWARD_DRIFT_MS);
    }

    #[test]
    fn clock_set_back_far_is_capped() {
        let mut c = HlcClock::new(node(1));
        let future = c.now(10 * MAX_FORWARD_DRIFT_MS);
        let t = c.now(1000);
        assert!(t.wall_ms <= 1000 + MAX_FORWARD_DRIFT_MS && t < future);
        assert!(c.now(1000) > t, "拉回之后仍然单调递增");
    }

    #[test]
    fn capped_only_cuts_far_future() {
        let h = Hlc { wall_ms: 5000, counter: 3, node: node(2) };
        assert_eq!(h.capped(5000), h);
        let far = Hlc { wall_ms: u64::MAX, counter: 3, node: node(2) };
        assert_eq!(far.capped(1000), Hlc { wall_ms: 1000 + MAX_FORWARD_DRIFT_MS, counter: 3, node: node(2) });
    }

    #[test]
    fn ties_broken_by_node() {
        let x = Hlc { wall_ms: 1, counter: 0, node: node(1) };
        let y = Hlc { wall_ms: 1, counter: 0, node: node(2) };
        assert!(x < y);
    }
}
