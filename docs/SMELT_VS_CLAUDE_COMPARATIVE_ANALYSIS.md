# Smelt vs Claude Code: Comparative Analysis of Agent Effectiveness

> **Goal**: Understand what makes a coding agent effective, and how the balance between hardcoded rules and model autonomy shapes that effectiveness.  
> **Sources**: Smelt (`crates/`, `runtime/lua/`) and Claude Code (`restored-src/src/`).  
> **Date**: 2026-05-13

---

## 1. Executive Summary

| Dimension | Smelt | Claude Code |
|-----------|-------|-------------|
| **Language** | Rust engine + Lua runtime | TypeScript monolith |
| **System prompt size** | ~50 lines (template) + sections | ~800 lines (hardcoded) + dynamic env |
| **Tool count** | 12 built-in + plugins | 30+ built-in + MCP + deferred |
| **Permission system** | Deterministic hooks (Lua) | 7-step pipeline + LLM classifier |
| **Compaction** | 1 strategy (auxiliary LLM) | 5 strategies (auto, reactive, micro, snip, collapse) |
| **Subagents** | None | Built-in (Explore, Plan, Verify, Fork, Custom) |
| **Mid-turn steering** | Yes (`Steer` command) | No direct equivalent |
| **Tool execution** | Concurrent + sequential | Streaming + concurrent + sequential |
| **Plugin extensibility** | Lua plugins (user-writable) | Internal only (no user plugins) |

**Thesis**: Both agents use extensive hardcoded rules, but they differ fundamentally in *how* those rules are enforced and *what* is left to the model. Smelt constrains the model through **tool visibility** and **deterministic permission hooks**. Claude Code constrains the model through **prose instructions** and, critically, a **secondary LLM classifier** that acts as a probabilistic gate. The most effective approach appears to be: **hardcode invariants, structure constraints via schemas/filtering, and reserve model autonomy for tactics within safe boundaries**.

---

## 2. Architecture Philosophy

### 2.1 Smelt: Engine + Runtime Separation

Smelt separates concerns into:
- **Rust engine** (`crates/engine/`): Handles the LLM loop, HTTP, provider abstraction, message serialization
- **Rust TUI** (`crates/tui/`): Terminal UI, prompt parsing, attachment handling
- **Lua runtime** (`runtime/lua/smelt/`): Tool definitions, prompt sections, agent behavior plugins

**Implication**: Users can write Lua plugins to add tools, mutate the system prompt, and change agent behavior without recompiling. The engine is a stable substrate; behavior lives in the runtime.

### 2.2 Claude Code: Monolithic TypeScript

Claude Code is a single TypeScript/React codebase. Tools are TypeScript classes with ~30 methods each. Behavior is controlled via:
- Feature flags (`feature('FLAG')`)
- Build-time defines (`process.env.USER_TYPE === 'ant'`)
- Settings and permission rules

**Implication**: All behavior is internal. Users can configure permissions and rules, but cannot add tools or change the system prompt structure (except via `CLAUDE.md` and `appendSystemPrompt`).

### 2.3 Effectiveness Implication

| Aspect | Smelt's Approach | Claude Code's Approach | Winner |
|--------|-----------------|----------------------|--------|
| **Customizability** | High (Lua plugins) | Low (rules only) | Smelt |
| **Consistency** | Varies by plugin quality | High (single codebase) | Claude Code |
| **Safety auditability** | Harder (distributed Lua) | Easier (centralized TS) | Claude Code |
| **Iteration speed** | Fast (Lua hot-reload) | Slow (rebuild + deploy) | Smelt |
| **Tool richness** | Moderate (10 hooks) | High (30 methods) | Claude Code |

---

## 3. System Prompt Design: Template vs. Monolith

### 3.1 Smelt: Template-Driven, Mutable Sections

```
base prompt (template: prompts/system.txt)
+ behavior section (interactive/autonomous)
+ write_access section (conditional on mode)
+ skills section (injected by skill loader)
+ instructions section (user-provided)
= assembled prompt
```

**Lua mutation**: `smelt.prompt.set_section("plan_mode", PLAN_PROMPT)` injects read-only constraints.

**Size**: ~2-3 KB rendered. Compact. The template uses Jinja2-style conditionals (`{% if write_access %}`).

### 3.2 Claude Code: Massive Hardcoded Prompt

```
static sections (~2KB, cacheable globally):
  - intro, system, doing tasks, actions, using tools, tone, output efficiency
dynamic sections (~3-5KB, per-session):
  - session guidance, memory, env info, language, output style, MCP instructions, scratchpad, etc.
```

**Size**: ~5-8 KB total. The `prompts.ts` file alone is 53 KB of source code generating the prompt.

**Key difference**: Claude Code's prompt contains **extremely specific behavioral rules**:
- "Don't add features, refactor code, or make 'improvements' beyond what was asked"
- "Default to writing no comments. Only add one when the WHY is non-obvious"
- "Report outcomes faithfully: if tests fail, say so... Never claim 'all tests pass' when output shows failures"
- "Before reporting a task complete, verify it actually works"

### 3.3 Effectiveness Analysis

**Hardcoded prose rules** have a U-shaped effectiveness curve:

| Rule Type | Effectiveness | Why |
|-----------|-------------|-----|
| **Workflow rules** ("use Read before Edit") | High | Prevents predictable failure modes |
| **Style rules** ("no emojis", "file_path:line_number") | High | Consistent output, easier parsing |
| **Safety rules** ("ask before destructive ops") | High | Non-negotiable boundaries |
| **Over-specific rules** ("keep text ≤25 words between tool calls") | Low/Medium | Creates rigidity; model may follow letter not spirit |
| **Anti-pattern rules** ("don't add comments") | Medium | Depends heavily on model; may be ignored or over-applied |
| **Verification rules** ("run tests before claiming success") | Medium/High | Good intent, but model may hallucinate success |

**Observation**: Claude Code's prompt is ~10x larger than Smelt's. Does more instruction = better behavior? The evidence is mixed:
- **Pro**: More instructions cover more edge cases; the model has less room to "improvise" badly
- **Con**: Large prompts consume context window; conflicting instructions confuse the model; excessive constraints make the agent feel robotic

**Smelt's leaner prompt** trusts the model more but constrains it structurally (via tool visibility and deterministic hooks). **Claude Code's massive prompt** tries to specify every behavioral dimension in prose.

---

## 4. Tool Visibility: Structural Constraints vs. Prose Instructions

### 4.1 Smelt: Tool Filtering as Structural Constraint

```rust
// Tools are filtered PER ITERATION
let tool_defs: Vec<ToolDefinition> = if self.provider.tool_calling() {
    let mut defs = self.dispatcher.definitions()
        .into_iter()
        .filter(|d| self.dispatcher.is_visible(d.function.name.as_str(), self.mode))
        .collect();
    // Plugin tools filter by mode
    for pt in &self.tools {
        if let Some(ref modes) = pt.modes {
            if !modes.contains(&self.mode) { continue; }
        }
        defs.push(...);
    }
    defs
} else { Vec::new() };
```

**Key insight**: The model **cannot use tools it doesn't see**. This is a **hard structural constraint** — no amount of model creativity can bypass it. The `exit_plan_mode` tool is literally invisible outside Plan mode.

### 4.2 Claude Code: Tool Filtering + Deferred Loading

```typescript
export function assembleToolPool(permissionContext, mcpTools): Tools {
    const builtInTools = getTools(permissionContext);  // filtered by deny rules, isEnabled
    const allowedMcpTools = filterToolsByDenyRules(mcpTools, permissionContext);
    return uniqBy([...builtInTools].sort(byName).concat(allowedMcpTools.sort(byName)), 'name');
}
```

**Additional**: Tool Search enables **deferred loading**:
```typescript
// Non-essential tools sent with defer_loading: true
const toolSchemas = await Promise.all(
    filteredTools.map(tool => toolToAPISchema(tool, { deferLoading: willDefer(tool) }))
);
```

The model must **discover** deferred tools via `tool_reference` blocks before they become callable.

### 4.3 Effectiveness Analysis

| Approach | Strength | Weakness |
|----------|----------|----------|
| **Smelt: mode-gated visibility** | Absolute guarantee; simple mental model | Coarse-grained (all-or-nothing per mode) |
| **Claude Code: deny rules + deferred** | Fine-grained user control; scales to unlimited tools | Complex; model can still see and misuse non-deferred tools |
| **Hybrid: structural + prose** | Best of both | Most complex to implement |

**Winner for safety**: Smelt's mode-gated visibility. The model physically cannot call `write_file` in Plan mode because the tool schema is absent from the prompt.

**Winner for scalability**: Claude Code's deferred loading. Supports hundreds of MCP tools without context bloat.

---

## 5. Permission Philosophy: Deterministic Hooks vs. LLM Classifier

This is the **most consequential difference** between the two agents.

### 5.1 Smelt: Deterministic Permission Hooks

Each tool defines hooks evaluated at call time:

```lua
-- bash.lua
decide = function(args, mode)
    local tool = smelt.permissions.check_tool(mode, "bash")
    if tool == "deny" then return "deny" end
    local sub = smelt.permissions.check(mode, "bash", args.command or "")
    if sub == "deny" then return "deny" end
    if tool == "allow" and sub == "ask" then return "ask" end
    return sub
end,

approval_patterns = function(args)
    -- Returns patterns like "git *" for auto-approval
end,

confirm_text = function(args)
    return args.command or ""
end,
```

**Pipeline**:
1. `decide()` → `allow` / `ask` / `deny`
2. If `ask`: show confirm dialog with `confirm_text` + `approval_patterns`
3. If `allow`: execute immediately

**Characteristics**:
- **Deterministic**: Same input → same decision, every time
- **Transparent**: Users can read the Lua and understand the logic
- **Fast**: No extra LLM call
- **Predictable**: The model knows which tools are auto-allowed vs. ask

### 5.2 Claude Code: Multi-Layer Pipeline + LLM Classifier

**Layer 1: Deterministic rules** (steps 1a-1g):
```
1a. Deny rule? → DENY
1b. Ask rule? → ASK
1c. Tool.checkPermissions() → allow/ask/deny/passthrough
1d. Tool denied? → DENY
1e. Requires interaction? → ASK
1f. Content-specific ask rule? → ASK
1g. Safety check? → ASK (bypass-immune)
```

**Layer 2: Mode transformation**:
```
bypassPermissions → ALLOW (except safety)
acceptEdits → auto-allow file edits
dontAsk → ASK becomes DENY
auto → RUN CLASSIFIER
```

**Layer 3: Auto mode classifier** (a **secondary LLM**):
```typescript
// Fast-path 1: acceptEdits check
if (acceptEditsResult.behavior === 'allow') → ALLOW

// Fast-path 2: safe-tool allowlist
if (isAutoModeAllowlistedTool(tool.name)) → ALLOW

// Main path: classifyYoloAction()
classifierResult = await classifyYoloAction(
    context.messages,   // FULL conversation history
    action,             // formatted tool use
    context.options.tools,
    permissionContext,
    signal
)
// Two-stage: quick check → full analysis if needed
// Returns: allowed / blocked (with reason) / unavailable
```

**Characteristics**:
- **Probabilistic for auto mode**: The classifier LLM may approve or deny the same action inconsistently
- **Opaque**: The classifier's reasoning is not directly inspectable
- **Expensive**: Extra LLM call per tool use in auto mode (~100-500ms latency, extra tokens)
- **Fail-closed**: If classifier unavailable → DENY (configurable)
- **Adaptive**: Tracks consecutive denials; falls back to prompting after threshold

### 5.3 Effectiveness Analysis: Hardcoded Rules vs. Model Decision

**Question**: When should you constrain the agent with hardcoded rules, and when should you let the model decide?

#### When Hardcoded Rules Win

| Scenario | Example | Why Rules Work |
|----------|---------|---------------|
| **Safety invariants** | "Never write to .git/" | Non-negotiable; deterministic; no LLM hallucination risk |
| **Predictable workflows** | "Read before Edit" | Prevents a known failure mode at 100% reliability |
| **Permission boundaries** | "Bash(`rm -rf`) → ask" | User must approve destructive actions; no false negatives allowed |
| **Token efficiency** | "Use Glob instead of `find`" | Saves tokens; the model doesn't need to reason about it |
| **Consistency** | "Always use file_path:line_number" | Users rely on this pattern for navigation |

#### When Model Autonomy Wins

| Scenario | Example | Why Model Autonomy Works |
|----------|---------|------------------------|
| **Novel situations** | Unusual project structure | Rules can't cover every case; model adapts |
| **Context-dependent trade-offs** | "Should I run tests or read more first?" | Depends on confidence, time, user patience |
| **Exploratory tasks** | "Find the bug" | Rigid rules would prevent creative debugging |
| **Natural language nuance** | "Make it cleaner" | Rules can't specify what "cleaner" means |
| **Multi-step planning** | Tool call ordering | Model reasons about dependencies better than static rules |

#### The Classifier Approach: Best of Both Worlds?

Claude Code's auto mode classifier attempts to get the benefits of both:
- **Hardcoded rules** for the 90% case (fast-path checks, allowlists, deny rules)
- **LLM classifier** for the 10% edge case where context matters

**Does it work?** The telemetry suggests mixed results:
- The classifier has **two stages** (quick + full), implying the quick stage catches obvious cases
- There's a **fail-closed gate** (`tengu_iron_gate_closed`), suggesting they don't fully trust it
- **Consecutive denial tracking** with fallback to prompting suggests false positives are a problem
- The classifier prompt includes the **full conversation history**, making it expensive

**Smelt's approach is simpler but less nuanced**: A tool's `decide()` hook has access to `args` and `mode`, but not the full conversation history. It can't say "this Bash command looks safe because the user just asked for it explicitly."

### 5.4 The Effectiveness Spectrum

```
Fully Constrained ←————————————————————→ Fully Autonomous

Smelt's hooks          Claude Code's      Claude Code's
(mode + decide)        deterministic      auto mode
                       pipeline           classifier
                       (bypass/dontAsk)

• Predictable          • Flexible         • Context-aware
• Fast                 • User-controlled  • Adaptive
• Simple               • Transparent      • Expensive
• Brittle edge cases   • Manual effort    • Probabilistic
```

**Recommendation**: The most effective permission system combines:
1. **Structural constraints** (tool visibility) for coarse safety
2. **Deterministic rules** (deny/ask/allow) for common cases
3. **LLM classifier** only for high-context decisions where false positives are acceptable
4. **Always allow user override** — the human is the ultimate authority

---

## 6. Compaction Strategies

### 6.1 Comparison

| Strategy | Smelt | Claude Code |
|----------|-------|-------------|
| **Trigger** | Token threshold (80% of context window) | Multiple: token threshold, 413 error, media size, proactive |
| **Mechanism** | Auxiliary LLM call (no tools) | 5 strategies: auto, reactive, micro, snip, collapse |
| **Preserves** | System prompt + recent user message | Configurable; preserves critical attachments |
| **Cost** | 1 LLM call per compaction | Variable; microcompact uses cache editing (no LLM) |
| **Recovery** | None (compaction is final) | Reactive compact can recover from 413 errors |

### 6.2 Effectiveness

Claude Code's **multi-strategy approach** is more effective for long sessions:
- **Microcompact** (cache editing) is essentially free — the server deletes old tool results
- **Reactive compact** recovers from API errors without user intervention
- **Context collapse** preserves granular history while compressing search operations

Smelt's **single strategy** is simpler but less resilient:
- If compaction fails, the turn errors out
- No recovery from prompt-too-long mid-turn

**Insight**: Context window management is a **hard constraint** that benefits from multiple redundant strategies. The "best" compaction is the one that fires before the API rejects the request.

---

## 7. Mid-Turn Mutability

### 7.1 Smelt: Steering

Smelt allows **mid-turn steering**:
- User sends a message while the agent is working
- The `Steer` command injects a new user message into history
- The current LLM call is allowed to finish
- The loop continues, and the model responds to the steer instead

**Effectiveness**: High for real-time collaboration. The user can course-correct without waiting for the turn to finish.

### 7.2 Claude Code: No Direct Equivalent

Claude Code has no mid-turn user message injection. The user must:
- Cancel the current turn (`Ctrl+C`)
- Wait for tool execution to finish
- Submit a new message

**Exception**: The `AskUserQuestion` tool allows sequential user dialogs, but these are **model-initiated**, not user-initiated.

**Effectiveness**: Lower for real-time collaboration; higher for deterministic turn completion.

---

## 8. Subagents and Delegation

### 8.1 Smelt: No Subagents

Smelt has no built-in subagent system. The agent runs in a single loop with a single context window.

### 8.2 Claude Code: Rich Subagent Ecosystem

Claude Code has a sophisticated subagent system:
- **Explore agent**: Read-only research (keeps main context clean)
- **Plan agent**: Read-only planning with `ExitPlanMode` tool
- **Verify agent**: Adversarial verification of implementation
- **Fork agent**: Background execution with output excluded from context
- **Custom agents**: User-defined via `.claude/agents/` frontmatter

**Effectiveness**: Subagents are a **force multiplier**:
- They parallelize work
- They protect the main context window
- They enforce read-only constraints (Explore/Plan can't write)
- They enable verification (another LLM checks the first LLM's work)

**Trade-off**: Subagents add latency, cost, and complexity. The main agent must delegate effectively — poor delegation wastes tokens.

---

## 9. What Makes an Agent Effective? A Framework

Based on the comparison, we propose a framework for agent effectiveness:

### 9.1 The Three Pillars

```
         EFFECTIVENESS
              /|\
             / | \
            /  |  \
    SAFETY   RELIABILITY   UTILITY
    (don't   (do what you   (help the user
     harm)    say you'll do)  achieve goals)
```

### 9.2 How Constraints Map to Pillars

| Pillar | Hardcoded Rules Help | Model Autonomy Helps |
|--------|---------------------|---------------------|
| **Safety** | Tool visibility, deny rules, safety checks, deterministic hooks | Context-aware risk assessment (classifier) |
| **Reliability** | Workflow rules (read before edit), verification instructions | Adapting to project-specific conventions |
| **Utility** | Tool definitions, parallel execution | Creative problem-solving, novel approaches |

### 9.3 The Optimal Balance

**Our recommendation** (synthesized from both approaches):

1. **Structural constraints > prose rules**
   - Use tool visibility, schema validation, and mode filtering instead of "please don't do X" instructions
   - Example: Hide `write_file` in Plan mode (Smelt) rather than saying "don't write files in plan mode" (Claude Code tries both)

2. **Deterministic gates > probabilistic classifiers for safety**
   - Use explicit `decide()` hooks or permission rules for high-stakes decisions
   - Reserve LLM classifiers for ambiguous cases where false positives are acceptable
   - Always allow user override

3. **Lean system prompt + rich tool definitions**
   - The system prompt should specify **what** and **why**, not **how**
   - Tool schemas and descriptions should encode the "how"
   - Example: Smelt's `edit_file` schema enforces uniqueness; Claude Code's prompt says "use Edit for modifications"

4. **Multiple compaction strategies**
   - Context window is the ultimate bottleneck
   - Layer proactive, reactive, and server-side strategies
   - Prefer server-side deletion (cache editing) over LLM summarization when possible

5. **Subagents for separation of concerns**
   - Read-only research should not share a context window with write operations
   - Verification should be independent
   - Forks should not pollute the main transcript

6. **Mid-turn mutability for collaboration**
   - Users should be able to steer the agent without canceling
   - The model should handle injected context gracefully

---

## 10. Specific Recommendations for Smelt

Based on Claude Code's strengths, Smelt could consider:

1. **Add a verification tool**: After `write_file` or `edit_file`, optionally spawn a read-only verifier to check the change
2. **Explore deferred tool loading**: For large plugin ecosystems, consider `ToolSearch`-style deferred loading
3. **Add reactive compaction**: If the API returns 413, attempt emergency summarization before surfacing the error
4. **Consider a lightweight classifier**: For `auto` mode, a small/fast model could approve obvious read-only operations without deterministic hooks needing to enumerate every safe case
5. **Tool result budget**: Persist oversized tool results to disk automatically (Claude Code does this)

## 11. Specific Recommendations for Claude Code

Based on Smelt's strengths, Claude Code could consider:

1. **Simplify the system prompt**: Test whether a leaner prompt (like Smelt's ~2KB) with stronger structural constraints achieves equivalent behavior
2. **Make tool definitions more inspectable**: Users can't see or modify `checkPermissions()` logic; Lua-style hooks would increase transparency
3. **Add mid-turn steering**: Allow users to inject messages without canceling
4. **Reduce classifier dependency**: The auto-mode classifier is expensive and opaque; more deterministic fast-paths could reduce reliance on it
5. **Plugin system**: Users want to add tools; a Lua or WASM runtime would enable this

---

## 12. Conclusion

Both Smelt and Claude Code are effective agents, but they optimize for different trade-offs:

- **Smelt** optimizes for **simplicity, transparency, and extensibility**. It uses structural constraints (tool visibility, deterministic hooks) and a lean system prompt. It trusts the model within safe boundaries.

- **Claude Code** optimizes for **control, consistency, and scale**. It uses massive prose instructions, a multi-layer permission pipeline, and a secondary LLM classifier. It constrains the model extensively and verifies its work.

**The most effective agent** likely lies in the middle:
- **Structural constraints** for safety and reliability
- **Lean prompts** for token efficiency and model flexibility
- **Deterministic hooks** for common permission decisions
- **LLM classifiers** only for genuinely ambiguous cases
- **Subagents** for separation of concerns
- **User override** as the ultimate authority

The lesson is not "more rules = better agent." The lesson is **"the right kind of constraints in the right places."** Hardcode the invariants, structure the workflows, and let the model breathe everywhere else.
