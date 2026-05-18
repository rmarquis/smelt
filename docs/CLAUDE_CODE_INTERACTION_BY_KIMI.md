# Claude Code Architectural Analysis: Keystroke to LLM Request to Rendered Output

> **Scope:** This document exhaustively maps every code path that transforms a user keystroke into an LLM API request and back to rendered output in the Claude Code codebase. It covers `claude-code/src/`.
>
> **Generated:** 2026-05-18 by Kimi Code CLI via multi-agent codebase exploration.
> **Source:** Extracted TypeScript source from `@anthropic-ai/claude-code` v2.1.88 npm package.

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Entry Point and Bootstrap Sequence](#2-entry-point-and-bootstrap-sequence)
3. [CLI Layer and UI Mode Dispatch](#3-cli-layer-and-ui-mode-dispatch)
4. [Terminal UI: Input Capture and the REPL](#4-terminal-ui-input-capture-and-the-repl)
5. [The Query Engine: Agent Core and Turn Lifecycle](#5-the-query-engine-agent-core-and-turn-lifecycle)
6. [API Layer: HTTP Request Building and Streaming](#6-api-layer-http-request-building-and-streaming)
7. [Response Streaming: From SSE to Rendered Output](#7-response-streaming-from-sse-to-rendered-output)
8. [Tool Call Lifecycle](#8-tool-call-lifecycle)
9. [Permission System](#9-permission-system)
10. [State Management](#10-state-management)
11. [Context and Conversation History](#11-context-and-conversation-history)
12. [Alternative UI Modes and Entrypoints](#12-alternative-ui-modes-and-entrypoints)
13. [Complete Data Flow Summary](#13-complete-data-flow-summary)

---

## 1. Executive Summary

Claude Code is a **TypeScript/React terminal application** built on a heavily customized fork of **Ink** (React for terminals). It has a layered architecture:

| Layer | Module | Responsibility |
|-------|--------|---------------|
| **CLI** | `src/entrypoints/cli.tsx`, `src/main.tsx` | Argument parsing, feature flags, mode dispatch |
| **UI** | `src/screens/REPL.tsx`, `src/components/` | Interactive terminal UI (Ink/React components) |
| **Query** | `src/query.ts`, `src/QueryEngine.ts` | Agent orchestration, turn lifecycle, message history |
| **API** | `src/services/api/` | Anthropic SDK integration, streaming, retries, provider abstraction |
| **Tools** | `src/tools/`, `src/services/tools/` | Built-in tools, MCP tools, tool execution, hooks |
| **State** | `src/state/` | Zustand-like store, AppState, permission context |
| **Ink** | `src/ink/` | Custom React reconciler, terminal DOM, screen buffer, ANSI output |

The high-level flow is:

```
User keystroke
    → Ink raw-mode stdin parser
    → useInput event emitter
    → useTextInput cursor logic
    → PromptInput onSubmit
    → REPL.onSubmit()
    → REPL.onQuery() / QueryEngine.submitMessage()
    → query() loop → queryModelWithStreaming()
    → anthropic.beta.messages.create({ stream: true })
    → SSE stream consumption
    → content_block_start / delta / stop events
    → AssistantMessage yielded
    → REPL.setMessages() → React re-render
    → Ink reconciler → Yoga layout → screen buffer
    → log-update diff → ANSI escape sequences
```

Key architectural decisions:
- **Custom Ink fork** for terminal rendering (not the npm package) with Yoga flexbox layout
- **React reconciler** renders to a terminal DOM, not HTML DOM
- **QueryEngine** owns mutable message history across turns
- **`query()` is an async generator** that yields stream events and assistant messages
- **Tool execution** is partitioned into concurrent (read-only/safe) and serial (destructive) batches
- **Permission system** supports multiple modes (default, acceptEdits, bypassPermissions, plan, auto, bubble) with rule-based matching and YOLO classifier

---

## 2. Entry Point and Bootstrap Sequence

### 2.1 Binary Entry Point

**File:** `claude-code/src/entrypoints/cli.tsx`  
**Function:** `main()` (line 33)

Fast-path dispatch based on argv:
- `--version` / `-v` → print version and exit (zero imports)
- `--dump-system-prompt` → render system prompt and exit
- `--claude-in-chrome-mcp` / `--chrome-native-host` / `--computer-use-mcp` → MCP server modes
- `--daemon-worker` → daemon worker mode
- `remote-control` / `rc` / `remote` / `sync` / `bridge` → bridge/remote control mode
- `daemon` → long-running supervisor
- `ps` / `logs` / `attach` / `kill` / `--bg` → background session management
- `new` / `list` / `reply` → template jobs
- `environment-runner` → headless BYOC runner
- `self-hosted-runner` → self-hosted runner

### 2.2 Main Entry Point

**File:** `claude-code/src/main.tsx`  
**Function:** `main()` (line 585)

Heavy initialization sequence:
1. **Security:** `NoDefaultCurrentDirectoryInExePath = '1'` (Windows PATH hijacking prevention)
2. **Warning handler** and **SIGINT** handler setup
3. **Deep link / URL scheme** handling (`cc://`, `cc+unix://`)
4. **Assistant mode** (`claude assistant [sessionId]`)
5. **SSH remote** (`claude ssh <host>`)
6. **Non-interactive detection** (`-p`, `--print`, `--init-only`, `--sdk-url`)
7. **Settings loading** (`--settings`, `--setting-sources`)
8. **Entrypoint tagging** (`cli`, `sdk-cli`, `sdk-ts`, `sdk-py`, `remote`, `github-action`, etc.)
9. **Run Commander CLI** → `preAction` hook:
   - Await MDM settings + keychain prefetch
   - `init()` — full initialization
   - `initSinks()` — telemetry sinks
   - Run migrations
   - Load remote managed settings + policy limits

### 2.3 Initialization (`init()`)

**File:** `claude-code/src/entrypoints/init.ts`

1. Load global config
2. Initialize auth (OAuth, API keys)
3. Initialize telemetry
4. Initialize GrowthBook feature flags
5. Initialize settings
6. Initialize plugins
7. Initialize skills
8. Initialize MCP clients

---

## 3. CLI Layer and UI Mode Dispatch

### 3.1 Commander Setup

**File:** `claude-code/src/main.tsx` (line 884)

Uses `@commander-js/extra-typings` with sorted help. The default action (no subcommand) launches the interactive REPL.

Key options:
- `-p, --print` — headless mode
- `--bare` — minimal mode (skip hooks, LSP, plugins, auto-memory)
- `--output-format` — `text`, `json`, `stream-json`
- `--permission-mode` — `default`, `acceptEdits`, `bypassPermissions`, `dontAsk`, `plan`, `auto`, `bubble`
- `--model`, `--effort`, `--agent`, `--betas`
- `--continue`, `--resume`, `--fork-session`
- `--allowed-tools`, `--disallowed-tools`
- `--mcp-config`

### 3.2 UI Mode Dispatch

**File:** `claude-code/src/main.tsx` (within `run()`)

After `preAction` init, the default command handler:
1. Checks for `--print` → `runHeadless()`
2. Checks for `--init-only` → run setup hooks and exit
3. Otherwise → interactive path:
   - Show setup screens (trust dialog, login, onboarding)
   - Launch REPL via `launchRepl()`

### 3.3 REPL Launch

**File:** `claude-code/src/replLauncher.tsx`

```tsx
export async function launchRepl(root, appProps, replProps, renderAndRun) {
  await renderAndRun(root,
    <App {...appProps}>
      <REPL {...replProps} />
    </App>
  );
}
```

**File:** `claude-code/src/interactiveHelpers.tsx`

`renderAndRun()` creates an Ink instance and renders the React tree.

---

## 4. Terminal UI: Input Capture and the REPL

### 4.1 Ink: Custom React for Terminal

**Files:**
- `claude-code/src/ink/ink.tsx` — `Ink` class (1723 lines)
- `claude-code/src/ink/reconciler.ts` — Custom React reconciler
- `claude-code/src/ink/renderer.ts` — DOM → screen buffer
- `claude-code/src/ink/screen.ts` — Screen buffer (cell grid)
- `claude-code/src/ink/log-update.ts` — Diff engine for terminal output

**How it works:**
1. **React reconciler** renders React components to a terminal DOM (Yoga-based flexbox)
2. **Terminal DOM** — `dom.ts` defines `DOMElement` nodes with Yoga layout
3. **Screen buffer** — `screen.ts` maintains a grid of cells (char + style)
4. **Diff + output** — `log-update.ts` compares previous screen to new and emits minimal ANSI escapes
5. **Raw mode + input** — `ink.tsx` sets stdin to raw mode, parses keypresses, dispatches through `EventEmitter`
6. **Alt screen** — `<AlternateScreen>` enters alternate buffer (`\x1b[?1049h`)

### 4.2 Input Capture Chain

**File:** `claude-code/src/ink/hooks/use-input.ts`

```tsx
const useInput = (inputHandler, options) => {
  const { setRawMode, internal_eventEmitter } = useStdin()

  useLayoutEffect(() => {
    if (options.isActive === false) return
    setRawMode(true)
    return () => setRawMode(false)
  }, [options.isActive, setRawMode])

  useEffect(() => {
    internal_eventEmitter?.on('input', handleData)
    return () => internal_eventEmitter?.removeListener('input', handleData)
  }, [internal_eventEmitter, handleData])
}
```

### 4.3 Text Input Logic

**File:** `claude-code/src/hooks/useTextInput.ts` (529 lines)

Core input logic:
- Maps keystrokes to cursor operations via a `Cursor` class
- Readline bindings: `Ctrl+A/E/K/U/W/Y`, `Alt+B/F/D`, arrows, Home/End
- Submit (`Enter`), exit (`Ctrl+C`, `Ctrl+D`), history (`↑/↓`)
- Returns `onInput`, `renderedValue`, `cursorLine`, `cursorColumn`

### 4.4 PromptInput Component

**File:** `claude-code/src/components/PromptInput/PromptInput.tsx` (2339 lines)

High-level input bar managing:
- Slash command highlighting
- History navigation (`useArrowKeyHistory`)
- Suggestions (`useTypeahead`)
- Footer pills (tasks, teams, bridge, companion)
- Vim mode, bash mode, image paste
- Calls `onSubmit(input, helpers)` on Enter

### 4.5 REPL Component

**File:** `claude-code/src/screens/REPL.tsx` (5006 lines)

The REPL is the **entire interactive session**. It manages:
- Full conversation state (`messages`, `setMessages`)
- Query lifecycle (loading, streaming, aborting)
- Input state (`inputValue`, `inputMode`)
- Tool permission queues, prompts, dialogs
- Remote sessions, SSH sessions, backgrounding
- Transcript mode (`Ctrl+O`) vs. prompt mode

**Key functions:**
- `REPL({ commands, debug, initialTools, ... })` — main export (line 572)
- `onSubmit(input, helpers, ...)` — handles user submission (line 3142)
- `onQuery(newMessages, abortController, ...)` — guards and dispatches query (line 2855)
- `onQueryImpl(...)` — calls `query()` and streams results (line 2661)
- `onQueryEvent(event)` — processes streaming events (line 2584)

---

## 5. The Query Engine: Agent Core and Turn Lifecycle

### 5.1 QueryEngine

**File:** `claude-code/src/QueryEngine.ts`

Top-level orchestrator for a single conversation session. Owns mutable message history, usage tracking, file state cache, and abort controller across turns.

**`submitMessage(prompt, options)`** flow:
1. `processUserInput()` — handles slash commands, attachments
2. `fetchSystemPromptParts()` — builds system prompt
3. Yields `buildSystemInitMessage()` — signals turn start
4. Calls `query()` in a `for await...of` loop
5. Handles stream events, persists messages, tracks usage/cost
6. Yields normalized SDK messages

### 5.2 query() — The Core Agent Loop

**File:** `claude-code/src/query.ts`  
**Function:** `query()` (line 219) / `queryLoop()` (line 241)

The agentic turn loop. Repeatedly calls the model, executes tools, continues until the model stops or a limit is reached.

**Per iteration:**
1. **Pre-processing:**
   - `getMessagesAfterCompactBoundary(messages)` — strips pre-compact history
   - `applyToolResultBudget()` — enforces size limits on tool results
   - `snipCompactIfNeeded()` — truncates old history
   - `microcompactMessages()` — runs micro-compaction
   - `autoCompactIfNeeded()` — auto-compaction if token count high
   - Rebuilds `messagesForQuery`

2. **API Call:**
   - `deps.callModel()` → `queryModelWithStreaming()`
   - Receives async generator of `StreamEvent | AssistantMessage | SystemAPIErrorMessage`

3. **Streaming consumption:**
   - Accumulates `assistantMessages` and `toolUseBlocks`
   - If `toolUseBlocks` present → `needsFollowUp = true`

4. **Tool Execution:**
   - `runTools()` from `services/tools/toolOrchestration.ts`
   - Yields tool result messages as `user` messages
   - Loops back to step 1

5. **Stop hooks / budget checks:**
   - `handleStopHooks()` — checks if turn should continue
   - `checkTokenBudget()` — +500k auto-continue feature

### 5.3 Message Types

**File:** `claude-code/src/types/message.ts`

| Type | Description |
|------|-------------|
| `UserMessage` | User input + attachments + tool results |
| `AssistantMessage` | LLM response with content blocks + usage |
| `StreamEvent` | Raw SSE event from API |
| `SystemAPIErrorMessage` | Synthetic error on API failure |
| `AttachmentMessage` | File/image attachments |
| `ProgressMessage` | Tool execution progress |
| `TombstoneMessage` | Compacted/deleted message placeholder |
| `CompactBoundaryMessage` | Context compaction marker |

---

## 6. API Layer: HTTP Request Building and Streaming

### 6.1 queryModelWithStreaming

**File:** `claude-code/src/services/api/claude.ts`

```ts
async function* queryModelWithStreaming({ messages, systemPrompt, thinkingConfig, tools, signal, options }) {
  const params = paramsFromContext(retryContext)
  const result = await anthropic.beta.messages.create(
    { ...params, stream: true },
    { signal, headers: { [CLIENT_REQUEST_ID_HEADER]: randomUUID() } }
  ).withResponse()
  // ... consume stream
}
```

### 6.2 Request Building

**File:** `claude-code/src/services/api/claude.ts` — `paramsFromContext()`

```ts
return {
  model: normalizeModelStringForAPI(options.model),
  messages: addCacheBreakpoints(messagesForAPI, enablePromptCaching, ...),
  system: buildSystemPromptBlocks(systemPrompt, enablePromptCaching, ...),
  tools: allTools,
  tool_choice: options.toolChoice,
  betas: betasParams,
  metadata: getAPIMetadata(),
  max_tokens: maxOutputTokens,
  thinking,
  temperature,
  context_management,
  output_config: { effort, task_budget, format },
  speed, // fast mode
  ...extraBodyParams,
}
```

**Key transformations:**
- **Message normalization:** `normalizeMessagesForAPI()` — internal `Message[]` → Anthropic SDK `MessageParam[]`
- **System prompt blocks:** `buildSystemPromptBlocks()` — splits into `TextBlockParam[]` with optional `cache_control`
- **Cache breakpoints:** `addCacheBreakpoints()` — injects `cache_control: { type: 'ephemeral' }` on last message
- **Beta headers:** dynamically merged based on model, provider, feature gates
- **Tool schemas:** `toolToAPISchema()` — converts internal `Tool` to Anthropic `BetaToolUnion`

### 6.3 API Client

**File:** `claude-code/src/services/api/client.ts`  
**Function:** `getAnthropicClient()`

Provider support:

| Provider | SDK Class | Env Vars |
|----------|-----------|----------|
| **First-party** | `Anthropic` | `ANTHROPIC_API_KEY`, OAuth tokens |
| **AWS Bedrock** | `AnthropicBedrock` | `AWS_REGION`, `AWS_BEARER_TOKEN_BEDROCK` |
| **Azure Foundry** | `AnthropicFoundry` | `ANTHROPIC_FOUNDRY_RESOURCE`, `ANTHROPIC_FOUNDRY_API_KEY` |
| **Vertex AI** | `AnthropicVertex` | `ANTHROPIC_VERTEX_PROJECT_ID`, `CLOUD_ML_REGION` |

Headers:
- `x-app: cli`
- `User-Agent`
- `X-Claude-Code-Session-Id`
- `Authorization: Bearer <token>`
- `x-client-request-id`

### 6.4 Retry Logic

**File:** `claude-code/src/services/api/withRetry.ts`  
**Function:** `withRetry()`

Up to `DEFAULT_MAX_RETRIES = 10` attempts:
- **529 / overloaded:** Exponential backoff; after 3 consecutive 529s → `FallbackTriggeredError` (switch to `fallbackModel`)
- **Fast mode:** On 429/529, short delays or cooldown (switch to standard speed)
- **Auth errors (401/403):** Refresh OAuth/AWS/GCP credentials, recreate client
- **Persistent retry mode:** For unattended sessions, retries indefinitely with chunked keep-alive yields
- **Context overflow:** Parse 400 errors, reduce `max_tokens` for next attempt

---

## 7. Response Streaming: From SSE to Rendered Output

### 7.1 Stream Consumption

**File:** `claude-code/src/services/api/claude.ts` (lines ~1930–2400)

```ts
for await (const part of stream) {
  switch (part.type) {
    case 'message_start':
      partialMessage = part.message
      ttftMs = Date.now() - startTime
      break
    case 'content_block_start':
      contentBlocks[index] = initializeBlock(part)
      break
    case 'content_block_delta':
      appendDelta(contentBlocks[index], part.delta)
      break
    case 'content_block_stop':
      newMessages.push(buildAssistantMessage(contentBlocks[index], partialMessage))
      yield newMessages[newMessages.length - 1]
      break
    case 'message_delta':
      updateUsage(lastMessage, part.usage)
      break
    case 'message_stop':
      // end of stream
      break
  }
}
```

**Stream watchdog:** `setTimeout` aborts if no chunks for 90s (`CLAUDE_ENABLE_STREAM_WATCHDOG`).

### 7.2 REPL Event Processing

**File:** `claude-code/src/screens/REPL.tsx`  
**Function:** `onQueryEvent(event)` (line 2584)

Processes events from `handleMessageFromStream`:
- `message_start` → sets loading state
- `content_block_start` / `delta` → updates streaming text
- `content_block_stop` → appends completed block to messages
- `tool_use` blocks → queues for execution
- `message_stop` → finishes turn or triggers tool execution

### 7.3 Message Rendering

**File:** `claude-code/src/components/Messages.tsx` (834 lines)

Message list container:
- Normalizes, filters, groups, collapses messages
- Applies "brief mode" filter
- Computes virtual scroll or render-cap slicing

**File:** `claude-code/src/components/Message.tsx` (627 lines)

Per-message renderer:
- `attachment` → `AttachmentMessage`
- `assistant` → `AssistantTextMessage`, `AssistantToolUseMessage`, `AssistantThinkingMessage`
- `user` → `UserTextMessage`, `UserImageMessage`, `UserToolResultMessage`
- `system` → `SystemTextMessage`, `CompactBoundaryMessage`

**File:** `claude-code/src/components/VirtualMessageList.tsx` (1082 lines)

Fullscreen mode (alt screen):
- `useVirtualScroll()` — only mounts visible messages
- Measures item heights via Yoga layout
- Handles sticky prompt headers
- Transcript search (`/`, `n`, `N`)

### 7.4 Ink Rendering Pipeline

```
React state update (setMessages)
    → React re-render (REPL + Messages + PromptInput)
    → Ink reconciler (diff virtual DOM)
    → Yoga layout (compute flexbox positions)
    → Renderer (walk tree, write to screen buffer)
    → log-update (diff prev vs current buffer)
    → ANSI escape sequences to stdout
```

---

## 8. Tool Call Lifecycle

### 8.1 Tool Definition

**File:** `claude-code/src/Tool.ts` (lines 362–695)

```ts
interface Tool {
  name: string
  aliases?: string[]
  inputSchema: ZodSchema
  outputSchema?: ZodSchema
  call(args, context, canUseTool, parentMessage, onProgress): Promise<...>
  description(): Promise<string>
  prompt(): Promise<string>
  isEnabled(): boolean
  isConcurrencySafe(): boolean
  isReadOnly(): boolean
  isDestructive(): boolean
  validateInput?(input, context): Promise<void>
  checkPermissions?(input, context): Promise<...>
  renderToolUseMessage?(...): ReactNode
  renderToolResultMessage?(...): ReactNode
  renderToolUseProgressMessage?(...): ReactNode
}
```

**Builder:** `buildTool(def: ToolDef)` (line 783) spreads `TOOL_DEFAULTS` for safe defaults.

### 8.2 Tool Registry

**File:** `claude-code/src/tools.ts`

- `getAllBaseTools()` (line 193) — exhaustive built-in list gated by feature flags
- `getTools(permissionContext)` (line 271) — filters by deny-rules, REPL mode, `isEnabled()`
- `assembleToolPool(permissionContext, mcpTools)` (line 345) — combines built-ins + MCP tools

**Built-in tools** (under `src/tools/<ToolName>/`):
- `AgentTool` — subagent spawning
- `BashTool` — shell command execution
- `FileReadTool` — file reading
- `FileEditTool` — file editing
- `GlobTool` — file pattern matching
- `GrepTool` — regex search
- `WebSearchTool` — web search
- `WebFetchTool` — URL fetching
- `TodoWriteTool` — todo list management
- `AskUserQuestionTool` — structured user questions
- `SleepTool` — explicit pause
- `ThinkingTool` — explicit thinking step
- And more...

### 8.3 Tool Execution Flow

**File:** `claude-code/src/services/tools/toolOrchestration.ts`  
**Function:** `runTools()` (line 19)

Partitions tool calls:
- **Concurrent:** Read-only / `isConcurrencySafe===true` tools (up to `CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY`, default 10)
- **Serial:** Non-safe tools run one-at-a-time

**File:** `claude-code/src/services/tools/toolExecution.ts`  
**Function:** `runToolUse()` (line 337)

1. **Lookup:** `findToolByName()` — falls back to alias matching
2. **Abort check:** if signal aborted → yield cancellation
3. **Validation:** `inputSchema.safeParse(input)` → `InputValidationError` if fails
4. **Custom validation:** `tool.validateInput?.()`
5. **Pre-tool hooks:** `runPreToolUseHooks()` — progress, permissions, updated inputs, stop reasons
6. **Permission resolution:** `resolveHookPermissionDecision()` → `canUseTool()`
7. **Execution:** `tool.call()` with parsed input and context
8. **Post-tool hooks:** `runPostToolUseHooks()` — modify output, inject attachments, block continuation
9. **Result mapping:** `tool.mapToolResultToToolResultBlockParam()` → Anthropic SDK `ToolResultBlockParam`
10. **Yield** resulting `UserMessage` containing `tool_result`

### 8.4 Tool Result Format

Tool results sent back to the LLM as:
```ts
{
  role: 'user',
  content: [{
    type: 'tool_result',
    tool_use_id: toolUse.id,
    content: resultText,
    is_error: false,
  }]
}
```

Constructed in `utils/messages.ts` (`createUserMessage`) and normalized via `normalizeMessagesForAPI()`.

---

## 9. Permission System

### 9.1 Permission Types

**File:** `claude-code/src/types/permissions.ts` (441 lines)

```ts
type PermissionMode = 'default' | 'acceptEdits' | 'bypassPermissions' | 'dontAsk' | 'plan' | 'auto' | 'bubble'
type PermissionBehavior = 'allow' | 'deny' | 'ask'
```

### 9.2 Permission Engine

**File:** `claude-code/src/utils/permissions/permissions.ts` (1486 lines)  
**Function:** `hasPermissionsToUseTool()` (line 473)

1. **Rule-based checks:** `hasPermissionsToUseToolInner()`
   - Mode checks, safety checks, sandbox checks
2. **Allow:** reset denial tracking, return allow
3. **Ask:**
   - `dontAsk` mode → auto-deny
   - `auto` mode → run **YOLO classifier** (`classifyYoloAction()`)
     - `acceptEdits` fast-path for safe tools
     - If classifier blocks → update denial tracking; if exceeds limit → fall back to ask
   - `shouldAvoidPermissionPrompts` (headless) → run `PermissionRequest` hooks, then auto-deny
4. Return final `PermissionDecision`

### 9.3 Rule Matching

- `getAllowRules()`, `getDenyRules()`, `getAskRules()` — flatten `ToolPermissionRulesBySource`
- `toolMatchesRule()` — exact name, MCP server-level (`mcp__server__*`), wildcard
- `getRuleByContentsForTool()` — content-aware matching (e.g., `prefix:*`)

### 9.4 Interactive Permission UI

**File:** `claude-code/src/hooks/useCanUseTool.tsx` (204 lines)

When `hasPermissionsToUseTool` returns `'ask'`:
1. **Coordinator path:** `handleCoordinatorPermission()`
2. **Swarm worker path:** `handleSwarmWorkerPermission()`
3. **Speculative classifier:** Bash classifier check (2s timeout)
4. **Interactive path:** `handleInteractivePermission()` — pushes `ToolUseConfirm` to permission queue

**File:** `claude-code/src/hooks/toolPermission/handlers/interactiveHandler.ts` (536 lines)

Races between:
- Local user dialog (allow / deny / abort)
- Bridge (claude.ai remote control) response
- Channel (Telegram / iMessage) response
- PermissionRequest hooks
- Bash classifier auto-approval

Uses `createResolveOnce()` for atomic claim-and-resolve.

---

## 10. State Management

### 10.1 Store Primitive

**File:** `claude-code/src/state/store.ts`

Simple `createStore<T>(initialState, onChange?)`:
```ts
{
  getState,
  setState,  // updater function: (prev) => next
  subscribe
}
```

### 10.2 AppState

**File:** `claude-code/src/state/AppStateStore.ts` (569 lines)

Large immutable state containing:
- `settings: SettingsJson`
- `tasks: { [taskId: string]: TaskState }`
- `toolPermissionContext: ToolPermissionContext`
- `mcp: { clients, tools, commands, resources }`
- `plugins: { enabled, disabled, commands, errors }`
- `agentDefinitions`, `fileHistory`, `attribution`, `todos`
- `notifications`, `elicitation`, `inbox`
- `speculation: SpeculationState`
- `fastMode`, `effortValue`, `authVersion`
- Bridge / remote fields
- Team / swarm fields

### 10.3 React Integration

**File:** `claude-code/src/state/AppState.tsx` (200 lines)

- `AppStateProvider` — creates store
- `useAppState(selector)` — `useSyncExternalStore` hook
- `useSetAppState()` — returns `store.setState`
- `useAppStateStore()` — returns store object

---

## 11. Context and Conversation History

### 11.1 System Context

**File:** `claude-code/src/context.ts` (189 lines)

- `getSystemContext()` — memoized, returns `{ gitStatus?, cacheBreaker? }`
  - Runs `git status --short`, `git log --oneline -n 5`, branch info
  - Skipped in remote mode or when git instructions disabled
- `getUserContext()` — memoized, returns `{ claudeMd?, currentDate }`
  - Discovers and reads `CLAUDE.md` / `claude.md` files

### 11.2 Tool-Use Context

**File:** `claude-code/src/Tool.ts` (lines 158–300)

`ToolUseContext` passed to every `tool.call()`:
- `options`: commands, tools, model, MCP clients, agent definitions
- `messages: Message[]` — full conversation history
- `getAppState()`, `setAppState()`
- `abortController`, `readFileState`
- `requestPrompt()` — interactive prompt callback
- `agentId`, `agentType` — for subagents
- `queryTracking?: { chainId, depth }`
- `renderedSystemPrompt` — parent's frozen system prompt bytes

### 11.3 Prompt History

**File:** `claude-code/src/history.ts` (464 lines)

User prompt history (up-arrow / Ctrl+R):
- Stores in `~/.claude-code/history.jsonl` with lockfile
- `LogEntry`: `display`, `pastedContents`, `timestamp`, `project`, `sessionId`
- `getHistory()` — current-session first, then others, newest-first, deduped

### 11.4 Context Compaction

**Files:**
- `claude-code/src/services/compact/compact.ts`
- `claude-code/src/services/compact/autoCompact.ts`
- `claude-code/src/services/compact/microCompact.ts`

- **Auto-compaction:** triggered when token count exceeds threshold
- **Micro-compaction:** trims old messages aggressively
- **Snip replay:** history truncation with injected "snip" markers
- **Compact boundary:** marks where compaction occurred; messages before boundary stripped from API calls

---

## 12. Alternative UI Modes and Entrypoints

### 12.1 Print Mode (Headless)

**File:** `claude-code/src/cli/print.ts`

`-p/--print` mode: read prompt → run query → print output → exit.

### 12.2 Stream JSON Mode

**File:** `claude-code/src/cli/structuredIO.ts`

`--output-format=stream-json`: NDJSON stream of events for programmatic consumption.

### 12.3 MCP Server Mode

**Files:**
- `claude-code/src/entrypoints/mcp.ts`
- `claude-code/src/utils/claudeInChrome/mcpServer.ts`
- `claude-code/src/utils/computerUse/mcpServer.ts`

`--claude-in-chrome-mcp`, `--computer-use-mcp`: run as MCP server.

### 12.4 Bridge / Remote Control

**Files:**
- `claude-code/src/bridge/bridgeMain.ts`
- `claude-code/src/bridge/replBridge.ts`

`claude remote-control`: serve local machine as bridge environment for claude.ai remote control.

### 12.5 Daemon Mode

**Files:**
- `claude-code/src/daemon/main.ts`
- `claude-code/src/daemon/workerRegistry.ts`

`claude daemon`: long-running supervisor with worker processes.

### 12.6 Background Sessions

**File:** `claude-code/src/cli/bg.ts`

`claude ps`, `claude logs`, `claude attach`, `claude kill`: manage background sessions.

### 12.7 SDK Entrypoints

**Files:**
- `claude-code/src/entrypoints/sdk/coreTypes.ts`
- `claude-code/src/entrypoints/sdk/coreSchemas.ts`

TypeScript SDK types and schemas for external integration.

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
│  INK RAW-MODE STDIN PARSER                                                  │
│  claude-code/src/ink/ink.tsx :: Ink class                                   │
│  - Sets stdin to raw mode                                                   │
│  - Parses keypress sequences (parse-keypress.ts)                            │
│  - Dispatches through EventEmitter                                          │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  USEINPUT HOOK                                                              │
│  claude-code/src/ink/hooks/use-input.ts                                     │
│  - Registers handler on internal_eventEmitter                               │
│  - Calls setRawMode(true) when active                                       │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  TEXT INPUT LOGIC                                                           │
│  claude-code/src/hooks/useTextInput.ts                                      │
│  - Maps keys to Cursor operations                                           │
│  - Readline bindings (Ctrl+A/E/K/U/W/Y, Alt+B/F/D)                          │
│  - Calls onChange(text), tracks cursor position                             │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  PROMPT INPUT COMPONENT                                                     │
│  claude-code/src/components/PromptInput/PromptInput.tsx                     │
│  - Slash command highlighting                                               │
│  - History navigation, typeahead suggestions                                │
│  - Vim mode, bash mode, image paste                                         │
│  - Calls onSubmit(input, helpers) on Enter                                  │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  REPL ON SUBMIT                                                             │
│  claude-code/src/screens/REPL.tsx :: onSubmit()                             │
│  - Adds to history                                                          │
│  - Calls handlePromptSubmit() or onQuery()                                  │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  QUERY ENGINE                                                               │
│  claude-code/src/QueryEngine.ts :: submitMessage()                          │
│  - processUserInput() (slash commands, attachments)                         │
│  - fetchSystemPromptParts()                                                 │
│  - for await event of query()                                               │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  QUERY LOOP                                                                 │
│  claude-code/src/query.ts :: query() / queryLoop()                          │
│  - Pre-process: compact, snip, apply budgets                                │
│  - deps.callModel() → queryModelWithStreaming()                             │
│  - Consume stream → accumulate assistantMessages + toolUseBlocks            │
│  - If toolUseBlocks → runTools() → yield tool results                       │
│  - Loop until stop_reason !== 'tool_use' or max turns/budget                │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  API REQUEST BUILDING                                                       │
│  claude-code/src/services/api/claude.ts :: paramsFromContext()              │
│  - normalizeMessagesForAPI()                                                │
│  - buildSystemPromptBlocks()                                                │
│  - addCacheBreakpoints()                                                    │
│  - toolToAPISchema()                                                        │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  ANTHROPIC SDK CALL                                                         │
│  claude-code/src/services/api/claude.ts                                     │
│  - getAnthropicClient() → Anthropic / Bedrock / Vertex / Foundry            │
│  - anthropic.beta.messages.create({ ...params, stream: true })              │
│  - withRetry() wraps with up to 10 retries                                  │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼ HTTP SSE
┌─────────────────────────────────────────────────────────────────────────────┐
│  STREAM CONSUMPTION                                                         │
│  claude-code/src/services/api/claude.ts                                     │
│  - message_start → init partialMessage, capture ttftMs                      │
│  - content_block_start → init accumulator                                   │
│  - content_block_delta → append delta (text, thinking, input_json)          │
│  - content_block_stop → build AssistantMessage, yield                       │
│  - message_delta → update usage, stop_reason, cost                          │
│  - message_stop → end of stream                                             │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼ AssistantMessage
┌─────────────────────────────────────────────────────────────────────────────┐
│  REPL EVENT PROCESSING                                                      │
│  claude-code/src/screens/REPL.tsx :: onQueryEvent()                         │
│  - setMessages() → appends to conversation state                            │
│  - setStreamingText() → updates live display                                │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  REACT RE-RENDER                                                            │
│  - REPL renders Messages + PromptInput                                      │
│  - Messages.tsx normalizes, groups, filters                                 │
│  - Message.tsx renders per-message content blocks                           │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  INK RECONCILER + RENDERER                                                  │
│  claude-code/src/ink/reconciler.ts :: custom React reconciler               │
│  claude-code/src/ink/renderer.ts :: DOM → screen buffer                     │
│  claude-code/src/ink/screen.ts :: cell grid                                 │
│  - Yoga flexbox layout                                                      │
│  - Walk tree, write text into cell grid                                     │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  DIFF + ANSI OUTPUT                                                         │
│  claude-code/src/ink/log-update.ts                                          │
│  - Compare previous screen buffer to current                                │
│  - Emit minimal ANSI escapes (cursor move, write, clear)                    │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼ ANSI escapes
┌─────────────────────────────────────────────────────────────────────────────┐
│  TERMINAL DISPLAY                                                           │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 13.2 Tool Call Data Flow

```
LLM response includes tool_use blocks
    │
    ▼ content_block_stop
query() detects toolUseBlocks → needsFollowUp = true
    │
    ▼
runTools() (toolOrchestration.ts)
    │
    ├──► partitionToolCalls()
    │      ├──► Concurrent batch (read-only / isConcurrencySafe)
    │      └──► Serial batch (destructive / non-safe)
    │
    ▼
runToolUse() (toolExecution.ts)
    │
    ├──► findToolByName()
    ├──► inputSchema.safeParse(input)
    ├──► validateInput()
    ├──► runPreToolUseHooks()
    ├──► canUseTool() → permission decision
    ├──► tool.call() → executes
    ├──► runPostToolUseHooks()
    ├──► mapToolResultToToolResultBlockParam()
    └──► yield UserMessage with tool_result
    │
    ▼
Added to messages → loop back to query()
```

### 13.3 Permission Request Data Flow

```
tool.call() → canUseTool() → returns 'ask'
    │
    ▼
useCanUseTool() → handleInteractivePermission()
    │
    ▼
Permission queue (ToolUseConfirm)
    │
    ├──► Local dialog (allow / deny / abort)
    ├──► Bridge (claude.ai remote control)
    ├──► Channel (Telegram / iMessage)
    ├──► PermissionRequest hooks
    └──► Bash classifier auto-approval
    │
    ▼ Race resolved
hasPermissionsToUseTool returns allow/deny
    │
    ▼
Tool execution continues or is blocked
```

### 13.4 Key Architectural Decisions

1. **Custom Ink fork:** Claude Code uses a heavily customized React reconciler for terminals, not the public Ink npm package. This gives fine-grained control over layout (Yoga), screen buffer, diffing, cursor management, and alt-screen support.

2. **Async generator query loop:** The core agent loop (`query()`) is an async generator that yields events. This unifies streaming, tool execution, and error handling in a single iterable sequence consumed by the REPL.

3. **QueryEngine owns mutable history:** Unlike functional React state, `QueryEngine` maintains mutable message history across turns. The REPL mirrors this into React state for rendering.

4. **Tool partitioning:** Tool calls are automatically partitioned into concurrent (safe/read-only) and serial (destructive) batches. This maximizes throughput while preserving safety.

5. **Permission modes with classifier:** The permission system supports 7 modes including `auto` which uses a YOLO classifier to auto-approve safe actions. Denial tracking prevents classifier loops.

6. **Context compaction:** Multiple compaction strategies (auto, micro, snip) manage token budget. Compact boundaries strip old history from API calls while preserving it in the transcript.

7. **Cache breakpoints:** The system injects `cache_control: { type: 'ephemeral' }` markers on messages to leverage Anthropic's prompt caching, reducing API costs.

8. **Feature flags via `feature()`:** Build-time feature gates (from `bun:bundle`) enable dead code elimination for internal/enterprise features.

---

## Appendix A: Key Files Reference

| Concern | File | Key Class / Function |
|---------|------|---------------------|
| CLI entry | `src/entrypoints/cli.tsx` | `main()` |
| Main entry | `src/main.tsx` | `main()`, `run()` |
| Init | `src/entrypoints/init.ts` | `init()` |
| REPL | `src/screens/REPL.tsx` | `REPL`, `onSubmit()`, `onQuery()` |
| App shell | `src/components/App.tsx` | `App` |
| Prompt input | `src/components/PromptInput/PromptInput.tsx` | `PromptInput` |
| Text input hook | `src/hooks/useTextInput.ts` | `useTextInput()` |
| Base text input | `src/components/BaseTextInput.tsx` | `BaseTextInput` |
| Messages list | `src/components/Messages.tsx` | `Messages` |
| Message renderer | `src/components/Message.tsx` | `Message` |
| Virtual scroll | `src/components/VirtualMessageList.tsx` | `VirtualMessageList` |
| Query engine | `src/QueryEngine.ts` | `QueryEngine`, `submitMessage()` |
| Query loop | `src/query.ts` | `query()`, `queryLoop()` |
| API streaming | `src/services/api/claude.ts` | `queryModelWithStreaming()` |
| API client | `src/services/api/client.ts` | `getAnthropicClient()` |
| Retry logic | `src/services/api/withRetry.ts` | `withRetry()` |
| Tool type | `src/Tool.ts` | `Tool`, `buildTool()`, `ToolUseContext` |
| Tool registry | `src/tools.ts` | `getAllBaseTools()`, `getTools()`, `assembleToolPool()` |
| Tool execution | `src/services/tools/toolExecution.ts` | `runToolUse()` |
| Tool orchestration | `src/services/tools/toolOrchestration.ts` | `runTools()`, `partitionToolCalls()` |
| Tool hooks | `src/services/tools/toolHooks.ts` | `runPreToolUseHooks()`, `runPostToolUseHooks()` |
| Store | `src/state/store.ts` | `createStore<T>()` |
| AppState | `src/state/AppStateStore.ts` | `AppState`, `getDefaultAppState()` |
| React state | `src/state/AppState.tsx` | `useAppState()`, `useSetAppState()` |
| Permissions | `src/utils/permissions/permissions.ts` | `hasPermissionsToUseTool()` |
| Interactive perm | `src/hooks/useCanUseTool.tsx` | `useCanUseTool()` |
| Perm handlers | `src/hooks/toolPermission/handlers/interactiveHandler.ts` | `handleInteractivePermission()` |
| System context | `src/context.ts` | `getSystemContext()`, `getUserContext()` |
| Prompt history | `src/history.ts` | `addToHistory()`, `getHistory()` |
| Ink core | `src/ink/ink.tsx` | `Ink` class |
| Ink reconciler | `src/ink/reconciler.ts` | custom reconciler |
| Ink renderer | `src/ink/renderer.ts` | renderer |
| Ink screen | `src/ink/screen.ts` | screen buffer |
| Ink input | `src/ink/hooks/use-input.ts` | `useInput()` |
| Print mode | `src/cli/print.ts` | `runHeadless()` |
| Structured IO | `src/cli/structuredIO.ts` | stream-json output |
| MCP entry | `src/entrypoints/mcp.ts` | MCP server entry |
| Bridge main | `src/bridge/bridgeMain.ts` | `bridgeMain()` |
| Daemon main | `src/daemon/main.ts` | `daemonMain()` |
| Background sessions | `src/cli/bg.ts` | `psHandler()`, `attachHandler()` |
| Bash tool | `src/tools/BashTool/BashTool.tsx` | `BashTool` |
| File read tool | `src/tools/FileReadTool/FileReadTool.ts` | `FileReadTool` |
| Agent tool | `src/tools/AgentTool/AgentTool.tsx` | `AgentTool` |
| Compact | `src/services/compact/compact.ts` | compaction logic |
| Auto compact | `src/services/compact/autoCompact.ts` | `autoCompactIfNeeded()` |
| Message utils | `src/utils/messages.ts` | `createUserMessage()`, `normalizeMessagesForAPI()` |
| API utils | `src/utils/api.ts` | `toolToAPISchema()`, `prependUserContext()` |

---

*End of analysis.*
