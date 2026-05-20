# Comparative Architectural Analysis: Smelt vs. Kimi-CLI vs. Claude Code vs. Codex CLI

> **Scope:** This document compares the end-to-end architectures of four coding agent CLIs — how each transforms a user keystroke into an LLM API request and back to rendered output. Based on exhaustive code analysis of:
> - **Smelt** (`docs/SMELT_CODE_INTERACTION_BY_KIMI.md`) — Rust, open-source
> - **Kimi-CLI** (`docs/KIMICLI_CODE_INTERACTION_BY_KIMI.md`) — Python, open-source
> - **Claude Code** (`docs/CLAUDE_CODE_INTERACTION_BY_KIMI.md`) — TypeScript/React, extracted from npm
> - **Codex CLI** (`docs/CODEX_INTERACTION_COMPARISON_BY_KIMI.md`) — Rust, open-source (OpenAI)
>
> **Generated:** 2026-05-20 by Kimi Code CLI

---

## Table of Contents

1. [At-a-Glance Comparison](#1-at-a-glance-comparison)
2. [Language & Runtime](#2-language--runtime)
3. [UI Architecture: Input Capture](#3-ui-architecture-input-capture)
4. [UI Architecture: Output Rendering](#4-ui-architecture-output-rendering)
5. [LLM Abstraction & Provider Layer](#5-llm-abstraction--provider-layer)
6. [Streaming Architecture](#6-streaming-architecture)
7. [Agent Core & Turn Lifecycle](#7-agent-core--turn-lifecycle)
8. [Tool System](#8-tool-system)
9. [Permission & Approval System](#9-permission--approval-system)
10. [State Management](#10-state-management)
11. [Extensibility & Plugin Model](#11-extensibility--plugin-model)
12. [Session & Context Management](#12-session--context-management)
13. [Process Architecture & Concurrency](#13-process-architecture--concurrency)
14. [Architectural Philosophy](#14-architectural-philosophy)
15. [Summary: When Each Design Shines](#15-summary-when-each-design-shines)

---

## 1. At-a-Glance Comparison

| Dimension | **Smelt** | **Kimi-CLI** | **Claude Code** | **Codex CLI** |
|-----------|-----------|--------------|-----------------|---------------|
| **Language** | Rust | Python 3.12+ | TypeScript (Bun-bundled) | Rust |
| **UI Framework** | Custom TUI stack (`crossterm` + custom compositor) | `prompt-toolkit` + `rich` | Custom fork of **Ink** (React for terminal) | `crossterm` + `ratatui` |
| **Layout Engine** | Custom `smelt_term` grid + `smelt_edit` window system | Rich's `Live` + prompt_toolkit layout | **Yoga** flexbox + custom React reconciler | `ratatui` widgets + custom inline viewport |
| **Input Capture** | `crossterm::EventStream` (async) | `prompt-toolkit` `PromptSession` | Custom `useInput` on raw stdin via EventEmitter | `crossterm::EventStream` (async) via `EventBroker` |
| **Rendering** | Double-buffered cell diff (`Compositor`) | Rich `Live` display + ANSI injection | React reconciler → screen buffer → ANSI diff | `ratatui` styled lines + inline viewport history insertion |
| **Agent Core** | `Turn::run()` in `engine` actor | `KimiSoul._step()` via `kosong.step()` | `query()` async generator + `QueryEngine` | `CodexThread` + `ThreadManager` in `app-server` |
| **LLM Abstraction** | Per-provider `read_stream()` in `engine` crate | **Kosong** package (`generate()` + `step()`) | Anthropic SDK first-party + Bedrock/Vertex/Foundry | OpenAI Responses API (`codex_api` crate) |
| **Tool System** | Lua-registered + core Rust tools | Python classes (`CallableTool2`) + MCP | TypeScript `Tool` interface + Zod schemas + MCP | Rust tool implementations + MCP (`codex_core::tools`) |
| **Tool Concurrency** | `FuturesUnordered` concurrent + sequential | Kosong handles via `asyncio.Task` futures | Partitioned into concurrent (safe) + serial (destructive) batches | Async execution via `exec_env` + `exec_policy` |
| **Permission System** | `ToolDispatcher.evaluate_hooks()` → `Decision` | `ApprovalRuntime` + `RootWireHub` broadcast | 7 modes + YOLO classifier + rule-based matching | ExecPolicy + sandbox + approval overlays |
| **State Management** | Reactive cells (`smelt_core::Cells`) + `Core` bundle | `Runtime` dataclass + `Context` history | Zustand-like store (`createStore<T>()`) + React context | `App` struct + `ChatWidget` state machines + transcript cells |
| **Extensibility** | **Lua-first** (tools, commands, keymaps, themes, plugins) | Python plugins + YAML agent specs | Hooks (pre/post tool), plugins, skills, MCP | MCP servers + plugins + hooks |
| **Message History** | Engine owns authority; TUI mirrors via events | `Context` manager + `Session` | `QueryEngine` owns mutable history; REPL mirrors for render | `ThreadManager` owns history; TUI receives via JSON-RPC notifications |
| **Headless Mode** | `HeadlessApp` with `HeadlessSink` | `Print` class (`--print`) | `--print` / `--output-format` | Headless batch via app-server JSON-RPC |
| **Multi-Frontend** | TUI only (headless is stdout sink) | Shell, Print, ACP, Wire (JSON-RPC), Web, Vis | Print, MCP server, Bridge, Daemon, SDK, Stream-JSON | TUI, headless, wire/IDE (JSON-RPC), realtime voice |
| **Context Compaction** | Auto-compact in engine turn loop | Auto-compact + max_steps_per_turn | Auto-compact, micro-compact, snip replay, compact boundaries | Auto-compact via `/responses/compact` endpoint |
| **Key Unique Feature** | Parallel block layout (8 threads) + Lua runtime | Wire protocol decouples soul from UI | Custom Ink reconciler + Yoga flexbox terminal DOM | Inline viewport (preserves scrollback) + WebSocket streaming |

---

## 2. Language & Runtime

### Smelt (Rust + Tokio)
- **Compiled binary** with zero-cost abstractions
- **Tokio** async runtime for the engine actor; main thread for TUI + Lua
- **Thread-scoped workers** for parallel transcript block layout (up to 8)
- Memory safety guarantees; `AGENTS.md` explicitly documents UTF-8 boundary safety primitives
- Lua runtime (`mlua`) for scripting; single-threaded coroutine model

### Kimi-CLI (Python + asyncio)
- **Interpreted** with `asyncio` event loop
- Everything runs on a single event loop; tool execution yields via coroutines
- **Kosong** is a first-class Python package (not external dependency) providing LLM abstraction
- Heavy use of **Pydantic** for validation and type safety
- `prompt-toolkit` and `rich` are mature, battle-tested libraries

### Claude Code (TypeScript + Bun/Node)
- **Bundled** as a single JS file via Bun; distributed via npm
- **Event-driven** with Node.js event loop; async/await throughout
- **Custom Ink fork** is the dominant architectural choice — a full React reconciler for terminals
- Heavy use of **Zod** for schema validation (tools, settings, MCP configs)
- Feature flags via `bun:bundle` `feature()` for build-time dead code elimination

### Codex CLI (Rust + Tokio)
- **Compiled binary** distributed via npm wrapper that spawns native `codex-tui`
- **Tokio** async runtime; single event loop with `select!` over multiple channels
- **App-server JSON-RPC boundary** runs in-process or separate process
- Heavy use of **protocol crates** (`codex_protocol`, `codex_app_server_protocol`) for type safety across RPC boundary
- `ratatui` + `crossterm` for TUI; `reqwest` + `tokio-tungstenite` for HTTP/WebSocket transport

**Key Difference:** Smelt and Codex CLI are both Rust + Tokio, but diverge significantly in architecture. Smelt uses a **custom compositor and Lua runtime**. Codex CLI uses **standard ratatui widgets and an app-server JSON-RPC boundary**. Kimi-CLI leans on Python's ecosystem (rich, prompt-toolkit, pydantic, fastmcp). Claude Code brings web frontend patterns (React, component trees, hooks) to the terminal via a custom reconciler.

---

## 3. UI Architecture: Input Capture

| Aspect | Smelt | Kimi-CLI | Claude Code | Codex CLI |
|--------|-------|----------|-------------|-----------|
| **Raw mode entry** | `crossterm::enable_raw_mode()` + alternate screen | `prompt-toolkit` handles it internally | `Ink` class sets raw mode on stdin | `crossterm::enable_raw_mode()` + inline viewport (not alt-screen by default) |
| **Key event source** | `crossterm::event::EventStream` (async stream) | `prompt-toolkit` key processor | Custom `parse-keypress.ts` → EventEmitter | `crossterm::event::EventStream` via `EventBroker` with pause/resume |
| **Event loop** | `tokio::select!` over 8 branches | `asyncio` while-loop with queue | React component lifecycle + `useInput` hooks | `tokio::select!` over app events / thread events / tui events / server events |
| **Input editing** | `PromptState` + `smelt_edit` vim bridge | `CustomPromptSession` + `useTextInput` | `useTextInput` hook with `Cursor` class | `TextArea` in `ChatComposer` with popup overlays |
| **Key bindings** | Lua-registered keymaps + static `BINDINGS` table | `prompt-toolkit` key bindings + modal delegates | Ink `useInput` + readline bindings in hook | `ChatKeymap` resolved from config + global shortcuts in `App` |
| **Chord support** | Multi-key chord buffering in `pending_chord` | Limited (Ctrl-X toggle, Shift-Tab) | Readline-style (Ctrl+A/E/K/U/W/Y) | Global shortcuts (Ctrl+L, Alt+Up/Down, Esc backtrack) |
| **Vim mode** | Yes — full vim bridge via `smelt_edit` | No | Yes — vim motions in text input | No |
| **Paste handling** | Bracketed paste + image path detection | `prompt-toolkit` paste + image placeholder | `usePasteHandler` for bracketed paste + images | Bracketed paste + paste-burst detection |
| **Completions** | Tab completer + file mention (`@`) | Slash command + file mention (`@`) completers | Typeahead suggestions + slash commands | Slash commands + `@` mentions + file search popups |

**Key Difference:** Smelt and Codex CLI both use **crossterm** but with very different architectures. Smelt's input is event-driven async into tokio select with a custom compositor. Codex CLI uses crossterm through an **`EventBroker` that can pause/resume** the stream when handing the terminal to external editors — a critical design choice for shell integration. Kimi-CLI delegates entirely to **prompt-toolkit's blocking-async model**. Claude Code uses a **React hooks model** where `useInput` subscribes to a global EventEmitter.

---

## 4. UI Architecture: Output Rendering

### Smelt: Custom Compositor with Parallel Layout

```
BlockHistory (immutable blocks + mutable ToolState sidecars)
    → TranscriptProjection::project()
    → BlockBufferCache::ensure_many()  [parallel, up to 8 threads]
    → layout_block_into() → LineBuilder → Buffer
    → stitch blocks into unified transcript Buffer
    → Window::render() → GridSlice
    → Compositor::render_with() → flush_diff() → ANSI
```

- **Parallel block layout:** Each block rendered independently on worker threads
- **Immutable blocks + mutable sidecars:** Allows permanent per-block layout caches
- **Double-buffered diff:** Only changed terminal cells emitted
- **Synchronized update markers:** Each frame wrapped in `\x1b[?2026h/l` to prevent tearing during high-frequency diff flushes
- **Plugin pre-rendering:** Lua tool render hooks run on main thread before parallel layout

### Kimi-CLI: Rich Live + Prompt-Toolkit ANSI Injection

```
WireMessage stream
    → _PromptLiveView.visualize_loop()
    → dispatch_wire_message() updates blocks
    → _flush_prompt_refresh() → prompt_session.invalidate()
    → _render_agent_prompt_message() → render_to_ansi()
    → FormattedText injected into prompt_toolkit layout
    OR
    → _LiveView.compose_agent_output()
    → rich.live.Live(refresh_per_second=10)
    → console.print() for committed blocks
```

- **Two rendering layers:** `_LiveView` (Rich) for non-interactive, `_PromptLiveView` (prompt_toolkit) for interactive
- **Incremental commitment:** Completed blocks flushed to terminal history; only tail stays in Live
- **ANSI bridge:** Rich renderables converted to ANSI for prompt_toolkit consumption

### Claude Code: React Reconciler → Yoga → Screen Buffer

```
React state update (setMessages)
    → React re-render (REPL + Messages + PromptInput)
    → Ink custom reconciler (diff virtual DOM)
    → Yoga flexbox layout (compute positions)
    → Renderer (walk tree, write to cell grid)
    → log-update (diff prev vs current buffer)
    → ANSI escape sequences
```

- **Full React component tree:** `<REPL>` → `<Messages>` → `<Message>` → `<AssistantTextMessage>` etc.
- **Yoga layout:** CSS-like flexbox for terminal positioning
- **Virtual scrolling:** Only visible messages mounted in fullscreen alt-screen mode
- **Component-based message rendering:** Each message type has its own React component

### Codex CLI: ratatui + Inline Viewport

```
ServerNotification stream
    → ChatWidget::on_agent_message_delta()
    → MarkdownStreamCollector (newline-gated buffer)
    → StreamController / PlanStreamController (commit animation)
    → markdown_render.rs (pulldown-cmark → ratatui Line/Span)
    → HistoryCell trait objects (AgentMarkdownCell, ExecCell, etc.)
    → ChatWidget::render() into transcript
    → Tui::draw() with inline viewport
    → history line insertion into terminal scrollback
    → crossterm::SynchronizedUpdate → ANSI
```

- **Inline viewport:** Codex renders above the shell prompt, not in the alternate screen. Terminal scrollback is preserved natively.
- **`pulldown-cmark` + `ratatui`:** Markdown parsed into styled terminal lines with tables, code blocks, and syntax highlighting (`syntect` + `two-face`)
- **Commit animation:** `StreamController` gradually reveals lines to simulate typing speed (`CommitTick` events)
- **Markdown newline-gating:** `MarkdownStreamCollector` only commits up to the last newline to avoid rendering broken markdown blocks

**Key Difference:** Smelt optimizes for **performance** (parallel layout, diff rendering, minimal allocations). Kimi-CLI optimizes for **familiarity** (Rich + prompt-toolkit are standard Python TUI libraries). Claude Code optimizes for **expressiveness** (React component model enables complex, composable UIs). Codex CLI optimizes for **shell integration** (inline viewport preserves scrollback, pause/resume event stream for `$EDITOR` handoff).

---

## 5. LLM Abstraction & Provider Layer

| Aspect | Smelt | Kimi-CLI | Claude Code | Codex CLI |
|--------|-------|----------|-------------|-----------|
| **Primary provider** | OpenAI-compatible (default), Anthropic, Codex, Copilot | Kimi (Moonshot) default, plus OpenAI, Anthropic, Gemini, Vertex | Anthropic first-party (default), Bedrock, Vertex, Foundry | OpenAI first-party (Responses API) |
| **Abstraction layer** | `Provider::chat()` + per-provider `read_stream()` | **Kosong** (`ChatProvider` protocol) | Anthropic SDK directly (`anthropic.beta.messages.create`) | `ModelClient` / `ModelClientSession` in `codex_core` |
| **Request building** | Per-provider `build_body()` (OpenAI, Anthropic, Codex) | Per-provider SDK class (`Kimi`, `OpenAILegacy`, `Anthropic`) | `paramsFromContext()` closure with normalization | `build_responses_request()` → `ResponsesApiRequest` |
| **Message format** | Internal `Message` → provider-specific JSON | Kosong `Message` → provider-specific | Internal `Message` → Anthropic SDK `MessageParam` | OpenAI Responses API `ResponseItem` format |
| **Auth** | Bearer token, OAuth (Codex/Copilot), env vars | OAuth + env vars (`KIMI_API_KEY`, `OPENAI_API_KEY`) | OAuth, API key, AWS/GCP credentials, keychain | API key, ChatGPT OAuth, session cookies |
| **Cache control** | Not documented | Kimi `prompt_cache_key` | `cache_control: { type: 'ephemeral' }` (prompt caching) | `prompt_cache_key` on thread ID |
| **Retry logic** | Cancellation token per chunk + provider-level retry | `tenacity` library | Custom `withRetry()` — up to 10 retries, fallback model, auth refresh | Custom retry + backoff in `codex_client` transport |
| **Streaming parser** | Custom SSE parser per provider | Kosong `StreamedMessagePart` with `merge_in_place()` | Raw Anthropic SSE stream, explicit event switch | SSE via `eventsource_stream` + WebSocket via `tokio-tungstenite` |
| **Non-streaming fallback** | Not documented | Not documented | Yes — `executeNonStreamingRequest()` on empty stream | WebSocket prewarm → HTTP SSE fallback |
| **Stream watchdog** | Cancellation token per chunk | Not documented | 90s timeout (`CLAUDE_ENABLE_STREAM_WATCHDOG`) | Cancellation token per turn |

**Key Difference:** Smelt and Kimi-CLI treat the provider layer as an **adapter** over multiple providers (OpenAI, Anthropic, Gemini). Claude Code is **deeply integrated** with the Anthropic SDK, leveraging beta features. Codex CLI is **tightly coupled to OpenAI's Responses API** — it uses WebSocket as the primary transport with HTTP SSE fallback, sticky `x-codex-turn-state` routing tokens, and the `/responses/compact` endpoint for context compaction.

---

## 6. Streaming Architecture

### Smelt: Callback-Based Deltas

```rust
provider.chat(..., |delta| {
    match delta {
        StreamDelta::Text(t) => emit(EngineEvent::TextDelta { delta: t }),
        StreamDelta::Thinking(t) => emit(EngineEvent::ThinkingDelta { delta: t }),
        StreamDelta::ToolArgs { ... } => emit(EngineEvent::ToolArgsDelta { ... }),
    }
})
```

- Engine emits `EngineEvent`s through mpsc channel
- TUI receives via `EngineClient::recv()` — **blocked by `Confirms`** when permission dialog open
- `StreamParser` incrementally builds `BlockHistory` from text deltas

### Kimi-CLI: Async Generator with Callbacks

```python
async def generate(..., on_message_part=None, on_tool_call=None):
    async for part in stream:
        await callback(on_message_part, part)
        if not pending_part.merge_in_place(part):
            # part complete
            if isinstance(pending_part, ToolCall):
                await callback(on_tool_call, pending_part)
```

- `kosong.generate()` is an async function (not generator) with callbacks
- `kosong.step()` adds tool dispatch on top
- Wire protocol sends every part immediately to UI

### Claude Code: Async Generator Yielding Messages

```typescript
async function* queryModelWithStreaming(...) {
    for await (const part of stream) {
        switch (part.type) {
            case 'content_block_stop':
                yield buildAssistantMessage(...)
                break
            case 'message_delta':
                updateUsage(lastMessage, part.usage)
                break
        }
    }
}
```

- `query()` is an **async generator** yielding `AssistantMessage | StreamEvent | SystemAPIErrorMessage`
- `QueryEngine.submitMessage()` consumes in `for await...of` loop
- REPL receives events and calls `setMessages()` for React re-render

### Codex CLI: Server Notifications over JSON-RPC

```rust
// In CodexThread
model_client_session.stream(prompt, model_info, ...)
    → ResponseEvent stream
    → emit ServerNotification::AgentMessageDelta { delta }
    → app-server broadcasts to subscribers
    → TUI receives via app_server.next_event()
    → ChatWidget::on_agent_message_delta(delta)
```

- **WebSocket preferred:** `ModelClientSession` lazily opens a WebSocket and reuses it across a turn
- **HTTP SSE fallback:** If WebSocket fails or is disabled, falls back to `ResponsesClient::stream_request()`
- **Sticky routing:** `x-codex-turn-state` token ensures all requests in a turn route to the same backend
- **Newline-gated commit:** `MarkdownStreamCollector` buffers deltas and only commits up to the last newline
- **Commit animation:** `StreamController` drives `CommitTick` events to simulate typing speed

**Key Difference:** Smelt uses **callbacks within the engine** that emit events across a channel. Kimi-CLI uses **callbacks at the Kosong layer** that push to a Wire message bus. Claude Code uses **async generators** that yield complete messages. Codex CLI uses a **notification stream over JSON-RPC** — the core streams to the app server, which broadcasts to TUI subscribers. This decouples rendering from LLM I/O more explicitly than the others.

---

## 7. Agent Core & Turn Lifecycle

### Smelt: Engine Actor Pattern

```
TuiApp
  → UiCommand::StartTurn → engine cmd_tx
  → engine_task() tokio::spawn
    → Turn::run()
      → loop {
          call_llm() → Provider::chat()
          if no tool_calls → TurnComplete
          classify_tools() → ToolExecutionPlan
          execute_concurrent() → FuturesUnordered
          run_sequential()
          collect_results() → Message::tool()
          loop back
        }
  → EngineEvent::TurnComplete → TUI updates session.messages
```

- **Engine runs in separate tokio task** — UI never blocks on I/O
- All history authority lives in the engine; TUI gets snapshots via events
- Turn loop is explicit: LLM call → tool classification → concurrent execution → sequential execution → results → repeat

### Kimi-CLI: Soul + Kosong Step Pattern

```
Shell
  → run_soul_command()
  → run_soul()
    → KimiSoul.run()
      → _turn(user_message)
        → _agent_loop()
          → _step()
            → kosong.step(
                chat_provider, system_prompt, toolset, history,
                on_message_part=wire_send,
                on_tool_result=wire_send
              )
            → await tool_results
            → _grow_context()
            → check max_steps, auto-compact
            → loop back
```

- **Soul is the agent abstraction**; `run_soul()` is the UI/soul orchestrator
- Kosong handles the LLM call + tool dispatch; KimiSoul manages context growth
- Wire protocol decouples soul from UI — soul sends messages, UI renders them

### Claude Code: QueryEngine + Async Generator Loop

```
REPL.onSubmit()
  → QueryEngine.submitMessage()
    → for await event of query() {
        handle stream events
        persist messages
        track usage/cost
      }
  → query() / queryLoop()
    → pre-process (compact, snip, budget)
    → deps.callModel() → queryModelWithStreaming()
    → accumulate assistantMessages + toolUseBlocks
    → if toolUseBlocks → runTools()
    → yield tool results as user messages
    → handleStopHooks()
    → checkTokenBudget()
    → loop back
```

- **QueryEngine owns mutable message history** across turns
- `query()` is **externally stateless** (pure async generator yielding events) but **internally stateful** — it carries a mutable `State` struct across loop iterations
- REPL mirrors QueryEngine state into React state for rendering

### Codex CLI: Thread Manager + Codex Thread

```
TUI ChatComposer submit
  → AppServerSession::turn_start()
    → JSON-RPC → app-server MessageProcessor
      → TurnRequestProcessor
        → ThreadManager.start_turn(thread_id, turn_params)
          → CodexThread
            → ModelClientSession::stream(prompt, model_info, ...)
            → process ResponseEvent stream
              → text deltas → AgentMessageDelta notifications
              → tool calls → ItemStarted / ItemCompleted
              → tool execution → exec_env + exec_policy
            → emit TurnCompleted notification
```

- **ThreadManager** manages thread lifecycle (start, resume, fork, shutdown)
- **CodexThread** runs the turn loop: LLM streaming → tool execution → result collection
- **ModelClientSession** is turn-scoped; creates fresh WebSocket per turn
- **App-server boundary** decouples core from TUI — notifications flow back via JSON-RPC

**Key Difference:** Smelt's engine is a **long-running actor** that maintains state internally. Kimi-CLI's soul is a **stateful object** that delegates LLM interaction to Kosong. Claude Code's `query()` is **externally a stateless async generator**. Codex CLI uses a **manager-thread pattern** where `ThreadManager` owns multiple `CodexThread` instances, each with its own `ModelClientSession` — the app-server boundary adds an extra layer of decoupling not present in the others.

---

## 8. Tool System

| Aspect | Smelt | Kimi-CLI | Claude Code | Codex CLI |
|--------|-------|----------|-------------|-----------|
| **Tool definition** | Lua `smelt.tools.register()` or Rust `ToolDispatcher` | Python class `CallableTool2[T]` with Pydantic params | TypeScript `Tool` interface with Zod `inputSchema` | Rust tool implementations in `codex_core::tools` |
| **Schema generation** | Lua table → JSON Schema | Pydantic model → JSON Schema | Zod schema → JSON Schema (`zodToJsonSchema`) | Rust structs → JSON Schema |
| **Built-in tools** | Lua: bash, read_file, edit_file, write_file, glob, grep, web_search, web_fetch | Python: Shell, ReadFile, WriteFile, StrReplaceFile, Glob, Grep, SearchWeb, FetchURL | TypeScript: BashTool, FileReadTool, FileEditTool, GlobTool, GrepTool, WebSearchTool, etc. | Rust: Shell, ReadFile, WriteFile, Glob, Grep, WebSearch, FetchURL |
| **MCP integration** | `McpDispatcher` via `McpManager` | `fastmcp.Client` per server, `MCPTool` wrapper | `fastmcp` client, tool wrapping, official registry | `rmcp-client` + `McpTool` wrapper |
| **Tool execution** | Lua coroutine (main thread) or Rust async | `asyncio` coroutine | Async function with abort signal | Async via `exec_env` + sandbox |
| **Concurrency** | `FuturesUnordered` (core tools) + sequential (Lua) | Kosong handles via `asyncio.Task` futures | Partitioned: concurrent (safe) + serial (destructive) | `exec_env` handles concurrency |
| **Deduplication** | Result dedup (`result_dedup.rs`) — avoids redundant tokens in history | Same-step + cross-step dedup in `KimiToolset` | Not documented in analysis | Not documented in analysis |
| **Hooks** | PreToolUse / PostToolUse via Lua middleware | `HookEngine` with `PreToolUse` / `PostToolUse` / `Stop` | Pre-tool hooks + post-tool hooks + stop hooks | Hook runtime in core |
| **Tool rendering** | Lua `render()` hook → `RenderedLayout` cached | `display` blocks (diff, shell, todo) in `ToolReturnValue` | `renderToolUseMessage()`, `renderToolResultMessage()` React components | HistoryCell trait objects (`ExecCell`, `McpToolCallCell`) |
| **Result budget** | Not documented | `ToolResultBuilder` (50K chars default) | `applyToolResultBudget()` | Output truncation in `exec_env` |
| **MCP output budget** | Not documented | 100K char budget, media dropped silently | Not documented | Not documented |

**Key Difference:** All four support MCP. Smelt is **Lua-first** — most built-in tools are Lua scripts. Kimi-CLI uses **Python classes with dependency injection**. Claude Code uses **TypeScript interfaces with React renderers**. Codex CLI uses **Rust trait-based tools** with a clear separation between tool execution (`exec_env`) and policy (`exec_policy`).

---

## 9. Permission & Approval System

| Aspect | Smelt | Kimi-CLI | Claude Code | Codex CLI |
|--------|-------|----------|-------------|-----------|
| **Modes** | `yolo`, `afk`, normal (implicit via hooks) | `yolo`, `afk`, normal | `default`, `acceptEdits`, `bypassPermissions`, `dontAsk`, `plan`, `auto`, `bubble` | Approval policy per turn (`AskForApproval::OnRequest`, `Always`, etc.) |
| **Rule system** | `approval_patterns`, `decide()` hook in Lua tools | `approval_patterns`, `decide()` hook, `permission_defaults` | `alwaysAllowRules`, `alwaysDenyRules`, `alwaysAskRules`, content-aware matching | ExecPolicy + sandbox policy + network policy |
| **Classifier** | Not documented | Not documented | **YOLO classifier** (`classifyYoloAction()`) auto-approves safe actions | Guardian assessment for auto-review |
| **Multi-consumer** | Not documented | **RootWireHub** broadcasts to all UI consumers | Races between local, bridge, channel, hooks | App-server broadcasts to all subscribers |
| **Engine blocking** | `EngineClient::recv()` returns `pending()` during confirms | `ApprovalRuntime.wait_for_response()` blocks tool | `canUseTool()` async function awaited in tool execution | ExecPolicy blocks until approval resolved |
| **UI dialog** | Confirm overlay in TUI | `ApprovalRequestPanel` (Rich) + `ApprovalPromptDelegate` modal | Interactive permission queue in REPL | `ApprovalOverlay` in bottom pane |
| **Auto-approve** | `yolo` mode | `yolo` / `afk` mode | `auto` mode with classifier + `acceptEdits` fast-path | `AskForApproval::Always` |
| **Sandbox** | Not documented | Not documented | `SandboxManager` with unsandboxed command guards | `ExecEnv` + `ExecPolicy` with sandbox levels |

**Key Difference:** Claude Code has the **most sophisticated permission system** with 7 modes and a YOLO classifier. Kimi-CLI's `RootWireHub` is architecturally elegant — any consumer can resolve an approval. Codex CLI integrates approvals into the **JSON-RPC flow** — `CommandExecutionRequestApproval` notifications are sent to the TUI, which shows an `ApprovalOverlay`. Smelt keeps permissions simpler, delegating decisions to Lua tool hooks.

---

## 10. State Management

### Smelt: Reactive Cells + Core Bundle

```rust
pub struct Core {
    pub config: AppConfig,
    pub session: Session,
    pub confirms: Confirms,        // permission dialog queue
    pub cells: Cells,              // reactive name→value registry
    pub engine: EngineClient,
    pub files: FileStateCache,
    pub permissions: Arc<Permissions>,
    // ...
}
```

- **Reactive cells:** Typed `HashMap<String, Rc<dyn Any>>` with deferred subscriber notification
- **Confirms gates engine:** `EngineClient::recv()` returns `pending()` while confirms open
- **Thread-local bridge:** `host.rs` installs raw pointer to `Core` for Lua bindings

### Kimi-CLI: Runtime Dataclass + Context

```python
@dataclass(slots=True, kw_only=True)
class Runtime:
    config: Config
    oauth: OAuthManager
    llm: LLM | None
    session: Session
    approval: Approval
    notifications: NotificationManager
    background_tasks: BackgroundTaskManager
    skills: dict[str, Skill]
    approval_runtime: ApprovalRuntime
    root_wire_hub: RootWireHub
    # ...
```

- **Runtime is a pure data bag** — no reactive system
- **State changes** flow through the Wire protocol or direct method calls
- **Context** (`Context` class) manages conversation history file

### Claude Code: Zustand-like Store + React Context

```typescript
// store.ts
createStore<T>(initialState, onChange?) → { getState, setState, subscribe }

// AppState.tsx
<AppStateProvider initialState={...}>
  {children}
</AppStateProvider>

// Hooks
useAppState(selector)      // useSyncExternalStore
useSetAppState()           // store.setState
```

- **Zustand pattern:** Simple store with selectors; React integration via `useSyncExternalStore`
- **AppState** is a large immutable object (~450 lines of shape)
- **On-change handler** persists state to disk

### Codex CLI: App Struct + ChatWidget State Machines

```rust
pub(crate) struct App {
    chat_widget: ChatWidget,
    transcript_cells: Vec<Arc<dyn HistoryCell>>,
    thread_event_channels: HashMap<ThreadId, ThreadEventChannel>,
    config: Config,
    // ...
}

pub(crate) struct ChatWidget {
    transcript: TranscriptState,
    stream_controller: Option<StreamController>,
    bottom_pane: BottomPane,
    turn_lifecycle: TurnLifecycleState,
    // ... 50+ fields
}
```

- **App is a monolithic state container** owning the ChatWidget, transcript cells, thread channels
- **ChatWidget is a state machine** with 50+ fields tracking streaming, approvals, popups, status
- **HistoryCell trait** for polymorphic transcript entries (markdown, exec, tool calls, notices)
- **No reactive system** — state mutations happen in event handlers, rendering triggered by `request_redraw()`

**Key Difference:** Smelt uses **explicit reactive cells** (pub/sub within Core). Kimi-CLI uses **direct references** (Runtime passed everywhere). Claude Code uses **React's state model** (store + selectors + re-renders). Codex CLI uses **imperative state mutation** in event handlers — closer to traditional game/UI loop patterns than reactive frameworks.

---

## 11. Extensibility & Plugin Model

| Aspect | Smelt | Kimi-CLI | Claude Code | Codex CLI |
|--------|-------|----------|-------------|-----------|
| **Primary extension language** | **Lua** (first-class) | Python (plugins) + YAML (agents) | TypeScript (plugins) + JSON (settings) | Rust (compile-time) + MCP (runtime) |
| **Tool registration** | `smelt.tools.register()` at runtime | `toolset.load_tools()` with dependency injection | `getTools()` + `assembleToolPool()` | Tool definitions in `codex_core::tools` |
| **Command registration** | `smelt.cmd.register()` at runtime | Slash command registries | Slash commands in `commands.ts` | Slash commands in `ChatComposer` |
| **Keymap registration** | `smelt.keymap.set()` at runtime | Not documented | Not documented | `tui.keymap` config → `RuntimeKeymap` |
| **Theme customization** | `smelt.theme.use()` | `/theme` slash command (Reload exception) | Built-in themes + settings | `tui.theme` config |
| **Agent specs** | Not documented | **YAML with inheritance** (`agentspec.py`) | `--agents` JSON flag | Not documented |
| **Skill system** | `smelt.skills` API | Skills discovered from dirs, formatted into system prompt | Skills loaded at init, bundled + custom | `codex_core_skills` + skills watcher |
| **Plugin loading** | Lua autoload (`tools/`, `commands/`, `plugins/`) | `loadAllPluginsCacheOnly()`, versioned plugins | `initBuiltinPlugins()`, `initializeVersionedPlugins()` | Plugin marketplace + install/uninstall |
| **MCP plugins** | `McpDispatcher` | `fastmcp` client + `MCPTool` | `fastmcp` client + official registry | `rmcp-client` + MCP tool exposure |
| **Hook system** | Lua middleware (`before`/`after` tool hooks) | `HookEngine` (`PreToolUse`, `PostToolUse`, `Stop`) | Pre-tool hooks, post-tool hooks, post-sampling hooks, stop hooks | Hook runtime in core |
| **Reload** | `/reload` re-runs all Lua init | `/reload` re-runs config | Not documented | Config reload via `config/batchWrite` |

**Key Difference:** Smelt is **designed as a Lua runtime with a Rust core**. Kimi-CLI uses **YAML agent specs with inheritance** as a distinctive feature. Claude Code has the **most hooks** but they are internal/TypeScript. Codex CLI is **the least user-extensible at the code level** — it relies on MCP servers and plugins for runtime extension, with the core being a fixed Rust binary.

---

## 12. Session & Context Management

| Aspect | Smelt | Kimi-CLI | Claude Code | Codex CLI |
|--------|-------|----------|-------------|-----------|
| **Session storage** | `~/.local/state/smelt/sessions/<id>/` | `~/.local/share/kimi/sessions/` | `~/.claude/projects/<cwd>/` | `~/.codex/` + thread store |
| **Session files** | `session.json`, `meta.json`, `content.txt`, `blobs/` | `context.json`, `session.json` | Transcript files, log files | Thread metadata, rollout traces |
| **Resume** | `--resume [id]` loads session | `/resume` or `--resume` | `--continue`, `--resume`, `--from-pr` | `--resume` with thread ID |
| **Fork** | `session.fork()` clones messages + new ID | `/fork` command | `--fork-session` | `thread/fork` JSON-RPC |
| **Context file** | Not documented | `context.json` with system prompt + messages | Not documented | Thread state managed by core |
| **System prompt** | `AGENTS.md` + skill section + `--system-prompt` | Jinja2-rendered from `system.md` + `AGENTS.md` | `fetchSystemPromptParts()` + `CLAUDE.md` | Base instructions + `AGENTS.md` + personality |
| **Context sources** | `AGENTS.md` (merged root→leaf) | `AGENTS.md`, skills, work dir listing, OS, shell | Git status, `CLAUDE.md`, current date, system context | `AGENTS.md`, work dir, git info, skills |
| **Compaction** | Auto-compact on context threshold (`maybe_compact()`) + manual `/compact` | Auto-compact + `max_steps_per_turn` | Auto-compact, micro-compact, snip, compact boundaries | `/responses/compact` endpoint + manual `/compact` |
| **Token tracking** | `token_snapshots`, `cost_snapshots` | `StatusUpdate` with `context_usage` | `calculateTokenWarningState()` + `checkTokenBudget()` | `ThreadTokenUsage` + rate limit snapshots |
| **Background tasks** | Not documented | `BackgroundTaskManager` with `TaskList`/`TaskOutput`/`TaskStop` tools | Not documented in analysis | `thread/backgroundTerminals/clean` |

**Key Difference:** All four read `AGENTS.md` for project-specific instructions. Claude Code has the **richest context sources** (git status, branch info, commit history). Kimi-CLI's **YAML agent specs with inheritance** are unique. Codex CLI integrates deeply with **OpenAI's thread model** — threads are first-class server-side objects with metadata, git info, and memory modes. Smelt's session storage is the **most explicitly structured** (separate meta, content, and blob files).

---

## 13. Process Architecture & Concurrency

### Smelt
- **Multi-threaded:** Tokio runtime + thread-scoped workers for layout
- **Engine isolation:** LLM I/O isolated in dedicated tokio task
- **Communication:** Unbounded mpsc channels (cmd, event, host)
- **Lua single-threaded:** Coroutines yield to main thread; never touch workers
- **Cancellation:** `CancellationToken` per turn, checked every SSE chunk

### Kimi-CLI
- **Single event loop:** Everything on one `asyncio` loop
- **Soul/UI concurrency:** `run_soul()` runs soul task + UI task + notification pump concurrently
- **Wire decoupling:** Async message bus between soul and UI
- **Tool execution:** `asyncio.Task` futures for async tools; `ToolResultFuture` for results
- **Background tasks:** Separate `BackgroundTaskManager` for long-running processes

### Claude Code
- **Single event loop:** Node.js event loop
- **React concurrent features:** Not used (terminal rendering is synchronous)
- **Query isolation:** Each query gets its own abort controller
- **Tool concurrency:** Partitioned batches run via `Promise.all` (concurrent) and `for...of` (serial)
- **Streaming:** Async generator consumption interleaved with React renders

### Codex CLI
- **Single event loop:** Tokio runtime with `select!` over multiple channels
- **App-server boundary:** Core logic runs either in-process or as separate process; TUI communicates via JSON-RPC
- **Thread isolation:** Each conversation thread is a separate `CodexThread` managed by `ThreadManager`
- **Event broker:** crossterm EventStream can be paused/resumed for external editor handoff
- **WebSocket reuse:** Per-turn `ModelClientSession` caches WebSocket connection across a single turn

**Key Difference:** Smelt is the **only one with true OS-level parallelism** (Rust threads for layout). Kimi-CLI and Claude Code are both single-threaded event loops. Codex CLI uses Tokio's **single-threaded async model** with explicit decoupling via the app-server JSON-RPC boundary — this enables IDE integration and headless modes without code changes.

---

## 14. Architectural Philosophy

### Smelt: "Lua-First, Rust-For-Performance"
- **Design goal:** Maximize user customization while maintaining performance
- **Lua is not an afterthought** — it's the primary interface for tools, commands, keymaps, themes, and plugins
- **Rust provides primitives:** buffer editing, HTTP, rendering, UTF-8 safety
- **Engine/UI separation:** Clean actor model with explicit protocol
- **Performance focus:** Parallel rendering, double-buffered diff, minimal allocations

### Kimi-CLI: "Wire-Decoupled, Kosong-Centered"
- **Design goal:** Multiple frontends with a shared agent core
- **Wire protocol is the architectural center** — everything flows through it
- **Kosong as reusable package:** LLM abstraction could be used outside kimi-cli
- **Pythonic pragmatism:** Leverages mature libraries (rich, prompt-toolkit, pydantic, fastmcp)
- **YAML agent specs:** Declarative agent behavior with inheritance

### Claude Code: "React-For-Terminals, SDK-Integrated"
- **Design goal:** Rich, web-like terminal UI with deep Anthropic integration
- **Custom Ink fork is the big bet** — brings React's component model to the terminal
- **Deep SDK integration:** Uses every beta feature (thinking, cache control, tool streaming, effort levels)
- **Enterprise features:** Remote control, bridge mode, daemon, policy limits, managed settings
- **Hooks over plugins:** Extensibility via hooks rather than user-loaded plugins

### Codex CLI: "Shell-Native, Server-Bound"
- **Design goal:** Seamless shell integration with OpenAI's hosted infrastructure
- **Inline viewport preserves scrollback** — Codex feels like a native shell command, not a fullscreen app
- **App-server JSON-RPC boundary** enables TUI, IDE, and headless modes with the same core
- **WebSocket-first streaming** with sticky routing for reliability
- **OpenAI-centric:** Tightly integrated with Responses API, not designed for multi-provider portability

---

## 15. Summary: When Each Design Shines

| Scenario | Best Fit | Why |
|----------|----------|-----|
| **Maximum customization** | Smelt | Lua-first design lets users redefine almost every behavior |
| **Multiple frontends (IDE, web, TUI)** | Kimi-CLI | Wire protocol natively supports shell, print, ACP, web, vis, wire/stdio |
| **Rich terminal UI with complex layouts** | Claude Code | React component model + Yoga flexbox enables sophisticated UIs |
| **Shell-native feel (preserves scrollback)** | Codex CLI | Inline viewport + pause/resume event stream for seamless `$EDITOR` handoff |
| **Performance-critical rendering** | Smelt | Parallel block layout (8 threads), double-buffered diff, Rust speed |
| **Rapid prototyping / Python ecosystem** | Kimi-CLI | Python's library ecosystem (rich, prompt-toolkit, fastmcp) |
| **Deep Anthropic feature utilization** | Claude Code | First-party SDK integration with all beta features |
| **Deep OpenAI feature utilization** | Codex CLI | Native Responses API with WebSocket, compact endpoint, reasoning summaries |
| **Cross-provider portability** | Smelt / Kimi-CLI | Both abstract across OpenAI, Anthropic, Gemini, etc. |
| **Enterprise deployment** | Claude Code | Policy limits, remote managed settings, bridge mode, daemon |
| **Team collaboration features** | Kimi-CLI | Subagent spawning, background tasks, D-Mail (inter-agent messaging) |
| **IDE integration** | Codex CLI | JSON-RPC app-server boundary designed for IDE consumption |
| **Minimal dependencies / small binary** | Smelt | Rust binary with no runtime dependencies |
| **Voice/realtime conversation** | Codex CLI | Built-in realtime audio via WebRTC + WebSocket sideband |

---

## Appendix: Structural Comparison of the Data Flow

### Smelt
```
Keystroke → crossterm → tokio select → PromptState → process_input()
  → UiCommand::StartTurn → mpsc channel → engine actor (tokio task)
    → Turn::run() → Provider::chat() → SSE → StreamDelta callback
      → EngineEvent → mpsc channel → TUI dispatch
        → StreamParser → BlockHistory → TranscriptProjection (parallel)
          → Window::render → Compositor diff → ANSI
```

### Kimi-CLI
```
Keystroke → prompt-toolkit → Shell.run() → run_soul_command()
  → run_soul() → KimiSoul._step() → kosong.step()
    → ChatProvider.generate() → HTTP stream → StreamedMessagePart
      → on_message_part callback → Wire.soul_side.send()
        → Wire.ui_side.receive() → _PromptLiveView
          → Rich Live / prompt_toolkit FormattedText → ANSI
```

### Claude Code
```
Keystroke → Ink raw stdin → useInput → useTextInput → PromptInput.onSubmit()
  → REPL.onQuery() → QueryEngine.submitMessage()
    → query() async generator → queryModelWithStreaming()
      → anthropic.beta.messages.create() → SSE
        → for await part → yield AssistantMessage
          → REPL.setMessages() → React re-render
            → Ink reconciler → Yoga layout → screen buffer
              → log-update diff → ANSI
```

### Codex CLI
```
Keystroke → crossterm → EventBroker → tokio select → App::run()
  → ChatWidget::handle_key_event() → ChatComposer → submit
    → AppServerSession::turn_start() → JSON-RPC
      → app-server MessageProcessor → TurnRequestProcessor
        → ThreadManager → CodexThread → ModelClientSession::stream()
          → WebSocket / HTTP SSE → ResponseEvent stream
            → ServerNotification → app_server.next_event()
              → ChatWidget::on_agent_message_delta()
                → MarkdownStreamCollector → StreamController
                  → markdown_render.rs → HistoryCell → transcript
                    → Tui::draw() → inline viewport → ANSI
```

---

*End of comparison.*
