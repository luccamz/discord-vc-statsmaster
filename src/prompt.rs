use crate::commands::tasks::{prompt_text, task_menu, PromptTask};
use crate::state::Prompts;
use poise::serenity_prelude as serenity;
use std::sync::Arc;

/// Posts the "Currently working on" prompt and deletes it once the user leaves the channel.
pub async fn post_task_prompt(
    http: &Arc<serenity::Http>,
    shard: &serenity::ShardMessenger,
    prompts: &Prompts,
    key: (u64, u64),
    channel_id: i64,
    tasks: &[PromptTask],
    silent: bool,
) {
    let selected = tasks.iter().find(|t| t.last_selected).map(|t| t.id);
    let user_id = key.0 as i64;
    let mut message = serenity::CreateMessage::new()
        .content(prompt_text(user_id, tasks, selected))
        .select_menu(task_menu(format!("task_select_{user_id}"), tasks, selected));
    if silent {
        message = message.flags(serenity::MessageFlags::SUPPRESS_NOTIFICATIONS);
    }
    let channel = serenity::ChannelId::new(channel_id as u64);
    let Ok(message) = channel.send_message(&**http, message).await else {
        return;
    };

    let http = http.clone();
    let shard_messenger = shard.clone();
    let prompts = prompts.clone();
    let prompt = (message.channel_id, message.id);
    prompts
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, prompt);

    tokio::spawn(async move {
        use serenity::futures::StreamExt;

        let mut collector = serenity::collector::collect(&shard_messenger, move |event| {
            if let serenity::Event::VoiceStateUpdate(data) = event {
                let state = &data.voice_state;
                if state.user_id.get() as i64 == user_id
                    && state.channel_id.map(|c| c.get() as i64) != Some(channel_id)
                {
                    return Some(());
                }
            }
            None
        });

        let _ = tokio::time::timeout(std::time::Duration::from_secs(86400), collector.next()).await;

        let _ = message.delete(&http).await;
        let mut prompts = prompts.lock().unwrap_or_else(|e| e.into_inner());
        if prompts.get(&key) == Some(&prompt) {
            prompts.remove(&key);
        }
    });
}
