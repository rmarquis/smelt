# Smelt Architectural Analysis: Keystroke to LLM Request to Rendered Output

> **Scope:** This document exhaustively maps every code path that transforms a user keystroke into an LLM API request and back to rendered output in the smelt codebase. It covers `src/`, `runtime/`, and all relevant crates (`tui`, `core`, `engine`, `protocol`, `term`, `edit`, `buffer`).
> 
> **Generated:** 2026-05-18 by Kimi Code CLI via multi-agent codebase exploration.

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Entry Point and Bootstrap Sequence](#2-entry-point-and-bootstrap-sequence)
3. [Keystroke Capture and the TUI Event Loop](#3-keystroke-capture-and-the-tui-event-loop)
4. [Input Processing: From Key Event to Submit](#4-input-processing-from-key-event-to-submit)
5. [Engine Turn Lifecycle: From Submit to LLM Request](#5-engine-turn-lifecycle-from-submit-to-llm-request)
6. [Provider Layer: HTTP Request Building and Streaming](#6-provider-layer-http-request-building-and-streaming)
7. [Response Streaming: From SSE to Rendered Output](#7-response-streaming-from-sse-to-rendered-output)
8. [Tool Call Lifecycle](#8-tool-call-lifecycle)
9. [Transcript Rendering Pipeline](#9-transcript-rendering-pipeline)
10. [Lua Runtime Integration](#10-lua-runtime-integration)
11. [Headless Mode Alternative Path](#11-headless-mode-alternative-path)
12. [Complete Data Flow Summary](#12-complete-data-flow-summary)

---

## 1. Executive Summary

Smelt is a **coding agent TUI** with a three-tier architecture:

| Tier | Crate | Responsibility |
|------|-------|---------------|
| **Frontend** | `smelt_tui` | Terminal raw-mode event loop, compositor, input handling, transcript rendering |
| **Core** | `smelt_core` | Session state, message history, Lua runtime, permissions, engine client |
| **Engine** | `smelt_engine` | Async actor that talks to LLM providers, handles streaming, tool dispatch |
| **Protocol** | `smelt_protocol` | Serializable wire types between engine and UI |

The high-level flow is:

```
User keystroke
    → crossterm EventStream
    → TuiApp event dispatch
    → PromptState input handling
    → Submit → process_input()
    → UiCommand::StartTurn → engine actor
    → Provider::chat() → HTTP POST + SSE
    → StreamDelta callbacks → EngineEvent
    → TUI engine event dispatch
    → StreamParser → BlockHistory rewrite
    → TranscriptProjection → parallel block layout
    → Window::render → GridSlice → Compositor diff
    → Terminal escape sequences
```

All message history lives in the engine; the TUI maintains a **read-only mirror** in `core.session.messages` updated via `EngineEvent::TurnComplete`. The transcript display is **decoupled** from the conversation history: the engine drives the stream, and the TUI projects it into terminal cells through an immutable block store + mutable sidecar state pattern.

---

## 2. Entry Point and Bootstrap Sequence

### 2.1 Binary Entry Point

**File:** `src/main.rs`  
**Function:** `main()` (line 122)

The binary starts with a **two-pass Lua initialization** so that `early.lua` can declare extra CLI flags before `argv` is parsed:

```rust
let mut lua_runtime = tui::lua::LuaRuntime::new();   // 1. Build runtime
lua_runtime.load_early_init();                        // 2. ~/.config/smelt/early.lua
lua_runtime.load_project_early_init(&cwd);            // 3. .smelt/early.lua
// ... parse CLI flags registered by early.lua ...
```

After CLI parsing, the full Lua stack is loaded:

```rust
lua_runtime.load_autoload();          // 4. Require bundled modules (tools, commands, plugins)
lua_runtime.load_user_config();       // 5. ~/.config/smelt/init.lua
lua_runtime.load_global_plugins();    // 6. ~/.config/smelt/plugins/*.lua
lua_runtime.load_project_config(&cwd); // 7. .smelt/init.lua + plugins
```

### 2.2 Startup Resolution

**File:** `src/startup.rs`  
**Function:** `resolve()` (line 80)

Resolves the active model via priority chain:
1. CLI `--model`
2. Lua `smelt.defaults{model=...}`
3. Cached `selected_model` from previous session
4. First model in config

Then resolves API base, API key, provider type, and assembles `ResolvedStartup`.

### 2.3 Engine Construction

**File:** `src/main.rs` (line 395)

```rust
let engine_handle = engine::start(
    engine::EngineConfig { api, model, auxiliary, instructions, ... },
    dispatcher,  // McpDispatcher wrapping McpManager
);
```

`engine::start()` (in `crates/engine/src/lib.rs`) spawns the engine actor:
- Creates three unbounded mpsc channels: `cmd_tx/rx` (UI→engine), `event_tx/rx` (engine→UI), `host_tx/rx` (engine→host callbacks)
- Spawns `tokio::spawn(agent::engine_task(...))`

### 2.4 TUI App Construction

**File:** `src/main.rs` (line 519)

```rust
let mut app = tui::app::TuiApp::new(
    app_config, engine_handle, permissions, shared_session,
    startup_auth_error, lua_runtime, project_trust, clock, env,
);
app.run(ctx_rx, args.message).await;
```

`TuiApp::new()` (`crates/tui/src/app.rs`, line 243):
1. Creates the prompt buffer with `PromptBufferParser` (renders attachment markers as styled pills)
2. Seeds reactive cells (`tokens_used`, `model`, `vim_mode`, etc.)
3. Loads the transcript projection cache
4. Sets up the compositor with layout tree

---

## 3. Keystroke Capture and the TUI Event Loop

### 3.1 Terminal Envelope Setup

**File:** `crates/tui/src/term_setup.rs`

`TuiTerminal::claim()` switches the terminal into raw mode:
- Enables raw mode (`terminal::enable_raw_mode()`)
- Enters alternate screen
- Enables bracketed paste, focus change reporting, mouse capture
- Hides the hardware cursor

This causes the terminal to deliver individual key events rather than line-buffered input.

### 3.2 Main Application Loop

**File:** `crates/tui/src/app.rs`  
**Function:** `TuiApp::run()` (lines 753–1166)

```rust
let mut term_events = EventStream::new();  // crossterm async event stream
let mut sigwinch = tokio::signal::unix::signal(SignalKind::window_change());
```

The loop has two phases per iteration:

**Phase A: Housekeeping** (lines 860–986)
1. `tick_timers()` — Fire Lua timer callbacks
2. `publish_diff_cells()` / `drain_cells_pending()` — Reactive cell updates
3. `drive_lua_tasks()` — Resume parked Lua coroutines
4. `drain_finished_blocks()` — Notify Lua of completed blocks
5. `pump_lua()` — Process Lua callback queue
6. `ui.dispatch_tick()` — Deliver tick events to compositor leaves
7. `core.engine.try_recv()` — Drain engine events
8. Handle deferred permission dialogs
9. `tick_statusline()` — Recompute custom statusline items
10. `render_normal(agent_running)` — Render one frame

**Phase B: Blocking `tokio::select!`** (lines 1001–1152)

| Branch | Source | Handler |
|--------|--------|---------|
| `term_events` | `EventStream::new()` | `dispatch_terminal_event(ev)` |
| `core.engine.recv()` | Engine backend | `dispatch_engine_event(ev)` |
| `host_rx.recv()` | Engine host callbacks | `dispatch_host_call(call)` |
| `lua_wakeup_rx.recv()` | Lua async tasks | `flush_lua_callbacks(); drive_lua_tasks()` |
| `auto_reload_rx.recv()` | Filesystem watcher | `reload_lua()` |
| `exec handle rx` | Shell-exec child | `append_exec_output()` / `finish_exec()` |
| `tokio::time::sleep(...)` | Frame timer | `tick_drag_autoscroll(); render_normal()` |
| `sigwinch.recv()` | OS resize signal | `handle_resize(w, h)` |

Terminal events are **coalesced** in a small inner loop using `event::poll(Duration::ZERO)` so rapid key repeats are batched into one render.

### 3.3 Terminal Event Dispatch

**File:** `crates/tui/src/app/events.rs`  
**Function:** `dispatch_terminal_event()` (line 13)

Priority dispatch:
1. `FocusGained` / `FocusLost` → update `term_focused`
2. Global chords (`Shift+Tab` → cycle mode, `Ctrl+T` → cycle reasoning, `Ctrl+L` → redraw)
3. Overlay/modal focus → `run_key_cascade()` or `cmdline_handle_key()` (swallows key)
4. Running shell exec + `Ctrl+C` → kill child process
5. Route to `handle_event_idle()` or `handle_event_running()` depending on `self.agent`

**Shared dispatch layer:** `dispatch_common()` (line 184)
1. `Event::Paste` → clear ghost text
2. `Event::Resize` → `handle_resize()`
3. `Event::Mouse` → `handle_mouse()`
4. Buffer-local Lua keymaps (`ui.dispatch_event(...)`)
5. Global Lua keymaps (`lua.run_keymap()`), with multi-key chord buffering
6. Pane chord (`Ctrl-W`) navigation
7. `:` character → opens cmdline (unless in insert mode)
8. Content focus → transcript viewer keys
9. Overlay keys (dismiss notification)

---

## 4. Input Processing: From Key Event to Submit

### 4.1 Prompt State Machine

**File:** `crates/tui/src/input/mod.rs`  
**Struct:** `PromptState` (line 84)

`PromptState` owns the side-cars for the prompt edit surface:
- `store: Arc<Mutex<AttachmentStore>>` — image/file attachments
- `completer: Option<CompleterSession>` — active Tab-completion
- `stash: Option<InputSnapshot>` — Ctrl+S stash/unstash
- `from_paste: bool` — tracks paste origin (skips `!` shell escape)

The **canonical text buffer** lives in the compositor (Buffer ID: `PROMPT_EDIT_BUF`, Window ID: `PROMPT_WIN`), not in `PromptState`.

### 4.2 Prompt Event Handling

**File:** `crates/tui/src/input/mod.rs`  
**Function:** `PromptState::handle_event()` (line 993)

Priority inside the prompt:
1. **Completer** — if open, arrows/Tab/Enter navigate it
2. **Vim bridge** — `dispatch_vim()` bridges to `smelt_edit` vim engine
3. **Paste** — `Event::Paste(data)` handles image paths, clipboard images, or plain text
4. **Ctrl-X chords** — `C-x C-e` opens `$EDITOR`
5. **Keymap lookup** — `keymap::lookup(code, modifiers, &key_ctx)` → `execute_key_action()`
6. **Character insertion** — plain `KeyCode::Char(c)`

### 4.3 Key Actions and Submit

**File:** `crates/tui/src/input/mod.rs`  
**Function:** `execute_key_action()` (line 526)

Large match on `KeyAction`:
- `Submit` → `Action::Submit { content, display }` — builds `Content` with images, clears buffer
- Navigation: `MoveLeft`, `MoveRight`, `MoveUp`, `MoveDown`, `MoveWordLeft`, etc.
- Editing: `Backspace`, `DeleteWord`, `KillToEndOfLine`, `Yank`, `Undo`
- Selection: `Shift+arrows` extend `selection_anchor`
- Clipboard: `CopySelection`, `CutSelection`, `ClipboardImage`

All text mutations go through `smelt_buffer::text` primitives (per `AGENTS.md` conventions):
```rust
ctx.buf.text_mut().insert(ctx.win.cpos, '\n');
ctx.buf.text_mut().replace_range(start..end, "");
```

### 4.4 Input Dispatch

**File:** `crates/tui/src/app/events.rs`  
**Function:** `process_input()` (line 625)

After `Action::Submit`:
1. Push raw text to `input_history`
2. Normalize `:` prefix to `/`
3. Call `crate::commands::run_command(self, &dispatch_input)`
   - `!cmd` → spawn shell escape (`start_shell_escape()`)
   - `/name arg` → run Lua-registered command handler
4. If not a known command and not shell escape → `InputOutcome::StartAgent`

**File:** `crates/tui/src/commands.rs`  
**Function:** `run_command()` (line 70)

Commands parsed into `ParsedCommand::Shell` or `ParsedCommand::Slash`. Slash commands dispatch through Lua runtime (`app.lua.run_command(&name, arg)`).

### 4.5 Starting the Agent Turn

**File:** `crates/tui/src/app/agent.rs`  
**Function:** `start_agent()`

When `InputOutcome::StartAgent` is returned:
1. Build `Content` from prompt text + attachments
2. Clear prompt buffer
3. Send `UiCommand::StartTurn` to engine with:
   - `turn_id`
   - `content` (user message)
   - `mode`, `model`, `reasoning_effort`
   - `history` (current session messages)
   - `tools` (Lua-registered + core tool definitions)
   - `system_prompt` (assembled from AGENTS.md + skill section + overrides)

---

## 5. Engine Turn Lifecycle: From Submit to LLM Request

### 5.1 Engine Actor

**File:** `crates/engine/src/agent.rs`  
**Function:** `engine_task()` (line 29)

Single `tokio::select!` loop over `cmd_rx.recv()`:
- `UiCommand::StartTurn(payload)` → build `Turn` and run it
- `UiCommand::Compact` / `GenerateTitle` / `Btw` / `EngineAsk` → spawn background one-shot tasks
- `UiCommand::SetModel` / `ReloadAgentConfig` / `Cancel` → update config or cancel active turn

### 5.2 Turn Construction

**File:** `crates/engine/src/agent.rs` (lines 56–100)

When `StartTurn` arrives:
1. Build `Provider` from config + overrides
2. Resolve system prompt (priority: TUI override → `config.system_prompt_override` → `build_system_prompt_full()`)
3. Construct `Turn` struct with:
   - `provider`, `dispatcher`, `cmd_rx`, `event_tx`, `host_tx`
   - `cancel: CancellationToken::new()`
   - `messages: Vec::new()`
   - `tools` (from TUI)

### 5.3 Turn Run Loop

**File:** `crates/engine/src/agent.rs`  
**Function:** `Turn::run()` (line 799)

```rust
loop {
    // 1. Recompute visible tool definitions
    let tool_defs = self.visible_tool_defs();
    
    // 2. Call LLM
    let response = self.call_llm(&tool_defs).await?;
    
    // 3. No tool calls? → push assistant message, emit TurnComplete, return
    if response.tool_calls.is_empty() {
        self.push_assistant_message(&response);
        self.emit_turn_complete();
        return;
    }
    
    // 4. Tool calls? → classify, execute, collect results, loop
    let plan = self.classify_tools(&response.tool_calls)?;
    self.execute_concurrent(&plan).await?;
    self.run_sequential(&plan).await?;
    self.collect_results(&plan)?;
}
```

### 5.4 History Assembly

**File:** `crates/engine/src/agent.rs` (within `call_llm`)

Messages vector built as:
```
[system_prompt, …history_messages, user_content]
```

Where `history` comes from `StartTurnPayload.history` (the TUI's current session messages).

### 5.5 LLM Call

**File:** `crates/engine/src/agent.rs`  
**Function:** `Turn::call_llm()` (line 1632)

```rust
let response = self.provider.chat(
    &self.model,
    &messages,
    tool_defs,
    ChatOptions { temperature, top_p, top_k, ... },
    &self.cancel,
    |delta| { /* on_delta callback */ },
).await?;
```

The `on_delta` closure emits `EngineEvent::TextDelta`, `EngineEvent::ThinkingDelta`, and `EngineEvent::ToolArgsDelta` in real-time as SSE chunks arrive.

---

## 6. Provider Layer: HTTP Request Building and Streaming

### 6.1 Provider Abstraction

**File:** `crates/engine/src/provider/mod.rs`  
**Struct:** `Provider` (line 436)

`Provider` is a thin wrapper around `reqwest::Client`. Per `ProviderKind`, it chooses URL + JSON body builder:

| ProviderKind | Endpoint | Body builder |
|---|---|---|
| `OpenAiCompatible` | `{api_base}/chat/completions` | `chat_completions::build_body` |
| `OpenAi` | `{api_base}/responses` | `openai::build_body` |
| `Codex` | `codex::CODEX_API_ENDPOINT` | `openai::build_body` (with tweaks) |
| `Anthropic` / `AnthropicCompatible` | `{api_base}/messages` | `anthropic::build_body` |
| `Copilot` | proxy from token | `chat_completions::build_body` |

**Auth headers:**
- OpenAI-compatible: `Authorization: Bearer {api_key}`
- Anthropic: `x-api-key` + `anthropic-version`
- Codex: bearer via `codex::ensure_access_token_full`
- Copilot: bearer via `copilot::ensure_access_token_full`

### 6.2 Request Body Building

**Files:**
- `crates/engine/src/provider/chat_completions.rs` — OpenAI-compatible / Copilot
- `crates/engine/src/provider/openai.rs` — Native OpenAI / Codex
- `crates/engine/src/provider/anthropic.rs` — Anthropic

Each builder constructs the provider-specific JSON body from:
- `messages` (with role/content mapping per provider)
- `tools` (JSON Schema function definitions)
- `model`, `temperature`, `top_p`, `top_k`
- `stream: true` (always enabled)

### 6.3 HTTP Transport

**File:** `crates/engine/src/provider/mod.rs`  
**Function:** `Provider::chat()` (line 436)

```rust
let resp = self.client.post(&url)
    .headers(auth_headers)
    .json(&body)
    .send()
    .await?;
```

Then delegates to provider-specific `read_stream()`.

### 6.4 SSE Parsing

**File:** `crates/engine/src/provider/sse.rs`  
**Function:** `read_events()`

Drains `reqwest` response bytes into a buffer and parses `data: …` lines into `serde_json::Value` events. Cancellation checked every chunk via `tokio::select!`.

### 6.5 Stream State Machines

Each provider implements `read_stream(resp, cancel, on_delta)`:

**OpenAI-compatible / Copilot** (`chat_completions.rs`):
- Accumulates into `StreamState { content, reasoning, tool_calls: HashMap<usize, ...>, usage }`
- Deltas: `delta.content`, `delta.reasoning_content`, `delta.tool_calls[].function.arguments`

**Native OpenAI / Codex** (`openai.rs`):
- Event types: `response.output_text.delta`, `response.function_call_arguments.delta`, `response.reasoning.delta`

**Anthropic** (`anthropic.rs`):
- Events: `content_block_start` / `content_block_delta` with `text_delta`, `thinking_delta`, `input_json_delta`

---

## 7. Response Streaming: From SSE to Rendered Output

### 7.1 Delta Callback

**File:** `crates/engine/src/agent.rs` (within `call_llm`)

```rust
let on_delta = |delta: StreamDelta| {
    match delta {
        StreamDelta::Text(t) => self.emit(EngineEvent::TextDelta { delta: t }),
        StreamDelta::Thinking(t) => self.emit(EngineEvent::ThinkingDelta { delta: t }),
        StreamDelta::ToolArgs { call_id, tool_name, delta } => {
            self.emit(EngineEvent::ToolArgsDelta { call_id, tool_name, delta })
        }
    }
};
```

### 7.2 Engine → TUI Event Delivery

**File:** `crates/core/src/engine_client.rs`  
**Struct:** `EngineClient`

- `send(cmd)` — fire-and-forget to engine
- `recv()` — async; returns `EngineEvent`; **blocked by `Confirms`** when a permission dialog is open
- `try_recv()` — non-blocking; returns `Err(Empty)` when confirms pending

### 7.3 TUI Engine Event Dispatch

**File:** `crates/tui/src/app/engine_events.rs`  
**Function:** `dispatch_engine_event()`

Key event handlers:
- `Ready` → mark engine ready
- `TextDelta { delta }` → `self.parser.append_streaming_text(&mut self.transcript.history, delta)`
- `ThinkingDelta { delta }` → `self.parser.append_streaming_thinking(&mut self.transcript.history, delta)`
- `ToolStarted { call_id, tool_name, args }` → create `ToolState` with `Status::Pending`
- `ToolOutput { call_id, chunk }` → append to tool output buffer
- `ToolFinished { call_id, result, elapsed_ms }` → `self.finish_tool(&call_id, ...)`
- `TurnComplete { turn_id, messages, meta }` → `self.set_history(messages)` + update session metadata
- `RequestPermission { request_id, ... }` → queue confirm dialog
- `ToolDispatch { request_id, call_id, tool_name, args }` → `self.handle_tool_call(request_id, call_id, tool_name, args)`

### 7.4 Stream Parsing into Blocks

**File:** `crates/core/src/content/stream_parser.rs`  
**Struct:** `StreamParser`

`append_streaming_text(history, delta)` (line 136):
1. Iterates `delta` chars, builds `current_line`
2. On `\n`, calls `process_text_line`
3. `process_text_line` detects structural boundaries:
   - `` ```lang `` → enters `in_code_block = Some(lang)`
   - `` ``` `` → exits code block, flushes
   - `|` prefix → accumulates table rows
   - empty line → flushes paragraph
4. `sync_streaming_text` (line 166) rewrites the in-flight block in `BlockHistory` using `history.rewrite(id, block)`
5. `flush_streaming_text` (line 325) finalizes to `Status::Done` on turn end

### 7.5 Thinking Text

**File:** `crates/core/src/content/stream_parser.rs`

`append_streaming_thinking()` accumulates reasoning text into `Block::Thinking` blocks. These are rendered with a collapsible UI and distinct styling.

---

## 8. Tool Call Lifecycle

### 8.1 Tool Classification

**File:** `crates/engine/src/agent.rs`  
**Function:** `Turn::classify_tools()` (line 1023)

For each `ToolCall` returned by the LLM:
1. Look up whether it's a **Lua tool** (`ToolDef` from TUI) or **core tool** (`ToolDispatcher`)
2. Evaluate permission hooks:
   - Lua tools with `hooks.any()` → emit `EngineEvent::ToolHooksRequest` → await `UiCommand::ToolHooksResponse`
   - Core tools → `dispatcher.evaluate_hooks(name, args, mode)` → returns `Decision::Allow | Deny | Ask | Error`

### 8.2 Execution Plan

**File:** `crates/engine/src/agent.rs`  
**Struct:** `ToolExecutionPlan` (line 416)

- `ready` — core tools allowed, run concurrently via `FuturesUnordered`
- `pending_perms` — core tools awaiting user permission (`RequestPermission`)
- `pending_tools` — Lua tools dispatched to TUI (`ToolDispatch`), await `ToolResult`
- `sequential_tools` — tools marked `Sequential` (run one-at-a-time after concurrent work)

### 8.3 Concurrent Execution

**File:** `crates/engine/src/agent.rs`  
**Function:** `Turn::execute_concurrent()` (line 1164)

`tokio::select!` loop over:
- `cmd_rx.recv()` — handles `PermissionDecision`, `ToolHooksResponse`, `ToolResult`, `CallCoreTool`, `Cancel`, steering
- `futs.next()` — core tool completions
- `side_futs.next()` — `CallCoreTool` completions

### 8.4 Sequential Execution

**File:** `crates/engine/src/agent.rs`  
**Function:** `Turn::run_sequential()` (line 1523)

Dispatches each sequential tool via `ToolDispatch` and blocks on `wait_for_tool_result(request_id)`.

### 8.5 Results Collection

**File:** `crates/engine/src/agent.rs`  
**Function:** `Turn::collect_results()` (line 1566)

1. Emits `ToolFinished` + pushes `Message::tool(call_id, content, is_error)` to history
2. Applies duplicate-result deduplication (`result_dedup::duplicate_of`)
3. Emits `Messages` snapshot
4. Loops back to `call_llm()`

### 8.6 Lua Tool Execution in TUI

**File:** `crates/tui/src/app/agent.rs`  
**Function:** `handle_tool_call()` (lines 341–373)

```rust
match self.lua.execute_tool(tool_name, args, request_id, call_id, env, now) {
    ToolExecResult::Immediate { content, is_error } => {
        self.core.engine.send(UiCommand::ToolResult { request_id, call_id, content, is_error });
    }
    ToolExecResult::Pending => {} // Result comes later via task runtime
}
```

**File:** `crates/core/src/lua/runtime.rs`  
**Function:** `execute_tool()` (lines 1324–1400)

1. Look up tool's `execute` handle in `LuaShared::tools`
2. Convert JSON args to Lua table
3. Build `ctx` table with `call_id`, `mode`, `session_id`, `session_dir`
4. Run `tools.middleware{before=...}` hooks
5. Spawn Lua coroutine via `LuaTaskRuntime::spawn()`
6. Single-step immediately:
   - Completes synchronously → `ToolExecResult::Immediate`
   - Yields → `ToolExecResult::Pending`

### 8.7 Core Tool Execution

**File:** `crates/engine/src/tools/mod.rs`  
**Trait:** `ToolDispatcher`

```rust
pub trait ToolDispatcher: Send + Sync {
    fn definitions(&self) -> Vec<ToolDefinition>;
    fn dispatch(&self, name: &str, args: HashMap<String, Value>, ctx: &ToolContext) -> Option<ToolFuture>;
    fn evaluate_hooks(&self, name: &str, args: &HashMap<String, Value>, mode: AgentMode) -> Option<ToolHooks>;
}
```

**Implementations:**
- `EmptyDispatcher` — no-op
- `McpDispatcher` (`crates/core/src/mcp/dispatcher.rs`) — dispatches to MCP servers

### 8.8 Sending Results Back to Engine

**File:** `crates/core/src/lua/api/tools.rs` (lines 264–280)

Lua tools call `smelt.tools.resolve(request_id, call_id, { content = "...", is_error = false })` which sends `UiCommand::ToolResult` back to the engine.

Core tools return `ToolResult { content, is_error, metadata }` directly from their `dispatch()` future.

---

## 9. Transcript Rendering Pipeline

### 9.1 Block Model

**File:** `crates/core/src/transcript_model.rs`

`BlockHistory` is an **append-only, content-addressed block store**:

```rust
pub enum Block {
    User { text, image_labels },
    Thinking { content },
    Text { content },
    CodeLine { content, lang },
    ToolCall { call_id, name, summary, args },
    Exec { command, output },
    Compacted { summary },
}
```

Mutable runtime state lives in `BlockHistory.tool_states: HashMap<String, ToolState>`:
```rust
pub struct ToolState {
    pub status: ToolStatus,       // Pending | Confirm | Ok | Err | Denied
    pub elapsed: Option<Duration>,
    pub output: Option<ToolOutputRef>,
    pub render_cache: Option<(u16, RenderedLayout)>,
}
```

This separation allows **permanent per-block layout caches** — `Block::ToolCall` is immutable so its layout key never changes.

### 9.2 Frame Layout

**File:** `crates/tui/src/content/layout.rs`  
**Function:** `build_layout_tree()`

Produces a `LayoutTree` splitting the terminal vertically:
```
┌─────────────────────────┐
│       Transcript        │  ← Fill
├─────────────────────────┤
│      Prompt Above       │  ← Length(above_rows)
├─────────────────────────┤
│      Prompt Input       │  ← Length(input_rows)
├─────────────────────────┤
│      Prompt Below       │  ← Length(1)
├─────────────────────────┤
│       Statusline        │  ← Length(1)
└─────────────────────────┘
```

Prompt block is capped at half terminal height.

### 9.3 Render Loop

**File:** `crates/tui/src/app/render_loop.rs`  
**Function:** `TuiApp::render_normal()` (line 8)

Each frame:
1. `measure_prompt_rows` → compute `above_rows`, `input_rows`
2. `build_layout_tree` → `ui.set_layout(...)`
3. `sync_transcript_layer` → project transcript buffer
4. `sync_prompt_above_layer` / `sync_input_layer` / `sync_prompt_below_layer`
5. `refresh_status_bar`
6. `sync_completer_overlay`
7. `ui.render_with_paints(...)` → flush to stdout

### 9.4 Transcript Projection

**File:** `crates/tui/src/content/transcript_buf.rs`  
**Function:** `TranscriptProjection::project()` (line 88)

```rust
pub fn project(
    &mut self,
    buf: &mut Buffer,
    history: &mut BlockHistory,
    width: u16,
    show_thinking: bool,
    theme: &Theme,
    ephemeral: Option<&Buffer>,
    scroll_top: u16,
    viewport_rows: u16,
) -> ProjectOutput
```

**Cache check:** If `(generation, width, show_thinking, ephemeral_fingerprint)` hasn't changed, skip work.

**Parallel layout** (`block_buffers.rs` line 37):
```rust
pub fn ensure_many(&mut self, history, ids, keys, theme)
```
- For each block whose `LayoutKey` (width + view_state + content_hash) is not cached, spawn a **thread-scoped worker** (capped at 8 threads)
- Worker calls `layout_block_into(&mut buf, theme, block, tool_state, &lctx)`

**Per-block layout** (`transcript_parsers/mod.rs` line 33):
```rust
pub fn layout_block_into(buf, theme, block, state, ctx) -> Outcome {
    let mut col = LineBuilder::new(buf, theme, ctx.width);
    render_block(&mut col, block, state, width, show_thinking);
    col.finish();
}
```

`render_block` dispatches by variant:
- `User` → `user::render`
- `Thinking` → `thinking::render`
- `Text` → `text::render` → `markdown::render_markdown_inner`
- `CodeLine` → `code_line::render` (syntect syntax highlighting)
- `ToolCall` → `tool_call::render` → `tools::render_tool`
- `Exec` → `exec::render`
- `Compacted` → `compacted::render`

### 9.5 Stitching and Grid Dispatch

Back in `TranscriptProjection::project` (line 160):
```rust
for i in 0..n {
    let block_buf = self.cache.get(id, key);
    let gap = history.block_gap(i);
    // append gap blank lines
    // append each line of block_buf + highlights + decorations
}
```

**File:** `crates/edit/src/window.rs`  
**Function:** `Window::render()` (line 1380)

`Ui::render_with_paints` walks the layout tree and for each leaf:
1. Uses cached `WrappedLayout` (word-wrap of Buffer lines to window width)
2. For each screen row:
   - Maps `visual_row = scroll_top + row` → `logical_row` + wrap `chunk_idx`
   - Paints gutter, text cells with highlight spans, cursor line bg, selection bg, virtual text
3. If cursor shape is `Block`, overwrites cell at cursor position

### 9.6 Terminal Output

**File:** `crates/term/src/compositor.rs`  
**Function:** `Compositor::render_with()`

```rust
self.current.clear_all();
paint(&mut self.current, theme);  // writes all leaves into Grid

w.queue(BeginSynchronizedUpdate)?;
flush_diff(w, self.current.diff(&self.previous))?;
w.queue(EndSynchronizedUpdate)?;
w.flush()?;

self.current.swap_with(&mut self.previous);
```

- **Double-buffered diff:** Only changed cells emit `MoveTo` + color sets + `Print`
- Hardware cursor stays hidden; visible "cursor" is a styled cell painted into the grid

### 9.7 Tool Rendering

**File:** `crates/tui/src/content/transcript_parsers/tools.rs`  
**Function:** `render_tool()` (line 18)

1. **Header line:** colored pill (`⏺`) + tool name + styled summary + elapsed time
2. **Body (one of):**
   - Plugin-rendered: `replay_rendered(out, layout, inner_width)` — replays pre-baked `RenderedLayout`
   - Raw output: `print_tool_output` — plain text with 2-cell gutter, capped at 20 tail lines
   - Denied: no body

**Plugin pre-rendering** (`crates/tui/src/app/transcript.rs`, line 356):
```rust
fn prerender_tool_blocks(&mut self, width: u16) {
    // Walk ToolCall blocks
    // If render_cache missing or width-stale:
    //   layout = lua.render_tool_layout(&name, &args, output, ctx)
    //   state.render_cache = Some((width, rendered));
}
```

This runs on the **main thread** before parallel layout so workers never touch Lua.

---

## 10. Lua Runtime Integration

### 10.1 Two-Tier Architecture

| Tier | Crate | APIs |
|------|-------|------|
| **Host** | `smelt_core` | `tools`, `cmd`, `fs`, `process`, `task`, `timer`, `state`, `provider`, `mcp`, `clipboard`, `grep`, `fuzzy`, `mode`, `trust` |
| **UiHost** | `smelt_tui` | `win`, `buf`, `keymap`, `prompt`, `picker`, `theme`, `engine`, `overlay`, `render`, `vim`, `confirm`, `transcript` |

### 10.2 Bootstrap Sequence

**File:** `src/main.rs` (lines 147–200)

```rust
let mut lua_runtime = tui::lua::LuaRuntime::new();   // Build runtime, register APIs
lua_runtime.load_early_init();                        // ~/.config/smelt/early.lua
lua_runtime.load_project_early_init(&cwd);            // .smelt/early.lua
// ... parse CLI flags ...
lua_runtime.load_autoload();                          // Require bundled modules
lua_runtime.load_user_config();                       // ~/.config/smelt/init.lua
lua_runtime.load_global_plugins();                    // ~/.config/smelt/plugins/*.lua
lua_runtime.load_project_config(&cwd);                // .smelt/init.lua + plugins
```

**Phase system** (`crates/core/src/lua/shared.rs`):
- `Early` — restricted APIs (`builtins`, `cli`, `phase`, `provider` only)
- `Init` — full surface after `load_autoload()`
- `Running` — full surface; dynamic registration allowed

**Autoload directories** (`runtime/lua/smelt/`):
- `tools/` — `bash`, `read_file`, `edit_file`, `write_file`, `glob`, `grep`, `web_search`, `web_fetch`, `ask_user_question`, `load_skill`, `notebook_edit`
- `commands/` — `btw`, `color`, `compact`, `help`, `history_search`, `messages`, `model`, `quit`, `reflect`, `reload`, `session`, `settings`, `simplify`, `stats`, `theme`, `toggles`, `trust`
- `plugins/` — `background_commands`, `esc_chord`, `perf_panel`, `plan_mode`, `predict`
- `dialogs/` — `confirm`, `permissions`, `resume`, `rewind`

### 10.3 Tool Registration from Lua

**File:** `crates/core/src/lua/api/tools.rs`  
**Function:** `register()`

`smelt.tools.register` accepts:
- `name`, `execute` (required)
- `description`, `parameters` (JSON Schema)
- `summary`, `render`, `preview`, `decide`, `preflight`, `approval_patterns`
- `execution_mode` (`"concurrent"` / `"sequential"`)
- `override_core` — replaces core Rust tool of same name

Stores callbacks as `LuaHandle` (registry-key-backed) in `shared.tools: HashMap<String, ToolHandles>`.

### 10.4 Keymaps from Lua

**File:** `crates/tui/src/lua/api/keymap.rs`

```lua
smelt.keymap.set("n", "<C-g>", function() smelt.notify("ctrl-g") end)
```

Stored in `shared.keymaps: HashMap<(mode, chord), LuaHandle>`. Chords canonicalized to nvim-style (`<C-g>`, `<Space>`, etc.).

**Dispatch** (`crates/core/src/lua/runtime.rs`):
```rust
pub fn run_keymap(&self, chord: &str, current_mode: &str, chord_ctx: &KeyChordContext) -> KeymapResult
```

### 10.5 Lua ↔ App Bridge

**File:** `crates/tui/src/lua/app_ref.rs`

Thread-local raw pointers installed at every Lua-entry boundary:
- `APP` — `*mut TuiApp` for UiHost-tier bindings
- `CORE_PTR` — `*mut Core` for Host-tier bindings

```rust
thread_local! {
    static APP: RefCell<Option<*mut TuiApp>> = ...;
}
```

Safety: Lua is single-threaded, so reborrowing is always exclusive.

### 10.6 Task Runtime and Yielding

**File:** `crates/core/src/lua/task.rs`

Lua tools can yield using:
- `smelt.sleep(ms)` — parks coroutine, resumed by tick loop
- `smelt.task.external(start)` — allocates external task id, parks until `smelt.task.resume(id, value)`
- `smelt.tools.call(name, args)` — delegates to core tool asynchronously

`drive_tasks(now)` steps ready coroutines one at a time. Results collected as `TaskDriveOutput::ToolComplete`.

---

## 11. Headless Mode Alternative Path

### 11.1 Headless App Construction

**File:** `src/main.rs` (lines 455–490)

When `--headless` is passed:
```rust
let mut core = smelt_core::Core::new(
    app_config, engine_handle,
    smelt_core::FrontendKind::Headless,
    permissions, clock, env,
);
let sink = smelt_core::HeadlessSink::new(output_format, color_mode, args.verbose);
let mut headless = smelt_core::HeadlessApp::new(core, sink);
headless.run_oneshot(args.message.unwrap(), headless_cancel).await;
```

### 11.2 Headless Event Loop

**File:** `crates/core/src/headless_app.rs`

`HeadlessApp::run_oneshot()`:
1. Sends `UiCommand::StartTurn`
2. Loops on `core.engine.recv()`
3. Prints text/tools/tokens via `HeadlessSink`
4. Auto-approves permissions in `Yolo` mode
5. Exits after `TurnComplete` or `TurnError`

**Output formats:**
- `Text` — plain text with ANSI colors
- `Json` — NDJSON stream of events

---

## 12. Complete Data Flow Summary

### 12.1 Full Pipeline

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              USER KEYSTROKE                                   │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  TERMINAL RAW MODE                                                            │
│  crates/tui/src/term_setup.rs :: TuiTerminal::claim()                        │
│  - enable_raw_mode(), alternate screen, bracketed paste, mouse capture       │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  CROSSTERM EVENT STREAM                                                       │
│  crates/tui/src/app.rs :: run() → EventStream::new()                         │
│  - Yields KeyEvent, MouseEvent, Paste, Resize, Focus                         │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  EVENT DISPATCH                                                               │
│  crates/tui/src/app/events.rs :: dispatch_terminal_event()                   │
│  - Global chords → mode toggle, redraw                                         │
│  - Overlay/modal → run_key_cascade() / cmdline_handle_key()                  │
│  - dispatch_common() → Lua keymaps, pane chords, cmdline open                │
│  - handle_event_idle() / handle_event_running()                              │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  PROMPT INPUT STATE MACHINE                                                   │
│  crates/tui/src/input/mod.rs :: PromptState::handle_event()                  │
│  1. Completer navigation                                                       │
│  2. Vim bridge (smelt_edit)                                                    │
│  3. Paste handling (images, clipboard)                                         │
│  4. Ctrl-X chords (C-x C-e → $EDITOR)                                         │
│  5. Keymap lookup → execute_key_action()                                     │
│  6. Character insertion                                                        │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  SUBMIT                                                                       │
│  crates/tui/src/input/mod.rs :: Action::Submit { content, display }          │
│  - Builds Content with attachments                                             │
│  - Clears prompt buffer                                                        │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  INPUT DISPATCH                                                               │
│  crates/tui/src/app/events.rs :: process_input()                             │
│  - Slash commands (/quit, /model, etc.) → Lua handlers                       │
│  - Shell escapes (!cmd) → spawn child process                                │
│  - Plain text → InputOutcome::StartAgent                                     │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  START AGENT TURN                                                             │
│  crates/tui/src/app/agent.rs :: start_agent()                                │
│  - Assembles StartTurnPayload { turn_id, content, mode, model,               │
│    reasoning_effort, history, tools, system_prompt }                          │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼ UiCommand::StartTurn
┌─────────────────────────────────────────────────────────────────────────────┐
│  ENGINE ACTOR                                                                 │
│  crates/engine/src/agent.rs :: engine_task() → Turn::run()                   │
│  - Builds [system_prompt, …history, user_content]                            │
│  - call_llm() → Provider::chat()                                             │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼ HTTP POST + SSE
┌─────────────────────────────────────────────────────────────────────────────┐
│  PROVIDER LAYER                                                               │
│  crates/engine/src/provider/mod.rs :: Provider::chat()                       │
│  - build_body() per provider (OpenAI, Anthropic, Codex, Copilot)             │
│  - reqwest POST → sse::read_events() → provider::read_stream()               │
│  - StreamDelta → on_delta callback                                           │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼ EngineEvent::TextDelta / ThinkingDelta / ToolArgsDelta
┌─────────────────────────────────────────────────────────────────────────────┐
│  ENGINE → TUI EVENT STREAM                                                    │
│  crates/core/src/engine_client.rs :: EngineClient::recv()                    │
│  crates/tui/src/app/engine_events.rs :: dispatch_engine_event()              │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  STREAM PARSER                                                                │
│  crates/core/src/content/stream_parser.rs                                    │
│  - append_streaming_text() → process_text_line() → detect code blocks,       │
│    tables, paragraphs → sync_streaming_text() → BlockHistory::rewrite()      │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼ BlockHistory.generation bump
┌─────────────────────────────────────────────────────────────────────────────┐
│  TRANSCRIPT PROJECTION                                                        │
│  crates/tui/src/content/transcript_buf.rs :: TranscriptProjection::project() │
│  - Cache check (generation, width, show_thinking, ephemeral)                 │
│  - Parallel layout: ensure_many() → layout_block_into()                      │
│  - Stitch blocks into unified Buffer                                         │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  COMPOSITOR / GRID                                                            │
│  crates/edit/src/window.rs :: Window::render()                               │
│  crates/term/src/compositor.rs :: Compositor::render_with()                  │
│  - WrappedLayout → GridSlice → flush_diff()                                  │
│  - Only changed cells emit terminal escapes                                  │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼ ANSI escapes
┌─────────────────────────────────────────────────────────────────────────────┐
│  TERMINAL DISPLAY                                                             │
│  - Synchronized update begin/end                                              │
│  - MoveTo + SetForegroundColor + SetBackgroundColor + Print                  │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 12.2 Tool Call Data Flow

```
LLM response includes tool_calls
    │
    ▼
Engine::classify_tools() → ToolExecutionPlan
    │
    ├──► Core tools (MCP) ─────────────────────────────────────────────┐
    │      │                                                            │
    │      ▼                                                            │
    │   ToolDispatcher::dispatch() → ToolFuture → await result          │
    │      │                                                            │
    │      ▼ ToolResult { content, is_error }                           │
    │                                                                    │
    ├──► Lua tools ─────────────────────────────────────────────────────┤
    │      │                                                            │
    │      ▼ EngineEvent::ToolDispatch                                  │
    │   TUI::handle_tool_call() → lua.execute_tool()                    │
    │      │                                                            │
    │      ├──► Immediate → UiCommand::ToolResult ──────────────────────┤
    │      └──► Pending → Lua coroutine yields                          │
    │             │                                                      │
    │             ▼ smelt.tools.resolve() or task completes             │
    │                  → UiCommand::ToolResult ─────────────────────────┤
    │                                                                    │
    ▼                                                                    │
Engine::collect_results() ◄─────────────────────────────────────────────┘
    │
    ├──► Message::tool(call_id, content, is_error) → messages vector
    ├──► EngineEvent::ToolFinished → TUI updates ToolState
    └──► Loop back to call_llm()
```

### 12.3 Key Architectural Decisions

1. **Immutable blocks + mutable sidecars** (`BlockHistory` + `ToolState`) allow permanent per-block layout caches and parallel rendering.

2. **Parallel block layout** (up to 8 workers) renders transcript blocks independently; main thread only stitches. Plugin tool rendering is pre-baked on the main thread so workers never touch Lua.

3. **Double-buffered terminal diff** means only changed cells are emitted per frame, keeping the TUI responsive even with large transcripts.

4. **Engine actor pattern** isolates all LLM I/O in a single tokio task. The TUI communicates via async channels, never blocking the render loop.

5. **Lua as first-class citizen:** Tools, commands, keymaps, themes, dialogs, and plugins are all implemented in Lua. The Rust core provides primitives and performance-critical paths (rendering, buffer editing, HTTP).

6. **Reactive cells** (`smelt_core::Cells`) decouple state producers from consumers. Lua callbacks and Rust code publish values, subscribers fire after borrow release.

7. **Permission system** gates tool execution through `evaluate_hooks()` → `Decision::Allow | Ask | Deny`. The engine is paused while confirms are pending (`EngineClient::recv()` returns `pending()`).

---

## Appendix A: Key Files Reference

| Concern | File | Key Symbol |
|---------|------|-----------|
| Binary entry | `src/main.rs` | `main()` |
| Startup resolution | `src/startup.rs` | `resolve()` |
| TUI main loop | `crates/tui/src/app.rs` | `TuiApp::run()` |
| Event dispatch | `crates/tui/src/app/events.rs` | `dispatch_terminal_event()` |
| Prompt state | `crates/tui/src/input/mod.rs` | `PromptState::handle_event()` |
| Keymap table | `crates/tui/src/keymap.rs` | `static BINDINGS` |
| Render loop | `crates/tui/src/app/render_loop.rs` | `render_normal()` |
| Engine actor | `crates/engine/src/agent.rs` | `engine_task()`, `Turn::run()` |
| Provider dispatch | `crates/engine/src/provider/mod.rs` | `Provider::chat()` |
| SSE parsing | `crates/engine/src/provider/sse.rs` | `read_events()` |
| OpenAI stream | `crates/engine/src/provider/openai.rs` | `read_stream()` |
| Anthropic stream | `crates/engine/src/provider/anthropic.rs` | `read_stream()` |
| Tool dispatcher | `crates/engine/src/tools/mod.rs` | `ToolDispatcher` trait |
| MCP dispatcher | `crates/core/src/mcp/dispatcher.rs` | `McpDispatcher` |
| Core state | `crates/core/src/runtime.rs` | `Core` struct |
| Engine client | `crates/core/src/engine_client.rs` | `EngineClient` |
| Session | `crates/core/src/session.rs` | `Session` |
| Transcript model | `crates/core/src/transcript_model.rs` | `BlockHistory`, `Block` |
| Stream parser | `crates/core/src/content/stream_parser.rs` | `StreamParser` |
| Transcript projection | `crates/tui/src/content/transcript_buf.rs` | `TranscriptProjection::project()` |
| Block layout | `crates/tui/src/content/block_buffers.rs` | `ensure_many()` |
| Block renderers | `crates/tui/src/content/transcript_parsers/` | `layout_block_into()` |
| Tool rendering | `crates/tui/src/content/transcript_parsers/tools.rs` | `render_tool()` |
| Compositor | `crates/term/src/compositor.rs` | `Compositor::render_with()` |
| Window render | `crates/edit/src/window.rs` | `Window::render()` |
| Lua runtime | `crates/core/src/lua/runtime.rs` | `LuaRuntime` |
| Lua tools API | `crates/core/src/lua/api/tools.rs` | `smelt.tools.register` |
| Lua keymap API | `crates/tui/src/lua/api/keymap.rs` | `smelt.keymap.set` |
| Lua app bridge | `crates/tui/src/lua/app_ref.rs` | `install_app_ptr()` |
| Headless app | `crates/core/src/headless_app.rs` | `HeadlessApp::run_oneshot()` |
| Protocol events | `crates/protocol/src/event.rs` | `EngineEvent`, `UiCommand` |
| Protocol messages | `crates/protocol/src/message.rs` | `Message`, `ToolOutcome` |

---

*End of analysis.*
