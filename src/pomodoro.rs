use crate::state::{Pomodoro, Pomodoros};
use poise::serenity_prelude::{ChannelId, GuildId, Http, UserId};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, MutexGuard};
use std::time::Duration;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Work,
    Pause,
}

/// `reps` work phases interleaved with `reps - 1` pauses; endless when `None`.
pub fn phase_sequence(reps: Option<u32>) -> impl Iterator<Item = Phase> {
    let total = reps.map_or(usize::MAX, |r| 2 * r.max(1) as usize - 1);
    [Phase::Work, Phase::Pause].into_iter().cycle().take(total)
}

pub fn should_cancel(allowed: &[Option<ChannelId>; 2], new: Option<ChannelId>) -> bool {
    new.is_none() || !allowed.contains(&new)
}

pub struct Config {
    pub http: Arc<Http>,
    pub pomodoros: Pomodoros,
    pub guild: GuildId,
    pub user: UserId,
    pub work_channel: ChannelId,
    pub lobby: ChannelId,
    pub work: Duration,
    pub pause: Duration,
    pub reps: Option<u32>,
}

impl Config {
    fn key(&self) -> (u64, u64) {
        (self.user.get(), self.guild.get())
    }

    fn channel(&self, phase: Phase) -> ChannelId {
        match phase {
            Phase::Work => self.work_channel,
            Phase::Pause => self.lobby,
        }
    }
}

fn lock(pomodoros: &Pomodoros) -> MutexGuard<'_, HashMap<(u64, u64), Pomodoro>> {
    pomodoros.lock().unwrap_or_else(|e| e.into_inner())
}

/// Starts a pomodoro for the user, replacing any running one.
pub fn start(cfg: Config) {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let key = cfg.key();
    let pomodoros = cfg.pomodoros.clone();
    let allowed = [Some(cfg.work_channel), None];

    // Spawn under the lock so the task cannot observe the map before its entry exists.
    let mut map = lock(&pomodoros);
    let task = tokio::spawn(run(cfg, id));
    if let Some(old) = map.insert(key, Pomodoro { id, allowed, task }) {
        old.task.abort();
    }
}

/// Cancels the user's pomodoro if they left the channel it expects them in.
pub fn on_voice_update(pomodoros: &Pomodoros, key: (u64, u64), new: Option<ChannelId>) {
    let mut map = lock(pomodoros);
    if map
        .get(&key)
        .is_some_and(|p| should_cancel(&p.allowed, new))
    {
        if let Some(p) = map.remove(&key) {
            p.task.abort();
        }
    }
}

/// Returns false if this run was cancelled or replaced.
fn set_allowed(cfg: &Config, id: u64, allowed: [Option<ChannelId>; 2]) -> bool {
    match lock(&cfg.pomodoros).get_mut(&cfg.key()) {
        Some(p) if p.id == id => {
            p.allowed = allowed;
            true
        }
        _ => false,
    }
}

fn remove_own(cfg: &Config, id: u64) -> bool {
    let mut map = lock(&cfg.pomodoros);
    if map.get(&cfg.key()).is_some_and(|p| p.id == id) {
        map.remove(&cfg.key());
        true
    } else {
        false
    }
}

async fn run(cfg: Config, id: u64) {
    let mut phases = phase_sequence(cfg.reps).peekable();
    while let Some(phase) = phases.next() {
        let duration = match phase {
            Phase::Work => cfg.work,
            Phase::Pause => cfg.pause,
        };
        tokio::time::sleep(duration).await;

        let Some(&next) = phases.peek() else { break };
        let (from, to) = (cfg.channel(phase), cfg.channel(next));
        if !set_allowed(&cfg, id, [Some(from), Some(to)]) {
            return;
        }
        if cfg
            .guild
            .move_member(&*cfg.http, cfg.user, to)
            .await
            .is_err()
        {
            remove_own(&cfg, id);
            return;
        }
        if !set_allowed(&cfg, id, [Some(to), None]) {
            return;
        }
    }

    // Remove first so the disconnect's voice update finds no entry to cancel.
    if remove_own(&cfg, id) {
        let _ = cfg.guild.disconnect_member(&*cfg.http, cfg.user).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Phase::*;

    #[test]
    fn finite_sequence_starts_and_ends_with_work() {
        assert_eq!(phase_sequence(Some(1)).collect::<Vec<_>>(), [Work]);
        assert_eq!(
            phase_sequence(Some(3)).collect::<Vec<_>>(),
            [Work, Pause, Work, Pause, Work]
        );
    }

    #[test]
    fn infinite_sequence_alternates() {
        let seq: Vec<_> = phase_sequence(None).take(1000).collect();
        assert_eq!(seq.len(), 1000);
        assert!(seq.chunks(2).all(|c| c == [Work, Pause]));
    }

    #[test]
    fn cancel_only_outside_allowed_channels() {
        let (a, b, c) = (ChannelId::new(1), ChannelId::new(2), ChannelId::new(3));
        let single = [Some(a), None];
        let moving = [Some(a), Some(b)];
        assert!(!should_cancel(&single, Some(a)));
        assert!(should_cancel(&single, Some(b)));
        assert!(should_cancel(&single, None));
        assert!(!should_cancel(&moving, Some(b)));
        assert!(should_cancel(&moving, Some(c)));
        assert!(should_cancel(&moving, None));
    }
}
