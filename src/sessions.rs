use crate::state::{Data, Error, Pomodoros, Prompts, SessionData};
use poise::serenity_prelude as serenity;
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::Mutex;

/// Adds the session's time to the user's weekly total and to the active task, atomically.
pub async fn commit(
    db: &SqlitePool,
    user_id: i64,
    guild_id: i64,
    session: &SessionData,
    now: i64,
) -> Result<(), Error> {
    let duration = now - session.start_time;
    if !session.counted || duration <= 0 {
        return Ok(());
    }

    let mut tx = db.begin().await?;
    sqlx::query!(
        "INSERT INTO voice_stats (user_id, guild_id, total_seconds) 
         VALUES (?, ?, ?) ON CONFLICT(user_id, guild_id) 
         DO UPDATE SET total_seconds = total_seconds + ?",
        user_id,
        guild_id,
        duration,
        duration
    )
    .execute(&mut *tx)
    .await?;

    if let Some(task_id) = session.active_task_id {
        sqlx::query!(
            "UPDATE user_tasks 
             SET time_spent_seconds = time_spent_seconds + ?,
                 record_session_seconds = MAX(record_session_seconds, ?)
             WHERE task_id = ?",
            duration,
            duration,
            task_id
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Reconciles sessions with the voice states Discord sends on startup and on reconnect,
/// because voice updates missed while disconnected are not replayed.
pub async fn sync_guild(
    http: &Arc<serenity::Http>,
    data: &Data,
    guild: &serenity::Guild,
) -> Result<(), Error> {
    let guild_id = guild.id.get();
    let gid = guild_id as i64;
    let tracked: HashSet<u64> = sqlx::query_scalar!(
        "SELECT channel_id FROM tracked_channels WHERE guild_id = ?",
        gid
    )
    .fetch_all(&data.db)
    .await?
    .into_iter()
    .map(|c| c as u64)
    .collect();

    let in_tracked: HashMap<u64, u64> = guild
        .voice_states
        .values()
        .filter_map(|v| Some((v.user_id.get(), v.channel_id?.get())))
        .filter(|(_, channel)| tracked.contains(channel))
        .collect();

    let mut last_tasks = HashMap::new();
    for &user in in_tracked.keys() {
        let uid = user as i64;
        let task = sqlx::query_scalar!(
            "SELECT task_id FROM user_tasks 
             WHERE user_id = ? AND is_last_selected = 1 
               AND completed_at IS NULL AND terminated_at IS NULL",
            uid
        )
        .fetch_optional(&data.db)
        .await?
        .flatten();
        last_tasks.insert(user, task);
    }

    let now = chrono::Utc::now().timestamp();
    let stale: Vec<(u64, SessionData)> = {
        let mut sessions = data.active_sessions.lock().await;
        let stale_users: Vec<u64> = sessions
            .iter()
            .filter(|(&(user, g), s)| g == guild_id && in_tracked.get(&user) != Some(&s.channel_id))
            .map(|(&(user, _), _)| user)
            .collect();
        let stale = stale_users
            .into_iter()
            .filter_map(|user| Some((user, sessions.remove(&(user, guild_id))?)))
            .collect();
        for (&user, &channel) in &in_tracked {
            sessions.entry((user, guild_id)).or_insert(SessionData {
                start_time: now,
                active_task_id: last_tasks[&user],
                channel_id: channel,
                counted: true,
            });
        }
        stale
    };
    for (user, session) in stale {
        commit(&data.db, user as i64, gid, &session, now).await?;
    }

    for key in crate::pomodoro::users_in_guild(&data.pomodoros, guild_id) {
        let channel = guild
            .voice_states
            .get(&serenity::UserId::new(key.0))
            .and_then(|v| v.channel_id);
        crate::pomodoro::on_voice_update(http, &data.pomodoros, key, channel);
    }
    Ok(())
}

/// Commits all open sessions on SIGTERM or Ctrl+C, deletes the bot's live messages, then
/// stops the shards.
pub async fn flush_on_shutdown(
    sessions: Arc<Mutex<HashMap<(u64, u64), SessionData>>>,
    db: SqlitePool,
    shards: Arc<serenity::ShardManager>,
    http: Arc<serenity::Http>,
    pomodoros: Pomodoros,
    prompts: Prompts,
) {
    let mut terminate = signal(SignalKind::terminate()).expect("failed to listen for SIGTERM");
    tokio::select! {
        _ = terminate.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }

    // Keep the lock until the shards stop so that no new session starts.
    let mut sessions = sessions.lock().await;
    let now = chrono::Utc::now().timestamp();
    for ((user, guild), session) in sessions.drain() {
        if let Err(e) = commit(&db, user as i64, guild as i64, &session, now).await {
            eprintln!(
                "Failed to commit session of user {} on shutdown: {}",
                user, e
            );
        }
    }

    let mut leftovers: Vec<_> = prompts
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain()
        .collect();
    leftovers.extend(crate::pomodoro::drain_status_messages(&pomodoros));
    let cleanup = async {
        for (channel, msg) in leftovers {
            let _ = channel.delete_message(&*http, msg).await;
        }
    };
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), cleanup).await;

    shards.shutdown_all().await;
}
