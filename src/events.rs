use crate::commands::build_todo_components;
use crate::state::{Data, Error, SessionData};
use poise::serenity_prelude as serenity;
use std::collections::hash_map::Entry;

pub async fn handle_event(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    data: &Data,
) -> Result<(), Error> {
    match event {
        serenity::FullEvent::VoiceStateUpdate { old: _, new } => {
            let user_id = new.user_id.get() as i64;
            let Some(guild_id_nonzero) = new.guild_id else {
                return Ok(());
            };
            let guild_id = guild_id_nonzero.get() as i64;

            let mut sessions = data.active_sessions.lock().await;
            let now = chrono::Utc::now().timestamp();

            let mut is_in_tracked_channel = false;
            let mut current_channel_id = 0;
            if let Some(channel_id_non_zero) = new.channel_id {
                let channel_id = channel_id_non_zero.get() as i64;
                current_channel_id = channel_id;
                let existing = sqlx::query!(
                    "SELECT channel_id FROM tracked_channels WHERE channel_id = ?",
                    channel_id
                )
                .fetch_optional(&data.db)
                .await?;
                is_in_tracked_channel = existing.is_some();
            }

            if is_in_tracked_channel {
                if let Entry::Vacant(e) = sessions.entry((user_id as u64, guild_id as u64)) {
                    // Fetch pending tasks and find the last selected task
                    let pending_tasks = sqlx::query!(
                        "SELECT task_id, description, is_last_selected 
                         FROM user_tasks 
                         WHERE user_id = ? 
                           AND completed_at IS NULL 
                           AND terminated_at IS NULL",
                        user_id
                    )
                    .fetch_all(&data.db)
                    .await?;

                    let mut initial_task_id = None;
                    let mut active_task_name = "Nothing in particular".to_string();

                    for task in &pending_tasks {
                        if task.is_last_selected {
                            initial_task_id = Some(task.task_id.unwrap());
                            active_task_name = task.description.clone();
                        }
                    }

                    e.insert(SessionData {
                        start_time: now,
                        active_task_id: initial_task_id,
                    });

                    drop(sessions);

                    let prompt_content = format!(
                        "<@{}> Currently working on.. {}. Change status?",
                        user_id, active_task_name
                    );

                    let mut options = Vec::new();
                    let mut no_task_opt =
                        serenity::CreateSelectMenuOption::new("Nothing in particular", "none");

                    if initial_task_id.is_none() {
                        no_task_opt = no_task_opt.default_selection(true);
                    }
                    options.push(no_task_opt);

                    for task in pending_tasks {
                        let mut opt = serenity::CreateSelectMenuOption::new(
                            &task.description,
                            task.task_id.unwrap().to_string(),
                        );
                        if task.is_last_selected {
                            opt = opt.default_selection(true);
                        }
                        options.push(opt);
                    }

                    options.truncate(25);

                    let menu = serenity::CreateSelectMenu::new(
                        format!("task_select_{}", user_id),
                        serenity::CreateSelectMenuKind::String { options },
                    );

                    let channel = serenity::ChannelId::new(current_channel_id as u64);
                    if let Ok(message) = channel
                        .send_message(
                            ctx,
                            serenity::CreateMessage::new()
                                .content(prompt_content)
                                .select_menu(menu),
                        )
                        .await
                    {
                        let http = ctx.http.clone();

                        // Spawns a background task to delete the prompt after 2 minutes
                        tokio::spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_secs(120)).await;
                            let _ = message.delete(&http).await;
                        });
                    }
                }
            } else if let Some(session) = sessions.remove(&(user_id as u64, guild_id as u64)) {
                let duration = now - session.start_time;
                if duration > 0 {
                    sqlx::query!(
                        "INSERT INTO voice_stats (user_id, guild_id, total_seconds) 
                         VALUES (?, ?, ?) ON CONFLICT(user_id, guild_id) 
                         DO UPDATE SET total_seconds = total_seconds + ?",
                        user_id,
                        guild_id,
                        duration,
                        duration
                    )
                    .execute(&data.db)
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
                        .execute(&data.db)
                        .await?;
                    }
                }
            }
        }
        serenity::FullEvent::InteractionCreate {
            interaction: serenity::Interaction::Component(component),
        } => {
            let user_id = component.user.id.get() as i64;
            let Some(guild_id_nonzero) = component.guild_id else {
                return Ok(());
            };
            let guild_id = guild_id_nonzero.get() as i64;

            if component.data.custom_id.starts_with("task_select_") {
                let extracted_id = component.data.custom_id.replace("task_select_", "");
                if extracted_id != user_id.to_string() {
                    component
                        .create_response(
                            ctx,
                            serenity::CreateInteractionResponse::Message(
                                serenity::CreateInteractionResponseMessage::new()
                                    .content("Nuh huh, can't touch it!")
                                    .ephemeral(true), // Ensures only the clicking user sees this warning
                            ),
                        )
                        .await?;
                    return Ok(());
                }

                let serenity::ComponentInteractionDataKind::StringSelect { ref values } =
                    component.data.kind
                else {
                    return Ok(());
                };
                let selected_value = &values[0];

                let now = chrono::Utc::now().timestamp();
                let mut sessions = data.active_sessions.lock().await;

                if let Some(session) = sessions.get_mut(&(user_id as u64, guild_id as u64)) {
                    let duration = now - session.start_time;

                    if duration > 0 {
                        sqlx::query!(
                            "INSERT INTO voice_stats (user_id, guild_id, total_seconds) 
                             VALUES (?, ?, ?) ON CONFLICT(user_id, guild_id) 
                             DO UPDATE SET total_seconds = total_seconds + ?",
                            user_id,
                            guild_id,
                            duration,
                            duration
                        )
                        .execute(&data.db)
                        .await?;

                        if let Some(old_task_id) = session.active_task_id {
                            sqlx::query!(
                                "UPDATE user_tasks 
                                 SET time_spent_seconds = time_spent_seconds + ?,
                                     record_session_seconds = MAX(record_session_seconds, ?)
                                 WHERE task_id = ?",
                                duration,
                                duration,
                                old_task_id
                            )
                            .execute(&data.db)
                            .await?;
                        }
                    }

                    session.start_time = now;
                    if selected_value == "none" {
                        session.active_task_id = None;
                    } else {
                        session.active_task_id = Some(selected_value.parse().unwrap());
                    }
                }
                drop(sessions);

                sqlx::query!(
                    "UPDATE user_tasks SET is_last_selected = 0 WHERE user_id = ?",
                    user_id
                )
                .execute(&data.db)
                .await?;

                let selected_desc = if selected_value != "none" {
                    let task_id: i64 = selected_value.parse().unwrap();
                    sqlx::query!(
                        "UPDATE user_tasks SET is_last_selected = 1 WHERE task_id = ?",
                        task_id
                    )
                    .execute(&data.db)
                    .await?;

                    let row = sqlx::query!(
                        "SELECT description FROM user_tasks WHERE task_id = ?",
                        task_id
                    )
                    .fetch_one(&data.db)
                    .await?;
                    row.description
                } else {
                    "Nothing in particular".to_string()
                };

                component
                    .create_response(
                        ctx,
                        serenity::CreateInteractionResponse::UpdateMessage(
                            serenity::CreateInteractionResponseMessage::new()
                                .content(format!(
                                    "<@{}> Active task updated to: **{}**.",
                                    user_id, selected_desc
                                ))
                                .components(vec![]), // Removes the dropdown menu
                        ),
                    )
                    .await?;

                let msg = component.message.clone();
                let http = ctx.http.clone();

                // Spawns a background task to delete the confirmation text after 5 seconds
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    let _ = msg.delete(&http).await;
                });
            } else if component.data.custom_id.starts_with("task_toggle_") {
                let parts: Vec<&str> = component.data.custom_id.split('_').collect();
                if parts.len() == 4 {
                    let task_id: i64 = parts[2].parse().unwrap();
                    let target_user_id: i64 = parts[3].parse().unwrap();

                    if target_user_id != user_id {
                        component
                            .create_response(
                                ctx,
                                serenity::CreateInteractionResponse::Message(
                                    serenity::CreateInteractionResponseMessage::new()
                                        .content("You cannot modify another user's todo list.")
                                        .ephemeral(true), // Ensures only the clicking user sees this warning
                                ),
                            )
                            .await?;
                        return Ok(());
                    }

                    let now = chrono::Utc::now().timestamp();

                    sqlx::query!(
                        "UPDATE user_tasks 
                         SET completed_at = CASE WHEN completed_at IS NULL THEN ? ELSE NULL END,
                             is_last_selected = CASE WHEN completed_at IS NULL THEN 0 ELSE is_last_selected END
                         WHERE task_id = ?",
                        now, task_id
                    )
                    .execute(&data.db)
                    .await?;

                    sqlx::query!(
                        "DELETE FROM user_tasks 
                         WHERE user_id = ? AND completed_at IS NOT NULL 
                         AND task_id NOT IN (
                             SELECT task_id FROM user_tasks 
                             WHERE user_id = ? AND completed_at IS NOT NULL 
                             ORDER BY completed_at DESC LIMIT 20
                         )",
                        user_id,
                        user_id
                    )
                    .execute(&data.db)
                    .await?;

                    let new_components = build_todo_components(&data.db, user_id).await?;

                    component
                        .create_response(
                            ctx,
                            serenity::CreateInteractionResponse::UpdateMessage(
                                serenity::CreateInteractionResponseMessage::new()
                                    .components(new_components),
                            ),
                        )
                        .await?;
                }
            } else if component.data.custom_id.starts_with("task_delete_") {
                let extracted_id = component.data.custom_id.replace("task_delete_", "");
                if extracted_id != user_id.to_string() {
                    component
                        .create_response(
                            ctx,
                            serenity::CreateInteractionResponse::Message(
                                serenity::CreateInteractionResponseMessage::new()
                                    .content("Nope, not gonna happen...")
                                    .ephemeral(true), // Ensures only the clicking user sees this warning
                            ),
                        )
                        .await?;
                    return Ok(());
                }

                let serenity::ComponentInteractionDataKind::StringSelect { ref values } =
                    component.data.kind
                else {
                    return Ok(());
                };

                for val in values {
                    let task_id: i64 = val.parse().unwrap();
                    sqlx::query!("DELETE FROM user_tasks WHERE task_id = ?", task_id)
                        .execute(&data.db)
                        .await?;
                }

                component
                    .create_response(
                        ctx,
                        serenity::CreateInteractionResponse::UpdateMessage(
                            serenity::CreateInteractionResponseMessage::new()
                                .content(format!("Successfully deleted {} task(s).", values.len()))
                                .components(vec![]),
                        ),
                    )
                    .await?;
            }
        }
        _ => {}
    }
    Ok(())
}
