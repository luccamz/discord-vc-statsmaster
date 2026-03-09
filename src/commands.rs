use crate::state::{Context, Error};
use crate::tasks::WeekdayChoice;
use chrono::TimeZone;
use poise::serenity_prelude as serenity;
use sqlx::SqlitePool;

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
        .color(0xF1C40F);

    ctx.send(poise::CreateReply::default().embed(embed)).await?;

    Ok(())
}

/// Make current session not count
#[poise::command(slash_command, guild_only)]
pub async fn cancel_session(ctx: Context<'_>) -> Result<(), Error> {
    let user_id = ctx.author().id.get();
    let guild_id = ctx.guild_id().unwrap().get();

    let components = vec![serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new("confirm_cancel")
            .label("Confirm")
            .style(serenity::ButtonStyle::Danger),
        serenity::CreateButton::new("cancel_cancel") // hah !
            .label("Cancel")
            .style(serenity::ButtonStyle::Secondary),
    ])];

    let reply = poise::CreateReply::default()
        .content("Are you sure you want to cancel this sesh?")
        .ephemeral(true)
        .components(components);

    let handle = ctx.send(reply).await?;

    if let Some(mci) = serenity::ComponentInteractionCollector::new(ctx)
        .author_id(ctx.author().id)
        .channel_id(ctx.channel_id())
        .timeout(std::time::Duration::from_secs(30))
        .await
    {
        if mci.data.custom_id == "confirm_cancel" {
            let mut sessions = ctx.data().active_sessions.lock().await;

            sessions.remove(&(user_id, guild_id));

            mci.create_response(
                ctx,
                serenity::CreateInteractionResponse::UpdateMessage(
                    serenity::CreateInteractionResponseMessage::new()
                        .content("Session cancelled!")
                        .components(vec![]),
                ),
            )
            .await?;
        } else {
            mci.create_response(
                ctx,
                serenity::CreateInteractionResponse::UpdateMessage(
                    serenity::CreateInteractionResponseMessage::new()
                        .content("Didn't cancel.")
                        .components(vec![]),
                ),
            )
            .await?;
        }
    } else {
        let _ = handle
            .edit(
                ctx,
                poise::CreateReply::default()
                    .content("Didn't cancel (timeout).")
                    .components(vec![]),
            )
            .await;
    }

    Ok(())
}

/// Resets time of specified user or the whole server
#[poise::command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
pub async fn reset_stats(
    ctx: Context<'_>,
    #[description = "Specific user to reset"] user: Option<serenity::User>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap().get() as i64;

    let target_text = match &user {
        Some(u) => format!("<@{}>'s", u.id.get()),
        None => "the entire server's".to_string(),
    };

    let components = vec![serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new("confirm_reset")
            .label("Confirm")
            .style(serenity::ButtonStyle::Danger),
        serenity::CreateButton::new("cancel_reset")
            .label("Cancel")
            .style(serenity::ButtonStyle::Secondary),
    ])];

    let reply = poise::CreateReply::default()
        .content(format!(
            "Are you sure you want to reset {} voice statistics?",
            target_text
        ))
        .components(components);

    let handle = ctx.send(reply).await?;

    if let Some(mci) = serenity::ComponentInteractionCollector::new(ctx)
        .author_id(ctx.author().id)
        .channel_id(ctx.channel_id())
        .timeout(std::time::Duration::from_secs(30))
        .await
    {
        if mci.data.custom_id == "confirm_reset" {
            match user {
                Some(u) => {
                    let user_id = u.id.get() as i64;
                    sqlx::query!(
                        "UPDATE voice_stats 
                        SET total_seconds = 0
                        WHERE user_id = ? AND guild_id = ?",
                        user_id,
                        guild_id
                    )
                    .execute(&ctx.data().db)
                    .await?;

                    mci.create_response(
                        ctx,
                        serenity::CreateInteractionResponse::UpdateMessage(
                            serenity::CreateInteractionResponseMessage::new()
                                .content(format!("Reset voice statistics for <@{}>.", user_id))
                                .components(vec![]),
                        ),
                    )
                    .await?;
                }
                None => {
                    sqlx::query!(
                        "UPDATE voice_stats 
                        SET total_seconds = 0
                        WHERE guild_id = ?",
                        guild_id
                    )
                    .execute(&ctx.data().db)
                    .await?;

                    mci.create_response(
                        ctx,
                        serenity::CreateInteractionResponse::UpdateMessage(
                            serenity::CreateInteractionResponseMessage::new()
                                .content("Reset all voice statistics for this server.")
                                .components(vec![]),
                        ),
                    )
                    .await?;
                }
            }
        } else {
            mci.create_response(
                ctx,
                serenity::CreateInteractionResponse::UpdateMessage(
                    serenity::CreateInteractionResponseMessage::new()
                        .content("Reset cancelled.")
                        .components(vec![]),
                ),
            )
            .await?;
        }
    } else {
        let _ = handle
            .edit(
                ctx,
                poise::CreateReply::default()
                    .content("Reset cancelled (timeout).")
                    .components(vec![]),
            )
            .await;
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

/// Adds a new task to your personal todo list.
#[poise::command(slash_command, guild_only)]
pub async fn add_task(
    ctx: Context<'_>,
    #[description = "Task description"] description: String,
    #[description = "Optional deadline (DD/MM/YYYY HH:MM)"] deadline: Option<String>,
) -> Result<(), Error> {
    let user_id = ctx.author().id.get() as i64;

    let mut deadline_ts = None;
    if let Some(dl_str) = deadline {
        let setting = sqlx::query!(
            "SELECT timezone_offset FROM user_settings WHERE user_id = ?",
            user_id
        )
        .fetch_optional(&ctx.data().db)
        .await?;

        let offset = setting.map(|s| s.timezone_offset).unwrap_or(0);

        match chrono::NaiveDateTime::parse_from_str(&dl_str, "%d/%m/%Y %H:%M") {
            Ok(dt) => {
                let utc_dt = dt - chrono::Duration::hours(offset);
                deadline_ts = Some(utc_dt.and_utc().timestamp());
            }
            Err(_) => {
                ctx.say(
                    "Invalid deadline format. Use `DD/MM/YYYY HH:MM` (e.g., `31/12/2026 23:59`).",
                )
                .await?;
                return Ok(());
            }
        }
    }

    sqlx::query!(
        "INSERT INTO user_tasks (user_id, description, deadline) VALUES (?, ?, ?)",
        user_id,
        description,
        deadline_ts
    )
    .execute(&ctx.data().db)
    .await?;

    ctx.say(format!("Added task: **{}**", description)).await?;
    Ok(())
}

/// Views and manages your interactive todo list.
#[poise::command(slash_command, guild_only)]
pub async fn todo(ctx: Context<'_>) -> Result<(), Error> {
    let user_id = ctx.author().id.get() as i64;
    let components = build_todo_components(&ctx.data().db, user_id).await?;

    if components.is_empty() {
        ctx.say("Your todo list is empty. Use `/add_task` to create one.")
            .await?;
        return Ok(());
    }

    let reply = poise::CreateReply::default()
        .content(
            "Here is your interactive todo list. Click a task to toggle its completion status.",
        )
        .components(components);

    ctx.send(reply).await?;
    Ok(())
}

/// Deletes multiple tasks from your list.
#[poise::command(slash_command, guild_only)]
pub async fn delete_tasks(ctx: Context<'_>) -> Result<(), Error> {
    let user_id = ctx.author().id.get() as i64;

    let tasks = sqlx::query!(
        "SELECT task_id, description, completed_at, terminated_at FROM user_tasks WHERE user_id = ? ORDER BY completed_at ASC, terminated_at ASC, task_id DESC",
        user_id
    )
    .fetch_all(&ctx.data().db)
    .await?;

    if tasks.is_empty() {
        ctx.say("You have no tasks to delete.").await?;
        return Ok(());
    }

    let mut options = Vec::new();
    for task in tasks {
        let prefix = if task.terminated_at.is_some() {
            "[-] "
        } else if task.completed_at.is_some() {
            "[x] "
        } else {
            "[ ] "
        };

        let max_desc_len = 80 - prefix.len();
        let safe_desc = if task.description.len() > max_desc_len {
            format!("{}...", &task.description[..max_desc_len - 3])
        } else {
            task.description.clone()
        };

        let label = format!("{}{}", prefix, safe_desc);

        options.push(serenity::CreateSelectMenuOption::new(
            label,
            task.task_id.unwrap().to_string(),
        ));
    }

    options.truncate(25);
    let max_selectable = options.len() as u8;

    let menu = serenity::CreateSelectMenu::new(
        format!("task_delete_{}", user_id),
        serenity::CreateSelectMenuKind::String { options },
    )
    .min_values(1)
    .max_values(max_selectable); // Enables multiple selection

    let reply = poise::CreateReply::default()
        .content("Select the tasks you wish to permanently delete:")
        .components(vec![serenity::CreateActionRow::SelectMenu(menu)]);

    ctx.send(reply).await?;
    Ok(())
}

pub async fn build_todo_components(
    db: &SqlitePool,
    user_id: i64,
) -> Result<Vec<serenity::CreateActionRow>, Error> {
    let tasks = sqlx::query!(
        "SELECT task_id, description, completed_at, terminated_at, time_spent_seconds, deadline 
         FROM user_tasks 
         WHERE user_id = ? 
         ORDER BY 
            CASE 
                WHEN completed_at IS NULL AND terminated_at IS NULL THEN 0 
                WHEN completed_at IS NOT NULL THEN 1 
                ELSE 2 
            END ASC,
            deadline ASC, 
            task_id DESC 
         LIMIT 25",
        user_id
    )
    .fetch_all(db)
    .await?;

    let mut rows = Vec::new();
    let now = chrono::Utc::now();

    for chunk in tasks.chunks(5) {
        let mut buttons = Vec::new();
        for task in chunk {
            let hours = task.time_spent_seconds / 3600;
            let minutes = (task.time_spent_seconds % 3600) / 60;
            let time_str = format!("({}h {}m)", hours, minutes);

            let deadline_str = match task.deadline {
                Some(ts) => {
                    let dt = chrono::Utc.timestamp_opt(ts, 0).unwrap();
                    let diff = dt - now;

                    if diff.num_seconds() < 0 {
                        " [Overdue]".to_string()
                    } else if diff.num_days() > 0 {
                        format!(" [{}d left]", diff.num_days())
                    } else if diff.num_hours() > 0 {
                        format!(" [{}h left]", diff.num_hours())
                    } else if diff.num_minutes() > 0 {
                        format!(" [{}m left]", diff.num_minutes())
                    } else {
                        " [<1m left]".to_string()
                    }
                }
                None => String::new(),
            };

            let prefix = if task.terminated_at.is_some() {
                "[-]"
            } else if task.completed_at.is_some() {
                "[x]"
            } else {
                "[ ]"
            };

            let style = if task.terminated_at.is_some() {
                serenity::ButtonStyle::Danger
            } else if task.completed_at.is_some() {
                serenity::ButtonStyle::Success
            } else {
                serenity::ButtonStyle::Secondary
            };

            let max_desc_len = 80 - prefix.len() - time_str.len() - deadline_str.len() - 3;
            let safe_desc = if task.description.len() > max_desc_len {
                format!("{}...", &task.description[..max_desc_len - 3])
            } else {
                task.description.clone()
            };

            let label = format!("{} {}{}{}", prefix, safe_desc, deadline_str, time_str);

            let mut button = serenity::CreateButton::new(format!(
                "task_toggle_{}_{}",
                task.task_id.unwrap(),
                user_id
            ))
            .label(label)
            .style(style);

            if task.terminated_at.is_some() {
                button = button.disabled(true);
            }

            buttons.push(button);
        }
        rows.push(serenity::CreateActionRow::Buttons(buttons));
    }

    Ok(rows)
}
