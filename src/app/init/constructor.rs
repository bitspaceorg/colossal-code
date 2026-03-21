use agent_core::{Agent, AgentMessage};
use color_eyre::Result;
use ratatui::layout::Rect;
use std::{collections::HashMap, sync::Arc, time::Instant};
use tokio::sync::mpsc;

use crate::app::init::constructor_backend::BackendConfig;
use crate::app::{
    AgentState, App, Mode, PersistenceState, Phase, RichEditor, SafetyState, SessionManager,
    Survey, UiState,
};

impl App {
    pub(crate) async fn new() -> Result<Self> {
        let title_lines = Self::create_title_lines();
        let visible_chars = vec![0; title_lines.len()];

        let (input_tx, input_rx) = mpsc::unbounded_channel::<AgentMessage>();
        let (output_tx, output_rx) = mpsc::unbounded_channel::<AgentMessage>();

        let history_file_path = Self::get_history_file_path()?;
        let command_history = Self::load_history(&history_file_path);

        let _ = Self::initialize_config_file();
        let _ = Self::initialize_conversations_dir();

        let current_model = Self::load_model_setting();
        let current_context_tokens = Self::detect_context_tokens(current_model.as_deref());
        let auto_summarize_threshold = Self::load_auto_summarize_threshold_setting();
        let scroll_messages_enabled = Self::load_scroll_setting();

        let backend_env = BackendConfig::read().into_environment();
        let limit_thinking_to_first_token = backend_env.limit_thinking_to_first_token;
        Self::apply_backend_environment(&backend_env);

        let agent = Agent::new_with_model(current_model.clone())
            .await
            .map_err(|e| color_eyre::eyre::eyre!("Failed to initialize agent: {}", e))?;

        agent
            .initialize_backend()
            .await
            .map_err(|e| color_eyre::eyre::eyre!("Failed to load backend: {}", e))?;

        let agent_arc = Arc::new(agent);
        Self::spawn_agent_runtime(Arc::clone(&agent_arc), input_rx, output_tx);

        Ok(Self {
            input: String::new(),
            messages: Vec::new(),
            message_types: Vec::new(),
            message_states: Vec::new(),
            message_metadata: Vec::new(),
            message_timestamps: Vec::new(),
            character_index: 0,
            input_modified: false,
            mode: Mode::Normal,
            status_left: Self::compute_status_left_initial()?,
            phase: Phase::Ascii,
            title_lines,
            visible_chars,
            visible_tips: 0,
            last_update: Instant::now(),
            initial_screen_cleared: false,
            cached_mode_content: None,
            editor: RichEditor::new(),
            command_input: String::new(),
            exit: false,
            nav_scroll_offset: 0,
            message_scroll_offset: 0,
            follow_messages_tail: true,
            scroll_messages_enabled,
            last_messages_area: Rect::default(),
            last_message_total_lines: 0,
            last_message_scroll_at: None,
            expanded_edit_file_diffs: std::collections::HashSet::new(),
            visible_edit_file_artifacts: Vec::new(),
            terminal_cursor_hidden: false,
            nav_needs_init: false,
            flash_highlight: None,
            ctrl_c_pressed: None,
            survey: Survey::new(10, 0.33),
            safety_state: SafetyState::default(),
            agent: Some(agent_arc),
            agent_tx: Some(input_tx),
            agent_rx: Some(output_rx),
            agent_state: AgentState::default(),
            is_thinking: false,
            thinking_indicator_active: false,
            thinking_loader_frame: 0,
            thinking_last_update: Instant::now(),
            thinking_snowflake_frames: vec!["✽ ", "✻ ", "✹ ", "❆ ", "❅ "],
            thinking_words: vec![
                "Discombobulating",
                "Fabricating",
                "Procrastinating",
                "Dilly-dallying",
                "Waffling",
                "Rambling",
                "Babbling",
                "Daydreaming",
                "Woolgathering",
                "Muddling",
                "Overthinking",
                "Pondering",
                "Wondering",
                "Speculating",
                "Ruminating",
                "Meditating",
                "Contemplating",
                "Justifying",
                "Rationalizing",
                "Concocting",
                "Scheming",
                "Contriving",
                "Improvising",
                "Inventing",
                "Juggling",
                "Balancing",
                "Spinning",
                "Flipping",
                "Twisting",
                "Tangling",
                "Untangling",
                "Wrangling",
                "Wrestling",
                "Struggling",
                "Scrambling",
                "Hustling",
                "Bustling",
                "Fidgeting",
                "Squirming",
                "Floundering",
                "Stumbling",
                "Trudging",
                "Meandering",
                "Wandering",
                "Roaming",
                "Drifting",
                "Sailing",
                "Surfing",
                "Skimming",
                "Scanning",
                "Browsing",
                "Foraging",
                "Hunting",
                "Tracking",
                "Digging",
                "Excavating",
                "Burrowing",
                "Mining",
                "Fishing",
                "Netting",
                "Harvesting",
                "Sifting",
                "Filtering",
                "Shuffling",
                "Juggling",
                "Mixing",
                "Blending",
                "Stirring",
                "Brewing",
                "Stewing",
                "Marinating",
                "Cooking",
                "Baking",
                "Toasting",
                "Roasting",
                "Grilling",
                "Seasoning",
                "Garnishing",
                "Polishing",
                "Refining",
                "Sharpening",
                "Sanding",
                "Hammering",
                "Chiseling",
                "Painting",
                "Sketching",
                "Drafting",
                "Editing",
                "Proofing",
                "Revising",
                "Rewriting",
                "Compiling",
                "Assembling",
                "Skedaddling",
                "Bamboozling",
                "Hoodwinking",
                "Ramshackling",
                "Fiddling",
                "Hocus-pocusing",
                "Abracadabra-ing",
                "Wiggling",
                "Quibbling",
                "Flipping",
                "Flopping",
                "Fizzling",
                "Gobsmacking",
                "Zig-zagging",
                "Zapping",
                "Snickering",
                "Shazam-ing",
                "Floofing",
                "Snazzling",
                "Glorpifying",
                "Yapping",
                "Crinkling",
                "Boopity-booping",
                "Bumbling",
                "Mumbling",
                "Razzle-dazzling",
                "Piffle-poofing",
                "Squashing",
                "Flabbering",
                "Mingling",
                "Mangling",
                "Bippity-boppitying",
                "Jumble-wumbling",
                "Ding-a-linging",
                "Skronking",
                "Zoodling",
                "Zaddling",
                "Dippy-dappitying",
                "Swozzling",
                "Frazzling",
                "Snarf-blasting",
            ],
            thinking_current_word: "Thinking".to_string(),
            thinking_current_summary: None,
            thinking_position: 0,
            thinking_last_word_change: Instant::now(),
            thinking_last_tick: Instant::now(),
            thinking_start_time: None,
            thinking_token_count: 0,
            limit_thinking_to_first_token,
            generation_stats: None,
            generation_stats_rendered: false,
            streaming_completion_tokens: 0,
            last_known_context_tokens: 0,
            command_history,
            history_index: None,
            temp_input: None,
            history_file_path,
            queued_messages: Vec::new(),
            editing_queue_index: None,
            show_queue_choice: false,
            queue_choice_input: String::new(),
            export_pending: false,
            review_pending: None,
            spec_pending: None,
            orchestration_pending: None,
            orchestration_in_progress: false,
            compact_pending: None,
            last_compacted_summary: None,
            is_auto_summarize: false,
            auto_summarize_threshold,
            context_sync_pending: false,
            context_sync_started: None,
            context_inject_expected: false,
            compaction_resume_prompt: None,
            compaction_resume_ready: false,
            compaction_history: Vec::new(),
            show_summary_history: false,
            summary_history_selected: 0,
            persistence_state: PersistenceState::default(),
            nav_snapshot: None,
            session_manager: SessionManager::new(),
            autocomplete_active: false,
            autocomplete_suggestions: Vec::new(),
            autocomplete_selected_index: 0,
            thinking_raw_content: String::new(),
            vim_mode_enabled: Self::load_vim_mode_setting(),
            vim_input_editor: RichEditor::new(),
            show_background_tasks: false,
            background_tasks: Vec::new(),
            background_tasks_selected: 0,
            viewing_task: None,
            ui_state: UiState::default(),
            help_commands_selected: 0,
            resume_conversations: Vec::new(),
            resume_selected: 0,
            resume_load_pending: false,
            is_fork_mode: false,
            show_todos: false,
            show_model_selection: false,
            available_models: Vec::new(),
            model_selected_index: 0,
            show_rewind: false,
            rewind_points: Vec::new(),
            rewind_selected: 0,
            current_file_changes: Vec::new(),
            last_tool_args: None,
            current_model,
            current_context_tokens,
            current_spec: None,
            spec_pane_selected: 0,
            step_tool_calls: HashMap::new(),
            step_label_overrides: HashMap::new(),
            active_step_prefix: None,
            active_tool_call: None,
            next_tool_call_id: 0,
            orchestrator_control: None,
            orchestrator_event_rx: None,
            orchestrator_task: None,
            orchestrator_sessions: HashMap::new(),
            orchestrator_history: Vec::new(),
            latest_summaries: HashMap::new(),
            orchestrator_paused: false,
            has_orchestrator_activity: false,
            spec_pane_show_history: false,
            spec_step_drawer_open: false,
            show_history_panel: false,
            history_panel_selected: 0,
            status_message: None,
            sub_agent_contexts: HashMap::new(),
            expanded_sub_agent: None,
            expanded_sub_agent_before_alt_w: None,
            mode_before_sub_agent: None,
            rendering_sub_agent_view: false,
            rendering_sub_agent_prefix: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::app::init::constructor_backend::BackendConfig;

    #[test]
    fn external_backend_uses_expected_defaults() {
        let config = BackendConfig {
            mode: "external".to_string(),
            base_url: None,
            api_key: "secret".to_string(),
            completions_path: None,
            google_user_project: Some("proj-123".to_string()),
        };

        let env = config.into_environment();

        assert_eq!(env.backend_mode, "external");
        assert_eq!(env.base_url.as_deref(), Some("https://api.openai.com"));
        assert_eq!(env.completions_path.as_deref(), Some("chat/completions"));
        assert_eq!(env.api_key.as_deref(), Some("secret"));
        assert_eq!(env.google_user_project.as_deref(), Some("proj-123"));
        assert!(env.limit_thinking_to_first_token);
    }

    #[test]
    fn local_backend_clears_http_specific_settings() {
        let config = BackendConfig {
            mode: "local".to_string(),
            base_url: Some("https://example.com".to_string()),
            api_key: "secret".to_string(),
            completions_path: Some("custom/path".to_string()),
            google_user_project: None,
        };

        let env = config.into_environment();

        assert_eq!(env.backend_mode, "local");
        assert!(env.base_url.is_none());
        assert!(env.completions_path.is_none());
        assert!(env.api_key.is_none());
        assert!(!env.limit_thinking_to_first_token);
    }

    #[test]
    fn unknown_backend_falls_back_to_http_defaults() {
        let config = BackendConfig {
            mode: "custom".to_string(),
            base_url: None,
            api_key: String::new(),
            completions_path: None,
            google_user_project: None,
        };

        let env = config.into_environment();

        assert_eq!(env.backend_mode, "http");
        assert_eq!(env.base_url.as_deref(), Some("http://127.0.0.1:8080"));
        assert_eq!(
            env.completions_path.as_deref(),
            Some("/v1/chat/completions")
        );
        assert!(env.api_key.is_none());
        assert!(!env.limit_thinking_to_first_token);
    }
}
