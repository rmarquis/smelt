# Smelt: Keystroke → LLM → Screen — A Complete Code-Path Map

This document traces every significant code path that transforms a user keystroke into an LLM API request and brings the response back to the terminal screen. It is architecture-first: the goal is to understand how the agent actually works, not to document every function signature.

---

## Crate Graph

```
smelt-agent (binary: src/)
    ├── tui                 — TUI app, event loop, rendering
    │   ├── smelt-core      — shared state, Lua runtime, permissions, session
    │   │   ├── smelt-buffer    — text buffer, undo, UTF-8 safe ops
    │   │   └── smelt-edit      — editor widget (prompt input)
    │   ├── engine          — agent task, HTTP, tool dispatch
    │   │   └── protocol    — shared wire types (EngineEvent, UiCommand, Message)
    │   └── smelt-term      — compositor, windows, surfaces, ANSI flush
    └── smelt-perf          — allocation counter + perf-span timing
```

Key constraint: **`tui` is single-threaded** (all Lua, all rendering, all event dispatch happen on the main thread). The **`engine` task** is a separate `tokio::spawn`. They communicate over two `mpsc::unbounded_channel` pairs (UI→Engine `UiCommand`, Engine→UI `EngineEvent`) plus a third `host_rx` channel for synchronous engine→UI RPC calls (middleware hooks).

---

## Phase 0: Two-Pass Startup (`src/main.rs`)

```
main()
 ├─ LuaRuntime::new()
 ├─ lua_runtime.load_early_init()           -- runs runtime/lua/smelt/_bootstrap.lua (early.lua hook)
 │      smelt.cli.register_flag{} calls populate LuaShared::cli_flag_specs
 ├─ lua_runtime.load_project_early_init()
 ├─ parse_with_lua_flags()                  -- extends clap command with Lua specs, then parses argv
 ├─ lua_runtime.load_autoload()
 ├─ lua_runtime.load_user_config()          -- init.lua (smelt.provider.register, smelt.defaults, etc.)
 ├─ lua_runtime.load_global_plugins()
 ├─ lua_runtime.load_project_config()       -- .smelt/init.lua if trusted
 ├─ lua_runtime.to_config()                 -- converts Lua registry into smelt_core::config::Config
 ├─ startup::resolve()                      -- resolves model, API key, settings, mode, reasoning effort
 ├─ McpManager::start()                     -- starts MCP server processes (stdio/HTTP)
 ├─ engine::start()                         -- spawns engine_task; returns EngineHandle
 │      engine_task lives here forever, awaiting UiCommands on cmd_rx
 └─ TuiApp::new() + app.run()
```

### startup::resolve() detail (`src/startup.rs`)

Walks `Config` (from Lua) + CLI args + `SessionCache` (from previous session) to produce a `ResolvedStartup`:
- Model priority: `--model` > `smelt.defaults{model=…}` > cached model > first model in list
- API key: read from env var named by `api_key_env`
- Codex / Copilot: spawns background tasks to refresh model lists
- Mode / reasoning cycle: CLI > Lua defaults > last-session cache

---

## Phase 1: TuiApp::run() — Main Event Loop (`crates/tui/src/app.rs:753`)

```rust
loop {
    // Per-tick housekeeping (always runs before blocking select!)
    tick_timers()            // fire due Lua timer callbacks
    publish_diff_cells()     // update vim_mode, spinner_frame, now, confirms_pending cells
    drain_cells_pending()    // invoke Lua cell subscribers
    drive_lua_tasks()        // advance parked Lua coroutines
    ui.dispatch_tick()       // fire window-registered tick callbacks
    flush_lua_callbacks()    // drain queued Lua invocations from above

    // Try context-window result (one-shot fetch at startup)
    ctx_rx.try_recv()

    // Drain engine events (non-blocking)
    drain_host_calls()
    loop { engine.try_recv() → handle_engine_event() + dispatch_control() }

    // Process next queued message if agent is idle
    if agent.is_none() && queued_messages.is_not_empty() { process_input() }

    // Render
    render_normal(agent_running)

    // Block until next event
    tokio::select! {
        term_events    → dispatch_terminal_event()
        engine.recv()  → dispatch_engine_event()
        host_rx.recv() → dispatch_host_call()
        lua_wakeup_rx  → flush_lua_callbacks() + drive_lua_tasks()
        auto_reload_rx → reload_lua()
        exec output    → append_exec_output() / finish_exec()
        timer          → tick_drag_autoscroll() (animation)
        SIGWINCH       → handle_resize()
    }
}
```

The loop is `biased`: terminal events have higher priority than engine events, which have higher priority than host calls, etc.

---

## Phase 2: Terminal Event → Keystroke Dispatch (`crates/tui/src/app/events.rs`)

```
dispatch_terminal_event(ev)
 ├─ FocusGained/Lost → set term_focused; return
 ├─ Check global key actions (overlay not focused, no cmdline):
 │      keymap::lookup() → ToggleMode / CycleReasoning / Redraw
 ├─ Overlay/modal focused?
 │   ├─ Resize → handle_resize()
 │   ├─ Key:
 │   │   ├─ cmdline_is_focused() → cmdline_handle_key(k)     [cmdline.rs]
 │   │   └─ run_key_cascade(k)   [5-tier: specific keymap → global Lua → fallback → vim → modal dismiss]
 │   └─ Mouse → fall through
 ├─ Ctrl+C while exec running → kill exec
 └─ Route by agent state:
     ├─ agent running → handle_event_running(ev)
     └─ agent idle   → handle_event_idle(ev)
```

Both idle and running paths call `dispatch_common(ev)` first:

```
dispatch_common(ev)
 ├─ Paste → clear_prompt_completer()
 ├─ Resize → handle_resize()
 ├─ Mouse  → handle_mouse()   [mouse.rs]
 ├─ Key (no overlay):
 │   ├─ ui.dispatch_event() → buffer-local Lua keymaps (win_set_keymap)
 │   └─ Global Lua keymaps:
 │       ├─ Single-key: lua.run_keymap(token, vim_mode, None)
 │       └─ Multi-key chord: match_chord() via LuaChordOracle
 │             [smelt_core::keymap::match_chord with 500 ms timeout]
 ├─ Pane chord (Ctrl-W prefix) → handle_pane_chord()
 ├─ ':' (not in insert) → open_cmdline()
 ├─ Content focus → handle_event_app_history()
 └─ Overlay keys → dismiss notification
```

### handle_event_idle (agent not running)

```
handle_event_idle(ev)
 ├─ dispatch_common(ev) — as above
 ├─ Single Esc (non-vim) → noop
 ├─ Keymap lookup → AcceptGhostText / Quit / ClearBuffer
 └─ input.handle_event(ev, history, clipboard, now)   [smelt_edit / PromptState]
      Returns Action::{Submit, SubmitEmpty, Redraw, EditInEditor, …}
 └─ dispatch_input_action(action):
      Submit {content, display} → EventOutcome::Submit
```

On `EventOutcome::Submit`:
```
redact_user_submission() (if settings.redact_secrets)
process_input(text):
 ├─ input_history.push(text)
 ├─ commands::run_command(self, text)   [crates/tui/src/commands.rs]
 │    Lua slash commands, built-in commands (/model, /session, etc.)
 │    Returns CommandAction::Continue or CommandAction::Exec(handle)
 ├─ is_command('/…') → return Continue (command already handled)
 ├─ '!' prefix (shell escape) → return Continue (exec, not agent)
 └─ cells.set_dyn("input_submit", text) + pump_lua()
    → return InputOutcome::StartAgent
apply_input_outcome(StartAgent, content, display):
 └─ begin_agent_turn(display, content)
```

---

## Phase 3: begin_agent_turn() → UiCommand::StartTurn (`crates/tui/src/app/agent.rs:26`)

```
begin_agent_turn(display, content)
 ├─ sleep_inhibit.acquire()     (prevents system sleep during inference)
 ├─ begin_turn()                (transcript housekeeping, working state)
 ├─ show_user_message()         (push user block to transcript display)
 ├─ session.messages.push(Message::user(content.clone()))
 │   sync_session_snapshot()
 │   session.messages.pop()     (not yet committed; will come from engine TurnComplete)
 ├─ maybe_generate_title()      → engine.send(UiCommand::GenerateTitle) if first turn
 └─ dispatch_turn(content):
     ├─ resolve_api_key()
     ├─ working.begin(TurnPhase::Working)
     ├─ cells.set_dyn("turn_start", EventStub) + pump_lua()
     ├─ rebuild_system_prompt():
     │    build_defaults(cwd, mode, is_interactive, skill_section, instructions)
     │    → PromptSections::assemble()        [prompt sections in order]
     ├─ lua.tool_defs(mode)    → Vec<protocol::ToolDef> (Lua-registered tools)
     ├─ allocate turn_id (monotonic counter)
     └─ engine.send(UiCommand::StartTurn(Box::new(StartTurnPayload {
            turn_id, content, mode, model, reasoning_effort,
            history: session.messages.clone(),
            api_base, api_key,
            system_prompt: Some(assembled_prompt),
            tools: lua_tool_defs,
            …
        })))
     returns TurnState { turn_id, pending: [], _perf }
```

`self.agent = Some(turn_state)` — the TUI is now in "agent running" state.

---

## Phase 4: engine_task Receives StartTurn (`crates/engine/src/agent.rs:56`)

The engine task runs in a `tokio::spawn`. It loops on `cmd_rx.recv()`:

```
engine_task(config, dispatcher, cmd_rx, event_tx, host_tx)
 ├─ Build reqwest::Client (with smelt/ user-agent)
 ├─ pricing::spawn_catalog_fetch()   (background cost lookup)
 ├─ event_tx.send(EngineEvent::Ready)
 └─ loop:
     cmd_rx.recv() →
     UiCommand::StartTurn(payload):
       ├─ build_provider(api, client, api_base, api_key, model_overrides, clock)
       │    Provider::new(api_base, api_key, provider_type, client, clock)
       │    → ProviderKind::{OpenAiCompatible, OpenAi, Codex, AnthropicCompatible, Anthropic, Copilot}
       ├─ system_prompt = payload.system_prompt
       │    or config.system_prompt_override
       │    or build_system_prompt_full(mode, cwd, instructions, skill_section)
       │         [minijinja template: engine/src/prompts/system.txt]
       ├─ Turn { provider, dispatcher, cmd_rx, event_tx, host_tx, … }
       └─ turn.run(input_content, history).await
```

---

## Phase 5: Turn::run() — Agent Agentic Loop (`crates/engine/src/agent.rs:799`)

```
Turn::run(content, history)
 ├─ provider.reset_turn_state()    (clear Codex sticky turn token)
 ├─ messages = [system_prompt] + history + [user(content)]
 ├─ emit_messages_snapshot()       → EngineEvent::Messages
 └─ loop:
     ├─ drain_commands()            (handle queued UiCommands between iterations)
     ├─ regenerate_system_prompt()  (if mode changed mid-turn)
     ├─ Compute tool_defs:
     │    dispatcher.definitions() filtered by is_visible(name, mode)
     │    minus names shadowed by plugin tools with override_core=true
     │    plus Lua plugin tools (filtered by modes if set)
     ├─ Check cancel token
     ├─ Middleware on_request:
     │    host_call(HostCall::ProviderRequest { messages }) → await TUI reply
     │    [TUI runs Lua smelt.provider.middleware{on_request=…} chain]
     │    If replacement returned, swap messages
     ├─ (result, partial_text, partial_reasoning) = call_llm(tool_defs).await
     ├─ On Err(Cancelled) → commit_partial_assistant, emit_turn_complete(interrupted=true), return
     ├─ On Err(QuotaExceeded) → emit_turn_complete, emit TurnError, return
     ├─ On Err(other) → emit_turn_complete, emit TurnError, return
     ├─ send_usage() → EngineEvent::TokenUsage
     ├─ maybe_compact(prompt_tokens) — auto-compact if above threshold
     ├─ If tool_calls empty:
     │    apply_response_hooks(Message::assistant(content, reasoning, None))
     │         [HostCall::ProviderResponse → TUI middleware chain]
     │    messages.push(msg)
     │    emit_messages_snapshot()
     │    emit_turn_complete(false)
     │    return
     └─ Tool calls present:
         apply_response_hooks(Message::assistant(content, reasoning, Some(tool_calls)))
         messages.push(msg)
         emit_messages_snapshot()
         plan = classify_tools(tool_calls)
         execute_concurrent(plan, completed).await
         run_sequential(plan, completed).await
         collect_results(plan, completed)
         push tool messages → loop again (model sees tool results)
```

---

## Phase 6: call_llm() — HTTP Request (`crates/engine/src/agent.rs:1631`)

```
call_llm(tool_defs)
 ├─ Set up callbacks:
 │    on_retry(delay, attempt) → event_tx.send(EngineEvent::Retrying)
 │    on_delta(delta):
 │      StreamDelta::Text(s)    → partial_text += s; send TextDelta
 │      StreamDelta::Thinking(s)→ partial_reasoning += s; send ThinkingDelta
 │      StreamDelta::ToolArgs   → send ToolArgsDelta
 │
 ├─ opts = ChatOptions { cancel, on_retry, on_delta }
 ├─ chat_future = provider.chat(messages, tool_defs, model, reasoning_effort, opts)
 └─ tokio::select! loop (cancel-aware):
     chat_future completes → break with result
     cmd_rx.recv():
       UiCommand::Cancel         → cancel.cancel(), set cancel_received=true
       SetAgentMode {mode,…}     → update self.mode / system_prompt / tools
       SetReasoningEffort {…}    → update self.reasoning_effort
       SetModel {…}              → deferred (pending_model)
       Steer / Unsteer           → deferred to deferred_turn_cmds
       other                     → handle_background_cmd()
```

### provider.chat() (`crates/engine/src/provider/mod.rs:436`)

```
Provider::chat(messages, tools, model, effort, opts)
 ├─ Codex: codex::ensure_access_token_full() (OAuth token refresh if needed)
 ├─ Copilot: copilot::ensure_access_token_full()
 ├─ Build (url, body):
 │    OpenAiCompatible → POST /chat/completions; chat_completions::build_body()
 │    OpenAi           → POST /responses; openai::build_body()
 │    Codex            → codex::CODEX_API_ENDPOINT; openai::build_body() + tweaks
 │    Anthropic*       → POST /messages; anthropic::build_body()
 │    Copilot          → POST {proxy-ep}/chat/completions; chat_completions::build_body()
 ├─ Apply response_format if set (json_schema structured output)
 ├─ Set stream=true if on_delta provided (or always for Codex)
 └─ Retry loop (up to 9 attempts, exponential backoff 500ms→256s):
     ├─ Build reqwest::Request:
     │    Add auth headers (Bearer / x-api-key / Codex-turn-state / Copilot headers)
     │    Add anthropic-version: 2023-06-01
     ├─ tokio::select! { cancel.cancelled() | req.send() }
     ├─ On 4xx/5xx → ProviderError::from_http() → retry or return Err
     ├─ Codex 401 recovery: refresh OAuth tokens and retry once
     ├─ Copilot 401 recovery: refresh OAuth tokens and retry once
     └─ On success:
         if streaming:
           chat_completions::read_stream()  (SSE loop, calls on_delta per chunk)
           openai::read_stream()
           anthropic::read_stream()
         else:
           resp.json() → parse_response()
         → ParsedResponse { content, reasoning, tool_calls, usage }
         → LLMResponse { content, reasoning_content, tool_calls, usage, tokens_per_sec }
```

SSE streaming parsers (`crates/engine/src/provider/sse.rs` + provider-specific files) accumulate token deltas and call `on_delta` for each content chunk, so text appears progressively in the TUI.

---

## Phase 7: classify_tools() and Tool Execution (`crates/engine/src/agent.rs:1023`)

For each `ToolCall` from the LLM response:

```
classify_tools(tool_calls) → ToolExecutionPlan
 For each tc in tool_calls:
  ├─ emit EngineEvent::ToolStarted { call_id, tool_name, args }
  │       [TUI side: start_tool(), push to pending, fire Lua cell tool_start]
  ├─ Is it a Lua plugin tool (in self.tools)?
  │   YES:
  │   ├─ has_hooks? → emit ToolHooksRequest; push to pending_tool_hooks
  │   └─ is_sequential? → push to sequential_tools
  │                     → emit ToolDispatch; push to pending_tools
  └─ Is it a core/MCP tool (in dispatcher)?
      evaluate_hooks(name, args, mode):
        Decision::Allow → ready slot (run concurrently via dispatcher.dispatch())
        Decision::Deny  → push_tool_result("permission denied…", is_error=false)
        Decision::Error → push_tool_result(err, is_error=true)
        Decision::Ask   → emit RequestPermission; pending_perms slot
```

### execute_concurrent() (`crates/engine/src/agent.rs:1163`)

```
FuturesUnordered<dispatch future> for all ready slots
tokio::select! loop until outstanding == 0:
 ├─ cancel.cancelled() → break (cancelled=true)
 ├─ cmd_rx.recv():
 │   Cancel                → cancel.cancel()
 │   PermissionDecision:
 │     approved → launch dispatch future for slot
 │     denied  → synthetic denial result, outstanding -= 1
 │   ToolHooksResponse:
 │     Allow → launch dispatch or add to sequential
 │     Deny  → emit ToolFinished(denial), outstanding -= 1
 │     Error → emit ToolFinished(error), outstanding -= 1
 │     Ask   → emit RequestPermission; add to pending_tool_perms
 │   ToolResult (Lua plugin returned):
 │     emit ToolFinished; record result; outstanding -= 1
 │   CallCoreTool (Lua side-call via smelt.tools.call):
 │     dispatch core tool via side_futs
 │   Steer/Unsteer/SetAgentMode/SetReasoningEffort/SetModel → deferred
 ├─ futs.next() → core tool completed → completed[idx] = result; outstanding -= 1
 └─ side_futs.next() → send EngineEvent::CoreToolResult (resumes Lua coroutine)
```

After all concurrent tools finish, `run_sequential()` fires sequential tools one-at-a-time (used by `ask_user_question` and similar blocking tools).

---

## Phase 8: TUI Processes EngineEvents (`crates/tui/src/app/engine_events.rs`)

The TUI drains `engine.try_recv()` each main-loop tick. `handle_engine_event()` matches each event:

| EngineEvent | TUI action |
|---|---|
| `TextDelta { delta }` | `append_streaming_text(delta)` → transcript streaming block updated |
| `ThinkingDelta { delta }` | `append_streaming_thinking(delta)` → thinking block |
| `ToolArgsDelta {…}` | `cells.set_dyn("stream_delta", …)` → Lua observers notified |
| `Text { content }` | flush streaming text; `push_block(Block::Text)` |
| `Thinking { content }` | flush streaming thinking; `push_block(Block::Thinking)` |
| `ToolStarted {…}` | `start_tool()` → transcript tool widget created; Lua `tool_start` cell |
| `ToolOutput { chunk }` | `append_active_output(call_id, chunk)` → tool output appended |
| `ToolFinished {…}` | `finish_tool()` → tool widget finalized; Lua `tool_end` cell |
| `RequestPermission {…}` | `SessionControl::NeedsConfirm` → see permission flow below |
| `ToolDispatch {…}` | `handle_tool_call()` → `lua.execute_tool()` |
| `ToolHooksRequest {…}` | evaluate Lua hooks + permission policy → send `UiCommand::ToolHooksResponse` |
| `CoreToolResult {…}` | `lua.resolve_core_tool_call()` → resumes suspended Lua coroutine |
| `TokenUsage {…}` | update session costs, metrics, `tokens_used` cell |
| `Retrying {…}` | `working.begin(TurnPhase::Retrying)` → spinner updates |
| `CompactionComplete {…}` | `apply_compaction(messages)` → rebuild transcript from compacted history |
| `TitleGenerated {…}` | update session title + slug, task label |
| `TurnComplete {…}` | `set_history(messages)` → session committed; `SessionControl::Done` |
| `TurnError {…}` | `notify_error(message)`; `SessionControl::Done` |
| `TurnComplete` / `Done` | `dispatch_control(Done)` → `discard_turn(false)` → `finish_turn()` |

### finish_turn()

```
finish_turn(cancelled)
 ├─ sleep_inhibit.release()
 ├─ if cancelled: engine.send(UiCommand::Cancel)
 ├─ cells.set_dyn("turn_end", TurnEnd { cancelled }) + pump_lua()
 ├─ flush_streaming_thinking() + flush_streaming_text()
 ├─ finish_transcript_turn()
 ├─ if cancelled: restore queued_messages to prompt input
 │   else: clear_prompt_completer()
 ├─ session.turn_metas.push(meta)
 ├─ snapshot_tokens()
 ├─ save_session()
 └─ maybe_auto_compact()    → if context > threshold: send UiCommand::Compact
```

---

## Phase 9: Permission Flow

When `RequestPermission` arrives:

```
dispatch_control(SessionControl::NeedsConfirm(req))
 ├─ approvals.is_auto_approved(permissions, mode, tool_name, args, summary)
 │    → if yes: send_permission_decision(request_id, approved=true, None); return true
 ├─ permissions.decide(mode, tool_name, args, false)
 │    → Decision::Allow: send_permission_decision(approved=true); return true
 ├─ If user was typing recently (< 1500ms): defer to pending_dialogs queue
 └─ Open confirm dialog:
     confirms.register(req) → handle_id
     cells.set_dyn("confirm_requested", ConfirmRequested { handle_id, … })
     lua.fire_confirm_open(handle_id)
     [Lua: runtime/lua/smelt/dialogs/permissions.lua opens overlay dialog]
```

User choice → `resolve_confirm((choice, message), call_id, request_id, tool_name)`:
```
Yes        → send_permission_decision(approved=true)
Always (session/workspace) → add to approvals; send approved
AlwaysPatterns → add patterns to approvals; send approved
AlwaysDir  → add dir to approvals; send approved
No         → send_permission_decision(approved=false); finish_tool(Denied); return true (cancel)
```

The `UiCommand::PermissionDecision` goes back to `execute_concurrent()` in the engine task.

---

## Phase 10: Lua Tool Dispatch Flow

For `EngineEvent::ToolDispatch { request_id, call_id, tool_name, args }`:

```
handle_tool_call(request_id, call_id, tool_name, args)
 └─ lua.execute_tool(tool_name, args, request_id, call_id, ToolEnv, now)
     ├─ Look up Lua function registered with smelt.tools.register { execute = fn }
     ├─ Execute in Lua (synchronous or async coroutine)
     └─ Returns:
         Immediate { content, is_error }:
           engine.send(UiCommand::ToolResult { request_id, call_id, content, is_error })
         Pending:
           (Lua coroutine parked; resumed later by drive_lua_tasks() or lua_wakeup_rx)
           When it resolves → engine.send(UiCommand::ToolResult)
```

Lua tools can call back into core tools via `smelt.tools.call(name, args)` → `engine.send(UiCommand::CallCoreTool)` → `EngineEvent::CoreToolResult` → `lua.resolve_core_tool_call()` → resume coroutine.

---

## Phase 11: Rendering (`crates/tui/src/app/render_loop.rs`)

Called each iteration of the main loop as `render_normal(agent_running)`:

```
render_normal(agent_running)
 ├─ update_spinner()
 ├─ theme::populate_ui_theme()
 ├─ adjust_tail_scroll()         (freeze tail follow during selection/drag)
 ├─ ui.sync_scroll_links()
 │
 ├─ Layout pass:
 │    measure_prompt_rows()      (count rows needed for input + above)
 │    ui.set_layout(build_layout_tree(LayoutInput, statusline_win))
 │    layout::LayoutState::from_ui()  → record prompt rect, viewport_rows
 │
 ├─ sync_transcript_layer(width, viewport_rows, has_transcript_cursor):
 │    project_transcript_buffer(width, viewport_rows, scroll_top, show_thinking)
 │      → converts Transcript model to Buffer lines:
 │          For each Block in transcript:
 │            Block::User {text}        → render as markdown-ish text
 │            Block::Text {content}     → render as markdown (highlight inline code etc.)
 │            Block::Thinking {content} → render thinking lines (if show_thinking)
 │            Block::Tool {…}           → render tool header + output
 │            Block::StreamingText      → partial text (during inference)
 │            Block::StreamingThinking  → partial thinking
 │    Apply syntax highlighting (via syntect)
 │    Attach diff highlights, selection highlights
 │    Update TRANSCRIPT_WIN buffer + scroll
 │
 ├─ sync_prompt_above_layer(term_w, queued):
 │    compute_prompt_above() → model/reasoning bar, queued message count
 │
 ├─ sync_input_layer(prompt_rect, has_prompt_cursor):
 │    buf.ensure_rendered_at(content_width)   (smelt-buffer wrapping)
 │    input.sync_display_coords()             (cursor position)
 │    compute_input()                          (syntax highlights on prompt text)
 │    Set cursor shape (Block) if focused
 │
 ├─ sync_prompt_below_layer(term_w)
 ├─ working.set_paused(blocking_dialog_open)
 ├─ refresh_status_bar()     → Lua statusline sources via lua.tick_statusline()
 ├─ finalize_layer_rects()   → re-assert window focus
 ├─ sync_completer_overlay() → open/update picker overlay for / or ./ completions
 │
 └─ ui.render_with_paints(&mut stdout, paint_fn):
     ├─ smelt-term compositor: diff current frame against previous frame
     ├─ Generate minimal ANSI escape sequences (colors, cursor moves)
     ├─ For paint regions: invoke Lua paint callbacks (custom Lua-drawn surfaces)
     └─ stdout.flush()
```

The `smelt-term` compositor (`crates/term/src/compositor.rs`) tracks a previous `Snapshot` grid and a current `Surface` grid. Only cells that changed get emitted, which keeps rendering efficient even at 60 fps.

---

## Phase 12: Host Callback Channel (Middleware) (`crates/tui/src/app/host_dispatch.rs`)

The engine task can perform a synchronous RPC back to the TUI main thread via `host_tx`:

```
Turn::host_call(build_fn) → Option<Reply>
 ├─ (tx, rx) = oneshot::channel()
 ├─ host_tx.send(build_fn(tx))   — sends to TUI
 └─ rx.await.ok()                — blocks engine until TUI replies

TUI drain_host_calls() / dispatch_host_call(call):
 ProviderRequest { messages, reply }:
   run_middleware_chain(messages, "on_request", |s| &s.hooks.provider_request)
   → serialize messages to Lua, pass through each registered hook fn, deserialize
   → reply.send(mutated_or_none)

 ProviderResponse { message, reply }:
   run_middleware_chain(message, "on_response", |s| &s.hooks.provider_response)
   → reply.send(mutated_or_none)
```

This is how `smelt.provider.middleware { on_request=…, on_response=… }` in Lua can inspect or modify every message before it goes to the LLM and every response that comes back.

---

## Phase 13: Compaction

Auto-compaction fires at the end of a turn when `session_cost / context_window > threshold` (default 80%):

```
maybe_auto_compact() → maybe send UiCommand::Compact { history, instructions }

engine_task receives UiCommand::Compact:
 ├─ config.aux_or_primary(AuxiliaryTask::Compaction)
 │    → use auxiliary.compaction model if configured, else primary
 ├─ build_provider(…)
 └─ compact::run_compact(provider, history, model, instructions, cancel, opts).await
      → calls provider.chat() with a summarisation prompt
      → returns (Vec<Message>, TokenUsage)
 → emit EngineEvent::CompactionComplete { messages }

TUI handle_engine_event(CompactionComplete):
 └─ apply_compaction(messages)
     ├─ set_history(messages)
     ├─ Rebuild transcript from scratch
     └─ compact_epoch += 1
```

Mid-turn auto-compact (`Turn::maybe_compact()`) uses the same mechanism but runs inline, splicing the compacted history into `self.messages` and continuing the current turn.

---

## Auxiliary LLM Requests

Three fire-and-forget background tasks bypass the main turn loop:

| Task | Trigger | Sends | Receives |
|---|---|---|---|
| **Title generation** | End of first turn with messages | `UiCommand::GenerateTitle` | `EngineEvent::TitleGenerated` |
| **Btw** | `/btw` command | `UiCommand::Btw { question, history }` | `EngineEvent::BtwResponse` |
| **EngineAsk** | `smelt.engine.ask()` from Lua | `UiCommand::EngineAsk { id, system, messages, task }` | `EngineEvent::EngineAskResponse { id, content }` |

All three use `spawn_title_generation` / `spawn_btw_request` / `spawn_engine_ask` patterns: they spawn a `tokio::spawn` inside the engine task, make a non-streaming `provider.chat()` call, and send the result back via `event_tx`.

---

## Headless Mode (`smelt_core::headless_app`)

When `--headless` is set:
- No TUI, no terminal claimed, no Lua UI
- `HeadlessApp::run_oneshot(message, cancel)` sends `UiCommand::StartTurn`, then drains engine events in a simple loop
- Output: `HeadlessSink` writes `TextDelta`, `ToolStarted`, `ToolFinished` etc. to stdout in text or JSON format
- Same `engine_task`, same `Provider::chat()`, same tool dispatch — only the frontend differs

---

## Key Invariants

1. **Thread boundary**: Everything in `tui` (Lua, rendering, permission dialogs) runs on the main thread. The engine task is the only other tokio task that does I/O. The two communicate via unbounded channels — the engine never blocks on the TUI, and the TUI drains the engine non-blocking each tick (except for `host_call` which is a deliberate synchronous RPC).

2. **turn_id fencing**: Every `EngineEvent` that carries a `turn_id` is validated against `TurnState::turn_id`. Stale events from a cancelled turn are silently dropped.

3. **Message ownership**: The canonical `session.messages` list lives on the TUI. The engine receives a snapshot copy at `StartTurn`, works with it locally, and hands back the final committed list via `TurnComplete { messages }`. The TUI calls `set_history(messages)` to update the session.

4. **Lua coroutines**: Async Lua tools park as coroutines on the Lua VM. `drive_lua_tasks()` advances them each tick. The `lua_wakeup_rx` channel lets cross-thread operations (e.g. a completed `CallCoreTool`) wake up the main loop to drain parked coroutines immediately.

5. **UTF-8 safety**: All text mutations go through `smelt_buffer::text` helpers or `Buffer::text_mut()`. Raw Rust slice/drain on `source()` is forbidden in feature code to prevent panics on mid-character byte offsets.
