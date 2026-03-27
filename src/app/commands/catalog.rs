pub(crate) const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/clear", "clear conversation history and free up context"),
    ("/connect", "connect an OpenAI-compatible provider account"),
    ("/exit", "exit the repl"),
    (
        "/export",
        "export the current conversation to a file or clipboard",
    ),
    (
        "/fork",
        "fork (copy) a saved conversation as a new conversation",
    ),
    ("/help", "show help information and available commands"),
    ("/model", "set the ai model for colossal code"),
    ("/resume", "resume a conversation"),
    (
        "/review",
        "review code changes. options: -t <all|committed|uncommitted>, --base <branch>, --base-commit <commit>, --no-tool",
    ),
    (
        "/rewind",
        "restore the code and/or conversation to a previous point",
    ),
    (
        "/safety",
        "configure safety mode (yolo/regular/readonly) and permissions",
    ),
    ("/shells", "list and manage background shell sessions"),
    ("/status", "show tool statuses"),
    (
        "/stats",
        "show the total token count and duration of the current session",
    ),
    (
        "/summarize",
        "summarize conversation to reduce context. optional: /summarize [custom instructions]",
    ),
    (
        "/autosummarize",
        "show or set the auto-summarize trigger percent (percent of context used)",
    ),
    ("/todos", "list current todo items"),
    ("/vim", "toggle between vim and normal editing modes"),
    (
        "/spec",
        "show current spec or load a new spec. usage: /spec [path|goal]",
    ),
    (
        "/spec split",
        "split a step into sub-steps. usage: /spec split <index>",
    ),
    (
        "/spec status",
        "show detailed spec status as JSON (steps + history)",
    ),
    ("/spec abort", "abort the current orchestrator run"),
];
