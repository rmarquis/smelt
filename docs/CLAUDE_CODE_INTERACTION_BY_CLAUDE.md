# Claude Code: Full Interaction Map (Keystroke → LLM → Render)

> **Source**: Extracted TypeScript source from `@anthropic-ai/claude-code` v2.1.88 npm package
> (`cli.js.map` source map, committed to `claude-code/` in this repo).
> **Author of analysis**: Claude Sonnet 4.6, 2026-05-18.

---

## Overview

Claude Code is a TypeScript CLI built on **React/Ink** for terminal rendering and the **Anthropic
SDK** for streaming API calls. It supports two primary runtime modes:

- **Interactive REPL** — a full TUI driven by React/Ink, prompt input, and real-time streaming
- **Headless SDK mode** — a non-interactive API where callers receive `StreamEvent` / `Message`
  objects directly (used by the Agent SDK, desktop integrations, CI, etc.)

The central pipeline is an **async generator chain**:

```
user input → normalizeMessages → query() → makeAPIRequest() → stream events → tool dispatch → React render
```

All paths converge on the `query()` generator in `src/query.ts`, which owns the full agentic loop.

---

## Phase 1 — CLI Entry Point (`src/main.tsx`)

`main.tsx` (785 KB compiled) is the dual-mode entry point.

### Interactive mode startup

```
$ claude [flags]
    ↓
main.tsx:main()
    ↓
React.render(<App />, stdout)            // Ink mounts the TUI
    ↓
AppState initialized via getAppState()   // Redux-like singleton
    ↓
initReplBridge()                         // Optional bridge to desktop app
    ↓
<PromptInput> rendered                   // Ink component — awaits keystrokes
```

### Headless / SDK mode startup

```
claude --headless / SDK caller
    ↓
main.tsx headless branch
    ↓
query(params) called directly            // No React, no Ink
    ↓
StreamEvent | Message yielded to caller
```

### Key globals initialized at startup

| Name | Location | Purpose |
|------|----------|---------|
| `AppState` | `src/state/AppState.ts` | Central mutable store (model, permission context, session ID, …) |
| `bootstrap/state.ts` | `src/bootstrap/state.ts` | Session-scoped singletons: `getSessionId()`, `getLastApiCompletionTimestamp()`, prompt cache allowlists |
| `GrowthBook` | `src/services/analytics/growthbook.ts` | Feature flags (`feature('...')`) — many codepaths gated here |

---

## Phase 2 — Keystroke Capture and Input Processing

In interactive mode, Ink renders a `<PromptInput>` component that captures raw terminal events.

```
Terminal keystroke
    ↓
Ink's <TextInput> / readline handler
    ↓
User presses Enter
    ↓
onSubmit callback
    ↓
handleUserPromptSubmit()                 // src/interactiveHelpers.tsx
    ↓
executeHooks('UserPromptSubmit', ...)    // shell hooks run here (sync)
    ↓
createUserMessage(content)              // wraps string in Message type
    ↓
dispatch to message queue
```

### Message queue

Slash commands (e.g., `/compact`, `/clear`) are injected into `messageQueueManager`
(`src/utils/messageQueueManager.ts`) and consumed before the next query iteration.
`getCommandsByMaxPriority()` sorts them by priority; `isSlashCommand()` identifies them.

---

## Phase 3 — Message Normalization

Before any API call, the message history is normalized:

```
Message[]  (internal type, includes UI-only fields)
    ↓
normalizeMessagesForAPI(messages)        // src/utils/messages.ts
    ↓
BetaMessageParam[]  (SDK wire type)
```

Key transforms applied:
- Strip UI-only message types (tombstones, attachment-only messages not carrying tool results)
- `stripSignatureBlocks()` — removes thinking/redacted-thinking from messages that must not carry them
- `stripAdvisorBlocks()` — removes advisor-tool blocks from non-1P builds
- `stripCallerFieldFromAssistantMessage()` — strips internal routing metadata
- `stripToolReferenceBlocksFromUserMessage()` — removes tool reference extensions
- `ensureToolResultPairing()` — enforces that every `tool_use` block has a matching `tool_result`
- Inject `userContext` prefix via `prependUserContext()` (dynamic system prompt injection)
- Append `systemContext` via `appendSystemContext()` (e.g., current date, env info)

---

## Phase 4 — The Query Loop (`src/query.ts`)

`query()` is a thin wrapper over `queryLoop()`. Everything agentic lives inside `queryLoop()`.

```typescript
export async function* query(params: QueryParams): AsyncGenerator<...> {
  const consumedCommandUuids: string[] = []
  const terminal = yield* queryLoop(params, consumedCommandUuids)
  for (const uuid of consumedCommandUuids) {
    notifyCommandLifecycle(uuid, 'completed')
  }
  return terminal
}
```

### `QueryParams` type

```typescript
type QueryParams = {
  messages: Message[]
  systemPrompt: SystemPrompt
  userContext: { [k: string]: string }
  systemContext: { [k: string]: string }
  canUseTool: CanUseToolFn          // permission callback — varies by mode
  toolUseContext: ToolUseContext     // tools, model, abort controller, options…
  fallbackModel?: string
  querySource: QuerySource          // 'repl_main_thread' | 'agent:…' | 'compact' | …
  maxOutputTokensOverride?: number
  maxTurns?: number
  skipCacheWrite?: boolean
  taskBudget?: { total: number }    // task-budgets beta
  deps?: QueryDeps                  // injectable deps for testing
}
```

### Loop state (`State` type)

```typescript
type State = {
  messages: Message[]
  toolUseContext: ToolUseContext
  autoCompactTracking: AutoCompactTrackingState | undefined
  maxOutputTokensRecoveryCount: number   // capped at MAX_OUTPUT_TOKENS_RECOVERY_LIMIT = 3
  hasAttemptedReactiveCompact: boolean
  maxOutputTokensOverride: number | undefined
  pendingToolUseSummary: Promise<...> | undefined
  stopHookActive: boolean | undefined
  turnCount: number
  transition: Continue | undefined       // why the previous iteration continued
}
```

### Iteration skeleton

Each loop iteration follows this order:

```
1. Skill discovery prefetch (non-blocking, fires in parallel)
2. yield { type: 'stream_request_start' }
3. Apply tool result budget (trim oversized tool outputs)
4. Apply snip compact (HISTORY_SNIP feature flag)
5. Apply microcompact / cached microcompact
6. Apply context collapse (CONTEXT_COLLAPSE feature flag)
7. Build full system prompt
8. Auto-compact check → maybe compact + yield boundary messages + continue
9. Blocking limit check → maybe yield error and return
10. API streaming loop:
      for await (const message of deps.callModel(...)) { ... }
          ↓ yield StreamEvents upstream
          ↓ addTool() to StreamingToolExecutor as tool_use blocks arrive
11. Tool execution (streaming or batch)
12. yield tool result messages
13. Continue? → check for tool_use blocks → if yes, loop
14. Stop hooks / stop hook retry
15. return Terminal reason
```

---

## Phase 5 — Compaction Pipeline

Three compaction strategies, selected by feature flags:

### Auto-compact (`services/compact/autoCompact.ts`)

Triggered when token count exceeds a threshold. Calls a secondary LLM to summarize
the conversation, then replaces the history with the summary.

```
calculateTokenWarningState(tokenCount, model)
    ↓ if at threshold
deps.autocompact(messagesForQuery, ...)
    ↓
secondary LLM call (querySource = 'compact')
    ↓
buildPostCompactMessages(compactionResult)
    ↓
yield boundary messages
    ↓
messagesForQuery = postCompactMessages
    ↓ loop continues with compressed history
```

### Reactive compact (`services/compact/reactiveCompact.ts`)

Feature-flagged (`REACTIVE_COMPACT`). Intercepts a `prompt_too_long` API error during
streaming and triggers compaction reactively rather than proactively.

### Context collapse (`services/contextCollapse/index.ts`)

Feature-flagged (`CONTEXT_COLLAPSE`). Projects a collapsed view of the conversation
by replaying a commit log of collapse operations. Runs before auto-compact so that
collapse may prevent auto-compact from firing.

### Microcompact (`services/compact/microCompact.ts`)

Fine-grained cached compaction. Operates purely by `tool_use_id` — replaces content
references without inspecting content, invisible to the cache layer. Runs before
auto-compact.

---

## Phase 6 — API Request (`src/services/api/claude.ts`)

`deps.callModel()` dispatches to `makeAPIRequest()` in `services/api/claude.ts`,
which calls the Anthropic SDK's `beta.messages.stream()`.

### Request construction

```typescript
// Assembled inside makeAPIRequest():
{
  model,
  max_tokens,
  system: [
    { type: 'text', text: syspromptPrefix, cache_control: { type: 'ephemeral' } },
    { type: 'text', text: userSysprompt,   cache_control: { type: 'ephemeral' } },
  ],
  messages: normalizedMessages,
  tools: toolToAPISchema(tools),
  stream: true,
  betas: getMergedBetas(model),   // e.g. 'prompt-caching-2024-07-31', 'tools-2024-04-04'
}
```

### Beta headers sent

| Header | Purpose |
|--------|---------|
| `PROMPT_CACHING_SCOPE_BETA_HEADER` | Enables prompt caching with TTL |
| `EFFORT_BETA_HEADER` | Extended thinking / effort levels |
| `FAST_MODE_BETA_HEADER` | Fast mode (Opus with faster output) |
| `AFK_MODE_BETA_HEADER` | AFK mode |
| `CONTEXT_MANAGEMENT_BETA_HEADER` | API-side context management |
| `TASK_BUDGETS_BETA_HEADER` | Task token budgets |
| `STRUCTURED_OUTPUTS_BETA_HEADER` | Structured output format |
| `REDACT_THINKING_BETA_HEADER` | Thinking redaction |
| `CONTEXT_1M_BETA_HEADER` | 1M context window |

### Prompt caching

```typescript
getCacheControl({ scope, querySource }): { type: 'ephemeral', ttl?: '1h', scope?: CacheScope }
```

- Default TTL: 5 minutes (`ephemeral` without TTL)
- 1-hour TTL available via `getPromptCache1hEligible()` allowlist
- `splitSysPromptPrefix()` places cache points at the system prompt boundary
- `DISABLE_PROMPT_CACHING` env var globally disables; model-specific env vars exist too

### Retry / fallback

`withRetry()` (`services/api/withRetry.ts`) wraps the API call:
- Retries on 529 (`is529Error()`) with exponential backoff
- `FallbackTriggeredError` switches to `fallbackModel` (e.g., Sonnet → Haiku)
- `CannotRetryError` propagates immediately

---

## Phase 7 — Stream Processing

The API returns a server-sent event stream. Claude Code processes it inside the
`for await ... of deps.callModel(...)` loop.

### Stream event types yielded upstream

| Event type | Description |
|-----------|-------------|
| `stream_request_start` | Emitted before the API call |
| `text` | Streaming text delta |
| `thinking` | Extended thinking block delta |
| `tool_use` (partial) | Tool call block starts streaming |
| `tool_use` (complete) | Full tool call received — triggers `addTool()` |
| `assistant` (`Message`) | Complete assistant message after stream closes |
| `user` (`Message`) | Tool result(s) assembled after tool execution |
| `error` (`AssistantMessage` with `apiError`) | API error converted to message |
| `TombstoneMessage` | Placeholder for replaced/truncated messages |
| `ToolUseSummaryMessage` | Summary injected after tool execution |
| `RequestStartEvent` | Metadata event at loop iteration start |

### `max_output_tokens` recovery

```
API returns stop_reason: 'max_tokens'
    ↓
isWithheldMaxOutputTokens() → withhold error from upstream (avoids leaking to SDK)
    ↓
maxOutputTokensRecoveryCount < MAX_OUTPUT_TOKENS_RECOVERY_LIMIT (= 3)
    ↓
continue loop with accumulated assistant messages
```

---

## Phase 8 — Tool Dispatch (`src/services/tools/StreamingToolExecutor.ts`)

`StreamingToolExecutor` executes tools as their blocks stream in, with concurrency control.

### Concurrency model

```
Tool block arrives mid-stream
    ↓
executor.addTool(block, assistantMessage)
    ↓
isConcurrencySafe(tool)?
    ├── yes → start immediately if no exclusive tool running
    └── no  → wait until all concurrent tools finish, then run exclusively
```

### Tool lifecycle states

`'queued' → 'executing' → 'completed' → 'yielded'`

Results are **buffered and emitted in order** (not arrival order) to maintain determinism.

Progress messages are stored separately and yielded immediately (not buffered).

### Sibling abort

When a Bash tool errors, `siblingAbortController.abort()` kills sibling subprocesses.
This does **not** abort the parent `toolUseContext.abortController` — the query turn continues.

### Permission check

Before any tool executes, `canUseTool(tool, input, ...)` is called. This is the
`CanUseToolFn` callback passed into `QueryParams` — its implementation varies by mode:

| Mode | Implementation |
|------|----------------|
| Interactive REPL | `useCanUseTool()` hook — may render a React permission prompt |
| Headless / SDK | `permissionPromptTool` — sends a permission request back to caller |
| Swarm worker | `handleSwarmWorkerPermission()` — delegates to coordinator |
| Coordinator | `handleCoordinatorPermission()` — may propagate up or auto-decide |

---

## Phase 9 — Permission System

Permission checking runs in `hasPermissionsToUseTool()` (`src/utils/permissions/permissions.ts`).

### Decision cascade

```
1. alwaysAllowRules  → 'allow' immediately
2. alwaysDenyRules   → 'deny' immediately
3. alwaysAskRules    → 'ask' (defer to mode)
4. Permission mode:
   ├── bypassPermissions  → always allow
   ├── acceptEdits        → allow file writes, ask for shell
   ├── plan               → read-only, deny all writes
   ├── dontAsk            → allow all (non-interactive equivalent)
   ├── auto               → TRANSCRIPT_CLASSIFIER: ML classifier decides
   └── default            → ask user interactively
5. canUseTool callback with result
```

### Permission modes (`src/types/permissions.ts`)

```typescript
type ExternalPermissionMode = 'acceptEdits' | 'bypassPermissions' | 'default' | 'dontAsk' | 'plan'
type InternalPermissionMode = ExternalPermissionMode | 'auto' | 'bubble'
```

`'auto'` is gated behind `feature('TRANSCRIPT_CLASSIFIER')`.
`'bubble'` is used internally for sub-agent permission delegation.

### Rule sources

`PermissionRuleSource`: `userSettings | projectSettings | localSettings | flagSettings | policySettings | cliArg | command | session`

Rules are matched by `toolName` and optional `ruleContent` (glob pattern for arguments).

---

## Phase 10 — React/Ink Rendering

In interactive mode, every yielded `StreamEvent | Message` flows into the React state.

```
query() generator yielded value
    ↓
REPL event loop consumes via async iteration
    ↓
dispatch to AppState (setAppState)
    ↓
React re-render triggered (Ink)
    ↓
<TranscriptMessage> renders each Message
<StreamingText> renders live text deltas
<ToolResultBlock> renders tool output
<PermissionRequest> renders permission prompts
```

### Component hierarchy (simplified)

```
<App>
  <Header>          — model name, token count, session info
  <Transcript>      — scrollable message history
    <UserMessage>
    <AssistantMessage>
      <ThinkingBlock>
      <StreamingText>
      <ToolUseBlock>
        <ToolInput>
        <ToolResult>
  <PermissionRequest>  — overlaid when canUseTool returns 'ask'
  <PromptInput>     — active text field at bottom
  <StatusBar>       — token warning, cost, model
```

### Ink specifics

- Ink renders to stdout using `react-reconciler` over terminal escape sequences
- `<Static>` is used for completed messages (avoids re-rendering old output)
- Live streaming text uses `<Text>` with state updates on every delta event
- `useCanUseTool` is a React hook that updates `toolUseConfirmQueue` state,
  causing `<PermissionRequest>` to appear synchronously in the TUI

---

## Phase 11 — Stop Hooks and Loop Termination

After the API stream closes and tools finish, stop conditions are evaluated:

```
needsFollowUp = false (no tool_use blocks in response)
    ↓
handleStopHooks()  →  src/query/stopHooks.ts
    ├── executePostSamplingHooks()   — PostToolUse shell hooks
    ├── executeStopFailureHooks()    — Stop hooks that signal retry
    └── stopHookActive?             — retry loop iteration if hook requests it
    ↓
No retry needed
    ↓
return { reason: 'end_turn' | 'max_turns' | 'blocking_limit' | ... }  (Terminal)
```

`Terminal` reasons:

| Reason | Meaning |
|--------|---------|
| `end_turn` | Model said stop with no pending tools |
| `max_turns` | `maxTurns` limit reached |
| `blocking_limit` | Token count at hard blocking threshold |
| `abort` | User or signal aborted |
| `error` | Unrecoverable API error |

---

## Phase 12 — Hooks System

Hooks are shell commands registered in `settings.json` under `hooks:`. They fire at
lifecycle events and can influence the pipeline via exit codes and stdout JSON.

### Hook events

| Event | When | Can block? |
|-------|------|-----------|
| `UserPromptSubmit` | After user presses Enter, before query | Yes (sync) |
| `PreToolUse` | Before each tool call, inside `canUseTool` | Yes — can approve/deny |
| `PostToolUse` | After each tool result is assembled | No (observational) |
| `Stop` | When the model returns `end_turn` | Yes — can trigger retry |
| `Notification` | When Claude sends a notification | No |

### Hook output protocol

Sync hooks return JSON to stdout:

```json
{
  "continue": true,
  "decision": "approve",
  "reason": "...",
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "allow",
    "updatedInput": { ... }
  }
}
```

- `continue: false` + `stopReason` terminates the session
- `decision: "block"` in `PreToolUse` overrides `canUseTool` to deny
- `updatedInput` in `PreToolUse` rewrites tool arguments before execution

---

## Phase 13 — Subagents and Swarm Mode

Claude Code supports multi-agent workflows via `AgentTool` and the swarm/coordinator
pattern.

### Agent spawning

```
Model emits tool_use { name: "Task", input: { prompt: "..." } }
    ↓
AgentTool.call()
    ↓
runForkedAgent() / runSubagent()
    ↓
query(params) called recursively with:
  - agentId set in toolUseContext
  - querySource = 'agent:<id>'
  - canUseTool = handleSwarmWorkerPermission (bubbles to coordinator)
  - separate message history
```

### Coordinator / worker split

The coordinator (root agent) owns the terminal TUI and the permission prompt.
Workers delegate permission decisions upward via the `'bubble'` permission mode.
`handleSwarmWorkerPermission()` sends an IPC message to the coordinator and awaits
the response.

### `AgentSummary` service

`src/services/AgentSummary/agentSummary.ts` — generates summaries of agent sub-turns
for injection back into the parent context as `ToolUseSummaryMessage` items.

---

## Phase 14 — Compaction: Secondary LLM Call

When auto-compact fires, a secondary `query()` call is made with `querySource = 'compact'`.

```
autocompact() in query/deps.ts
    ↓
query({
  messages: fullHistory,
  systemPrompt: compactPrompt,      // src/services/compact/prompt.ts
  querySource: 'compact',
  maxTurns: 1,
  canUseTool: noOpCanUseTool,       // no tools in compact turn
})
    ↓
LLM generates summary text
    ↓
buildPostCompactMessages(result):
  [ CompactBoundaryMessage, summary AssistantMessage, ... original attachments ]
    ↓
yield boundary + summary messages to parent query()
    ↓
parent continues with compressed history
```

The compact turn uses the same model as the main loop unless overridden. It is
excluded from the blocking limit check (`querySource !== 'compact'`) to avoid deadlock.

---

## Phase 15 — Headless / SDK Mode Output

In headless mode, the async generator is consumed directly by the caller.
All `StreamEvent | Message` values are serialized to NDJSON on stdout
or returned through the Agent SDK interface.

```typescript
// SDK caller pattern:
for await (const event of query(params)) {
  switch (event.type) {
    case 'text': // streaming text
    case 'tool_use': // tool call
    case 'assistant': // complete message
    case 'user': // tool result
    case 'error': // API error
  }
}
```

The `cli/structuredIO.ts` / `cli/print.ts` modules format events for
non-interactive sessions (NDJSON output, `--output-format json`, etc.).

---

## Complete Data-Flow Diagram

```
┌─────────────────────────────────────────────────────────────────────┐
│  Terminal / SDK Caller                                              │
│                                                                     │
│  [keystroke] ──► <PromptInput> ──► onSubmit()                       │
│                                         │                           │
│                                         ▼                           │
│                              UserPromptSubmit hook (shell)          │
│                                         │                           │
│                                         ▼                           │
│                              createUserMessage()                    │
│                                         │                           │
└─────────────────────────────────────────┼───────────────────────────┘
                                          │
                    ┌─────────────────────▼──────────────────────┐
                    │  query() / queryLoop()   src/query.ts       │
                    │                                             │
                    │  normalizeMessagesForAPI()                  │
                    │  ──► tool result budget                     │
                    │  ──► snip compact                           │
                    │  ──► microcompact                           │
                    │  ──► context collapse                       │
                    │  ──► auto-compact (secondary LLM call)      │
                    │                                             │
                    │  deps.callModel(...)  ◄───── streaming      │
                    │      │                                      │
                    │      │  for await (message of stream)       │
                    │      │    yield StreamEvent upstream        │
                    │      │    addTool() → StreamingToolExecutor │
                    │      │                                      │
                    │  getRemainingResults()                      │
                    │      │                                      │
                    │      ▼                                      │
                    │  canUseTool(tool, input)  ◄── permission    │
                    │      │                        cascade       │
                    │      ▼                                      │
                    │  tool.call(input)                           │
                    │      │                                      │
                    │      ▼                                      │
                    │  yield user message (tool result)           │
                    │      │                                      │
                    │  needsFollowUp? ──yes──► loop               │
                    │      │ no                                   │
                    │  handleStopHooks()                          │
                    │      │                                      │
                    │  return Terminal                            │
                    └─────────────────────────────────────────────┘
                                          │
                    ┌─────────────────────▼──────────────────────┐
                    │  Anthropic API   (services/api/claude.ts)   │
                    │                                             │
                    │  beta.messages.stream({                     │
                    │    model, system, messages, tools, ...      │
                    │  })                                         │
                    │                                             │
                    │  ──► SSE stream of BetaRawMessageStreamEvent│
                    │  ──► prompt caching (ephemeral, 1h TTL)    │
                    │  ──► withRetry() wrapper (529, fallback)    │
                    └─────────────────────────────────────────────┘
                                          │
                    ┌─────────────────────▼──────────────────────┐
                    │  React/Ink Render  (interactive mode only)  │
                    │                                             │
                    │  AppState updated via setAppState()         │
                    │  React re-renders <Transcript>              │
                    │  <StreamingText> updates live               │
                    │  <PermissionRequest> appears if needed      │
                    └─────────────────────────────────────────────┘
```

---

## Key Types Reference

| Type | File | Description |
|------|------|-------------|
| `QueryParams` | `src/query.ts` | Parameters for one query turn |
| `State` | `src/query.ts` | Mutable per-iteration loop state |
| `Message` | `src/types/message.ts` (inferred) | Internal message union type |
| `StreamEvent` | `src/types/message.ts` | Events yielded during streaming |
| `CanUseToolFn` | `src/hooks/useCanUseTool.tsx` | Permission callback type |
| `ToolUseContext` | `src/Tool.ts` | Tools, model, abort signal, options |
| `AppState` | `src/state/AppState.ts` | Global mutable store |
| `PermissionMode` | `src/types/permissions.ts` | `default | plan | bypassPermissions | …` |
| `TrackedTool` | `src/services/tools/StreamingToolExecutor.ts` | Per-tool execution state |
| `Terminal` | `src/query/transitions.ts` | Loop termination reason |
| `Continue` | `src/query/transitions.ts` | Loop continuation reason |
| `QuerySource` | `src/constants/querySource.ts` | `repl_main_thread | agent:… | compact | …` |

---

## Key Invariants

1. **One generator, one turn**: `query()` covers exactly one user-visible turn. The
   outer REPL loop calls it again for each new user message.

2. **Thinking preservation rule**: A message containing `thinking` or
   `redacted_thinking` blocks must be part of a query with `max_thinking_length > 0`,
   must not be the last block, and must be preserved for the full assistant trajectory
   (until the tool results are sent and the next assistant message arrives).
   Violation causes Anthropic API errors.

3. **`max_output_tokens` recovery limit = 3**: The loop will retry at most 3 times
   when the model hits its output token limit. Error messages are withheld from
   upstream consumers during the retry window.

4. **Tool ordering**: Results are yielded in tool-registration order (not completion
   order), even when concurrent-safe tools run in parallel.

5. **normalizeMessagesForAPI is the sole wire boundary**: No `Message` object reaches
   the Anthropic API directly. Everything passes through normalization.

6. **`querySource` gates compaction exclusions**: Compact/session_memory agents skip
   the blocking limit check and the proactive auto-compact, to avoid deadlock.

7. **Cache control is per-message-boundary**: The system prompt prefix and user system
   prompt each carry `cache_control: { type: 'ephemeral' }`. Tool definitions and
   message history receive cache points at the splits defined in `splitSysPromptPrefix()`.

---

## Comparison: smelt / kimi-cli / claude-code

| Dimension | smelt | kimi-cli | claude-code |
|-----------|-------|---------|-------------|
| **Language** | Rust | Python 3.12+ | TypeScript (Bun runtime) |
| **UI framework** | Custom Rust TUI (smelt_tui) | Rich `Live` display | React/Ink |
| **Async model** | Tokio (multi-threaded) | asyncio (single event loop) | JS event loop (async generators) |
| **LLM abstraction** | Direct reqwest HTTP | `kosong.ChatProvider` protocol | Anthropic SDK `beta.messages.stream()` |
| **Event/message bus** | Rust channels (mpsc/broadcast) | `Wire` SPMC BroadcastQueue | Async generator `yield` chain |
| **Tool dispatch** | Sequential, blocking per tool | `asyncio.create_task()` concurrent | `StreamingToolExecutor` with concurrency/serial partitioning |
| **Permission gate** | Not present (smelt is the editor) | `Approval.request()` → `ApprovalRuntime` | `CanUseToolFn` callback cascade + permission mode + rules |
| **Prompt caching** | Not applicable | Via `kosong` / OpenAI-compat | `cache_control: ephemeral` per-boundary, 1h TTL option |
| **Compaction** | Not applicable | `SimpleCompaction` (secondary LLM) | Auto/reactive/microcompact/context-collapse (feature-flagged) |
| **Subagents** | Not applicable | `LaborMarket` + `SubagentStore` | `AgentTool` + swarm coordinator/worker split |
| **History rewind** | Undo system in buffer | D-Mail / `BackToTheFuture` exception | `TombstoneMessage` / compaction boundary |
| **Hook system** | Not present | Shell hooks on 8 events | Shell hooks on 5 events (`UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Stop`, `Notification`) |
| **Headless mode** | N/A (pure editor) | `run_print()` / `run_acp()` / `run_wire_stdio()` | `--headless` / Agent SDK |
| **Session persistence** | Buffer/undo ring on disk | `wire.jsonl` Wire message log | Session storage + content replacement records |
| **Feature flags** | Compile-time features | Python import guards | `feature('...')` via GrowthBook + `bun:bundle` |
