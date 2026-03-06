use crate::state::{Context, Error};
use crate::tasks::WeekdayChoice;
use poise::serenity_prelude as serenity;

/// Displays your total accumulated voice time in this server
#[poise::command(slash_command, guild_only)]
pub async fn stats(ctx: Context<'_>) -> Result<(), Error> {
    let user_id = ctx.author().id.get() as i64;
    let guild_id = ctx.guild_id().unwrap().get() as i64;

    let record = sqlx::query!(
        "SELECT total_seconds, personal_record FROM voice_stats WHERE user_id = ? AND guild_id = ?",
        user_id,
        guild_id
    )
    .fetch_optional(&ctx.data().db)
    .await?;

    let mut embed = serenity::CreateEmbed::new()
        .title("Voice Statistics")
        .color(0x3498DB)
        .thumbnail(ctx.author().face());

    match record {
        Some(row) => {
            let total_seconds: i64 = row.total_seconds;
            let hours = total_seconds / 3600;
            let minutes = (total_seconds % 3600) / 60;
            let percent = if row.personal_record > 0 {
                Some((total_seconds as f64 / row.personal_record as f64) * 100.0)
            } else {
                None
            };

            embed = embed.field(
                "Total Time",
                format!(
                    "**{}** hours and **{}** minutes.. **{:.0}%** of your PR!",
                    hours,
                    minutes,
                    percent.unwrap_or(9999999.0)
                ),
                false,
            );
        }
        None => {
            embed = embed.description("You have no recorded voice time in this server.");
        }
    }

    let reply = poise::CreateReply::default().embed(embed);
    ctx.send(reply).await?;

    Ok(())
}

/// Displays the top users by voice time. Defaults to top 5 if no limit is provided.
#[poise::command(slash_command, guild_only)]
pub async fn leaderboard(
    ctx: Context<'_>,
    #[description = "Number of users to display (default 5)"] limit: Option<i64>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap().get() as i64;
    let fetch_limit = limit.unwrap_or(5).clamp(1, 50);

    let records = sqlx::query!(
        "SELECT user_id, total_seconds FROM voice_stats WHERE guild_id = ? ORDER BY total_seconds DESC LIMIT ?",
        guild_id,
        fetch_limit
    )
        .fetch_all(&ctx.data().db)
        .await?;

    if records.is_empty() {
        ctx.say("The leaderboard is currently empty.").await?;
        return Ok(());
    }

    let mut description = String::new();

    for (index, row) in records.into_iter().enumerate() {
        let user_id: i64 = row.user_id;
        let total_seconds: i64 = row.total_seconds;

        let hours = total_seconds / 3600;
        let minutes = (total_seconds % 3600) / 60;

        description.push_str(&format!(
            "**{}.** <@{}> • {}h {}m\n",
            index + 1,
            user_id,
            hours,
            minutes
        ));
    }

    let embed = serenity::CreateEmbed::new()
        .title(format!("Top {} Users in Voice Activity", fetch_limit))
        .description(description)
        .color(0xF1C40F); // Gold color

    ctx.send(poise::CreateReply::default().embed(embed)).await?;

    Ok(())
}

/// Resets voice time. Provide a user to reset only their stats, or omit to reset the entire server.
#[poise::command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
pub async fn reset_stats(
    ctx: Context<'_>,
    #[description = "Specific user to reset"] user: Option<serenity::User>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap().get() as i64;

    match user {
        Some(u) => {
            let user_id = u.id.get() as i64;
            sqlx::query!(
                "UPDATE voice_stats 
                SET total_seconds = 0,
                    personal_record = MAX(personal_record, total_seconds)
                WHERE user_id = ? AND guild_id = ?",
                user_id,
                guild_id
            )
            .execute(&ctx.data().db)
            .await?;
            ctx.say(format!("Reset voice statistics for <@{}>.", user_id))
                .await?;
        }
        None => {
            sqlx::query!(
                "UPDATE voice_stats 
                SET total_seconds = 0,
                    personal_record = MAX(personal_record, total_seconds)
                WHERE guild_id = ?",
                guild_id
            )
            .execute(&ctx.data().db)
            .await?;
            ctx.say("Reset all voice statistics for this server.")
                .await?;
        }
    }
    Ok(())
}

/// Toggles tracking of voice activity for a specific channel.
#[poise::command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
pub async fn toggle_tracking(
    ctx: Context<'_>,
    #[description = "Voice channel to toggle tracking for"] channel: serenity::Channel,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap().get() as i64;
    let channel_id = channel.id().get() as i64;

    let existing = sqlx::query!(
        "SELECT channel_id FROM tracked_channels WHERE channel_id = ?",
        channel_id
    )
    .fetch_optional(&ctx.data().db)
    .await?;

    if existing.is_some() {
        sqlx::query!(
            "DELETE FROM tracked_channels WHERE channel_id = ?",
            channel_id
        )
        .execute(&ctx.data().db)
        .await?;
        ctx.say(format!(
            "Stopped tracking voice activity in <#{}>.",
            channel_id
        ))
        .await?;
    } else {
        sqlx::query!(
            "INSERT INTO tracked_channels (channel_id, guild_id) VALUES (?, ?)",
            channel_id,
            guild_id
        )
        .execute(&ctx.data().db)
        .await?;
        ctx.say(format!(
            "Started tracking voice activity in <#{}>.",
            channel_id
        ))
        .await?;
    }
    Ok(())
}

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
    // Validate inputs
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

    // Calculate UTC time
    let mut utc_hour = hour - utc_offset;
    let mut utc_day = day.to_chrono_num() as i64;

    // Handle day wrapping if the timezone offset pushes the hour past midnight
    if utc_hour < 0 {
        utc_hour += 24;
        // rem_euclid handles negative number wrapping correctly in Rust
        utc_day = (utc_day - 1).rem_euclid(7);
    } else if utc_hour >= 24 {
        utc_hour -= 24;
        utc_day = (utc_day + 1).rem_euclid(7);
    }

    // Insert the calculated UTC schedule into the database
    sqlx::query!(
        "INSERT INTO guild_settings (guild_id, announcement_channel_id, reset_day, reset_hour, reset_minute) 
         VALUES (?, ?, ?, ?, ?) 
         ON CONFLICT(guild_id) DO UPDATE SET 
         announcement_channel_id = excluded.announcement_channel_id,
         reset_day = excluded.reset_day,
         reset_hour = excluded.reset_hour,
         reset_minute = excluded.reset_minute",
        guild_id,
        channel_id,
        utc_day,
        utc_hour,
        minute
    )
    .execute(&ctx.data().db)
    .await?;

    ctx.say(format!(
        "Schedule configured. Leaderboard will reset locally every {:?} at {:02}:{:02} (UTC Offset: {}).", 
        day, hour, minute, utc_offset
    )).await?;

    Ok(())
}
