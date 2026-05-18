# Kimi Code CLI — Architectural Analysis
## Every Code Path from Keystroke to LLM Request to Rendered Output

_Analyzed by Claude Sonnet 4.6 — May 2026_

---

## Overview

Kimi Code CLI is a Python 3.12+ terminal agent. Unlike smelt's two-thread model (TUI + async engine), kimi-cli runs everything inside a single `asyncio` event loop. UI, soul logic, LLM calls, and tool execution are all co-routines in the same loop. Inter-component communication uses Python `asyncio.Queue` channels wrapped in a `Wire` abstraction, while the LLM abstraction is provided by the bundled `kosong` package.

**Key architectural pillars:**

| Concept | Implementation |
|---|---|
| Entry point | `cli/__init__.py` → `kimi()` Typer callback |
| Session management | `Session.create/find/continue_` with `~/.kimi/` storage |
| App factory | `KimiCLI.create()` — config, OAuth, LLM, Runtime, Agent, Soul |
| Agent loop | `KimiSoul._agent_loop()` / `_step()` |
| LLM abstraction | `kosong.step()` → `ChatProvider.generate()` (streaming) |
| Wire protocol | `Wire` SPMC queue (soul-side → UI-side), `WireMessage` Pydantic models |
| Tool dispatch | `KimiToolset.handle()` → `asyncio.create_task(_call())` |
| Approval gating | `Approval.request()` → `ApprovalRuntime` → Wire `ApprovalRequest` |
| UI rendering | Rich `Live` display driven by Wire events (`visualize()`) |
| Compaction | `SimpleCompaction.compact()` → second LLM call for summarisation |

---

## Phase 0 — Process Entry and CLI Parsing

**Files:** `src/kimi_cli/__main__.py`, `src/kimi_cli/cli/__init__.py`

### 0.1 `__main__` → Typer

```
python -m kimi_cli  (or `kimi` console script)
  → __main__.py → cli/__init__.py:cli() Typer app
```

`cli/__main__.py` routes to `cli/__init__.py`. The Typer app is built at module level with `LazySubcommandGroup` for deferred sub-command loading.

### 0.2 `kimi()` callback — flag validation and mode selection

The `@cli.callback(invoke_without_command=True)` function `kimi()` in `cli/__init__.py` handles all flags:

- **Mode flags**: `--print` → `ui="print"`, `--acp` → `ui="acp"`, `--wire` → `ui="wire"`, default → `ui="shell"`
- **Session flags**: `--session ID` (resume), `--continue` (previous session for cwd), or none (new session)
- **Agent spec**: `--agent default|okabe` resolves to a YAML file path; `--agent-file` takes a custom path
- **MCP config**: `--mcp-config-file` / `--mcp-config` parsed as JSON
- **Loop control**: `--max-steps-per-turn`, `--max-retries-per-step`, `--max-ralph-iterations`
- **Approval**: `--yolo` / `--afk` / `--plan`

Logging is enabled early via `enable_logging(debug, redirect_stderr=False)` — stderr is NOT yet redirected so Typer parse errors remain visible. The redirect happens inside `_run()`, just before `KimiCLI.create()`.

### 0.3 Session lifecycle

```python
session = await Session.create(work_dir)        # new
session = await Session.find(work_dir, id)      # resume by ID
session = await Session.continue_(work_dir)     # previous session for cwd
```

Sessions live in `~/.kimi/<session_id>/`. Key files: `context.jsonl` (conversation history), `wire.jsonl` (event replay log), `state.json` (approval/plan state, custom title), `subagents/` (subagent instances).

### 0.4 Reload/switch loop

`_reload_loop()` wraps `_run()` in a `while True` loop. Three exceptions drive restarts:
- `Reload(session_id=...)` — `/model`, `/theme`, `/fork`, `/new`, session switch
- `SwitchToWeb` — opens browser web UI via uvicorn
- `SwitchToVis` — opens tracing visualizer

The final `asyncio.run(_reload_loop(session_id))` enters the event loop.

---

## Phase 1 — App Factory: `KimiCLI.create()`

**File:** `src/kimi_cli/app.py`

`KimiCLI.create()` is the single factory that builds every runtime component before the UI starts. It runs sequentially inside `asyncio.run()`.

### 1.1 Config loading

```python
config = load_config(config_file)  # ~/.kimi/config.toml
```

CLI overrides (`--config`, `--config-file`, `--model`, `--thinking`, `--max-*`) are applied to `config.loop_control`.

### 1.2 LLM construction

```python
llm = create_llm(provider, model, thinking=thinking, session_id=session.id, oauth=oauth)
```

`create_llm()` in `llm.py` dispatches on `provider.type`:

| `provider.type` | `ChatProvider` class |
|---|---|
| `"kimi"` | `kosong.chat_provider.kimi.Kimi` (OpenAI-compatible via `AsyncOpenAI`) |
| `"openai_legacy"` | `kosong.contrib.chat_provider.openai_legacy.OpenAILegacy` |
| `"openai_responses"` | `kosong.contrib.chat_provider.openai_responses.OpenAIResponses` |
| `"anthropic"` | `kosong.contrib.chat_provider.anthropic.Anthropic` |
| `"gemini"` / `"google_genai"` | `kosong.contrib.chat_provider.google_genai.GoogleGenAI` |
| `"vertexai"` | `GoogleGenAI(vertexai=True)` |
| `"_echo"` / `"_scripted_echo"` | Test stubs |

The `Kimi` provider wraps `AsyncOpenAI` with `base_url` pointing at `https://api.moonshot.ai/v1` (or `KIMI_BASE_URL`). A background task `_refresh_managed_models_silent()` runs in parallel to update OAuth-managed model lists.

### 1.3 Runtime creation

```python
runtime = await Runtime.create(config, oauth, llm, session, yolo, afk=afk, ...)
```

`Runtime.create()` in `soul/agent.py` runs several async steps in parallel:

```python
ls_output, agents_md, environment = await asyncio.gather(
    list_directory(session.work_dir),     # workspace ls
    load_agents_md(session.work_dir),     # merge AGENTS.md chain
    Environment.detect(),                  # OS/shell detection
)
```

It then:
- Discovers skills via `discover_skills_from_roots()`
- Restores `additional_dirs` from `session.state`
- Builds `ApprovalState` from merged yolo/afk flags + persisted `session.state.approval`
- Constructs `DenwaRenji`, `LaborMarket`, `NotificationManager`, `BackgroundTaskManager`
- Creates `SubagentStore`, `ApprovalRuntime`, `RootWireHub`

The `Runtime` dataclass holds every singleton service that tools and the soul need.

### 1.4 Agent loading

```python
agent = await load_agent(agent_file, runtime, mcp_configs=mcp_configs, ...)
```

`load_agent()` in `soul/agent.py`:

1. **Parses YAML spec** (`AgentSpec`) from `src/kimi_cli/agents/default/agent.yaml`
2. **Renders system prompt** via Jinja2 with builtin vars: `KIMI_NOW`, `KIMI_WORK_DIR`, `KIMI_WORK_DIR_LS`, `KIMI_AGENTS_MD`, `KIMI_SKILLS`, `KIMI_OS`, `KIMI_SHELL` — all collected in `BuiltinSystemPromptArgs`
3. **Registers builtin subagent types** in `LaborMarket` (e.g. `coder`, `explore`, `plan` from `agents/default/`)
4. **Loads tools** via `KimiToolset.load_tools(tool_paths, deps)` — each path is an import string like `kimi_cli.tools.shell:Shell`
5. **Loads plugin tools** from `~/.kimi/plugins/`
6. **Loads MCP tools** in background (`in_background=True`) or defers them for first-turn startup

### 1.5 Context restore and Soul creation

```python
context = Context(session.context_file)
await context.restore()          # load history from context.jsonl

soul = KimiSoul(agent, context=context)
```

`KimiSoul.__init__` builds the slash command registry (soul-level + skill commands + flow commands), binds plan-mode state to tools, and initialises the steer queue.

### 1.6 Hook engine + telemetry

```python
hook_engine = HookEngine(config.hooks, cwd=str(session.work_dir))
soul.set_hook_engine(hook_engine)
```

Hooks are shell commands that fire on events (see Phase 6). Telemetry is attached last.

---

## Phase 2 — UI Entry: Shell `run()`

**File:** `src/kimi_cli/ui/shell/__init__.py` — `Shell.run()`

### 2.1 Prompt session construction

`CustomPromptSession` from `ui/shell/prompt.py` wraps `prompt_toolkit` with:
- Status bar (`status_provider` → `KimiSoul.status`)
- MCP status block
- Background task count badge
- Agent-mode vs shell-mode slash command completion
- Plan mode toggle callback (Ctrl-P)

### 2.2 Prompt router task

```python
prompt_task = asyncio.create_task(
    self._route_prompt_events(prompt_session, idle_events, resume_prompt)
)
```

`_route_prompt_events()` loops forever:
1. `await prompt_session.prompt_next()` — blocks on terminal input (prompt_toolkit)
2. On `KeyboardInterrupt` → puts `_PromptEvent(kind="interrupt")` in `idle_events`
3. On `EOFError` → puts `_PromptEvent(kind="eof")`
4. On normal input → clears `resume_prompt`, puts `_PromptEvent(kind="input", user_input=user_input)`

The `resume_prompt` event gate prevents the router from reading while the agent is blocking (no-steer mode).

### 2.3 Main loop

```python
while True:
    result = await bg_watcher.wait_for_next(idle_events)
    event = result
    # ... handle event.kind in ("interrupt", "eof", "input", "bg_noop", "cwd_lost", "error")
```

`_BackgroundCompletionWatcher.wait_for_next()` races between `idle_events.get()` and `background_tasks.completion_event.wait()`. If a background task finishes while idle, it fires a synthetic auto-trigger prompt `"<system-reminder>Background tasks completed while you were idle.</system-reminder>"`.

### 2.4 Input classification

```python
action = classify_input(input_text, is_streaming=False)
```

`classify_input()` in `ui/shell/visualize/_input_router.py` returns an `InputAction`:
- `BTW` — `/btw <question>` side-channel queries
- `IGNORED` — whitespace-only input
- Otherwise falls through to shell-command or agent routing

### 2.5 Shell vs agent routing

```python
if user_input.mode == PromptMode.SHELL:
    await self._run_shell_command(user_input.command)  # exec in subprocess

# resolve slash command
if slash_cmd_call := self._agent_slash_command_call(user_input):
    is_soul_slash = shell_slash_registry.find_command(slash_cmd_call.name) is None
    if is_soul_slash:
        await self.run_soul_command(slash_cmd_call.raw_input)
    else:
        await self._run_slash_command(slash_cmd_call)   # shell-level command
else:
    await self.run_soul_command(user_input.content)
```

Shell-mode commands (Ctrl-X toggled) are executed in a subprocess via `asyncio.create_subprocess_shell`. Agent input goes to `run_soul_command()`.

---

## Phase 3 — `run_soul_command()` → Wire Setup

**File:** `src/kimi_cli/ui/shell/__init__.py`, `src/kimi_cli/soul/__init__.py`

### 3.1 `run_soul_command()`

```python
async def run_soul_command(self, command: str | list[ContentPart]) -> bool:
    cancel_event = asyncio.Event()
    install_sigint_handler(lambda: cancel_event.set())
    async for msg in self.soul_run_messages(command, cancel_event):
        ...  # UI handles each WireMessage
```

### 3.2 `run_soul()` — Wire instantiation

`run_soul()` in `soul/__init__.py` is the central connector:

```python
wire = Wire(file_backend=wire_file)      # SPMC channel with optional file recorder
wire_token = _current_wire.set(wire)     # set ContextVar for wire_send()

ui_task = asyncio.create_task(ui_loop_fn(wire))    # UI consumer
soul_task = asyncio.create_task(soul.run(...))      # agent producer
notification_task = asyncio.create_task(
    _pump_notifications_to_wire(runtime, wire)      # background pump, 1s interval
)
```

The `Wire` object has two `BroadcastQueue` channels:
- **`_raw_queue`**: every message immediately, in order
- **`_merged_queue`**: adjacent `MergeableMixin` messages (e.g. `TextPart` tokens) are merged before publication

A `_WireRecorder` task subscribes to `_merged_queue` and appends each message to `wire.jsonl` for session replay.

`wire_send(msg)` in `soul/__init__.py` gets the current `Wire` from a `ContextVar` and calls `wire.soul_side.send(msg)`.

### 3.3 Cancellation

`asyncio.wait([soul_task, cancel_event_task], return_when=FIRST_COMPLETED)`:
- If `cancel_event` is set: `soul_task.cancel()` → `RunCancelled` raised
- If `soul_task` finishes: `cancel_event_task.cancel()`

On exit: `wire.shutdown()` closes both queues, breaking the UI loop. The UI task is awaited with a 0.5 s timeout.

---

## Phase 4 — Shell UI Loop: `visualize()`

**File:** `src/kimi_cli/ui/shell/visualize/_live_view.py`, `_interactive.py`

The `ui_loop_fn` passed to `run_soul()` is `visualize()` from `ui/shell/visualize/`. It:

1. Subscribes to `wire.ui_side(merge=True)` — receives merged Wire messages
2. Opens a Rich `Live` context for the bottom dynamic area
3. Loops `await wire_ui.receive()` and dispatches by message type:

| `WireMessage` type | Shell UI action |
|---|---|
| `TurnBegin` | Reset display state, start new turn rendering |
| `StepBegin(n=N)` | Show step counter |
| `TextPart(text=...)` | Stream text into active content block (Rich Markdown) |
| `ThinkPart(text=...)` | Stream thinking text (if `show_thinking_stream` config) |
| `ToolCall(...)` | Create a `_ToolCallBlock` with tool name + args |
| `ToolResult(...)` | Update the tool call block with result / display block |
| `StatusUpdate(...)` | Update context usage bar, token counts, plan mode |
| `ApprovalRequest(...)` | Show approval panel, await user response |
| `CompactionBegin/End` | Show "compacting context…" indicator |
| `MCPLoadingBegin/End` | Show "loading MCP…" spinner |
| `StepRetry(...)` | Show retry countdown |
| `StepInterrupted` | Show error indicator |
| `TurnEnd` | Finalise current turn, flush Rich Live output |
| `SubagentEvent(...)` | Render nested subagent wire messages recursively |

Rich `Live` renders to the terminal using ANSI escape sequences via `console.print()`. There is no custom diff compositor — Rich handles its own incremental updates.

### 4.1 Approval flow in the shell UI

When `ApprovalRequest` arrives:

1. `_approval_panel.py` builds a Rich panel with tool name, description, display blocks (diffs, shell output)
2. An `ApprovalPromptDelegate` attaches to the `CustomPromptSession`, replacing the normal prompt with an approval prompt
3. User types `y`/`n`/`a` (approve/reject/approve-for-session) or presses Enter
4. The response is sent as `ApprovalResponse` back via `wire.soul_side.send(response)` — bridged through `RootWireHub` to `ApprovalRuntime`

---

## Phase 5 — `KimiSoul.run()` — Turn Entry

**File:** `src/kimi_cli/soul/kimisoul.py`

`KimiSoul.run()` is called as an `asyncio.Task` by `run_soul()`. It runs in the same event loop thread.

### 5.1 Turn setup

```python
created_approval_source = ApprovalSource(kind="foreground_turn", id=uuid.uuid4().hex)
set_current_approval_source(created_approval_source)     # ContextVar

await self._runtime.oauth.ensure_fresh(self._runtime)    # OAuth token refresh
set_session_id(self._runtime.session.id)                 # ContextVar for toolset
```

### 5.2 UserPromptSubmit hook

```python
hook_results = await self._hook_engine.trigger(
    "UserPromptSubmit",
    matcher_value=text_input_for_hook,
    input_data=events.user_prompt_submit(...),
)
for result in hook_results:
    if result.action == "block":
        wire_send(TurnBegin(...))
        wire_send(TextPart(text=result.reason or "Prompt blocked by hook."))
        wire_send(TurnEnd())
        return
```

Hooks are shell commands defined in `config.toml` under `[hooks]`. Each hook receives event data on stdin as JSON and can exit with code 0 (allow) or 2 (block).

### 5.3 Slash command vs turn routing

```python
wire_send(TurnBegin(user_input=user_input))

if command_call := parse_slash_command_call(text_input):
    command = self._find_slash_command(command_call.name)
    ret = command.func(self, command_call.args)
    if isinstance(ret, Awaitable):
        await ret
elif self._loop_control.max_ralph_iterations != 0:
    runner = FlowRunner.ralph_loop(user_message, ...)
    await runner.run(self, "")
else:
    await self._turn(user_message)
```

**Slash commands** registered in `soul/slash.py` (soul-level: `/compact`, `/btw`, `/plan`, `/afk`, `/new`, etc.) execute directly without going through the LLM.

**Ralph mode** (`max_ralph_iterations != 0`) wraps the turn in a `FlowRunner` that repeatedly re-prompts until the LLM outputs `<choice>STOP</choice>`.

**Normal turn** delegates to `_turn()`.

### 5.4 Stop hook

After the turn completes:

```python
stop_results = await self._hook_engine.trigger("Stop", ...)
for result in stop_results:
    if result.action == "block" and result.reason:
        await self._turn(Message(role="user", content=result.reason))
        break
```

The Stop hook can inject a follow-up turn (once, to prevent infinite loops).

### 5.5 Auto-title

If the session has no custom title, the first 50 characters of the user prompt are used as the title, persisted to `session_state.json` with a read-modify-write to avoid clobbering concurrent web changes.

---

## Phase 6 — `_turn()` and `_agent_loop()`

**File:** `src/kimi_cli/soul/kimisoul.py`

### 6.1 `_turn()`

```python
async def _turn(self, user_message: Message) -> TurnOutcome:
    self._current_turn_id = uuid.uuid4().hex
    await self._checkpoint()          # write checkpoint 0 to context.jsonl
    await self._context.append_message(user_message)
    return await self._agent_loop()
```

`Context.checkpoint()` writes a checkpoint marker into `context.jsonl`. This is used by D-Mail rewinding (see Section 6.5).

### 6.2 `_agent_loop()` — step iteration

```
1. Turn Initialization
   a. Drain stale steers from previous turn
   b. Await deferred MCP loading (emit MCPLoadingBegin/End Wire events)

2. Step Loop (while True):
   a. Step Guard     — raise MaxStepsReached if step_no > max
   b. StepBegin wire event
   c. Context Compaction — if tokens ≥ trigger threshold
   d. Checkpoint     — persist before LLM call
   e. Step Execution — await self._step()
   f. Error Handling — BackToTheFuture or fatal exception
   g. Outcome Resolution — steers / stop / continue

3. Turn Resolution — return TurnOutcome
```

### 6.3 `_step()` — LLM call and tool execution

#### 6.3.1 Notification delivery (root soul only)

```python
await self._runtime.notifications.deliver_pending(
    "llm", limit=4,
    before_claim=self._runtime.background_tasks.reconcile,
    on_notification=_append_notification,
)
```

Up to 4 background task completion notifications are injected as system-reminder user messages before the LLM call.

#### 6.3.2 Dynamic injection

```python
injections = await self._collect_injections()
```

Registered `DynamicInjectionProvider`s are polled each step:
- `PlanModeInjectionProvider` — reminds the LLM it is in read-only plan mode
- `AfkModeInjectionProvider` — reminds the LLM no user is present

Injections are combined into one `system_reminder` user message appended to history.

#### 6.3.3 History normalization

```python
effective_history = normalize_history(self._context.history)
```

Adjacent user messages are merged (e.g. main user message + injection reminder).

#### 6.3.4 LLM call via `kosong.step()`

```python
result = await _kosong_step_with_retry()   # tenacity retry wrapper
```

Inside `_run_step_once()`:

```python
self._agent.toolset.begin_step(self._last_tool_calls, step_no=..., turn_id=...)
return await kosong.step(
    chat_provider,
    self._agent.system_prompt,
    self._agent.toolset,
    effective_history,
    on_message_part=wire_send,    # TextPart/ThinkPart tokens go to Wire immediately
    on_tool_result=wire_send,     # ToolResult goes to Wire when tool finishes
)
```

Retry policy (tenacity):
- Retryable: `APIConnectionError`, `APITimeoutError`, `APIEmptyResponseError`, HTTP 429/500/502/503/504
- Wait: exponential jitter (initial=0.3s, max=5s)
- Max attempts: `config.loop_control.max_retries_per_step`
- On retry: emit `StepRetry` Wire event

Connection recovery (`_run_with_connection_recovery()`):
- On `APIStatusError(401)` with OAuth provider: force token refresh, retry once
- On `APIConnectionError`/`APITimeoutError` with `RetryableChatProvider`: call `chat_provider.on_retryable_error()` (rebuilds the `AsyncOpenAI` client), retry once

#### 6.3.5 Token usage and StatusUpdate

```python
await self._context.update_token_count(usage.input)
wire_send(StatusUpdate(token_usage=usage, context_usage=..., context_tokens=..., ...))
```

#### 6.3.6 Tool results

```python
results = await result.tool_results()
```

`StepResult.tool_results()` in `kosong/__init__.py` awaits all `ToolResultFuture`s in tool-call order. Each future was created and resolved by `KimiToolset.handle()` (see Phase 7).

#### 6.3.7 Context growth

```python
await asyncio.shield(self._grow_context(result, results))
```

`asyncio.shield` prevents `CancelledError` from corrupting the context during a mid-step cancel. Appends the assistant message and tool result messages to `context.jsonl`.

#### 6.3.8 Outcome resolution

```python
# Pure rejection (no user feedback, root soul) → stop turn
if rejected_errors and not any(e.has_feedback for e in rejected_errors) and self.is_root:
    return StepOutcome(stop_reason="tool_rejected", ...)

# D-Mail check
if dmail := self._denwa_renji.fetch_pending_dmail():
    raise BackToTheFuture(dmail.checkpoint_id, [dmail_message])

# More tool calls → return None (continue loop)
if result.tool_calls:
    return None

# No tool calls → turn is done
return StepOutcome(stop_reason="no_tool_calls", ...)
```

---

## Phase 7 — `kosong.step()` → `generate()` → HTTP

**File:** `packages/kosong/src/kosong/__init__.py`, `packages/kosong/src/kosong/chat_provider/kimi.py`

### 7.1 `kosong.step()`

```python
async def step(chat_provider, system_prompt, toolset, history, *, on_message_part, on_tool_result):
    async def on_tool_call(tool_call: ToolCall):
        tool_calls.append(tool_call)
        result = toolset.handle(tool_call)           # fires immediately (sync dispatch)
        # result is ToolResult (immediate) or asyncio.Task[ToolResult] (deferred)
        tool_result_futures[tool_call.id] = result

    result = await generate(
        chat_provider, system_prompt, toolset.tools, history,
        on_message_part=on_message_part,
        on_tool_call=on_tool_call,
    )
    return StepResult(result.id, result.message, result.usage, tool_calls, tool_result_futures)
```

Tool calls are dispatched **as soon as they arrive in the stream** — the model does not need to finish generating before tools start executing.

### 7.2 `generate()` → `ChatProvider.generate()`

```python
result = await generate(chat_provider, ...)
```

`generate()` in `kosong/_generate.py` calls `chat_provider.generate(system_prompt, tools, history)` and iterates the `StreamedMessage` async iterator, calling `on_message_part` for each `TextPart`, `ThinkPart`, `ToolCallPart`, and `on_tool_call` for each complete `ToolCall`.

### 7.3 `Kimi.generate()` — HTTP POST

```python
response = await self.client.chat.completions.create(
    model=self.model,
    messages=messages,            # system + history converted to OpenAI format
    tools=list(_convert_tool(t) for t in tools),
    stream=True,
    stream_options={"include_usage": True},
    max_tokens=32000,
    **self._generation_kwargs,    # temperature, top_p, reasoning_effort, prompt_cache_key, etc.
)
return KimiStreamedMessage(response)
```

The `AsyncOpenAI` client sends the HTTP POST to `https://api.moonshot.ai/v1/chat/completions` with an `Authorization: Bearer <api_key>` header and `Content-Type: application/json`. The response is an SSE stream that `KimiStreamedMessage` wraps as an `AsyncIterator[StreamedMessagePart]`.

**Other providers** follow the same pattern with their respective client libraries (anthropic SDK, Google GenAI SDK, etc.).

---

## Phase 8 — `KimiToolset.handle()` — Tool Dispatch

**File:** `src/kimi_cli/soul/toolset.py`

`toolset.handle(tool_call)` is called synchronously by `kosong.step()` when each tool call arrives in the stream. It returns an `asyncio.Task` that resolves to `ToolResult`.

### 8.1 Deduplication

```python
call_key = (tool_call.function.name, tool_call.function.arguments or "{}")

# Same-step dedup: same (name, args) within this step → wait for original task
if call_key in self._current_step_tasks:
    return asyncio.create_task(_await_dup())

# Cross-step dedup: same (name, args) as last step → append reminder text to result
is_cross_step_dup = call_key in self._previous_step_calls
```

### 8.2 Tool lookup and argument parsing

```python
tool = self._tool_dict[tool_call.function.name]
arguments = json.loads(tool_call.function.arguments or "{}", strict=False)
```

### 8.3 Async task creation

```python
async def _call():
    # PreToolUse hook
    results = await self._hook_engine.trigger("PreToolUse", ...)
    for result in results:
        if result.action == "block":
            return ToolResult(tool_call_id=..., return_value=ToolError(...))

    # Execute tool
    ret = await tool.call(arguments)

    # PostToolUse hook (fire-and-forget)
    asyncio.create_task(self._hook_engine.trigger("PostToolUse", ...))

    return ToolResult(tool_call_id=..., return_value=ret)

task = asyncio.create_task(_call())
```

Multiple tool calls in one step execute **concurrently** as independent asyncio tasks.

---

## Phase 9 — Tool Execution: Approval and Built-in Tools

**Files:** `src/kimi_cli/soul/approval.py`, `src/kimi_cli/tools/`

### 9.1 Approval gating

Every tool that needs user permission calls:

```python
result = await self._runtime.approval.request(sender, action, description, display=[...])
if not result:
    return result.rejection_error()
```

`Approval.request()` logic:

```
is_auto_approve() (yolo or afk)  →  ApprovalResult(approved=True)
action in auto_approve_actions    →  ApprovalResult(approved=True)
else:
    self._runtime.create_request(request_id, ...)
    # → ApprovalRuntime.create_request()
    # → RootWireHub.publish(ApprovalRequest)
    # → Wire subscribers receive ApprovalRequest
    # → Shell UI shows approval panel
    # → User responds → wire.soul_side.send(ApprovalResponse)
    # → ApprovalRuntime.resolve(request_id, response)
    # → approval future resolves
```

`ApprovalRuntime` in `approval_runtime/runtime.py` is the session-level store of pending approval requests. `RootWireHub` in `wire/root_hub.py` is a broadcast channel that fans the approval request out to all active Wire subscriptions (shell UI, ACP, Web, etc.).

### 9.2 Built-in tools

| Tool class | Module | Key permission action |
|---|---|---|
| `Shell` | `tools/shell/__init__.py` | `"shell:bash"` |
| `ReadFile` | `tools/file/read.py` | auto-approved (read-only) |
| `WriteFile` | `tools/file/write.py` | `"file:write:<path>"` |
| `StrReplaceFile` | `tools/file/replace.py` | `"file:replace:<path>"` |
| `Glob` | `tools/file/glob.py` | auto-approved |
| `Grep` | `tools/file/grep_local.py` | auto-approved |
| `WebFetch` | `tools/web/fetch.py` | `"web:fetch:<url>"` |
| `WebSearch` | `tools/web/search.py` | `"web:search"` |
| `AskUserQuestion` | `tools/ask_user/__init__.py` | pauses for user answer |
| `Think` | `tools/think/__init__.py` | auto-approved |
| `SetTodoList` | `tools/todo/__init__.py` | auto-approved |
| `Agent` | `tools/agent/__init__.py` | spawns subagent |
| `EnterPlanMode` / `ExitPlanMode` | `tools/plan/` | auto-approved |
| `SendDMail` | `tools/dmail/__init__.py` | triggers BackToTheFuture rewind |

### 9.3 MCP tools

`MCPTool.__call__()` in `soul/toolset.py`:

```python
result = await self._runtime.approval.request(...)   # gated like built-in tools
async with self._client as client:
    result = await client.call_tool(
        self._mcp_tool.name, kwargs,
        timeout=self._timeout, raise_on_error=False
    )
    return convert_mcp_tool_result(result)
```

MCP tools connect via `fastmcp.Client` which handles stdio, SSE, and OAuth transports.

### 9.4 Wire external tools (ACP)

`WireExternalTool.__call__()` sends a `ToolCallRequest` over the Wire and `await`s the client's `ToolCallResponse`. This lets ACP/IDE clients implement their own tools.

---

## Phase 10 — Subagents

**Files:** `src/kimi_cli/tools/agent/__init__.py`, `src/kimi_cli/subagents/`

The `Agent` tool creates or resumes a subagent:

1. `LaborMarket` looks up the registered `AgentTypeDefinition` by type name
2. `SubagentStore` persists instance metadata, prompts, and context under `session/subagents/<agent_id>/`
3. A new `Runtime` is created via `runtime.copy_for_subagent(agent_id=..., subagent_type=...)` — sharing OAuth, approval, labor market, and notifications but with isolated `DenwaRenji` and adjusted background task visibility
4. A new `KimiSoul` is instantiated with the subagent's `Agent`
5. `run_soul(subagent_soul, task_prompt, subagent_ui_loop_fn, ...)` is called

The subagent's `Wire` messages are wrapped in `SubagentEvent(parent_tool_call_id=..., event=<event>)` and forwarded to the parent Wire so the shell UI renders them nested under the parent tool call block.

---

## Phase 11 — Context Compaction

**File:** `src/kimi_cli/soul/compaction.py`, invoked from `kimisoul.py`

Compaction triggers when:

```python
should_auto_compact(token_count, max_context_size, trigger_ratio=..., reserved_context_size=...)
# True if: token_count >= max_context_size * trigger_ratio
#       OR: token_count + reserved_context_size >= max_context_size
```

`KimiSoul.compact_context()`:

1. Fires `PreCompact` hook
2. Emits `CompactionBegin` Wire event
3. Calls `SimpleCompaction.compact(context.history, llm)`:
   - Sends full conversation history to the LLM with a compaction system prompt
   - LLM returns a concise summary message
   - Returns `CompactionResult(messages=[summary_msg], usage=...)`
4. Clears `context.jsonl`, writes new system prompt + checkpoint, appends summary message
5. If root: appends active background task snapshot message
6. Emits `CompactionEnd` Wire event, fires `PostCompact` hook
7. Notifies injection providers via `on_context_compacted()` so they reset one-shot throttles

---

## Phase 12 — D-Mail / BackToTheFuture

**File:** `src/kimi_cli/soul/denwarenji.py`, `src/kimi_cli/tools/dmail/__init__.py`

`SendDMail` is a special tool that rewrites history. When called:

1. `DenwaRenji.set_pending_dmail(checkpoint_id, message)` stores a rewind request
2. After `_step()` completes tool execution, `_step()` calls `self._denwa_renji.fetch_pending_dmail()`
3. If a D-Mail is pending, `_step()` raises `BackToTheFuture(checkpoint_id, messages)`
4. `_agent_loop()` catches `BackToTheFuture`:
   - `await self._context.revert_to(checkpoint_id)` truncates `context.jsonl` back to the checkpoint
   - Appends the D-Mail message as if it were a new user message
   - Loops — the LLM sees the rewritten history from the checkpoint

This enables "time-travel" debugging: the agent can discover a mistake, send a message to its past self, and re-run from the checkpoint with corrected context.

---

## Phase 13 — ACP / Wire / Print UI Modes

### 13.1 ACP server (`kimi acp`)

**File:** `src/kimi_cli/acp/server.py`

Runs a FastAPI server implementing the Agent Client Protocol (ACP). IDE clients connect over WebSocket. Each client message creates a new Wire and calls `run_soul()`, then streams `WireMessage` objects as ACP events.

### 13.2 Wire server (`kimi --wire`)

**File:** `src/kimi_cli/wire/server.py`

Reads `WireMessageEnvelope` JSON lines from stdin, writes them to stdout. Lets external tools drive the agent programmatically over pipes.

### 13.3 Print mode (`kimi --print`)

**File:** `src/kimi_cli/ui/print/__init__.py`

Non-interactive mode. Reads the prompt from stdin (or `--prompt`), calls `KimiCLI.run()` (the generator interface), and prints Wire messages to stdout as plain text or structured JSON (`--output-format stream-json`). Auto-approves all tool calls (`runtime_afk=True`). Background tasks have a configurable wait ceiling before being killed.

### 13.4 Web UI (`kimi web`, `SwitchToWeb`)

**File:** `src/kimi_cli/web/`

A FastAPI + uvicorn server with a bundled React web UI (served from `deps/`). Web sessions run in separate subprocess workers (`__web-worker`) that pipe Wire messages back over JSON-RPC.

---

## Phase 14 — Hook Engine

**File:** `src/kimi_cli/hooks/engine.py`, `src/kimi_cli/hooks/runner.py`

Hooks are shell commands configured in `config.toml` under `[hooks.<event>]`. The `HookEngine.trigger(event, matcher_value, input_data)` API:

1. Finds matching hook rules (event type + optional glob match on `matcher_value`)
2. Runs matched hooks in parallel via `asyncio.create_subprocess_shell`
3. Passes `input_data` as JSON on stdin
4. Reads hook exit code: 0 → allow, 2 → block (with reason from stdout)
5. Returns `list[HookResult]`

Hook events available:

| Event | Trigger point |
|---|---|
| `UserPromptSubmit` | Before turn starts |
| `PreToolUse` | Before each tool call |
| `PostToolUse` | After successful tool call |
| `PostToolUseFailure` | After failed tool call |
| `Stop` | After turn ends normally |
| `StopFailure` | After turn fails with exception |
| `PreCompact` | Before compaction |
| `PostCompact` | After compaction |
| `Notification` | When LLM-sink notification delivered |
| `SessionStart` | On session begin |
| `SessionEnd` | On session end |

Wire clients can also implement hooks by responding to `HookRequest` messages with `HookResponse`.

---

## Summary: Complete Data Flow

```
Terminal keypress (prompt_toolkit)
  │
  ▼
Shell._route_prompt_events()
  │  UserInput event → idle_events queue
  ▼
Shell.run() main loop
  │  classify_input() → agent or shell mode
  ▼
Shell.run_soul_command(text)
  │
  ▼
run_soul(soul, text, ui_loop_fn, cancel_event)
  │  creates Wire, sets ContextVar
  │  spawns soul_task + ui_task + notification_task
  ▼
KimiSoul.run(text)
  │  UserPromptSubmit hook check
  │  wire_send(TurnBegin)
  │  slash command OR _turn()
  ▼
KimiSoul._turn()
  │  Context.checkpoint() + append_message()
  ▼
KimiSoul._agent_loop()          ◄──────────── loop until no_tool_calls or tool_rejected
  │  MCP loading wait
  │  wire_send(StepBegin)
  │  should_auto_compact? → compact_context() → LLM summarise → Context.clear/rewrite
  │  Context.checkpoint()
  ▼
KimiSoul._step()
  │  deliver notifications (root)
  │  dynamic injection (plan/afk reminders)
  │  normalize_history()
  │  KimiToolset.begin_step()
  ▼
kosong.step(chat_provider, system_prompt, toolset, history, on_message_part=wire_send)
  │  generate() → ChatProvider.generate()
  ▼
Kimi.generate()
  │  AsyncOpenAI → HTTPS POST /v1/chat/completions
  │  SSE stream → KimiStreamedMessage async iterator
  ▼
  ╔═══════════════════════════════════════════════════════╗
  ║  Per streamed chunk:                                  ║
  ║    TextPart / ThinkPart → wire_send() → UI renders   ║
  ║    ToolCallPart → accumulate                          ║
  ║    ToolCall (complete) → toolset.handle() → Task      ║
  ╚═══════════════════════════════════════════════════════╝
  │
  ▼  (after generate() returns full message)
StepResult.tool_results()         ← await all ToolResultFutures
  │
  ├─ KimiToolset.handle(tool_call)
  │    │  dedup check
  │    │  PreToolUse hook
  │    │  Approval.request() ──► ApprovalRuntime ──► RootWireHub
  │    │                             ▲                    │
  │    │                             │              ApprovalRequest Wire event
  │    │                             │                    │
  │    │                         Shell UI shows panel ◄───┘
  │    │                         User y/n → ApprovalResponse
  │    │                             │
  │    │                         approval future resolves
  │    │
  │    ├─ Built-in tool: Shell/ReadFile/WriteFile/WebFetch/...
  │    ├─ MCPTool: fastmcp client.call_tool()
  │    ├─ WireExternalTool: ToolCallRequest → Wire client
  │    └─ ToolResult → wire_send() (PostToolUse hook fire-and-forget)
  │
  ▼
_grow_context(result, tool_results)   context.jsonl updated
  │
  ▼
Outcome check:
  rejected?      → StepOutcome(tool_rejected) → TurnOutcome → wire_send(TurnEnd)
  D-Mail?        → BackToTheFuture → context.revert_to() → loop
  more tools?    → return None → loop
  no tools?      → StepOutcome(no_tool_calls) → TurnOutcome → wire_send(TurnEnd)
```

---

## Key Invariants

| Invariant | Where enforced |
|---|---|
| `wire_send()` only valid while soul is running | `ContextVar` assertion in `wire_send()` |
| Tool approval always from a tool call context | `Approval.request()` asserts `current_tool_call` |
| Context mutations protected from mid-step cancel | `asyncio.shield(_grow_context(...))` |
| Same-step tool call dedup: one task, results shared | `KimiToolset._current_step_tasks` |
| Cross-step dedup: reminder injected in result | `_append_reminder_to_return_value()` |
| D-Mail checkpoint_id always valid | `DenwaRenji` guarantees `0 ≤ id < n_checkpoints` |
| Session state persisted before LLM call | `Context.checkpoint()` before each `_step()` |
| UTF-8 safety in file tools | `kaos.path.KaosPath` wraps all file I/O |
| Wire messages serialisable | All `WireMessage` types are Pydantic `BaseModel` |
| MCP tool output capped | `MCP_MAX_OUTPUT_CHARS = 100_000` in `convert_mcp_tool_result()` |

---

## Comparison Points with smelt

| Dimension | kimi-cli | smelt |
|---|---|---|
| Language | Python 3.12+ | Rust |
| Concurrency | `asyncio` single event loop | Two-thread: TUI main + `tokio::spawn` engine task |
| IPC | `Wire` asyncio queue (in-process) | `mpsc::unbounded_channel` between threads |
| LLM abstraction | `kosong` package (`ChatProvider` protocol) | Direct HTTP (`reqwest`) in `engine::provider` |
| Streaming | `AsyncOpenAI` SSE via `httpx` | `reqwest` SSE, hand-parsed per provider |
| Tool dispatch | Concurrent `asyncio.Task`s, awaited in call order | `FuturesUnordered` + sequential queue |
| Scripting | Python plugins / YAML agent specs | Lua 5.4 (mlua), keymaps, hooks |
| UI rendering | Rich `Live` (block-based, terminal-native) | Custom diff compositor (`smelt-term`) |
| Approval flow | `ApprovalRuntime` + `RootWireHub` broadcast | `host_rx` synchronous RPC channel |
| Compaction | `SimpleCompaction` (single LLM summarise call) | smelt also uses a secondary LLM call |
| Subagents | `LaborMarket`, `SubagentStore`, nested Wire wrapping | Not present in smelt |
| Session persistence | JSONL wire log + `context.jsonl` + `state.json` | No persistent session (in-memory) |
| Multi-provider | 7 provider types via `create_llm()` | 6 provider kinds via `ProviderKind` |
| D-Mail / rewind | `DenwaRenji` + `BackToTheFuture` context revert | Not present in smelt |
