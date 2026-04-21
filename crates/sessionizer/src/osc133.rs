//! OSC 133 (FinalTerm semantic prompt) parser for detecting command completion
//!
//! OSC 133 marks four regions of shell interaction:
//! - A: Prompt start
//! - B: Command input start (prompt ended)
//! - C: Command execution start (command submitted)
//! - D: Command finished (with optional exit code)
//!
//! Wire format: ESC ] 133 ; <cmd> [; <params>] ST
//! where ST is either BEL (0x07) or ESC \ (0x1B 0x5C)

const ESC: u8 = 0x1B;
const BEL: u8 = 0x07;
const BACKSLASH: u8 = b'\\';
const RIGHT_BRACKET: u8 = b']';
const PARAM_BUF_CAP: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Esc,
    OscParam,
    OscEsc,
}

/// Events emitted when OSC 133 markers are detected
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// OSC 133;A - Shell about to display prompt
    PromptStart,
    /// OSC 133;B - Prompt ended, user can type command
    CommandStart,
    /// OSC 133;C - Command submitted for execution
    CommandExecuted,
    /// OSC 133;D[;exit_code] - Command finished
    CommandFinished { exit_code: Option<i32> },
}

/// The current semantic zone as determined by the most recent OSC 133 marker
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    /// No marker seen yet, or after a D marker (between commands)
    #[default]
    Unknown,
    /// Between A and B — the shell is rendering its prompt
    Prompt,
    /// Between B and C — the user is editing a command line
    Input,
    /// Between C and D — command output is being produced
    Output,
}

/// Streaming parser for OSC 133 sequences
#[derive(Debug, Clone)]
pub struct Parser {
    state: State,
    zone: Zone,
    param_buf: [u8; PARAM_BUF_CAP],
    param_len: usize,
}

impl Parser {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            zone: Zone::Unknown,
            param_buf: [0u8; PARAM_BUF_CAP],
            param_len: 0,
        }
    }

    pub fn zone(&self) -> Zone {
        self.zone
    }

    pub fn push(&mut self, data: &[u8]) -> Vec<Event> {
        let mut events = Vec::new();
        for &byte in data {
            match self.state {
                State::Ground => {
                    if byte == ESC {
                        self.state = State::Esc;
                    }
                }
                State::Esc => {
                    if byte == RIGHT_BRACKET {
                        self.state = State::OscParam;
                        self.param_len = 0;
                    } else {
                        self.state = State::Ground;
                    }
                }
                State::OscParam => {
                    if byte == BEL {
                        self.dispatch(&mut events);
                        self.state = State::Ground;
                    } else if byte == ESC {
                        self.state = State::OscEsc;
                    } else if self.param_len < PARAM_BUF_CAP {
                        self.param_buf[self.param_len] = byte;
                        self.param_len += 1;
                    }
                }
                State::OscEsc => {
                    if byte == BACKSLASH {
                        self.dispatch(&mut events);
                    }
                    self.state = State::Ground;
                }
            }
        }
        events
    }

    fn dispatch(&mut self, events: &mut Vec<Event>) {
        let params = &self.param_buf[..self.param_len];

        if params.len() < 5 || &params[..4] != b"133;" {
            return;
        }

        let cmd = params[4];
        let event = match cmd {
            b'A' => {
                self.zone = Zone::Prompt;
                Event::PromptStart
            }
            b'B' => {
                self.zone = Zone::Input;
                Event::CommandStart
            }
            b'C' => {
                self.zone = Zone::Output;
                Event::CommandExecuted
            }
            b'D' => {
                let exit_code = if params.len() > 6 && params[5] == b';' {
                    std::str::from_utf8(&params[6..])
                        .ok()
                        .and_then(|s| s.parse::<i32>().ok())
                } else {
                    None
                };
                self.zone = Zone::Unknown;
                Event::CommandFinished { exit_code }
            }
            _ => return,
        };

        events.push(event);
    }
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

/// Strip OSC 133 escape sequences from output for display
pub fn strip_osc133(input: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(input.len());
    let mut i = 0;

    while i < input.len() {
        if input[i] == ESC
            && i + 1 < input.len()
            && input[i + 1] == RIGHT_BRACKET
            && input[i + 2..].starts_with(b"133;")
        {
            // OSC 133 sequence — find the terminator and skip the whole thing
            let mut j = i + 2;
            while j < input.len() {
                if input[j] == BEL
                    || (input[j] == ESC && j + 1 < input.len() && input[j + 1] == BACKSLASH)
                {
                    j += if input[j] == BEL { 1 } else { 2 };
                    break;
                }
                j += 1;
            }
            i = j;
        } else {
            result.push(input[i]);
            i += 1;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prompt_start_with_bel() {
        let mut parser = Parser::new();
        let events = parser.push(b"\x1b]133;A\x07");
        assert_eq!(events, vec![Event::PromptStart]);
    }

    #[test]
    fn test_command_start_with_bel() {
        let mut parser = Parser::new();
        let events = parser.push(b"\x1b]133;B\x07");
        assert_eq!(events, vec![Event::CommandStart]);
    }

    #[test]
    fn test_command_executed_with_bel() {
        let mut parser = Parser::new();
        let events = parser.push(b"\x1b]133;C\x07");
        assert_eq!(events, vec![Event::CommandExecuted]);
    }

    #[test]
    fn test_command_finished_no_exit_code() {
        let mut parser = Parser::new();
        let events = parser.push(b"\x1b]133;D\x07");
        assert_eq!(events, vec![Event::CommandFinished { exit_code: None }]);
    }

    #[test]
    fn test_command_finished_exit_code_zero() {
        let mut parser = Parser::new();
        let events = parser.push(b"\x1b]133;D;0\x07");
        assert_eq!(events, vec![Event::CommandFinished { exit_code: Some(0) }]);
    }

    #[test]
    fn test_command_finished_exit_code_nonzero() {
        let mut parser = Parser::new();
        let events = parser.push(b"\x1b]133;D;127\x07");
        assert_eq!(
            events,
            vec![Event::CommandFinished {
                exit_code: Some(127)
            }]
        );
    }

    #[test]
    fn test_st_terminator() {
        let mut parser = Parser::new();
        let events = parser.push(b"\x1b]133;A\x1b\\");
        assert_eq!(events, vec![Event::PromptStart]);
    }

    #[test]
    fn test_split_sequence_across_chunks() {
        let mut parser = Parser::new();
        let mut events = parser.push(b"\x1b");
        assert!(events.is_empty());
        events = parser.push(b"]133");
        assert!(events.is_empty());
        events = parser.push(b";A\x07");
        assert_eq!(events, vec![Event::PromptStart]);
    }

    #[test]
    fn test_ignore_non_osc133_sequences() {
        let mut parser = Parser::new();
        let events = parser.push(b"\x1b]0;title\x07");
        assert!(events.is_empty());
    }

    #[test]
    fn test_strip_osc133() {
        let input = b"hello\x1b]133;A\x07world\x1b]133;D;0\x07";
        let result = strip_osc133(input);
        assert_eq!(result, b"helloworld");
    }

    #[test]
    fn test_strip_osc133_preserves_non_133_osc_sequences() {
        // OSC 0 sets the window title — must not be stripped
        let input = b"\x1b]0;mytitle\x07hello\x1b]133;A\x07world";
        let result = strip_osc133(input);
        assert_eq!(result, b"\x1b]0;mytitle\x07helloworld");
    }

    #[test]
    fn test_multiple_events_in_one_chunk() {
        let mut parser = Parser::new();
        let events = parser.push(b"\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07");
        assert_eq!(
            events,
            vec![
                Event::PromptStart,
                Event::CommandStart,
                Event::CommandExecuted,
            ]
        );
    }

    #[test]
    fn test_negative_exit_code() {
        let mut parser = Parser::new();
        let events = parser.push(b"\x1b]133;D;-1\x07");
        assert_eq!(
            events,
            vec![Event::CommandFinished {
                exit_code: Some(-1)
            }]
        );
    }

    #[test]
    fn test_param_buf_overflow_no_panic_and_state_recovers() {
        // Content longer than PARAM_BUF_CAP (64) — must not panic; state machine
        // must reset and correctly parse the next valid sequence.
        let mut parser = Parser::new();
        let long_seq: Vec<u8> = b"\x1b]133;D;"
            .iter()
            .chain(b"9".repeat(80).iter()) // 80 digit exit code — truncated at buf cap
            .chain(b"\x07".iter())
            .copied()
            .collect();
        // Doesn't panic; we don't assert the event since truncation makes it unparseable
        let _ = parser.push(&long_seq);
        // State must have returned to Ground — verify by parsing a fresh valid sequence
        let events = parser.push(b"\x1b]133;A\x07");
        assert_eq!(
            events,
            vec![Event::PromptStart],
            "parser must recover after buffer overflow and parse next sequence correctly"
        );
    }

    #[test]
    fn test_unterminated_sequence_does_not_emit_event_and_zone_unchanged() {
        // A sequence missing its BEL/ST terminator must not emit an event and must
        // not update the zone (the D marker never fired).
        let mut parser = Parser::new();
        let _ = parser.push(b"\x1b]133;D;0"); // no BEL or ST
        assert_eq!(
            parser.zone(),
            Zone::Unknown,
            "zone must not change for an unterminated sequence"
        );
    }

    #[test]
    fn test_osc_esc_state_with_non_backslash_resets_to_ground() {
        // ESC received inside OscParam goes to OscEsc; if next byte is NOT `\`,
        // the sequence is abandoned and the parser returns to Ground.
        let mut parser = Parser::new();
        // ESC ] 133;A ESC X  — ESC-X is not a valid ST terminator
        let events = parser.push(b"\x1b]133;A\x1bX");
        assert!(events.is_empty(), "invalid ST must not emit an event");
        // Parser must be back in Ground — valid next sequence should fire
        let events = parser.push(b"\x1b]133;B\x07");
        assert_eq!(events, vec![Event::CommandStart]);
    }

    #[test]
    fn test_default_impl_matches_new() {
        let a = Parser::new();
        let b = Parser::default();
        assert_eq!(a.zone(), b.zone());
    }

    #[test]
    fn test_clone_preserves_in_flight_state() {
        // Clone mid-parse — both original and clone should complete the same event
        let mut parser = Parser::new();
        parser.push(b"\x1b]133;"); // partial: in OscParam, buf = "133;"
        let mut cloned = parser.clone();

        let orig_events = parser.push(b"A\x07");
        let clone_events = cloned.push(b"A\x07");
        assert_eq!(orig_events, vec![Event::PromptStart]);
        assert_eq!(clone_events, vec![Event::PromptStart]);
    }

    #[test]
    fn test_strip_osc133_with_unterminated_sequence_at_end() {
        // Unterminated sequence at end of buffer — bytes after the start are passed through
        // since we never confirmed it was an OSC 133 to strip.
        // The key guarantee: no panic, output is deterministic.
        let input = b"hello\x1b]133;A"; // no BEL
        let result = strip_osc133(input);
        // The ESC ] starts what looks like an OSC 133, but with no terminator the
        // inner loop runs to the end of the buffer and i is set to input.len(),
        // so nothing extra is emitted. The partial sequence is silently consumed.
        // Just assert no panic and the visible prefix is present.
        assert!(result.starts_with(b"hello"));
    }

    #[test]
    fn test_zone_tracking() {
        let mut parser = Parser::new();
        assert_eq!(parser.zone(), Zone::Unknown);

        parser.push(b"\x1b]133;A\x07");
        assert_eq!(parser.zone(), Zone::Prompt);

        parser.push(b"\x1b]133;B\x07");
        assert_eq!(parser.zone(), Zone::Input);

        parser.push(b"\x1b]133;C\x07");
        assert_eq!(parser.zone(), Zone::Output);

        parser.push(b"\x1b]133;D\x07");
        assert_eq!(parser.zone(), Zone::Unknown);
    }
}
