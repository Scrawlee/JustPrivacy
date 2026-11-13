use std::{collections::HashMap, sync::Mutex, time::{Duration, Instant}};

/// Limiteur à fenêtre fixe, en mémoire uniquement : aucune adresse IP n'est écrite sur disque.
#[derive(Default)]
pub struct RateLimiter(Mutex<HashMap<String, (Instant, u32)>>);

impl RateLimiter {
    /// Renvoie `true` si l'action est autorisée.
    pub fn hit(&self, key: &str, max: u32, window: Duration) -> bool {
        let now = Instant::now();
        let mut map = self.0.lock().unwrap();
        if map.len() > 10_000 {
            map.retain(|_, (start, _)| now.duration_since(*start) < Duration::from_secs(3600));
        }
        let entry = map.entry(key.to_string()).or_insert((now, 0));
        if now.duration_since(entry.0) >= window {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= max
    }
}
