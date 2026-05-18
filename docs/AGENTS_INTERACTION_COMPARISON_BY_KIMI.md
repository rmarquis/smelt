# Comparative Architectural Analysis: Smelt vs. Kimi-CLI vs. Claude Code

> **Scope:** This document compares the end-to-end architectures of three coding agent CLIs — how each transforms a user keystroke into an LLM API request and back to rendered output. Based on exhaustive code analysis of:
> - **Smelt** (`docs/SMELT_CODE_INTERACTION_BY_KIMI.md`) — Rust, open-source
> - **Kimi-CLI** (`docs/KIMICLI_CODE_INTERACTION_BY_KIMI.md`) — Python, open-source
> - **Claude Code** (`docs/CLAUDE_CODE_INTERACTION_BY_KIMI.md`) — TypeScript/React, extracted from npm
>
> **Generated:** 2026-05-18 by Kimi Code CLI

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

| Dimension | **Smelt** | **Kimi-CLI** | **Claude Code** |
|-----------|-----------|--------------|-----------------|
| **Language** | Rust | Python 3.12+ | TypeScript (Bun-bundled) |
| **UI Framework** | Custom TUI stack (`crossterm` + custom compositor) | `prompt-toolkit` + `rich` | Custom fork of **Ink** (React for terminal) |
| **Layout Engine** | Custom `smelt_term` grid + `smelt_edit` window system | Rich's `Live` + prompt_toolkit layout | **Yoga** flexbox + custom React reconciler |
| **Input Capture** | `crossterm::EventStream` (async) | `prompt-toolkit` `PromptSession` | Custom `useInput` on raw stdin via EventEmitter |
| **Rendering** | Double-buffered cell diff (`Compositor`) | Rich `Live` display + ANSI injection | React reconciler → screen buffer → ANSI diff |
| **Agent Core** | `Turn::run()` in `engine` actor | `KimiSoul._step()` via `kosong.step()` | `query()` async generator + `QueryEngine` |
| **LLM Abstraction** | Per-provider `read_stream()` in `engine` crate | **Kosong** package (`generate()` + `step()`) | Anthropic SDK first-party + Bedrock/Vertex/Foundry |
| **Tool System** | Lua-registered + core Rust tools | Python classes (`CallableTool2`) + MCP | TypeScript `Tool` interface + Zod schemas + MCP |
| **Tool Concurrency** | `FuturesUnordered` concurrent + sequential | Kosong handles via `asyncio.Task` futures | Partitioned into concurrent (safe) + serial (destructive) batches |
| **Permission System** | `ToolDispatcher.evaluate_hooks()` → `Decision` | `ApprovalRuntime` + `RootWireHub` broadcast | 7 modes + YOLO classifier + rule-based matching |
| **State Management** | Reactive cells (`smelt_core::Cells`) + `Core` bundle | `Runtime` dataclass + `Context` history | Zustand-like store (`createStore<T>()`) + React context |
| **Extensibility** | **Lua-first** (tools, commands, keymaps, themes, plugins) | Python plugins + YAML agent specs | Hooks (pre/post tool), plugins, skills, MCP |
| **Message History** | Engine owns authority; TUI mirrors via events | `Context` manager + `Session` | `QueryEngine` owns mutable history; REPL mirrors for render |
| **Headless Mode** | `HeadlessApp` with `HeadlessSink` | `Print` class (`--print`) | `--print` / `--output-format` |
| **Multi-Frontend** | TUI only (headless is stdout sink) | Shell, Print, ACP, Wire (JSON-RPC), Web, Vis | Print, MCP server, Bridge, Daemon, SDK, Stream-JSON |
| **Context Compaction** | Auto-compact in engine turn loop | Auto-compact + max_steps_per_turn | Auto-compact, micro-compact, snip replay, compact boundaries |
| **Key Unique Feature** | Parallel block layout (8 threads) + Lua runtime | Wire protocol decouples soul from UI | Custom Ink reconciler + Yoga flexbox terminal DOM |

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

**Key Difference:** Smelt leverages Rust's type system and ownership for safety and parallelism. Kimi-CLI leans on Python's ecosystem (rich, prompt-toolkit, pydantic, fastmcp). Claude Code brings web frontend patterns (React, component trees, hooks) to the terminal via a custom reconciler.

---

## 3. UI Architecture: Input Capture

| Aspect | Smelt | Kimi-CLI | Claude Code |
|--------|-------|----------|-------------|
| **Raw mode entry** | `crossterm::enable_raw_mode()` + alternate screen | `prompt-toolkit` handles it internally | `Ink` class sets raw mode on stdin |
| **Key event source** | `crossterm::event::EventStream` (async stream) | `prompt-toolkit` key processor | Custom `parse-keypress.ts` → EventEmitter |
| **Event loop** | `tokio::select!` over 8 branches | `asyncio` while-loop with queue | React component lifecycle + `useInput` hooks |
| **Input editing** | `PromptState` + `smelt_edit` vim bridge | `CustomPromptSession` + `useTextInput` | `useTextInput` hook with `Cursor` class |
| **Key bindings** | Lua-registered keymaps + static `BINDINGS` table | `prompt-toolkit` key bindings + modal delegates | Ink `useInput` + readline bindings in hook |
| **Chord support** | Multi-key chord buffering in `pending_chord` | Limited (Ctrl-X toggle, Shift-Tab) | Readline-style (Ctrl+A/E/K/U/W/Y) |
| **Vim mode** | Yes — full vim bridge via `smelt_edit` | No | Yes — vim motions in text input |
| **Paste handling** | Bracketed paste + image path detection | `prompt-toolkit` paste + image placeholder | `usePasteHandler` for bracketed paste + images |
| **Completions** | Tab completer + file mention (`@`) | Slash command + file mention (`@`) completers | Typeahead suggestions + slash commands |

**Key Difference:** Smelt's input is **event-driven async** (crossterm streams into tokio select). Kimi-CLI delegates entirely to **prompt-toolkit's blocking-async model**. Claude Code uses a **React hooks model** where `useInput` subscribes to a global EventEmitter — fundamentally a publish/subscribe pattern rather than an event loop.

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

**Key Difference:** Smelt optimizes for **performance** (parallel layout, diff rendering, minimal allocations). Kimi-CLI optimizes for **familiarity** (Rich + prompt-toolkit are standard Python TUI libraries). Claude Code optimizes for **expressiveness** (React component model enables complex, composable UIs with state management patterns from web development).

---

## 5. LLM Abstraction & Provider Layer

| Aspect | Smelt | Kimi-CLI | Claude Code |
|--------|-------|----------|-------------|
| **Primary provider** | OpenAI-compatible (default), Anthropic, Codex, Copilot | Kimi (Moonshot) default, plus OpenAI, Anthropic, Gemini, Vertex | Anthropic first-party (default), Bedrock, Vertex, Foundry |
| **Abstraction layer** | `Provider::chat()` + per-provider `read_stream()` | **Kosong** (`ChatProvider` protocol) | Anthropic SDK directly (`anthropic.beta.messages.create`) |
| **Request building** | Per-provider `build_body()` (OpenAI, Anthropic, Codex) | Per-provider SDK class (`Kimi`, `OpenAILegacy`, `Anthropic`) | `paramsFromContext()` closure with normalization |
| **Message format** | Internal `Message` → provider-specific JSON | Kosong `Message` → provider-specific | Internal `Message` → Anthropic SDK `MessageParam` |
| **Auth** | Bearer token, OAuth (Codex/Copilot), env vars | OAuth + env vars (`KIMI_API_KEY`, `OPENAI_API_KEY`) | OAuth, API key, AWS/GCP credentials, keychain |
| **Cache control** | Not documented | Kimi `prompt_cache_key` | `cache_control: { type: 'ephemeral' }` (prompt caching) |
| **Retry logic** | Basic (via reqwest) | `tenacity` library | Custom `withRetry()` — up to 10 retries, fallback model, auth refresh |
| **Streaming parser** | Custom SSE parser per provider | Kosong `StreamedMessagePart` with `merge_in_place()` | Raw Anthropic SSE stream, explicit event switch |
| **Non-streaming fallback** | Not documented | Not documented | Yes — `executeNonStreamingRequest()` on empty stream |
| **Stream watchdog** | Cancellation token per chunk | Not documented | 90s timeout (`CLAUDE_ENABLE_STREAM_WATCHDOG`) |

**Key Difference:** Smelt treats the provider layer as a **thin adapter** over HTTP. Kimi-CLI extracts LLM interaction into a **reusable package (Kosong)** that could be used independently. Claude Code is **deeply integrated** with the Anthropic SDK, leveraging beta features (tool streaming, thinking blocks, cache control, effort levels) that are not standardized across providers.

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

**Key Difference:** Smelt uses **callbacks within the engine** that emit events across a channel. Kimi-CLI uses **callbacks at the Kosong layer** that push to a Wire message bus. Claude Code uses **async generators** that yield complete messages, which is the most "native" pattern for JavaScript/TypeScript async iteration.

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
- `query()` is a pure async generator — no side effects except yielding
- REPL mirrors QueryEngine state into React state for rendering

**Key Difference:** Smelt's engine is a **long-running actor** that maintains state internally. Kimi-CLI's soul is a **stateful object** that delegates LLM interaction to Kosong. Claude Code's `query()` is a **stateless async generator** that operates on immutable parameters — state lives in `QueryEngine` which wraps it.

---

## 8. Tool System

| Aspect | Smelt | Kimi-CLI | Claude Code |
|--------|-------|----------|-------------|
| **Tool definition** | Lua `smelt.tools.register()` or Rust `ToolDispatcher` | Python class `CallableTool2[T]` with Pydantic params | TypeScript `Tool` interface with Zod `inputSchema` |
| **Schema generation** | Lua table → JSON Schema | Pydantic model → JSON Schema | Zod schema → JSON Schema (`zodToJsonSchema`) |
| **Built-in tools** | Lua: bash, read_file, edit_file, write_file, glob, grep, web_search, web_fetch | Python: Shell, ReadFile, WriteFile, StrReplaceFile, Glob, Grep, SearchWeb, FetchURL | TypeScript: BashTool, FileReadTool, FileEditTool, GlobTool, GrepTool, WebSearchTool, etc. |
| **MCP integration** | `McpDispatcher` via `McpManager` | `fastmcp.Client` per server, `MCPTool` wrapper | `fastmcp` client, tool wrapping, official registry |
| **Tool execution** | Lua coroutine (main thread) or Rust async | `asyncio` coroutine | Async function with abort signal |
| **Concurrency** | `FuturesUnordered` (core tools) + sequential (Lua) | Kosong handles via `asyncio.Task` futures | Partitioned: concurrent (safe) + serial (destructive) |
| **Deduplication** | Same-step dedup in engine; cross-step nag | Same-step + cross-step dedup in `KimiToolset` | Not documented in analysis |
| **Hooks** | PreToolUse / PostToolUse via Lua middleware | `HookEngine` with `PreToolUse` / `PostToolUse` / `Stop` | Pre-tool hooks + post-tool hooks + stop hooks |
| **Tool rendering** | Lua `render()` hook → `RenderedLayout` cached | `display` blocks (diff, shell, todo) in `ToolReturnValue` | `renderToolUseMessage()`, `renderToolResultMessage()` React components |
| **Result budget** | Not documented | `ToolResultBuilder` (50K chars default) | `applyToolResultBudget()` |
| **MCP output budget** | Not documented | 100K char budget, media dropped silently | Not documented |

**Key Difference:** All three support MCP, but their **native tool models differ significantly**. Smelt is **Lua-first** — most built-in tools are Lua scripts that can be overridden by users. Kimi-CLI uses **Python classes with dependency injection**. Claude Code uses **TypeScript interfaces with React renderers** — tools can define how they appear in the UI.

---

## 9. Permission & Approval System

| Aspect | Smelt | Kimi-CLI | Claude Code |
|--------|-------|----------|-------------|
| **Modes** | `yolo`, `afk`, normal (implicit via hooks) | `yolo`, `afk`, normal | `default`, `acceptEdits`, `bypassPermissions`, `dontAsk`, `plan`, `auto`, `bubble` |
| **Rule system** | `approval_patterns`, `decide()` hook in Lua tools | `approval_patterns`, `decide()` hook, `permission_defaults` | `alwaysAllowRules`, `alwaysDenyRules`, `alwaysAskRules`, content-aware matching |
| **Classifier** | Not documented | Not documented | **YOLO classifier** (`classifyYoloAction()`) auto-approves safe actions |
| **Multi-consumer** | Not documented | **RootWireHub** broadcasts to all UI consumers | Races between local, bridge, channel, hooks |
| **Engine blocking** | `EngineClient::recv()` returns `pending()` during confirms | `ApprovalRuntime.wait_for_response()` blocks tool | `canUseTool()` async function awaited in tool execution |
| **UI dialog** | Confirm overlay in TUI | `ApprovalRequestPanel` (Rich) + `ApprovalPromptDelegate` modal | Interactive permission queue in REPL |
| **Auto-approve** | `yolo` mode | `yolo` / `afk` mode | `auto` mode with classifier + `acceptEdits` fast-path |
| **Sandbox** | Not documented | `SandboxManager` | `SandboxManager` with unsandboxed command guards |

**Key Difference:** Claude Code has the **most sophisticated permission system** with 7 modes, a YOLO classifier, denial tracking, and multi-consumer races. Kimi-CLI's `RootWireHub` is architecturally elegant — any consumer can resolve an approval. Smelt keeps permissions simpler, delegating decisions to Lua tool hooks and the `ToolDispatcher`.

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

**Key Difference:** Smelt uses **explicit reactive cells** (pub/sub within Core). Kimi-CLI uses **direct references** (Runtime passed everywhere). Claude Code uses **React's state model** (store + selectors + re-renders) which is natural for a React application but adds overhead.

---

## 11. Extensibility & Plugin Model

| Aspect | Smelt | Kimi-CLI | Claude Code |
|--------|-------|----------|-------------|
| **Primary extension language** | **Lua** (first-class) | Python (plugins) + YAML (agents) | TypeScript (plugins) + JSON (settings) |
| **Tool registration** | `smelt.tools.register()` at runtime | `toolset.load_tools()` with dependency injection | `getTools()` + `assembleToolPool()` |
| **Command registration** | `smelt.cmd.register()` at runtime | Slash command registries | Slash commands in `commands.ts` |
| **Keymap registration** | `smelt.keymap.set()` at runtime | Not documented | Not documented |
| **Theme customization** | `smelt.theme.use()` | `smelt.theme.set()` | Built-in themes + settings |
| **Agent specs** | Not documented | **YAML with inheritance** (`agentspec.py`) | `--agents` JSON flag |
| **Skill system** | `smelt.skills` API | Skills discovered from dirs, formatted into system prompt | Skills loaded at init, bundled + custom |
| **Plugin loading** | Lua autoload (`tools/`, `commands/`, `plugins/`) | `loadAllPluginsCacheOnly()`, versioned plugins | `initBuiltinPlugins()`, `initializeVersionedPlugins()` |
| **MCP plugins** | `McpDispatcher` | `fastmcp` client + `MCPTool` | `fastmcp` client + official registry |
| **Hook system** | Lua middleware (`before`/`after` tool hooks) | `HookEngine` (`PreToolUse`, `PostToolUse`, `Stop`) | Pre-tool hooks, post-tool hooks, post-sampling hooks, stop hooks |
| **Reload** | `/reload` re-runs all Lua init | `/reload` re-runs config | Not documented |

**Key Difference:** Smelt is **designed as a Lua runtime with a Rust core** — the boundary is deliberate, with Lua providing all user-facing behavior. Kimi-CLI uses **YAML agent specs with inheritance** as a distinctive feature. Claude Code has the **most hooks** (pre-tool, post-tool, post-sampling, stop) but they are internal/TypeScript rather than user-scriptable.

---

## 12. Session & Context Management

| Aspect | Smelt | Kimi-CLI | Claude Code |
|--------|-------|----------|-------------|
| **Session storage** | `~/.local/state/smelt/sessions/<id>/` | `~/.local/share/kimi/sessions/` | `~/.claude/projects/<cwd>/` |
| **Session files** | `session.json`, `meta.json`, `content.txt`, `blobs/` | `context.json`, `session.json` | Transcript files, log files |
| **Resume** | `--resume [id]` loads session | `/resume` or `--resume` | `--continue`, `--resume`, `--from-pr` |
| **Fork** | `session.fork()` clones messages + new ID | `/fork` command | `--fork-session` |
| **Context file** | Not documented | `context.json` with system prompt + messages | Not documented |
| **System prompt** | `AGENTS.md` + skill section + `--system-prompt` | Jinja2-rendered from `system.md` + `AGENTS.md` | `fetchSystemPromptParts()` + `CLAUDE.md` |
| **Context sources** | `AGENTS.md` (merged root→leaf) | `AGENTS.md`, skills, work dir listing, OS, shell | Git status, `CLAUDE.md`, current date, system context |
| **Compaction** | Engine auto-compact + manual `/compact` | Auto-compact + `max_steps_per_turn` | Auto-compact, micro-compact, snip, compact boundaries |
| **Token tracking** | `token_snapshots`, `cost_snapshots` | `StatusUpdate` with `context_usage` | `checkTokenBudget()`, +500k auto-continue |
| **Background tasks** | Not documented | `BackgroundTaskManager` with `TaskList`/`TaskOutput`/`TaskStop` tools | Not documented in analysis |

**Key Difference:** All three read `AGENTS.md` for project-specific instructions. Claude Code has the ** richest context sources** (git status, branch info, commit history). Kimi-CLI's **YAML agent specs with inheritance** are unique. Smelt's session storage is the **most explicitly structured** (separate meta, content, and blob files).

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

**Key Difference:** Smelt is the **only one with true OS-level parallelism** (Rust threads for layout). Kimi-CLI and Claude Code are both single-threaded event loops, but Kimi-CLI uses `asyncio.Task` for concurrency while Claude Code uses async generators and Promise batches.

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

---

## 15. Summary: When Each Design Shines

| Scenario | Best Fit | Why |
|----------|----------|-----|
| **Maximum customization** | Smelt | Lua-first design lets users redefine almost every behavior |
| **Multiple frontends (IDE, web, TUI)** | Kimi-CLI | Wire protocol natively supports shell, print, ACP, web, vis, wire/stdio |
| **Rich terminal UI with complex layouts** | Claude Code | React component model + Yoga flexbox enables sophisticated UIs |
| **Performance-critical rendering** | Smelt | Parallel block layout (8 threads), double-buffered diff, Rust speed |
| **Rapid prototyping / Python ecosystem** | Kimi-CLI | Python's library ecosystem (rich, prompt-toolkit, fastmcp) |
| **Deep Anthropic feature utilization** | Claude Code | First-party SDK integration with all beta features |
| **Cross-provider portability** | Smelt / Kimi-CLI | Both abstract across OpenAI, Anthropic, Gemini, etc. |
| **Enterprise deployment** | Claude Code | Policy limits, remote managed settings, bridge mode, daemon |
| **Minimal dependencies / small binary** | Smelt | Rust binary with no runtime dependencies |
| **Team collaboration features** | Kimi-CLI | Subagent spawning, background tasks, dmail (inter-agent messaging) |

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

---

*End of comparison.*
