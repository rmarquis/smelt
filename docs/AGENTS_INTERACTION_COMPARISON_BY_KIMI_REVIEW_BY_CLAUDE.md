# Review of AGENTS_INTERACTION_COMPARISON_BY_KIMI.md

> **Reviewer:** Claude Sonnet 4.6, 2026-05-18.
> **Document reviewed:** `docs/AGENTS_INTERACTION_COMPARISON_BY_KIMI.md`
> **Basis for review:** The three Kimi source analyses (`*_BY_KIMI.md`), the three
> Claude source analyses (`*_BY_CLAUDE.md`), and direct reads of the source files
> (`claude-code/src/query.ts`, `claude-code/src/services/api/claude.ts`,
> `claude-code/src/services/tools/StreamingToolExecutor.ts`,
> `kimi-cli/src/kimi_cli/wire/__init__.py`,
> `kimi-cli/src/kimi_cli/wire/types.py`).

---

## Overall Assessment

The Kimi comparison is **solid work** — well-structured, confident where confidence is
warranted, and the section 14 philosophy capsules ("Lua-First, Rust-For-Performance" /
"Wire-Decoupled, Kosong-Centered" / "React-For-Terminals, SDK-Integrated") are the
sharpest single-sentence characterisations of these three systems I've read. The section 15
"When Each Design Shines" table adds genuine value; it's the kind of synthesis that's easy
to skip and easy to get wrong, and Kimi gets it mostly right.

The reliability concern is concentrated in section 8 (tool deduplication misattribution)
and one item in section 9 (a possible hallucination). Everything else is accurate or is
a defensible framing choice. I'd rate the document **B+/A-**: better structured than my
own comparison, with a smaller number of errors but errors that could mislead someone
trying to understand smelt's tool execution model.

---

## What I Agree With

### Philosophy summaries (section 14) are apt

The three one-liners are accurate shorthand for the dominant design concern in each codebase:

- Smelt's Lua-first design is not a scripting layer bolted on top — the boundary between
  Rust primitives and Lua behaviour is deliberate. Lua registers tools, commands, keymaps,
  themes, and plugins; Rust provides buffer editing, HTTP, rendering, and UTF-8 safety.
- kimi-cli's Wire protocol being the "architectural center" is exactly right. It is not
  just IPC between soul and UI — it is what makes the multi-frontend model (shell, print,
  ACP, web, vis, wire/stdio) possible without changing the agent. The soul does not know
  which frontend is consuming it.
- claude-code's "deep SDK integration" framing is correct. It is not a generic agent
  framework that happens to call Anthropic — it uses beta features (`cache_control`,
  `thinking`, `effort`, `task_budget`, `context_management`) that are not standardised
  across providers.

### Streaming architecture (section 6)

The observation that `kosong.generate()` is an async *function* with callbacks — not an
async generator — is correct and non-obvious. It's a meaningful distinction because the
callback model allows `on_tool_call` to fire mid-stream and dispatch tool tasks
concurrently while the stream continues, which is harder to express cleanly with a
generator. The comparison correctly contrasts this with claude-code's async generator
yield pattern.

### Section 15 table

"Team collaboration → Kimi-CLI" via subagent spawning and D-Mail is a real differentiator.
"Minimal dependencies / small binary → Smelt" is also correct — the Rust binary has no
runtime, no interpreter, no package manager. "Enterprise deployment → Claude Code" is
accurate: policy limits, remote managed settings, bridge mode, and daemon supervision are
absent from the other two.

### Section 13: process architecture

"React concurrent features: Not used — terminal rendering is synchronous" for claude-code
is accurate and often overlooked. The Ink reconciler runs synchronously; there is no
concurrent mode, Suspense, or transitions. React's concurrent scheduler is irrelevant in
this context.

### Section 7: agent core descriptions

The distinction between smelt's long-running engine actor (state maintained internally,
TUI gets snapshots) vs. kimi-cli's stateful soul object (delegates LLM interaction to
kosong) vs. claude-code's query loop (state in `QueryEngine` wrapping a generator) is
drawn correctly.

---

## Where I Disagree or Would Push Back

### 1. Tool deduplication is misattributed to smelt (section 8) — factual error

The table row reads:

> Smelt — Deduplication: "Same-step dedup in engine; cross-step nag"

This is kimi-cli's behaviour, not smelt's. The mechanism lives in
`KimiToolset.handle()` (`kimi-cli/src/kimi_cli/soul/toolset.py`):
`begin_step()` / `end_step()` track a `(name, arguments)` hash per step; a second
identical call within the same step awaits the first task's result instead of executing
again; a call identical to one from the previous step executes but appends a "nag
reminder" to context.

Smelt has `result_dedup::duplicate_of` in `Turn::collect_results()`, which deduplicates
*tool results* — it prevents the same result *content* from being inserted into the
message history twice if two tool calls happen to produce identical output. This is a
different mechanism solving a different problem (avoiding redundant context tokens, not
avoiding redundant executions). It is not "same-step dedup" in the kimi-cli sense.

The confusion likely stems from both mechanisms appearing in the same section of their
respective analyses, but they should not be conflated.

**Consequence:** anyone relying on this table to understand smelt's tool execution model
will expect a deduplication guard that does not exist there.

### 2. Theme API attributed to wrong system (section 11) — likely copy-paste error

The extensibility table says:

> Kimi-CLI — Theme customization: `smelt.theme.set()`

`smelt.*` is smelt's Lua API surface. kimi-cli does not expose a `smelt.theme.set()`
function. kimi-cli's theming works through the `/theme` slash command, which raises a
`Reload` exception that restarts the application with the new theme applied. There is
no programmatic theme API exposed to user scripts in kimi-cli.

The smelt row correctly says `smelt.theme.use()`. The kimi-cli row appears to have
inherited this by mistake.

### 3. kimi-cli's `SandboxManager` is unverified (section 9) — possible hallucination

Section 9 states:

> kimi-cli — Sandbox: `SandboxManager`

A `SandboxManager` does not appear anywhere in the kimi-cli analysis document
(`KIMICLI_CODE_INTERACTION_BY_KIMI.md`), nor in the source files I read. The claude-code
entry for the same row ("SandboxManager with unsandboxed command guards") is plausible —
claude-code has shell execution guarding. For kimi-cli, the `Shell` tool uses
`approval: Approval` for permission gating, not a sandbox abstraction. This claim should
be verified against the source before being cited.

### 4. Smelt's auto-compact is asserted more confidently than the evidence warrants (section 12)

The at-a-glance table says:

> Smelt — Context Compaction: "Auto-compact in engine turn loop"

The underlying smelt analysis documents a `Compacted` block type in `BlockHistory` and a
`/compact` Lua command. It does not document an automatic trigger inside `Turn::run()`.
The comparison upgrades this to auto-compact without evidence for the trigger condition or
threshold. By contrast, kimi-cli's `should_auto_compact()` function inside `_agent_loop()`
is explicitly described. Smelt may have only manual compaction (`/compact`), which would
make the at-a-glance characterisation misleading.

### 5. `query()` is described as "stateless" but carries internal mutable state (section 7)

Section 7 says:

> Claude Code's `query()` is a **stateless async generator** — no side effects except
> yielding — state lives in `QueryEngine` which wraps it

This is externally true but internally wrong. `query()` carries a mutable `State` struct
across loop iterations:

```typescript
type State = {
  messages: Message[]
  maxOutputTokensRecoveryCount: number   // survives across iterations
  hasAttemptedReactiveCompact: boolean
  maxOutputTokensOverride: number | undefined
  pendingToolUseSummary: ...
  stopHookActive: boolean | undefined
  turnCount: number
  transition: Continue | undefined       // records why the previous iteration continued
  ...
}
```

`QueryEngine` does not see this state — it is loop-local. But calling `query()` stateless
implies callers can restart it from any point, which is not true: `maxOutputTokensRecoveryCount`
(capped at `MAX_OUTPUT_TOKENS_RECOVERY_LIMIT = 3`) and the `hasAttemptedReactiveCompact`
flag are the reason recovery paths fire at most once per turn. The state is why the generator
is *single-use*, not restartable. "Externally stateless, internally stateful" is the accurate
framing.

### 6. Smelt's retry logic is characterised as "Basic (via reqwest)" — understated (section 5)

The retry table says:

> Smelt — Retry logic: Basic (via reqwest)

The cancellation token is polled inside a `tokio::select!` on every SSE chunk, which is
deliberate and not "reqwest retry". More importantly, the table gives claude-code's retry
logic three lines of detail (fallback model, auth refresh, persistent retry mode) while
smelt gets one dismissive phrase. This may be a gap in the underlying smelt analysis
rather than the comparison's fault — but the asymmetry risks underselling smelt's
resilience story when the analysis simply didn't dig into it.

---

## What Is Missing

### Smelt's synchronized update markers

Each compositor frame is wrapped in `\x1b[?2026h` / `\x1b[?2026l` (synchronized update
begin/end). This prevents the terminal from rendering partial frames during the diff flush,
which is the main cause of visual tearing in high-frequency rendering loops. Neither the
comparison nor the underlying analysis mentions this, even though it is a meaningful
quality-of-life detail absent from the other two systems.

### kimi-cli's D-Mail / `BackToTheFuture` as a history rewind mechanism

Section 15 mentions D-Mail as "inter-agent messaging", which is accurate but incomplete.
`BackToTheFuture` is a Python exception that unwinds the asyncio call stack back to a
named checkpoint in the conversation history — it is a *time-travel* mechanism, not just a
message-passing mechanism. This is qualitatively different from claude-code's
`TombstoneMessage` (a render placeholder that marks a deleted message in the transcript)
and from smelt's undo ring (which operates on the prompt buffer, not the conversation
history). The comparison collapses all three into a single "History rewind" row without
surfacing this difference.

### claude-code's `querySource` partitioning as a correctness invariant

The fact that `querySource = 'compact'` causes the query loop to skip the blocking-limit
check is not a performance optimisation — it is a correctness invariant. If the compact
agent were blocked by the same token limit it is trying to fix, it would deadlock. This
is the kind of non-obvious invariant that distinguishes a mature system from a simple one,
and it is absent from the comparison.

### claude-code's `task_budget` is distinct from the token budget

Section 12 conflates "token tracking" and "token budget" for claude-code. The `task_budget`
parameter (`output_config.task_budget`, behind the `task-budgets-2026-03-13` beta) counts
*context tokens consumed by the server* across compaction boundaries and tells the server
how much of the total budget has been spent. The `+500k auto-continue` feature
(`checkTokenBudget()`) is a different mechanism that detects when the model's output was
cut short and continues the turn. These are orthogonal; conflating them obscures how
claude-code manages cost across long multi-compaction runs.

---

## Summary

| Category | Verdict |
|----------|---------|
| Philosophy summaries (§14) | Accurate and sharp |
| Streaming architecture (§6) | Accurate |
| Agent core descriptions (§7) | Accurate, one internal nuance missed |
| Tool deduplication row (§8) | **Factual error** — kimi-cli behaviour attributed to smelt |
| Theme API row (§11) | **Copy-paste error** — `smelt.theme.set()` attributed to kimi-cli |
| kimi-cli SandboxManager (§9) | **Unverified** — not in source analysis, possible hallucination |
| Smelt auto-compact (§12) | Overstated — evidence supports only manual compaction |
| `query()` stateless claim (§7) | Framing is misleading — internally stateful loop |
| Smelt retry logic (§5) | Understated by comparison to claude-code coverage |
| Synchronized update markers | Missing |
| D-Mail as history rewind | Understated — collapsed into generic "inter-agent messaging" |
| `querySource` invariant | Missing |
| `task_budget` vs token budget | Conflated |
