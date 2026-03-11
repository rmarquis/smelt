use super::{bool_arg, str_arg, timeout_arg, Tool, ToolContext, ToolFuture, ToolResult};
use protocol::EngineEvent;
use serde_json::Value;
use std::collections::HashMap;
use tokio::io::{AsyncBufReadExt, BufReader};

pub struct BashTool;

impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a bash command and return its output. The working directory persists between calls. Commands time out after 2 minutes by default (configurable up to 10 minutes). Use run_in_background for long-running processes."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Shell command to execute"},
                "timeout_ms": {"type": "integer", "description": "Timeout in milliseconds (default: 120000, max: 600000)"},
                "run_in_background": {"type": "boolean", "description": "Run the command in the background and return a process ID. Use read_process_output to check output and stop_process to kill it."}
            },
            "required": ["command"]
        })
    }

    fn needs_confirm(&self, args: &HashMap<String, Value>) -> Option<String> {
        Some(str_arg(args, "command"))
    }

    fn approval_pattern(&self, args: &HashMap<String, Value>) -> Option<String> {
        let cmd = str_arg(args, "command");
        let subcmds = crate::permissions::split_shell_commands_with_ops(&cmd);
        let mut result = String::new();
        for (subcmd, op) in &subcmds {
            let bin = subcmd.split_whitespace().next().unwrap_or("");
            if !bin.is_empty() {
                if !result.is_empty() {
                    result.push(' ');
                }
                result.push_str(bin);
                result.push_str(" *");
            }
            if let Some(op) = op {
                result.push_str(&format!(" {op}"));
            }
        }
        Some(result)
    }

    fn execute<'a>(
        &'a self,
        args: HashMap<String, Value>,
        ctx: &'a ToolContext<'a>,
    ) -> ToolFuture<'a> {
        Box::pin(async move {
            let command = str_arg(&args, "command");

            if bool_arg(&args, "run_in_background") {
                return execute_background(&command, ctx).await;
            }

            execute_streaming(&command, &args, ctx).await
        })
    }
}

async fn execute_background(command: &str, ctx: &ToolContext<'_>) -> ToolResult {
    match tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => {
            let id = ctx.processes.next_id();
            ctx.processes
                .spawn(id.clone(), command, child, ctx.proc_done_tx.clone());
            ToolResult {
                content: format!("background process started with id: {id}"),
                is_error: false,
            }
        }
        Err(e) => ToolResult {
            content: e.to_string(),
            is_error: true,
        },
    }
}

async fn execute_streaming(
    command: &str,
    args: &HashMap<String, Value>,
    ctx: &ToolContext<'_>,
) -> ToolResult {
    let timeout = timeout_arg(args, 120);

    let mut child = match tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return ToolResult {
                content: e.to_string(),
                is_error: true,
            }
        }
    };

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let mut stdout_reader = BufReader::new(stdout).lines();
    let mut stderr_reader = BufReader::new(stderr).lines();
    let mut output = String::new();
    let mut stdout_done = false;
    let mut stderr_done = false;

    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);

    loop {
        if stdout_done && stderr_done {
            break;
        }
        tokio::select! {
            line = stdout_reader.next_line(), if !stdout_done => {
                match line {
                    Ok(Some(line)) => {
                        let _ = ctx.event_tx.send(EngineEvent::ToolOutput {
                            call_id: ctx.call_id.to_string(),
                            chunk: line.clone(),
                        });
                        if !output.is_empty() { output.push('\n'); }
                        output.push_str(&line);
                    }
                    _ => stdout_done = true,
                }
            }
            line = stderr_reader.next_line(), if !stderr_done => {
                match line {
                    Ok(Some(line)) => {
                        let _ = ctx.event_tx.send(EngineEvent::ToolOutput {
                            call_id: ctx.call_id.to_string(),
                            chunk: line.clone(),
                        });
                        if !output.is_empty() { output.push('\n'); }
                        output.push_str(&line);
                    }
                    _ => stderr_done = true,
                }
            }
            _ = &mut deadline => {
                let _ = child.kill().await;
                return ToolResult {
                    content: format!("timed out after {:.0}s", timeout.as_secs_f64()),
                    is_error: true,
                };
            }
        }
    }

    let status = child.wait().await;
    let is_error = status.map(|s| !s.success()).unwrap_or(true);
    ToolResult {
        content: output,
        is_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(cmd: &str) -> String {
        let tool = BashTool;
        let mut args = HashMap::new();
        args.insert("command".into(), Value::String(cmd.into()));
        tool.approval_pattern(&args).unwrap()
    }

    #[test]
    fn simple_command() {
        assert_eq!(pattern("cargo build"), "cargo *");
    }

    #[test]
    fn chain_and() {
        assert_eq!(pattern("cargo fmt && cargo clippy"), "cargo * && cargo *");
    }

    #[test]
    fn chain_or() {
        assert_eq!(pattern("make || make install"), "make * || make *");
    }

    #[test]
    fn chain_semicolon() {
        assert_eq!(pattern("cd /tmp; rm -rf foo"), "cd * ; rm *");
    }

    #[test]
    fn pipe() {
        assert_eq!(pattern("cat file.txt | grep foo"), "cat * | grep *");
    }

    #[test]
    fn ls_and_rm() {
        assert_eq!(pattern("ls && rm README.md"), "ls * && rm *");
    }

    #[test]
    fn mixed() {
        assert_eq!(
            pattern("cd /tmp && rm -rf * | grep err; echo done"),
            "cd * && rm * | grep * ; echo *"
        );
    }

    #[test]
    fn background_operator() {
        assert_eq!(pattern("sleep 5 & echo done"), "sleep * & echo *");
    }

    #[test]
    fn quoted_operator_not_split() {
        assert_eq!(pattern(r#"grep "&&" file.txt"#), "grep *");
    }

    #[test]
    fn empty_command() {
        assert_eq!(pattern(""), "");
    }

    #[test]
    fn only_whitespace() {
        assert_eq!(pattern("   "), "");
    }
}
