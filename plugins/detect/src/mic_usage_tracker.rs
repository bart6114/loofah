use std::collections::HashMap;
use std::time::Duration;

const COOLDOWN_DURATION: Duration = Duration::from_mins(10);

#[derive(Default)]
pub struct MicUsageTracker {
    cooldowns: HashMap<String, tokio::time::Instant>,
}

impl MicUsageTracker {
    pub fn claim(&mut self, app_id: &str) -> bool {
        let now = tokio::time::Instant::now();
        self.cooldowns
            .retain(|_, fired_at| now.duration_since(*fired_at) < COOLDOWN_DURATION);
        if self.cooldowns.contains_key(app_id) {
            return false;
        }
        self.cooldowns.insert(app_id.to_string(), now);
        true
    }
}
