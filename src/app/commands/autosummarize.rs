use crate::{
    App, MAX_AUTO_SUMMARIZE_THRESHOLD, MIN_AUTO_SUMMARIZE_THRESHOLD, MessageState, MessageType,
};

impl App {
    pub(crate) fn handle_auto_summarize_threshold_command(&mut self, command: &str) -> bool {
        let parts: Vec<&str> = command.split_whitespace().collect();

        if parts.len() == 1 {
            let status_text = format!(
                " ⎿ Auto-summarize triggers when {}. Use '/autosummarize [percent-used]' to change it.",
                self.auto_summarize_hint()
            );
            self.messages.push(status_text);
            self.message_types.push(MessageType::Agent);
            self.message_states.push(MessageState::Sent);
            return true;
        }

        let value_token = parts[1].trim().trim_end_matches('%');
        match value_token.parse::<f32>() {
            Ok(value) => {
                if !(MIN_AUTO_SUMMARIZE_THRESHOLD..=MAX_AUTO_SUMMARIZE_THRESHOLD).contains(&value) {
                    self.messages.push(format!(
                        " ⎿ Enter a value between {:.0}% and {:.0}% (percent of context used).",
                        MIN_AUTO_SUMMARIZE_THRESHOLD, MAX_AUTO_SUMMARIZE_THRESHOLD
                    ));
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                    return true;
                }

                self.auto_summarize_threshold = Self::clamp_auto_summarize_threshold(value);
                if let Err(e) = self.save_config() {
                    self.messages.push(format!(
                        " ⎿ Auto-summarize updated but failed to persist setting: {}",
                        e
                    ));
                    self.message_types.push(MessageType::Agent);
                    self.message_states.push(MessageState::Sent);
                    return true;
                }

                self.messages.push(format!(
                    " ⎿ Auto-summarize now triggers when {}.",
                    self.auto_summarize_hint()
                ));
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
                true
            }
            Err(_) => {
                self.messages.push(
                    " ⎿ Invalid auto-summarize threshold. Provide a numeric percent of context used."
                        .to_string(),
                );
                self.message_types.push(MessageType::Agent);
                self.message_states.push(MessageState::Sent);
                true
            }
        }
    }
}
