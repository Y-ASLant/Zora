use super::*;
use settings::Setting;
use warpui::{App, Entity, SingletonEntity};

use crate::ai::agent_providers::{llm_id, AgentProviderSecrets};
use crate::ai::execution_profiles::profiles::AIExecutionProfilesModel;
use crate::ai::mcp::TemplatableMCPServerManager;
use crate::auth::{AuthManager, AuthStateProvider};
use crate::cloud_object::model::persistence::ObjectStoreModel;
use crate::cloud_object::update_manager::UpdateManager;
use crate::network::NetworkStatus;
use crate::settings::{AISettings, AgentProvider, AgentProviderApiType, AgentProviderModel};
use crate::test_util::settings::initialize_settings_for_tests;
use crate::workspaces::user_workspaces::UserWorkspaces;
use crate::LaunchMode;

struct TestTerminalEntity;

impl Entity for TestTerminalEntity {
    type Event = ();
}

fn sample_provider(
    id: &str,
    name: &str,
    api_type: AgentProviderApiType,
    model: &str,
) -> AgentProvider {
    AgentProvider {
        id: id.to_owned(),
        name: name.to_owned(),
        kind: Default::default(),
        api_type,
        base_url: api_type.default_base_url().to_owned(),
        models: vec![AgentProviderModel::from_id(model.to_owned())],
        extra_headers: Vec::new(),
    }
}

fn install_llm_test_singletons(app: &mut App) {
    initialize_settings_for_tests(app);
    app.add_singleton_model(AgentProviderSecrets::new);
    app.add_singleton_model(|_| NetworkStatus::new());
    app.add_singleton_model(|_| AuthStateProvider::new_for_test());
    app.add_singleton_model(AuthManager::new_for_test);
    app.add_singleton_model(UserWorkspaces::default_mock);
    app.add_singleton_model(UpdateManager::mock);
    app.add_singleton_model(ObjectStoreModel::mock);
    app.add_singleton_model(|_| TemplatableMCPServerManager::default());
}

#[test]
fn llm_info_deserializes_without_base_model_name() {
    let raw = r#"{
            "display_name": "gpt-4o",
            "id": "gpt-4o",
            "usage_metadata": {
                "request_multiplier": 1,
                "credit_multiplier": null
            },
            "description": null,
            "disable_reason": null,
            "vision_supported": false,
            "spec": null,
            "provider": "Unknown"
        }"#;

    let info: LLMInfo = serde_json::from_str(raw).expect("should deserialize");
    assert_eq!(info.display_name, "gpt-4o");
    assert_eq!(info.base_model_name, "gpt-4o");
}

#[test]
fn llm_info_deserializes_host_configs_as_vec() {
    // Wire format from server: host_configs is a Vec
    let raw = r#"{
            "display_name": "gpt-4o",
            "id": "gpt-4o",
            "usage_metadata": { "request_multiplier": 1, "credit_multiplier": null },
            "provider": "OpenAI",
            "host_configs": [
                { "enabled": true, "model_routing_host": "DirectApi" },
                { "enabled": false, "model_routing_host": "AwsBedrock" }
            ]
        }"#;

    let info: LLMInfo = serde_json::from_str(raw).expect("should deserialize vec format");
    assert_eq!(info.display_name, "gpt-4o");
    assert_eq!(info.host_configs.len(), 2);
    assert!(
        info.host_configs
            .get(&LLMModelHost::DirectApi)
            .unwrap()
            .enabled
    );
    assert!(
        !info
            .host_configs
            .get(&LLMModelHost::AwsBedrock)
            .unwrap()
            .enabled
    );
}

#[test]
fn llm_info_round_trip_serializes_and_deserializes() {
    // Start with wire format (Vec)
    let wire_json = r#"{
            "display_name": "claude-3",
            "base_model_name": "claude-3",
            "id": "claude-3",
            "usage_metadata": { "request_multiplier": 2, "credit_multiplier": 1.5 },
            "description": "A powerful model",
            "vision_supported": true,
            "provider": "Anthropic",
            "host_configs": [
                { "enabled": true, "model_routing_host": "DirectApi" }
            ]
        }"#;

    // Deserialize from wire format
    let info: LLMInfo = serde_json::from_str(wire_json).expect("should deserialize");

    // Serialize (produces HashMap format)
    let serialized = serde_json::to_string(&info).expect("should serialize");

    // Deserialize again (from HashMap format)
    let round_tripped: LLMInfo =
        serde_json::from_str(&serialized).expect("should deserialize after round trip");

    assert_eq!(info, round_tripped);
}

#[test]
fn profile_base_model_takes_precedence_over_global_last_used_model() {
    App::test((), |mut app| async move {
        install_llm_test_singletons(&mut app);

        let deepseek_provider_id = "deepseek";
        let openai_provider_id = "openai";
        let deepseek_model_id = llm_id::encode(deepseek_provider_id, "deepseek-v4-flash");
        let openai_model_id = llm_id::encode(openai_provider_id, "gpt-5.5");

        app.update(|ctx| {
            AISettings::handle(ctx).update(ctx, |settings, ctx| {
                let _ = settings.agent_providers.set_value(
                    vec![
                        sample_provider(
                            deepseek_provider_id,
                            "DeepSeek",
                            AgentProviderApiType::DeepSeek,
                            "deepseek-v4-flash",
                        ),
                        sample_provider(
                            openai_provider_id,
                            "OpenAI",
                            AgentProviderApiType::OpenAi,
                            "gpt-5.5",
                        ),
                    ],
                    ctx,
                );
                let _ = settings
                    .byop_last_used_model_id
                    .set_value(openai_model_id.to_string(), ctx);
            });
        });

        let profile_model = app.add_singleton_model(|ctx| {
            AIExecutionProfilesModel::new(&LaunchMode::new_for_unit_test(), ctx)
        });
        app.add_singleton_model(LLMPreferences::new);

        profile_model.update(&mut app, |model, ctx| {
            let default_profile_id = model.default_profile_id();
            model.set_base_model(default_profile_id, Some(deepseek_model_id.clone()), ctx);
        });

        app.read(|ctx| {
            let active_llm = LLMPreferences::as_ref(ctx).get_active_base_model(ctx, None);
            assert_eq!(
                active_llm.id, deepseek_model_id,
                "profile default model should not be overridden by stale global last-used model"
            );
        });
    });
}

#[test]
fn profile_selector_model_selection_updates_profile_default() {
    App::test((), |mut app| async move {
        install_llm_test_singletons(&mut app);

        let flash_provider_id = "deepseek-flash";
        let pro_provider_id = "deepseek-pro";
        let flash_model_id = llm_id::encode(flash_provider_id, "deepseek-v4-flash");
        let pro_model_id = llm_id::encode(pro_provider_id, "deepseek-v4-pro");

        app.update(|ctx| {
            AISettings::handle(ctx).update(ctx, |settings, ctx| {
                let _ = settings.agent_providers.set_value(
                    vec![
                        sample_provider(
                            flash_provider_id,
                            "DeepSeek Flash",
                            AgentProviderApiType::DeepSeek,
                            "deepseek-v4-flash",
                        ),
                        sample_provider(
                            pro_provider_id,
                            "DeepSeek Pro",
                            AgentProviderApiType::DeepSeek,
                            "deepseek-v4-pro",
                        ),
                    ],
                    ctx,
                );
            });
        });

        let profile_model = app.add_singleton_model(|ctx| {
            AIExecutionProfilesModel::new(&LaunchMode::new_for_unit_test(), ctx)
        });
        let llm_preferences = app.add_singleton_model(LLMPreferences::new);
        let source_terminal_id = app.add_model(|_| TestTerminalEntity).id();
        let new_terminal_id = app.add_model(|_| TestTerminalEntity).id();

        profile_model.update(&mut app, |model, ctx| {
            let default_profile_id = model.default_profile_id();
            model.set_base_model(default_profile_id, Some(flash_model_id.clone()), ctx);
        });

        llm_preferences.update(&mut app, |preferences, ctx| {
            preferences.update_profile_default_agent_mode_llm(
                &pro_model_id,
                source_terminal_id,
                ctx,
            );
        });

        app.read(|ctx| {
            let profiles = AIExecutionProfilesModel::as_ref(ctx);
            let default_profile = profiles.default_profile(ctx);
            assert_eq!(
                default_profile.data().base_model,
                Some(pro_model_id.clone())
            );

            let preferences = LLMPreferences::as_ref(ctx);
            assert_eq!(
                preferences
                    .get_active_base_model(ctx, Some(source_terminal_id))
                    .id,
                pro_model_id
            );
            assert_eq!(
                preferences
                    .get_active_base_model(ctx, Some(new_terminal_id))
                    .id,
                pro_model_id,
                "新的 Agent 入口应该读取 active profile 上持久化的模型"
            );
            assert!(preferences
                .get_base_llm_override(source_terminal_id)
                .is_none());
            assert_eq!(
                AISettings::as_ref(ctx).byop_last_used_model_id.to_string(),
                pro_model_id.to_string()
            );
        });
    });
}
