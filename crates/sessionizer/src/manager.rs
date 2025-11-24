use crate::error::{ColossalErr, SandboxErr};
use crate::safety::assess_command_safety;
use crate::session::{ExecCommandSession, PersistentShellSession, SemanticSearchSession};
use crate::types::{ExecCommandOutput, ExecCommandParams, ExitStatus, SessionId, StreamEvent, WriteStdinParams};
use crate::utils::truncate_output;
use anyhow::Result;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::select;
use async_channel::{unbounded, Receiver};
use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::sync::Arc;
use qdrant_client::Qdrant;
use std::path::PathBuf;
use crate::hash_id::hash_id_exists;

/// Clean shell output by removing prompts and command echoes
fn clean_shell_output(output: &str, command: &str) -> String {
    // First, remove ANSI escape codes
    let ansi_regex = regex::Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]|\x1b\][^\x07]*\x07|\x1b\][^\x1b]*\x1b\\|\x1bk[^\x1b]*\x1b\\").unwrap();
    let without_ansi = ansi_regex.replace_all(output, "");

    // Also remove other control characters except newlines
    let control_regex = regex::Regex::new(r"[\x00-\x08\x0B-\x1F\x7F]").unwrap();
    let clean_text = control_regex.replace_all(&without_ansi, "");

    // Remove any lines containing the command marker pattern
    // This handles the echoed command line that includes __CMD_DONE___
    let marker_regex = regex::Regex::new(r".*__CMD_DONE_.*").unwrap();
    let without_markers = marker_regex.replace_all(&clean_text, "");

    let lines: Vec<&str> = without_markers.lines().collect();
    let mut cleaned_lines = Vec::new();
    let mut skip_next = false;

    for (i, line) in lines.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }

        let trimmed = line.trim();

        // Skip empty lines
        if trimmed.is_empty() {
            continue;
        }

        // Skip shell prompts (various patterns)
        if trimmed.ends_with('$') || trimmed.ends_with('#') || trimmed.ends_with('>') || trimmed.ends_with('%') {
            continue;
        }

        // Skip lines that look like shell prompts
        if (trimmed.contains('@') || trimmed.contains('~')) &&
           (trimmed.ends_with('$') || trimmed.ends_with('#') || trimmed.ends_with('%')) {
            continue;
        }

        // Skip continuation prompts
        if trimmed == ">" || trimmed == ">>" {
            continue;
        }

        // Skip lines that match the exact command (first line is often the echo)
        if i == 0 && trimmed == command.trim() {
            continue;
        }

        cleaned_lines.push(trimmed);
    }

    cleaned_lines.join("\n")
}

// Data structure for persisting session state
#[derive(Debug, Serialize, Deserialize)]
struct PersistentSessionState {
    session_id: SessionId,
    shell_path: String,
    initial_cwd: std::path::PathBuf,
    env_vars: std::collections::HashMap<String, String>,
    current_cwd: std::path::PathBuf,
    created_at: std::time::SystemTime,
}

// Metadata for session lifecycle management
#[derive(Debug)]
struct SessionMetadata {
    created_at: Instant,
    created_at_system_time: std::time::SystemTime,
    last_activity: Instant,
    timeout_duration: Duration,
}

#[derive(Debug)]
pub struct SessionManager {
    next_session_id: AtomicU32,
    process_id: u32, // Store PID for unique session IDs across processes
    pub sessions: Mutex<Vec<(SessionId, ExecCommandSession)>>,
    pub persistent_shell_sessions: Mutex<Vec<(SessionId, PersistentShellSession)>>,
    pub semantic_search_sessions: Mutex<Vec<(SessionId, SemanticSearchSession)>>,
    session_metadata: Mutex<HashMap<SessionId, SessionMetadata>>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self {
            next_session_id: AtomicU32::new(0),
            process_id: std::process::id(),
            sessions: Mutex::new(Vec::new()),
            persistent_shell_sessions: Mutex::new(Vec::new()),
            semantic_search_sessions: Mutex::new(Vec::new()),
            session_metadata: Mutex::new(HashMap::new()),
        }
    }
}

impl SessionManager {
    /// Generate a globally unique session ID across all process instances
    /// Format: {pid}_{counter} where pid ensures uniqueness across processes
    fn generate_unique_session_id(&self) -> SessionId {
        let counter = self.next_session_id.fetch_add(1, Ordering::SeqCst);
        SessionId(format!("{}_{}", self.process_id, counter))
    }

    /// Create a session metadata entry
    fn create_session_metadata(&self, session_id: SessionId, timeout_duration: Duration) {
        let metadata = SessionMetadata {
            created_at: Instant::now(),
            created_at_system_time: std::time::SystemTime::now(),
            last_activity: Instant::now(),
            timeout_duration,
        };
        self.session_metadata.lock().unwrap().insert(session_id, metadata);
    }

    /// Update last activity time for a session
    fn update_session_activity(&self, session_id: SessionId) {
        if let Some(metadata) = self.session_metadata.lock().unwrap().get_mut(&session_id) {
            metadata.last_activity = Instant::now();
        }
    }

    /// Check if a session has timed out
    fn is_session_timed_out(&self, session_id: SessionId) -> bool {
        if let Some(metadata) = self.session_metadata.lock().unwrap().get(&session_id) {
            Instant::now().duration_since(metadata.last_activity) > metadata.timeout_duration
        } else {
            // If we don't have metadata, assume it's not timed out
            false
        }
    }

    /// Get session age
    pub fn get_session_age(&self, session_id: SessionId) -> Option<Duration> {
        self.session_metadata.lock().unwrap().get(&session_id)
            .map(|metadata| Instant::now().duration_since(metadata.created_at))
    }

    /// Get time since last activity
    pub fn get_time_since_last_activity(&self, session_id: SessionId) -> Option<Duration> {
        self.session_metadata.lock().unwrap().get(&session_id)
            .map(|metadata| Instant::now().duration_since(metadata.last_activity))
    }

    /// Cleanup timed out sessions
    pub async fn cleanup_timed_out_sessions(&self) -> Result<Vec<SessionId>, ColossalErr> {
        let mut timed_out_sessions = Vec::new();
        
        // Collect timed out session IDs
        {
            let metadata_map = self.session_metadata.lock().unwrap();
            for (session_id, _) in metadata_map.iter() {
                if self.is_session_timed_out(session_id.clone()) {
                    timed_out_sessions.push(session_id.clone());
                }
            }
        }
        
        // Terminate timed out sessions
        for session_id in timed_out_sessions.iter() {
            if let Err(_e) = self.terminate_session(session_id.clone()).await {
                // eprintln!("Failed to terminate timed out session {}: {}", session_id.as_str(), e);
            }
        }
        
        Ok(timed_out_sessions)
    }

    /// Save session state to disk
    pub fn save_session_state(&self, filepath: &str) -> Result<(), ColossalErr> {
        let mut persistent_sessions = Vec::new();
        
        // Collect state from persistent shell sessions
        {
            let sessions = self.persistent_shell_sessions.lock().unwrap();
            let metadata_map = self.session_metadata.lock().unwrap();
            for (session_id, session) in sessions.iter() {
                // Get the actual creation time from session metadata
                let created_at = if let Some(metadata) = metadata_map.get(session_id) {
                    metadata.created_at_system_time
                } else {
                    // Fallback to current time if metadata not found
                    std::time::SystemTime::now()
                };
                
                let state = PersistentSessionState {
                    session_id: session_id.clone(),
                    shell_path: session.shell_path().to_string(),
                    initial_cwd: session.initial_cwd().clone(),
                    env_vars: session.get_all_env(),
                    current_cwd: session.current_cwd(),
                    created_at, // Use the actual creation time
                };
                persistent_sessions.push(state);
            }
        }
        
        // Save to file
        let file = File::create(filepath)
            .map_err(|e| ColossalErr::Io(e))?;
        let writer = BufWriter::new(file);
        serde_json::to_writer(writer, &persistent_sessions)
            .map_err(|e| ColossalErr::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
        
        Ok(())
    }

    /// Load session state from disk
    pub async fn load_session_state(&self, filepath: &str, sandbox_policy: crate::protocol::SandboxPolicy) -> Result<Vec<SessionId>, ColossalErr> {
        // Load from file
        let file = File::open(filepath)
            .map_err(|e| ColossalErr::Io(e))?;
        let reader = BufReader::new(file);
        let persistent_sessions: Vec<PersistentSessionState> = serde_json::from_reader(reader)
            .map_err(|e| ColossalErr::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
        
        let mut session_ids = Vec::new();
        
        // Recreate sessions
        for state in persistent_sessions {
            let (session, _exit_rx) = crate::session::create_persistent_shell_session(
                state.shell_path.clone(),
                false, // login shell
                sandbox_policy.clone(),
                state.initial_cwd.clone(),
            ).await?;
            
            // Restore environment variables
            for (key, value) in state.env_vars.iter() {
                session.set_env(key.clone(), value.clone());
            }
            
            // Restore current working directory
            session.update_cwd(state.current_cwd.clone());
            
            self.persistent_shell_sessions.lock().unwrap().push((state.session_id.clone(), session));
            session_ids.push(state.session_id.clone());
            
            // Restore metadata
            // Note: We're using a default timeout since we don't persist the original timeout
            self.create_session_metadata(state.session_id.clone(), Duration::from_secs(1800)); // 30 minutes default
        }
        
        Ok(session_ids)
    }
    
    pub async fn handle_exec_command_request(
        &self,
        params: ExecCommandParams,
    ) -> Result<ExecCommandOutput, ColossalErr> {
        // eprintln!("Starting handle_exec_command_request with command: {:?}", params.command);
        let approved_commands: HashSet<Vec<String>> = HashSet::new();
        let safety_check = assess_command_safety(
            &params.command,
            params.ask_for_approval.unwrap_or(crate::safety::yolo_mode()),
            &params.sandbox_policy,
            &approved_commands,
            false,
        );
        let _sandbox_type = match safety_check {
            crate::safety::SafetyCheck::AutoApprove { sandbox_type } => Some(sandbox_type),
            crate::safety::SafetyCheck::AskUser => return Err(ColossalErr::Sandbox(SandboxErr::Denied(-1, "User approval required".to_string(), ()))), 
            crate::safety::SafetyCheck::Reject { reason } => return Err(ColossalErr::Sandbox(SandboxErr::Denied(-1, reason, ()))), 
        };
        // eprintln!("Sandbox type: {:?}", sandbox_type);
        let session_id = self.generate_unique_session_id();
        
        // Format the command as a string
        let formatted_command = params.shell.format_default_shell_invocation(params.command.clone(), false)
            .ok_or_else(|| ColossalErr::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Failed to format command for shell",
            )))?;
        let command_str = shlex::try_join(formatted_command.iter().map(|s| s.as_str()))
            .map_err(|_| ColossalErr::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Failed to join command arguments",
            )))?;
            
        let (session, exit_rx) = crate::session::create_sandboxed_exec_session(
            command_str,
            params.shell.path().to_string_lossy().to_string(),
            false, // login shell
            params.sandbox_policy.clone(),
            params.cwd.clone(),
        ).await?;
        
        self.sessions.lock().unwrap().push((session_id.clone(), session));
        
        // Create session metadata with timeout (None for background processes)
        let timeout_duration = params.timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_secs(600)); // 10 minutes default for foreground
        self.create_session_metadata(session_id.clone(), timeout_duration);
        let mut output_rx1 = {
            let sessions = self.sessions.lock().unwrap();
            let session = sessions.iter().find(|(id, _)| *id == session_id)
                .map(|(_, session)| session)
                .ok_or_else(|| ColossalErr::Sandbox(SandboxErr::Denied(-1, format!("unknown session id {}", session_id.as_str()), ())))?;
            session.output_receiver()
        };
        
        // Handle background vs foreground execution differently
        if params.is_background {
            // Background execution: spawn a task to write output to log file
            let log_file_path = std::path::PathBuf::from(format!("/tmp/shell_{}.log", session_id.as_str()));
            let log_file_path_clone = log_file_path.clone();
            let session_id_clone = session_id.clone();

            tokio::spawn(async move {
                let file = tokio::fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&log_file_path_clone)
                    .await;

                if let Ok(mut file) = file {
                    use tokio::io::AsyncWriteExt;
                    let mut output_rx = output_rx1;
                    loop {
                        match output_rx.recv().await {
                            Ok(chunk) => {
                                if let Err(_) = file.write_all(&chunk).await {
                                    break;
                                }
                                let _ = file.flush().await;
                            }
                            Err(_) => break,
                        }
                    }
                }
            });

            // Return immediately with Ongoing status
            Ok(ExecCommandOutput {
                duration: Duration::from_secs(0),
                exit_status: ExitStatus::Ongoing(session_id),
                stdout: String::new(),
                stderr: String::new(),
                aggregated_output: format!("Background process started. Session ID: {}. Log file: {}", session_id_clone.as_str(), log_file_path.display()),
                log_file: Some(log_file_path),
            })
        } else {
            // Foreground execution: original behavior
            let cap_bytes = params.max_output_tokens.saturating_mul(4) as usize;
            let mut stdout_buf = Vec::with_capacity(8192.min(cap_bytes));
            let stderr_buf = Vec::with_capacity(8192.min(cap_bytes));
            let mut aggregated_buf = Vec::with_capacity(8192.min(cap_bytes));
            let start_time = Instant::now();
            let deadline = start_time + Duration::from_millis(params.timeout_ms.unwrap_or(10000));
            let mut exit_code: Option<i32> = None;
            let mut exit_future = Box::pin(exit_rx);
            loop {
                if Instant::now() >= deadline {
                    // eprintln!("Command timed out after {}ms", params.timeout_ms.unwrap_or(10000));
                    break;
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                select! {
                    biased;
                    exit_result = &mut *exit_future => {
                        match exit_result {
                            Ok(code) => {
                                // eprintln!("Process exited with code: {}", code);
                                exit_code = Some(code);
                            }
                            Err(_) => {
                                // eprintln!("Exit channel closed unexpectedly");
                                exit_code = Some(-1);
                            }
                        }
                        let grace_deadline = Instant::now() + Duration::from_millis(100);
                        while Instant::now() < grace_deadline {
                            match output_rx1.try_recv() {
                                Ok(chunk) => {
                                    stdout_buf.extend_from_slice(&chunk);
                                    aggregated_buf.extend_from_slice(&chunk);
                                }
                                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                                Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
                                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                            }
                        }
                        break;
                    }
                    chunk = tokio::time::timeout(remaining, output_rx1.recv()) => {
                        if let Ok(Ok(chunk)) = chunk {
                            stdout_buf.extend_from_slice(&chunk);
                            aggregated_buf.extend_from_slice(&chunk);
                        }
                    }
                }
            }
            let stdout = String::from_utf8_lossy(&truncate_output(stdout_buf, cap_bytes).text).to_string();
            let stderr = String::from_utf8_lossy(&truncate_output(stderr_buf, cap_bytes).text).to_string();
            let aggregated_output = String::from_utf8_lossy(&truncate_output(aggregated_buf, cap_bytes).text).to_string();
            let exit_status = match exit_code {
                Some(code) => ExitStatus::Completed { code },
                None => {
                    if Instant::now() >= deadline {
                        ExitStatus::Timeout
                    } else {
                        ExitStatus::Killed
                    }
                }
            };
            // eprintln!("Command completed with status: {:?}", exit_status);
            Ok(ExecCommandOutput {
                duration: Instant::now().duration_since(start_time),
                exit_status,
                stdout,
                stderr,
                aggregated_output,
                log_file: None,
            })
        }
    }

    /// Read output from a background process log file
    pub async fn read_background_output(
        &self,
        session_id: SessionId,
    ) -> Result<String, ColossalErr> {
        let log_file_path = std::path::PathBuf::from(format!("/tmp/shell_{}.log", session_id.as_str()));

        // Check if log file exists
        if !log_file_path.exists() {
            return Err(ColossalErr::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Log file not found for session {}", session_id.as_str()),
            )));
        }

        // Read the log file
        let contents = tokio::fs::read_to_string(&log_file_path)
            .await
            .map_err(|e| ColossalErr::Io(e))?;

        Ok(contents)
    }

    pub async fn handle_write_stdin_request(
        &self,
        params: WriteStdinParams,
    ) -> Result<ExecCommandOutput, ColossalErr> {
        // eprintln!("Starting handle_write_stdin_request for session: {}", params.session_id.as_str());
        let (writer_tx, mut output_rx) = {
            let sessions = self.sessions.lock().unwrap();
            let session = sessions
                .iter()
                .find(|(id, _)| *id == params.session_id)
                .map(|(_, session)| session)
                .ok_or_else(|| ColossalErr::Sandbox(SandboxErr::Denied(-1, format!("unknown session id {}", params.session_id.as_str()), ())))?;
            (session.writer_sender(), session.output_receiver())
        };
        if !params.chars.is_empty() {
            // eprintln!("Writing to stdin: {}", params.chars);
            writer_tx.send(params.chars.into_bytes())
                .await
                .map_err(|_| ColossalErr::Io(std::io::Error::new(std::io::ErrorKind::Other, "failed to write to stdin")))?;
        }
        let cap_bytes = params.max_output_tokens.saturating_mul(4) as usize;
        let mut stdout_buf = Vec::with_capacity(8192.min(cap_bytes));
        let mut stderr_buf = Vec::with_capacity(8192.min(cap_bytes));
        let mut aggregated_buf = Vec::with_capacity(8192.min(cap_bytes));
        let start_time = Instant::now();
        let deadline = start_time + Duration::from_millis(params.yield_time_ms);
        loop {
            let now = Instant::now();
            if now >= deadline {
                // eprintln!("Write stdin timed out after {}ms", params.yield_time_ms);
                break;
            }
            let remaining = deadline.saturating_duration_since(now);
            select! {
                chunk = tokio::time::timeout(remaining, output_rx.recv()) => {
                    if let Ok(Ok(chunk)) = chunk {
                        stdout_buf.extend_from_slice(&chunk);
                        aggregated_buf.extend_from_slice(&chunk);
                    }
                }
            }
        }
        let stdout = String::from_utf8_lossy(&truncate_output(stdout_buf, cap_bytes).text).to_string();
        let stderr = String::from_utf8_lossy(&truncate_output(stderr_buf, cap_bytes).text).to_string();
        let aggregated_output = String::from_utf8_lossy(&truncate_output(aggregated_buf, cap_bytes).text).to_string();
        // eprintln!("Write stdin completed");
        Ok(ExecCommandOutput {
            duration: Instant::now().duration_since(start_time),
            exit_status: ExitStatus::Ongoing(params.session_id),
            stdout,
            stderr,
            aggregated_output,
            log_file: None,
        })
    }

    pub async fn stream_exec_command(
        &self,
        params: ExecCommandParams,
    ) -> Result<(SessionId, Receiver<StreamEvent>), ColossalErr> {
        // eprintln!("Starting stream_exec_command with command: {:?}", params.command);
        let approved_commands: HashSet<Vec<String>> = HashSet::new();
        let safety_check = assess_command_safety(
            &params.command,
            crate::safety::yolo_mode(),
            &params.sandbox_policy,
            &approved_commands,
            false,
        );
        let _sandbox_type = match safety_check {
            crate::safety::SafetyCheck::AutoApprove { sandbox_type } => Some(sandbox_type),
            crate::safety::SafetyCheck::AskUser => return Err(ColossalErr::Sandbox(SandboxErr::Denied(-1, "User approval required".to_string(), ()))), 
            crate::safety::SafetyCheck::Reject { reason } => return Err(ColossalErr::Sandbox(SandboxErr::Denied(-1, reason, ()))), 
        };
        // eprintln!("Sandbox type: {:?}", sandbox_type);
        let session_id = self.generate_unique_session_id();
        
        // Format the command as a string
        let formatted_command = params.shell.format_default_shell_invocation(params.command.clone(), true)
            .ok_or_else(|| ColossalErr::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Failed to format command for shell",
            )))?;
        let command_str = shlex::try_join(formatted_command.iter().map(|s| s.as_str()))
            .map_err(|_| ColossalErr::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Failed to join command arguments",
            )))?;
            
        let (session, exit_rx) = crate::session::create_sandboxed_exec_session(
            command_str,
            params.shell.path().to_string_lossy().to_string(),
            false, // login shell
            params.sandbox_policy.clone(),
            params.cwd.clone(),
        ).await?;
        
        // eprintln!("Session created: {}", session_id.as_str());
        self.sessions.lock().unwrap().push((session_id.clone(), session));
        let output_rx = {
            let sessions = self.sessions.lock().unwrap();
            let session = sessions.iter().find(|(id, _)| *id == session_id)
                .map(|(_, session)| session)
                .ok_or_else(|| ColossalErr::Sandbox(SandboxErr::Denied(-1, format!("unknown session id {}", session_id.as_str()), ())))?;
            session.output_receiver()
        };
        let (tx, rx) = unbounded::<StreamEvent>();
        
        // Spawn the streaming task
        tokio::spawn(async move {
            let mut output_rx = output_rx;
            let mut exit_future = Box::pin(exit_rx);
            let process_exited = false;
            loop {
                select! {
                    biased;
                    exit_result = &mut *exit_future, if !process_exited => {
                        match exit_result {
                            Ok(code) => {
                                // eprintln!("Process exited with code: {}", code);
                                if tx.send(StreamEvent::Exit(code)).await.is_err() {
                                    // eprintln!("Failed to send exit event for session {}", session_id.as_str());
                                    break;
                                }
                                let grace_deadline = Instant::now() + Duration::from_millis(200);
                                while Instant::now() < grace_deadline {
                                    match output_rx.try_recv() {
                                        Ok(chunk) => {
                                            let raw_output = String::from_utf8_lossy(&chunk);
                                            
                                            // Filter out PTY debug messages (more comprehensive filtering)
                                            if raw_output.contains("read") && raw_output.contains("bytes from pty") {
                                                continue; // Skip PTY debug messages
                                            }
                                            if raw_output.contains("read ") && raw_output.contains(" bytes from pty") {
                                                continue; // Skip PTY debug messages with different spacing
                                            }
                                            if raw_output.trim().chars().all(|c| c.is_digit(10) || c.is_whitespace()) && !raw_output.trim().is_empty() {
                                                continue; // Skip pure numeric output (likely debug fragments)
                                            }
                                            
                                            // Only process non-empty output
                                            if raw_output.trim().is_empty() {
                                                continue;
                                            }
                                            
                                            let output = format!("STDOUT: {}", raw_output);
                                            if tx.send(StreamEvent::Stdout(output)).await.is_err() {
                                                // eprintln!("Failed to send output event after exit");
                                                break;
                                            }
                                        }
                                        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                                        Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
                                        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                                    }
                                    tokio::time::sleep(Duration::from_millis(10)).await;
                                }
                                break;
                            }
                            Err(_) => {
                                // eprintln!("Exit channel closed unexpectedly for session {}", session_id.as_str());
                                if tx.send(StreamEvent::Error("Process exited unexpectedly".to_string())).await.is_err() {
                                    // eprintln!("Failed to send error event for session {}", session_id.as_str());
                                }
                                break;
                            }
                        }
                    }
                    output_result = output_rx.recv(), if !process_exited => {
                        match output_result {
                            Ok(chunk) => {
                                let raw_output = String::from_utf8_lossy(&chunk);
                                
                                // Filter out PTY debug messages (more comprehensive filtering)
                                if raw_output.contains("read") && raw_output.contains("bytes from pty") {
                                    continue; // Skip PTY debug messages
                                }
                                if raw_output.contains("read ") && raw_output.contains(" bytes from pty") {
                                    continue; // Skip PTY debug messages with different spacing
                                }
                                if raw_output.trim().chars().all(|c| c.is_digit(10) || c.is_whitespace()) && !raw_output.trim().is_empty() {
                                    continue; // Skip pure numeric output (likely debug fragments)
                                }
                                
                                // Only process non-empty output
                                if raw_output.trim().is_empty() {
                                    continue;
                                }
                                
                                let output = format!("STDOUT: {}", raw_output);
                                if tx.send(StreamEvent::Stdout(output)).await.is_err() {
                                    // eprintln!("Failed to send output event for session {}", session_id.as_str());
                                    break;
                                }
                            }
                            Err(_) => {
                                // eprintln!("Output channel closed for session {}", session_id.as_str());
                            }
                        }
                    }
                    else => {
                        // eprintln!("Streaming task for session {} completed: all branches exhausted", session_id.as_str());
                        break;
                    }
                }
            }
            // eprintln!("Streaming task for session {} terminated", session_id.as_str());
        });
        Ok((session_id, rx))
    }

    /// Create a persistent shell session that can accept multiple commands
    pub async fn create_persistent_shell_session(
        &self,
        shell: String,
        login: bool,
        sandbox_policy: crate::protocol::SandboxPolicy,
        shared_state: Arc<crate::session::SharedSessionState>,
        timeout_duration: Option<Duration>,
    ) -> Result<SessionId, ColossalErr> {
        let session_id = self.generate_unique_session_id();
        
        let (session, _exit_rx) = crate::session::create_persistent_shell_session(
            shell,
            login,
            sandbox_policy,
            shared_state.get_cwd(), // Pass the cwd from shared state
        ).await?;
        
        self.persistent_shell_sessions.lock().unwrap().push((session_id.clone(), session));
        
        // Create session metadata with a default timeout of 30 minutes for persistent sessions
        let timeout = timeout_duration.unwrap_or(Duration::from_secs(1800)); // 30 minutes default
        self.create_session_metadata(session_id.clone(), timeout);
        
        Ok(session_id)
    }

    /// Send a command to a persistent shell session
    pub async fn send_command_to_shell_session(
        &self,
        session_id: SessionId,
        command: String,
    ) -> Result<Receiver<StreamEvent>, ColossalErr> {
        // Add command to history
        {
            let sessions = self.persistent_shell_sessions.lock().unwrap();
            if let Some((_, session)) = sessions.iter().find(|(id, _)| *id == session_id) {
                session.add_to_history(command.clone());
            }
        }
        
        self.send_input_to_shell_session(session_id, format!("{}\n", command), None).await
    }

    /// Execute a command in a persistent shell session and return aggregated output (non-streaming)
    pub async fn exec_command_in_shell_session(
        &self,
        session_id: SessionId,
        command: String,
        timeout_ms: Option<u64>,
        max_output_tokens: u32,
        ask_for_approval: Option<crate::safety::AskForApproval>,
    ) -> Result<ExecCommandOutput, ColossalErr> {
        let start_time = Instant::now();

        // Check safety before executing
        {
            let sessions = self.persistent_shell_sessions.lock().unwrap();
            let session = sessions.iter().find(|(id, _)| *id == session_id)
                .map(|(_, session)| session)
                .ok_or_else(|| ColossalErr::Sandbox(crate::error::SandboxErr::Denied(-1, format!("unknown session id {}", session_id.as_str()), ())))?;
            
            let approved_commands: HashSet<Vec<String>> = HashSet::new();
            let command_parts = shlex::split(&command).unwrap_or_else(|| vec![command.clone()]);
            let safety_check = assess_command_safety(
                &command_parts,
                ask_for_approval.unwrap_or(crate::safety::yolo_mode()),
                session.sandbox_policy(),
                &approved_commands,
                true, // is_pty
            );
            
            match safety_check {
                crate::safety::SafetyCheck::AutoApprove { .. } => {},
                crate::safety::SafetyCheck::AskUser => return Err(ColossalErr::Sandbox(SandboxErr::Denied(-1, "User approval required".to_string(), ()))),
                crate::safety::SafetyCheck::Reject { reason } => return Err(ColossalErr::Sandbox(SandboxErr::Denied(-1, reason, ()))),
            }
        }

        // Wait for shell to be ready before sending commands
        {
            let sessions = self.persistent_shell_sessions.lock().unwrap();
            let session = sessions.iter().find(|(id, _)| *id == session_id)
                .map(|(_, session)| session)
                .ok_or_else(|| ColossalErr::Sandbox(crate::error::SandboxErr::Denied(-1, format!("unknown session id {}", session_id.as_str()), ())))?;

            // Wait up to 10 seconds for shell to be ready
            session.wait_until_ready(Duration::from_secs(10)).await?;
        }

        // First, send Ctrl+C to clear any potentially hung state in the shell
        let _ = self.send_input_to_shell_session(session_id.clone(), "\x03".to_string(), None).await;
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Generate a unique marker to detect command completion
        let marker = format!("__CMD_DONE_{}__", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos());

        // Send the command followed by an echo of the marker
        let command_with_marker = format!("{}; echo '{}'", command, marker);
        let stream_rx = self.send_command_to_shell_session(session_id, command_with_marker).await?;

        let mut stdout_parts = Vec::new();
        let timeout_duration = timeout_ms.map(Duration::from_millis).unwrap_or(Duration::from_secs(30));
        let deadline = start_time + timeout_duration;
        let mut command_completed = false;

        // Collect all streaming output until we see the marker
        while let Ok(event) = tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            stream_rx.recv()
        ).await {
            match event {
                Ok(StreamEvent::Stdout(output)) => {
                    // Check if the output contains the completion marker
                    if output.contains(&marker) {
                        command_completed = true;
                        // Remove the marker from the output
                        let cleaned = output.replace(&marker, "");
                        if !cleaned.trim().is_empty() {
                            stdout_parts.push(cleaned);
                        }
                        break;
                    }
                    stdout_parts.push(output);
                }
                Ok(StreamEvent::Stderr(output)) => {
                    stdout_parts.push(output);
                }
                Ok(StreamEvent::Exit(_)) => break,
                Ok(StreamEvent::Error(_)) => break,
                Err(_) => break, // Channel closed
            }
        }

        let aggregated_output = stdout_parts.join("");
        let cleaned_output = clean_shell_output(&aggregated_output, &command);

        Ok(ExecCommandOutput {
            duration: Instant::now().duration_since(start_time),
            exit_status: if command_completed || Instant::now() < deadline {
                ExitStatus::Completed { code: 0 }
            } else {
                ExitStatus::Timeout
            },
            stdout: cleaned_output.clone(),
            stderr: String::new(),
            aggregated_output: cleaned_output,
            log_file: None,
        })
    }

    /// Send raw input to a persistent shell session (without automatic newline)
    pub async fn send_input_to_shell_session(
        &self,
        session_id: SessionId,
        input: String,
        ask_for_approval: Option<crate::safety::AskForApproval>,
    ) -> Result<Receiver<StreamEvent>, ColossalErr> {
        // Check safety if approval strategy is provided
        if let Some(approval_strategy) = ask_for_approval {
            let sessions = self.persistent_shell_sessions.lock().unwrap();
            let session = sessions.iter().find(|(id, _)| *id == session_id)
                .map(|(_, session)| session)
                .ok_or_else(|| ColossalErr::Sandbox(SandboxErr::Denied(-1, format!("unknown session id {}", session_id.as_str()), ())))?;
            
            let approved_commands: HashSet<Vec<String>> = HashSet::new();
            // Split input into potential command parts
            // Input might be "ls -la\n" or just "ls -la".
            let command_parts = shlex::split(&input).unwrap_or_else(|| vec![input.clone()]);
            
            // Only check if it looks like a command (not just control chars)
            if !input.trim().is_empty() && !input.chars().all(|c| c.is_control()) {
                let safety_check = assess_command_safety(
                    &command_parts,
                    approval_strategy,
                    session.sandbox_policy(),
                    &approved_commands,
                    true, // is_pty
                );
                
                match safety_check {
                    crate::safety::SafetyCheck::AutoApprove { .. } => {},
                    crate::safety::SafetyCheck::AskUser => return Err(ColossalErr::Sandbox(SandboxErr::Denied(-1, "User approval required".to_string(), ()))),
                    crate::safety::SafetyCheck::Reject { reason } => return Err(ColossalErr::Sandbox(SandboxErr::Denied(-1, reason, ()))),
                }
            }
        }

        let (writer_tx, output_rx) = {
            let sessions = self.persistent_shell_sessions.lock().unwrap();
            let session = sessions.iter().find(|(id, _)| *id == session_id)
                .map(|(_, session)| session)
                .ok_or_else(|| ColossalErr::Sandbox(SandboxErr::Denied(-1, format!("unknown session id {}", session_id.as_str()), ())))?;
            (session.writer_sender(), session.output_receiver())
        };
        
        // Update session activity
        self.update_session_activity(session_id);
        
        // Send the input with newline to execute it
        let input_with_newline = format!("{}\n", input);
        writer_tx.send(input_with_newline.into_bytes())
            .await
            .map_err(|_| ColossalErr::Io(std::io::Error::new(std::io::ErrorKind::Other, "failed to send input to shell")))?;
        
        // Create a receiver for the output
        let (tx, rx) = unbounded::<StreamEvent>();
        
        // Spawn a task to forward output to the receiver
        tokio::spawn(async move {
            let mut output_rx = output_rx;
            loop {
                match output_rx.recv().await {
                    Ok(chunk) => {
                        let raw_output = String::from_utf8_lossy(&chunk);
                        
                        // Filter out PTY debug messages (more comprehensive filtering)
                        if raw_output.contains("read") && raw_output.contains("bytes from pty") {
                            continue; // Skip PTY debug messages
                        }
                        if raw_output.contains("read ") && raw_output.contains(" bytes from pty") {
                            continue; // Skip PTY debug messages with different spacing
                        }
                        if raw_output.trim().chars().all(|c| c.is_digit(10) || c.is_whitespace()) && !raw_output.trim().is_empty() {
                            continue; // Skip pure numeric output (likely debug fragments)
                        }
                        
                        // Only process non-empty output
                        if raw_output.trim().is_empty() {
                            continue;
                        }

                        let event = StreamEvent::Stdout(raw_output.to_string());
                        if tx.send(event).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        
        Ok(rx)
    }

    /// Set an environment variable in a persistent shell session
    pub async fn set_env_in_shell_session(
        &self,
        session_id: SessionId,
        key: String,
        value: String,
    ) -> Result<(), ColossalErr> {
        // Store in the session's environment map
        {
            let sessions = self.persistent_shell_sessions.lock().unwrap();
            let session = sessions.iter().find(|(id, _)| *id == session_id)
                .map(|(_, session)| session)
                .ok_or_else(|| ColossalErr::Sandbox(SandboxErr::Denied(-1, format!("unknown session id {}", session_id.as_str()), ())))?;
            
            session.set_env(key.clone(), value.clone());
        }
        
        // Also send the export command to the shell
        let export_command = format!("export {}='{}'", key, value);
        let _ = self.exec_command_in_shell_session(
            session_id,
            export_command,
            Some(2000), // 2 second timeout
            100, // small output limit
            None,
        ).await;
        
        Ok(())
    }

    /// Get an environment variable from a persistent shell session
    pub fn get_env_from_shell_session(
        &self,
        session_id: SessionId,
        key: &str,
    ) -> Result<Option<String>, ColossalErr> {
        let sessions = self.persistent_shell_sessions.lock().unwrap();
        let session = sessions.iter().find(|(id, _)| *id == session_id)
            .map(|(_, session)| session)
            .ok_or_else(|| ColossalErr::Sandbox(SandboxErr::Denied(-1, format!("unknown session id {}", session_id.as_str()), ())))?;
        
        Ok(session.get_env(key))
    }

    /// Update the current working directory of a persistent shell session
    pub fn update_cwd_in_shell_session(
        &self,
        session_id: SessionId,
        new_cwd: std::path::PathBuf,
    ) -> Result<(), ColossalErr> {
        let sessions = self.persistent_shell_sessions.lock().unwrap();
        let session = sessions.iter().find(|(id, _)| *id == session_id)
            .map(|(_, session)| session)
            .ok_or_else(|| ColossalErr::Sandbox(SandboxErr::Denied(-1, format!("unknown session id {}", session_id.as_str()), ())))?;
        
        session.update_cwd(new_cwd);
        Ok(())
    }

    /// Get command history from a persistent shell session
    pub fn get_shell_session_history(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<String>, ColossalErr> {
        let sessions = self.persistent_shell_sessions.lock().unwrap();
        let session = sessions.iter().find(|(id, _)| *id == session_id)
            .map(|(_, session)| session)
            .ok_or_else(|| ColossalErr::Sandbox(SandboxErr::Denied(-1, format!("unknown session id {}", session_id.as_str()), ())))?;
        
        Ok(session.get_history())
    }

    /// Create a semantic search session that indexes files in the specified directory
    pub async fn create_semantic_search_session(
        &self,
        cwd: PathBuf,
        sandbox_policy: crate::protocol::SandboxPolicy,
        timeout_duration: Option<Duration>,
    ) -> Result<SessionId, ColossalErr> {
        // Generate a hash-based ID for the session
        let hash_id = crate::hash_id::generate_hash_id_from_path(&cwd);
        
        // Check for collisions - if a session with this hash already exists, generate a new one
        let mut session_id = SessionId::new(hash_id.clone());
        let mut attempts = 0;
        let max_attempts = 10;
        
        let semantic_sessions = self.semantic_search_sessions.lock().unwrap();
        while hash_id_exists(semantic_sessions.as_slice(), session_id.as_str()) && attempts < max_attempts {
            // Generate a new hash ID with a random suffix
            let new_hash_id = format!("{}_{}", hash_id, attempts);
            session_id = SessionId::new(new_hash_id);
            attempts += 1;
        }
        drop(semantic_sessions); // Release the lock
        
        // If we've exhausted our attempts, return an error
        if attempts >= max_attempts {
            return Err(ColossalErr::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "Unable to generate unique session ID after maximum attempts",
            )));
        }
        
        // Create Qdrant client
        let client = Arc::new(Qdrant::from_url("http://localhost:6334").build()
            .map_err(|e| ColossalErr::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?);
        
        // Create a unique collection name for this session using the hash ID
        let collection_name = format!("session_{}", session_id.as_str());
        
        // Create the collection in Qdrant
        let _ = client.create_collection(
            qdrant_client::qdrant::CreateCollectionBuilder::new(&collection_name)
                .vectors_config(qdrant_client::qdrant::VectorParamsBuilder::new(768, qdrant_client::qdrant::Distance::Cosine))
                .quantization_config(qdrant_client::qdrant::ScalarQuantizationBuilder::default())
        ).await;
        
        // Create indexing status
        let status = Arc::new(std::sync::RwLock::new(crate::semantic_search_lib::IngestStatus {
            state: "initializing".to_string(),
            total: 0,
            ingested: 0,
            progress_percent: 0.0,
        }));
        
        // Start indexing in a separate task
        let client_clone = client.clone();
        let status_clone = status.clone();
        let cwd_clone = cwd.clone();
        let collection_name_clone = collection_name.clone();
        let sandbox_policy_clone = sandbox_policy.clone();

        let indexing_handle = tokio::spawn(async move {
            // IMPORTANT: Apply sandbox to this spawned task's thread
            // This sandbox policy will apply to:
            // 1. The indexing task itself
            // 2. Any file watcher event processing (if enabled)
            // 3. All file I/O operations performed by this task
            // The sandbox is NOT inherited - it must be explicitly applied
            if let Err(e) = crate::landlock::apply_sandbox_policy_to_current_thread(&sandbox_policy_clone, &cwd_clone) {
                let mut s = status_clone.write().unwrap();
                s.state = format!("sandbox error: {}", e);
                s.progress_percent = 0.0;
                return;
            }

            if let Err(e) = crate::semantic_search_lib::index_codebase(
                cwd_clone.to_str().unwrap(),
                &client_clone,
                &collection_name_clone,
                status_clone.clone(),
            ).await {
                let mut s = status_clone.write().unwrap();
                s.state = format!("error: {}", e);
                s.progress_percent = 0.0;
            }
        });
        
        let session = SemanticSearchSession::new(
            client,
            cwd.clone(),
            status,
            Some(indexing_handle),
            collection_name,
            sandbox_policy,
        )?;

        self.semantic_search_sessions.lock().unwrap().push((session_id.clone(), session));
        
        // Create session metadata with a default timeout
        let timeout = timeout_duration.unwrap_or(Duration::from_secs(1800)); // 30 minutes default
        self.create_session_metadata(session_id.clone(), timeout);
        
        Ok(session_id)
    }

    /// Get the status of a semantic search session
    pub fn get_semantic_search_session_status(
        &self,
        session_id: SessionId,
    ) -> Result<crate::semantic_search_lib::IngestStatus, ColossalErr> {
        let sessions = self.semantic_search_sessions.lock().unwrap();
        let session = sessions.iter().find(|(id, _)| *id == session_id)
            .map(|(_, session)| session)
            .ok_or_else(|| ColossalErr::Sandbox(SandboxErr::Denied(-1, format!("unknown session id {}", session_id.as_str()), ())))?;
        
        Ok(session.get_status())
    }

    /// Search for code chunks using semantic search in a session
    pub async fn search_in_semantic_search_session(
        &self,
        session_id: SessionId,
        query: &str,
        limit: u64,
    ) -> Result<Vec<(f32, qdrant_client::Payload)>, ColossalErr> {
        let sessions = self.semantic_search_sessions.lock().unwrap();
        let session = sessions.iter().find(|(id, _)| *id == session_id)
            .map(|(_, session)| session)
            .ok_or_else(|| ColossalErr::Sandbox(SandboxErr::Denied(-1, format!("unknown session id {}", session_id.as_str()), ())))?;
        
        // Call the search method on the session
        session.search(query, limit)
            .await
            .map_err(|e| {
                // Convert the error to a string to avoid Send/Sync issues
                let error_string = format!("{}", e);
                ColossalErr::Io(std::io::Error::new(std::io::ErrorKind::Other, error_string))
            })
    }

    /// Search for code chunks and return formatted results
    pub async fn search_and_format_results(
        &self,
        session_id: SessionId,
        query: &str,
        limit: u64,
    ) -> Result<String, ColossalErr> {
        let raw_results = self.search_in_semantic_search_session(session_id, query, limit).await?;
        
        // Convert raw results to our SearchResult format
        let mut search_results: Vec<crate::search_results::SearchResult> = Vec::new();
        
        for (score, payload) in raw_results {
            // Convert Payload to serde_json::Value to access fields
            let json_value: serde_json::Value = payload.into();
            
            if let (Some(file_name), Some(kind), Some(start_byte), Some(end_byte), Some(source_code)) = (
                json_value.get("file_name").and_then(|v| v.as_str()).map(|s| s.to_string()),
                json_value.get("kind").and_then(|v| v.as_str()).map(|s| s.to_string()),
                json_value.get("start_byte").and_then(|v| v.as_u64()),
                json_value.get("end_byte").and_then(|v| v.as_u64()),
                json_value.get("source_code").and_then(|v| v.as_str()).map(|s| s.to_string()),
            ) {
                search_results.push(crate::search_results::SearchResult::new(
                    score,
                    std::path::PathBuf::from(file_name),
                    kind,
                    start_byte,
                    end_byte,
                    source_code,
                ));
            }
        }
        
        // Create SearchResults object and sort by score
        let mut results = crate::search_results::SearchResults::new(query.to_string(), search_results);
        results.sort_by_score();
        
        // Return formatted results
        Ok(results.format())
    }

    /// List all active session IDs with detailed information
    pub fn list_sessions_detailed(&self) -> Vec<(SessionId, String, Duration, Duration)> {
        let sessions = self.sessions.lock().unwrap();
        let persistent_sessions = self.persistent_shell_sessions.lock().unwrap();
        let semantic_search_sessions = self.semantic_search_sessions.lock().unwrap();
        let metadata_map = self.session_metadata.lock().unwrap();
        
        let mut session_info = Vec::new();
        
        // Regular command sessions
        for (session_id, _) in sessions.iter() {
            if let Some(metadata) = metadata_map.get(session_id) {
                let age = Instant::now().duration_since(metadata.created_at);
                let inactive_time = Instant::now().duration_since(metadata.last_activity);
                session_info.push((session_id.clone(), "command".to_string(), age, inactive_time));
            }
        }
        
        // Persistent shell sessions
        for (session_id, session) in persistent_sessions.iter() {
            if let Some(metadata) = metadata_map.get(session_id) {
                let age = Instant::now().duration_since(metadata.created_at);
                let inactive_time = Instant::now().duration_since(metadata.last_activity);
                session_info.push((session_id.clone(), format!("shell:{}", session.shell_path()), age, inactive_time));
            }
        }
        
        // Semantic search sessions
        for (session_id, session) in semantic_search_sessions.iter() {
            if let Some(metadata) = metadata_map.get(session_id) {
                let age = Instant::now().duration_since(metadata.created_at);
                let inactive_time = Instant::now().duration_since(metadata.last_activity);
                session_info.push((session_id.clone(), "semantic-search".to_string(), age, inactive_time));
            }
        }
        
        session_info
    }

    /// Get detailed information about a specific session
    pub fn get_session_info(&self, session_id: SessionId) -> Option<(String, Duration, Duration, Option<std::path::PathBuf>)> {
        let metadata_map = self.session_metadata.lock().unwrap();
        
        // Check regular sessions
        {
            let sessions = self.sessions.lock().unwrap();
            if sessions.iter().any(|(id, _)| *id == session_id) {
                if let Some(metadata) = metadata_map.get(&session_id) {
                    let age = Instant::now().duration_since(metadata.created_at);
                    let inactive_time = Instant::now().duration_since(metadata.last_activity);
                    return Some(("command".to_string(), age, inactive_time, None));
                }
            }
        }
        
        // Check persistent shell sessions
        {
            let sessions = self.persistent_shell_sessions.lock().unwrap();
            if let Some((_, session)) = sessions.iter().find(|(id, _)| *id == session_id) {
                if let Some(metadata) = metadata_map.get(&session_id) {
                    let age = Instant::now().duration_since(metadata.created_at);
                    let inactive_time = Instant::now().duration_since(metadata.last_activity);
                    return Some(("shell".to_string(), age, inactive_time, Some(session.current_cwd())));
                }
            }
        }
        
        // Check semantic search sessions
        {
            let sessions = self.semantic_search_sessions.lock().unwrap();
            if let Some((_, session)) = sessions.iter().find(|(id, _)| *id == session_id) {
                if let Some(metadata) = metadata_map.get(&session_id) {
                    let age = Instant::now().duration_since(metadata.created_at);
                    let inactive_time = Instant::now().duration_since(metadata.last_activity);
                    return Some(("semantic-search".to_string(), age, inactive_time, Some(session.current_cwd())));
                }
            }
        }
        
        None
    }

    /// Terminate a session by ID
    pub async fn terminate_session(&self, session_id: SessionId) -> Result<(), ColossalErr> {
        // Try to find in regular sessions first
        {
            let mut sessions = self.sessions.lock().unwrap();
            if let Some(pos) = sessions.iter().position(|(id, _)| *id == session_id) {
                let (_, session) = sessions.remove(pos);
                // Kill the process
                if let Err(_e) = session.kill() {
                    // eprintln!("Failed to kill process for session {}: {}", session_id.as_str(), e);
                }
                return Ok(());
            }
        }
        
        // Try to find in persistent shell sessions
        {
            let mut sessions = self.persistent_shell_sessions.lock().unwrap();
            if let Some(pos) = sessions.iter().position(|(id, _)| *id == session_id) {
                let (_, session) = sessions.remove(pos);
                // Kill the process
                if let Err(_e) = session.kill() {
                    // eprintln!("Failed to kill process for session {}: {}", session_id.as_str(), e);
                }
                return Ok(());
            }
        }
        
        // Try to find in semantic search sessions
        {
            let mut sessions = self.semantic_search_sessions.lock().unwrap();
            if let Some(pos) = sessions.iter().position(|(id, _)| *id == session_id) {
                let (_, session) = sessions.remove(pos);
                // Kill the semantic search session
                if let Err(_e) = session.kill() {
                    // eprintln!("Failed to kill semantic search session {}: {}", session_id.as_str(), e);
                }
                return Ok(());
            }
        }
        
        Err(ColossalErr::Sandbox(SandboxErr::Denied(-1, format!("unknown session id {}", session_id.as_str()), ())))
    }
    
    pub async fn stream_exec_command_enhanced(
        &self,
        params: ExecCommandParams,
    ) -> Result<(SessionId, Receiver<StreamEvent>), ColossalErr> {
        // eprintln!("Starting enhanced stream_exec_command with command: {:?}", params.command);
        let approved_commands: HashSet<Vec<String>> = HashSet::new();
        let safety_check = assess_command_safety(
            &params.command,
            params.ask_for_approval.unwrap_or(crate::safety::yolo_mode()),
            &params.sandbox_policy,
            &approved_commands,
            false,
        );
        let _sandbox_type = match safety_check {
            crate::safety::SafetyCheck::AutoApprove { sandbox_type } => Some(sandbox_type),
            crate::safety::SafetyCheck::AskUser => return Err(ColossalErr::Sandbox(SandboxErr::Denied(-1, "User approval required".to_string(), ()))), 
            crate::safety::SafetyCheck::Reject { reason } => return Err(ColossalErr::Sandbox(SandboxErr::Denied(-1, reason, ()))), 
        };
        // eprintln!("Sandbox type: {:?}", sandbox_type);
        let session_id = self.generate_unique_session_id();
        
        // Format the command as a string
        let formatted_command = params.shell.format_default_shell_invocation(params.command.clone(), true)
            .ok_or_else(|| ColossalErr::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Failed to format command for shell",
            )))?;
        let command_str = shlex::try_join(formatted_command.iter().map(|s| s.as_str()))
            .map_err(|_| ColossalErr::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Failed to join command arguments",
            )))?;
            
        let (session, exit_rx) = crate::session::create_sandboxed_exec_session(
            command_str,
            params.shell.path().to_string_lossy().to_string(),
            false, // login shell
            params.sandbox_policy.clone(),
            params.cwd.clone(),
        ).await?;
        
        // eprintln!("Session created: {}", session_id.as_str());
        self.sessions.lock().unwrap().push((session_id.clone(), session));
        let output_rx = {
            let sessions = self.sessions.lock().unwrap();
            let session = sessions.iter().find(|(id, _)| *id == session_id)
                .map(|(_, session)| session)
                .ok_or_else(|| ColossalErr::Sandbox(SandboxErr::Denied(-1, format!("unknown session id {}", session_id.as_str()), ())))?;
            session.subscribe_stream() // Use the enhanced subscription method
        };
        let (tx, rx) = unbounded::<StreamEvent>();
        
        // Spawn the streaming task with backpressure handling
        tokio::spawn(async move {
            let mut output_rx = output_rx;
            let mut exit_future = Box::pin(exit_rx);
            let mut buffer = String::new(); // Buffer to handle split messages
            
            loop {
                select! {
                    biased;
                    // Handle process exit
                    exit_result = &mut *exit_future => {
                        match exit_result {
                            Ok(code) => {
                                // eprintln!("Process exited with code: {}", code);
                                if tx.send(StreamEvent::Exit(code)).await.is_err() {
                                    // eprintln!("Failed to send exit event for session {}", session_id.as_str());
                                    break;
                                }
                                
                                // Grace period to collect any remaining output
                                let grace_deadline = Instant::now() + Duration::from_millis(200);
                                while Instant::now() < grace_deadline {
                                    match output_rx.try_recv() {
                                        Ok(chunk) => {
                                            let raw_output = String::from_utf8_lossy(&chunk);
                                            buffer.push_str(&raw_output);
                                            
                                            // Process complete lines from buffer
                                            while let Some(newline_pos) = buffer.find('\n') {
                                                let line = buffer[..newline_pos].to_string();
                                                buffer = buffer[newline_pos + 1..].to_string();
                                                
                                                // Filter out PTY debug messages (more comprehensive filtering)
                                                if line.contains("read") && line.contains("bytes from pty") {
                                                    continue; // Skip PTY debug messages
                                                }
                                                if line.contains("read ") && line.contains(" bytes from pty") {
                                                    continue; // Skip PTY debug messages with different spacing
                                                }
                                                if line.trim().chars().all(|c| c.is_digit(10) || c.is_whitespace()) && !line.trim().is_empty() {
                                                    continue; // Skip pure numeric output (likely debug fragments)
                                                }
                                                
                                                // Only process non-empty output
                                                if line.trim().is_empty() {
                                                    continue;
                                                }
                                                
                                                let output = format!("STDOUT: {}\n", line);
                                                let event = StreamEvent::Stdout(output);
                                                if tx.send(event).await.is_err() {
                                                    // eprintln!("Failed to send output event after exit");
                                                    break;
                                                }
                                            }
                                        }
                                        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                                        Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
                                        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                                    }
                                    tokio::time::sleep(Duration::from_millis(10)).await;
                                }
                                break;
                            }
                            Err(_) => {
                                // eprintln!("Exit channel closed unexpectedly for session {}", session_id.as_str());
                                if tx.send(StreamEvent::Error("Process exited unexpectedly".to_string())).await.is_err() {
                                    // eprintln!("Failed to send error event for session {}", session_id.as_str());
                                }
                                break;
                            }
                        }
                    }
                    // Handle output streaming with backpressure
                    output_result = output_rx.recv() => {
                        match output_result {
                            Ok(chunk) => {
                                let raw_output = String::from_utf8_lossy(&chunk);
                                buffer.push_str(&raw_output);
                                
                                // Process complete lines from buffer
                                while let Some(newline_pos) = buffer.find('\n') {
                                    let line = buffer[..newline_pos].to_string();
                                    buffer = buffer[newline_pos + 1..].to_string();
                                    
                                    // Filter out PTY debug messages (more comprehensive filtering)
                                    if line.contains("read") && line.contains("bytes from pty") {
                                        continue; // Skip PTY debug messages
                                    }
                                    if line.contains("read ") && line.contains(" bytes from pty") {
                                        continue; // Skip PTY debug messages with different spacing
                                    }
                                    if line.trim().chars().all(|c| c.is_digit(10) || c.is_whitespace()) && !line.trim().is_empty() {
                                        continue; // Skip pure numeric output (likely debug fragments)
                                    }
                                    
                                    // Only process non-empty output
                                    if line.trim().is_empty() {
                                        continue;
                                    }
                                    
                                    // Add STDOUT prefix to distinguish output
                                    let output = format!("STDOUT: {}\n", line);
                                    let event = StreamEvent::Stdout(output);
                                    
                                    // Send with timeout to handle backpressure
                                    match tokio::time::timeout(Duration::from_secs(5), tx.send(event)).await {
                                        Ok(Ok(())) => {
                                            // Successfully sent
                                        }
                                        Ok(Err(_)) => {
                                            // eprintln!("Failed to send output event for session {} - receiver dropped", session_id.as_str());
                                            break;
                                        }
                                        Err(_) => {
                                            // eprintln!("Timeout sending output event for session {} - backpressure detected", session_id.as_str());
                                            // Continue processing, don't break - this is backpressure handling
                                        }
                                    }
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                // eprintln!("Output channel closed for session {}", session_id.as_str());
                                break;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_skipped)) => {
                                // eprintln!("Output channel lagged for session {}, skipped {} messages", session_id.as_str(), skipped);
                                // Continue processing, this is expected in high-throughput scenarios
                            }
                        }
                    }
                    else => {
                        // eprintln!("Enhanced streaming task for session {} completed: all branches exhausted", session_id.as_str());
                        break;
                    }
                }
            }
            // eprintln!("Enhanced streaming task for session {} terminated", session_id.as_str());
        });
        Ok((session_id, rx))
    }
}
