use super::{str_arg, Tool, ToolContext, ToolFuture, ToolResult};
use serde_json::Value;
use std::collections::HashMap;

pub struct ExitPlanModeTool;

impl Tool for ExitPlanModeTool {
    fn name(&self) -> &str {
        "exit_plan_mode"
    }

    fn description(&self) -> &str {
        "Signal that planning is complete and ready for user approval. Call this when your plan is finalized."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "plan_summary": {
                    "type": "string",
                    "description": "A concise summary of the implementation plan for the user to approve."
                }
            },
            "required": ["plan_summary"]
        })
    }

    fn execute<'a>(
        &'a self,
        args: HashMap<String, Value>,
        _ctx: &'a ToolContext<'a>,
    ) -> ToolFuture<'a> {
        Box::pin(async move {
            let summary = str_arg(&args, "plan_summary");
            ToolResult {
                content: summary,
                is_error: false,
            }
        })
    }
}
