# Agent Interaction Architecture: Cross-System Comparison

> **Sources:** Kimi Code CLI architectural analyses of smelt, kimi-cli, and claude-code,
> generated 2026-05-18 (`docs/SMELT_CODE_INTERACTION_BY_KIMI.md`,
> `docs/KIMICLI_CODE_INTERACTION_BY_KIMI.md`, `docs/CLAUDE_CODE_INTERACTION_BY_KIMI.md`).
>
> **Author of comparison:** Claude Sonnet 4.6, 2026-05-18.

---

## 1. High-Level Identity

| Dimension | smelt | kimi-cli | claude-code |
|-----------|-------|----------|-------------|
| **Language** | Rust | Python 3.12+ | TypeScript (Bun runtime) |
| **Binary model** | Native binary, `cargo build` | Python wheel + `uv`/`pip` | npm package (`@anthropic-ai/claude-code`) |
| **Runtime concurrency** | Tokio multi-threaded async | asyncio single event loop | JS event loop (async generators) |
| **Primary use** | Editor + agent TUI (smelt-the-editor owns the buffer) | Coding agent CLI | Coding agent CLI |
| **UI philosophy** | Custom compositor + double-buffered terminal grid | Rich `Live` + prompt-toolkit | Custom Ink fork (React for terminal) |

The three systems occupy distinct points on the spectrum from *"editor that talks to an LLM"*
(smelt) to *"LLM client with an editor inside"* (kimi-cli, claude-code). This shapes nearly
every architectural decision downstream.

---

## 2. Entry Point and Bootstrap

### Pattern

All three use a two-phase init: a fast, minimal "early" phase for flag registration, then a
heavier "full" phase for loading the actual session.

| Phase | smelt | kimi-cli | claude-code |
|-------|-------|----------|-------------|
| **Early** | `early.lua` — registers extra CLI flags before `argv` parsing | Typer root `kimi()` callback (~30 flags, resolves conflicts) | `cli.tsx` fast-path dispatch (version, dump-prompt, MCP modes) |
| **Full** | Lua autoload → user config → project config → engine actor | `KimiCLI.create()` factory: config, OAuth, LLM, Runtime, agent spec | `init()`: auth, feature flags, plugins, MCP clients, skills |
| **Dependency injection** | `ResolvedStartup` struct passed into `TuiApp` | `Runtime.create()` + YAML agent spec → DI by type annotation | `preAction` Commander hook → globals populated before command runs |

### Key difference

smelt is the only one that does a **two-pass Lua init** — `early.lua` executes before `argv`
is parsed, which lets plugins declare their own CLI flags. kimi-cli and claude-code parse a
fixed (if large) flag set up front.

---

## 3. UI Framework and Rendering Architecture

This is the deepest architectural divide among the three.

### smelt — Custom Rust compositor

```
crossterm EventStream
  → TuiApp event dispatch
  → smelt_edit Buffer (Yoga-like layout)
  → TranscriptProjection (parallel block layout, up to 8 threads)
  → Compositor double-buffer diff
  → crossterm MoveTo + Print escape sequences
```

**Key design:** The transcript is a **read-only projection** of an append-only `BlockHistory`
store. Blocks are content-addressed and immutable; only `ToolState` sidecars are mutable.
This enables permanent per-block layout caches and parallel rendering without locks.

The compositor maintains two `Grid`s (current/previous) and only emits diffs. It wraps
output in **synchronized update markers** (`\x1b[?2026h/l`) to prevent tearing.

### kimi-cli — Rich Live + prompt-toolkit

```
prompt-toolkit CustomPromptSession (raw-mode, key bindings, completers)
  → Shell input loop (idle_events queue)
  → Wire message bus (BroadcastQueue)
  → _PromptLiveView.visualize_loop()
  → Rich Live(refresh_per_second=10, transient=True)
  → console.print() for committed output
```

**Key design:** The rendering is split between two layers. The **Live** area is transient
(re-rendered from scratch each tick). Completed content blocks are **committed** to terminal
history via `console.print()` — they leave the Live area permanently, preventing unbounded
growth. Agent status renders through prompt-toolkit's `FormattedText` layout.

### claude-code — Custom Ink fork (React)

```
Ink raw-mode stdin → EventEmitter
  → useInput hook
  → useTextInput (Cursor, readline bindings)
  → PromptInput component
  → REPL component (5006 lines, entire session)
  → React re-render
  → Ink reconciler → Yoga flexbox → screen buffer
  → log-update diff → ANSI escapes
```

**Key design:** The React component tree *is* the UI state machine. `AppState` is a
Zustand-like store with `useSyncExternalStore`. The REPL component owns the full conversation
state and re-renders on every stream event. A custom `log-update` module diffs the screen
buffer to emit minimal ANSI output.

### Summary table

| Concern | smelt | kimi-cli | claude-code |
|---------|-------|----------|-------------|
| **Rendering model** | Imperative double-buffer diff | Transient Live + committed history | React reconciler + screen buffer diff |
| **Layout engine** | Yoga-like flexbox (editor chrome + splits) + parallel linear projection (transcript) | Rich's layout engine | Yoga flexbox |
| **Input framework** | crossterm EventStream | prompt-toolkit `PromptSession` | Custom Ink `useInput` + `useTextInput` |
| **Markdown rendering** | Custom block renderer (syntect for code) | Rich markdown + syntax highlighting | Custom React components |
| **Concurrent layout** | Yes — up to 8 Rayon-ish threads for blocks | No | No |
| **Virtual scroll** | `TranscriptProjection` with scroll anchor | Not present | `VirtualMessageList` (fullscreen mode) |

---

## 4. Input Capture and Keystroke-to-Submit Pipeline

### smelt

```
crossterm KeyEvent
  → dispatch_terminal_event() [priority cascade]
  → PromptState::handle_event() [completer → vim → paste → chords → keymap → char]
  → execute_key_action() [Submit → Content{text, attachments}]
  → process_input() [slash commands, shell escapes, → StartAgent]
  → start_agent() → UiCommand::StartTurn
```

The prompt buffer is a real `smelt_edit` buffer — the same editor used for file editing.
Text mutations go through `smelt_buffer::text` primitives (UTF-8 boundary-safe). Vim mode
is a first-class citizen via the vim bridge.

### kimi-cli

```
prompt-toolkit CustomPromptSession (key bindings, modal delegates)
  → Shell._route_prompt_events() → idle_events queue
  → Input event → slash dispatch or run_soul_command()
  → run_soul() [Wire creation, soul task, UI task, notification pump]
  → KimiSoul.run() [UserPromptSubmit hook, slash detection, _turn()]
```

Input is line-oriented. prompt-toolkit provides completers, history, and image paste
(via placeholder manager). Slash commands are dispatched from a unified index of soul-level
and shell-level commands.

### claude-code

```
Ink stdin raw-mode → EventEmitter → useInput
  → useTextInput [Cursor class, readline bindings]
  → PromptInput component [slash highlighting, typeahead, vim mode]
  → REPL.onSubmit() [history add, hooks]
  → REPL.onQuery() / QueryEngine.submitMessage()
  → query() loop
```

Input is React state. The `Cursor` class handles readline-style editing. `PromptInput` is
a 2339-line component managing slash command highlighting, typeahead suggestions, vim mode,
bash mode, and image paste.

### Key comparison

| Concern | smelt | kimi-cli | claude-code |
|---------|-------|----------|-------------|
| **Buffer type** | Real editor buffer (smelt_edit) | prompt-toolkit Buffer | React state (string) |
| **Vim mode** | Full bridge to smelt_edit vim engine | Not present | Basic vim mode |
| **Slash commands** | Lua command handlers | Two registries (soul-level + shell-level) | Parsed in PromptInput, dispatched in REPL |
| **Image paste** | Via attachment store + markers | Via placeholder manager | Via bash mode paste handler |
| **History** | `input_history` in TuiApp | prompt-toolkit history | `~/.claude-code/history.jsonl` |
| **Shell escape** | `!cmd` → spawn child | `PromptMode.SHELL` toggle | `--bash` mode flag |

---

## 5. LLM Abstraction Layer

### smelt — `Provider` trait (Rust)

```rust
Provider::chat(model, messages, tool_defs, options, cancel, on_delta) -> ChatResponse
```

`Provider` is a thin `reqwest::Client` wrapper. Provider kind selects URL and body builder:
- `OpenAiCompatible` → `/chat/completions`
- `Anthropic` / `AnthropicCompatible` → `/messages`
- `OpenAi` / `Codex` → `/responses`
- `Copilot` → proxied bearer

The body builders (`chat_completions.rs`, `anthropic.rs`, `openai.rs`) are separate Rust
modules with no shared abstraction — each speaks its own wire format.

### kimi-cli — `ChatProvider` protocol (Python)

```python
class ChatProvider(Protocol):
    async def generate(self, system_prompt, tools, history) -> AsyncIterable[StreamedMessagePart]
```

`kosong` is a first-party LLM abstraction package. Providers implement the `ChatProvider`
protocol. `kosong.generate()` layers delta merging over it; `kosong.step()` layers tool
dispatch over `generate`. Switching providers is a factory decision (`create_llm()`), not a
code change.

Provider implementations:
- `kimi.py` — Moonshot AI via `AsyncOpenAI` (SSE)
- `anthropic.py` — Anthropic SDK
- `openai_legacy.py` / `openai_responses.py` — OpenAI
- `google_genai.py` — Google GenAI / Vertex AI

### claude-code — Anthropic SDK (TypeScript)

```typescript
anthropic.beta.messages.create({ ...params, stream: true })
```

Claude Code is Anthropic-native. It uses `@anthropic-ai/sdk` directly — no provider
abstraction. Multi-provider support exists via SDK subclasses:
- `Anthropic` — direct API
- `AnthropicBedrock` — AWS Bedrock
- `AnthropicVertex` — Google Vertex
- `AnthropicFoundry` — Azure

### Key comparison

| Concern | smelt | kimi-cli | claude-code |
|---------|-------|----------|-------------|
| **Abstraction style** | Per-provider body builders, shared HTTP layer | `ChatProvider` protocol — full provider swap | Single SDK, subclass per cloud provider |
| **Providers supported** | OpenAI-compat, Anthropic, OpenAI native, Copilot, Codex | Moonshot, Anthropic, OpenAI, Google | Anthropic, Bedrock, Vertex, Foundry |
| **Thinking/reasoning** | `reasoning_effort` param, `StreamDelta::Thinking` | `with_thinking("high")` wrapper | `thinkingConfig`, `REDACT_THINKING_BETA_HEADER` |
| **Auth** | API key per provider, Copilot/Codex token refresh | OAuth via `OAuthManager`, env var overrides | OAuth, API key, AWS/GCP/Azure credential refresh |
| **Streaming protocol** | Raw `reqwest` SSE → `sse::read_events()` | `AsyncOpenAI` or SDK streaming | Anthropic SDK `beta.messages.stream()` |

---

## 6. The Agent / Turn Loop

This is the heart of each system.

### smelt — Engine actor with synchronous Tokio loop

```rust
// engine_task() — single tokio::select! loop
UiCommand::StartTurn → Turn::run() {
  loop {
    call_llm() → Provider::chat()
    if no tool_calls → push_assistant_message, emit TurnComplete, return
    classify_tools() → ToolExecutionPlan
    execute_concurrent() → FuturesUnordered
    run_sequential()
    collect_results() → messages.push(tool)
    // loop back to call_llm()
  }
}
```

The engine is a **separate Tokio task** (actor). The TUI communicates via
`mpsc` channels (`UiCommand` → engine, `EngineEvent` → TUI). The engine never touches
the TUI; the TUI never touches the engine directly.

History is assembled fresh each turn from `StartTurnPayload.history` — the TUI owns the
authoritative message list and ships it to the engine with every turn.

### kimi-cli — Soul + kosong coroutine loop

```python
# KimiSoul._agent_loop()
while not done:
    result = await kosong.step(
        chat_provider, system_prompt, toolset, effective_history,
        on_message_part=wire_send,
        on_tool_result=wire_send,
    )
    # await all tool_result_futures
    _grow_context(assistant_msg, tool_messages)
    # check max_steps_per_turn, auto-compaction
```

`kosong.step()` is a coroutine that wraps `kosong.generate()`. It dispatches tool calls
concurrently via `asyncio.create_task()` as they stream in (`on_tool_call` callback), then
returns a `StepResult` with futures. The soul `await`s all futures before growing context.

The `Wire` SPMC channel is the side-channel for streaming output to the UI — the soul loop
does not need to yield control to the UI between tool executions.

### claude-code — `query()` async generator loop

```typescript
// queryLoop() — while(true) with continue sites
while (true) {
  // pre-processing: compaction, budgets, microcompact
  yield { type: 'stream_request_start' }
  for await (const message of deps.callModel(...)) {
    yield message  // passes StreamEvents to REPL live
    addTool() to StreamingToolExecutor
  }
  getRemainingResults() // yield buffered tool results in order
  if (!needsFollowUp) break
}
return { reason: 'end_turn' | 'max_turns' | ... }
```

The generator **yields values directly to the REPL** — there is no separate channel. The
REPL consumes the generator with `for await...of` and updates React state on each event.
`StreamingToolExecutor` starts executing concurrent-safe tools as their blocks arrive during
streaming, before the stream closes.

### Key comparison

| Concern | smelt | kimi-cli | claude-code |
|---------|-------|----------|-------------|
| **Loop model** | Separate Tokio actor, channel IPC | Coroutine inside soul task | Async generator, REPL consumes directly |
| **Message history owner** | TUI sends history to engine each turn | `Context` object inside `KimiSoul` | `QueryEngine` / `State.messages` inside loop |
| **Streaming to UI** | `EngineEvent` over mpsc channel | `Wire.soul_side.send()` → broadcast queue | `yield` from generator |
| **Tool start during stream** | Tools classified after stream ends | `on_tool_call` fires mid-stream, `create_task()` | `addTool()` mid-stream, `StreamingToolExecutor` |
| **Multi-iteration state** | Fresh `Turn` per start_turn | `_agent_loop` loop variable | `State` struct with `Continue` reason |
| **Max retries** | Provider retry via `CancellationToken` | Retry in `kosong` layer | `MAX_OUTPUT_TOKENS_RECOVERY_LIMIT = 3` |

---

## 7. Tool Dispatch and Execution

### Architecture overview

| Layer | smelt | kimi-cli | claude-code |
|-------|-------|----------|-------------|
| **Tool definition** | `ToolDef` (Lua) + `ToolDispatcher` trait (Rust) | `CallableTool2[T]` Pydantic style | `Tool` interface with Zod schema |
| **Tool registry** | `shared.tools` (Lua map) + `McpDispatcher` | `KimiToolset._tool_dict` | `getAllBaseTools()` + `assembleToolPool()` |
| **DI for tools** | Lua environment table + closure capture | `__init__` type annotation injection | `ToolUseContext` passed to `tool.call()` |
| **Concurrency partitioning** | `ToolExecutionPlan`: concurrent / sequential | `asyncio.create_task()` for all, same-step dedup prevents double-execution | `StreamingToolExecutor`: concurrent-safe / exclusive |
| **Concurrent execution** | `FuturesUnordered` (Tokio) | `asyncio.Task` with `Future` per call | `Promise` array, resolved in registration order |
| **Sequential execution** | `run_sequential()` — dispatches one, awaits result | Not explicit; dedup prevents parallel calls of the same tool | Non-concurrent tools wait for all concurrent ones to finish |
| **Result ordering** | Completion time non-deterministic; results keyed by `tool_call_id` and assembled in call order | Futures keyed by `tool_call_id` | Results yielded in registration order (not completion order) |

### Same-tool deduplication

| | smelt | kimi-cli | claude-code |
|-|-------|----------|-------------|
| **Same-step dedup** | Not present (engine handles tools once per loop) | Yes — `begin_step()` / `end_step()` track `(name, arguments)` hash; second call awaits first | Not present (tool IDs are unique per response) |
| **Cross-step dedup** | Not present | Yes — nag reminder appended to context | Not present |

### MCP integration

| | smelt | kimi-cli | claude-code |
|-|-------|----------|-------------|
| **MCP client** | `McpDispatcher` wrapping `McpManager` | `fastmcp.Client` per server | Anthropic SDK MCP clients |
| **Init timing** | At startup, passed to engine | Background `asyncio.Task` (optional) | During `init()`, before REPL |
| **Auth gating** | Not mentioned | OAuth authorization check before connect | Not mentioned |
| **Output budget** | Not mentioned | 100,000 chars hard limit per result | Not mentioned |
| **Tool wrapping** | `ToolDispatcher::dispatch()` → `ToolFuture` | `MCPTool` extending `CallableTool` | Assembled into tool pool alongside builtins |

---

## 8. Permission System

The three systems converge on similar goals (gate dangerous tool calls) but take very
different implementation paths.

### smelt — `evaluate_hooks()` + `Decision` enum

```rust
dispatcher.evaluate_hooks(name, args, mode) -> Option<ToolHooks>
// Decision::Allow | Deny | Ask | Error
```

Permission evaluation happens **in the engine** before tool dispatch. The engine actor
emits `EngineEvent::RequestPermission`, which pauses `EngineClient::recv()` — the TUI
blocks on confirms by returning `pending()` from `try_recv()`. The Lua `decide` callback
can veto a tool call before it reaches the engine.

### kimi-cli — `Approval.request()` + `ApprovalRuntime` + `RootWireHub`

```python
Approval.request(action, sender, preview)
  → ApprovalRuntime.create_request()
  → RootWireHub.publish_nowait(ApprovalRequest)
  → UI consumers race to resolve
  → ApprovalRuntime.resolve(request_id, response)
```

The approval system is a **broadcast**: `RootWireHub` delivers the `ApprovalRequest` to
all connected UI consumers simultaneously (shell, web, vis, IDE via WireServer). Any
consumer can resolve it. Tools `await` the future.

Approval granularity: approve-once / approve-for-session / reject / reject-with-feedback.

### claude-code — `CanUseToolFn` callback + permission cascade

```typescript
canUseTool(tool, input, toolUseContext, assistantMessage, toolUseID)
  → hasPermissionsToUseTool() [rule matching]
  → alwaysAllow → alwaysDeny → alwaysAsk → mode → ask user
```

`CanUseToolFn` is a **callback parameter** — different implementations are passed for
different modes (interactive REPL, headless, swarm worker, coordinator). The REPL
implementation pushes to a React state queue; the permission dialog renders synchronously
in the TUI.

### Key comparison

| Concern | smelt | kimi-cli | claude-code |
|---------|-------|----------|-------------|
| **Decision point** | Engine (before dispatch) | Tool call site (before `tool.__call__()`) | Tool execution (`runToolUse`, before `tool.call()`) |
| **Blocking mechanism** | `EngineClient::recv()` returns `pending()` | `asyncio.Future` awaited in tool | `Promise` awaited in `canUseTool()` |
| **Multi-consumer approval** | No (single TUI dialog) | Architecturally yes — `RootWireHub` broadcasts to all consumers; in practice the typical deployment has a single active resolver | No (single REPL dialog) |
| **Permission modes** | `Decision::Allow/Deny/Ask/Error` | `yolo`, `afk`, per-tool patterns, `ApprovalRuntime` | 7 modes: `default`, `acceptEdits`, `bypassPermissions`, `dontAsk`, `plan`, `auto`, `bubble` |
| **Rule storage** | Lua `approval_patterns`, `decide` callback | Config + session state | `ToolPermissionRulesBySource` (user/project/local/flag/policy/cli/command/session) |
| **Auto-approve** | Lua `decide` returning `Allow` | `yolo` flag / `afk` mode | `bypassPermissions` mode / YOLO classifier (ML, `auto` mode) |
| **Swarm delegation** | Not present | Not present | `'bubble'` mode → swarm coordinator |

---

## 9. Streaming Architecture

### Delta path

```
smelt:       Provider → StreamDelta → on_delta closure → EngineEvent → mpsc channel → TUI dispatch
kimi-cli:    ChatProvider → StreamedMessagePart → on_message_part → wire.soul_side.send() → broadcast queue → visualize_loop()
claude-code: SDK → BetaRawMessageStreamEvent → stream loop → yield StreamEvent → REPL.onQueryEvent()
```

### Merging / coalescing

| | smelt | kimi-cli | claude-code |
|-|-------|----------|-------------|
| **Text merging** | `StreamParser.append_streaming_text()` accumulates into lines, detects block boundaries | `merge_in_place()` in `kosong.generate()` coalesces adjacent `TextPart` deltas | `content_block_delta` events append to accumulator; full block emitted on `content_block_stop` |
| **Merge location** | Stream parser (TUI side) | LLM abstraction layer (kosong) | API layer (claude.ts stream consumer) |
| **Wire merging** | Not applicable | `WireSoulSide` merge buffer for `MergeableMixin` messages | Not applicable |

### Tool argument streaming

| | smelt | kimi-cli | claude-code |
|-|-------|----------|-------------|
| **Args stream** | `EngineEvent::ToolArgsDelta { call_id, tool_name, delta }` | `ToolCallPart` Wire messages | `input_json_delta` content block delta |
| **Tool start trigger** | `ToolStarted` event (after args complete) | `on_tool_call` fires when `ToolCall` fully assembled | `addTool()` called when `content_block_stop` for a `tool_use` block |
| **Parallel start** | After full response → `ToolExecutionPlan` | During stream — `create_task()` on `on_tool_call` | During stream — `addTool()` starts concurrent-safe tools immediately |

### Backpressure / flow control

| | smelt | kimi-cli | claude-code |
|-|-------|----------|-------------|
| **Backpressure** | `mpsc::unbounded_channel` — no backpressure | `BroadcastQueue.publish_nowait()` — no backpressure; `QueueShutDown` on teardown | JS event loop — generator `yield` is cooperative |
| **Stream watchdog** | `CancellationToken` polled every SSE chunk | Not mentioned | 90s timeout via `setTimeout` (`CLAUDE_ENABLE_STREAM_WATCHDOG`) |
| **Retry on stream fail** | `CancellationToken` + provider-level retry | `kosong` layer retry | `withRetry()` — up to 10 attempts; fallback model on 3× 529 |

---

## 10. Context Management and Compaction

### smelt — No compaction (yet)

Smelt does not implement context compaction. It sends the full `history` array to the
engine on each turn. The engine builds `[system_prompt, …history, user_content]` fresh.
A `Compacted` block type exists in `BlockHistory` (for displaying compacted summaries),
but the mechanism to trigger compaction is not described in the analysis.

`/compact` is available as a Lua command, suggesting it is triggered manually.

### kimi-cli — `SimpleCompaction` via secondary LLM call

```python
# should_auto_compact() → if True:
SimpleCompaction(secondary_llm).compact(context)
  → secondary LLM call (system prompt = compaction instructions)
  → replaces context.messages with [summary_message]
  → wire.soul_side.send(CompactionBegin / CompactionEnd)
```

Compaction is triggered by `should_auto_compact()` inside `_agent_loop()`. It uses a
secondary LLM call (may be the same provider) and replaces the full history with a single
summary message. `PreCompact` hook fires before compaction.

### claude-code — Multi-strategy compaction (feature-flagged)

| Strategy | Trigger | Mechanism |
|---------|---------|-----------|
| **Auto-compact** | Token count ≥ threshold | Secondary `query()` call with `querySource='compact'`; replaces history with `CompactBoundaryMessage` + summary |
| **Reactive compact** | `prompt_too_long` API error during stream | Intercepts error, compacts, retries (`REACTIVE_COMPACT` flag) |
| **Microcompact** | Per-iteration | Operates by `tool_use_id` — content replacement without inspecting content; cache-transparent |
| **Context collapse** | Per-iteration | Commit-log of collapse operations; projects a view (`CONTEXT_COLLAPSE` flag) |
| **Snip** | Per-iteration | Truncates old history; injects snip boundary markers (`HISTORY_SNIP` flag) |

All strategies share `getMessagesAfterCompactBoundary()` — messages before the boundary
are stripped from API calls but preserved in the transcript.

### Comparison

| Concern | smelt | kimi-cli | claude-code |
|---------|-------|----------|-------------|
| **Compaction** | Manual `/compact` command | Automatic `SimpleCompaction` | 5 strategies, feature-flagged |
| **Secondary LLM** | Not mentioned | Yes — same or different provider | Yes — `querySource='compact'`, excluded from blocking limit check |
| **History preservation** | Full history sent each turn | Context replaced with summary | `CompactBoundaryMessage` preserves transcript while stripping API payload |
| **Token tracking** | Not mentioned | `should_auto_compact()` checks count | `calculateTokenWarningState()` + `tokenCountWithEstimation()` |
| **Prompt caching** | Not applicable (raw reqwest) | Via provider's caching support | `cache_control: ephemeral` per-boundary; 1h TTL option |

---

## 11. Hook System

All three systems support user-defined shell hooks at lifecycle events, but differ in
scope and protocol.

### smelt — Lua callbacks + `evaluate_hooks`

Hooks are Lua functions registered via `smelt.tools.register`:
- `decide(name, args, mode)` → `Decision` — permission veto before tool runs
- `preflight(name, args)` → run before execution
- `approval_patterns` — glob patterns for auto-approve

Tool middleware: `tools.middleware{before=..., after=...}` runs around every tool call.

Lua timers and task runtime allow time-based callbacks. No shell-subprocess hook mechanism.

### kimi-cli — Shell subprocess hooks (8 events)

| Event | When | Can block? |
|-------|------|-----------|
| `UserPromptSubmit` | Before soul run | Yes — `continue: false` stops the turn |
| `PreToolUse` | Before tool execution | Yes — `decision: block` denies |
| `PostToolUse` | After tool result | No |
| `PostToolUseFailure` | After tool error | No |
| `Stop` | After turn end | Yes — can re-trigger |
| `PreCompact` | Before compaction | No |
| `HookRequest` | From wire client | Yes — wire consumers resolve |
| `Notification` | When notification fires | No |

Hooks output JSON to stdout. `HookRequest` is unique — it allows wire-connected IDE clients
to evaluate hooks on the IDE side.

### claude-code — Shell subprocess hooks (5 events)

| Event | When | Can block? |
|-------|------|-----------|
| `UserPromptSubmit` | After Enter, before query | Yes — sync |
| `PreToolUse` | Inside `canUseTool`, before execution | Yes — `decision: block` denies; `updatedInput` rewrites args |
| `PostToolUse` | After tool result assembled | No |
| `Stop` | After model `end_turn` | Yes — can trigger loop retry |
| `Notification` | When Claude sends notification | No |

### Key comparison

| Concern | smelt | kimi-cli | claude-code |
|---------|-------|----------|-------------|
| **Hook language** | Lua (in-process) | Shell subprocess (JSON stdio) | Shell subprocess (JSON stdio) |
| **Tool arg rewrite** | Via `decide` return value | Not mentioned | `updatedInput` in `PreToolUse` response |
| **Permission via hook** | `Decision::Allow/Deny` from `decide` | `decision: block/approve` from `PreToolUse` | `decision: approve/block` from `PreToolUse` |
| **IDE hook delegation** | Not present | `HookRequest` over WireServer | Not present |
| **Middleware** | `tools.middleware{before, after}` | Not present | `runPreToolUseHooks()` / `runPostToolUseHooks()` |

---

## 12. Subagents and Multi-Agent Architecture

| Concern | smelt | kimi-cli | claude-code |
|---------|-------|----------|-------------|
| **Subagent spawning** | Not present (no `Agent` tool in analysis) | `LaborMarket` + `SubagentStore`, `Agent` tool | `AgentTool`, `runForkedAgent()` / `runSubagent()` |
| **Subagent isolation** | N/A | Separate `KimiSoul` instance, separate Wire | Separate `query()` call, separate `agentId` |
| **Permission delegation** | N/A | `SubagentEvent` on parent Wire | `'bubble'` permission mode → swarm coordinator |
| **Swarm coordinator** | N/A | Not present | Yes — coordinator owns TUI + permission prompt; workers delegate via `handleSwarmWorkerPermission()` |
| **Inter-agent messaging** | N/A | `SendDMail` tool — passes structured messages to named agents across sessions | Not present (coordinator/worker IPC via channel) |
| **Result summary** | N/A | Not detailed | `AgentSummary` service → `ToolUseSummaryMessage` |
| **History rewind** | Undo ring in buffer | `BackToTheFuture` exception — unwinds the asyncio call stack to a named checkpoint in conversation history | `TombstoneMessage` / compaction boundary |

smelt has no subagent system — it is a single-agent editor. kimi-cli's primary subagent
system is `LaborMarket` + `SubagentStore` + the `Agent` tool; D-Mail / `BackToTheFuture`
is a separate, more experimental mechanism for inter-session IPC and history rewind.
claude-code has the most complete swarm infrastructure with coordinator/worker permission
delegation.

---

## 13. Session Persistence and Headless Modes

### Session storage

| Concern | smelt | kimi-cli | claude-code |
|---------|-------|----------|-------------|
| **Session file** | Implicit via `session.rs` | `wire.jsonl` (merged Wire messages), session TOML | `~/.claude-code/` session files, content replacement records |
| **Resume** | Not detailed | `/resume` slash command → `Reload` exception | `--continue`, `--resume`, `--fork-session` flags |
| **History format** | Not detailed | JSONL Wire messages (replayable) | NDJSON session events |
| **Compaction persistence** | N/A | Summary replaces context | `CompactBoundaryMessage` in session |

### Headless modes

| Mode | smelt | kimi-cli | claude-code |
|------|-------|----------|-------------|
| **Non-interactive** | `HeadlessApp::run_oneshot()` — single `StartTurn`, auto-approves in yolo mode | `Print` — read input → run soul → output | `--print` / `runHeadless()` |
| **Structured output** | `Json` NDJSON stream | JSON output format | `--output-format=stream-json` NDJSON |
| **IDE integration** | Not present | `WireServer` — JSON-RPC over stdio | Bridge / remote control (`bridgeMain.ts`) |
| **Web UI** | Not present | `FastAPI` + WebSocket | Not present |
| **MCP server mode** | Not present (MCP as tool *client* only) | `ACP` — Agent Client Protocol server | `--claude-in-chrome-mcp`, `--computer-use-mcp` |
| **SDK** | Not present | `run_acp()` / `run_wire_stdio()` | TypeScript SDK (`entrypoints/sdk/`) |

---

## 14. Extension / Plugin Architecture

| Concern | smelt | kimi-cli | claude-code |
|---------|-------|----------|-------------|
| **Extension language** | Lua (in-process, mlua) | Python (in-process, importlib DI) | TypeScript (in-process, `feature()` gates) |
| **Tool registration** | `smelt.tools.register(name, execute, ...)` | `CallableTool2[T]` class, YAML agent spec | `buildTool(def)`, `getAllBaseTools()` |
| **Command registration** | `smelt.cmd.register(name, ...)` | YAML `allowed_tools` / `exclude_tools` | Slash commands are hardcoded; **skills** injected into system prompt serve a similar discovery role |
| **Keymap registration** | `smelt.keymap.set(mode, chord, fn)` | Not present | Not present |
| **Theme registration** | `smelt.theme.set(...)` | `/theme` switch via `Reload` | Not present |
| **Plugin discovery** | `~/.config/smelt/plugins/*.lua` | Plugin tools via YAML | `src/services/plugins/` |
| **Agent spec** | Not present | YAML agent spec with `extend` inheritance | Not present (single agent, tools configured at runtime) |
| **Skills system** | Lua skill section in system prompt | `KIMI_SKILLS` in Jinja2 system prompt template | Loaded at `init()`, bundled + custom; injected into system prompt — closest analogue to slash command extensibility |
| **Feature flags** | Lua `smelt.feature(name)` | Python import guards | `feature('...')` via GrowthBook + `bun:bundle` dead-code elimination |

smelt is the most extensible at the TUI layer — Lua can register keymaps, themes, commands,
and custom dialogs. kimi-cli is the most extensible at the agent layer — YAML specs allow
defining entirely different agents with different system prompts, tools, and subagents.
claude-code's primary user-facing extensibility mechanism is the **skills system** (loaded
at init, injected into the system prompt); its slash commands and plugin API are largely
internal build-time constructs.

---

## 15. System Prompt Construction

| Concern | smelt | kimi-cli | claude-code |
|---------|-------|----------|-------------|
| **Source** | `build_system_prompt_full()` — priority: TUI override → config override → built-in | Jinja2 template from YAML agent spec + `_load_system_prompt()` | Assembles `systemPrompt` + `userContext` + `systemContext` |
| **Dynamic injection** | AGENTS.md content | Jinja2 builtins: `KIMI_NOW`, `KIMI_WORK_DIR`, `KIMI_AGENTS_MD`, `KIMI_SKILLS`, `KIMI_OS`, `KIMI_SHELL` | `userContext` dict (date, CLAUDE.md) + `systemContext` dict (git status, cache breaker) |
| **Per-step injection** | Not mentioned | Dynamic providers: plan-mode reminders, AFK reminders injected as `<system-reminder>` user messages | Not present (static per-turn) |
| **Skills** | Lua skill section | `KIMI_SKILLS` in system prompt; discovered from filesystem | Skills loaded via `init()`, referenced in system prompt |
| **Cache breakpoints** | Not present | Provider-dependent | `splitSysPromptPrefix()` + `cache_control: ephemeral` at system prompt boundary |

---

## 16. Full Pipeline Comparison

```
                smelt                    kimi-cli                 claude-code
                ─────                    ────────                 ───────────
KEYSTROKE       crossterm EventStream    prompt-toolkit           Ink raw-mode stdin
                                         CustomPromptSession       EventEmitter

INPUT           PromptState              Shell._route_prompt_     useTextInput (Cursor)
HANDLING        ::handle_event()          events() + slash        PromptInput onSubmit
                (vim bridge, keymap,      dispatch
                 completer)

SUBMIT          Action::Submit           run_soul_command()       REPL.onSubmit()
                → process_input()         → run_soul()             → onQuery()
                → UiCommand::StartTurn    → KimiSoul.run()         → query()

HISTORY         TUI sends full history   Context owns history     State.messages in
OWNERSHIP       to engine each turn      inside KimiSoul           queryLoop()

SYSTEM          build_system_prompt_     Jinja2 from YAML        appendSystemContext()
PROMPT          full()                   + dynamic injections     + prependUserContext()

LLM             Provider::chat()         kosong.step()           deps.callModel()
CALL            (reqwest, SSE)            → ChatProvider.generate() → SDK stream

STREAMING       EngineEvent::TextDelta   Wire.soul_side.send()   yield StreamEvent
TRANSPORT       → mpsc channel           → BroadcastQueue         from async generator

STREAM          StreamParser             kosong merge_in_place() content_block_delta
PARSING         ::append_streaming_text() + WireSoulSide          accumulator
                (block detection)         merge buffer

TOOL            classify_tools()         KimiToolset.handle()    StreamingToolExecutor
DISPATCH        → ToolExecutionPlan       → asyncio.create_task() .addTool() mid-stream
                → FuturesUnordered

PERMISSION      evaluate_hooks()         Approval.request()      canUseTool() callback
GATE            → EngineEvent::Request    → ApprovalRuntime        → hasPermissionsToUseTool()
                Permission               → RootWireHub broadcast   → permission cascade

UI UPDATE       EngineEvent dispatch     Wire.ui_side.receive()  yield → REPL.onQueryEvent()
                → StreamParser           → visualize_loop()        → setMessages()
                → BlockHistory.rewrite() → Rich Live update         → React re-render

RENDER          TranscriptProjection     _LiveView compose +      Ink reconciler
PIPELINE        ::project() parallel     console.print() commit   → Yoga layout
                layout → Compositor      + prompt-toolkit ANSI     → screen buffer
                double-buffer diff       injection                  → log-update diff

OUTPUT          crossterm MoveTo+Print   Rich/ANSI to stdout      ANSI escapes to stdout
                (synchronized update)    + FormattedText layout
```

---

## 17. Verdict: Design Philosophy

### smelt — *Performance and correctness through architecture*

The dominant concern is rendering fidelity and edit correctness. The compositor double-buffer,
parallel block layout, content-addressed block store, and UTF-8 boundary-safe text primitives
all exist to make a large transcript render fast and a large file edit safely. The engine actor
pattern isolates LLM I/O — the TUI render loop never stalls on network. Lua sits on top as a
scripting layer, not as the core.

### kimi-cli — *Flexibility through protocol and declarative configuration*

The dominant concern is making the agent's behavior controllable and observable from multiple
surfaces. The `Wire` SPMC channel is one linchpin: any number of UI consumers (shell, web,
vis, IDE) can subscribe without the soul knowing about them. The YAML agent spec + `extend`
inheritance is the other: it enables defining entirely different agents — different system
prompts, toolsets, and subagent hierarchies — without modifying Python code. The `kosong`
provider abstraction makes switching LLM backends a config decision. The approval broadcast
via `RootWireHub` means any connected client can gate a tool call.

### claude-code — *Throughput and safety at scale*

The dominant concern is running safely in automated and agentic contexts. The `CanUseToolFn`
callback parameterization means the same `query()` loop works for interactive REPL,
headless SDK, swarm workers, and coordinators with different permission behaviors injected.
The multi-strategy compaction pipeline (5 strategies, feature-flagged, with circuit breakers)
manages a very large context budget. `StreamingToolExecutor` starts concurrent-safe tools
mid-stream to maximize throughput. The `feature()` + GrowthBook system enables controlled
rollout of new capabilities to large user populations.

---

## 18. Cross-Cutting Theme: Mutability Architecture

A theme that cuts across all seventeen sections above is each system's stance toward
mutability. The language choice is not incidental — it reflects a deliberate philosophy
that shapes rendering, concurrency, and the entire event transport model.

| System | Mutability model | Why it follows from the language |
|--------|-----------------|----------------------------------|
| **smelt** | **Aggressively immutable** — content-addressed `BlockHistory`, append-only block store, pure `TranscriptProjection` | Rust's ownership model makes shared mutable state expensive; immutability is the path of least resistance. Immutable blocks can be safely shared across threads without locks, which is what enables the parallel layout workers. |
| **kimi-cli** | **Mutable object-oriented** — `Context` mutated in-place, `KimiSoul` holds and grows `messages` across steps | Python's object model normalizes shared mutable state. The `Wire` side-channel exists precisely because the soul mutates state that UI consumers must *observe* — if the soul's state were immutable values, consumers could hold references directly. |
| **claude-code** | **React-mutable** — Zustand store with immutable snapshots, `setState` triggers reconciler | JavaScript/React's model: state is replaced (not mutated), triggering a diff. The reconciler makes re-rendering the entire `REPL` on every stream delta cheap enough that no finer-grained invalidation is needed. |

This spectrum explains several downstream choices that otherwise look arbitrary:

- **Why smelt needs parallel layout workers** — immutable blocks have stable identity and
  content, so their layout can be cached and recomputed on any thread without coordination.
  A mutable block store would require locks or a copy-per-worker strategy.

- **Why kimi-cli needs a broadcast queue** — the soul mutates conversation state that
  multiple consumers (shell, web, vis, IDE) need to track. If the state were immutable and
  passed by value, consumers could snapshot it directly; because it evolves in-place,
  the `Wire` protocol must push deltas outward.

- **Why claude-code re-renders `REPL` on every stream event** — React's reconciler
  diffing makes this cheap enough to be the default. The reconciler handles granular
  invalidation; the application code does not need to. In a mutable imperative system
  (like smelt's compositor), re-running the full render pass on every text delta would
  require the double-buffer diff to suppress unnecessary terminal writes.

### Failure isolation follows the same spectrum

| System | Failure model |
|--------|--------------|
| **smelt** | Rust `Result`/`Option` throughout; the engine actor runs in a separate Tokio task — a panic there does not stall the TUI render loop, which continues to process input and redraw |
| **kimi-cli** | Python exceptions used as control flow: `Reload` resets session state, `BackToTheFuture` unwinds the agent stack, `QueueShutDown` tears down the Wire — exceptions are first-class state machine transitions |
| **claude-code** | TypeScript Promise chains with recovery loops: `withRetry()` with up to 10 attempts, fallback model on persistent 529 errors, `MAX_OUTPUT_TOKENS_RECOVERY_LIMIT` as a circuit breaker — failures are expected and budgeted for |

The smelt model isolates failures by process boundary (actor). The kimi-cli model absorbs
failures by catching and converting them to state transitions. The claude-code model
retries failures with exponential backoff and graceful degradation.
