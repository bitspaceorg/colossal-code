// Session Management Module
//
// SANDBOX POLICY INHERITANCE RULES:
// ==================================
//
// This module manages three types of sessions, each with different sandbox application strategies:
//
// 1. ExecCommandSession & PersistentShellSession (PTY-based):
//    - Sandbox applied in create_sandboxed_exec_session() / create_persistent_shell_session()
//    - Applied AFTER opening PTY but BEFORE spawning child process
//    - Child processes automatically inherit the sandbox restrictions
//    - Location: session.rs:762, 926
//
// 2. SemanticSearchSession (indexing & file watching):
//    - Sandbox applied in manager.rs in the spawned indexing task (manager.rs:896)
//    - Sandbox is NOT inherited by spawned tasks - must be explicitly applied
//    - File watcher runs in the same thread/task context, so inherits sandbox
//    - File event processing (process_file_events) inherits sandbox from calling context
//
// 3. Tools Execution (tools.rs):
//    - Sandbox applied before spawning tools process (tools.rs:53 for Linux)
//    - Different mechanism for macOS (seatbelt)
//
// KEY PRINCIPLE:
// All components within a session MUST use the same SandboxPolicy instance.
// Spawned tasks/threads do NOT inherit sandbox automatically - they must apply it explicitly.

use crate::error::ColossalErr;
use crate::protocol::SandboxPolicy;
use crate::sandboxing::{SandboxCommand, SandboxExecRequest, SandboxManager};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use qdrant_client::Payload;
use qdrant_client::Qdrant;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::{Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc as tokio_mpsc;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

fn shell_program_name(shell: &str) -> String {
    PathBuf::from(shell)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(shell)
        .to_ascii_lowercase()
}

fn shell_descriptor(shell: &str) -> crate::shell::Shell {
    let path = PathBuf::from(shell);
    let program_name = shell_program_name(shell);
    if program_name == "nu" || program_name == "nu.exe" || program_name.contains("nushell") {
        crate::shell::Shell::new_managed_nu(path)
    } else {
        crate::shell::Shell::new_posix(program_name, path)
    }
}

fn maybe_remove_bwrap_new_session(request: &mut SandboxExecRequest) {
    if request.sandbox != crate::sandboxing::SandboxType::LinuxBubblewrap {
        return;
    }

    request.args.retain(|arg| arg != "--new-session");
}

/// Shared session state that can be accessed by both PTY and semantic search sessions
#[derive(Debug)]
pub struct SharedSessionState {
    /// Current working directory
    cwd: Arc<Mutex<PathBuf>>,
    /// Environment variables
    env: Arc<Mutex<HashMap<String, String>>>,
    /// Listeners for state changes
    listeners: Arc<Mutex<Vec<mpsc::UnboundedSender<SessionStateEvent>>>>,
}

/// Events that can be sent when session state changes
#[derive(Debug, Clone)]
pub enum SessionStateEvent {
    /// Working directory changed
    CwdChanged(PathBuf),
    /// Environment variable added/updated
    EnvVarSet(String, String),
    /// Environment variable removed
    EnvVarUnset(String),
}

impl SharedSessionState {
    /// Create a new shared session state
    pub fn new(initial_cwd: PathBuf) -> Self {
        Self {
            cwd: Arc::new(Mutex::new(initial_cwd)),
            env: Arc::new(Mutex::new(HashMap::new())),
            listeners: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Get the current working directory
    pub fn get_cwd(&self) -> PathBuf {
        self.cwd.lock().unwrap().clone()
    }

    /// Set the current working directory
    pub fn set_cwd(&self, new_cwd: PathBuf) {
        *self.cwd.lock().unwrap() = new_cwd.clone();
        self.notify_listeners(SessionStateEvent::CwdChanged(new_cwd));
    }

    /// Get an environment variable
    pub fn get_env_var(&self, key: &str) -> Option<String> {
        self.env.lock().unwrap().get(key).cloned()
    }

    /// Set an environment variable
    pub fn set_env_var(&self, key: String, value: String) {
        self.env.lock().unwrap().insert(key.clone(), value.clone());
        self.notify_listeners(SessionStateEvent::EnvVarSet(key, value));
    }

    /// Remove an environment variable
    pub fn unset_env_var(&self, key: &str) {
        self.env.lock().unwrap().remove(key);
        self.notify_listeners(SessionStateEvent::EnvVarUnset(key.to_string()));
    }

    /// Get all environment variables
    pub fn get_all_env_vars(&self) -> HashMap<String, String> {
        self.env.lock().unwrap().clone()
    }

    /// Add a listener for state changes
    pub fn add_listener(&self, listener: mpsc::UnboundedSender<SessionStateEvent>) {
        self.listeners.lock().unwrap().push(listener);
    }

    /// Notify all listeners of a state change
    fn notify_listeners(&self, event: SessionStateEvent) {
        let mut listeners = self.listeners.lock().unwrap();
        listeners.retain(|listener| listener.send(event.clone()).is_ok());
    }
}

#[derive(Debug)]
pub struct ExecCommandSession {
    writer_tx: mpsc::Sender<Vec<u8>>,
    output_tx: broadcast::Sender<Vec<u8>>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    _reader_handle: JoinHandle<()>,
    _writer_handle: JoinHandle<()>,
    wait_handle: JoinHandle<Result<i32, std::io::Error>>,
    sandbox_policy: SandboxPolicy,
    cwd: PathBuf,
}

#[derive(Debug)]
pub struct PersistentShellSession {
    writer_tx: mpsc::Sender<Vec<u8>>,
    output_tx: broadcast::Sender<Vec<u8>>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    _reader_handle: JoinHandle<()>,
    _writer_handle: JoinHandle<()>,
    wait_handle: JoinHandle<Result<i32, std::io::Error>>,
    shell_path: String,
    initial_cwd: PathBuf,
    // Shared session state
    shared_state: Arc<SharedSessionState>,
    // Command history
    history: Arc<Mutex<Vec<String>>>,
    // Current history position for navigation
    history_position: Arc<Mutex<usize>>,
    // Sandbox policy for this session
    sandbox_policy: SandboxPolicy,
    // Ready state - tracks whether shell is initialized and ready to accept commands
    ready: Arc<tokio::sync::RwLock<bool>>,
}

pub struct SemanticSearchSession {
    // Qdrant client for semantic search operations
    client: Arc<Qdrant>,
    // Current working directory for indexing
    cwd: PathBuf,
    // Indexing status
    status: Arc<RwLock<crate::semantic_search_lib::IngestStatus>>,
    // Indexing task handle
    indexing_handle: Option<JoinHandle<()>>,
    // Collection name for this session
    collection_name: String,
    // File watcher for monitoring changes
    file_watcher: Option<Arc<Mutex<RecommendedWatcher>>>,
    // File watcher task handle - processes file events in the background
    file_watcher_handle: Option<JoinHandle<()>>,
    // Debounce map to track last modification time of files
    _debounce_map: Arc<Mutex<HashMap<PathBuf, u64>>>,
    // Debounce delay in milliseconds
    _debounce_delay: u64,
    // File content cache to track old content for diff calculation
    _file_content_cache: Arc<Mutex<HashMap<PathBuf, String>>>,
    // Sandbox policy for this session (used by file watcher and indexing)
    sandbox_policy: SandboxPolicy,
}

// Manual Debug implementation for SemanticSearchSession since Qdrant doesn't implement Debug
impl std::fmt::Debug for SemanticSearchSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SemanticSearchSession")
            .field("cwd", &self.cwd)
            .field("collection_name", &self.collection_name)
            .finish()
    }
}

impl ExecCommandSession {
    pub fn new(
        writer_tx: mpsc::Sender<Vec<u8>>,
        output_tx: broadcast::Sender<Vec<u8>>,
        killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
        reader_handle: JoinHandle<()>,
        writer_handle: JoinHandle<()>,
        wait_handle: JoinHandle<Result<i32, std::io::Error>>,
        sandbox_policy: SandboxPolicy,
        cwd: PathBuf,
    ) -> Self {
        Self {
            writer_tx,
            output_tx,
            killer,
            _reader_handle: reader_handle,
            _writer_handle: writer_handle,
            wait_handle,
            sandbox_policy,
            cwd,
        }
    }

    pub fn writer_sender(&self) -> mpsc::Sender<Vec<u8>> {
        self.writer_tx.clone()
    }

    pub fn output_receiver(&self) -> broadcast::Receiver<Vec<u8>> {
        self.output_tx.subscribe()
    }

    pub fn wait_handle(&self) -> &JoinHandle<Result<i32, std::io::Error>> {
        &self.wait_handle
    }

    /// Subscribe to output stream with enhanced streaming capabilities
    /// This provides a more structured approach to streaming with better error handling
    pub fn subscribe_stream(&self) -> broadcast::Receiver<Vec<u8>> {
        self.output_tx.subscribe()
    }

    /// Kill the session process
    pub fn kill(&self) -> Result<(), std::io::Error> {
        // Clone the killer so we can call kill() which takes self by value
        self.killer.clone_killer().kill()
    }

    /// Get the sandbox policy for this session
    pub fn sandbox_policy(&self) -> &SandboxPolicy {
        &self.sandbox_policy
    }

    /// Get the current working directory for this session
    pub fn cwd(&self) -> &PathBuf {
        &self.cwd
    }
}

impl PersistentShellSession {
    pub fn new(
        writer_tx: mpsc::Sender<Vec<u8>>,
        output_tx: broadcast::Sender<Vec<u8>>,
        killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
        reader_handle: JoinHandle<()>,
        writer_handle: JoinHandle<()>,
        wait_handle: JoinHandle<Result<i32, std::io::Error>>,
        shell_path: String,
        initial_cwd: PathBuf,
        shared_state: Arc<SharedSessionState>,
        sandbox_policy: SandboxPolicy,
    ) -> Self {
        Self {
            writer_tx,
            output_tx,
            killer,
            _reader_handle: reader_handle,
            _writer_handle: writer_handle,
            wait_handle,
            shell_path,
            initial_cwd,
            shared_state,
            history: Arc::new(Mutex::new(Vec::new())),
            history_position: Arc::new(Mutex::new(0)),
            sandbox_policy,
            ready: Arc::new(tokio::sync::RwLock::new(false)),
        }
    }

    pub async fn is_ready(&self) -> bool {
        *self.ready.read().await
    }

    pub async fn mark_ready(&self) {
        *self.ready.write().await = true;
    }

    pub async fn wait_until_ready(&self, timeout: std::time::Duration) -> Result<(), ColossalErr> {
        let start = std::time::Instant::now();
        while !self.is_ready().await {
            if start.elapsed() > timeout {
                return Err(ColossalErr::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Shell session failed to become ready within timeout",
                )));
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        Ok(())
    }

    pub fn writer_sender(&self) -> mpsc::Sender<Vec<u8>> {
        self.writer_tx.clone()
    }

    pub fn output_receiver(&self) -> broadcast::Receiver<Vec<u8>> {
        self.output_tx.subscribe()
    }

    pub fn wait_handle(&self) -> &JoinHandle<Result<i32, std::io::Error>> {
        &self.wait_handle
    }

    /// Subscribe to output stream with enhanced streaming capabilities
    /// This provides a more structured approach to streaming with better error handling
    pub fn subscribe_stream(&self) -> broadcast::Receiver<Vec<u8>> {
        self.output_tx.subscribe()
    }

    /// Kill the session process
    pub fn kill(&self) -> Result<(), std::io::Error> {
        // Clone the killer so we can call kill() which takes self by value
        self.killer.clone_killer().kill()
    }

    /// Get the shell path
    pub fn shell_path(&self) -> &str {
        &self.shell_path
    }

    /// Get the initial working directory
    pub fn initial_cwd(&self) -> PathBuf {
        self.initial_cwd.clone()
    }

    /// Get the current working directory
    pub fn current_cwd(&self) -> PathBuf {
        self.shared_state.get_cwd()
    }

    /// Set an environment variable
    pub fn set_env(&self, key: String, value: String) {
        self.shared_state.set_env_var(key, value);
    }

    /// Remove an environment variable
    pub fn unset_env(&self, key: &str) {
        self.shared_state.unset_env_var(key);
    }

    /// Get an environment variable
    pub fn get_env(&self, key: &str) -> Option<String> {
        self.shared_state.get_env_var(key)
    }

    /// Get all environment variables
    pub fn get_all_env(&self) -> HashMap<String, String> {
        self.shared_state.get_all_env_vars()
    }

    /// Update the current working directory
    pub fn update_cwd(&self, new_cwd: PathBuf) {
        self.shared_state.set_cwd(new_cwd);
    }

    /// Add a command to history
    pub fn add_to_history(&self, command: String) {
        let mut history = self.history.lock().unwrap();
        if !command.trim().is_empty() {
            history.push(command);
            // Keep only the last 1000 commands
            let len = history.len();
            if len > 1000 {
                history.drain(0..len - 1000);
            }
        }
        // Reset history position
        *self.history_position.lock().unwrap() = history.len();
    }

    /// Get command history
    pub fn get_history(&self) -> Vec<String> {
        self.history.lock().unwrap().clone()
    }

    /// Get previous command from history
    pub fn get_previous_history_item(&self) -> Option<String> {
        let history = self.history.lock().unwrap();
        let mut pos = self.history_position.lock().unwrap();

        if history.is_empty() {
            return None;
        }

        if *pos > 0 {
            *pos -= 1;
        }

        history.get(*pos).cloned()
    }

    /// Get next command from history
    pub fn get_next_history_item(&self) -> Option<String> {
        let history = self.history.lock().unwrap();
        let mut pos = self.history_position.lock().unwrap();

        if *pos < history.len() {
            *pos += 1;
        }

        if *pos < history.len() {
            history.get(*pos).cloned()
        } else {
            // Return empty string when we've reached the end (current command line)
            Some(String::new())
        }
    }

    /// Get the sandbox policy for this session
    pub fn sandbox_policy(&self) -> &SandboxPolicy {
        &self.sandbox_policy
    }
}

impl SemanticSearchSession {
    pub fn new(
        client: Arc<Qdrant>,
        cwd: PathBuf,
        status: Arc<RwLock<crate::semantic_search_lib::IngestStatus>>,
        indexing_handle: Option<JoinHandle<()>>,
        collection_name: String,
        sandbox_policy: crate::protocol::SandboxPolicy,
    ) -> Result<Self, ColossalErr> {
        // NOTE: Sandbox is applied by the spawned indexing task in manager.rs
        // We don't apply it here to avoid double-application

        // Create a channel for file events
        let (file_event_tx, mut file_event_rx) = tokio_mpsc::unbounded_channel();

        // Create file watcher
        let file_watcher = match RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                match res {
                    Ok(event) => {
                        // Filter for relevant events (create, modify, delete)
                        match event.kind {
                            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                                let _ = file_event_tx.send(event);
                            }
                            _ => {}
                        }
                    }
                    Err(_e) => {
                        // eprintln!("File watcher error: {:?}", e);
                    }
                }
            },
            notify::Config::default(),
        ) {
            Ok(watcher) => {
                let watcher = Arc::new(Mutex::new(watcher));
                // Start watching the current directory
                if let Ok(mut watcher_guard) = watcher.lock() {
                    let _ = watcher_guard.watch(&cwd, RecursiveMode::Recursive);
                }
                Some(watcher)
            }
            Err(_e) => {
                // eprintln!("Failed to create file watcher: {:?}", e);
                None
            }
        };

        // Spawn file watcher task if watcher was created successfully
        let file_watcher_handle = if file_watcher.is_some() {
            let client_clone = client.clone();
            let collection_name_clone = collection_name.clone();
            let debounce_map_clone = Arc::new(Mutex::new(HashMap::new()));
            let file_content_cache_clone = Arc::new(Mutex::new(HashMap::new()));
            let cwd_clone = cwd.clone();
            #[cfg(target_os = "linux")]
            let sandbox_policy_clone = sandbox_policy.clone();

            let handle = tokio::spawn(async move {
                // Apply sandbox to this file watcher task
                #[cfg(target_os = "linux")]
                if let Err(_e) = crate::landlock::apply_sandbox_policy_to_current_thread(
                    &sandbox_policy_clone,
                    &cwd_clone,
                ) {
                    // eprintln!("Failed to apply sandbox to file watcher task: {}", e);
                    return;
                }

                // Process file events as they come in
                while let Some(event) = file_event_rx.recv().await {
                    Self::process_file_event_static(
                        &client_clone,
                        &collection_name_clone,
                        &debounce_map_clone,
                        &file_content_cache_clone,
                        &cwd_clone,
                        event,
                    )
                    .await;
                }
            });

            Some(handle)
        } else {
            None
        };

        Ok(Self {
            client,
            cwd,
            status,
            indexing_handle,
            collection_name,
            file_watcher,
            file_watcher_handle,
            _debounce_map: Arc::new(Mutex::new(HashMap::new())),
            _debounce_delay: 1000, // 1 second debounce delay
            _file_content_cache: Arc::new(Mutex::new(HashMap::new())),
            sandbox_policy,
        })
    }

    /// Get the current working directory
    pub fn current_cwd(&self) -> PathBuf {
        self.cwd.clone()
    }

    /// Update the current working directory
    pub fn update_cwd(&mut self, new_cwd: PathBuf) {
        // eprintln!("Updating semantic search working directory from {:?} to {:?}", self.cwd, new_cwd);

        let old_cwd = self.cwd.clone();
        self.cwd = new_cwd.clone();

        // 1. Update the file watcher to monitor the new directory
        if let Some(ref watcher) = self.file_watcher {
            if let Ok(mut watcher_guard) = watcher.lock() {
                // Unwatch the old directory
                if let Err(_e) = watcher_guard.unwatch(&old_cwd) {
                    // eprintln!("  - Failed to unwatch old directory {}: {}", old_cwd.display(), e);
                } else {
                    // eprintln!("  - Unwatched old directory: {}", old_cwd.display());
                }

                // Watch the new directory
                if let Err(_e) = watcher_guard.watch(&new_cwd, notify::RecursiveMode::Recursive) {
                    // eprintln!("  - Failed to watch new directory {}: {}", new_cwd.display(), e);
                } else {
                    // eprintln!("  - Now watching new directory: {}", new_cwd.display());
                }
            }
        }

        // 2. Update status to indicate reindexing
        if let Ok(mut status) = self.status.write() {
            status.state = "indexing".to_string();
            status.progress_percent = 0.0;
        }

        // 3. Trigger reindexing of the new directory
        // Note: We could spawn a new indexing task here, but for simplicity,
        // we'll let the file watcher pick up changes as files are accessed.
        // A full reindex would require spawning a new background task similar
        // to what's done in manager.rs create_semantic_search_session()

        // eprintln!("Working directory updated successfully");
    }

    /// Get the indexing status
    pub fn get_status(&self) -> crate::semantic_search_lib::IngestStatus {
        self.status.read().unwrap().clone()
    }

    /// Kill/stop the semantic search session
    pub fn kill(&self) -> Result<(), std::io::Error> {
        // 1. Update status to indicate termination
        if let Ok(mut status) = self.status.write() {
            status.state = "terminating".to_string();
            status.progress_percent = 0.0;
        }

        // 2. Stop the file watcher
        if let Some(ref watcher) = self.file_watcher {
            if let Ok(mut watcher_guard) = watcher.lock() {
                // Unwatch the directory
                let _ = watcher_guard.unwatch(&self.cwd);
            }
        }

        // 3. Abort the file watcher task
        if let Some(ref handle) = self.file_watcher_handle {
            handle.abort();
        }

        // 4. Abort the indexing task
        if let Some(ref handle) = self.indexing_handle {
            handle.abort();
        }

        // 5. Delete the Qdrant collection
        // Note: This is a blocking operation in an async context, but since
        // we're in a Drop-like cleanup, we'll use tokio::task::block_in_place
        let client = self.client.clone();
        let collection_name = self.collection_name.clone();

        std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async {
                if let Err(_e) = client.delete_collection(&collection_name).await {
                    // eprintln!("  - Failed to delete Qdrant collection '{}': {}", collection_name, e);
                } else {
                    // eprintln!("  - Deleted Qdrant collection '{}'", collection_name);
                }
            });
        });

        Ok(())
    }

    /// Static method to process file events - used by the background file watcher task
    async fn process_file_event_static(
        client: &Arc<Qdrant>,
        collection_name: &str,
        debounce_map: &Arc<Mutex<HashMap<PathBuf, u64>>>,
        file_content_cache: &Arc<Mutex<HashMap<PathBuf, String>>>,
        _cwd: &PathBuf,
        event: notify::Event,
    ) {
        match event.kind {
            EventKind::Create(_) => {
                for path in event.paths {
                    if Self::is_supported_file_static(&path) {
                        if Self::should_process_event_static(&path, debounce_map, 1000).await {
                            Self::index_file_static(
                                client,
                                collection_name,
                                file_content_cache,
                                &path,
                            )
                            .await;
                        }
                    }
                }
            }
            EventKind::Modify(_) => {
                for path in event.paths {
                    if Self::is_supported_file_static(&path) {
                        if Self::should_process_event_static(&path, debounce_map, 1000).await {
                            Self::update_file_index_static(
                                client,
                                collection_name,
                                file_content_cache,
                                &path,
                            )
                            .await;
                        }
                    }
                }
            }
            EventKind::Remove(_) => {
                for path in event.paths {
                    if Self::is_supported_file_static(&path) {
                        Self::remove_file_from_index_static(
                            client,
                            collection_name,
                            file_content_cache,
                            &path,
                        )
                        .await;
                    }
                }
            }
            _ => {}
        }
    }

    /// Check if we should process a file event based on debounce timing (static version)
    async fn should_process_event_static(
        path: &PathBuf,
        debounce_map: &Arc<Mutex<HashMap<PathBuf, u64>>>,
        debounce_delay: u64,
    ) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| std::time::Duration::from_secs(0))
            .as_millis() as u64;

        let mut debounce_map = debounce_map.lock().unwrap();

        // Check if we've seen this file recently
        if let Some(last_modified) = debounce_map.get(path) {
            // If less than debounce_delay has passed, don't process
            if now - last_modified < debounce_delay {
                // Update the timestamp but don't process
                debounce_map.insert(path.clone(), now);
                return false;
            }
        }

        // Update the timestamp and allow processing
        debounce_map.insert(path.clone(), now);
        true
    }

    /// Check if a file is a supported code file (static version)
    fn is_supported_file_static(path: &PathBuf) -> bool {
        chunker::ChunkerFactory::is_supported(path)
    }

    /// Index a new file (static version)
    async fn index_file_static(
        client: &Arc<Qdrant>,
        collection_name: &str,
        file_content_cache: &Arc<Mutex<HashMap<PathBuf, String>>>,
        path: &PathBuf,
    ) {
        println!("Indexing new file: {:?}", path);

        // Read the file content
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(_e) => {
                // eprintln!("Failed to read file {}: {}", path.display(), e);
                return;
            }
        };

        // Store the content in the cache
        {
            let mut cache = file_content_cache.lock().unwrap();
            cache.insert(path.clone(), content.clone());
        }

        // Use the chunker crate to parse the file
        let chunks = match chunker::chunk_file(path.to_str().unwrap()) {
            Ok(chunks) => chunks,
            Err(_e) => {
                // eprintln!("Failed to chunk file {}: {}", path.display(), e);
                return;
            }
        };

        // Generate embeddings and add to Qdrant
        for chunk in chunks.into_iter() {
            if let Ok(response) = crate::semantic_search_lib::call(&chunk.source_code).await {
                if let Ok(embedding) =
                    crate::semantic_search_lib::parse_response_to_vec(response).await
                {
                    // Use UUID for point ID to avoid collisions
                    let point_id = uuid::Uuid::new_v4().to_string();
                    let payload: qdrant_client::Payload = serde_json::json!({
                        "file_name": chunk.file_name,
                        "kind": chunk.kind,
                        "start_byte": chunk.start_byte,
                        "end_byte": chunk.end_byte,
                        "source_code": chunk.source_code,
                        "point_id": point_id,
                    })
                    .try_into()
                    .unwrap_or_default();

                    let point = qdrant_client::qdrant::PointStruct::new(
                        point_id.clone(),
                        embedding,
                        payload,
                    );

                    // Upsert the point to Qdrant
                    let _ = client
                        .upsert_points(qdrant_client::qdrant::UpsertPointsBuilder::new(
                            collection_name,
                            vec![point],
                        ))
                        .await;
                }
            }
        }
    }

    /// Update the index for a modified file (static version)
    async fn update_file_index_static(
        client: &Arc<Qdrant>,
        collection_name: &str,
        file_content_cache: &Arc<Mutex<HashMap<PathBuf, String>>>,
        path: &PathBuf,
    ) {
        println!("Updating index for modified file: {:?}", path);

        // Read the new file content
        let new_content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(_e) => {
                // eprintln!("Failed to read file {}: {}", path.display(), e);
                return;
            }
        };

        // Get the old content from cache
        let old_content = {
            let cache = file_content_cache.lock().unwrap();
            cache.get(path).cloned().unwrap_or_default()
        };

        // Update the cache with new content
        {
            let mut cache = file_content_cache.lock().unwrap();
            cache.insert(path.clone(), new_content.clone());
        }

        // Calculate the change range
        if let Some((start_byte, end_byte)) =
            crate::semantic_search_lib::calculate_change_range(&old_content, &new_content)
        {
            println!("Detected change range: {}-{}", start_byte, end_byte);
            // Use the update_affected_chunks function to update only affected parts
            if crate::semantic_search_lib::update_affected_chunks(
                client,
                collection_name,
                &path.to_string_lossy(),
                start_byte,
                end_byte,
                &new_content
                    [start_byte as usize..std::cmp::min(end_byte as usize, new_content.len())],
            )
            .await
            .is_err()
            {
                // eprintln!("Failed to update affected chunks, falling back to full reindex");
                // Fall back to full reindexing
                Self::remove_file_from_index_static(
                    client,
                    collection_name,
                    file_content_cache,
                    path,
                )
                .await;
                Self::index_file_static(client, collection_name, file_content_cache, path).await;
            }
        } else {
            println!("No changes detected in file: {:?}", path);
        }
    }

    /// Remove a file from the index (static version)
    async fn remove_file_from_index_static(
        client: &Arc<Qdrant>,
        collection_name: &str,
        file_content_cache: &Arc<Mutex<HashMap<PathBuf, String>>>,
        path: &PathBuf,
    ) {
        println!("Removing file from index: {:?}", path);

        // Remove the file content from the cache
        {
            let mut cache = file_content_cache.lock().unwrap();
            cache.remove(path);
        }

        // Create a filter to match points for this file
        let filter =
            qdrant_client::qdrant::Filter::must([qdrant_client::qdrant::Condition::matches(
                "file_name",
                path.to_string_lossy().to_string(),
            )]);

        // Delete points matching the filter
        let delete_points = qdrant_client::qdrant::DeletePointsBuilder::new(collection_name)
            .points(filter)
            .wait(true)
            .build();
        let _ = client.delete_points(delete_points).await;
    }

    /// Search for code chunks using semantic search
    pub async fn search(
        &self,
        query: &str,
        limit: u64,
    ) -> Result<Vec<(f32, Payload)>, Box<dyn std::error::Error + Send + Sync>> {
        crate::semantic_search_lib::search_codebase(
            query,
            &self.client,
            &self.collection_name,
            limit,
        )
        .await
    }

    /// Get the sandbox policy for this session
    pub fn sandbox_policy(&self) -> &SandboxPolicy {
        &self.sandbox_policy
    }
}

/// Create a sandboxed command execution session with PTY support
///
/// SANDBOX APPLICATION ORDER:
/// 1. Open PTY (requires /dev/ptmx access)
/// 2. Wrap the command with the platform sandbox launcher when needed
/// 3. Spawn the wrapped child process under the PTY
///
/// The child process will run under the sandbox restrictions.
pub async fn create_sandboxed_exec_session(
    command: String,
    _shell: String,
    _login: bool,
    sandbox_policy: SandboxPolicy,
    cwd: PathBuf,
    extra_env: HashMap<String, String>,
) -> Result<(ExecCommandSession, tokio::sync::oneshot::Receiver<i32>), ColossalErr> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| ColossalErr::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;

    let args = shlex::split(&command).ok_or_else(|| {
        ColossalErr::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Failed to parse command string",
        ))
    })?;
    let program = args.first().ok_or_else(|| {
        ColossalErr::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Failed to parse command string",
        ))
    })?;
    let mut env = HashMap::new();
    env.insert("HISTFILE".to_string(), "/dev/null".to_string());
    env.insert("HISTSIZE".to_string(), "0".to_string());
    env.insert("SAVEHIST".to_string(), "0".to_string());
    env.insert("HISTCONTROL".to_string(), "ignoreboth".to_string());
    env.extend(extra_env);
    let mut request = SandboxManager::new().prepare_spawn(
        SandboxCommand {
            program: PathBuf::from(program),
            args: args[1..].to_vec(),
            cwd: cwd.clone(),
            env,
        },
        &sandbox_policy,
    )?;
    maybe_remove_bwrap_new_session(&mut request);

    // Windows: If WindowsRestrictedToken sandbox is requested with ConPTY handles,
    // use our own spawn function that applies both sandbox AND PTY.
    // portable_pty's CommandBuilder doesn't support restricted tokens.
    #[cfg(target_os = "windows")]
    if request.sandbox == crate::sandboxing::SandboxType::WindowsRestrictedToken {
        if let Some(conpty_handles) = request.conpty_handles.take() {
            use crate::windows_sandbox::conpty::{ConptyHandles, spawn_sandboxed_pty_process};
            use crate::windows_sandbox::token::get_current_token_for_restriction;

            let token = get_current_token_for_restriction().map_err(|e| {
                crate::windows_sandbox::conpty::close_conpty_handles(conpty_handles);
                ColossalErr::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to get sandbox token: {}", e),
                ))
            })?;

            let mut argv = vec![request.program.to_string_lossy().to_string()];
            argv.extend(request.args.iter().cloned());

            let (process_info, handles) = spawn_sandboxed_pty_process(
                token,
                &argv,
                &request.cwd,
                &request.env,
                conpty_handles,
            )
            .map_err(|e| {
                ColossalErr::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to spawn sandboxed PTY process: {}", e),
                ))
            })?;

            // Wrap the Windows process to work with our session
            struct WindowsChild {
                pid: u32,
                input_write: HANDLE,
                output_read: HANDLE,
            }

            use portable_pty::Child;
            use std::sync::Mutex;

            impl Child for WindowsChild {
                fn kill(&self) -> std::io::Result<()> {
                    unsafe {
                        windows_sys::Win32::System::Threading::TerminateProcess(
                            self.pid as *mut _,
                            1,
                        );
                    }
                    Ok(())
                }
                fn wait(&self) -> std::io::Result<portable_pty::ExitStatus> {
                    Ok(portable_pty::ExitStatus::from_raw(0))
                }
                fn try_wait(&self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
                    Ok(Some(portable_pty::ExitStatus::from_raw(0)))
                }
                fn id(&self) -> u32 {
                    self.pid
                }
            }

            let child: Box<dyn portable_pty::Child + Send + Sync> = Box::new(WindowsChild {
                pid: process_info.dwProcessId,
                input_write: handles.input_write,
                output_read: handles.output_read,
            });

            let killer = child.clone_killer();

            // For ConPTY, we need to handle I/O manually since we're not using portable_pty's spawn
            let (writer_tx, mut writer_rx) = mpsc::channel::<Vec<u8>>(128);
            let (output_tx, _) = tokio::sync::broadcast::channel::<Vec<u8>>(256);

            // Writer task: forward write requests to ConPTY input
            let input_write = handles.input_write;
            let writer_handle = tokio::spawn(async move {
                use windows_sys::Win32::Foundation::WriteFile;
                let mut buffer = [0u8; 8192];
                loop {
                    match writer_rx.recv().await {
                        Some(data) => {
                            let mut offset = 0;
                            while offset < data.len() {
                                let written = unsafe {
                                    WriteFile(
                                        input_write,
                                        data.as_ptr().add(offset),
                                        data.len() as u32,
                                        std::ptr::null_mut(),
                                        std::ptr::null_mut(),
                                    )
                                };
                                if written == 0 {
                                    break;
                                }
                                offset += written as usize;
                            }
                        }
                        None => break,
                    }
                }
            });

            // Reader task: drain ConPTY output
            let output_read = handles.output_read;
            let output_tx_clone = output_tx.clone();
            let reader_handle = tokio::task::spawn_blocking(move || {
                let mut buf = [0u8; 8192];
                loop {
                    let mut bytes_read: u32 = 0;
                    let ok = unsafe {
                        windows_sys::Win32::Foundation::ReadFile(
                            output_read,
                            buf.as_mut_ptr(),
                            buf.len() as u32,
                            &mut bytes_read,
                            std::ptr::null_mut(),
                        )
                    };
                    if ok == 0 || bytes_read == 0 {
                        break;
                    }
                    let _ = output_tx_clone.send(buf[..bytes_read as usize].to_vec());
                }
            });

            // Session identifier
            let id = uuid::Uuid::new_v4().to_string();
            let start_time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();

            let session = ExecCommandSession {
                id: id.clone(),
                child: Arc::new(Mutex::new(Some(child))),
                writer: Arc::new(Mutex::new(writer_tx)),
                output_rx: Arc::new(RwLock::new(Some(output_tx))),
                killer,
                reader_handle,
                writer_handle,
                start_time,
            };

            let (tx, rx) = tokio::sync::oneshot::channel();
            let session_child = session.child.clone();
            let session_output = session.output_rx.clone();
            tokio::spawn(async move {
                // Wait for process to exit
                // In a full implementation, we'd monitor the Windows process
                let _ = tx.send(0);
                *session_child.lock().unwrap() = None;
                let _ = session_output.write().unwrap().take();
            });

            return Ok((session, rx));
        } else {
            return Err(ColossalErr::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "PTY sandbox on Windows requires ConPTY support but handles not available",
            )));
        }
    }

    // macOS uses external wrapper (sandbox-exec) which works with PTY.
    // No additional checks needed - if seatbelt fails, spawn_command will error.

    let mut child;
    let mut attempts = 0;
    loop {
        let mut command_builder = CommandBuilder::new(&request.program);
        for arg in &request.args {
            command_builder.arg(arg);
        }

        // Set working directory
        command_builder.cwd(&request.cwd);
        for (key, value) in &request.env {
            command_builder.env(key, value);
        }

        match pair.slave.spawn_command(command_builder) {
            Ok(c) => {
                child = c;
                break;
            }
            Err(e) => {
                if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
                    if io_err.kind() == std::io::ErrorKind::WouldBlock {
                        attempts += 1;
                        if attempts > 5 {
                            return Err(ColossalErr::Io(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                "Failed to spawn command after several retries",
                            )));
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        continue;
                    }
                }
                return Err(ColossalErr::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                )));
            }
        }
    }

    // Obtain a killer that can signal the process independently of `.wait()`.
    let killer = child.clone_killer();

    // Channel to forward write requests to the PTY writer.
    let (writer_tx, mut writer_rx) = mpsc::channel::<Vec<u8>>(128);
    // Broadcast for streaming PTY output to readers: subscribers receive from subscription time.
    let (output_tx, _) = tokio::sync::broadcast::channel::<Vec<u8>>(256);

    // Reader task: drain PTY and forward chunks to output channel.
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| ColossalErr::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
    let output_tx_clone = output_tx.clone();
    let reader_handle = tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    // Forward to broadcast; best-effort if there are subscribers.
                    // Don't stop if send fails - just means no active subscribers right now
                    let _ = output_tx_clone.send(buf[..n].to_vec());
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {
                    // Retry on EINTR
                    continue;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // We're in a blocking thread; back off briefly and retry.
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                }
                Err(_e) => {
                    // PTY read error - exit loop
                    break;
                }
            }
        }

        // The loop has finished, so we can drop the sender.
        // This will close the channel and signal to receivers that there is no more output.
        drop(output_tx_clone);
    });

    // Writer task: apply stdin writes to the PTY writer.
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| ColossalErr::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
    let writer = Arc::new(StdMutex::new(writer));
    let writer_handle = tokio::spawn({
        let writer = writer.clone();
        async move {
            while let Some(bytes) = writer_rx.recv().await {
                let writer = writer.clone();
                // Perform blocking write on a blocking thread.
                let _ = tokio::task::spawn_blocking(move || {
                    if let Ok(mut guard) = writer.lock() {
                        let _ = guard.write_all(&bytes);
                        let _ = guard.flush();
                    }
                })
                .await;
            }
            // Close the writer to signal EOF to the process
            let _ = tokio::task::spawn_blocking(move || {
                // Explicitly drop the writer to close it
                drop(writer);
            })
            .await;
        }
    });

    // Keep the child alive until it exits, then signal exit code.
    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel::<i32>();
    let wait_handle = tokio::task::spawn_blocking(move || {
        let code = match child.wait() {
            Ok(status) => status.exit_code() as i32,
            Err(_) => -1,
        };
        let _ = exit_tx.send(code);
        Ok(code)
    });

    // Create and store the session with channels.
    let session = ExecCommandSession::new(
        writer_tx,
        output_tx,
        killer,
        reader_handle,
        writer_handle,
        wait_handle,
        sandbox_policy,
        cwd,
    );
    Ok((session, exit_rx))
}

/// Create a persistent shell session with PTY support
///
/// SANDBOX APPLICATION ORDER:
/// 1. Open PTY (requires /dev/ptmx access)
/// 2. Wrap the shell with the platform sandbox launcher when needed
/// 3. Spawn the wrapped shell process under the PTY
///
/// The shell and all commands executed within it will run under the sandbox restrictions.
pub async fn create_persistent_shell_session(
    shell: String,
    login: bool,
    sandbox_policy: SandboxPolicy,
    cwd: PathBuf,
) -> Result<(PersistentShellSession, tokio::sync::oneshot::Receiver<i32>), ColossalErr> {
    let initial_cwd = cwd.clone();
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| ColossalErr::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;

    let mut env = HashMap::new();
    env.insert("HISTFILE".to_string(), "/dev/null".to_string());
    env.insert("HISTSIZE".to_string(), "0".to_string());
    env.insert("SAVEHIST".to_string(), "0".to_string());
    env.insert("HISTCONTROL".to_string(), "ignoreboth".to_string());
    env.insert("TERM".to_string(), "dumb".to_string());
    env.insert("PS1".to_string(), "".to_string());
    env.insert("PROMPT".to_string(), "".to_string());
    env.insert("RPROMPT".to_string(), "".to_string());
    env.insert("RPS1".to_string(), "".to_string());
    env.insert("NO_COLOR".to_string(), "1".to_string());
    let shell_descriptor = shell_descriptor(&shell);
    let shell_args = shell_descriptor.persistent_shell_args(login);
    let mut request = SandboxManager::new().prepare_spawn(
        SandboxCommand {
            program: PathBuf::from(&shell),
            args: shell_args,
            cwd: cwd.clone(),
            env,
        },
        &sandbox_policy,
    )?;
    maybe_remove_bwrap_new_session(&mut request);

    // Windows: If WindowsRestrictedToken sandbox is requested with ConPTY handles,
    // use our own spawn function that applies both sandbox AND PTY.
    #[cfg(target_os = "windows")]
    if request.sandbox == crate::sandboxing::SandboxType::WindowsRestrictedToken {
        if let Some(conpty_handles) = request.conpty_handles.take() {
            use crate::windows_sandbox::conpty::{ConptyHandles, spawn_sandboxed_pty_process};
            use crate::windows_sandbox::token::get_current_token_for_restriction;

            let token = get_current_token_for_restriction().map_err(|e| {
                crate::windows_sandbox::conpty::close_conpty_handles(conpty_handles);
                ColossalErr::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to get sandbox token: {}", e),
                ))
            })?;

            let mut argv = vec![request.program.to_string_lossy().to_string()];
            argv.extend(request.args.iter().cloned());

            let (process_info, handles) = spawn_sandboxed_pty_process(
                token,
                &argv,
                &request.cwd,
                &request.env,
                conpty_handles,
            )
            .map_err(|e| {
                ColossalErr::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to spawn sandboxed PTY process: {}", e),
                ))
            })?;

            // Wrap the Windows process to work with our session
            struct WindowsChild {
                pid: u32,
            }

            use portable_pty::Child;
            use std::sync::Mutex;

            impl Child for WindowsChild {
                fn kill(&self) -> std::io::Result<()> {
                    unsafe {
                        windows_sys::Win32::System::Threading::TerminateProcess(
                            self.pid as *mut _,
                            1,
                        );
                    }
                    Ok(())
                }
                fn wait(&self) -> std::io::Result<portable_pty::ExitStatus> {
                    Ok(portable_pty::ExitStatus::from_raw(0))
                }
                fn try_wait(&self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
                    Ok(Some(portable_pty::ExitStatus::from_raw(0)))
                }
                fn id(&self) -> u32 {
                    self.pid
                }
            }

            let child: Box<dyn portable_pty::Child + Send + Sync> = Box::new(WindowsChild {
                pid: process_info.dwProcessId,
            });

            let killer = child.clone_killer();

            // For ConPTY, handle I/O manually
            let (writer_tx, mut writer_rx) = mpsc::channel::<Vec<u8>>(128);
            let (output_tx, _) = tokio::sync::broadcast::channel::<Vec<u8>>(256);

            // Writer task
            let input_write = handles.input_write;
            let writer_handle = tokio::spawn(async move {
                use windows_sys::Win32::Foundation::WriteFile;
                loop {
                    match writer_rx.recv().await {
                        Some(data) => {
                            let mut offset = 0;
                            while offset < data.len() {
                                let written = unsafe {
                                    WriteFile(
                                        input_write,
                                        data.as_ptr().add(offset),
                                        data.len() as u32,
                                        std::ptr::null_mut(),
                                        std::ptr::null_mut(),
                                    )
                                };
                                if written == 0 {
                                    break;
                                }
                                offset += written as usize;
                            }
                        }
                        None => break,
                    }
                }
            });

            // Reader task
            let output_read = handles.output_read;
            let output_tx_clone = output_tx.clone();
            let reader_handle = tokio::task::spawn_blocking(move || {
                let mut buf = [0u8; 8192];
                loop {
                    let mut bytes_read: u32 = 0;
                    let ok = unsafe {
                        windows_sys::Win32::Foundation::ReadFile(
                            output_read,
                            buf.as_mut_ptr(),
                            buf.len() as u32,
                            &mut bytes_read,
                            std::ptr::null_mut(),
                        )
                    };
                    if ok == 0 || bytes_read == 0 {
                        break;
                    }
                    let _ = output_tx_clone.send(buf[..bytes_read as usize].to_vec());
                }
            });

            let id = uuid::Uuid::new_v4().to_string();
            let start_time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();

            let session = PersistentShellSession {
                id: id.clone(),
                shell: shell.clone(),
                child: Arc::new(Mutex::new(Some(child))),
                shared_state: Arc::new(RwLock::new(ShellSessionState::Idle)),
                writer: Arc::new(Mutex::new(writer_tx)),
                output_rx: Arc::new(RwLock::new(Some(output_tx))),
                killer,
                reader_handle,
                writer_handle,
                start_time,
            };

            let (tx, rx) = tokio::sync::oneshot::channel();
            let session_child = session.child.clone();
            let session_output = session.output_rx.clone();
            tokio::spawn(async move {
                let _ = tx.send(0);
                *session_child.lock().unwrap() = None;
                let _ = session_output.write().unwrap().take();
            });

            return Ok((session, rx));
        } else {
            return Err(ColossalErr::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "PTY sandbox on Windows requires ConPTY support but handles not available",
            )));
        }
    }

    // macOS uses external wrapper (sandbox-exec) which works with PTY.

    let mut child;
    let mut attempts = 0;
    loop {
        let mut command_builder = CommandBuilder::new(&request.program);
        for arg in &request.args {
            command_builder.arg(arg);
        }

        // Set working directory
        command_builder.cwd(&request.cwd);
        for (key, value) in &request.env {
            command_builder.env(key, value);
        }

        match pair.slave.spawn_command(command_builder) {
            Ok(c) => {
                child = c;
                break;
            }
            Err(e) => {
                if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
                    if io_err.kind() == std::io::ErrorKind::WouldBlock {
                        attempts += 1;
                        if attempts > 5 {
                            return Err(ColossalErr::Io(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                "Failed to spawn command after several retries",
                            )));
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        continue;
                    }
                }
                return Err(ColossalErr::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                )));
            }
        }
    }

    // Obtain a killer that can signal the process independently of `.wait()`.
    let killer = child.clone_killer();

    // Channel to forward write requests to the PTY writer.
    let (writer_tx, mut writer_rx) = mpsc::channel::<Vec<u8>>(128);
    // Broadcast for streaming PTY output to readers: subscribers receive from subscription time.
    let (output_tx, _) = tokio::sync::broadcast::channel::<Vec<u8>>(256);

    // Reader task: drain PTY and forward chunks to output channel.
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| ColossalErr::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
    let output_tx_clone = output_tx.clone();
    let reader_handle = tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    // Forward to broadcast; best-effort if there are subscribers.
                    // Don't stop if send fails - just means no active subscribers right now
                    let _ = output_tx_clone.send(buf[..n].to_vec());
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {
                    // Retry on EINTR
                    continue;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // We're in a blocking thread; back off briefly and retry.
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                }
                Err(_e) => {
                    // PTY read error - exit loop
                    break;
                }
            }
        }

        // The loop has finished, so we can drop the sender.
        // This will close the channel and signal to receivers that there is no more output.
        drop(output_tx_clone);
    });

    // Writer task: apply stdin writes to the PTY writer.
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| ColossalErr::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
    let writer = Arc::new(StdMutex::new(writer));
    let writer_handle = tokio::spawn({
        let writer = writer.clone();
        async move {
            while let Some(bytes) = writer_rx.recv().await {
                let writer = writer.clone();
                // Perform blocking write on a blocking thread.
                let _ = tokio::task::spawn_blocking(move || {
                    if let Ok(mut guard) = writer.lock() {
                        let _ = guard.write_all(&bytes);
                        let _ = guard.flush();
                    }
                })
                .await;
            }
            // Close the writer to signal EOF to the process
            let _ = tokio::task::spawn_blocking(move || {
                // Explicitly drop the writer to close it
                drop(writer);
            })
            .await;
        }
    });

    // Keep the child alive until it exits, then signal exit code.
    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel::<i32>();
    let wait_handle = tokio::task::spawn_blocking(move || {
        let code = match child.wait() {
            Ok(status) => status.exit_code() as i32,
            Err(_) => -1,
        };
        let _ = exit_tx.send(code);
        Ok(code)
    });

    // Create shared state for both PTY and semantic search sessions
    let shared_state = Arc::new(SharedSessionState::new(cwd.clone()));

    // Create and store the session with channels.
    let session = PersistentShellSession::new(
        writer_tx.clone(),
        output_tx.clone(),
        killer,
        reader_handle,
        writer_handle,
        wait_handle,
        shell,
        initial_cwd,
        shared_state,
        sandbox_policy,
    );

    // Spawn a task to detect shell readiness by sending a test command
    let ready_flag = session.ready.clone();
    let test_writer_tx = writer_tx.clone();
    let mut test_output_rx = output_tx.subscribe();

    tokio::spawn(async move {
        // Give the shell a moment to spawn
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Send a simple echo command to test if shell is ready
        let test_marker = "__SHELL_READY__";
        let test_cmd = format!("stty -echo -echoctl 2>/dev/null; echo '{}'\r", test_marker);

        if test_writer_tx.send(test_cmd.into_bytes()).await.is_err() {
            // Failed to send, mark as ready anyway to avoid blocking
            *ready_flag.write().await = true;
            return;
        }

        // Wait for the marker in output
        let timeout = tokio::time::Duration::from_secs(3);
        let start = tokio::time::Instant::now();

        while start.elapsed() < timeout {
            match tokio::time::timeout(
                tokio::time::Duration::from_millis(100),
                test_output_rx.recv(),
            )
            .await
            {
                Ok(Ok(output)) => {
                    let output_str = String::from_utf8_lossy(&output);
                    if output_str.contains(test_marker) {
                        // Shell is ready!
                        *ready_flag.write().await = true;
                        return;
                    }
                }
                Ok(Err(_)) => {
                    // Channel closed
                    break;
                }
                Err(_) => {
                    // Timeout on recv, continue waiting
                    continue;
                }
            }
        }

        // Timeout or error - mark as ready anyway to avoid blocking forever
        *ready_flag.write().await = true;
    });

    Ok((session, exit_rx))
}
