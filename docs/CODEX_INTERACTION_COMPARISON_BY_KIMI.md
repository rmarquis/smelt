# Codex CLI Architectural Analysis: Keystroke to LLM Request to Rendered Output

> **Scope:** This document exhaustively maps every code path that transforms a user keystroke into an LLM API request and back to rendered output in the OpenAI Codex CLI codebase. It covers `codex/codex-cli/`, `codex/codex-rs/tui/`, `codex/codex-rs/app-server/`, `codex/codex-rs/core/`, and the wire protocol.
>
> **Generated:** 2026-05-20 by Kimi Code CLI via multi-agent codebase exploration.

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Entry Point and Bootstrap Sequence](#2-entry-point-and-bootstrap-sequence)
3. [CLI Layer and UI Mode Dispatch](#3-cli-layer-and-ui-mode-dispatch)
4. [TUI: Input Capture and the Event Loop](#4-tui-input-capture-and-the-event-loop)
5. [ChatWidget: The Chat Surface and Turn Lifecycle](#5-chatwidget-the-chat-surface-and-turn-lifecycle)
6. [App Server Layer: JSON-RPC Facade](#6-app-server-layer-json-rpc-facade)
7. [Core Layer: Thread Management and Model Client](#7-core-layer-thread-management-and-model-client)
8. [API Client Layer: HTTP Request Building](#8-api-client-layer-http-request-building)
9. [Response Streaming: From Provider to Rendered Output](#9-response-streaming-from-provider-to-rendered-output)
10. [Tool Call Lifecycle](#10-tool-call-lifecycle)
11. [Approval System](#11-approval-system)
12. [Alternative UI Modes](#12-alternative-ui-modes)
13. [Complete Data Flow Summary](#13-complete-data-flow-summary)

---

## 1. Executive Summary

Codex CLI is a **Rust-based coding agent** with a layered architecture:

| Layer | Crate/Module | Responsibility |
|-------|-------------|---------------|
| **CLI Bootstrap** | `codex-cli` (Node.js) | Platform detection, native binary spawn |
| **TUI** | `codex_tui` | Interactive terminal UI (crossterm + ratatui) |
| **App Server** | `codex_app_server` | JSON-RPC server mediating TUI ↔ core |
| **Core** | `codex_core` | Session/thread management, tool execution, LLM orchestration |
| **API Client** | `codex_api` | OpenAI Responses API client (HTTP SSE + WebSocket) |
| **HTTP Transport** | `codex_client` | Low-level `reqwest`-based HTTP/WebSocket transport |
| **Protocol** | `codex_protocol` | Shared types, request/response schemas, config types |
| **Login** | `codex_login` | Authentication management (API key, ChatGPT OAuth) |

The high-level flow is:

```
User keystroke
    → crossterm EventStream
    → App::run() event loop
    → ChatWidget::handle_key_event()
    → ChatComposer submit
    → AppServerSession::turn_start()
    → JSON-RPC → app-server MessageProcessor
    → TurnRequestProcessor → ThreadManager → CodexThread
    → ModelClientSession::stream()
    → HTTP SSE / WebSocket
    → ServerNotification stream
    → ChatWidget::on_agent_message_delta()
    → MarkdownStreamCollector + StreamController
    → markdown_render.rs → ratatui styled lines
    → Tui::draw() → inline viewport
    → Terminal
```

Key architectural decisions:
- **crossterm + ratatui** for input handling and rendering (Rust TUI stack)
- **App-server JSON-RPC boundary** decouples TUI from core, enabling headless and IDE modes
- **Inline viewport** (not full alternate screen) so Codex renders above the shell prompt and scrollback stays in terminal history
- **WebSocket preferred** with HTTP SSE fallback for streaming; sticky `x-codex-turn-state` routing tokens
- **Markdown streaming** with newline-gated commit and typing-speed animation

---

## 2. Entry Point and Bootstrap Sequence

### 2.1 Binary Entry Point

**File:** `codex/codex-rs/tui/src/main.rs`

```rust
fn main() -> anyhow::Result<()> {
    arg0_dispatch_or_else(|arg0_paths: Arg0DispatchPaths| async move {
        let top_cli = TopCli::parse();
        let mut inner = top_cli.inner;
        // ...
        let exit_info = run_main(inner, arg0_paths, LoaderOverrides::default(), None).await?;
        // ...
    })
}
```

`arg0_dispatch_or_else` handles platform-specific binary dispatch (e.g., `codex-tui` vs platform wrappers).

### 2.2 TUI Initialization

**File:** `codex/codex-rs/tui/src/lib.rs` (entry via `run_main()`)

Creation sequence:
1. **Parse CLI** (`Cli`) — `clap`-derived args, config overrides
2. **Load config** (`codex_config::LoaderOverrides`) — TOML config file + CLI overrides
3. **Initialize TUI** (`tui::Tui`) — crossterm raw mode, bracketed paste, keyboard enhancements
4. **Create AppServerSession** — JSON-RPC client connection to local app server
5. **Bootstrap** (`app_server.bootstrap()`) — account info, model list, auth mode
6. **Create ChatWidget** — with config, model catalog, telemetry, initial user message
7. **Start thread** (`app_server.start_thread()`) — fresh or resumed session
8. **Run App** (`App::run()`) — enters the main event loop

---

## 3. CLI Layer and UI Mode Dispatch

### 3.1 UI Modes

Codex supports multiple frontends:

| Mode | Entry Point | Description |
|------|-------------|-------------|
| `tui` | `run_main()` → `App::run()` | Interactive terminal (primary mode) |
| `headless` | `run_main()` with flags | Non-interactive batch execution |
| `wire` | App-server JSON-RPC | IDE / editor integration |

### 3.2 TUI Mode Entry

**File:** `codex/codex-rs/tui/src/app.rs` (line 642)

```rust
pub async fn run(
    tui: &mut tui::Tui,
    mut app_server: AppServerSession,
    mut config: Config,
    // ...
) -> Result<AppExitInfo> {
    // Bootstrap, create ChatWidget, start thread, enter event loop
}
```

---

## 4. TUI: Input Capture and the Event Loop

### 4.1 Terminal Input Library

The TUI uses **`crossterm`** for raw terminal I/O and **`ratatui`** for rendering.

- **`crossterm::event::EventStream`** polls stdin asynchronously.
- **`codex-rs/tui/src/tui/event_stream.rs`** wraps this in an `EventBroker`/`TuiEventStream` abstraction so the stream can be **paused/resumed** when handing the terminal to external programs (e.g., `$EDITOR`).

### 4.2 Event Loop

The main loop lives in **`App::run()`** (`codex-rs/tui/src/app.rs`):

```rust
loop {
    select! {
        Some(event) = app_event_rx.recv() => { /* AppEvent from widgets */ }
        Some(event) = active_thread_rx.recv() => { /* Thread events from app server */ }
        Some(event) = tui_events.next() => { /* Key/Resize/Draw from crossterm */ }
        Some(event) = app_server.next_event() => { /* Server notifications */ }
    }
}
```

### 4.3 Key Dispatch

- `TuiEvent::Key(key_event)` → `App::handle_key_event()` → `ChatWidget::handle_key_event()`
- Global shortcuts (Ctrl+L clear, Alt+Up/Down agent switch, Esc backtrack) are handled in `App` first.
- Everything else flows into the **chat composer**.

### 4.4 Chat Composer

**`codex-rs/tui/src/bottom_pane/chat_composer.rs`** is the prompt input state machine:
- Edits a `TextArea` buffer
- Manages popup overlays (slash commands, `@` mentions, file search)
- Handles bracketed paste and **paste-burst detection** (for terminals that don't support bracketed paste)
- On `Enter`, submits the composed message as a `UserMessage`

---

## 5. ChatWidget: The Chat Surface and Turn Lifecycle

### 5.1 ChatWidget

**File:** `codex/codex-rs/tui/src/chatwidget.rs`

`ChatWidget` is the main chat surface. It owns:
- **Transcript state** (`TranscriptState`) — committed history cells
- **Streaming controllers** (`StreamController`, `PlanStreamController`) — in-flight output
- **Bottom pane** (`BottomPane`) — composer, popups, status line
- **Turn lifecycle** (`TurnLifecycleState`) — tracks active turns, interrupts, completion

### 5.2 User Input Submission

When the user submits a message:

```rust
// In ChatWidget / bottom pane
AppEvent::CodexOp(AppCommand::Submit { items, thread_id })
```

This flows to `App::handle_app_event()`, which calls:

```rust
app_server.turn_start(
    thread_id,
    items,           // Vec<UserInput>
    cwd,
    approval_policy,
    approvals_reviewer,
    permissions_override,
    workspace_roots,
    model,
    effort,
    summary,
    service_tier,
    collaboration_mode,
    personality,
    output_schema,
).await
```

### 5.3 Turn Lifecycle State

**File:** `codex/codex-rs/tui/src/chatwidget/turn_lifecycle.rs`

Tracks:
- Active turn ID
- Turn status (running, interrupted, completed)
- Steerability (whether the turn can accept mid-turn input)
- Abort reasons (interrupted, budget-limited)

---

## 6. App Server Layer: JSON-RPC Facade

### 6.1 AppServerSession

**File:** `codex/codex-rs/tui/src/app_server_session.rs`

Typed JSON-RPC client facade used by the TUI. Key methods:

| Method | JSON-RPC Request | Purpose |
|--------|-----------------|---------|
| `bootstrap()` | `initialize` | Account, models, auth mode |
| `start_thread()` | `thread/start` | New conversation thread |
| `turn_start()` | `turn/start` | Begin agent turn |
| `turn_steer()` | `turn/steer` | Mid-turn user input |
| `turn_interrupt()` | `turn/interrupt` | Cancel active turn |

### 6.2 App Server Message Processing

**File:** `codex/codex-rs/app-server/src/message_processor.rs`

Routes incoming JSON-RPC requests to processors:

```rust
// Turn requests go to TurnRequestProcessor
ClientRequest::TurnStart { .. } => turn_processor.handle_turn_start(..)
ClientRequest::TurnSteer { .. } => turn_processor.handle_turn_steer(..)
```

### 6.3 TurnRequestProcessor

**File:** `codex/codex-rs/app-server/src/request_processors/turn_processor.rs`

Delegates to **`codex-core::ThreadManager`**:

```rust
thread_manager.start_turn(thread_id, turn_params)
```

Which finds or creates the **`CodexThread`**, which in turn creates a **`ModelClientSession`** for streaming.

---

## 7. Core Layer: Thread Management and Model Client

### 7.1 ThreadManager

**File:** `codex/codex-rs/core/src/thread_manager.rs`

Manages thread lifecycle:
- `start_thread()` / `resume_thread()` / `fork_thread()`
- `start_turn()` — initiates a turn on a thread
- Routes server notifications back to subscribers

### 7.2 CodexThread

**File:** `codex/codex-rs/core/src/codex_thread.rs`

Single thread execution loop. When a turn starts:
1. Creates `ModelClientSession` from session-scoped `ModelClient`
2. Builds `Prompt` from conversation history, tools, base instructions
3. Calls `model_client_session.stream(prompt, model_info, ...)`
4. Processes `ResponseEvent` stream — text deltas, tool calls, completions
5. Executes tools via `exec_env` / `exec_policy`
6. Sends notifications back to app server subscribers

### 7.3 ModelClient

**File:** `codex/codex-rs/core/src/client.rs`

Session-scoped client holding:
- Auth manager
- Provider info
- Session/thread IDs
- WebSocket fallback state

```rust
pub struct ModelClient {
    state: Arc<ModelClientState>,
}
```

### 7.4 ModelClientSession

**File:** `codex/codex-rs/core/src/client.rs` (line 236)

Turn-scoped streaming session:
- Lazily opens WebSocket connection
- Caches `previous_response_id` for incremental requests
- Stores `x-codex-turn-state` sticky-routing token
- Falls back to HTTP SSE on WebSocket failure

---

## 8. API Client Layer: HTTP Request Building

### 8.1 Prompt Construction

**File:** `codex/codex-rs/core/src/client_common.rs`

```rust
pub struct Prompt {
    pub base_instructions: BaseInstructions,
    pub input: Vec<ResponseItem>,      // Conversation history
    pub tools: Vec<Tool>,              // Available tools
    pub parallel_tool_calls: bool,
    pub output_schema: Option<serde_json::Value>,
    pub output_schema_strict: bool,
}
```

### 8.2 Responses API Request

**File:** `codex/codex-rs/core/src/client.rs` (`build_responses_request()`)

Builds `ResponsesApiRequest`:
```rust
ResponsesApiRequest {
    model: model_info.slug.clone(),
    instructions: instructions.clone(),
    input,                           // Formatted conversation items
    tools,                           // Tool schemas
    tool_choice: "auto".to_string(),
    parallel_tool_calls,
    reasoning,                       // Reasoning effort + summary
    stream: true,
    include: vec!["reasoning.encrypted_content".to_string()],
    service_tier,
    prompt_cache_key: Some(thread_id.to_string()),
    text,                            // Output schema / verbosity
    client_metadata,
}
```

### 8.3 Transport Selection

**File:** `codex/codex-rs/core/src/client.rs`

| Transport | Path | Usage |
|-----------|------|-------|
| **WebSocket** | `ResponsesWebsocketClient` | Preferred; maintains sticky routing |
| **HTTP SSE** | `ResponsesClient::stream_request()` | Fallback if WS fails or disabled |

### 8.4 HTTP Transport

**File:** `codex/codex-rs/codex-client/src/transport.rs`

Wraps `reqwest`:
- TLS, custom CA, retries, backoff
- Request compression (zstd)
- SSE parsing (`codex-api/src/sse.rs`)
- WebSocket via `tokio-tungstenite`

### 8.5 Auth

**File:** `codex/codex-rs/login/`

`AuthManager` handles:
- API keys
- ChatGPT session cookies / OAuth refresh
- Account info resolution

---

## 9. Response Streaming: From Provider to Rendered Output

### 9.1 Stream Back to TUI

Responses stream back as **server notifications** over the app-server event channel:

| Notification | Handler |
|--------------|---------|
| `AgentMessageDelta { delta }` | `ChatWidget::on_agent_message_delta(delta)` |
| `PlanDelta { delta }` | `ChatWidget::on_plan_delta(delta)` |
| `ItemStarted { item }` | Tool/exec lifecycle handlers |
| `TurnCompleted { .. }` | `ChatWidget::finalize_completed_assistant_message(...)` |

### 9.2 Markdown Stream Collector

**File:** `codex/codex-rs/tui/src/markdown_stream.rs`

`MarkdownStreamCollector`:
- Buffers raw markdown text from deltas
- Commits only up to the last newline, so incomplete markdown blocks aren't rendered prematurely
- On finalization, drains the remainder

### 9.3 Stream Controller

**File:** `codex/codex-rs/tui/src/chatwidget/streaming.rs`

`StreamController` / `PlanStreamController`:
- Accumulates committed lines into a **live stream tail cell** (in-flight `HistoryCell`)
- Drives a **commit animation** (`CommitTick`) that gradually reveals lines to simulate typing speed
- `AdaptiveChunkingPolicy` controls chunk size based on backpressure

### 9.4 Markdown Renderer

**File:** `codex/codex-rs/tui/src/markdown_render.rs`

Final rendering stage:
- Uses **`pulldown-cmark`** to parse CommonMark into events
- Converts events into styled **`ratatui::text::Line`** / **`Span`** output
- Supports:
  - Headings, bold, italic, strikethrough, blockquotes
  - Fenced code blocks with **syntax highlighting** (`syntect` + `two-face`)
  - Tables with Unicode box-drawing borders and intelligent column width shrinking
  - Local file links (displayed relative to cwd)
- Width-aware wrapping via `wrapping.rs`

### 9.5 History Cells

**File:** `codex/codex-rs/tui/src/history_cell/`

Committed output stored as `HistoryCell` trait objects:

| Cell Type | File | Description |
|-----------|------|-------------|
| `AgentMarkdownCell` | `messages.rs` | Finalized assistant message |
| `ExecCell` | `exec.rs` | Command output |
| `PlainHistoryCell` | `base.rs` | User message |
| `WebSearchCell` | `search.rs` | Web search results |
| `McpToolCallCell` | `mcp.rs` | MCP tool invocations |
| `HookCell` | `hook_cell.rs` | Hook executions |

The transcript is rendered by `ChatWidget::render()` into the terminal viewport.

### 9.6 Terminal Drawing

**File:** `codex/codex-rs/tui/src/tui.rs`

Terminal state management:
- Uses a **custom inline viewport** (not full alternate screen by default), so Codex renders above the shell prompt and scrollback stays in terminal history
- `Tui::draw()` / `Tui::draw_with_resize_reflow()` handle viewport sizing, history line insertion, and synchronized updates
- `crossterm::SynchronizedUpdate` bracketing prevents tearing during redraws

---

## 10. Tool Call Lifecycle

### 10.1 Tool Execution in Core

**File:** `codex/codex-rs/core/src/exec.rs`

When the LLM requests a tool call:

1. **ItemStarted** notification emitted
2. **ExecEnv** resolves the tool (shell, file read/write, grep, etc.)
3. **ExecPolicy** checks permissions (sandbox, approvals)
4. **Tool execution** — async subprocess or inline operation
5. **Output capture** — stdout/stderr, truncation, formatting
6. **ItemCompleted** notification with results

### 10.2 Tool Types

**Directory:** `codex/codex-rs/core/src/tools/`

| Tool | Description |
|------|-------------|
| `Shell` | Bash execution with sandboxing |
| `ReadFile` | Text file reading with offsets |
| `WriteFile` | Overwrite/append |
| `StrReplaceFile` | String replacement |
| `Glob` / `Grep` | File pattern matching / regex search |
| `WebSearch` | Web search |
| `FetchURL` | URL fetching |

### 10.3 MCP Tools

**File:** `codex/codex-rs/core/src/mcp.rs`

External MCP (Model Context Protocol) servers:
- Discovered from config (`mcpServers`)
- Wrapped as `MCPTool` with schema exposure
- Executed via `rmcp-client` transport
- Output subject to character budget enforcement

---

## 11. Approval System

### 11.1 Approval Types

**File:** `codex/codex-rs/tui/src/bottom_pane/approval_overlay.rs`

| Approval | Trigger |
|----------|---------|
| **Command execution** | Shell tool calls |
| **File changes** | WriteFile / StrReplaceFile |
| **Network policy** | External network access |
| **Permissions request** | Additional permission grants |

### 11.2 Approval Flow

```
Core exec_policy detects need for approval
    │
    ▼
ServerNotification::CommandExecutionRequestApproval
    │
    ▼
App routes to ChatWidget
    │
    ▼
BottomPane shows ApprovalOverlay
    │
    ▼
User selects: Accept / AcceptForSession / AcceptWithAmendment / Cancel
    │
    ▼
AppServerSession resolves the approval request
    │
    ▼
Core unblocks tool execution
```

### 11.3 Guardian / Auto-Review

**File:** `codex/codex-rs/core/src/guardian/`

Optional AI-powered safety review:
- `GuardianAssessmentEvent` for denied actions
- User can approve guardian-denied actions explicitly

---

## 12. Alternative UI Modes

### 12.1 Headless Mode

Non-interactive batch execution. Reads input, runs agent, prints output, exits. Uses the same app-server JSON-RPC but with a null TUI.

### 12.2 Wire / IDE Mode

**File:** `codex/codex-rs/app-server/src/`

JSON-RPC server over stdio or TCP for IDE integration:
- `initialize` / `initialized`
- `thread/start`, `thread/resume`, `thread/fork`
- `turn/start`, `turn/steer`, `turn/interrupt`
- `messages/list` — conversation history
- Subscription to server notifications

---

## 13. Complete Data Flow Summary

### 13.1 Full Pipeline

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              USER KEYSTROKE                                   │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  CROSSTERM EVENT STREAM                                                     │
│  codex-rs/tui/src/tui/event_stream.rs :: EventBroker + TuiEventStream       │
│  - EventStream polls stdin asynchronously                                   │
│  - Pause/resume when handing terminal to external editors                   │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  APP EVENT LOOP                                                             │
│  codex-rs/tui/src/app.rs :: App::run()                                      │
│  - select! over app_event_rx / tui_events / app_server_events               │
│  - Global shortcuts (Ctrl+L, Alt+Up/Down, Esc)                              │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  CHATWIDGET KEY HANDLER                                                     │
│  codex-rs/tui/src/chatwidget.rs :: handle_key_event()                       │
│  - Routes to ChatComposer for input editing                                 │
│  - Slash commands, @mentions, file search popups                            │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  CHAT COMPOSER SUBMIT                                                       │
│  codex-rs/tui/src/bottom_pane/chat_composer.rs                              │
│  - TextArea buffer → UserMessage                                            │
│  - AppEvent::CodexOp(AppCommand::Submit { items, thread_id })               │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  APPSERVER SESSION FACADE                                                   │
│  codex-rs/tui/src/app_server_session.rs :: turn_start()                     │
│  - ClientRequest::TurnStart { thread_id, input, model, effort, ... }        │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  APP-SERVER MESSAGE PROCESSOR                                               │
│  codex-rs/app-server/src/message_processor.rs                               │
│  - Routes to TurnRequestProcessor                                           │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  TURN REQUEST PROCESSOR                                                     │
│  codex-rs/app-server/src/request_processors/turn_processor.rs               │
│  - Delegates to ThreadManager.start_turn()                                  │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  CORE THREAD MANAGER / CODEX THREAD                                         │
│  codex-rs/core/src/thread_manager.rs / codex_thread.rs                      │
│  - Builds Prompt from history + tools + instructions                        │
│  - Creates ModelClientSession                                               │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  MODEL CLIENT SESSION                                                       │
│  codex-rs/core/src/client.rs :: ModelClientSession::stream()                │
│  - build_responses_request() → ResponsesApiRequest                          │
│  - WebSocket preferred, HTTP SSE fallback                                   │
│  - x-codex-turn-state sticky routing token                                  │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  CODEX API CLIENT                                                           │
│  codex-rs/codex-api/src/endpoint/responses.rs                               │
│  codex-rs/codex-api/src/endpoint/responses_websocket.rs                     │
│  - HTTP POST with auth headers, session metadata, compression               │
│  - SSE: text/event-stream parsing                                           │
│  - WS: tokio-tungstenite persistent connection                              │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼ ResponseEvent stream
┌─────────────────────────────────────────────────────────────────────────────┐
│  CORE STREAM PROCESSING                                                     │
│  codex-rs/core/src/codex_thread.rs                                          │
│  - Text deltas → AgentMessageDelta notifications                            │
│  - Tool calls → ItemStarted / ItemCompleted                                 │
│  - Tool execution → ExecEnv + ExecPolicy                                    │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼ ServerNotification
┌─────────────────────────────────────────────────────────────────────────────┐
│  TUI NOTIFICATION STREAM                                                    │
│  codex-rs/tui/src/app.rs :: handle_app_server_event()                       │
│  - Routes to ChatWidget handlers                                            │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  CHATWIDGET STREAM HANDLERS                                                 │
│  codex-rs/tui/src/chatwidget.rs                                             │
│  - on_agent_message_delta() → MarkdownStreamCollector                       │
│  - StreamController commit animation                                        │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  MARKDOWN RENDERER                                                          │
│  codex-rs/tui/src/markdown_render.rs                                        │
│  - pulldown-cmark → ratatui styled Line/Span                                │
│  - Tables, code blocks, syntax highlighting, links                          │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  HISTORY CELLS + TRANSCRIPT                                                 │
│  codex-rs/tui/src/history_cell/                                             │
│  - AgentMarkdownCell, ExecCell, PlainHistoryCell, etc.                      │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  TUI DRAW                                                                   │
│  codex-rs/tui/src/tui.rs :: Tui::draw()                                     │
│  - Inline viewport (preserves terminal scrollback)                          │
│  - SynchronizedUpdate bracketing                                            │
│  - History line insertion for scrollback persistence                        │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
      Terminal
```

### 13.2 Tool Call Data Flow

```
LLM response includes tool_call item
    │
    ▼
Core receives ItemStarted notification
    │
    ▼
CodexThread resolves tool via exec_env
    │
    ▼
ExecPolicy checks permissions (sandbox, approval)
    │
    ├──► Approval required → ServerNotification::CommandExecutionRequestApproval
    │        → TUI shows approval overlay
    │        → User response → core unblocks
    │
    ▼
Tool execution (subprocess or inline)
    │
    ▼
Output captured, truncated, formatted
    │
    ▼
ItemCompleted notification with results
    │
    ▼
TUI renders ExecCell with output
    │
    ▼
Results added to conversation history → next LLM call
```

### 13.3 Key Architectural Decisions

1. **crossterm + ratatui stack:** Input handling uses crossterm (raw mode, event stream); rendering uses ratatui (styled lines, widgets, layouts). They meet at the terminal buffer level.

2. **App-server JSON-RPC boundary:** Decouples TUI from core logic, enabling multiple frontends (TUI, headless, IDE wire protocol) without changing agent logic.

3. **Inline viewport rendering:** Codex draws above the shell prompt rather than using the full alternate screen. This preserves native terminal scrollback history.

4. **WebSocket preferred with HTTP fallback:** ModelClientSession lazily opens a WebSocket and reuses it across a turn. If it fails, the session falls back to HTTP SSE for the remainder of the session.

5. **Markdown streaming with commit animation:** MarkdownStreamCollector newline-gates output to avoid rendering broken markdown. StreamController animates committed lines to simulate natural typing speed.

6. **Sticky turn-state routing:** The `x-codex-turn-state` header/token ensures all requests within a single turn route to the same backend instance.

7. **Pause/resume event stream:** The EventBroker drops and recreates the crossterm EventStream when handing the terminal to external editors, preventing stdin stealing.

---

## Appendix A: Key Files Reference

| Concern | File | Key Class / Function |
|---------|------|---------------------|
| Binary entry | `codex/codex-rs/tui/src/main.rs` | `main()` |
| TUI init | `codex/codex-rs/tui/src/lib.rs` | `run_main()` |
| App event loop | `codex/codex-rs/tui/src/app.rs` | `App::run()` |
| Event stream | `codex/codex-rs/tui/src/tui/event_stream.rs` | `EventBroker`, `TuiEventStream` |
| Terminal draw | `codex/codex-rs/tui/src/tui.rs` | `Tui::draw()` |
| App server session | `codex/codex-rs/tui/src/app_server_session.rs` | `AppServerSession::turn_start()` |
| ChatWidget | `codex/codex-rs/tui/src/chatwidget.rs` | `ChatWidget` |
| Chat composer | `codex/codex-rs/tui/src/bottom_pane/chat_composer.rs` | `ChatComposer` |
| Markdown stream | `codex/codex-rs/tui/src/markdown_stream.rs` | `MarkdownStreamCollector` |
| Markdown render | `codex/codex-rs/tui/src/markdown_render.rs` | `render_markdown_to_lines()` |
| Streaming controller | `codex/codex-rs/tui/src/chatwidget/streaming.rs` | `StreamController` |
| History cells | `codex/codex-rs/tui/src/history_cell/mod.rs` | `HistoryCell` trait |
| App server processor | `codex/codex-rs/app-server/src/message_processor.rs` | `MessageProcessor` |
| Turn processor | `codex/codex-rs/app-server/src/request_processors/turn_processor.rs` | `TurnRequestProcessor` |
| Thread manager | `codex/codex-rs/core/src/thread_manager.rs` | `ThreadManager` |
| Codex thread | `codex/codex-rs/core/src/codex_thread.rs` | `CodexThread` |
| Model client | `codex/codex-rs/core/src/client.rs` | `ModelClient`, `ModelClientSession` |
| Client common | `codex/codex-rs/core/src/client_common.rs` | `Prompt`, `ResponseEvent` |
| Responses API | `codex/codex-rs/codex-api/src/endpoint/responses.rs` | `ResponsesClient::stream_request()` |
| Responses WebSocket | `codex/codex-rs/codex-api/src/endpoint/responses_websocket.rs` | `ResponsesWebsocketClient` |
| HTTP transport | `codex/codex-rs/codex-client/src/transport.rs` | `ReqwestTransport` |
| Exec engine | `codex/codex-rs/core/src/exec.rs` | `exec_command()` |
| Exec policy | `codex/codex-rs/core/src/exec_policy.rs` | `ExecPolicy` |
| Tools | `codex/codex-rs/core/src/tools/` | Tool implementations |
| MCP integration | `codex/codex-rs/core/src/mcp.rs` | `McpClient`, `MCPTool` |
| Approval overlay | `codex/codex-rs/tui/src/bottom_pane/approval_overlay.rs` | `ApprovalOverlay` |
| Guardian | `codex/codex-rs/core/src/guardian/` | Guardian safety review |
| Login/auth | `codex/codex-rs/login/` | `AuthManager` |
| Protocol types | `codex/codex-rs/protocol/` | `ThreadId`, `ResponseItem`, etc. |

---

*End of analysis.*
