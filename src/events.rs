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
            let guild_id = match new.guild_id {
                Some(id) => id.get(),
                None => return Ok(()),
            } as i64;

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
                    e.insert(SessionData {
                        start_time: now,
                        active_task_id: None,
                    });

                    drop(sessions);

                    let pending_tasks = sqlx::query!(
                        "SELECT task_id, description FROM user_tasks WHERE user_id = ? AND completed_at IS NULL",
                        user_id
                    )
                    .fetch_all(&data.db)
                    .await?;

                    let prompt_content = format!(
                        "<@{}> Currently working on.. Nothing in particular. Change status?",
                        user_id
                    );

                    let mut options = Vec::new();
                    let no_task_opt =
                        serenity::CreateSelectMenuOption::new("Nothing in particular", "none")
                            .default_selection(true);
                    options.push(no_task_opt);

                    for task in pending_tasks {
                        let opt = serenity::CreateSelectMenuOption::new(
                            &task.description,
                            task.task_id.unwrap().to_string(), // Unwrapped Option
                        );
                        options.push(opt);
                    }

                    options.truncate(25);

                    let menu = serenity::CreateSelectMenu::new(
                        format!("task_select_{}", user_id),
                        serenity::CreateSelectMenuKind::String { options },
                    );

                    let channel = serenity::ChannelId::new(current_channel_id as u64);
                    let _ = channel
                        .send_message(
                            ctx, // Replaced &new.user_id with ctx
                            serenity::CreateMessage::new()
                                .content(prompt_content)
                                .select_menu(menu),
                        )
                        .await;
                }
            } else if let Some(session) = sessions.remove(&(user_id as u64, guild_id as u64)) {
                let duration = now - session.start_time;
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
            let guild_id = match component.guild_id {
                Some(id) => id.get() as i64,
                None => return Ok(()),
            };

            if component.data.custom_id.starts_with("task_select_") {
                let extracted_id = component.data.custom_id.replace("task_select_", "");
                if extracted_id != user_id.to_string() {
                    return Ok(());
                }

                let selected_value = match component.data.kind {
                    serenity::ComponentInteractionDataKind::StringSelect { ref values } => {
                        &values[0]
                    }
                    _ => return Ok(()),
                };

                let mut sessions = data.active_sessions.lock().await;
                if let Some(session) = sessions.get_mut(&(user_id as u64, guild_id as u64)) {
                    if selected_value == "none" {
                        session.active_task_id = None;
                    } else {
                        session.active_task_id = Some(selected_value.parse().unwrap());
                    }
                }

                component
                    .create_response(
                        ctx, // Replaced &component.user with ctx
                        serenity::CreateInteractionResponse::UpdateMessage(
                            serenity::CreateInteractionResponseMessage::new()
                                .content("Active task updated."),
                        ),
                    )
                    .await?;
            } else if component.data.custom_id.starts_with("task_toggle_") {
                let parts: Vec<&str> = component.data.custom_id.split('_').collect();
                if parts.len() == 4 {
                    let task_id: i64 = parts[2].parse().unwrap();
                    let target_user_id: i64 = parts[3].parse().unwrap();

                    if target_user_id != user_id {
                        return Ok(());
                    }

                    let now = chrono::Utc::now().timestamp();

                    sqlx::query!(
                        "UPDATE user_tasks 
                             SET completed_at = CASE WHEN completed_at IS NULL THEN ? ELSE NULL END
                             WHERE task_id = ?",
                        now,
                        task_id
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
                            ctx, // Replaced &component.user with ctx
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
                    return Ok(());
                }

                let selected_value = match component.data.kind {
                    serenity::ComponentInteractionDataKind::StringSelect { ref values } => {
                        &values[0]
                    }
                    _ => return Ok(()),
                };

                let task_id: i64 = selected_value.parse().unwrap();
                sqlx::query!("DELETE FROM user_tasks WHERE task_id = ?", task_id)
                    .execute(&data.db)
                    .await?;

                component
                    .create_response(
                        ctx, // Replaced &component.user with ctx
                        serenity::CreateInteractionResponse::UpdateMessage(
                            serenity::CreateInteractionResponseMessage::new()
                                .content("Task deleted successfully.")
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
