use std::time::{Duration, Instant};

pub struct Timer {
    started: Instant,
}

impl Timer {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Timer {
            started: Instant::now(),
        }
    }

    pub fn duration(&self) -> Duration {
        Instant::now().duration_since(self.started)
    }
}
