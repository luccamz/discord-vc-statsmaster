use crate::state::Error;
use chrono::{Datelike, Timelike};
use poise::serenity_prelude as serenity;
use sqlx::{Row, SqlitePool};
use std::sync::Arc;
use tokio::time::Duration;

#[derive(Debug, poise::ChoiceParameter)]
pub enum WeekdayChoice {
    Monday, Tuesday, Wednesday, Thursday, Friday, Saturday, Sunday
}

impl WeekdayChoice {
    pub fn to_chrono_num(&self) -> u32 {
        match self {
            WeekdayChoice::Monday => 0, WeekdayChoice::Tuesday => 1,
            WeekdayChoice::Wednesday => 2, WeekdayChoice::Thursday => 3,
            WeekdayChoice::Friday => 4, WeekdayChoice::Saturday => 5,
            WeekdayChoice::Sunday => 6,
        }
    }
}

async fn perform_guild_reset(
    http: &Arc<serenity::Http>, 
    db: &SqlitePool, 
    guild_id: i64, 
    channel_id: i64, 
    current_weeks: i64
) -> Result<(), Error> {
    let new_weeks = current_weeks + 1;
    
    sqlx::query("UPDATE guild_settings SET weeks_tracked = ? WHERE guild_id = ?")
        .bind(new_weeks)
        .bind(guild_id)
        .execute(db)
        .await?;

    let records = sqlx::query("SELECT user_id, total_seconds FROM voice_stats WHERE guild_id = ? ORDER BY total_seconds DESC LIMIT 5")
        .bind(guild_id)
        .fetch_all(db)
        .await?;

    let mut message = format!("**Weekly Voice Leaderboard (Week {})**\n", new_weeks);
    
    if records.is_empty() {
        message.push_str("No voice activity recorded this week.");
    } else {
        for (index, record) in records.into_iter().enumerate() {
            let user_id: i64 = record.get("user_id");
            let total_seconds: i64 = record.get("total_seconds");
            let hours = total_seconds / 3600;
            let minutes = (total_seconds % 3600) / 60;
            message.push_str(&format!("{}. <@{}>: {}h {}m", index + 1, user_id, hours, minutes));
            if total_seconds > record.get("personal_record") {
                message.push_str("- New personal record!\n");
            } else {
                message.push_str("\n");
            }
        }
    }

    let channel = serenity::ChannelId::new(channel_id as u64);
    let builder = serenity::CreateMessage::new().content(message);
    let _ = channel.send_message(http, builder).await;

    sqlx::query(
        "UPDATE voice_stats
        SET total_seconds = 0,
        personal_record = MAX(personal_record, total_seconds)
        WHERE guild_id = ?"
    )
    .bind(guild_id)
    .execute(db)
    .await?;

    Ok(())
}

pub async fn weekly_reset_task(http: Arc<serenity::Http>, db: SqlitePool) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    
    loop {
        interval.tick().await; // Wait for the next 60-second boundary
        
        let now = chrono::Utc::now();
        let current_day = now.weekday().num_days_from_monday() as i64;
        let current_hour = now.hour() as i64;
        let current_minute = now.minute() as i64;

        // Query only the guilds scheduled for this exact minute
        let pending_resets = sqlx::query(
            "SELECT guild_id, announcement_channel_id, weeks_tracked FROM guild_settings 
             WHERE reset_day = ? AND reset_hour = ? AND reset_minute = ?"
        )
        .bind(current_day)
        .bind(current_hour)
        .bind(current_minute)
        .fetch_all(&db)
        .await;

        if let Ok(settings) = pending_resets {
            for row in settings {
                let guild_id: i64 = row.get("guild_id");
                let channel_id: i64 = row.get("announcement_channel_id");
                let weeks_tracked: i64 = row.get("weeks_tracked");

                if let Err(e) = perform_guild_reset(&http, &db, guild_id, channel_id, weeks_tracked).await {
                    eprintln!("Failed to execute scheduled reset for guild {}: {}", guild_id, e);
                }
            }
        }
    }
}
