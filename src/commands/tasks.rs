use crate::state::{Context, Error};
use chrono::TimeZone;
use poise::serenity_prelude as serenity;
use sqlx::SqlitePool;

/// Adds a new task to your personal todo list.
#[poise::command(slash_command, guild_only)]
pub async fn add_task(
    ctx: Context<'_>,
    #[description = "Task description"]
    #[max_length = 200]
    description: String,
    #[description = "Format: DD/MM/YYYY HH:MM, 'today HH:MM', 'in 24 hours', 'in 7 days'"]
    deadline: Option<String>,
) -> Result<(), Error> {
    let user_id = ctx.author().id.get() as i64;
    let task_per_user_limit = 100;

    let mut deadline_ts = None;
    if let Some(dl_str) = deadline {
        let setting = sqlx::query!(
            "SELECT timezone_offset FROM user_settings WHERE user_id = ?",
            user_id
        )
        .fetch_optional(&ctx.data().db)
        .await?;

        let offset = setting.map(|s| s.timezone_offset).unwrap_or(0);
        let now_utc = chrono::Utc::now().naive_utc();
        let user_now = now_utc + chrono::Duration::hours(offset);
        let dl_lower = dl_str.to_lowercase();

        if dl_lower == "in 24 hours" {
            deadline_ts = Some(
                (now_utc + chrono::Duration::hours(24))
                    .and_utc()
                    .timestamp(),
            );
        } else if dl_lower == "in 7 days" {
            deadline_ts = Some((now_utc + chrono::Duration::days(7)).and_utc().timestamp());
        } else if let Some(time_str) = dl_lower.strip_prefix("today ") {
            match chrono::NaiveTime::parse_from_str(time_str.trim(), "%H:%M") {
                Ok(time) => {
                    // Extract the user's current local date, apply the requested time, and convert back to UTC
                    let local_dt = user_now.date().and_time(time);
                    let utc_dt = local_dt - chrono::Duration::hours(offset);
                    deadline_ts = Some(utc_dt.and_utc().timestamp());
                }
                Err(_) => {
                    ctx.send(
                        poise::CreateReply::default()
                            .content(
                                "Invalid time format. Use `today HH:MM` (e.g., `today 15:30`).",
                            )
                            .ephemeral(true),
                    )
                    .await?;
                    return Ok(());
                }
            }
        } else {
            match chrono::NaiveDateTime::parse_from_str(&dl_str, "%d/%m/%Y %H:%M") {
                Ok(dt) => {
                    let utc_dt = dt - chrono::Duration::hours(offset);
                    deadline_ts = Some(utc_dt.and_utc().timestamp());
                }
                Err(_) => {
                    ctx.send(
                        poise::CreateReply::default()
                            .content("Invalid deadline format. Use `DD/MM/YYYY HH:MM`, `today HH:MM`, `in 24 hours`, or `in 7 days`.")
                            .ephemeral(true)
                    ).await?;
                    return Ok(());
                }
            }
        }
    }

    let task_count =
        sqlx::query_scalar!("SELECT COUNT(*) FROM user_tasks WHERE user_id = ?", user_id)
            .fetch_one(&ctx.data().db)
            .await?;

    if task_count >= task_per_user_limit {
        let components = vec![serenity::CreateActionRow::Buttons(vec![
            serenity::CreateButton::new("confirm_trim")
                .label("Proceed & Trim")
                .style(serenity::ButtonStyle::Danger),
            serenity::CreateButton::new("cancel_trim")
                .label("Cancel")
                .style(serenity::ButtonStyle::Secondary),
        ])];

        let reply = ctx.send(
            poise::CreateReply::default()
                .content(format!(
                    "You have reached the maximum of **{}** tasks. Adding this task will automatically delete your oldest task (prioritizing terminated, then completed, then pending). Do you want to proceed?",
                    task_per_user_limit
                ))
                .components(components)
                .ephemeral(true)
        ).await?;

        let mut message = reply.into_message().await?;

        if let Some(interaction) = message
            .await_component_interaction(ctx.serenity_context())
            .timeout(std::time::Duration::from_secs(60))
            .await
        {
            if interaction.data.custom_id == "cancel_trim" {
                interaction
                    .create_response(
                        ctx.serenity_context(),
                        serenity::CreateInteractionResponse::UpdateMessage(
                            serenity::CreateInteractionResponseMessage::new()
                                .content("Task creation cancelled.")
                                .components(vec![]),
                        ),
                    )
                    .await?;
                return Ok(());
            } else {
                sqlx::query!(
                    "DELETE FROM user_tasks WHERE task_id = (
                        SELECT task_id FROM user_tasks 
                        WHERE user_id = ? 
                        ORDER BY 
                            CASE 
                                WHEN terminated_at IS NOT NULL THEN 1 
                                WHEN completed_at IS NOT NULL THEN 2 
                                ELSE 3 
                            END ASC, 
                            task_id ASC 
                        LIMIT 1
                    )",
                    user_id
                )
                .execute(&ctx.data().db)
                .await?;

                interaction
                    .create_response(
                        ctx.serenity_context(),
                        serenity::CreateInteractionResponse::UpdateMessage(
                            serenity::CreateInteractionResponseMessage::new()
                                .content(format!(
                                    "Task added: **{}** (Oldest task was trimmed)",
                                    description
                                ))
                                .components(vec![]),
                        ),
                    )
                    .await?;
            }
        } else {
            // Timeout cleanup
            message
                .edit(
                    ctx.serenity_context(),
                    serenity::EditMessage::new()
                        .content("Task creation timed out.")
                        .components(vec![]),
                )
                .await?;
            return Ok(());
        }
    } else {
        ctx.send(
            poise::CreateReply::default()
                .content(format!("Task added: **{}**", description))
                .ephemeral(true),
        )
        .await?;
    }

    sqlx::query!(
        "INSERT INTO user_tasks (user_id, description, deadline) VALUES (?, ?, ?)",
        user_id,
        description,
        deadline_ts
    )
    .execute(&ctx.data().db)
    .await?;

    let reply = poise::CreateReply::default()
        .content(format!("Added task: **{}**", description))
        .ephemeral(true);

    ctx.send(reply).await?;

    Ok(())
}

#[allow(dead_code)]
pub async fn pending_tasks_autocomplete(
    ctx: crate::state::Context<'_>,
    partial: &str,
) -> Vec<String> {
    let user_id = ctx.author().id.get() as i64;
    let partial_match = format!("%{}%", partial);

    let tasks = sqlx::query!(
        "SELECT task_id, description FROM user_tasks 
         WHERE user_id = ? AND completed_at IS NULL AND terminated_at IS NULL 
         AND description LIKE ? 
         ORDER BY task_id DESC 
         LIMIT 25",
        user_id,
        partial_match
    )
    .fetch_all(&ctx.data().db)
    .await
    .unwrap_or_default();

    tasks
        .into_iter()
        .map(|task| {
            let task_id = task.task_id.unwrap_or(0);

            let safe_desc = if task.description.len() > 80 {
                format!("{}...", &task.description[..80])
            } else {
                task.description
            };

            format!("{} - {}", task_id, safe_desc)
        })
        .collect()
}

/// Modifies or removes the deadline of an existing task.
#[poise::command(slash_command, guild_only)]
pub async fn edit_deadline(
    ctx: crate::state::Context<'_>,
    #[description = "Search and select the task to update"]
    #[autocomplete = "crate::pending_tasks_autocomplete"]
    // Ensure path matches your structure
    task_selection: String,
    #[description = "New deadline (DD/MM/YYYY HH:MM) or type 'clear' to remove"]
    new_deadline: String,
) -> Result<(), crate::state::Error> {
    let user_id = ctx.author().id.get() as i64;

    let task_id_str = task_selection.split(" - ").next().unwrap_or("");
    let task_id: i64 = match task_id_str.parse() {
        Ok(id) => id,
        Err(_) => {
            ctx.send(
                poise::CreateReply::default()
                    .content("Invalid task selection. Please select an option from the autocomplete menu.")
                    .ephemeral(true)
            ).await?;
            return Ok(());
        }
    };

    let mut deadline_ts = None;

    if new_deadline.to_lowercase() != "clear" {
        let setting = sqlx::query!(
            "SELECT timezone_offset FROM user_settings WHERE user_id = ?",
            user_id
        )
        .fetch_optional(&ctx.data().db)
        .await?;

        let offset = setting.map(|s| s.timezone_offset).unwrap_or(0);

        match chrono::NaiveDateTime::parse_from_str(&new_deadline, "%d/%m/%Y %H:%M") {
            Ok(dt) => {
                let utc_dt = dt - chrono::Duration::hours(offset);
                deadline_ts = Some(utc_dt.and_utc().timestamp());
            }
            Err(_) => {
                ctx.send(
                    poise::CreateReply::default()
                        .content("Invalid deadline format. Use `DD/MM/YYYY HH:MM` (e.g., `31/12/2026 23:59`) or type `clear`.")
                        .ephemeral(true)
                ).await?;
                return Ok(());
            }
        }
    }

    let result = sqlx::query!(
        "UPDATE user_tasks SET deadline = ? WHERE task_id = ? AND user_id = ?",
        deadline_ts,
        task_id,
        user_id
    )
    .execute(&ctx.data().db)
    .await?;

    if result.rows_affected() == 0 {
        ctx.send(
            poise::CreateReply::default()
                .content("Task not found or you do not have permission to edit it.")
                .ephemeral(true),
        )
        .await?;
    } else {
        let response_msg = match deadline_ts {
            Some(_) => format!("Task deadline updated to **{}**.", new_deadline),
            None => "Task deadline has been cleared.".to_string(),
        };

        ctx.send(
            poise::CreateReply::default()
                .content(response_msg)
                .ephemeral(true),
        )
        .await?;
    }

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
        let reply = poise::CreateReply::default()
            .content("You have no tasks to delete.")
            .ephemeral(true);

        ctx.send(reply).await?;

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
        .ephemeral(true)
        .components(vec![serenity::CreateActionRow::SelectMenu(menu)]);

    ctx.send(reply).await?;
    Ok(())
}
