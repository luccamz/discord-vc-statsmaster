use crate::state::{Pomodoro, Pomodoros};
use poise::serenity_prelude::{self as serenity, ChannelId, GuildId, Http, UserId};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, MutexGuard};
use std::time::Duration;
use tokio::sync::Notify;

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

    fn duration(&self, phase: Phase) -> Duration {
        match phase {
            Phase::Work => self.work,
            Phase::Pause => self.pause,
        }
    }
}

fn lock(pomodoros: &Pomodoros) -> MutexGuard<'_, HashMap<(u64, u64), Pomodoro>> {
    pomodoros.lock().unwrap_or_else(|e| e.into_inner())
}

/// Signals the task and cleans up what it left visible. The entry must already be removed.
fn stop(http: &Arc<Http>, p: Pomodoro) {
    p.cancel.notify_one();
    if let Some((channel, msg)) = p.status_msg {
        let http = http.clone();
        tokio::spawn(async move {
            let _ = channel.delete_message(&*http, msg).await;
        });
    }
}

/// Starts a pomodoro for the user, replacing any running one.
pub fn start(cfg: Config) {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (key, http) = (cfg.key(), cfg.http.clone());
    let pomodoros = cfg.pomodoros.clone();
    let entry = Pomodoro {
        id,
        allowed: [Some(cfg.work_channel), None],
        status_msg: None,
        cancel: Arc::new(Notify::new()),
    };
    let cancel = entry.cancel.clone();

    let old = lock(&pomodoros).insert(key, entry);
    if let Some(old) = old {
        stop(&http, old);
    }
    tokio::spawn(run(cfg, id, cancel));
}

pub fn users_in_guild(pomodoros: &Pomodoros, guild: u64) -> Vec<(u64, u64)> {
    lock(pomodoros)
        .keys()
        .filter(|&&(_, g)| g == guild)
        .copied()
        .collect()
}

/// Cancels the user's pomodoro if they left the channel it expects them in.
pub fn on_voice_update(
    http: &Arc<Http>,
    pomodoros: &Pomodoros,
    key: (u64, u64),
    new: Option<ChannelId>,
) {
    let removed = {
        let mut map = lock(pomodoros);
        match map.get(&key) {
            Some(p) if should_cancel(&p.allowed, new) => map.remove(&key),
            _ => None,
        }
    };
    if let Some(p) = removed {
        stop(http, p);
    }
}

/// Returns false if this run was cancelled or replaced.
fn update_own(cfg: &Config, id: u64, f: impl FnOnce(&mut Pomodoro)) -> bool {
    match lock(&cfg.pomodoros).get_mut(&cfg.key()) {
        Some(p) if p.id == id => {
            f(p);
            true
        }
        _ => false,
    }
}

fn remove_own(cfg: &Config, id: u64) -> Option<Pomodoro> {
    let mut map = lock(&cfg.pomodoros);
    if map.get(&cfg.key()).is_some_and(|p| p.id == id) {
        map.remove(&cfg.key())
    } else {
        None
    }
}

/// Posts the phase status in the VC chat and DMs the user. Returns false if cancelled.
async fn announce(cfg: &Config, id: u64, phase: Phase, round: u32, last: bool) -> bool {
    let ends = chrono::Utc::now().timestamp() + cfg.duration(phase).as_secs() as i64;
    let rounds = cfg.reps.map_or("∞".to_string(), |r| r.to_string());
    let (title, color, next) = match phase {
        Phase::Work => (
            format!("🍅 Focus · round {round}/{rounds}"),
            0xE74C3C,
            if last { "done" } else { "break" },
        ),
        Phase::Pause => ("☕ Break".to_string(), 0x2ECC71, "back to work"),
    };

    // The mention sits in the content because mentions inside embeds do not notify.
    let embed = serenity::CreateEmbed::new()
        .title(title)
        .description(format!("{next} <t:{ends}:R>"))
        .color(color);
    let sent = cfg
        .channel(phase)
        .send_message(
            &*cfg.http,
            serenity::CreateMessage::new()
                .content(format!("<@{}>", cfg.user))
                .embed(embed),
        )
        .await
        .ok()
        .map(|m| (m.channel_id, m.id));

    let mut previous = None;
    let alive = update_own(cfg, id, |p| {
        previous = std::mem::replace(&mut p.status_msg, sent)
    });
    if !alive {
        previous = sent;
    }
    if let Some((channel, msg)) = previous {
        let _ = channel.delete_message(&*cfg.http, msg).await;
    }
    alive
}

fn finish(cfg: &Config, id: u64) -> bool {
    let Some(p) = remove_own(cfg, id) else {
        return false;
    };
    stop(&cfg.http, p);
    true
}

async fn run(cfg: Config, id: u64, cancel: Arc<Notify>) {
    let mut phases = phase_sequence(cfg.reps).peekable();
    let mut round = 0;
    while let Some(phase) = phases.next() {
        if phase == Phase::Work {
            round += 1;
        }
        if !announce(&cfg, id, phase, round, phases.peek().is_none()).await {
            return;
        }
        tokio::select! {
            _ = tokio::time::sleep(cfg.duration(phase)) => {}
            _ = cancel.notified() => return,
        }

        let Some(&next) = phases.peek() else { break };
        let (from, to) = (cfg.channel(phase), cfg.channel(next));
        if !update_own(&cfg, id, |p| p.allowed = [Some(from), Some(to)]) {
            return;
        }
        if cfg
            .guild
            .move_member(&*cfg.http, cfg.user, to)
            .await
            .is_err()
        {
            if finish(&cfg, id) {
                let warning = format!(
                    "<@{}> ⚠️ Pomodoro stopped: I could not move you to <#{to}>.",
                    cfg.user
                );
                let _ = from
                    .send_message(&*cfg.http, serenity::CreateMessage::new().content(warning))
                    .await;
            }
            return;
        }
        if !update_own(&cfg, id, |p| p.allowed = [Some(to), None]) {
            return;
        }
    }

    // Remove first so the disconnect's voice update finds no entry to cancel.
    if finish(&cfg, id) {
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
