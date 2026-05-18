# Smelt LLM Interaction Sourcemap

> **Document type**: Architectural analysis — exhaustive map of every code path that transforms a user keystroke into an LLM API request and back to rendered output.  
> **Sources inspected**: Rust (`crates/engine/`, `crates/core/`, `crates/protocol/`, `crates/tui/`) and Lua (`runtime/lua/smelt/`).  
> **Date**: 2026-05-13

---

## 1. Overview — The Interaction Lifecycle

A single user turn in Smelt is not a simple "send message → get reply" request. It is a **stateful loop** that can involve multiple LLM calls, tool executions (concurrent and sequential), permission dialogs, mid-turn user steering, history compaction, and automatic retries.

The high-level phases are:

```
┌─────────────────┐     ┌──────────────────┐     ┌─────────────────┐
│  1. USER INPUT  │────▶│ 2. TUI ASSEMBLY  │────▶│ 3. ENGINE TURN  │
│   (keystroke)   │     │ (prompt, tools,  │     │ (LLM loop +     │
│                 │     │  history, mode)  │     │  tool exec)     │
└─────────────────┘     └──────────────────┘     └─────────────────┘
                                                          │
┌─────────────────┐     ┌──────────────────┐             │
│ 5. RENDERED UI  │◀────│ 4. EVENT STREAM  │◀────────────┘
│  (transcript)   │     │ (deltas + tools) │
└─────────────────┘     └──────────────────┘
```

The loop inside phase 3 repeats until the LLM responds with **no tool calls**.

---

## 2. Phase 1 — User Input (TUI / Rust)

**Source**: `crates/tui/src/input/mod.rs` (Rust), `crates/tui/src/app/events.rs` (Rust)

### 2.1 Input Capture

The TUI runs a terminal event loop (`app.rs`). When the user is at the prompt:

1. **Keystrokes** go through `PromptState::handle_event` (`input/mod.rs`).
2. The prompt buffer uses a custom `PromptBufferParser` (`content/prompt_parser.rs`) that:
   - Wraps lines at terminal width.
   - Highlights attachments (`[img.png]`) and `@refs` in blue.
   - Highlights command prefixes (`/model`) and exec bangs (`!cmd`).
3. **Attachments** are stored as `\u{FFFC}` (object replacement character) in the raw source; the parser expands them to display labels.

### 2.2 Submit Action

When the user presses `Enter`, `PromptState::build_content` expands the raw buffer:

- **Text**: attachment markers are expanded to their textual representation.
- **Images**: deduplicated; each image becomes a `ContentPart::ImageUrl { url: data_url, label }`.
- The result is a `protocol::Content` enum (`Text` or `Parts`).

### 2.3 Command Routing

`process_input` (`app/events.rs`) checks:

1. **Slash commands** (`/model`, `/clear`, etc.) — dispatched via `crate::commands::run_command`.
2. **Exec bangs** (`!cmd`) — spawn a shell process, **skip the agent entirely**.
3. **Normal text** — fires the `input_submit` Lua cell, returns `InputOutcome::StartAgent`.

### 2.4 Queuing While Agent Runs

If a turn is already in flight (`self.agent.is_some()`), new submissions are **queued** in `self.queued_messages: Vec<String>` instead of starting a new turn immediately. After the current turn ends, the queue is drained one message at a time.

---

## 3. Phase 2 — TUI Assembly (Rust + Lua)

**Source**: `crates/tui/src/app/agent.rs` (Rust), `crates/tui/src/prompt_sections.rs` (Rust), `runtime/lua/smelt/plugins/plan_mode.lua` (Lua)

### 3.1 System Prompt Construction

The system prompt is **not a single string**. It is an ordered map of named sections (`PromptSections`) that Lua plugins can mutate.

**Default sections** (`prompt_sections.rs::build_defaults`):

| Section | Source | Condition |
|---------|--------|-----------|
| `base` | Hard-coded base prompt (tools, code style, approach) | Always |
| `behavior` | `interactive_behavior()` or `autonomous_behavior()` | Always |
| `write_access` | Write-access reminder | `Apply` or `Yolo` mode |
| `skills` | Plugin-provided skill text | If non-empty |
| `instructions` | Extra user instructions | If non-empty |

**Assembly**: sections are concatenated with double newlines via `PromptSections::assemble()`.

**Lua API for mutation** (`crates/tui/src/lua/api/prompt.rs`):
- `smelt.prompt.set_section(name, content)` — insert/replace a section.
- `smelt.prompt.remove_section(name)` — remove a section.

**Plan mode example** (`runtime/lua/smelt/plugins/plan_mode.lua`):
- On `agent_mode == "plan"`, injects a `plan_mode` section containing strict read-only rules.
- Registers the `exit_plan_mode` tool (mode-gated to `plan` only).
- On mode change, removes the section and unregisters the tool.

### 3.2 Built-in System Prompt Template

**Source**: `crates/engine/src/prompts/system.txt` (rendered via MiniJinja in `crates/engine/src/lib.rs`)

The template is rendered with context variables:
- `cwd` — working directory
- `write_access` — true in `Apply`/`Yolo` mode
- `skills_section` — injected skill documentation
- `extra_instructions` — user-provided extra instructions

Key behavioral rules in the template:
- "Use dedicated tools over bash"
- "Always read a file with read_file before editing it"
- "Always use edit_file for modifying existing files"
- "Call multiple tools in parallel when there are no dependencies"
- "Never create files unless absolutely necessary"

### 3.3 Tool Collection for the Turn

**Source**: `crates/tui/src/app/agent.rs::dispatch_turn` (Rust)

Tools are gathered from **two sources** and sent to the engine in `StartTurnPayload.tools: Vec<ToolDef>`:

1. **Core/MCP tools** — provided by the engine's `ToolDispatcher`.
2. **Lua plugin tools** — registered via `smelt.tools.register(...)`.

**Lua tool registration** (`runtime/lua/smelt/tools/*.lua`, `crates/core/src/lua/api/tools.rs`):
- Each tool defines: `name`, `description`, `parameters` (JSON Schema), `execute`, plus optional hooks (`confirm_text`, `approval_patterns`, `preflight`, `summary`, `render`, `decide`, `modes`, `execution_mode`, `override`).
- `_bootstrap.lua` wraps the raw Rust register function to auto-inject `default_summary`.

---

## 4. Phase 3 — Engine Turn (Rust)

**Source**: `crates/engine/src/agent.rs` (Rust)

### 4.1 Turn Initialization

`engine_task` listens on `cmd_rx: mpsc::UnboundedReceiver<UiCommand>`. On `StartTurn(payload)`:

1. **Build Provider** with per-turn API overrides (`api_base`, `api_key`, `model_config_overrides`).
2. **Resolve system prompt** in priority order:
   - TUI-provided `system_prompt`
   - Engine config override (`config.system_prompt_override`)
   - Built-in template (`build_system_prompt_full(mode, cwd, instructions, skill_section)`)
3. **Construct `Turn`** struct holding:
   - `Provider`, `ToolDispatcher`, channels, config, cancel token
   - Empty `messages: Vec<Message>`
   - `mode`, `reasoning_effort`, `turn_id`, `model`, `system_prompt`
   - Lua-registered `tools: Vec<protocol::ToolDef>`
4. Call `turn.run(input_content, history).await`.

### 4.2 The Turn Loop (`Turn::run`)

```rust
async fn run(&mut self, content: Content, history: Vec<Message>) {
    self.provider.reset_turn_state();
    self.messages = Vec::with_capacity(history.len() + 2);
    self.messages.push(Message::system(&self.system_prompt));
    self.messages.extend(history);

    if !content.is_empty() {
        self.push_message(Message::user(content));
    }
    self.emit_messages_snapshot();

    let mut first = true;
    let mut empty_retries: u8 = 0;
    const MAX_EMPTY_RETRIES: u8 = 2;

    loop {
        if !first { self.drain_commands(); }
        first = false;
        self.regenerate_system_prompt();

        // Recompute tool definitions each iteration
        let tool_defs: Vec<ToolDefinition> = if self.provider.tool_calling() { ... } else { Vec::new() };

        if self.cancel.is_cancelled() { self.emit_turn_complete(true); return; }

        let (result, partial_text, partial_reasoning) = self.call_llm(&tool_defs).await;
        // ... handle response, execute tools, loop
    }
}
```

**Loop invariant**: each iteration sends the current `self.messages` (system + history + user input + all prior assistant/tool exchanges) plus the current tool definitions to the LLM.

### 4.3 Tool Definition Assembly (Per-Iteration)

**Source**: `crates/engine/src/agent.rs` lines 738–774

Tool definitions are rebuilt **every loop iteration** because the mode may have changed mid-turn:

```rust
let tool_defs: Vec<ToolDefinition> = if self.provider.tool_calling() {
    let mut defs: Vec<ToolDefinition> = self
        .dispatcher
        .definitions()
        .into_iter()
        .filter(|d| {
            self.dispatcher
                .is_visible(d.function.name.as_str(), self.mode)
        })
        .collect();

    // Plugin tools with `override_core` shadow core tools
    let overridden: HashSet<&str> = self
        .tools
        .iter()
        .filter(|pt| pt.override_core)
        .map(|pt| pt.name.as_str())
        .collect();
    if !overridden.is_empty() {
        defs.retain(|d| !overridden.contains(d.function.name.as_str()));
    }

    // Append Lua-registered tools filtered by mode
    for pt in &self.tools {
        if let Some(ref modes) = pt.modes {
            if !modes.contains(&self.mode) { continue; }
        }
        defs.push(ToolDefinition::new(FunctionSchema {
            name: pt.name.clone(),
            description: pt.description.clone(),
            parameters: pt.parameters.clone(),
        }));
    }
    defs
} else {
    Vec::new()
};
```

**Key insight**: NOT all tools are always presented. Tools are filtered by:
- `provider.tool_calling()` — if false, no tools are sent.
- `dispatcher.is_visible(name, mode)` — core tools can be hidden per mode.
- `pt.modes` — Lua tools can restrict themselves to specific modes.
- `pt.override_core` — Lua tools can shadow core tools of the same name.

### 4.4 Calling the LLM (`call_llm`)

**Source**: `crates/engine/src/agent.rs` lines 1522–1621

`call_llm` is a `tokio::select!` loop:

1. **Spawns** `provider.chat(&self.messages, tool_defs, &self.model, self.reasoning_effort, &opts)`.
2. **Streams deltas** back to the TUI via `EngineEvent::TextDelta` / `ThinkingDelta`.
3. **Handles mid-flight UI commands**:
   - `Cancel` → sets cancel token, lets the future finish.
   - `SetAgentMode { mode, system_prompt, tools }` → updates mode/system prompt/tool list for next iteration.
   - `SetReasoningEffort` → updates effort for next iteration.
   - `SetModel` → deferred until after the request completes.
   - `Steer { text }` / `Unsteer { count }` → deferred until after the request, then injected into history.

**Steer mechanism**: `Steer` injects a new user message into `self.messages` mid-turn. If a `Steer` arrived during the LLM call, `had_injected` is set to `true`, and the loop continues without emitting the assistant's text (so the model can respond to the new user message instead).

### 4.5 Provider-Level Body Construction

**Source**: `crates/engine/src/provider/mod.rs`, `crates/engine/src/provider/chat_completions.rs`, `crates/engine/src/provider/openai.rs`, `crates/engine/src/provider/anthropic.rs` (all Rust)

The engine supports multiple provider kinds. Each serializes the same `Vec<Message>` and `Vec<ToolDefinition>` into a provider-specific JSON body.

| Provider Kind | Endpoint | Body Builder |
|---------------|----------|--------------|
| `OpenAiCompatible` | `/chat/completions` | `chat_completions::build_body` |
| `OpenAi` | `/responses` | `openai::build_body` |
| `Codex` | Codex endpoint | `openai::build_body` (with `store: false`) |
| `Anthropic` / `AnthropicCompatible` | `/messages` | `anthropic::build_body` |
| `Copilot` | proxy `/chat/completions` | `chat_completions::build_body` |

#### 4.5.1 Chat Completions Format (OpenAI-compatible)

**Source**: `crates/engine/src/provider/chat_completions.rs`

```rust
pub(super) fn build_body(
    messages: &[Message],
    tools: &[ToolDefinition],
    model: &str,
    effort: ReasoningEffort,
    config: &ModelConfig,
) -> serde_json::Value {
    let api_messages: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| {
            let mut v = serde_json::to_value(m).unwrap();
            if let Some(obj) = v.as_object_mut() {
                obj.remove("is_error");
                if m.role == Role::Tool {
                    if let Some(s) = obj.get("content").and_then(|c| c.as_str()) {
                        let trimmed = trim_tool_output(s, MAX_TOOL_OUTPUT_LINES);
                        obj.insert("content".into(), serde_json::json!(trimmed));
                    }
                }
                super::sanitize_tool_call_arguments(obj);
            }
            v
        })
        .collect();

    let mut body = serde_json::json!({ "model": model, "messages": api_messages });
    if !tools.is_empty() { body["tools"] = serde_json::to_value(tools).unwrap(); }
    // temperature, top_p, top_k, min_p, repeat_penalty ...
    // reasoning_effort + chat_template_kwargs
    body
}
```

**Message serialization** (`protocol::Message` → JSON):
- `System` → `{"role": "system", "content": "..."}`
- `User` → `{"role": "user", "content": "..."}` (or multipart array with images)
- `Assistant` → `{"role": "assistant", "content": "...", "tool_calls": [...]}`
- `Tool` → `{"role": "tool", "tool_call_id": "...", "content": "..."}`

**Tool trimming**: tool outputs are truncated to `MAX_TOOL_OUTPUT_LINES` (default 2000 lines) before serialization.

**Tool definition serialization**:
```json
{
  "type": "function",
  "function": {
    "name": "...",
    "description": "...",
    "parameters": { ...json schema... }
  }
}
```

#### 4.5.2 OpenAI Responses API Format

**Source**: `crates/engine/src/provider/openai.rs`

Uses the newer **Responses API**:
- System messages → `"instructions"` string (concatenated).
- User messages → `"input"` array with `role: "user"`.
- Assistant text → `"type": "message"` with `output_text`.
- Assistant tool calls → `"type": "function_call"` with `call_id`, `name`, `arguments`.
- Tool results → `"type": "function_call_output"` with `call_id`, `output`.
- Tools → array of `{"type": "function", "name", "description", "parameters"}`.
- Reasoning → `{"effort": "...", "summary": "auto"}`.

#### 4.5.3 Anthropic Messages API Format

**Source**: `crates/engine/src/provider/anthropic.rs`

- System messages → concatenated into `body["system"]`.
- User → `{"role": "user", "content": text}`.
- Assistant text → `{"type": "text", "text": ...}` inside `content` array.
- Assistant tool calls → `{"type": "tool_use", "id", "name", "input": parsed_json}`.
- Tool results → `{"role": "user", "content": [{"type": "tool_result", "tool_use_id", "content"}]}`.
- Tools → `{"name", "description", "input_schema": parameters}`.
- Adaptive thinking for opus-4-6/sonnet-4-6: `{"thinking": {"type": "adaptive", "display": "summarized"}}`.

### 4.6 Streaming vs Batch

**Source**: `crates/engine/src/provider/mod.rs` lines 526–534

- **Streaming** is used when `opts.on_delta.is_some()` OR the provider is Codex.
- `body["stream"] = true`.
- For OpenAI-compatible, `body["stream_options"] = {"include_usage": true}`.
- Deltas are parsed via SSE (Server-Sent Events) and emitted as `TextDelta` / `ThinkingDelta` events.

### 4.7 Response Handling

**Source**: `crates/engine/src/agent.rs` lines 781–920

After `call_llm` returns:

1. **Token usage** is emitted (`EngineEvent::TokenUsage`).
2. **Auto-compaction check**: if `auto_compact` is on and prompt tokens exceed threshold (default 80% of context window), history is compacted via `compact::run_compact` (see §5).
3. **Empty response retry**: if the LLM returns no content, no reasoning, and no tool calls, and the last message was a tool result, the engine retries up to 2 times.
4. **No tool calls**: push `Message::assistant(content, reasoning, None)` to history, emit `TurnComplete`, turn ends.
5. **Has tool calls**: push `Message::assistant(content, reasoning, Some(tool_calls))`, then execute tools (see §4.8).

---

## 5. Phase 3b — History Compaction (Rust)

**Source**: `crates/engine/src/compact.rs` (Rust)

Compaction is a background LLM call that summarizes old history to free context window.

**Trigger**: `maybe_compact` checks if `prompt_tokens > context_window * 80%`. Only fires once per turn.

**Process**:
1. Build a summarization prompt from `prompts/compact.md` + history.
2. Call the auxiliary compaction model (or primary if no auxiliary configured) with `ReasoningEffort::Off` and **no tools**.
3. Replace old history with a single `Message::user("<summary_prefix>\n\n<summary>")`.
4. Emit `CompactionComplete { messages }`.

**Retry logic**:
- Empty summaries: retry up to 2 times.
- Context-window errors during compaction: drop oldest history message and retry (up to 20 trims).

---

## 6. Phase 3c — Tool Execution (Rust + Lua)

**Source**: `crates/engine/src/agent.rs` lines 922–1518 (Rust), `runtime/lua/smelt/tools/*.lua` (Lua)

### 6.1 Classification (`classify_tools`)

For each `ToolCall` in the LLM response:

1. **Parse arguments** (`serde_json::from_str::<HashMap<String, Value>>`).
2. **Emit `ToolStarted`** event to TUI.
3. **Check Lua plugin tools first**:
   - If the tool name matches a Lua-registered tool (and either `override_core` is true or no core tool exists with that name):
     - If `hooks.any()` → emit `ToolHooksRequest` to TUI for evaluation.
     - If `execution_mode == Sequential` → add to `sequential_tools` queue.
     - Otherwise → emit `ToolDispatch` to TUI for execution.
4. **Otherwise, core/MCP tool**:
   - Evaluate hooks via `dispatcher.evaluate_hooks(name, &args, mode)`.
   - Decision: `Allow` → dispatch immediately; `Ask` → emit `RequestPermission`; `Deny` → synthetic denial; `Error` → synthetic error.

### 6.2 Concurrent Execution (`execute_concurrent`)

**Source**: `crates/engine/src/agent.rs` lines 1062–1394

A `tokio::select!` loop manages:
- **`futs`**: `FuturesUnordered` of actively running core tool dispatches.
- **`side_futs`**: Side calls from Lua (`CallCoreTool`) — don't count against concurrency limit.
- **`cmd_rx`**: UI commands arriving mid-flight:
  - `Cancel` → cancel token, stop gracefully.
  - `PermissionDecision` → dispatch approved tools or record denial.
  - `ToolHooksResponse` → transition hooks result to allow/deny/ask.
  - `ToolResult` → record result from Lua tool execution.
  - `CallCoreTool` → spawn side future.
  - `Steer/Unsteer/SetAgentMode/SetReasoningEffort/SetModel` → deferred until after execution.

On cancel: emit cancelled results for all pending tools.

### 6.3 Sequential Execution (`run_sequential`)

**Source**: `crates/engine/src/agent.rs` lines 1415–1455

After concurrent tools finish, sequential tools run one-at-a-time:
- Emit `ToolDispatch`.
- Wait for `ToolResult` via `wait_for_tool_result(request_id)`.
- If cancelled, return `("cancelled", true)`.

### 6.4 Result Collection (`collect_results`)

**Source**: `crates/engine/src/agent.rs` lines 1458–1514

For each completed tool:
1. **Dedup check**: if the output duplicates a prior tool result in history, replace with a stub (`"Result is identical to ..."`).
2. Push `Message::tool(call_id, content, is_error)` to `self.messages`.
3. Emit `ToolFinished` event.

### 6.5 Tool Execution in Lua

**Source**: `runtime/lua/smelt/tools/*.lua` (Lua), `runtime/lua/smelt/_bootstrap.lua` (Lua)

When the TUI receives `ToolDispatch`, it calls the Lua `execute` function for that tool. Lua tools run in coroutines and can:
- Yield via `smelt.task.wait(id)` for async operations.
- Call other tools via `smelt.tools.call(name, args, parent_call_id)`.
- Spawn subprocesses via `smelt.process.run_streaming`.
- Make HTTP requests via `smelt.http.get/post`.

When done, the Lua runtime calls `smelt.tools.resolve(request_id, call_id, { content, is_error })`, which sends `UiCommand::ToolResult` back to the engine.

### 6.6 Permission Flow

**Source**: `crates/tui/src/app/agent.rs` (Rust)

When the engine emits `RequestPermission`:
1. TUI checks `is_auto_approved` against runtime approvals (session/workspace scope).
2. Checks hard `decide()` policy.
3. If user is actively typing (last keypress within 1500ms) and prompt is non-empty → **defer dialog** by pushing to `pending_dialogs` queue and setting tool status to `Confirm`.
4. Otherwise → opens confirm overlay immediately.

When user resolves:
- `Yes` → sends `PermissionDecision { approved: true }`.
- `Always(Session/Workspace)` → records approval, sends `PermissionDecision { approved: true }`.
- `AlwaysPatterns` → records pattern approval.
- `AlwaysDir` → records directory approval.
- `No` without message → cancels agent.
- `No` with message → denies this tool but continues turn.

---

## 7. Phase 4 — Event Stream to TUI (Rust)

**Source**: `crates/protocol/src/event.rs` (Rust)

The engine emits a stream of `EngineEvent`s:

| Event | Direction | Meaning |
|-------|-----------|---------|
| `Ready` | Engine → TUI | Engine started |
| `Thinking / ThinkingDelta` | Engine → TUI | Reasoning content |
| `Text / TextDelta` | Engine → TUI | Assistant text |
| `Steered { text, count }` | Engine → TUI | User message injected mid-turn |
| `ToolStarted` | Engine → TUI | Tool call began |
| `ToolOutput { call_id, chunk }` | Engine → TUI | Chunk of streamed tool output |
| `ToolFinished` | Engine → TUI | Tool call completed |
| `RequestPermission` | Engine → TUI | User approval needed |
| `ToolDispatch` | Engine → TUI | Lua tool needs execution |
| `ToolHooksRequest` | Engine → TUI | Lua tool hooks need evaluation |
| `CoreToolResult` | Engine → TUI | Side core tool result |
| `TokenUsage` | Engine → TUI | Token/cost metrics |
| `Retrying` | Engine → TUI | Provider retry in progress |
| `CompactionComplete` | Engine → TUI | History was compacted |
| `TurnComplete` | Engine → TUI | Turn finished (with canonical messages) |
| `TurnError` | Engine → TUI | Turn failed |

---

## 8. Phase 5 — Transcript Rendering (Rust)

**Source**: `crates/core/src/content/stream_parser.rs` (Rust), `crates/core/src/transcript_model.rs` (Rust)

### 8.1 Stream Parser

`StreamParser` converts `EngineEvent` deltas into `BlockHistory` mutations:

- **`ThinkingDelta`** → accumulates into `Block::Thinking`, rewrites in place.
- **`TextDelta`** → detects code fences, tables, paragraph breaks; commits finished structures as `Block::Text` or `Block::CodeLine`.
- **`ToolStarted`** → pushes `Block::ToolCall { call_id, name, summary, args }` + `ToolState { status: Pending }`.
- **`ToolOutput`** → appends to `ToolState::output`.
- **`ToolFinished`** → updates `ToolState::status` to `Ok`/`Err`/`Denied`.

### 8.2 Block Types

```rust
pub enum Block {
    User { text: String, image_labels: Vec<String> },
    Thinking { content: String },
    Text { content: String },
    CodeLine { content: String, lang: String },
    ToolCall { call_id: String, name: String, summary: String, args: HashMap<String, Value> },
    Exec { command: String, output: String },
    Compacted { summary: String },
}
```

### 8.3 Layout Caching

Each block computes a stable `content_hash` (via `seahash`). Layout is cached keyed by `LayoutKey { width, show_thinking, view_state, content_hash }`. Identical blocks produce identical hashes, so layout is instant for repeated content.

---

## 9. Complete Data Flow Diagram

```
USER INPUT
  │ (keystroke)
  ▼
┌─────────────────────────────┐
│ PromptState::handle_event   │  [Rust: crates/tui/src/input/mod.rs]
│ PromptBufferParser          │  [Rust: crates/tui/src/content/prompt_parser.rs]
└─────────────────────────────┘
  │ (KeyAction::Submit)
  ▼
┌─────────────────────────────┐
│ PromptState::build_content  │  [Rust: crates/tui/src/input/mod.rs]
│  → expands attachments      │
│  → deduplicates images      │
└─────────────────────────────┘
  │ (Content::Text / Content::Parts)
  ▼
┌─────────────────────────────┐
│ process_input               │  [Rust: crates/tui/src/app/events.rs]
│  → slash command?           │
│  → exec bang?               │
│  → StartAgent               │
└─────────────────────────────┘
  │
  ▼
┌─────────────────────────────┐
│ begin_agent_turn            │  [Rust: crates/tui/src/app/agent.rs]
│  → rebuild_system_prompt    │
│     → PromptSections        │  [Rust: crates/tui/src/prompt_sections.rs]
│     → Lua: plan_mode.lua    │  [Lua:  runtime/lua/smelt/plugins/plan_mode.lua]
│  → show_user_message        │
│  → dispatch_turn            │
└─────────────────────────────┘
  │ (UiCommand::StartTurn)
  ▼
┌─────────────────────────────┐
│ engine_task                 │  [Rust: crates/engine/src/agent.rs]
│  → build Provider           │
│  → resolve system prompt    │
│     → build_system_prompt   │  [Rust: crates/engine/src/lib.rs]
│     → prompts/system.txt    │  [Rust: crates/engine/src/prompts/system.txt]
│  → Turn::run                │
└─────────────────────────────┘
  │
  ▼
┌─────────────────────────────┐
│ Turn Loop (per iteration)   │  [Rust: crates/engine/src/agent.rs]
│  1. drain_commands          │
│  2. regenerate_system_prompt│
│  3. assemble tool_defs      │
│     → dispatcher.definitions│  [Rust: crates/engine/src/tools/mod.rs]
│     → filter by mode        │
│     → filter by visibility  │
│     → Lua-registered tools  │  [Lua:  runtime/lua/smelt/tools/*.lua]
│  4. call_llm                │
│     → provider.chat         │  [Rust: crates/engine/src/provider/mod.rs]
│     → build_body (provider- │
│        specific format)     │
│        * chat_completions   │  [Rust: crates/engine/src/provider/chat_completions.rs]
│        * openai responses   │  [Rust: crates/engine/src/provider/openai.rs]
│        * anthropic messages │  [Rust: crates/engine/src/provider/anthropic.rs]
│     → HTTP POST             │
│     → SSE stream or batch   │
│     → parse_response        │
│  5. handle response         │
│     → no tool_calls? → done │
│     → has tool_calls?       │
│        → classify_tools     │
│        → execute_concurrent │
│        → run_sequential     │
│        → collect_results    │
│        → push tool messages │
│        → loop back to 1     │
└─────────────────────────────┘
  │ (EngineEvent stream)
  ▼
┌─────────────────────────────┐
│ StreamParser                │  [Rust: crates/core/src/content/stream_parser.rs]
│  → BlockHistory mutations   │
│  → transcript rendering     │
└─────────────────────────────┘
  │
  ▼
┌─────────────────────────────┐
│ TUI Render                  │  [Rust: crates/tui/src/]
│  → LayoutKey cache          │
│  → Terminal output          │
└─────────────────────────────┘
```

---

## 10. Tool Inventory & Registration

### 10.1 Built-in Lua Tools

**Source**: `runtime/lua/smelt/tools/*.lua` (Lua)

| Tool | Mode Filter | Permission Defaults | Execution | Key Features |
|------|-------------|---------------------|-----------|--------------|
| `bash` | all | `ask` (via `decide` hook) | Concurrent | Streaming, timeout up to 10min, interactive/background detection, subpattern parser `"shell"`, auto-approval for read-only commands |
| `read_file` | all | `allow` | Concurrent | Images → data URL, notebooks → rendered, text with line numbers, dedup stub for unchanged re-reads |
| `write_file` | all | `apply=allow` | Concurrent | Refuses unread overwrites, mtime staleness check, flock on existing files, mkdir -p parent |
| `edit_file` | all | `apply=allow` | Concurrent | Exact string find/replace, `replace_all`, uniqueness check, flock + staleness |
| `grep` | all | `allow` | Concurrent | ripgrep primary, grep fallback, full rg option surface |
| `glob` | all | `allow` | Concurrent | Gitignore-aware, sorted newest-first, max 200 results |
| `web_search` | all | `ask` | Concurrent | DuckDuckGo HTML, 15-min cache, rotated User-Agent |
| `web_fetch` | all | `ask` (via `decide` hook) | Concurrent | Fetch + markdown conversion, domain redirect guard, image inline, auxiliary LLM extraction via `smelt.engine.ask` |
| `ask_user_question` | all | `allow` | **Sequential** | 1-4 questions with 2-4 options each, free-text "Other", blocks agent until reply |
| `edit_notebook` | all | `ask` | Concurrent | Cell replace/insert/delete by `cell_number` or `cell_id`, flock + staleness |
| `load_skill` | all | `ask` | Concurrent | Loads skill markdown by name from `smelt.skills.content` |
| `exit_plan_mode` | **plan only** | — | Sequential | Approval dialog with "yes, and auto-apply" / "yes" / "no", saves plan to session dir |

### 10.2 Tool Registration Flow

1. Lua calls `smelt.tools.register(ToolDef)`.
2. Rust (`crates/core/src/lua/api/tools.rs`) stores the definition in `LuaShared::tools` as `ToolHandles`.
3. A `protocol::ToolDef` JSON object is built and returned to the TUI.
4. TUI sends it to the engine in `StartTurnPayload.tools`.
5. Engine rebuilds the tool schema every loop iteration.

---

## 11. Prompt Breakdown — What the LLM Actually Sees

### 11.1 First Call of a Turn (OpenAI-compatible format)

```json
{
  "model": "gpt-4o",
  "messages": [
    { "role": "system", "content": "You are an expert coding agent...\nWorking directory: /home/user/project\n# Tools..." },
    { "role": "user", "content": "previous user message" },
    { "role": "assistant", "content": "previous assistant reply" },
    { "role": "user", "content": "current user message" }
  ],
  "tools": [
    { "type": "function", "function": { "name": "bash", "description": "...", "parameters": {...} } },
    { "type": "function", "function": { "name": "read_file", "description": "...", "parameters": {...} } },
    ...
  ],
  "temperature": 0.7,
  "reasoning_effort": "medium",
  "chat_template_kwargs": { "enable_thinking": true, "reasoning_effort": "medium" }
}
```

### 11.2 Second Call (After Tool Execution)

```json
{
  "model": "gpt-4o",
  "messages": [
    { "role": "system", "content": "..." },
    { "role": "user", "content": "previous user message" },
    { "role": "assistant", "content": "...", "tool_calls": [{"id":"call_1","function":{"name":"bash","arguments":"{\"command\":\"ls -la\"}"}}] },
    { "role": "tool", "tool_call_id": "call_1", "content": "total 128\ndrwxr-xr-x ..." },
    { "role": "assistant", "content": "...", "tool_calls": [{"id":"call_2","function":{"name":"read_file","arguments":"{\"file_path\":\"/home/user/project/README.md\"}"}}] },
    { "role": "tool", "tool_call_id": "call_2", "content": "# Project\n..." }
  ],
  "tools": [ ... ],
  ...
}
```

**Note**: tool outputs are trimmed to `MAX_TOOL_OUTPUT_LINES` (default 2000 lines) before being sent.

---

## 12. Mid-Turn Mutability

The following can change **mid-turn** (i.e., between loop iterations or even during an LLM call):

| Change | Trigger | Effect |
|--------|---------|--------|
| **Mode switch** | User toggles mode (`/mode plan`) | `SetAgentMode` → updates `self.mode`, system prompt, and tool list for next iteration. |
| **Reasoning effort** | User cycles reasoning (`ctrl+r`) | `SetReasoningEffort` → updates for next iteration. |
| **Model switch** | User changes model (`/model gpt-4o-mini`) | `SetModel` → deferred until after current LLM call. |
| **Steer** | User sends message while agent runs | `Steer` → injects user message into history mid-call; loop continues without emitting assistant text. |
| **Unsteer** | User removes last steer | `Unsteer` → removes last user message from history. |
| **Cancel** | User presses `Ctrl+C` | `Cancel` → sets cancel token; graceful stop. |

---

## 13. Auxiliary LLM Calls

**Source**: `crates/engine/src/agent.rs` (Rust)

The engine spawns auxiliary LLM calls outside the main turn loop:

| Task | Trigger | Tools Sent | Purpose |
|------|---------|------------|---------|
| **Title generation** | After first user message | No tools | Generate session title from user message + assistant tail. |
| **Compaction** | Token threshold exceeded OR `/compact` | No tools | Summarize old history into a handoff message. |
| **BTW** | Background question (`/btw`) | No tools | Answer a question without interrupting the main transcript. |
| **EngineAsk** | Tool auxiliary request (e.g., `web_fetch` extraction) | No tools | Isolated LLM call for extraction/summarization. |

---

## 14. Source Language Legend

| Tag | Meaning |
|-----|---------|
| **🦀 Rust** | Source file is in the Rust codebase (`crates/`, `src/`). |
| **🌙 Lua** | Source file is in the Lua runtime (`runtime/lua/smelt/`). |

All section headers in this document indicate the primary language of the code paths described.

---

## 15. Files Referenced

### Rust
- `crates/engine/src/agent.rs` — Main engine turn loop, tool execution, LLM calling.
- `crates/engine/src/lib.rs` — System prompt builder, engine config, handle types.
- `crates/engine/src/compact.rs` — History compaction logic.
- `crates/engine/src/provider/mod.rs` — Provider dispatch, HTTP requests, retries.
- `crates/engine/src/provider/chat_completions.rs` — OpenAI-compatible body builder.
- `crates/engine/src/provider/openai.rs` — OpenAI Responses API body builder.
- `crates/engine/src/provider/anthropic.rs` — Anthropic Messages API body builder.
- `crates/engine/src/tools/mod.rs` — ToolDispatcher trait.
- `crates/engine/src/prompts/system.txt` — Built-in system prompt template.
- `crates/core/src/lua/api/tools.rs` — Lua tool registration API.
- `crates/core/src/content/stream_parser.rs` — Event-to-block parser.
- `crates/core/src/transcript_model.rs` — Block store, layout caching.
- `crates/protocol/src/event.rs` — EngineEvent, UiCommand, ToolDef, Message types.
- `crates/protocol/src/message.rs` — Message, Role, ToolCall, Content types.
- `crates/tui/src/app/agent.rs` — TUI agent lifecycle, permission handling.
- `crates/tui/src/app/events.rs` — Input event routing.
- `crates/tui/src/input/mod.rs` — Prompt state, content building.
- `crates/tui/src/prompt_sections.rs` — System prompt section assembly.
- `crates/tui/src/content/prompt_parser.rs` — Prompt buffer parsing.

### Lua
- `runtime/lua/smelt/tools/bash.lua`
- `runtime/lua/smelt/tools/read_file.lua`
- `runtime/lua/smelt/tools/write_file.lua`
- `runtime/lua/smelt/tools/edit_file.lua`
- `runtime/lua/smelt/tools/grep.lua`
- `runtime/lua/smelt/tools/glob.lua`
- `runtime/lua/smelt/tools/web_search.lua`
- `runtime/lua/smelt/tools/web_fetch.lua`
- `runtime/lua/smelt/tools/ask_user_question.lua`
- `runtime/lua/smelt/tools/notebook_edit.lua`
- `runtime/lua/smelt/tools/load_skill.lua`
- `runtime/lua/smelt/plugins/plan_mode.lua`
- `runtime/lua/smelt/_bootstrap.lua`
- `runtime/lua/smelt/_meta/tools.lua`
- `runtime/lua/smelt/_meta/_types.lua`
