use crate::state::{Data, Error};
use poise::serenity_prelude as serenity;

pub async fn handle_event(event: &serenity::FullEvent, data: &Data) -> Result<(), Error> {
    if let serenity::FullEvent::VoiceStateUpdate { old: _, new } = event {
        let user_id = new.user_id.get() as i64;
        let guild_id = match new.guild_id {
            Some(id) => id.get(),
            None => return Ok(()),
        } as i64;

        let mut sessions = data.active_sessions.lock().await;
        let now = chrono::Utc::now().timestamp();

        let mut is_in_tracked_channel = false;
        if let Some(channel_id_non_zero) = new.channel_id {
            let channel_id = channel_id_non_zero.get() as i64;
            let existing = sqlx::query!(
                "SELECT channel_id FROM tracked_channels WHERE channel_id = ?",
                channel_id
            )
            .fetch_optional(&data.db)
            .await?;
            is_in_tracked_channel = existing.is_some();
        }

        if is_in_tracked_channel {
            sessions
                .entry((user_id as u64, guild_id as u64))
                .or_insert(now);
        } else if let Some(start_time) = sessions.remove(&(user_id as u64, guild_id as u64)) {
            let duration = now - start_time;
            if duration > 0 {
                sqlx::query!(
                    "INSERT INTO voice_stats (user_id, guild_id, total_seconds) 
                            VALUES (?, ?, ?) 
                            ON CONFLICT(user_id, guild_id) 
                            DO UPDATE SET total_seconds = total_seconds + ?",
                    user_id,
                    guild_id,
                    duration,
                    duration
                )
                .execute(&data.db)
                .await?;
            }
        }
    }
    Ok(())
}
