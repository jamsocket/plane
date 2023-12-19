use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

const PING_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
struct PingAttempt {
    /// A random number that identifies this attempt.
    id: u32,

    /// The time at which the ping was sent.
    time: Instant,
}

pub struct PingSender {
    current_attempt: Mutex<Option<PingAttempt>>,
}

impl PingSender {
    pub fn new() -> Self
    {
        let current_attempt = Mutex::default();

        Self { current_attempt }
    }

    pub async fn wait_for_next_attempt(&self) -> u32 {
        let current_attempt = self.current_attempt.lock().unwrap().take().clone();
        if let Some(current_attempt) = current_attempt {
            // Wait for the current attempt to finish.
            let elapsed = current_attempt.time.elapsed();
            if elapsed < PING_INTERVAL {
                tokio::time::sleep(PING_INTERVAL - elapsed).await;
            }
        }

        let mut current_attempt = self.current_attempt.lock().unwrap();
        let id = rand::random();
        *current_attempt = Some(PingAttempt {
            id,
            time: Instant::now(),
        });

        id
    }

    pub fn register_result(&self, id: u32) -> Option<Duration> {
        let mut current_attempt = self.current_attempt.lock().unwrap();
        if let Some(last_attempt) = current_attempt.take() {
            if last_attempt.id == id {
                return Some(last_attempt.time.elapsed())
            } else {
                *current_attempt = Some(last_attempt);
            }
        }
        None
    }
}
