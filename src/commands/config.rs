use crate::state::{Context, Error};
use crate::tasks::WeekdayChoice;
use poise::serenity_prelude as serenity;

/// Configures the weekly leaderboard reset schedule and announcement channel
#[poise::command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
pub async fn config_schedule(
    ctx: Context<'_>,
    #[description = "Target channel"] channel: serenity::Channel,
    #[description = "Day of the week"] day: WeekdayChoice,
    #[description = "Local Hour (0-23)"] hour: i64,
    #[description = "Minute (0-59)"] minute: i64,
    #[description = "UTC Offset (e.g., -5 for EST, 2 for CEST)"] utc_offset: i64,
) -> Result<(), Error> {
    if !(0..24).contains(&hour) || !(0..60).contains(&minute) {
        ctx.say("Invalid time constraint. Hour must be 0-23 and minute 0-59.")
            .await?;
        return Ok(());
    }
    if !(-12..=14).contains(&utc_offset) {
        ctx.say("Invalid UTC offset. Must be between -12 and 14.")
            .await?;
        return Ok(());
    }

    let guild_id = ctx.guild_id().unwrap().get() as i64;
    let channel_id = channel.id().get() as i64;

    let mut utc_hour = hour - utc_offset;
    let mut utc_day = day.to_chrono_num() as i64;

    if utc_hour < 0 {
        utc_hour += 24;
        utc_day = (utc_day - 1).rem_euclid(7);
    } else if utc_hour >= 24 {
        utc_hour -= 24;
        utc_day = (utc_day + 1).rem_euclid(7);
    }

    // A new schedule starts from now, so a slot that just passed does not reset at once.
    let now = chrono::Utc::now().timestamp();
    sqlx::query!(
        "INSERT INTO guild_settings (guild_id, announcement_channel_id, reset_day, reset_hour, reset_minute, last_reset_at) 
         VALUES (?, ?, ?, ?, ?, ?) 
         ON CONFLICT(guild_id) DO UPDATE SET 
         announcement_channel_id = excluded.announcement_channel_id,
         reset_day = excluded.reset_day,
         reset_hour = excluded.reset_hour,
         reset_minute = excluded.reset_minute,
         last_reset_at = excluded.last_reset_at",
        guild_id,
        channel_id,
        utc_day,
        utc_hour,
        minute,
        now
    )
    .execute(&ctx.data().db)
    .await?;

    ctx.say(format!(
        "Schedule configured. Leaderboard will reset locally every {:?} at {:02}:{:02} (UTC Offset: {}).", 
        day, hour, minute, utc_offset
    )).await?;

    Ok(())
}

/// Configures your personal timezone offset for deadlines.
#[poise::command(slash_command, guild_only)]
pub async fn config_timezone(
    ctx: Context<'_>,
    #[description = "UTC Offset (e.g., -5 for EST, 2 for CEST)"] offset: i64,
) -> Result<(), Error> {
    if !(-12..=14).contains(&offset) {
        ctx.say("Invalid UTC offset. Must be between -12 and 14.")
            .await?;
        return Ok(());
    }

    let user_id = ctx.author().id.get() as i64;

    sqlx::query!(
        "INSERT INTO user_settings (user_id, timezone_offset) VALUES (?, ?) 
         ON CONFLICT(user_id) DO UPDATE SET timezone_offset = excluded.timezone_offset",
        user_id,
        offset
    )
    .execute(&ctx.data().db)
    .await?;

    let sign = if offset >= 0 { "+" } else { "" };
    ctx.say(format!(
        "Your personal timezone offset has been set to UTC{}{}.",
        sign, offset
    ))
    .await?;

    Ok(())
}

/// Configures the channel for automated changelog broadcasts.
#[poise::command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
pub async fn config_changelogs(
    ctx: Context<'_>,
    #[description = "Target channel for update announcements"] channel: serenity::Channel,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap().get() as i64;
    let channel_id = channel.id().get() as i64;

    sqlx::query!(
        "INSERT INTO guild_settings (guild_id, announcement_channel_id, changelog_channel_id) 
         VALUES (?, 0, ?) 
         ON CONFLICT(guild_id) DO UPDATE SET 
         changelog_channel_id = excluded.changelog_channel_id",
        guild_id,
        channel_id
    )
    .execute(&ctx.data().db)
    .await?;

    let reply = poise::CreateReply::default()
        .content(format!(
            "Update changelogs will now be broadcast to <#{}>.",
            channel_id
        ))
        .ephemeral(true);

    ctx.send(reply).await?;

    Ok(())
}

/// Configures the voice channel used as the pomodoro pause lobby.
#[poise::command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
pub async fn config_pomodoro_lobby(
    ctx: Context<'_>,
    #[description = "Voice channel for pomodoro pauses"]
    #[channel_types("Voice")]
    channel: serenity::GuildChannel,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap().get() as i64;
    let channel_id = channel.id.get() as i64;

    let tracked = sqlx::query!(
        "SELECT channel_id FROM tracked_channels WHERE channel_id = ?",
        channel_id
    )
    .fetch_optional(&ctx.data().db)
    .await?;

    let content = if tracked.is_some() {
        "A tracked channel cannot be the pomodoro lobby.".to_string()
    } else {
        sqlx::query!(
            "INSERT INTO guild_settings (guild_id, announcement_channel_id, pomodoro_lobby_channel_id) 
             VALUES (?, 0, ?) 
             ON CONFLICT(guild_id) DO UPDATE SET 
             pomodoro_lobby_channel_id = excluded.pomodoro_lobby_channel_id",
            guild_id,
            channel_id
        )
        .execute(&ctx.data().db)
        .await?;
        format!("Pomodoro pauses will now happen in <#{}>.", channel_id)
    };

    ctx.send(
        poise::CreateReply::default()
            .content(content)
            .ephemeral(true),
    )
    .await?;

    Ok(())
}

pub async fn pomodoro_lobby(db: &sqlx::SqlitePool, guild_id: i64) -> Result<Option<i64>, Error> {
    Ok(sqlx::query!(
        "SELECT pomodoro_lobby_channel_id FROM guild_settings WHERE guild_id = ?",
        guild_id
    )
    .fetch_optional(db)
    .await?
    .and_then(|row| row.pomodoro_lobby_channel_id))
}
