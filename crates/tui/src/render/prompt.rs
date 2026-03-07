pub(super) struct PromptState {
    pub drawn: bool,
    pub dirty: bool,
    pub prev_rows: u16,
    /// Where the next frame starts drawing. Updated at the end of every
    /// `draw_frame` call (always fresh). On first frame or after clear,
    /// falls back to `cursor::position()` once.
    pub anchor_row: Option<u16>,
    /// Computed each frame inside `draw_frame`, exposed via `dialog_row()`
    /// getter for the app loop.
    pub prev_dialog_row: Option<u16>,
}

impl PromptState {
    pub fn new() -> Self {
        Self {
            drawn: false,
            dirty: true,
            prev_rows: 0,
            anchor_row: None,
            prev_dialog_row: None,
        }
    }
}
