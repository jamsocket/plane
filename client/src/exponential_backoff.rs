pub struct ExponentialBackoff {
    initial_duration_millis: i64,
    max_duration: Duration,
    defer_duration: Duration,
    multiplier: f64,
    step: i32,
    deferred_reset: Option<SystemTime>,
}

impl ExponentialBackoff {
    pub fn new(
        initial_duration: Duration,
        max_duration: Duration,
        multiplier: f64,
        defer_duration: Duration,
    ) -> Self {
        let initial_duration_millis = initial_duration.num_milliseconds();

        Self {
            initial_duration_millis,
            max_duration,
            multiplier,
            step: 0,
            defer_duration,
            deferred_reset: None,
        }
    }

    /// Reset the backoff, but only if `wait` is not called again for at least `defer_duration`.
    pub fn defer_reset(&mut self) {
        self.deferred_reset = Some(
            SystemTime::now()
                + self
                    .defer_duration
                    .to_std()
                    .expect("defer_duration is always valid"),
        );
    }

    pub async fn wait(&mut self) {
        if let Some(deferred_reset) = self.deferred_reset {
            self.deferred_reset = None;
            if SystemTime::now() > deferred_reset {
                self.reset();
                return;
            }
        }

        let duration = self.initial_duration_millis as f64 * self.multiplier.powi(self.step);
        let duration =
            Duration::try_milliseconds(duration as i64).expect("duration is always valid");
        let duration = duration.min(self.max_duration);

        tokio::time::sleep(duration.to_std().expect("duration is always valid")).await;

        self.step += 1;
    }

    pub fn reset(&mut self) {
        self.deferred_reset = None;
        self.step = 0;
    }
}

impl Default for ExponentialBackoff {
    fn default() -> Self {
        Self::new(
            Duration::try_seconds(1).expect("duration is always valid"),
            Duration::try_minutes(1).expect("duration is always valid"),
            1.1,
            Duration::try_minutes(1).expect("duration is always valid"),
        )
    }
}
