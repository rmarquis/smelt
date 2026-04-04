use super::highlight::{build_inline_diff_cache_ext, CachedInlineDiff};
use engine::tools::NotebookRenderData;
use protocol::{Message, TurnMeta};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

pub const RENDER_CACHE_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RenderCache {
    pub version: u32,
    pub session_hash: String,
    #[serde(default)]
    pub tool_outputs: HashMap<String, ToolOutputRenderCache>,
}

impl RenderCache {
    pub fn new(session_hash: String) -> Self {
        Self {
            version: RENDER_CACHE_VERSION,
            session_hash,
            tool_outputs: HashMap::new(),
        }
    }

    pub fn serialize(&self) -> Vec<u8> {
        use flate2::write::DeflateEncoder;
        use flate2::Compression;

        let payload = serde_json::to_vec(self).unwrap_or_default();
        let mut out = Vec::with_capacity(4 + payload.len() / 4);
        out.extend_from_slice(b"RCi1");
        let mut enc = DeflateEncoder::new(out, Compression::fast());
        std::io::Write::write_all(&mut enc, &payload).ok();
        enc.finish().unwrap_or_default()
    }

    pub fn deserialize(data: &[u8]) -> Option<Self> {
        use flate2::read::DeflateDecoder;
        use std::io::Read;

        if data.len() < 4 || &data[..4] != b"RCi1" {
            return None;
        }
        let mut dec = DeflateDecoder::new(&data[4..]);
        let mut payload = Vec::new();
        dec.read_to_end(&mut payload).ok()?;
        serde_json::from_slice(&payload).ok()
    }

    pub fn is_compatible(&self, session_hash: &str) -> bool {
        self.version == RENDER_CACHE_VERSION && self.session_hash == session_hash
    }

    pub fn get_tool_output(&self, call_id: &str) -> Option<&ToolOutputRenderCache> {
        self.tool_outputs.get(call_id)
    }

    pub fn insert_tool_output(&mut self, call_id: String, cache: ToolOutputRenderCache) {
        self.tool_outputs.insert(call_id, cache);
    }

    pub fn retain_history(&mut self, history: &[Message]) {
        let active: HashSet<&str> = history
            .iter()
            .filter_map(|msg| msg.tool_calls.as_ref())
            .flat_map(|calls| calls.iter().map(|call| call.id.as_str()))
            .collect();
        self.tool_outputs
            .retain(|call_id, _| active.contains(call_id.as_str()));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolOutputRenderCache {
    InlineDiff(CachedInlineDiff),
    NotebookEdit(CachedNotebookEdit),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedNotebookEdit {
    pub data: NotebookRenderData,
    pub diff: Option<CachedInlineDiff>,
}

pub fn session_render_hash(messages: &[Message], turn_metas: &[(usize, TurnMeta)]) -> String {
    let mut hasher = Sha256::new();
    if let Ok(bytes) = serde_json::to_vec(messages) {
        hasher.update(bytes);
    }
    if let Ok(bytes) = serde_json::to_vec(turn_metas) {
        hasher.update(bytes);
    }
    format!("{:x}", hasher.finalize())
}

pub fn build_tool_output_render_cache(
    name: &str,
    args: &HashMap<String, serde_json::Value>,
    content: &str,
    is_error: bool,
    metadata: Option<&serde_json::Value>,
) -> Option<ToolOutputRenderCache> {
    if is_error {
        return None;
    }
    match name {
        "edit_file" => {
            let old = args
                .get("old_string")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let new = args
                .get("new_string")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if new.is_empty() {
                return None;
            }
            let path = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            Some(ToolOutputRenderCache::InlineDiff(
                build_inline_diff_cache_ext(old, new, path, new, None),
            ))
        }
        "notebook_edit" => {
            let meta = metadata?;
            let data = serde_json::from_value::<NotebookRenderData>(meta.clone()).ok()?;
            let diff = if data.edit_mode == "insert" {
                None
            } else {
                Some(build_inline_diff_cache_ext(
                    &data.old_source,
                    &data.new_source,
                    &data.path,
                    &data.old_source,
                    Some(data.syntax_ext()),
                ))
            };
            Some(ToolOutputRenderCache::NotebookEdit(CachedNotebookEdit {
                data,
                diff,
            }))
        }
        // Preserve successful spawn_agent output lines exactly as returned.
        // No extra IR is needed here.
        _ => {
            let _ = content;
            None
        }
    }
}
