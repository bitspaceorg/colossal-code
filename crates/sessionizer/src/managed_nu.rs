use crate::error::ColossalErr;
use crate::manager::{
    NuReplayCommandState, NuReplayFileState, NuVariableState, PersistentSessionState,
};
use crate::protocol::SandboxPolicy;
use crate::types::{ExecCommandOutput, ExitStatus, SessionId};
use nu_cmd_lang::create_default_context;
use nu_command::add_shell_command_context;
use nu_engine::{CallExt, env_to_strings, eval_block};
use nu_parser::parse;
use nu_protocol::debugger::WithoutDebug;
use nu_protocol::engine::{Call, Command, EngineState, Stack, StateWorkingSet};
use nu_protocol::{
    Category, IntoValue, PipelineData, ShellError, Signature, Span, SyntaxShape, Type, Value,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// Minimal `run-external` implementation for the managed Nushell runtime.
///
/// This command is registered during `ManagedNuRuntime::new()` so that
/// external-command syntax (`^cmd arg`, `run-external cmd arg`) works inside
/// the embedded runtime.  The command spawns a child process, waits for it,
/// and returns captured stdout/stderr combined output as a Nu string value.
///
/// # Sandbox enforcement
///
/// No special sandbox logic lives here.  The managed runtime runs inside the
/// same OS-level sandbox (landlock / seccomp on Linux, Seatbelt on macOS) as
/// the rest of the agent process, so kernel-level policy automatically applies
/// to every process this command spawns.
///
/// # What is NOT a shell-state side effect
///
/// External processes are isolated from the Nu shell state: they cannot mutate
/// `$env`, the CWD tracker, custom commands, aliases, or session variables
/// unless they use explicit Nu pipes to do so.  After an external command
/// `sync_cwd_from_stack` is still called (in `run_segment`) to catch the
/// rare case where someone wired `$env.PWD = (^some-cmd)`.
#[derive(Clone)]
struct ManagedExternalCommand;

impl Command for ManagedExternalCommand {
    fn name(&self) -> &str {
        "run-external"
    }

    fn description(&self) -> &str {
        "Run an external command (managed Nu sandbox path)."
    }

    fn signature(&self) -> Signature {
        Signature::build(self.name())
            .input_output_types(vec![(Type::Any, Type::Any)])
            .rest(
                "command",
                SyntaxShape::OneOf(vec![SyntaxShape::String, SyntaxShape::Any]),
                "External command to run, with arguments.",
            )
            .category(Category::System)
            .allows_unknown_args()
    }

    fn run(
        &self,
        engine_state: &EngineState,
        stack: &mut Stack,
        call: &Call,
        _input: PipelineData,
    ) -> Result<PipelineData, ShellError> {
        let args: Vec<Value> = call.rest(engine_state, stack, 0)?;
        let mut iter = args.into_iter();

        let Some(cmd_val) = iter.next() else {
            return Err(ShellError::MissingParameter {
                param_name: "no command given".into(),
                span: call.head,
            });
        };

        let cmd_name = match cmd_val.coerce_into_string() {
            Ok(s) => s,
            Err(e) => return Err(e),
        };

        let mut cmd_args: Vec<String> = Vec::new();
        for arg in iter {
            match arg.coerce_into_string() {
                Ok(s) => cmd_args.push(s),
                Err(e) => return Err(e),
            }
        }

        // Use the Nu stack's env vars as the child process environment so that
        // any `load-env` / `$env.X = ...` mutations the agent made are visible
        // to child processes.
        let env_vars = env_to_strings(engine_state, stack).unwrap_or_default();

        // Derive CWD from the stack so `cd` changes are reflected.
        let cwd = engine_state
            .cwd(Some(stack))
            .map(|p| p.into_std_path_buf())
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());

        let output = std::process::Command::new(&cmd_name)
            .args(&cmd_args)
            .current_dir(&cwd)
            .envs(&env_vars)
            .output()
            .map_err(|_e| ShellError::ExternalNotSupported { span: call.head })?;

        // Combine stdout and stderr into a single string value, mirroring how
        // `collect_string` works for the rest of the managed runtime.
        let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        if !stderr_str.is_empty() {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(&stderr_str);
        }
        // Trim the trailing newline that most CLI tools append so callers get
        // clean string values without trailing whitespace.
        let combined = combined.trim_end_matches('\n').to_string();

        if output.status.success() {
            Ok(PipelineData::Value(
                Value::string(combined, call.head),
                None,
            ))
        } else {
            Err(ShellError::ExternalCommand {
                label: format!(
                    "External command `{cmd_name}` failed with exit code {}",
                    output.status.code().unwrap_or(-1)
                ),
                help: combined,
                span: call.head,
            })
        }
    }
}

fn shell_error_to_io(err: ShellError) -> ColossalErr {
    ColossalErr::Io(std::io::Error::other(err.to_string()))
}

fn parse_block(
    engine_state: &EngineState,
    source: &str,
) -> Result<
    (
        Arc<nu_protocol::ast::Block>,
        nu_protocol::engine::StateDelta,
    ),
    ColossalErr,
> {
    let mut working_set = StateWorkingSet::new(engine_state);
    let block = parse(&mut working_set, None, source.as_bytes(), false);

    if let Some(err) = working_set.parse_errors.first() {
        return Err(ColossalErr::Io(std::io::Error::other(err.to_string())));
    }
    if let Some(err) = working_set.compile_errors.first() {
        return Err(ColossalErr::Io(std::io::Error::other(err.to_string())));
    }

    Ok((block, working_set.render()))
}

fn split_top_level_segments(source: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut paren_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut backtick_quoted = false;
    let mut escaped = false;
    let mut in_comment = false;

    for ch in source.chars() {
        // Comments run to end-of-line.
        if in_comment {
            if ch == '\n' {
                in_comment = false;
            }
            continue;
        }

        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        let in_string = single_quoted || double_quoted || backtick_quoted;
        let nested = paren_depth > 0 || brace_depth > 0 || bracket_depth > 0;

        match ch {
            '\\' if double_quoted || backtick_quoted => {
                current.push(ch);
                escaped = true;
            }
            '#' if !in_string && !nested => {
                // Top-level comment — skip rest of line.
                in_comment = true;
            }
            '\'' if !double_quoted && !backtick_quoted => {
                single_quoted = !single_quoted;
                current.push(ch);
            }
            '"' if !single_quoted && !backtick_quoted => {
                double_quoted = !double_quoted;
                current.push(ch);
            }
            '`' if !single_quoted && !double_quoted => {
                backtick_quoted = !backtick_quoted;
                current.push(ch);
            }
            '{' if !in_string => {
                brace_depth += 1;
                current.push(ch);
            }
            '}' if !in_string => {
                brace_depth = brace_depth.saturating_sub(1);
                current.push(ch);
            }
            '(' if !in_string => {
                paren_depth += 1;
                current.push(ch);
            }
            ')' if !in_string => {
                paren_depth = paren_depth.saturating_sub(1);
                current.push(ch);
            }
            '[' if !in_string => {
                bracket_depth += 1;
                current.push(ch);
            }
            ']' if !in_string => {
                bracket_depth = bracket_depth.saturating_sub(1);
                current.push(ch);
            }
            ';' if !in_string && !nested => {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    segments.push(trimmed.to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    let trimmed = current.trim();
    if !trimmed.is_empty() {
        segments.push(trimmed.to_string());
    }

    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_simple_semicolons() {
        assert_eq!(split_top_level_segments("a; b; c"), vec!["a", "b", "c"]);
    }

    #[test]
    fn split_preserves_braces() {
        // Semicolons inside braces are not split points
        assert_eq!(
            split_top_level_segments("def x [] { \"a;b\" }; y"),
            vec!["def x [] { \"a;b\" }", "y"]
        );
    }

    #[test]
    fn split_preserves_nested_braces() {
        assert_eq!(
            split_top_level_segments("if true { cd foo; pwd }; echo done"),
            vec!["if true { cd foo; pwd }", "echo done"]
        );
    }

    #[test]
    fn split_preserves_single_quotes() {
        assert_eq!(
            split_top_level_segments("echo 'a;b'; echo c"),
            vec!["echo 'a;b'", "echo c"]
        );
    }

    #[test]
    fn split_preserves_double_quotes() {
        assert_eq!(
            split_top_level_segments(r#"echo "a;b"; echo c"#),
            vec![r#"echo "a;b""#, "echo c"]
        );
    }

    #[test]
    fn split_preserves_parens() {
        assert_eq!(split_top_level_segments("(1; 2); 3"), vec!["(1; 2)", "3"]);
    }

    #[test]
    fn split_preserves_brackets() {
        assert_eq!(split_top_level_segments("[a; b]; c"), vec!["[a; b]", "c"]);
    }

    #[test]
    fn split_handles_comments() {
        // Comment strips to end-of-line, but "a" and "b" are in the same
        // segment since newlines alone don't split (multiline support).
        assert_eq!(split_top_level_segments("a # comment\nb"), vec!["a b"]);
        // With a semicolon before the comment, they split properly.
        assert_eq!(split_top_level_segments("a; # comment\nb"), vec!["a", "b"]);
    }

    #[test]
    fn split_comment_does_not_affect_strings() {
        // # inside a string is not a comment
        assert_eq!(
            split_top_level_segments("echo \"#not a comment\"; next"),
            vec!["echo \"#not a comment\"", "next"]
        );
    }

    #[test]
    fn split_escaped_quote_in_double_string() {
        assert_eq!(
            split_top_level_segments(r#"echo "a\"b;c"; d"#),
            vec![r#"echo "a\"b;c""#, "d"]
        );
    }

    #[test]
    fn split_empty_input() {
        assert!(split_top_level_segments("").is_empty());
        assert!(split_top_level_segments("   ").is_empty());
        assert!(split_top_level_segments(";;;").is_empty());
    }

    #[test]
    fn split_multiline_def() {
        let input = "def greet [name: string] {\n  echo $name\n}; greet hi";
        assert_eq!(
            split_top_level_segments(input),
            vec!["def greet [name: string] {\n  echo $name\n}", "greet hi"]
        );
    }

    #[test]
    fn split_backtick_quoted() {
        assert_eq!(split_top_level_segments("`a;b`; c"), vec!["`a;b`", "c"]);
    }
}

/// Embedded Nushell runtime that owns all shell state in-process.
///
/// # Managed state contract
///
/// The following state categories are first-class: they persist across
/// `exec_command` calls, survive `snapshot`/`restore` cycles, and are
/// carried through agent-core session rotations (policy changes).
///
/// | Category        | Mutated by                              | Stored in              |
/// |-----------------|-----------------------------------------|------------------------|
/// | Environment     | `load-env`, `$env.X = val`, `hide-env`  | `stack` env vars       |
/// | Working dir     | `cd`, `update_cwd`                      | `current_cwd` field    |
/// | Custom commands | `def`, `export def`                     | `custom_commands` list |
/// | Aliases         | `alias`, `export alias`                 | `aliases` list         |
/// | Session vars    | top-level `let`, `mut`, assignment      | `variables` list       |
/// | Config          | `$env.config = ...`, sub-field writes   | `stack.config` / `nu_config` in snapshot |
///
/// # Config mutations (`$env.config`)
///
/// `$env.config` mutations are **fully supported** and **structurally
/// persistent**.  The runtime calls `Stack::update_config` after every
/// evaluation so config changes take effect immediately.  At snapshot time the
/// current `Stack::config` override is serialized as JSON into the
/// `nu_config` field of `PersistentSessionState`.  At restore time it is
/// deserialized back into the new stack, so the config survives rotation.
///
/// If the snapshot's `nu_config` JSON is missing or unparseable (e.g., an
/// older snapshot format), the runtime silently falls back to the Nu default
/// config.  The next `$env.config` mutation in the new session will overwrite
/// it as usual.
///
/// Both sub-field writes (`$env.config.show_banner = false`) and whole-config
/// replacement (`$env.config = {table: {mode: "light"}}`) are handled
/// correctly.  `fork_eval` respects the live config within the fork, and the
/// forked config is discarded with the fork (consistent with all other
/// fork-local mutations).
///
/// # External commands (`^cmd`, `run-external`)
///
/// External commands are **fully supported**.  The runtime registers a
/// `run-external` implementation (`ManagedExternalCommand`) that spawns child
/// processes using the Nu stack's environment variables and the current working
/// directory.  Nu's external-call syntax (`^cmd args`, `run-external cmd args`)
/// resolves through this implementation exactly as it would in a real Nu shell.
///
/// **Sandbox enforcement** is at the OS level (landlock/seccomp on Linux,
/// Seatbelt on macOS), not inside this runtime.  The policy decides what
/// processes may and may not do.  The managed runtime does not pre-filter
/// external commands.
///
/// **Persistence semantics**: external process side effects (filesystem
/// changes, spawned sub-processes, network activity) are governed by sandbox
/// policy and are not shell state.  They do not affect the snapshot fields.
/// After an external command `sync_cwd_from_stack` runs to catch the rare
/// case where a command wires `$env.PWD = (^some-cmd)`.
///
/// **Limitations**:
/// - Interactive programs that require a TTY will not work (the runtime runs
///   without a PTY).
/// - Commands that read stdin will see an empty pipe.
/// - `run-external` only captures stdout and stderr as a combined string; it
///   does not stream output.  Long-running or high-output commands should be
///   run through the agent-core PTY path instead.
///
/// # What does NOT survive rotation
///
/// - `overlay` subcommands are rejected: overlay state lives in `EngineState`'s
///   active overlay stack and cannot be snapshot/restored without re-executing
///   file-backed module sources.
/// - `module` / `use` / `source` / `source-env` / `export use` / `export
///   module` / `export extern` / `export const` are rejected.  These forms
///   load state into `EngineState` overlays with no structural extraction API.
///   Use `def` / `alias` directly.
/// - Block-local or def-local `let`/`mut` bindings are not persisted; only
///   top-level session variables are captured.
///
/// # `export def` / `export alias`
///
/// These are treated identically to `def` / `alias` at the top level and are
/// part of the persistence contract.
///
/// # Background / interactive / PTY
///
/// The managed runtime runs commands synchronously in-process without a PTY.
/// Interactive programs that require terminal input will not work.  Background
/// execution is handled at the agent-core layer, not here.
///
/// # replay_state (agent-core concept)
///
/// At the agent-core layer, `replay_state: true` on `exec_command` tells the
/// harness that the command intentionally mutates shell state that later
/// commands must observe (cd, export, def, alias). For managed nu, the
/// runtime captures this structurally — `replay_state` controls whether the
/// continuity snapshot is updated, not the mechanism of persistence.
/// `replay_state: false` commands run in isolation and do not affect the
/// continuity state that survives session rotation.
pub struct ManagedNuRuntime {
    engine_state: EngineState,
    stack: Stack,
    initial_cwd: PathBuf,
    current_cwd: PathBuf,
    shell_path: String,
    sandbox_policy: SandboxPolicy,
    history: Vec<String>,
    custom_commands: Vec<String>,
    aliases: Vec<crate::manager::NuAliasState>,
    variables: Vec<NuVariableState>,
    replay_commands: Vec<NuReplayCommandState>,
}

impl std::fmt::Debug for ManagedNuRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedNuRuntime")
            .field("initial_cwd", &self.initial_cwd)
            .field("current_cwd", &self.current_cwd)
            .field("shell_path", &self.shell_path)
            .field("sandbox_policy", &self.sandbox_policy)
            .field("history_len", &self.history.len())
            .field("custom_commands_len", &self.custom_commands.len())
            .field("aliases_len", &self.aliases.len())
            .field("variables_len", &self.variables.len())
            .field("replay_commands_len", &self.replay_commands.len())
            .finish()
    }
}

impl ManagedNuRuntime {
    pub fn new(
        shell_path: String,
        initial_cwd: PathBuf,
        sandbox_policy: SandboxPolicy,
    ) -> Result<Self, ColossalErr> {
        let mut engine_state = add_shell_command_context(create_default_context());
        engine_state.generate_nu_constant();

        // Register the managed `run-external` implementation so that external
        // command syntax (`^cmd`, `run-external cmd`) resolves correctly.
        let mut working_set = StateWorkingSet::new(&engine_state);
        working_set.add_decl(Box::new(ManagedExternalCommand));
        let delta = working_set.render();
        engine_state.merge_delta(delta).map_err(shell_error_to_io)?;

        let mut stack = Stack::new().collect_value();
        for (key, value) in std::env::vars() {
            stack.add_env_var(key, Value::string(value, Span::unknown()));
        }
        stack.add_env_var(
            "PWD".to_string(),
            Value::string(initial_cwd.to_string_lossy(), Span::unknown()),
        );

        let current_cwd = initial_cwd.clone();

        Ok(Self {
            engine_state,
            stack,
            initial_cwd,
            current_cwd,
            shell_path,
            sandbox_policy,
            history: Vec::new(),
            custom_commands: Vec::new(),
            aliases: Vec::new(),
            variables: Vec::new(),
            replay_commands: Vec::new(),
        })
    }

    pub fn from_snapshot(
        shell_path: String,
        sandbox_policy: SandboxPolicy,
        snapshot: &PersistentSessionState,
    ) -> Result<Self, ColossalErr> {
        let mut runtime = Self::new(shell_path, snapshot.initial_cwd.clone(), sandbox_policy)?;
        runtime.restore(snapshot)?;
        Ok(runtime)
    }

    pub fn shell_path(&self) -> &str {
        &self.shell_path
    }

    pub fn sandbox_policy(&self) -> &SandboxPolicy {
        &self.sandbox_policy
    }

    pub fn initial_cwd(&self) -> PathBuf {
        self.initial_cwd.clone()
    }

    pub fn history(&self) -> Vec<String> {
        self.history.clone()
    }

    fn extract_def_name(source: &str) -> Option<&str> {
        let rest = source
            .strip_prefix("export def ")
            .or_else(|| source.strip_prefix("def "))?;
        rest.split_whitespace()
            .find(|token| !token.starts_with('-'))
    }

    fn extract_alias_parts(source: &str) -> Option<(&str, &str)> {
        let rest = source
            .strip_prefix("export alias ")
            .or_else(|| source.strip_prefix("alias "))?;
        let (name_part, expansion) = rest.split_once('=')?;
        Some((name_part.trim(), expansion.trim()))
    }

    fn register_def(&mut self, source: &str) {
        if let Some(name) = Self::extract_def_name(source) {
            let name = name.to_string();
            self.custom_commands
                .retain(|existing| Self::extract_def_name(existing) != Some(name.as_str()));
            self.custom_commands.push(source.to_string());
        }
    }

    fn register_alias(&mut self, name: String, expansion: String) {
        self.aliases.retain(|existing| existing.name != name);
        self.aliases
            .push(crate::manager::NuAliasState { name, expansion });
    }

    fn is_persistent_variable_name(name: &str) -> bool {
        !matches!(name, "$env" | "$nu" | "$in")
    }

    fn is_replay_journal_segment(trimmed: &str) -> bool {
        trimmed.starts_with("overlay ")
            || trimmed.starts_with("module ")
            || trimmed.starts_with("use ")
            || trimmed.starts_with("source ")
            || trimmed.starts_with("source-env ")
            || trimmed.starts_with("export use ")
            || trimmed.starts_with("export module ")
            || trimmed.starts_with("export extern ")
            || trimmed.starts_with("export const ")
            || trimmed.starts_with("const ")
            || trimmed == "hide-env PWD"
            || trimmed == "hide-env 'PWD'"
            || trimmed == "hide-env \"PWD\""
    }

    fn replay_dependency_candidates(trimmed: &str) -> Vec<PathBuf> {
        let Some(tokens) = shlex::split(trimmed) else {
            return Vec::new();
        };
        let Some(first) = tokens.first().map(String::as_str) else {
            return Vec::new();
        };

        let candidate = match first {
            "source" | "source-env" | "use" => tokens.get(1),
            "export" => match tokens.get(1).map(String::as_str) {
                Some("use") => tokens.get(2),
                _ => None,
            },
            "overlay" => match tokens.get(1).map(String::as_str) {
                Some("use") => tokens.get(2),
                _ => None,
            },
            _ => None,
        };

        candidate
            .filter(|path| path.contains('/') || path.ends_with(".nu") || path.starts_with('.'))
            .map(|path| vec![PathBuf::from(path)])
            .unwrap_or_default()
    }

    fn resolve_replay_file(&self, path: &PathBuf) -> PathBuf {
        if path.is_absolute() {
            path.clone()
        } else {
            self.current_cwd().join(path)
        }
    }

    fn record_replay_segment(&mut self, trimmed: &str) -> Result<(), ColossalErr> {
        if !Self::is_replay_journal_segment(trimmed) {
            return Ok(());
        }

        let files = Self::replay_dependency_candidates(trimmed)
            .into_iter()
            .map(|candidate| {
                let path = self.resolve_replay_file(&candidate);
                let bytes = fs::read(&path).map_err(|err| {
                    ColossalErr::Io(std::io::Error::other(format!(
                        "failed to read managed Nu replay dependency {}: {}",
                        path.display(),
                        err
                    )))
                })?;
                let sha256 = format!("{:x}", Sha256::digest(bytes));
                Ok(NuReplayFileState { path, sha256 })
            })
            .collect::<Result<Vec<_>, ColossalErr>>()?;

        self.replay_commands.push(NuReplayCommandState {
            command: trimmed.to_string(),
            cwd: self.current_cwd(),
            files,
        });
        Ok(())
    }

    fn sync_persistent_variables_from_runtime(&mut self) {
        let mut variables_by_name = HashMap::new();
        for overlay in self.engine_state.active_overlays(&[]) {
            for (name_bytes, var_id) in &overlay.vars {
                let Ok(name) = String::from_utf8(name_bytes.clone()) else {
                    continue;
                };
                if !Self::is_persistent_variable_name(&name) {
                    continue;
                }

                let variable = self.engine_state.get_var(*var_id);
                if variable.const_val.is_some() {
                    continue;
                }

                let Ok(value) = self.stack.get_var(*var_id, Span::unknown()) else {
                    continue;
                };

                variables_by_name.insert(
                    name.clone(),
                    NuVariableState {
                        name,
                        mutable: variable.mutable,
                        value,
                    },
                );
            }
        }

        let mut variables: Vec<_> = variables_by_name.into_values().collect();
        variables.sort_by(|a, b| a.name.cmp(&b.name));
        self.variables = variables;
    }

    pub(crate) fn restore_persistent_variable(
        &mut self,
        variable: &NuVariableState,
    ) -> Result<(), ColossalErr> {
        let mut working_set = StateWorkingSet::new(&self.engine_state);
        let var_id = working_set.add_variable(
            variable.name.as_bytes().to_vec(),
            Span::unknown(),
            variable.value.get_type(),
            variable.mutable,
        );
        self.engine_state
            .merge_delta(working_set.render())
            .map_err(shell_error_to_io)?;
        self.stack.add_var(var_id, variable.value.clone());
        Ok(())
    }

    pub fn current_cwd(&self) -> PathBuf {
        self.current_cwd.clone()
    }

    pub fn get_env(&self, key: &str) -> Option<String> {
        self.stack
            .get_env_var(&self.engine_state, key)
            .and_then(|value| value.coerce_str().ok().map(|cow| cow.into_owned()))
    }

    pub fn set_env(&mut self, key: String, value: String) {
        self.stack
            .add_env_var(key, Value::string(value, Span::unknown()));
    }

    pub fn unset_env(&mut self, key: &str) {
        let _ = self.stack.remove_env_var(&self.engine_state, key);
    }

    pub fn update_cwd(&mut self, cwd: PathBuf) -> Result<(), ColossalErr> {
        self.stack.add_env_var(
            "PWD".to_string(),
            Value::string(cwd.to_string_lossy(), Span::unknown()),
        );
        self.current_cwd = cwd;
        Ok(())
    }

    fn eval_string(&mut self, source: &str) -> Result<String, ColossalErr> {
        let (block, delta) = parse_block(&self.engine_state, source)?;
        self.engine_state
            .merge_delta(delta)
            .map_err(shell_error_to_io)?;
        let result = eval_block::<WithoutDebug>(
            &self.engine_state,
            &mut self.stack,
            &block,
            PipelineData::empty(),
        )
        .map_err(shell_error_to_io)?;

        // Pick up any $env.config mutations the block may have made.  Nu stores
        // config in Stack::config, and update_config() syncs it from $env.config.
        // Errors are soft (config values that didn't parse are skipped); we
        // ignore warnings here since this is a library context without a REPL.
        let _ = self.stack.update_config(&self.engine_state);

        result
            .body
            .collect_string("", self.stack.get_config(&self.engine_state).as_ref())
            .map_err(shell_error_to_io)
    }

    fn eval_value(&mut self, source: &str) -> Result<Value, ColossalErr> {
        let (block, delta) = parse_block(&self.engine_state, source)?;
        self.engine_state
            .merge_delta(delta)
            .map_err(shell_error_to_io)?;
        let result = eval_block::<WithoutDebug>(
            &self.engine_state,
            &mut self.stack,
            &block,
            PipelineData::empty(),
        )
        .map_err(shell_error_to_io)?;

        // Propagate any config changes, same as eval_string.
        let _ = self.stack.update_config(&self.engine_state);

        result
            .body
            .into_value(Span::unknown())
            .map_err(shell_error_to_io)
    }

    fn resolve_cd_target(&self, path_str: &str) -> PathBuf {
        if path_str.is_empty() || path_str == "~" {
            return PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".to_string()));
        }
        if let Some(rest) = path_str.strip_prefix("~/") {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
            return PathBuf::from(home).join(rest);
        }
        let target = PathBuf::from(path_str);
        if target.is_absolute() {
            target
        } else {
            self.current_cwd().join(target)
        }
    }

    fn run_segment(&mut self, segment: &str) -> Result<String, ColossalErr> {
        let trimmed = segment.trim();
        if trimmed.is_empty() {
            return Ok(String::new());
        }

        if trimmed == "pwd" {
            return Ok(self.current_cwd().display().to_string());
        }

        if trimmed == "cd" || trimmed.starts_with("cd ") {
            let path_str = trimmed.strip_prefix("cd").unwrap_or("").trim();
            // Strip surrounding quotes that the caller or agent may include
            let path_str = path_str
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .or_else(|| {
                    path_str
                        .strip_prefix('\'')
                        .and_then(|s| s.strip_suffix('\''))
                })
                .unwrap_or(path_str);
            let resolved = self.resolve_cd_target(path_str);
            let resolved = std::fs::canonicalize(&resolved).map_err(|err| {
                ColossalErr::Io(std::io::Error::other(format!(
                    "cd failed for {}: {}",
                    resolved.display(),
                    err
                )))
            })?;
            self.update_cwd(resolved)?;
            return Ok(String::new());
        }

        if let Some(expr) = trimmed.strip_prefix("load-env ") {
            let value = self.eval_value(expr.trim())?;
            if let Value::Record { val, .. } = value {
                for (key, value) in val.into_owned() {
                    if key == "PWD" {
                        // Sync the current_cwd tracker when PWD is set via load-env
                        if let Ok(s) = value.coerce_str() {
                            let path = PathBuf::from(s.as_ref());
                            if path.is_dir() {
                                self.current_cwd = std::fs::canonicalize(&path).unwrap_or(path);
                            }
                        }
                    }
                    self.stack.add_env_var(key, value);
                }
                return Ok(String::new());
            }
            return Err(ColossalErr::Io(std::io::Error::other(
                "load-env requires a record value",
            )));
        }

        if let Some(var_name) = trimmed.strip_prefix("hide-env ") {
            let var_name = var_name.trim();
            // Strip surrounding quotes from the variable name
            let var_name = var_name
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .or_else(|| {
                    var_name
                        .strip_prefix('\'')
                        .and_then(|s| s.strip_suffix('\''))
                })
                .unwrap_or(var_name);
            self.unset_env(var_name);
            if var_name == "PWD" {
                self.record_replay_segment(trimmed)?;
            }
            return Ok(String::new());
        }

        if trimmed.starts_with("def ") || trimmed.starts_with("export def ") {
            let output = self.eval_string(trimmed)?;
            self.register_def(trimmed);
            return Ok(output);
        }

        if trimmed.starts_with("alias ") || trimmed.starts_with("export alias ") {
            if let Some((name, expansion)) = Self::extract_alias_parts(trimmed) {
                let output = self.eval_string(trimmed)?;
                self.register_alias(name.to_string(), expansion.to_string());
                return Ok(output);
            }
        }

        let output = self.eval_string(trimmed)?;
        self.record_replay_segment(trimmed)?;
        // After any general eval, sync current_cwd from the stack's PWD
        // in case the command mutated $env.PWD directly (via $env.config or
        // an external command that wrote to PWD).
        self.sync_cwd_from_stack();
        self.sync_persistent_variables_from_runtime();
        Ok(output)
    }

    /// If the stack's PWD differs from our tracked current_cwd, adopt the
    /// stack's value.  This catches `$env.PWD = ...` mutations that bypass
    /// our `cd` interceptor.
    fn sync_cwd_from_stack(&mut self) {
        if let Some(pwd_val) = self.stack.get_env_var(&self.engine_state, "PWD") {
            if let Ok(s) = pwd_val.coerce_str() {
                let path = PathBuf::from(s.as_ref());
                if path.is_dir() && path != self.current_cwd {
                    self.current_cwd = std::fs::canonicalize(&path).unwrap_or(path);
                }
            }
        }
    }

    pub fn exec_command(&mut self, command: String) -> Result<ExecCommandOutput, ColossalErr> {
        let start = Instant::now();
        self.history.push(command.clone());
        // Cap history to prevent unbounded growth in long-lived sessions
        const MAX_HISTORY: usize = 1000;
        if self.history.len() > MAX_HISTORY {
            self.history.drain(..self.history.len() - MAX_HISTORY);
        }

        let segments = split_top_level_segments(&command);

        let mut collected = String::new();
        for segment in segments {
            match self.run_segment(&segment) {
                Ok(output) => collected.push_str(&output),
                Err(err) => {
                    let err_msg = err.to_string();
                    let mut aggregated = collected.clone();
                    if !aggregated.is_empty() {
                        aggregated.push('\n');
                    }
                    aggregated.push_str(&err_msg);
                    return Ok(ExecCommandOutput {
                        duration: start.elapsed(),
                        exit_status: ExitStatus::Completed { code: 1 },
                        stdout: collected,
                        stderr: err_msg,
                        aggregated_output: aggregated,
                        log_file: None,
                    });
                }
            }
        }

        Ok(ExecCommandOutput {
            duration: start.elapsed(),
            exit_status: ExitStatus::Completed { code: 0 },
            stdout: collected.clone(),
            stderr: String::new(),
            aggregated_output: collected,
            log_file: None,
        })
    }

    pub fn snapshot(
        &mut self,
        session_id: SessionId,
        created_at: std::time::SystemTime,
    ) -> Result<PersistentSessionState, ColossalErr> {
        self.sync_persistent_variables_from_runtime();
        let structured_env: HashMap<String, Value> = self
            .stack
            .get_env_vars(&self.engine_state)
            .into_iter()
            .filter(|(key, _)| key != "config")
            .collect();
        let env_vars = structured_env
            .iter()
            .filter_map(|(key, value)| {
                value
                    .coerce_str()
                    .ok()
                    .map(|value| (key.clone(), value.into_owned()))
            })
            .collect();
        let structured_env_json = serde_json::to_string(&structured_env).ok();

        // Serialize the current config only if it has been mutated (i.e. the
        // stack has its own config override rather than deferring to the engine
        // state default).
        let nu_config = self
            .stack
            .config
            .as_ref()
            .and_then(|cfg| serde_json::to_string(cfg.as_ref()).ok());

        Ok(PersistentSessionState {
            version: crate::manager::SNAPSHOT_FORMAT_VERSION,
            session_id,
            shell_path: self.shell_path.clone(),
            initial_cwd: self.initial_cwd(),
            env_vars,
            current_cwd: self.current_cwd(),
            created_at,
            structured_env_json,
            nu_aliases: self.aliases.clone(),
            nu_custom_commands: self.custom_commands.clone(),
            nu_variables: self.variables.clone(),
            nu_replay_commands: self.replay_commands.clone(),
            replay_commands: Vec::new(),
            nu_config,
        })
    }

    /// Execute a command against a cloned copy of this runtime's engine state
    /// and stack.  The clone is discarded after evaluation so the original
    /// runtime is untouched.  This replaces the command-synthesis approach
    /// (`managed_nu_seeded_command`) used by `agent_core` for isolated
    /// (non-replay-state) commands.
    pub fn fork_eval(&self, command: String) -> Result<ExecCommandOutput, ColossalErr> {
        let start = std::time::Instant::now();
        let mut forked_engine = self.engine_state.clone();
        let mut forked_stack = self.stack.clone();

        // Ensure PWD is set correctly in the forked stack
        forked_stack.add_env_var(
            "PWD".to_string(),
            Value::string(self.current_cwd.to_string_lossy(), Span::unknown()),
        );

        let segments = split_top_level_segments(&command);

        let result = segments.into_iter().try_fold(
            String::new(),
            |mut collected, segment| -> Result<String, ColossalErr> {
                let trimmed = segment.trim();
                if trimmed.is_empty() {
                    return Ok(collected);
                }

                if trimmed == "pwd" {
                    let cwd = forked_engine
                        .cwd(Some(&forked_stack))
                        .map(|path| path.into_std_path_buf())
                        .unwrap_or_else(|_| self.current_cwd.clone());
                    collected.push_str(&cwd.display().to_string());
                    return Ok(collected);
                }

                let (block, delta) = parse_block(&forked_engine, trimmed)?;
                forked_engine
                    .merge_delta(delta)
                    .map_err(shell_error_to_io)?;
                let eval_result = eval_block::<WithoutDebug>(
                    &forked_engine,
                    &mut forked_stack,
                    &block,
                    PipelineData::empty(),
                )
                .map_err(shell_error_to_io)?;

                // Propagate any $env.config mutations into the forked stack's
                // config so subsequent expressions in the same fork_eval see
                // the updated config.  The forked stack is discarded at the end
                // of fork_eval; this does not affect the live runtime.
                let _ = forked_stack.update_config(&forked_engine);

                let output = eval_result
                    .body
                    .collect_string("", forked_stack.get_config(&forked_engine).as_ref())
                    .map_err(shell_error_to_io)?;
                collected.push_str(&output);
                Ok(collected)
            },
        );

        match result {
            Ok(collected) => Ok(ExecCommandOutput {
                duration: start.elapsed(),
                exit_status: ExitStatus::Completed { code: 0 },
                stdout: collected.clone(),
                stderr: String::new(),
                aggregated_output: collected,
                log_file: None,
            }),
            Err(err) => Ok(ExecCommandOutput {
                duration: start.elapsed(),
                exit_status: ExitStatus::Completed { code: 1 },
                stdout: String::new(),
                stderr: err.to_string(),
                aggregated_output: err.to_string(),
                log_file: None,
            }),
        }
    }

    pub fn restore(&mut self, snapshot: &PersistentSessionState) -> Result<(), ColossalErr> {
        if let Some(structured_env_json) = &snapshot.structured_env_json {
            if let Ok(structured_env) =
                serde_json::from_str::<HashMap<String, Value>>(structured_env_json)
            {
                for (key, value) in structured_env {
                    self.stack.add_env_var(key, value);
                }
            }
        } else {
            for (key, value) in &snapshot.env_vars {
                self.set_env(key.clone(), value.clone());
            }
        }
        self.update_cwd(snapshot.current_cwd.clone())?;

        for variable in &snapshot.nu_variables {
            self.restore_persistent_variable(variable)?;
        }

        for source in &snapshot.nu_custom_commands {
            self.eval_string(source)?;
            self.register_def(source);
        }
        for alias in &snapshot.nu_aliases {
            let source = format!("alias {} = {}", alias.name, alias.expansion);
            self.eval_string(&source)?;
            self.register_alias(alias.name.clone(), alias.expansion.clone());
        }

        for replay in &snapshot.nu_replay_commands {
            for file in &replay.files {
                let bytes = fs::read(&file.path).map_err(|err| {
                    ColossalErr::Io(std::io::Error::other(format!(
                        "failed to read managed Nu replay dependency {} during restore: {}",
                        file.path.display(),
                        err
                    )))
                })?;
                let current_hash = format!("{:x}", Sha256::digest(bytes));
                if current_hash != file.sha256 {
                    return Err(ColossalErr::Io(std::io::Error::other(format!(
                        "managed Nu replay dependency changed since snapshot: {}",
                        file.path.display()
                    ))));
                }
            }

            let original_cwd = self.current_cwd();
            self.update_cwd(replay.cwd.clone())?;
            let _ = self.run_segment(&replay.command)?;
            self.update_cwd(original_cwd)?;
        }

        self.variables = snapshot.nu_variables.clone();
        self.replay_commands = snapshot.nu_replay_commands.clone();

        // Restore config if the snapshot carries a serialized override.
        // A missing or malformed nu_config JSON is treated as "use default"
        // rather than an error so that snapshots from older versions or
        // partial-write failures degrade gracefully.
        if let Some(cfg_json) = &snapshot.nu_config {
            if let Ok(cfg) = serde_json::from_str::<nu_protocol::Config>(cfg_json) {
                self.stack.config = Some(Arc::new(cfg.clone()));
                // Set the config env var from the restored Config so $env.config
                // works during subsequent commands. Use Config::into_value which is the
                // standard Nu way to convert Config -> Value.
                let cfg_value = cfg.into_value(Span::unknown());
                self.stack.add_env_var("config".to_string(), cfg_value);
            }
            // If deserialization fails we log nothing and use the default —
            // the agent's next $env.config mutation will overwrite it anyway.
        }

        Ok(())
    }
}
