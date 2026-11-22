use crate::state::{Error, SessionData};
use chrono::{DateTime, Datelike, NaiveTime, Utc};
use poise::serenity_prelude as serenity;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::Duration;

#[derive(Debug, poise::ChoiceParameter)]
pub enum WeekdayChoice {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl WeekdayChoice {
    pub fn to_chrono_num(&self) -> u32 {
        match self {
            WeekdayChoice::Monday => 0,
            WeekdayChoice::Tuesday => 1,
            WeekdayChoice::Wednesday => 2,
            WeekdayChoice::Thursday => 3,
            WeekdayChoice::Friday => 4,
            WeekdayChoice::Saturday => 5,
            WeekdayChoice::Sunday => 6,
        }
    }
}

async fn perform_guild_reset(
    http: &Arc<serenity::Http>,
    db: &SqlitePool,
    sessions: &Mutex<HashMap<(u64, u64), SessionData>>,
    guild_id: i64,
    channel_id: i64,
    current_weeks: i64,
    scheduled: i64,
) -> Result<(), Error> {
    let new_weeks = current_weeks + 1;

    // Hold the lock so open sessions count towards this week and no commit lands mid-reset.
    let mut sessions = sessions.lock().await;
    let now = chrono::Utc::now().timestamp();
    for (&(user, guild), session) in sessions.iter_mut() {
        if guild == guild_id as u64 {
            crate::sessions::commit(db, user as i64, guild_id, session, now).await?;
            session.start_time = now;
        }
    }

    let mut tx = db.begin().await?;
    sqlx::query!(
        "UPDATE guild_settings SET weeks_tracked = ?, last_reset_at = ? WHERE guild_id = ?",
        new_weeks,
        scheduled,
        guild_id
    )
    .execute(&mut *tx)
    .await?;

    let records = sqlx::query!(
        "SELECT user_id, total_seconds, personal_record FROM voice_stats WHERE guild_id = ? ORDER BY total_seconds DESC LIMIT 5",
        guild_id
    )
        .fetch_all(&mut *tx)
        .await?;

    sqlx::query!(
        "UPDATE voice_stats
        SET total_seconds = 0,
        personal_record = MAX(personal_record, total_seconds)
        WHERE guild_id = ?",
        guild_id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    drop(sessions);

    let mut message = format!("**Weekly Voice Leaderboard (Week {})**\n", new_weeks);

    if records.is_empty() {
        message.push_str("No voice activity recorded this week.");
    } else {
        for (index, record) in records.into_iter().enumerate() {
            let user_id: i64 = record.user_id;
            let total_seconds: i64 = record.total_seconds;
            let hours = total_seconds / 3600;
            let minutes = (total_seconds % 3600) / 60;
            message.push_str(&format!(
                "{}. <@{}>: {}h {}m",
                index + 1,
                user_id,
                hours,
                minutes
            ));
            if total_seconds > record.personal_record {
                message.push_str(" - New personal record!\n");
            } else {
                message.push('\n');
            }
        }
    }

    let channel = serenity::ChannelId::new(channel_id as u64);
    let builder = serenity::CreateMessage::new().content(message);
    let _ = channel.send_message(http, builder).await;

    Ok(())
}

/// The latest scheduled reset at or before `now`, with `day` counted from Monday = 0, in UTC.
fn last_scheduled_reset(now: DateTime<Utc>, day: i64, hour: i64, minute: i64) -> Option<i64> {
    let time = NaiveTime::from_hms_opt(hour.try_into().ok()?, minute.try_into().ok()?, 0)?;
    let days_back = (now.weekday().num_days_from_monday() as i64 - day).rem_euclid(7);
    let scheduled = (now.date_naive() - chrono::Duration::days(days_back))
        .and_time(time)
        .and_utc();
    let scheduled = if scheduled > now {
        scheduled - chrono::Duration::weeks(1)
    } else {
        scheduled
    };
    Some(scheduled.timestamp())
}

pub async fn weekly_reset_task(
    http: Arc<serenity::Http>,
    db: SqlitePool,
    sessions: Arc<Mutex<HashMap<(u64, u64), SessionData>>>,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));

    loop {
        interval.tick().await;

        let settings = match sqlx::query!(
            "SELECT guild_id, announcement_channel_id, weeks_tracked, 
                    reset_day, reset_hour, reset_minute, last_reset_at 
             FROM guild_settings"
        )
        .fetch_all(&db)
        .await
        {
            Ok(settings) => settings,
            Err(e) => {
                eprintln!("Failed to load reset schedules: {}", e);
                continue;
            }
        };

        // Catch up on any reset missed while the bot was down, not only the exact minute.
        let now = chrono::Utc::now();
        for row in settings {
            let guild_id = row.guild_id;
            let Some(scheduled) =
                last_scheduled_reset(now, row.reset_day, row.reset_hour, row.reset_minute)
            else {
                continue;
            };

            let result = match row.last_reset_at {
                // First run for this guild: take the last scheduled time as the baseline.
                None => sqlx::query!(
                    "UPDATE guild_settings SET last_reset_at = ? WHERE guild_id = ?",
                    scheduled,
                    guild_id
                )
                .execute(&db)
                .await
                .map(|_| ())
                .map_err(Into::into),
                Some(last) if last < scheduled => {
                    perform_guild_reset(
                        &http,
                        &db,
                        &sessions,
                        guild_id,
                        row.announcement_channel_id,
                        row.weeks_tracked,
                        scheduled,
                    )
                    .await
                }
                Some(_) => Ok(()),
            };
            if let Err(e) = result {
                eprintln!(
                    "Failed to execute scheduled reset for guild {}: {}",
                    guild_id, e
                );
            }
        }
    }
}

pub async fn deadline_check_task(db: SqlitePool) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));

    loop {
        interval.tick().await;
        let now = chrono::Utc::now().timestamp();

        if let Err(e) = sqlx::query!(
            "UPDATE user_tasks 
             SET terminated_at = ?, 
                 is_last_selected = 0 
             WHERE completed_at IS NULL 
               AND terminated_at IS NULL 
               AND deadline <= ?",
            now,
            now
        )
        .execute(&db)
        .await
        {
            eprintln!("Failed to terminate overdue tasks: {}", e);
        }

        // Keep the 20 latest terminated tasks per user, like completed tasks.
        if let Err(e) = sqlx::query!(
            "DELETE FROM user_tasks WHERE task_id IN (
                 SELECT task_id FROM (
                     SELECT task_id, ROW_NUMBER() OVER (
                         PARTITION BY user_id ORDER BY terminated_at DESC
                     ) AS rn
                     FROM user_tasks WHERE terminated_at IS NOT NULL
                 ) WHERE rn > 20
             )"
        )
        .execute(&db)
        .await
        {
            eprintln!("Failed to trim terminated tasks: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn last_scheduled_reset_picks_latest_past_slot() {
        // 2026-11-23 is a Monday.
        let monday_9 = ts(2026, 11, 23, 9, 0);
        assert_eq!(
            last_scheduled_reset(monday_9, 0, 8, 30),
            Some(ts(2026, 11, 23, 8, 30).timestamp())
        );
        assert_eq!(
            last_scheduled_reset(monday_9, 0, 9, 0),
            Some(monday_9.timestamp())
        );
        assert_eq!(
            last_scheduled_reset(monday_9, 0, 9, 1),
            Some(ts(2026, 11, 16, 9, 1).timestamp())
        );
        assert_eq!(
            last_scheduled_reset(monday_9, 6, 23, 0),
            Some(ts(2026, 11, 22, 23, 0).timestamp())
        );
        assert_eq!(
            last_scheduled_reset(monday_9, 2, 0, 0),
            Some(ts(2026, 11, 18, 0, 0).timestamp())
        );
        assert_eq!(last_scheduled_reset(monday_9, 0, 24, 0), None);
    }
}
