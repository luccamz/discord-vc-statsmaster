use crate::state::{Pomodoro, Pomodoros};
use poise::serenity_prelude::{self as serenity, ChannelId, GuildId, Http, UserId};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, MutexGuard};
use std::time::Duration;
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::time::Instant;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// Button presses forwarded to the pomodoro task, tagged with the work round they belong to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Pause(u32),
    Resume(u32),
}

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
    pub db: sqlx::SqlitePool,
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

/// Deletes what the run left visible. The entry must already be removed: dropping it closes
/// the control channel, which ends the task.
fn stop(http: &Arc<Http>, p: Pomodoro) {
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
    let (control, rx) = mpsc::unbounded_channel();
    let entry = Pomodoro {
        id,
        allowed: [Some(cfg.work_channel), None],
        status_msg: None,
        control,
    };

    let old = lock(&pomodoros).insert(key, entry);
    if let Some(old) = old {
        stop(&http, old);
    }
    tokio::spawn(run(cfg, id, rx));
}

/// Forwards a button press to the user's pomodoro. Returns false if that run is gone.
pub fn control(pomodoros: &Pomodoros, key: (u64, u64), id: u64, msg: Control) -> bool {
    lock(pomodoros)
        .get(&key)
        .is_some_and(|p| p.id == id && p.control.send(msg).is_ok())
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

fn finish(cfg: &Config, id: u64) -> bool {
    let mut map = lock(&cfg.pomodoros);
    if !map.get(&cfg.key()).is_some_and(|p| p.id == id) {
        return false;
    }
    if let Some(p) = map.remove(&cfg.key()) {
        stop(&cfg.http, p);
    }
    true
}

struct Status {
    channel: ChannelId,
    notice: &'static str,
    title: String,
    color: u32,
    body: String,
    button: Option<serenity::CreateButton>,
}

/// Replaces the status message. Returns false if cancelled.
async fn announce(cfg: &Config, id: u64, status: Status) -> bool {
    // Notifications show only the content, not mentions or text inside embeds.
    let embed = serenity::CreateEmbed::new()
        .title(status.title)
        .description(status.body)
        .color(status.color);
    let mut message = serenity::CreateMessage::new()
        .content(format!("<@{}> {}", cfg.user, status.notice))
        .embed(embed);
    if let Some(button) = status.button {
        message = message.button(button);
    }
    let sent = status
        .channel
        .send_message(&*cfg.http, message)
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

/// Moves the user, accepting both channels while the move is in flight.
async fn move_user(cfg: &Config, id: u64, from: ChannelId, to: ChannelId) -> bool {
    if !update_own(cfg, id, |p| p.allowed = [Some(from), Some(to)]) {
        return false;
    }
    if cfg
        .guild
        .move_member(&*cfg.http, cfg.user, to)
        .await
        .is_err()
    {
        if finish(cfg, id) {
            let warning = format!(
                "<@{}> ⚠️ Pomodoro stopped: I could not move you to <#{to}>.",
                cfg.user
            );
            let _ = from
                .send_message(&*cfg.http, serenity::CreateMessage::new().content(warning))
                .await;
        }
        return false;
    }
    update_own(cfg, id, |p| p.allowed = [Some(to), None])
}

async fn count_work_cycle(cfg: &Config) {
    let (user_id, guild_id) = (cfg.user.get() as i64, cfg.guild.get() as i64);
    if let Err(e) = sqlx::query!(
        "INSERT INTO voice_stats (user_id, guild_id, work_cycles) VALUES (?, ?, 1) 
         ON CONFLICT(user_id, guild_id) DO UPDATE SET work_cycles = work_cycles + 1",
        user_id,
        guild_id
    )
    .execute(&cfg.db)
    .await
    {
        eprintln!("Failed to count work cycle for user {}: {}", user_id, e);
    }
}

enum Waited {
    Elapsed,
    Paused(Duration),
    Cancelled,
}

/// Sleeps until `duration` passes. Only a `Pause` for `pausable` stops it early; other
/// messages are dropped without moving the deadline.
async fn wait(
    rx: &mut UnboundedReceiver<Control>,
    duration: Duration,
    pausable: Option<u32>,
) -> Waited {
    let deadline = Instant::now() + duration;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => return Waited::Elapsed,
            msg = rx.recv() => match msg {
                None => return Waited::Cancelled,
                Some(Control::Pause(round)) if Some(round) == pausable => {
                    return Waited::Paused(deadline.saturating_duration_since(Instant::now()));
                }
                Some(_) => {}
            },
        }
    }
}

async fn wait_resume(rx: &mut UnboundedReceiver<Control>, round: u32) -> bool {
    loop {
        match rx.recv().await {
            None => return false,
            Some(Control::Resume(r)) if r == round => return true,
            Some(_) => {}
        }
    }
}

/// Progress like `● ● ◐ ○` for runs of up to 12 rounds; `done` counts finished work cycles.
fn round_dots(reps: Option<u32>, done: u32, in_progress: bool) -> String {
    match reps {
        Some(total) if total <= 12 => {
            let dots: Vec<&str> = (0..total)
                .map(|i| match i.cmp(&done) {
                    std::cmp::Ordering::Less => "●",
                    std::cmp::Ordering::Equal if in_progress => "◐",
                    _ => "○",
                })
                .collect();
            format!("\n{}", dots.join(" "))
        }
        _ => String::new(),
    }
}

fn ends_at(duration: Duration) -> i64 {
    chrono::Utc::now().timestamp() + duration.as_secs() as i64
}

/// Runs one work cycle, with any number of pauses. Returns false if the run ended.
async fn work(
    cfg: &Config,
    id: u64,
    rx: &mut UnboundedReceiver<Control>,
    round: u32,
    last: bool,
) -> bool {
    let rounds = cfg.reps.map_or("∞".to_string(), |r| r.to_string());
    let mut remaining = cfg.work;
    let mut notice = "🍅 Started work";
    loop {
        let next = if last { "done" } else { "break" };
        let pause = serenity::CreateButton::new(format!("pomo_pause_{}_{id}_{round}", cfg.user))
            .label("⏸ Pause")
            .style(serenity::ButtonStyle::Secondary);
        let status = Status {
            channel: cfg.work_channel,
            notice,
            title: format!("🍅 Focus · round {round}/{rounds}"),
            color: 0xE74C3C,
            body: format!(
                "{next} <t:{}:R>{}",
                ends_at(remaining),
                round_dots(cfg.reps, round - 1, true)
            ),
            button: Some(pause),
        };
        if !announce(cfg, id, status).await {
            return false;
        }

        match wait(rx, remaining, Some(round)).await {
            Waited::Cancelled => return false,
            Waited::Elapsed => {
                let alive = update_own(cfg, id, |_| ());
                if alive {
                    count_work_cycle(cfg).await;
                }
                return alive;
            }
            Waited::Paused(left) => remaining = left,
        }

        if !move_user(cfg, id, cfg.work_channel, cfg.lobby).await {
            return false;
        }
        let resume = serenity::CreateButton::new(format!("pomo_resume_{}_{id}_{round}", cfg.user))
            .label("▶ Resume")
            .style(serenity::ButtonStyle::Success);
        let secs = remaining.as_secs();
        let status = Status {
            channel: cfg.lobby,
            notice: "⏸ Paused work",
            title: format!("⏸ Paused · round {round}/{rounds}"),
            color: 0x95A5A6,
            body: format!(
                "{}m {}s of work left{}",
                secs / 60,
                secs % 60,
                round_dots(cfg.reps, round - 1, true)
            ),
            button: Some(resume),
        };
        if !announce(cfg, id, status).await
            || !wait_resume(rx, round).await
            || !move_user(cfg, id, cfg.lobby, cfg.work_channel).await
        {
            return false;
        }
        notice = "🍅 Resumed work";
    }
}

async fn take_break(cfg: &Config, id: u64, rx: &mut UnboundedReceiver<Control>, done: u32) -> bool {
    let status = Status {
        channel: cfg.lobby,
        notice: "☕ Started break",
        title: "☕ Break".to_string(),
        color: 0x2ECC71,
        body: format!(
            "back to work <t:{}:R>{}",
            ends_at(cfg.pause),
            round_dots(cfg.reps, done, false)
        ),
        button: None,
    };
    announce(cfg, id, status).await && matches!(wait(rx, cfg.pause, None).await, Waited::Elapsed)
}

async fn run(cfg: Config, id: u64, mut rx: UnboundedReceiver<Control>) {
    let mut phases = phase_sequence(cfg.reps).peekable();
    let mut round = 0;
    while let Some(phase) = phases.next() {
        let completed = match phase {
            Phase::Work => {
                round += 1;
                work(&cfg, id, &mut rx, round, phases.peek().is_none()).await
            }
            Phase::Pause => take_break(&cfg, id, &mut rx, round).await,
        };
        if !completed {
            return;
        }

        let Some(&next) = phases.peek() else { break };
        if !move_user(&cfg, id, cfg.channel(phase), cfg.channel(next)).await {
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

    #[tokio::test(start_paused = true)]
    async fn wait_elapses_and_ignores_stale_controls() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let start = Instant::now();
        tx.send(Control::Pause(1)).unwrap();
        tx.send(Control::Resume(2)).unwrap();
        let waited = wait(&mut rx, Duration::from_secs(60), Some(2)).await;
        assert!(matches!(waited, Waited::Elapsed));
        assert_eq!(start.elapsed(), Duration::from_secs(60));
    }

    #[tokio::test(start_paused = true)]
    async fn wait_pauses_with_time_left() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(20)).await;
            tx.send(Control::Pause(3)).unwrap();
            tokio::time::sleep(Duration::from_secs(3600)).await;
        });
        let waited = wait(&mut rx, Duration::from_secs(60), Some(3)).await;
        assert!(matches!(waited, Waited::Paused(left) if left == Duration::from_secs(40)));
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_the_sender_cancels() {
        let (tx, mut rx) = mpsc::unbounded_channel::<Control>();
        drop(tx);
        assert!(matches!(
            wait(&mut rx, Duration::from_secs(60), None).await,
            Waited::Cancelled
        ));
        assert!(!wait_resume(&mut rx, 1).await);
    }

    #[test]
    fn round_dots_show_progress() {
        assert_eq!(round_dots(Some(4), 2, true), "\n● ● ◐ ○");
        assert_eq!(round_dots(Some(4), 2, false), "\n● ● ○ ○");
        assert_eq!(round_dots(Some(1), 0, true), "\n◐");
        assert_eq!(round_dots(Some(13), 0, true), "");
        assert_eq!(round_dots(None, 5, true), "");
    }
}
