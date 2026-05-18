# Kimi Code CLI Architectural Analysis: Keystroke to LLM Request to Rendered Output

> **Scope:** This document exhaustively maps every code path that transforms a user keystroke into an LLM API request and back to rendered output in the kimi-cli codebase. It covers `kimi-cli/src/kimi_cli/`, `kimi-cli/packages/kosong/`, and the wire protocol.
>
> **Generated:** 2026-05-18 by Kimi Code CLI via multi-agent codebase exploration.

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Entry Point and Bootstrap Sequence](#2-entry-point-and-bootstrap-sequence)
3. [CLI Layer and UI Mode Dispatch](#3-cli-layer-and-ui-mode-dispatch)
4. [Shell UI: Input Capture and the Prompt Loop](#4-shell-ui-input-capture-and-the-prompt-loop)
5. [The Soul: Agent Core and Turn Lifecycle](#5-the-soul-agent-core-and-turn-lifecycle)
6. [Kosong Layer: LLM Abstraction and Streaming](#6-kosong-layer-llm-abstraction-and-streaming)
7. [Provider Layer: HTTP Request Building](#7-provider-layer-http-request-building)
8. [Response Streaming: From Provider to Rendered Output](#8-response-streaming-from-provider-to-rendered-output)
9. [Tool Call Lifecycle](#9-tool-call-lifecycle)
10. [Wire Protocol and Approval System](#10-wire-protocol-and-approval-system)
11. [Tool System and Runtime](#11-tool-system-and-runtime)
12. [Alternative UI Modes](#12-alternative-ui-modes)
13. [Complete Data Flow Summary](#13-complete-data-flow-summary)

---

## 1. Executive Summary

Kimi Code CLI is a **Python-based coding agent** with a layered architecture:

| Layer | Package/Module | Responsibility |
|-------|---------------|---------------|
| **CLI** | `kimi_cli.cli` | Typer-based command parsing, UI mode dispatch |
| **UI** | `kimi_cli.ui.shell` | Interactive terminal UI (prompt-toolkit + Rich) |
| **Soul** | `kimi_cli.soul` | Agent orchestration, turn lifecycle, context management |
| **Kosong** | `kosong` | LLM abstraction — unified message structures, tool dispatch, provider plugins |
| **Wire** | `kimi_cli.wire` | SPMC message bus between soul and UI; JSON-RPC server for IDE integration |
| **Tools** | `kimi_cli.tools` | Built-in tools (shell, file, web, think, etc.) |
| **Runtime** | `kimi_cli.soul.agent` | Shared services: config, session, approval, notifications, background tasks |

The high-level flow is:

```
User keystroke
    → prompt-toolkit CustomPromptSession
    → Shell input loop
    → run_soul_command() or slash dispatch
    → KimiSoul.run() → _turn() → _step()
    → kosong.step() → kosong.generate()
    → ChatProvider.generate() → HTTP streaming
    → StreamedMessagePart → on_message_part callback
    → Wire.soul_side.send() → Wire.ui_side.receive()
    → _PromptLiveView.visualize_loop()
    → Rich Live + prompt_toolkit FormattedText
    → ANSI escape sequences
```

Key architectural decisions:
- **Prompt-toolkit** for input handling (key bindings, completions, modal dialogs)
- **Rich** for output rendering (markdown, syntax highlighting, live display, panels)
- **Wire protocol** decouples soul from UI, enabling multiple frontends (shell, print, ACP, web, vis, wire/stdio)
- **Kosong** provides provider-agnostic LLM abstraction with unified streaming and tool calling
- **ApprovalRuntime** with `RootWireHub` broadcasts permission requests across all UI consumers

---

## 2. Entry Point and Bootstrap Sequence

### 2.1 Binary Entry Point

**File:** `kimi-cli/src/kimi_cli/__main__.py`  
**Function:** `main()` (line 12)

```python
def main(argv=None):
    install_crash_handlers()
    normalize_proxy_env()
    # ...
    from kimi_cli.cli import cli
    return cli(args=args, prog_name=_prog_name())
```

### 2.2 CLI Initialization

**File:** `kimi-cli/src/kimi_cli/cli/__init__.py`

Root `typer.Typer()` object with lazy-loaded subcommands via `LazySubcommandGroup` (`cli/_lazy_group.py`).

Lazy commands: `info`, `export`, `mcp`, `plugin`, `vis`, `web`  
Eager commands: `login`, `logout`

The root callback `kimi()` (lines 77–902) parses ~30 options (`--model`, `--yolo`, `--plan`, `--prompt`, `--print`, etc.), resolves conflicts, then enters `_reload_loop()`.

### 2.3 Reload Loop

**File:** `kimi-cli/src/kimi_cli/cli/__init__.py`  
**Function:** `_reload_loop()` / `_run()`

```python
match ui:
    case "shell":  await instance.run_shell(prompt, prefill_text=prefill_text)
    case "print":  await instance.run_print(...)
    case "acp":    await instance.run_acp()
    case "wire":   await instance.run_wire_stdio()
```

The reload loop catches `Reload`, `SwitchToWeb`, `SwitchToVis` exceptions to restart the CLI with new configuration.

### 2.4 KimiCLI Construction

**File:** `kimi-cli/src/kimi_cli/app.py`  
**Class:** `KimiCLI`  
**Factory:** `KimiCLI.create()` (line 121)

Creation sequence:
1. **Load config** (`load_config()`) — TOML/JSON config file
2. **Resolve model** — CLI `--model` > config default > fallback empty model
3. **OAuth setup** (`OAuthManager`) — device ID, token refresh
4. **Create LLM** (`create_llm()`) — builds `kosong.chat_provider` instance per provider type
5. **Create Runtime** (`Runtime.create()`) — session, approval, skills, notifications, background tasks, environment detection
6. **Load agent** (`load_agent()`) — parses YAML agent spec, loads system prompt via Jinja2, instantiates toolset
7. **Create KimiSoul** — pairs Agent + Context (conversation history manager)
8. **Initialize telemetry** — async event sink with device/session tracking

---

## 3. CLI Layer and UI Mode Dispatch

### 3.1 UI Modes

**File:** `kimi-cli/src/kimi_cli/app.py`

| Method | Mode | Description |
|--------|------|-------------|
| `run_shell()` | `shell` | Interactive terminal with prompt-toolkit + Rich |
| `run_print()` | `print` | Headless: read input, run agent, print output, exit |
| `run_acp()` | `acp` | Agent Client Protocol server |
| `run_wire_stdio()` | `wire` | JSON-RPC server over stdio for IDE integration |

### 3.2 Shell Mode Entry

**File:** `kimi-cli/src/kimi_cli/app.py` (line 686)

```python
async def run_shell(self, command=None, *, prefill_text=None):
    from kimi_cli.ui.shell import Shell
    shell = Shell(self._soul, welcome_info=..., prefill_text=prefill_text)
    return await shell.run(command)
```

### 3.3 Print Mode Entry

**File:** `kimi-cli/src/kimi_cli/app.py` (line 772)

```python
async def run_print(self, input_format, output_format, command=None, *, final_only=False):
    from kimi_cli.ui.print import Print
    print_ = Print(self._soul, input_format, output_format, ...)
    return await print_.run(command)
```

---

## 4. Shell UI: Input Capture and the Prompt Loop

### 4.1 Shell Class

**File:** `kimi-cli/src/kimi_cli/ui/shell/__init__.py`  
**Class:** `Shell` (line 183)

`Shell.run()` is the main entry point for interactive mode.

### 4.2 Prompt Session

**File:** `kimi-cli/src/kimi_cli/ui/shell/prompt.py`  
**Class:** `CustomPromptSession` (line 1180)

Built on **prompt-toolkit** (`PromptSession[str]`). Wraps a `Buffer` with:
- Custom key bindings
- Slash command completer
- File mention completer (`@path`)
- Bottom toolbar
- Modal delegates (for approval dialogs, questions, etc.)

### 4.3 Key Bindings

**File:** `kimi-cli/src/kimi_cli/ui/shell/prompt.py` (lines 1250–1505)

| Key | Action |
|-----|--------|
| `Ctrl-X` | Toggle AGENT ↔ SHELL mode |
| `Shift-Tab` | Toggle plan mode |
| `Ctrl-O` | Open external editor |
| `Ctrl-J` / `Alt-Enter` | Insert newline |
| `Ctrl-V` | Paste (supports images via placeholder manager) |
| `Ctrl-C` | Interrupt (during run) or cancel |
| `Ctrl-D` | Exit |
| `Enter` | Submit; if slash-completion open, accept + submit |

### 4.4 Input Modes

- `PromptMode.AGENT` — sends input to the LLM agent
- `PromptMode.SHELL` — runs local shell commands (`cd`, `ls`, etc.)

### 4.5 UserInput Model

**File:** `kimi-cli/src/kimi_cli/ui/shell/prompt.py` (line 865)

```python
class UserInput(BaseModel):
    mode: PromptMode
    command: str           # raw text
    resolved_command: str  # after placeholder expansion
    content: list[ContentPart]
```

### 4.6 Main Input Loop

**File:** `kimi-cli/src/kimi_cli/ui/shell/__init__.py` (lines 381–713)

```
Shell.run()
  │
  ├── prints welcome info
  ├── creates CustomPromptSession
  │
  └── while True:
        │
        ├── _route_prompt_events()  (prompt_toolkit → idle_events queue)
        │     └── await prompt_session.prompt_next()
        │
        ├── _BackgroundCompletionWatcher.wait_for_next()
        │     └── waits for: user input OR background task completion
        │
        └── processes event:
              ├── "input" → echo → check exit → check slash → run_soul_command()
              ├── "bg_noop" → ignore
              ├── "interrupt" → print tip
              ├── "eof" → break / print "Bye!"
              └── "cwd_lost" → crash report
```

### 4.7 Slash Command Dispatch

**File:** `kimi-cli/src/kimi_cli/ui/shell/slash.py`

Two registries:
```python
registry = SlashCommandRegistry[ShellSlashCmdFunc]()           # agent mode
shell_mode_registry = SlashCommandRegistry[ShellSlashCmdFunc]() # shell mode
```

Unified command index (`Shell.__init__`):
1. Soul-level slash commands from `soul.available_slash_commands`
2. Shell-level slash commands from `shell_slash_registry.list_commands()`

Dispatch flow (`Shell._run_slash_command`, lines 775–817):
1. Look up command in unified index
2. If soul-level command → `run_soul_command()`
3. If shell-level command → `command.func(self, command_call.args)`
4. Catches `Reload`, `SwitchToWeb`, `SwitchToVis` exceptions

Notable shell-level commands:
| Command | Effect |
|---------|--------|
| `/exit`, `/quit` | Exit |
| `/help` | Show help |
| `/model` | Interactive model switcher (raises `Reload`) |
| `/theme` | Toggle dark/light (raises `Reload`) |
| `/new` | New session (raises `Reload`) |
| `/sessions`, `/resume` | Session picker (raises `Reload`) |
| `/undo` | Fork at previous turn (raises `Reload`) |
| `/btw` | Side question modal |
| `/task` | Background task browser |
| `/web`, `/vis` | Switch UI mode |

### 4.8 Output Rendering

**Console singleton:** `kimi-cli/src/kimi_cli/ui/shell/console.py` — `console = _KimiConsole(...)` with neutral markdown theme. Provides `render_to_ansi()` for prompt_toolkit integration.

**Two rendering layers:**

#### Layer 1: Agent streaming output (`_LiveView`)
**File:** `kimi-cli/src/kimi_cli/ui/shell/visualize/_live_view.py`

Uses `rich.live.Live` (`refresh_per_second=10`, `transient=True`) for the bottom dynamic area.

Compose pipeline (line 395):
1. `compose_interactive_panels()` — approval/question panels
2. `compose_agent_output()` — spinners, content blocks, tool calls, notifications
3. `_StatusBlock.render()` — context usage / token count

#### Layer 2: Interactive prompt area (`_PromptLiveView`)
**File:** `kimi-cli/src/kimi_cli/ui/shell/visualize/_interactive.py`

Renders into the prompt_toolkit layout via `CustomPromptSession._render_agent_prompt_message()`. Agent status rendered as ANSI and injected into `FormattedText` layout.

**Renderable blocks** (`_blocks.py`):
- `_ContentBlock` — streaming markdown with incremental commitment (committed blocks flushed to terminal history, only tail stays in Live)
- `_ToolCallBlock` — tool call spinner + result rendering (diff panels, todo lists, brief display)
- `_NotificationBlock` — toast-like notifications
- `_StatusBlock` — context/token status line

---

## 5. The Soul: Agent Core and Turn Lifecycle

### 5.1 KimiSoul

**File:** `kimi-cli/src/kimi_cli/soul/kimisoul.py`  
**Class:** `KimiSoul` (lines 158–530)

Encapsulates the entire agent lifecycle for one session.

**Key dependencies:**
- `Agent` — loaded from YAML spec (system prompt, toolset, runtime)
- `Context` — conversation history manager (`soul/context.py`)
- `Runtime` — shared services (session, config, LLM, approval, notifications, skills, etc.)

### 5.2 run_soul Orchestrator

**File:** `kimi-cli/src/kimi_cli/soul/__init__.py`  
**Function:** `run_soul()` (lines 179–254)

Wires a `Soul` instance to the UI layer. Manages concurrency between soul, UI, cancellation, and notifications.

```python
async def run_soul(
    soul: Soul,
    user_input: str | list[ContentPart],
    ui_loop_fn: UILoopFn,
    cancel_event: asyncio.Event,
    ...
):
```

What it does:
1. Creates a `Wire` (async message bus)
2. Starts **UI loop task** (`ui_loop_fn(wire)`) — renders output
3. Starts **soul run task** (`soul.run(user_input)`) — runs agent logic
4. Starts **notification pump task** — pushes pending notifications every second
5. Waits for soul task or cancellation
6. On cancellation: cancels soul task, raises `RunCancelled`
7. On completion: shuts down wire, drains UI loop

### 5.3 KimiSoul.run()

**File:** `kimi-cli/src/kimi_cli/soul/kimisoul.py` (lines 577–716)

User input processing flow:
1. **Approval source setup** — creates foreground approval source for this turn
2. **OAuth refresh** — ensures tokens are fresh
3. **UserPromptSubmit hook** — user-configurable hooks can block the prompt
4. **Slash command detection** — parses `/command`; executes registered command instead of LLM
5. **Flow/ralph loop** — if `max_ralph_iterations` configured, wraps input in self-repeating decision flow
6. **Normal turn** — calls `self._turn(user_message)`
7. **Stop hook** — after turn finishes, checks if `Stop` hook wants to re-trigger

### 5.4 Turn Lifecycle

**File:** `kimi-cli/src/kimi_cli/soul/kimisoul.py`

```python
_turn(user_message)
  │
  ├── append user message to context
  ├── enter _agent_loop()
  │
  └── _agent_loop()
        │
        └── while not done:
              ├── _step() → kosong.step()
              ├── await tool_results
              ├── _grow_context() → append assistant + tool messages
              ├── check max_steps_per_turn
              └── check auto-compaction
```

### 5.5 Step Lifecycle

**File:** `kimi-cli/src/kimi_cli/soul/kimisoul.py`  
**Function:** `_step()` (lines 1001–1220)

1. **Notification delivery** (root only) — pending notifications appended as user messages
2. **Dynamic injection** — collects injections (plan-mode reminders, AFK prompts) as `<system-reminder>` user messages
3. **History normalization** — merges adjacent user messages
4. **Toolset begin_step** — resets per-step deduplication state
5. **LLM call** — `kosong.step(...)` with:
   - `chat_provider`
   - `system_prompt` (rendered from agent spec via Jinja2)
   - `toolset` (available tools)
   - `effective_history`
   - `on_message_part=wire_send`
   - `on_tool_result=wire_send`
6. **Wait for tool results**
7. **Grow context** — append assistant message + tool result messages

### 5.6 System Prompt Construction

**File:** `kimi-cli/src/kimi_cli/soul/agent.py` (lines 494–519)

Loaded from a file (e.g., `./system.md`) and rendered through **Jinja2** with builtins:
- `KIMI_NOW` — current ISO datetime
- `KIMI_WORK_DIR` — working directory
- `KIMI_WORK_DIR_LS` — directory listing
- `KIMI_AGENTS_MD` — merged `AGENTS.md` content
- `KIMI_SKILLS` — formatted skill descriptions
- `KIMI_ADDITIONAL_DIRS_INFO` — extra workspace directories
- `KIMI_OS`, `KIMI_SHELL`

### 5.7 Agent Specification (YAML)

**File:** `kimi-cli/src/kimi_cli/agentspec.py`  
**Examples:** `kimi-cli/src/kimi_cli/agents/default/agent.yaml`

```yaml
version: 1
agent:
  name: ""
  extend: ...
  system_prompt_path: ./system.md
  system_prompt_args:
    KEY: "value"
  model: "alias"
  when_to_use: "..."
  tools:
    - "kimi_cli.tools.shell:Shell"
  allowed_tools: []
  exclude_tools: []
  subagents:
    coder:
      path: ./coder.yaml
      description: "Good at SE tasks."
```

Inheritance (`extend`): fields merged — `system_prompt_args` merged, others overridden.

---

## 6. Kosong Layer: LLM Abstraction and Streaming

### 6.1 Kosong Overview

**Package:** `kimi-cli/packages/kosong/`

Kosong is the LLM abstraction layer. It unifies message structures, async tool orchestration, and pluggable chat providers.

Key exports:
- `kosong.generate()` — creates completion stream, merges streamed parts
- `kosong.step()` — layers tool dispatch over `generate`
- `kosong.message` — `Message`, `TextPart`, `ThinkPart`, `ToolCall`, etc.
- `kosong.tooling` — `Tool`, `Toolset`, `CallableTool2`, `ToolResult`
- `kosong.chat_provider` — `ChatProvider` protocol + implementations

### 6.2 generate()

**File:** `kimi-cli/packages/kosong/src/kosong/_generate.py`  
**Function:** `generate()` (lines 17–95)

```python
async def generate(chat_provider, system_prompt, tools, history, *, on_message_part=None, on_tool_call=None):
    message = Message(role="assistant", content=[])
    pending_part = None

    stream = await chat_provider.generate(system_prompt, tools, history)
    async for part in stream:
        if on_message_part:
            await callback(on_message_part, part.model_copy(deep=True))

        if pending_part is None:
            pending_part = part
        elif not pending_part.merge_in_place(part):
            _message_append(message, pending_part)
            if isinstance(pending_part, ToolCall) and on_tool_call:
                await callback(on_tool_call, pending_part)
            pending_part = part

    # end of message
    if pending_part is not None:
        _message_append(message, pending_part)
        if isinstance(pending_part, ToolCall) and on_tool_call:
            await callback(on_tool_call, pending_part)

    return GenerateResult(id=stream.id, message=message, usage=stream.usage)
```

**Key behaviors:**
- `merge_in_place()` coalesces adjacent text deltas into complete `TextPart`s
- When a `ToolCall` is fully assembled, `on_tool_call` fires — spawns tool execution **concurrently** while stream may continue
- After stream ends, returns `GenerateResult` with complete `Message` + `TokenUsage`

### 6.3 step()

**File:** `kimi-cli/packages/kosong/src/kosong/__init__.py`  
**Function:** `step()` (lines 104–180)

```python
async def step(chat_provider, system_prompt, toolset, history, *, on_message_part=None, on_tool_result=None):
    tool_calls = []
    tool_result_futures = {}

    async def on_tool_call(tool_call: ToolCall):
        tool_calls.append(tool_call)
        result = toolset.handle(tool_call)

        if isinstance(result, ToolResult):
            future = ToolResultFuture()
            future.set_result(result)
            tool_result_futures[tool_call.id] = future
        else:
            result.add_done_callback(future_done_callback)
            tool_result_futures[tool_call.id] = result

    result = await generate(
        chat_provider, system_prompt, toolset.tools, history,
        on_message_part=on_message_part,
        on_tool_call=on_tool_call,
    )

    return StepResult(result.id, result.message, result.usage, tool_calls, tool_result_futures)
```

Returns `StepResult` with:
- `message` — the generated assistant message
- `usage` — token usage statistics
- `tool_calls` — list of tool calls made
- `_tool_result_futures` — dict of `asyncio.Future[ToolResult]` for async tool execution

---

## 7. Provider Layer: HTTP Request Building

### 7.1 LLM Factory

**File:** `kimi-cli/src/kimi_cli/llm.py`  
**Function:** `create_llm()` (line 109)

Provider type dispatch:

| Provider Type | Implementation | Package |
|---|---|---|
| `kimi` | `Kimi` | `kosong.chat_provider.kimi` |
| `openai_legacy` | `OpenAILegacy` | `kosong.contrib.chat_provider.openai_legacy` |
| `openai_responses` | `OpenAIResponses` | `kosong.contrib.chat_provider.openai_responses` |
| `anthropic` | `Anthropic` | `kosong.contrib.chat_provider.anthropic` |
| `google_genai` / `gemini` | `GoogleGenAI` | `kosong.contrib.chat_provider.google_genai` |
| `vertexai` | `GoogleGenAI` (vertexai=True) | `kosong.contrib.chat_provider.google_genai` |
| `_echo` | `EchoChatProvider` | `kosong.chat_provider.echo` |
| `_scripted_echo` | `ScriptedEchoChatProvider` | `kosong.chat_provider.echo` |
| `_chaos` | `ChaosChatProvider` | `kosong.chat_provider.chaos` |

### 7.2 Provider Configuration

**File:** `kimi-cli/src/kimi_cli/llm.py` (lines 59–97)

Environment variable overrides:
- `KIMI_BASE_URL`, `KIMI_API_KEY`, `KIMI_MODEL_NAME`, `KIMI_MODEL_MAX_CONTEXT_SIZE`, `KIMI_MODEL_CAPABILITIES`
- `KIMI_MODEL_TEMPERATURE`, `KIMI_MODEL_TOP_P`, `KIMI_MODEL_MAX_TOKENS`
- `KIMI_MODEL_THINKING_KEEP`
- `OPENAI_BASE_URL`, `OPENAI_API_KEY`

### 7.3 Thinking Mode

**File:** `kimi-cli/src/kimi_cli/llm.py` (lines 240–262)

```python
thinking_on = "always_thinking" in capabilities or (thinking is True and "thinking" in capabilities)
if thinking_on:
    chat_provider = chat_provider.with_thinking("high")
elif thinking is False:
    chat_provider = chat_provider.with_thinking("off")
```

Moonshot-specific `thinking.keep` applied when in thinking mode.

### 7.4 LLM Dataclass

**File:** `kimi-cli/src/kimi_cli/llm.py` (lines 36–43)

```python
@dataclass(slots=True)
class LLM:
    chat_provider: ChatProvider
    max_context_size: int
    capabilities: set[ModelCapability]
    model_config: LLMModel | None = None
    provider_config: LLMProvider | None = None
```

---

## 8. Response Streaming: From Provider to Rendered Output

### 8.1 Stream Callback Chain

**File:** `kimi-cli/src/kimi_cli/soul/kimisoul.py` (within `_step()`)

```python
async def wire_send(part):
    wire.soul_side.send(part)

result = await kosong.step(
    chat_provider=runtime.llm.chat_provider,
    system_prompt=system_prompt,
    toolset=agent.toolset,
    history=effective_history,
    on_message_part=wire_send,
    on_tool_result=wire_send,
)
```

Every `StreamedMessagePart` (text delta, thinking delta, tool call fragment) and every `ToolResult` is immediately pushed to the `Wire`.

### 8.2 Wire Message Bus

**File:** `kimi-cli/src/kimi_cli/wire/__init__.py`  
**Classes:** `Wire`, `WireSoulSide`, `WireUISide`

SPMC channel:
- `WireSoulSide.send(msg)` — publishes to raw queue; mergeable messages (like `TextPart`) buffered and coalesced into merged queue
- `WireUISide.receive()` — awaits messages from raw or merged queue
- `Wire.shutdown()` — tears down both queues
- `_WireRecorder` optionally persists merged messages to `WireFile` (jsonl)

### 8.3 UI Loop Consumption

**File:** `kimi-cli/src/kimi_cli/ui/shell/visualize/_interactive.py`  
**Class:** `_PromptLiveView`  
**Function:** `visualize_loop()`

```python
async def visualize_loop(self):
    while True:
        msg = await wire.receive()
        self.dispatch_wire_message(msg)
        self._flush_prompt_refresh()
```

`dispatch_wire_message()` updates internal blocks:
- `TurnBegin` → reset state
- `StepBegin` → increment step counter
- `TextPart` → append to content block
- `ThinkPart` → append to thinking block
- `ToolCall` → create tool call block
- `ToolResult` → update tool call block with result
- `ToolCallPart` → streaming tool call args
- `ApprovalRequest` → queue approval panel
- `TurnEnd` / `StepInterrupted` → cleanup

`_flush_prompt_refresh()` calls `prompt_session.invalidate()` which triggers `_render_agent_prompt_message()` to re-render the bottom area.

### 8.4 Live View Rendering

**File:** `kimi-cli/src/kimi_cli/ui/shell/visualize/_live_view.py`  
**Class:** `_LiveView`

Uses `rich.live.Live` with `refresh_per_second=10`.

`compose_agent_output()` (line 395):
1. Spinners for active tool calls
2. Content blocks (streaming markdown)
3. Tool call blocks (with diff panels, todo lists, brief display)
4. Notification blocks
5. Status block (context usage, token count)

**Incremental commitment** (`_ContentBlock`):
- Completed content blocks are flushed to terminal history via `console.print()`
- Only the tail (incomplete block) stays in the `Live` display
- This prevents the `Live` area from growing unbounded

### 8.5 Prompt Toolkit Integration

**File:** `kimi-cli/src/kimi_cli/ui/shell/prompt.py`  
**Function:** `_render_agent_prompt_message()`

Converts Rich renderables to ANSI via `render_to_ansi()` and injects into prompt_toolkit's `FormattedText` layout.

---

## 9. Tool Call Lifecycle

### 9.1 Tool Classification

**File:** `kimi-cli/src/kimi_cli/soul/toolset.py`  
**Class:** `KimiToolset` (lines 125–682)

When the LLM requests a tool call, `KimiToolset.handle(tool_call)` runs:

1. **Sets context variable** `current_tool_call` for metadata access
2. **Same-step deduplication** — if exact same `(name, arguments)` already called this step, awaits original task and copies result
3. **Cross-step deduplication** — if same call made in previous step, executes but appends nag reminder
4. **Tool lookup** — resolves name in `self._tool_dict`
5. **JSON parsing** — parses arguments with `json.loads(..., strict=False)`
6. **PreToolUse hook** — fires `HookEngine`; if blocked, returns `ToolError`
7. **Tool execution** — `await tool.call(arguments)` (timed)
8. **Error handling** — on exception, fires `PostToolUseFailure` hook, returns `ToolRuntimeError`
9. **Success** — fires `PostToolUse` hook, tracks telemetry, returns `ToolResult`
10. **Stores task** in `_current_step_tasks` for same-step dedup

### 9.2 Tool Result to Message

**File:** `kimi-cli/src/kimi_cli/soul/message.py`  
**Function:** `tool_result_to_message()` (lines 35–62)

```python
def tool_result_to_message(tool_result: ToolResult) -> Message:
    if tool_result.return_value.is_error:
        content = [system(f"ERROR: {message}")]
        if output:
            content.extend(_output_to_content_parts(output))
    else:
        content = []
        if message: content.append(system(message))
        if output: content.extend(_output_to_content_parts(output))
        if not content: content.append(system("Tool output is empty."))
        elif not any(isinstance(p, TextPart) for p in content):
            content.insert(0, system("Tool returned non-text content."))

    return Message(role="tool", content=content, tool_call_id=tool_result.tool_call_id)
```

### 9.3 Context Growth

**File:** `kimi-cli/src/kimi_cli/soul/kimisoul.py` (around line 1222)

```python
tool_messages = [tool_result_to_message(tr) for tr in tool_results]
# append assistant message + tool messages to context
```

### 9.4 Deduplication State

**File:** `kimi-cli/src/kimi_cli/soul/toolset.py`

- `begin_step(previous_calls)` — resets per-step state
- `end_step()` — returns calls made this step (becomes `previous_calls` for next step)

---

## 10. Wire Protocol and Approval System

### 10.1 Wire Protocol

**File:** `kimi-cli/src/kimi_cli/wire/__init__.py`  
**Protocol version:** `WIRE_PROTOCOL_VERSION = "1.10"`

**Event types** (`wire/types.py`):
- `TurnBegin`, `SteerInput`, `TurnEnd`
- `StepBegin`, `StepInterrupted`, `StepRetry`
- `CompactionBegin`, `CompactionEnd`
- `ContentPart` (TextPart, ThinkPart, ImageURLPart, etc.)
- `ToolCall`, `ToolCallPart`, `ToolResult`
- `ApprovalRequest`, `ApprovalResponse`
- `QuestionRequest`, `HookRequest`
- `SubagentEvent`, `PlanDisplay`, `BtwBegin`, `BtwEnd`
- `StatusUpdate`, `Notification`

**Request types** (carry `asyncio.Future` for response):
- `ApprovalRequest` — asks user to approve an action
- `ToolCallRequest` — asks wire client to execute external tool
- `QuestionRequest` — asks user structured questions
- `HookRequest` — asks wire client to evaluate a hook

### 10.2 RootWireHub

**File:** `kimi-cli/src/kimi_cli/wire/root_hub.py`

Session-level broadcast hub for **out-of-turn** messages. Uses `BroadcastQueue` under the hood.

Why it exists: the main `Wire` is scoped to a single turn. Background agents or external approvals need a channel that survives across turns.

### 10.3 ApprovalRuntime

**File:** `kimi-cli/src/kimi_cli/approval_runtime/runtime.py`  
**Class:** `ApprovalRuntime`

Central authority for approval state. Shared across subagents.

```python
class ApprovalRuntime:
    def create_request(...) -> ApprovalRequestRecord
    async def wait_for_response(request_id, timeout=None) -> tuple[ApprovalResponseKind, str]
    def resolve(request_id, response, feedback="") -> bool
    def list_pending() -> list[ApprovalRequestRecord]
```

- `create_request()` builds record, publishes `ApprovalRequest` to `RootWireHub`
- `wait_for_response()` creates shared `asyncio.Future`, awaits it, auto-rejects on timeout
- `resolve()` sets future result, publishes `ApprovalResponse` on `RootWireHub`

### 10.4 Approval Flow

```
Tool calls Approval.request()
    │
    ▼
ApprovalRuntime.create_request()
    │
    ▼
RootWireHub.publish_nowait(ApprovalRequest)
    │
    ├──► Shell UI (_watch_root_wire_hub)
    │      ├── Interactive: _queue_approval_request() → ApprovalPromptDelegate modal
    │      └── Non-interactive: _LiveView.request_approval() → ApprovalRequestPanel
    │
    ├──► WireServer (_root_hub_loop) → JSON-RPC request to IDE client
    │
    ├──► Web UI
    │
    └──► Vis UI
    │
    ▼ User responds
ApprovalRuntime.resolve()
    │
    ▼
Tool unblocks (wait_for_response returns)
```

### 10.5 Approval UI

**Interactive shell mode:**
**File:** `kimi-cli/src/kimi_cli/ui/shell/visualize/_approval_panel.py`

`ApprovalRequestPanel` renders as Rich panel with:
- Sender and action
- Preview of diff/shell/brief display blocks
- 4 options: Approve once / Approve for session / Reject / Reject with feedback

`ApprovalPromptDelegate` (line 335) is a modal delegate attached to `CustomPromptSession`. Handles key events (↑/↓, 1-4, Enter, Escape, Ctrl-E for pager).

**Non-interactive live-view mode:**
**File:** `kimi-cli/src/kimi_cli/ui/shell/visualize/_live_view.py`

`_LiveView.request_approval()` appends to `_approval_request_queue`. `show_next_approval_request()` creates `ApprovalRequestPanel` in Rich Live display. Keyboard events dispatched via `_keyboard_listener`.

---

## 11. Tool System and Runtime

### 11.1 Runtime

**File:** `kimi-cli/src/kimi_cli/soul/agent.py`  
**Class:** `Runtime` (lines 172–369)

`@dataclass(slots=True, kw_only=True)` holding all shared services:

| Field | Purpose |
|---|---|
| `config` | Application configuration |
| `oauth` | OAuth token management |
| `llm` | The language model instance |
| `session` | Persistent session state |
| `builtin_args` | Pre-computed system prompt variables (time, cwd, AGENTS.md, skills, OS, shell) |
| `denwa_renji` | "Phone" system for subagent communication |
| `approval` | User-approval logic (yolo, afk, auto-approve) |
| `labor_market` | Registry of available subagent types |
| `environment` | Detected OS, shell path |
| `notifications` | User notification queue |
| `background_tasks` | Long-running shell task manager |
| `skills` | Discovered skills indexed by name |
| `approval_runtime` | ACP/wire approval runtime |
| `root_wire_hub` | Event routing hub |
| `hook_engine` | PreToolUse / PostToolUse hooks |

**Creation** (`Runtime.create()`, line 212) concurrently:
1. Lists cwd
2. Loads merged `AGENTS.md` files (root → leaf, leaf-first budget truncation)
3. Detects environment (OS, shell)
4. Discovers and formats skills
5. Initializes approval state from CLI flags and persisted session state

### 11.2 Tool Definition

**Base types** (from `kosong.tooling`):
- `CallableTool` — older single-dispatch style
- `CallableTool2[T]` — Pydantic-params style (used by all built-in tools)
- `Tool` — schema/description object exposed to the LLM

**Example tool:**
**File:** `kimi-cli/src/kimi_cli/tools/shell/__init__.py` (lines 60–260)

```python
class Params(BaseModel):
    command: str = Field(description="...")
    timeout: int = Field(default=60, ...)
    run_in_background: bool = Field(default=False, ...)

class Shell(CallableTool2[Params]):
    name: str = "Shell"
    params: type[Params] = Params

    def __init__(self, approval: Approval, environment: Environment, runtime: Runtime):
        ...

    async def __call__(self, params: Params) -> ToolReturnValue:
        ...
```

### 11.3 Tool Registration

**File:** `kimi-cli/src/kimi_cli/soul/agent.py` (lines 383–491)

1. Instantiate `KimiToolset()`
2. Build dependency map:
   ```python
   tool_deps = {
       KimiToolset: toolset,
       Runtime: runtime,
       Config: runtime.config,
       Approval: runtime.approval,
       Environment: runtime.environment,
       ...
   }
   ```
3. Load tool paths from agent spec via `toolset.load_tools(tools, tool_deps)`
4. `_load_tool()` (line 497) uses `importlib` + inspects `__init__` signatures + **injects dependencies by type annotation**
5. Load plugin tools
6. Load MCP tools (optionally in background)

### 11.4 Built-in Tools

**Directory:** `kimi-cli/src/kimi_cli/tools/`

| Tool | File | Description |
|---|---|---|
| `Shell` | `shell/__init__.py` | Bash execution, foreground/background, timeout |
| `ReadFile` | `file/read.py` | Text file reading with offsets, tail, truncation |
| `ReadMediaFile` | `file/read_media.py` | Image/video reading |
| `WriteFile` | `file/write.py` | Overwrite/append with approval diffs |
| `StrReplaceFile` | `file/replace.py` | String replacement |
| `Glob` | `file/glob.py` | File pattern matching |
| `Grep` | `file/grep_local.py` | Regex search |
| `SearchWeb` | `web/search.py` | Web search |
| `FetchURL` | `web/fetch.py` | URL fetching |
| `Agent` | `agent/__init__.py` | Subagent spawning |
| `Think` | `think/__init__.py` | Explicit thinking step |
| `SetTodoList` | `todo/__init__.py` | Todo list management |
| `TaskList` / `TaskOutput` / `TaskStop` | `background/` | Background task management |
| `SendDMail` | `dmail/__init__.py` | Inter-agent messaging |
| `AskUser` | `ask_user/__init__.py` | Structured user questions |
| `PlanMode` | `file/plan_mode.py` | Plan mode read-only enforcement |

Tool descriptions loaded from adjacent `.md` files via `load_desc()` + Jinja2.

### 11.5 MCP Integration

**File:** `kimi-cli/src/kimi_cli/soul/toolset.py` (lines 531–671, 691–758, 821–887)

`load_mcp_tools(mcp_configs, runtime, in_background=True)`:
1. Iterate `mcpServers` in each `MCPConfig`
2. Check OAuth authorization; skip unauthorized
3. Create `fastmcp.Client` per server
4. Store server info with status `"pending"`
5. If `in_background=True`, start `asyncio.Task` to connect
6. `_connect_server()` opens client, lists tools, wraps each in `MCPTool`, registers

**MCPTool wrapper** (lines 691–758):
```python
class MCPTool[T: ClientTransport](CallableTool):
    def __init__(self, server_name, mcp_tool, client, *, runtime: Runtime, ...):
        super().__init__(
            name=mcp_tool.name,
            description=f"This is an MCP tool from server `{server_name}`.\n\n{mcp_tool.description}",
            parameters=mcp_tool.inputSchema,
        )
```

On call: requests approval → `client.call_tool(name, kwargs, timeout)` → `convert_mcp_tool_result(result)`

**MCP result conversion** (lines 821–887):
- Converts each MCP content part
- Enforces **100,000 character budget** (`MCP_MAX_OUTPUT_CHARS`)
- Text truncated in-place; media dropped silently if over budget
- Unsupported content → `[Unsupported content: ...]` placeholders

### 11.6 Tool Result Builder

**File:** `kimi-cli/src/kimi_cli/tools/utils.py`  
**Class:** `ToolResultBuilder` (lines 54–179)

```python
builder = ToolResultBuilder(max_chars=50_000, max_line_length=2000)
builder.write("stdout line 1\n")
return builder.ok("Command executed successfully.", brief="Success")
# or
return builder.error("Command failed...", brief="Failed")
```

Features: character budget, per-line truncation, automatic truncation message, display block attachment, extra JSON data.

---

## 12. Alternative UI Modes

### 12.1 Print Mode (Headless)

**File:** `kimi-cli/src/kimi_cli/ui/print/__init__.py`  
**Class:** `Print`

Headless mode: read input → run agent → print output → exit.

```python
print_ = Print(soul, input_format, output_format, context_file, final_only=final_only)
return await print_.run(command)
```

Input formats: `text`, `json`, `image`  
Output formats: `text`, `json`, `markdown`, `code`

### 12.2 ACP Mode

**File:** `kimi-cli/src/kimi_cli/ui/acp/__init__.py`  
**Class:** `ACP`

Agent Client Protocol server. Exposes the agent as an MCP-compatible server.

### 12.3 Wire Mode

**File:** `kimi-cli/src/kimi_cli/wire/server.py`  
**Class:** `WireServer`

JSON-RPC server over stdio for IDE integration.

```python
server = WireServer(self._soul)
await server.serve()
```

Handles:
- `initialize` / `initialized`
- `messages/list` — conversation history
- `messages/send` — send user message
- `tools/call` — execute tool
- `approval/request` — request approval
- Subscriptions to `RootWireHub`

### 12.4 Web Mode

**File:** `kimi-cli/src/kimi_cli/web/app.py`

FastAPI + WebSocket server. Provides browser-based UI.

### 12.5 Vis Mode

**File:** `kimi-cli/src/kimi_cli/vis/app.py`

Terminal UI (TUI) using a separate visual interface.

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
│  PROMPT-TOOLKIT CUSTOM PROMPT SESSION                                       │
│  kimi-cli/src/kimi_cli/ui/shell/prompt.py :: CustomPromptSession            │
│  - Buffer with custom key bindings, completers, bottom toolbar              │
│  - Modal delegates for approval dialogs                                     │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  SHELL INPUT LOOP                                                           │
│  kimi-cli/src/kimi_cli/ui/shell/__init__.py :: Shell.run()                  │
│  - _route_prompt_events() → idle_events queue                               │
│  - _BackgroundCompletionWatcher.wait_for_next()                             │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  INPUT DISPATCH                                                             │
│  kimi-cli/src/kimi_cli/ui/shell/__init__.py                                 │
│  - Slash command? → _run_slash_command() or run_soul_command()              │
│  - Shell mode? → execute locally                                            │
│  - Agent mode? → run_soul_command()                                         │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  RUN SOUL                                                                   │
│  kimi-cli/src/kimi_cli/soul/__init__.py :: run_soul()                       │
│  - Creates Wire (soul ↔ UI message bus)                                     │
│  - Starts UI loop task + soul run task + notification pump                  │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  KIMISOUL RUN                                                               │
│  kimi-cli/src/kimi_cli/soul/kimisoul.py :: KimiSoul.run()                   │
│  - Slash command detection                                                  │
│  - _turn(user_message) → _agent_loop()                                      │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  AGENT STEP                                                                 │
│  kimi-cli/src/kimi_cli/soul/kimisoul.py :: _step()                          │
│  - Notifications + dynamic injections + history normalization               │
│  - kosong.step(chat_provider, system_prompt, toolset, history, ...)         │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  KOSONG GENERATE                                                            │
│  packages/kosong/src/kosong/_generate.py :: generate()                      │
│  - chat_provider.generate() → async stream of StreamedMessagePart           │
│  - merge_in_place() coalesces adjacent deltas                               │
│  - on_message_part callback for each part                                   │
│  - on_tool_call callback when ToolCall fully assembled                      │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  CHAT PROVIDER (HTTP STREAMING)                                             │
│  packages/kosong/src/kosong/chat_provider/kimi.py :: Kimi                   │
│  packages/kosong/src/kosong/contrib/chat_provider/anthropic.py              │
│  packages/kosong/src/kosong/contrib/chat_provider/openai_legacy.py          │
│  - HTTP POST with system_prompt, tools, history                             │
│  - Server-Sent Events (SSE) or chunked response                             │
│  - Yields TextPart, ThinkPart, ToolCallPart deltas                          │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼ StreamedMessagePart
┌─────────────────────────────────────────────────────────────────────────────┐
│  WIRE MESSAGE BUS                                                           │
│  kimi-cli/src/kimi_cli/wire/__init__.py :: WireSoulSide.send()              │
│  - on_message_part → wire.soul_side.send(part)                              │
│  - on_tool_result → wire.soul_side.send(tool_result)                        │
│  - Mergeable messages coalesced in merged queue                             │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼ WireMessage
┌─────────────────────────────────────────────────────────────────────────────┐
│  UI LOOP CONSUMPTION                                                        │
│  kimi-cli/src/kimi_cli/ui/shell/visualize/_interactive.py                   │
│  :: _PromptLiveView.visualize_loop()                                        │
│  - dispatch_wire_message(msg) updates blocks                                │
│  - _flush_prompt_refresh() → prompt_session.invalidate()                    │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  RENDER PIPELINE                                                            │
│  kimi-cli/src/kimi_cli/ui/shell/visualize/_live_view.py                     │
│  - compose_agent_output() → spinners, content, tools, notifications         │
│  - rich.live.Live(refresh_per_second=10, transient=True)                    │
│  kimi-cli/src/kimi_cli/ui/shell/prompt.py                                   │
│  - _render_agent_prompt_message() → render_to_ansi()                        │
│  - FormattedText injected into prompt_toolkit layout                        │
└─────────────────────────────────────────────────────────────────────────────┘
    │
    ▼ ANSI escapes
┌─────────────────────────────────────────────────────────────────────────────┐
│  TERMINAL DISPLAY                                                           │
│  - Rich Console writes to stdout                                            │
│  - prompt_toolkit Application renders full-screen layout                    │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 13.2 Tool Call Data Flow

```
LLM response includes ToolCall parts
    │
    ▼
kosong.generate() assembles complete ToolCall
    │
    ▼
on_tool_call callback fires
    │
    ▼
kosong.step() → toolset.handle(tool_call)
    │
    ▼
KimiToolset.handle()
    │
    ├──► Deduplication check (same-step / cross-step)
    ├──► PreToolUse hooks
    ├──► tool.__call__(params) ──► ToolResultBuilder / ToolOk / ToolError
    ├──► PostToolUse hooks
    └──► ToolResult
            │
            ▼
    wire.soul_side.send(ToolResult) ──► UI updates tool block
            │
            ▼
    tool_result_to_message() ──► Message(role="tool", content=[...])
            │
            ▼
    Added to context ──► next LLM call includes tool result
```

### 13.3 Approval Request Data Flow

```
Tool calls Approval.request()
    │
    ▼
ApprovalRuntime.create_request()
    │
    ▼
RootWireHub.publish_nowait(ApprovalRequest)
    │
    ├──► Shell UI: _watch_root_wire_hub()
    │      ├── Interactive: ApprovalPromptDelegate modal
    │      └── Live view: ApprovalRequestPanel
    │
    ├──► WireServer: JSON-RPC → IDE client
    │
    ├──► Web UI: WebSocket broadcast
    │
    └──► Vis UI: TUI panel
    │
    ▼ User responds
ApprovalRuntime.resolve(request_id, response)
    │
    ▼
Tool unblocks (wait_for_response returns ApprovalResult)
```

### 13.4 Key Architectural Decisions

1. **Prompt-toolkit + Rich separation:** Input handling uses prompt-toolkit (key bindings, modal dialogs, completions); output rendering uses Rich (markdown, syntax highlighting, live display). They meet at the ANSI boundary via `render_to_ansi()`.

2. **Wire protocol decouples soul from UI:** The `Wire` SPMC channel enables multiple frontends (shell, print, ACP, web, vis, wire/stdio) without changing agent logic.

3. **Kosong provider abstraction:** All LLM providers implement the same `ChatProvider` interface. Switching providers is a factory decision, not a code change.

4. **Incremental commitment for streaming:** Completed content blocks are flushed to terminal history via `console.print()`; only the tail stays in the `Live` display. Prevents unbounded growth.

5. **ApprovalRuntime with RootWireHub:** Approval requests broadcast to all UI consumers simultaneously. Any consumer can resolve the request, unblocking the tool.

6. **Dependency injection for tools:** Tools declare their dependencies via `__init__` type annotations. The runtime builds a dependency map and injects matching services automatically.

7. **YAML agent specs with inheritance:** Agent behavior (system prompt, tools, subagents) is configurable via YAML files that can extend each other.

8. **Same-step tool deduplication:** If the LLM calls the same tool with identical args twice in one step, the second call awaits the first's result. Cross-step calls get a nag reminder.

---

## Appendix A: Key Files Reference

| Concern | File | Key Class / Function |
|---------|------|---------------------|
| Binary entry | `kimi-cli/src/kimi_cli/__main__.py` | `main()` |
| CLI root | `kimi-cli/src/kimi_cli/cli/__init__.py` | `cli()`, `kimi()` |
| Lazy commands | `kimi-cli/src/kimi_cli/cli/_lazy_group.py` | `LazySubcommandGroup` |
| App factory | `kimi-cli/src/kimi_cli/app.py` | `KimiCLI.create()`, `KimiCLI.run_shell()` |
| LLM factory | `kimi-cli/src/kimi_cli/llm.py` | `create_llm()`, `LLM` |
| Config | `kimi-cli/src/kimi_cli/config.py` | `Config`, `load_config()` |
| Shell UI | `kimi-cli/src/kimi_cli/ui/shell/__init__.py` | `Shell`, `Shell.run()` |
| Prompt session | `kimi-cli/src/kimi_cli/ui/shell/prompt.py` | `CustomPromptSession` |
| Slash commands | `kimi-cli/src/kimi_cli/ui/shell/slash.py` | `registry`, `shell_mode_registry` |
| Console | `kimi-cli/src/kimi_cli/ui/shell/console.py` | `console`, `render_to_ansi()` |
| Live view | `kimi-cli/src/kimi_cli/ui/shell/visualize/_live_view.py` | `_LiveView` |
| Interactive view | `kimi-cli/src/kimi_cli/ui/shell/visualize/_interactive.py` | `_PromptLiveView` |
| Content blocks | `kimi-cli/src/kimi_cli/ui/shell/visualize/_blocks.py` | `_ContentBlock`, `_ToolCallBlock` |
| Approval panel | `kimi-cli/src/kimi_cli/ui/shell/visualize/_approval_panel.py` | `ApprovalRequestPanel` |
| Input router | `kimi-cli/src/kimi_cli/ui/shell/visualize/_input_router.py` | `_InputRouter` |
| Soul orchestrator | `kimi-cli/src/kimi_cli/soul/__init__.py` | `run_soul()` |
| KimiSoul | `kimi-cli/src/kimi_cli/soul/kimisoul.py` | `KimiSoul`, `_turn()`, `_step()` |
| Agent loading | `kimi-cli/src/kimi_cli/soul/agent.py` | `load_agent()`, `Runtime` |
| Context | `kimi-cli/src/kimi_cli/soul/context.py` | `Context` |
| Toolset | `kimi-cli/src/kimi_cli/soul/toolset.py` | `KimiToolset`, `handle()` |
| Approval | `kimi-cli/src/kimi_cli/soul/approval.py` | `Approval`, `Approval.request()` |
| Message conversion | `kimi-cli/src/kimi_cli/soul/message.py` | `tool_result_to_message()` |
| Agent spec | `kimi-cli/src/kimi_cli/agentspec.py` | `AgentSpec`, `ResolvedAgentSpec` |
| Kosong generate | `packages/kosong/src/kosong/_generate.py` | `generate()`, `GenerateResult` |
| Kosong step | `packages/kosong/src/kosong/__init__.py` | `step()`, `StepResult` |
| Wire channel | `kimi-cli/src/kimi_cli/wire/__init__.py` | `Wire`, `WireSoulSide`, `WireUISide` |
| Wire types | `kimi-cli/src/kimi_cli/wire/types.py` | `WireMessage`, `ApprovalRequest` |
| Wire server | `kimi-cli/src/kimi_cli/wire/server.py` | `WireServer` |
| RootWireHub | `kimi-cli/src/kimi_cli/wire/root_hub.py` | `RootWireHub` |
| Approval runtime | `kimi-cli/src/kimi_cli/approval_runtime/runtime.py` | `ApprovalRuntime` |
| Print UI | `kimi-cli/src/kimi_cli/ui/print/__init__.py` | `Print` |
| ACP UI | `kimi-cli/src/kimi_cli/ui/acp/__init__.py` | `ACP` |
| Web app | `kimi-cli/src/kimi_cli/web/app.py` | `create_web_app()` |
| Vis app | `kimi-cli/src/kimi_cli/vis/app.py` | `VisApp` |
| Shell tool | `kimi-cli/src/kimi_cli/tools/shell/__init__.py` | `Shell` |
| ReadFile tool | `kimi-cli/src/kimi_cli/tools/file/read.py` | `ReadFile` |
| MCP integration | `kimi-cli/src/kimi_cli/soul/toolset.py` | `MCPTool`, `load_mcp_tools()` |
| Tool result builder | `kimi-cli/src/kimi_cli/tools/utils.py` | `ToolResultBuilder` |
| Background tasks | `kimi-cli/src/kimi_cli/background/manager.py` | `BackgroundTaskManager` |
| Notifications | `kimi-cli/src/kimi_cli/notifications/manager.py` | `NotificationManager` |
| Session | `kimi-cli/src/kimi_cli/session.py` | `Session` |
| OAuth | `kimi-cli/src/kimi_cli/auth/oauth.py` | `OAuthManager` |
| Telemetry | `kimi-cli/src/kimi_cli/telemetry/sink.py` | `EventSink` |

---

*End of analysis.*
