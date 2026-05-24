---
template: home.html
---

# What makes smelt different?

Most coding agents treat the terminal as an afterthought — a chat box tacked onto a web UI or buried inside an IDE panel. smelt is built the other way around: a fast, native terminal application that you configure and extend like Neovim.

## Terminal-native, not web-wrapped

smelt runs where you already work — in your terminal. No browser tabs, no Electron shells, no IDE lock-in. The interface is a purpose-built TUI with Vim-style navigation, fuzzy pickers, and a modal command palette that feels like a natural extension of your shell.

## Hackable like Neovim

Your entire setup lives in Lua — not a JSON schema or a GUI settings panel. Keymaps, themes, slash commands, custom tools, statusline segments, and event handlers are all code you write and version-control. If you can write a Neovim plugin, you can extend smelt.

## Hot reload, zero downtime

Edit `init.lua` or any plugin file and press `F5` — your changes apply instantly without losing the conversation, scroll position, or agent state. Iterate on your config while you work, the same way you iterate on code.

## Rust core, Lua surface

The heavy lifting happens in Rust: filesystem I/O, process management, HTTP, and the rendering loop are all compiled and memory-safe. Lua sits on top as a fast, lightweight glue layer for configuration and plugins. You get Neovim-level extensibility without sacrificing performance or reliability.

## Model agnostic by design

Switch between local models (Ollama), API providers (OpenAI, Anthropic), and subscription services (ChatGPT, GitHub Copilot, Kimi) at runtime with `/model`. No vendor lock-in, no forced cloud dependencies.

## Sessions that persist and branch

Every conversation is automatically saved. Resume later, fork to explore an alternate approach, or rewind to undo a bad turn. Long chats are kept manageable with automatic compaction that summarizes old context without destroying the transcript.

## Modes for every level of trust

Normal mode asks before every edit. Plan mode generates a read-only blueprint first. Apply mode auto-approves file changes while still confirming shell commands. Yolo mode gets out of your way entirely on trusted codebases. You choose the safety level per project, not per tool call.
