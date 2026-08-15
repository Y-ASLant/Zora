use std::collections::HashMap;
use std::sync::mpsc::SyncSender;

use persistence::model::{AgentConversation, AgentConversationData};
use serde::{Deserialize, Serialize};
use warpui_extras::secure_storage::{self, AppContextExt as _};

use crate::ai::agent::api::ServerConversationToken;
use crate::ai::agent::conversation::AIConversationId;
use crate::persistence::ModelEvent;

const AGENT_CONVERSATION_SECRETS_KEY: &str = "AgentConversationSecrets";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct AgentConversationSecrets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_conversation_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from_server_conversation_token: Option<String>,
}

type PersistedAgentConversationSecrets = HashMap<String, AgentConversationSecrets>;

fn load_all(app: &warpui::AppContext) -> PersistedAgentConversationSecrets {
    match app
        .secure_storage()
        .read_value(AGENT_CONVERSATION_SECRETS_KEY)
    {
        Ok(json) => serde_json::from_str(&json).unwrap_or_else(|err| {
            log::error!("Failed to deserialize agent conversation secrets: {err:#}");
            HashMap::new()
        }),
        Err(secure_storage::Error::NotFound) => HashMap::new(),
        Err(err) => {
            log::error!("Failed to read agent conversation secrets: {err:#}");
            HashMap::new()
        }
    }
}

fn write_all(app: &warpui::AppContext, secrets: &PersistedAgentConversationSecrets) {
    let Ok(json) = serde_json::to_string(secrets) else {
        log::error!("Failed to serialize agent conversation secrets");
        return;
    };
    if let Err(err) = app
        .secure_storage()
        .write_value(AGENT_CONVERSATION_SECRETS_KEY, &json)
    {
        log::error!("Failed to write agent conversation secrets: {err:#}");
    }
}

pub(crate) fn persist_for_conversation(
    app: &warpui::AppContext,
    conversation_id: AIConversationId,
    server_conversation_token: Option<&ServerConversationToken>,
    forked_from_server_conversation_token: Option<&ServerConversationToken>,
) {
    let mut secrets = load_all(app);
    let value = AgentConversationSecrets {
        server_conversation_token: server_conversation_token
            .map(|token| token.as_str().to_string()),
        forked_from_server_conversation_token: forked_from_server_conversation_token
            .map(|token| token.as_str().to_string()),
    };
    let key = conversation_id.to_string();
    if value.server_conversation_token.is_none()
        && value.forked_from_server_conversation_token.is_none()
    {
        secrets.remove(&key);
    } else {
        secrets.insert(key, value);
    }
    write_all(app, &secrets);
}

pub(crate) fn migrate_and_scrub_persisted_conversation_secrets(
    app: &warpui::AppContext,
    conversations: &mut [AgentConversation],
    sqlite_sender: Option<&SyncSender<ModelEvent>>,
) {
    let mut secrets = load_all(app);
    let mut changed_rows = Vec::new();

    for conversation in conversations {
        let Ok(mut data) = serde_json::from_str::<AgentConversationData>(
            &conversation.conversation.conversation_data,
        ) else {
            continue;
        };

        let conversation_id = conversation.conversation.conversation_id.clone();
        if data.server_conversation_token.is_none()
            && data.forked_from_server_conversation_token.is_none()
        {
            if let Some(secret) = secrets.get(&conversation_id) {
                data.server_conversation_token = secret.server_conversation_token.clone();
                data.forked_from_server_conversation_token =
                    secret.forked_from_server_conversation_token.clone();
                match serde_json::to_string(&data) {
                    Ok(json) => conversation.conversation.conversation_data = json,
                    Err(err) => {
                        log::error!("Failed to hydrate agent conversation secrets: {err:#}");
                    }
                }
            }
            continue;
        }

        let mut scrubbed_data = data.clone();
        let entry = secrets.entry(conversation_id.clone()).or_default();
        if scrubbed_data.server_conversation_token.is_some() {
            entry.server_conversation_token = scrubbed_data.server_conversation_token.take();
        }
        if scrubbed_data
            .forked_from_server_conversation_token
            .is_some()
        {
            entry.forked_from_server_conversation_token =
                scrubbed_data.forked_from_server_conversation_token.take();
        }

        match serde_json::to_string(&scrubbed_data) {
            Ok(json) => {
                changed_rows.push((conversation_id, conversation.tasks.clone(), scrubbed_data));
                conversation.conversation.conversation_data =
                    serde_json::to_string(&data).unwrap_or(json);
            }
            Err(err) => {
                log::error!("Failed to scrub agent conversation data: {err:#}");
            }
        }
    }

    if changed_rows.is_empty() {
        return;
    }

    write_all(app, &secrets);
    if let Some(sqlite_sender) = sqlite_sender {
        for (conversation_id, updated_tasks, conversation_data) in changed_rows {
            if let Err(err) = sqlite_sender.send(ModelEvent::UpdateMultiAgentConversation {
                conversation_id,
                updated_tasks,
                conversation_data,
            }) {
                log::error!("Failed to persist scrubbed agent conversation data: {err:?}");
            }
        }
    }
}
