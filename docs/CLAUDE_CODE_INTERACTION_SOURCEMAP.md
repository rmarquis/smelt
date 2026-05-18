# Claude Code LLM Interaction Sourcemap

> **Document type**: Architectural analysis — exhaustive map of every code path that transforms a user keystroke into an LLM API request and back to rendered output.  
> **Source**: Reconstructed TypeScript from `@anthropic-ai/claude-code` v2.1.88 (`ChinaSiro/claude-code-sourcemap`).  
> **Date**: 2026-05-13

---

## 1. Overview — The Interaction Lifecycle

Claude Code is a TypeScript/React monolith built on top of the Anthropic SDK. A single user turn involves a sophisticated `queryLoop` generator that can spawn subagents, execute tools concurrently via a streaming executor, apply multiple compaction strategies, and route permission decisions through a multi-layer pipeline that may invoke a **secondary LLM classifier**.

```
┌─────────────────┐     ┌──────────────────────┐     ┌─────────────────┐
│  1. USER INPUT  │────▶│ 2. QUERYENGINE INIT  │────▶│ 3. QUERY LOOP   │
│   (keystroke)   │     │ (system prompt,      │     │ (LLM + tools +  │
│                 │     │  tools, permissions) │     │  compaction)    │
└─────────────────┘     └──────────────────────┘     └─────────────────┘
                                                              │
┌─────────────────┐     ┌──────────────────────┐             │
│ 5. RENDERED UI  │◀────│ 4. EVENT STREAM      │◀────────────┘
│  (transcript)   │     │ (deltas + tools)     │
└─────────────────┘     └──────────────────────┘
```

The `queryLoop` repeats until the assistant responds with **no `tool_use` blocks**.

---

## 2. Phase 1 — User Input & QueryEngine Init

**Source**: `src/QueryEngine.ts`, `src/utils/processUserInput/processUserInput.ts`

### 2.1 Input Capture

The `QueryEngine` class owns the conversation state. On `submitMessage()`:

1. **Process user input** via `processUserInput()` — handles slash commands (`/help`, `/clear`, `/compact`), attachments, skill loading, and message queue management.
2. **Build system prompt** via `fetchSystemPromptParts()` which calls `getSystemPrompt()` in `src/constants/prompts.ts`.
3. **Load skills and plugins** asynchronously.
4. **Yield a system init message** containing tool metadata, permission mode, commands, agents, and plugins.
5. If no API query is needed (e.g., pure slash command), yield result immediately.

### 2.2 Message State

`QueryEngine` maintains:
- `mutableMessages: Message[]` — the canonical conversation history
- `readFileState: FileStateCache` — per-session file read deduplication cache
- `permissionDenials: SDKPermissionDenial[]` — tracked for SDK consumers
- `abortController: AbortController` — cancellation signal

---

## 3. Phase 2 — System Prompt Assembly

**Source**: `src/constants/prompts.ts` (914 lines, 53 KB)

### 3.1 Static vs Dynamic Sections

Claude Code's system prompt is built from **static** (cross-session cacheable) and **dynamic** (per-session) sections, separated by a boundary marker:

```typescript
export const SYSTEM_PROMPT_DYNAMIC_BOUNDARY = '__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__'
```

**Static sections** (before boundary, eligible for `scope: 'global'` prompt caching):

| Section | Content |
|---------|---------|
| `Intro` | "You are an interactive agent that helps users with software engineering tasks" + cyber-risk instruction |
| `System` | All text outside tool use is displayed to user; tools execute in permission mode; system-reminder tags; automatic compression |
| `Doing tasks` | ~30 behavioral rules: don't propose changes without reading, don't create unnecessary files, avoid time estimates, diagnose before switching tactics, security warnings, code style rules (Ant-only: no comments unless WHY is non-obvious, verify before claiming success) |
| `Actions` | Detailed risky-action guidance: destructive ops, hard-to-reverse ops, shared-state ops, third-party uploads |
| `Using your tools` | Dedicated tools over Bash; parallel tool calls; task management (TodoWrite/TaskCreate) |
| `Tone and style` | No emojis, file_path:line_number references, owner/repo#123 format, no colon before tool calls |
| `Output efficiency` | External: "Go straight to the point, be extra concise". Ant: extensive prose-quality instructions (flowing prose, no fragments, inverted pyramid, match response to task) |

**Dynamic sections** (after boundary, per-session):

| Section | Content |
|---------|---------|
| `session_guidance` | Agent tool guidance, skill invocation rules, discover skills guidance, verification agent contract (Ant-only) |
| `memory` | Loaded from `memdir/` memory system |
| `env_info_simple` | Working directory, git status, platform, shell, OS version, model name, knowledge cutoff |
| `language` | User language preference |
| `output_style` | Custom output style config |
| `mcp_instructions` | Connected MCP server instructions |
| `scratchpad` | Per-session temp directory instructions |
| `frc` | Function result clearing (cached microcompact) |
| `summarize_tool_results` | "Write down important info from tool results, as they may be cleared later" |
| `numeric_length_anchors` | Ant-only: ≤25 words between tool calls, ≤100 words final response |
| `token_budget` | When user specifies token target, keep working until approaching it |
| `brief` | KAIROS brief mode instructions |

### 3.2 Subagent System Prompt Enhancement

Subagents get the parent's system prompt plus `enhanceSystemPromptWithEnvDetails()`:
- Notes about absolute paths, no emojis, no colon before tool calls
- Same `discoverSkillsGuidance` as main session
- Full `computeEnvInfo()` with model description

### 3.3 Prompt Caching Strategy

Claude Code aggressively optimizes for prompt caching:
- **Global cache scope**: static prefix cached across all users (requires `prompt_caching_scope` beta)
- **Ephemeral cache**: standard 5-minute TTL
- **1-hour TTL**: for eligible users (ant/subscribers within rate limits), latched per-session
- Cache control markers placed on the **last block** of each message
- Tool schemas sorted for cache stability; built-ins form a contiguous prefix

---

## 4. Phase 3 — Tool Assembly

**Source**: `src/tools.ts`, `src/Tool.ts`

### 4.1 Tool Pool Construction

Tools are assembled via `assembleToolPool(permissionContext, mcpTools)`:

1. **Get built-in tools** via `getTools(permissionContext)`:
   - Filter by deny rules (blanket denies strip tools before the model sees them)
   - Filter by `isEnabled()`
   - Hide `REPL_ONLY_TOOLS` when REPL mode is active
   - Simple mode (`CLAUDE_CODE_SIMPLE`): only Bash, Read, Edit (+ REPL if enabled)

2. **Filter MCP tools** by deny rules

3. **Deduplicate** by name (built-ins take precedence over MCP)

4. **Sort** for cache stability: built-ins first (contiguous prefix), then MCP tools

### 4.2 Tool Filtering Layers

**Not all tools are always presented.** Multiple filters apply:

| Filter | Mechanism |
|--------|-----------|
| **Deny rules** | `filterToolsByDenyRules()` — user-configured blanket denies (e.g., `mcp__server1` strips all tools from that server) |
| **isEnabled()** | Per-tool runtime check (feature flags, platform, etc.) |
| **REPL mode** | Hides primitive tools when REPL wraps them |
| **Simple mode** | Restricts to Bash/Read/Edit only |
| **Agent disallowed** | `ALL_AGENT_DISALLOWED_TOOLS` — subagents can't use TaskOutput, ExitPlanMode, EnterPlanMode, AskUserQuestion, TaskStop, Workflow |
| **Async agent allowed** | `ASYNC_AGENT_ALLOWED_TOOLS` — background agents restricted to read/search/edit tools |
| **Deferred loading** | `ToolSearch` — large tool pools defer non-essential tools; model must "discover" them via `tool_reference` blocks |
| **LSP deferred** | LSP tools deferred until initialization completes |

### 4.3 Tool Definition Structure

Each tool is a rich object (`Tool` type in `src/Tool.ts`):

```typescript
type Tool = {
  name: string
  aliases?: string[]
  searchHint?: string
  call(args, context, canUseTool, parentMessage, onProgress): Promise<ToolResult>
  description(input, options): Promise<string>
  inputSchema: z.ZodType
  inputJSONSchema?: ToolInputJSONSchema
  isConcurrencySafe(input): boolean
  isReadOnly(input): boolean
  isDestructive?(input): boolean
  checkPermissions(input, context): Promise<PermissionResult>
  validateInput?(input, context): Promise<ValidationResult>
  // ... 20+ more methods for rendering, progress, grouping, etc.
}
```

**Key difference from Smelt**: Claude Code tools are **TypeScript classes** with ~30 methods each (rendering, progress, permission checking, auto-classifier formatting, search text extraction, grouped rendering, etc.). Smelt tools are **Lua tables** with ~10 hooks.

---

## 5. Phase 4 — The Query Loop

**Source**: `src/query.ts` (the core agent loop)

### 5.1 Loop Structure

`queryLoop()` is an async generator with mutable `State` carried between iterations:

```typescript
type State = {
  messages: Message[]
  toolUseContext: ToolUseContext
  autoCompactTracking: AutoCompactTrackingState | undefined
  maxOutputTokensRecoveryCount: number
  hasAttemptedReactiveCompact: boolean
  maxOutputTokensOverride: number | undefined
  pendingToolUseSummary: Promise<ToolUseSummaryMessage | null> | undefined
  stopHookActive: boolean | undefined
  turnCount: number
  transition: Continue | undefined  // why previous iteration continued
}
```

### 5.2 Per-Iteration Setup

Before each API call, the loop performs **compaction preprocessing** (in order):

1. **Apply tool result budget** — persist oversized tool results to disk, replace with preview
2. **Snip compaction** — remove zombie messages and stale markers (HISTORY_SNIP feature)
3. **Microcompact** — cached microcompact (cache editing) removes old tool results
4. **Context collapse** — staged collapses of search/read operations into condensed summaries
5. **Autocompact** — proactive summary of old history when token threshold exceeded

### 5.3 API Call

Calls `deps.callModel()` which invokes `queryModelWithStreaming()` in `src/services/api/claude.ts`:

- Anthropic Messages API (`/v1/messages`)
- Beta headers for features: prompt caching, tool search, advisor, effort, task budgets, anti-distillation, etc.
- Streaming via SSE
- Non-streaming fallback on 529 errors or streaming failures
- Model fallback support (e.g., Opus → Sonnet on high demand)

### 5.4 Response Handling

During streaming:
- Accumulate assistant messages and tool_use blocks
- `StreamingToolExecutor` begins executing tools **as soon as their tool_use blocks arrive**
- Yield stream events, assistant messages, and completed tool results progressively

After streaming completes:
- If `needsFollowUp` (tool_use blocks present): execute remaining tools, append tool_results, continue loop
- If no tool_use: run stop hooks, check success, yield final result

### 5.5 Recovery Paths

The loop has multiple error recovery mechanisms:

| Error | Recovery |
|-------|----------|
| **Prompt too long (413)** | Context collapse drain → reactive compact → surface error |
| **Max output tokens** | Escalate to 64k tokens (once) → multi-turn recovery with meta message |
| **Media size error** | Reactive compact strips images → retry |
| **Model fallback (529)** | Switch to fallback model, retry entire request |
| **Streaming failure** | Fall back to non-streaming request |
| **API error** | Yield synthetic assistant error message |

---

## 6. Phase 5 — API Request Construction

**Source**: `src/services/api/claude.ts`

### 6.1 Request Building

The `queryModel()` generator builds:

```typescript
{
  model: "claude-sonnet-4-6",
  max_tokens: 8192,  // or 64000 for recovery
  system: [
    { type: "text", text: "...", cache_control: { type: "ephemeral", scope: "global" } },
    // ... more system blocks
  ],
  messages: [
    // user/assistant/tool_result blocks with cache_control on last block
  ],
  tools: [
    { name: "Bash", description: "...", input_schema: {...}, cache_control: {...} },
    // ... deferred tools have defer_loading: true
  ],
  thinking: { type: "adaptive", budget_tokens: 32000 },  // or disabled
  betas: ["prompt-caching-...", "tool-search-...", "advisor-..."],
  metadata: { user_id: "..." },
  // Optional: output_config with effort or task_budget
}
```

### 6.2 Beta Headers & Feature Gates

Claude Code uses numerous Anthropic beta features:

| Beta Header | Purpose |
|-------------|---------|
| `prompt-caching-2024-07-31` | Standard prompt caching |
| `prompt-caching-scope-2025-...` | Global scope for cross-user caching |
| `advanced-tool-use-2025-...` / `tool-search-tool-2025-...` | Deferred/dynamic tool loading |
| `advisor-2026-03-01` | Server-side advisor tool |
| `effort-2025-...` | Output effort control |
| `task-budgets-2026-03-13` | API-side token budget awareness |
| `context-management-2025-...` | Cached microcompact |
| `anti-distillation-...` | Fake tool injection (1P only) |
| `cache-editing-...` | Cache editing for microcompact |

### 6.3 Tool Search / Deferred Loading

When tool search is enabled:
- Non-essential tools are sent with `defer_loading: true`
- The model can "discover" them by emitting `tool_reference` blocks
- Discovered tools are added to the tool pool for subsequent turns
- LSP tools are deferred until LSP initialization completes
- This allows **unlimited tool quantities** without context bloat

---

## 7. Phase 6 — Permission System

**Source**: `src/utils/permissions/permissions.ts` (the most complex permission pipeline)

### 7.1 Permission Pipeline (`hasPermissionsToUseToolInner`)

The permission check is a **7-step deterministic pipeline**:

```
Step 1a: Deny rule for entire tool? → DENY
Step 1b: Ask rule for entire tool? → ASK (unless sandbox auto-allow)
Step 1c: Tool.checkPermissions(parsedInput, context) → allow/ask/deny/passthrough
Step 1d: Tool implementation denied? → DENY
Step 1e: Tool requires user interaction? → ASK
Step 1f: Content-specific ask rule? → ASK
Step 1g: Safety check (e.g., .git/, .claude/)? → ASK (bypass-immune)

Step 2a: bypassPermissions / plan+bypass available? → ALLOW
Step 2b: Always-allowed rule? → ALLOW

Step 3: Passthrough → ASK
```

### 7.2 Mode-Based Transformations

After the pipeline, mode transformations apply:

| Mode | Behavior |
|------|----------|
| `bypassPermissions` | Skip all prompts, allow everything (except safety checks) |
| `acceptEdits` | Auto-allow file edits in working directory |
| `plan` | Same as default, but records that bypass was available |
| `dontAsk` | Convert ASK → DENY |
| `auto` | **Run AI classifier** to approve/deny (see below) |
| `default` | Normal prompting |

### 7.3 Auto Mode — The Classifier LLM

When mode is `auto`, a **secondary LLM call** classifies whether each tool use is safe:

```typescript
// Fast-path 1: acceptEdits check — would this be allowed in acceptEdits mode?
if (acceptEditsResult.behavior === 'allow') → ALLOW

// Fast-path 2: safe-tool allowlist
if (isAutoModeAllowlistedTool(tool.name)) → ALLOW

// Main path: run classifier LLM
classifierResult = await classifyYoloAction(
  context.messages,  // full conversation history
  action,            // formatted tool use (e.g., "ls -la")
  context.options.tools,
  permissionContext,
  signal
)
```

**Classifier behavior**:
- Two-stage pipeline (stage 1: quick safety check, stage 2: full analysis if needed)
- Returns: `allowed`, `blocked` (with reason), or `unavailable`
- **Fail-closed** by default (`tengu_iron_gate_closed` gate): if classifier unavailable → DENY
- Tracks consecutive denials; after too many denials, falls back to prompting user
- Logs extensive telemetry: tokens, cost, latency, stage breakdown

**This is a key architectural difference from Smelt**: Claude Code uses a **separate LLM** as a security gate in auto mode. Smelt uses **explicit Lua hooks** (`decide`, `confirm_text`, `approval_patterns`) with no secondary LLM.

### 7.4 Permission Rules

Rules have 3 behaviors (`allow`, `ask`, `deny`), 6 sources (`userSettings`, `projectSettings`, `localSettings`, `cliArg`, `command`, `session`), and can target:
- Entire tools (`Bash`)
- Subcommands (`Bash(git *)`)
- MCP servers (`mcp__server1` or `mcp__server1__*`)
- Agent types (`Agent(Explore)`)

---

## 8. Phase 7 — Tool Execution

**Source**: `src/services/tools/StreamingToolExecutor.ts`, `src/services/tools/toolOrchestration.ts`

### 8.1 Streaming Tool Execution

Claude Code supports **streaming tool execution**: tools begin executing as soon as their `tool_use` blocks arrive from the API, before the full assistant message is complete.

`StreamingToolExecutor`:
- Receives `tool_use` blocks incrementally
- Calls `canUseTool()` for permission check
- Spawns tool execution
- Yields `tool_result` blocks as tools complete
- Handles abort by generating synthetic tool_results for in-progress tools

### 8.2 Tool Orchestration

`runTools()` manages parallel execution:
- Groups tools by concurrency safety
- Runs safe tools in parallel
- Runs unsafe tools sequentially
- Handles progress callbacks for long-running tools

### 8.3 Tool Result Format

Tool results are mapped to Anthropic's `tool_result` blocks:

```typescript
{
  type: "tool_result",
  tool_use_id: "tu_01xxx",
  content: "...output text...",
  is_error: false
}
```

Images in tool results are counted against `API_MAX_MEDIA_PER_REQUEST` (100); oldest media is stripped silently.

---

## 9. Phase 8 — Compaction Strategies

Claude Code has **five distinct compaction mechanisms**:

### 9.1 Autocompact (Proactive)

**Source**: `src/services/compact/autoCompact.ts`

- Trigger: token count exceeds threshold (model-dependent, ~80% of context window)
- Action: fork a **subagent** to summarize old history into a compact summary
- Preserves: system prompt, recent user message, and critical attachments
- Result: `compact_boundary` system message replaces old history

### 9.2 Reactive Compact

**Source**: `src/services/compact/reactiveCompact.ts`

- Trigger: API returns 413 (prompt too long) or media size error
- Action: emergency summary after the fact
- Withholds the error from SDK until recovery is attempted
- One-shot per turn; if retry still fails, surfaces error

### 9.3 Microcompact (Cache Editing)

**Source**: `src/services/compact/microCompact.ts`, `src/services/compact/cachedMicrocompact.ts`

- Uses Anthropic's **cache editing** beta to delete old tool results from the prompt cache
- Server-side operation; no LLM call needed
- More efficient than autocompact for tool-heavy conversations
- Tracks deleted tokens via `cache_deleted_input_tokens`

### 9.4 Snip Compaction

**Source**: `src/services/compact/snipCompact.ts`

- Removes "zombie messages" and stale markers from history
- Lightweight, runs before every API call
- Frees tokens without summarization

### 9.5 Context Collapse

**Source**: `src/services/contextCollapse/`

- Collapses search/read operations into condensed summaries
- Staged collapses committed when token pressure rises
- Read-time projection over full history; summaries live in collapse store, not REPL array
- Recovery from 413: drain staged collapses first, then fall back to reactive compact

---

## 10. Phase 9 — Subagents & Forks

**Source**: `src/tools/AgentTool/runAgent.ts`, `src/tools/AgentTool/forkSubagent.ts`

### 10.1 Agent Tool

The `Agent` tool spawns subagents with:
- Their own `systemPrompt` (from agent definition)
- Their own `toolPool` (filtered via `resolveAgentTools`)
- Their own `permissionMode` (can override parent's)
- Their own `abortController` (async agents are unlinked)
- Their own `MCP clients` (agent-specific frontmatter servers)

### 10.2 Agent Types

| Type | Purpose | Tool Restrictions |
|------|---------|-------------------|
| **Explore** | Broad codebase research | Read-only tools |
| **Plan** | Implementation planning | Read-only + AskUserQuestion |
| **Verify** | Adversarial verification of implementation | Read + Bash(test) |
| **Fork** | Background execution keeping output out of context | Inherits parent's exact tool pool for cache sharing |
| **Custom** | User-defined agents via `.claude/agents/` | Configurable via frontmatter |

### 10.3 Fork Subagent

Forks are special subagents that:
- Share the parent's **byte-identical API request prefix** for prompt cache hits
- Run asynchronously in the background
- Keep tool output out of the main context window
- Used for: research, multi-step implementation, verification

---

## 11. Phase 10 — Event Stream & Rendering

**Source**: `src/QueryEngine.ts`, React components in `src/components/`

### 11.1 Message Types

```typescript
type Message =
  | UserMessage
  | AssistantMessage
  | SystemMessage
  | ProgressMessage
  | AttachmentMessage
  | CompactBoundaryMessage
  | TombstoneMessage
```

### 11.2 Rendering

- **Assistant text**: streamed deltas aggregated into text blocks
- **Thinking blocks**: preserved across tool_use trajectories (complex rules in `query.ts` comments)
- **Tool use**: custom React components per tool (diffs for edits, previews for reads, etc.)
- **Tool results**: rendered via `renderToolResultMessage()` per tool
- **Progress**: real-time progress messages during long tool execution

---

## 12. What the LLM Actually Sees

### 12.1 First Call of a Turn (Anthropic Messages API)

```json
{
  "model": "claude-sonnet-4-6",
  "max_tokens": 8192,
  "system": [
    {
      "type": "text",
      "text": "You are an interactive agent...",
      "cache_control": { "type": "ephemeral", "scope": "global" }
    },
    {
      "type": "text",
      "text": "# Doing tasks\n - ...",
      "cache_control": { "type": "ephemeral", "scope": "global" }
    },
    "__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__",
    {
      "type": "text",
      "text": "# Environment\n - Working directory: /home/user/project..."
    }
  ],
  "messages": [
    {
      "role": "user",
      "content": [
        { "type": "text", "text": "previous user message" }
      ]
    },
    {
      "role": "assistant",
      "content": [
        { "type": "text", "text": "previous assistant reply" }
      ]
    },
    {
      "role": "user",
      "content": [
        { "type": "text", "text": "current user message", "cache_control": { "type": "ephemeral" } }
      ]
    }
  ],
  "tools": [
    {
      "name": "Bash",
      "description": "Execute bash commands...",
      "input_schema": { "type": "object", "properties": {...} },
      "cache_control": { "type": "ephemeral" }
    },
    {
      "name": "Read",
      "description": "Read file contents...",
      "input_schema": { ... },
      "cache_control": { "type": "ephemeral" }
    }
  ],
  "thinking": { "type": "adaptive", "budget_tokens": 32000 },
  "betas": ["prompt-caching-2024-07-31", "prompt-caching-scope-..."]
}
```

### 12.2 After Tool Execution

```json
{
  "model": "claude-sonnet-4-6",
  "messages": [
    // ... prior history ...
    {
      "role": "assistant",
      "content": [
        { "type": "text", "text": "Let me check the files." },
        { "type": "tool_use", "id": "tu_01abc", "name": "Bash", "input": { "command": "ls -la" } },
        { "type": "tool_use", "id": "tu_02def", "name": "Read", "input": { "file_path": "/home/user/project/README.md" } }
      ]
    },
    {
      "role": "user",
      "content": [
        {
          "type": "tool_result",
          "tool_use_id": "tu_01abc",
          "content": "total 128\ndrwxr-xr-x ..."
        },
        {
          "type": "tool_result",
          "tool_use_id": "tu_02def",
          "content": "# Project\n..."
        }
      ]
    }
  ]
}
```

---

## 13. Tool Inventory

### 13.1 Built-in Tools

| Tool | Category | Permission Default | Key Features |
|------|----------|-------------------|--------------|
| `Bash` | Shell | Ask (classifier in auto mode) | Streaming, timeout, sandbox support, subcommand rules |
| `Read` | File | Allow | Line offsets, image support, notebook rendering |
| `Edit` | File | Ask (acceptEdits auto-allows) | Exact-string replacement, diff preview |
| `Write` | File | Ask | Create new files, overwrite protection |
| `Glob` | Search | Allow | File pattern matching |
| `Grep` | Search | Allow | Content search, regex |
| `NotebookEdit` | File | Ask | Jupyter notebook cell editing |
| `WebFetch` | Web | Ask | URL fetching, content extraction |
| `WebSearch` | Web | Ask | Web search |
| `AskUserQuestion` | UX | Allow | Sequential user dialogs |
| `Agent` | Meta | Varies | Spawn subagents (Explore, Plan, Verify, Fork, Custom) |
| `TaskCreate/Get/Update/List` | Task | Varies | Task management (v2) |
| `TodoWrite` | Task | Allow | Todo list management |
| `Skill` | Meta | Allow | Execute user-defined skills |
| `ExitPlanMode` | Mode | — | Exit plan mode |
| `EnterPlanMode` | Mode | — | Enter plan mode |
| `TaskStop` | Control | — | Stop running tasks |
| `Brief` | UX | — | Proactive brief mode |
| `Config` | Settings | Ant-only | Configuration management |
| `Tungsten` | Internal | Ant-only | Internal tool |
| `REPL` | Shell | Ant-only | JavaScript VM wrapper |
| `PowerShell` | Shell | Ask | Windows PowerShell |
| `LSPTool` | Code | Env-gated | LSP integration |
| `ToolSearch` | Meta | Opt-in | Discover deferred tools |
| `ListMcpResources` / `ReadMcpResource` | MCP | — | MCP resource access |

### 13.2 Conditional/Feature-Gated Tools

Many tools are conditionally included based on feature flags:
- `SleepTool` (PROACTIVE / KAIROS)
- `CronCreate/Delete/List` (AGENT_TRIGGERS)
- `RemoteTriggerTool` (AGENT_TRIGGERS_REMOTE)
- `MonitorTool` (MONITOR_TOOL)
- `SendUserFileTool` (KAIROS)
- `PushNotificationTool` (KAIROS)
- `SubscribePRTool` (KAIROS_GITHUB_WEBHOOKS)
- `WebBrowserTool` (WEB_BROWSER_TOOL)
- `WorkflowTool` (WORKFLOW_SCRIPTS)
- `SnipTool` (HISTORY_SNIP)
- `TerminalCaptureTool` (TERMINAL_PANEL)
- `CtxInspectTool` (CONTEXT_COLLAPSE)

---

## 14. Source Files Referenced

### Core Interaction Flow
- `src/QueryEngine.ts` — Query lifecycle, session state, message management
- `src/query.ts` — The main agent loop (`queryLoop`)
- `src/services/api/claude.ts` — API request building, streaming, retries
- `src/services/api/withRetry.ts` — Retry logic with fallback

### Tools
- `src/tools.ts` — Tool pool assembly, filtering, deduplication
- `src/Tool.ts` — Tool type definition, `buildTool()` helper
- `src/services/tools/StreamingToolExecutor.ts` — Streaming execution
- `src/services/tools/toolOrchestration.ts` — Parallel/sequential execution

### Prompts
- `src/constants/prompts.ts` — System prompt construction (~800 lines)
- `src/constants/systemPromptSections.ts` — Section registry and caching
- `src/utils/api.ts` — `buildSystemPromptBlocks()`, cache control

### Permissions
- `src/utils/permissions/permissions.ts` — Main permission pipeline (~1000 lines)
- `src/utils/permissions/PermissionMode.ts` — Mode definitions
- `src/utils/permissions/yoloClassifier.ts` — Auto mode classifier
- `src/utils/permissions/classifierDecision.ts` — Classifier decision logic
- `src/utils/permissions/denialTracking.ts` — Denial limit tracking

### Compaction
- `src/services/compact/autoCompact.ts` — Proactive autocompact
- `src/services/compact/reactiveCompact.ts` — Reactive compact
- `src/services/compact/microCompact.ts` — Microcompact
- `src/services/compact/cachedMicrocompact.ts` — Cache editing
- `src/services/compact/snipCompact.ts` — Snip compaction
- `src/services/contextCollapse/` — Context collapse

### Subagents
- `src/tools/AgentTool/runAgent.ts` — Subagent execution
- `src/tools/AgentTool/forkSubagent.ts` — Fork subagents
- `src/tools/AgentTool/agentToolUtils.ts` — Tool resolution for agents

### Types & Messages
- `src/types/message.ts` — Message type definitions
- `src/utils/messages.ts` — Message creation, normalization, pairing

---

## 15. Key Architectural Characteristics

1. **Monolithic TypeScript**: Everything in one codebase; no plugin runtime like Smelt's Lua
2. **Extensive feature gating**: Heavy use of `feature('FLAG')` and `process.env.USER_TYPE === 'ant'` for internal vs external builds
3. **Prompt cache obsession**: Every design decision considers cache stability (sorting, boundary markers, latch headers, deduplication)
4. **Multiple LLMs**: Main model + classifier model (auto mode) + advisor model + compaction subagents
5. **Streaming-first**: Tools execute during streaming, not after full response
6. **Five compaction strategies**: Layered approaches to context window management
7. **Permission as a pipeline**: 7 deterministic steps + mode transformations + optional classifier
8. **Rich tool definitions**: ~30 methods per tool (rendering, progress, permissions, classifier formatting)
