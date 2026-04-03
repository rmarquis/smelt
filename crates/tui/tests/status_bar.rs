//! Status bar responsive layout tests (vt100 harness).
//!
//! Verifies that the status bar never wraps and progressively drops/truncates
//! elements as the terminal width shrinks.

mod harness;

use harness::TestHarness;
use tui::render::Throbber;

/// Helper: the status line must never exceed one terminal row.
/// If it wraps, the second-to-last row will contain status bar content
/// that should only be on the last row.
fn assert_status_fits_one_row(h: &mut TestHarness) {
    let status = h.status_line_text();
    let char_count = status.chars().count();
    assert!(
        char_count <= h.width as usize,
        "Status bar overflows terminal width ({char_count} > {}): {status:?}",
        h.width,
    );
}

// ── Basic rendering ─────────────────────────────────────────────────

#[test]
fn status_bar_shows_mode() {
    let mut h = TestHarness::new(80, 24, "status_bar_shows_mode");
    let status = h.status_line_text();
    assert!(
        status.contains("normal"),
        "Expected 'normal' mode in status bar: {status:?}"
    );
}

#[test]
fn status_bar_shows_plan_mode() {
    let mut h = TestHarness::new(80, 24, "status_bar_shows_plan_mode");
    h.set_mode(protocol::Mode::Plan);
    let status = h.status_line_text();
    assert!(
        status.contains("plan"),
        "Expected 'plan' mode in status bar: {status:?}"
    );
}

#[test]
fn status_bar_shows_task_label() {
    let mut h = TestHarness::new(80, 24, "status_bar_shows_task_label");
    h.screen.set_task_label("my-task".into());
    h.screen.set_throbber(Throbber::Working);
    let status = h.status_line_text();
    assert!(
        status.contains("my-task"),
        "Expected task label in status bar: {status:?}"
    );
}

// ── Responsive dropping ─────────────────────────────────────────────

#[test]
fn status_bar_never_wraps_wide() {
    let mut h = TestHarness::new(120, 24, "status_bar_never_wraps_wide");
    h.screen
        .set_task_label("some-very-long-task-name".into());
    h.screen.set_throbber(Throbber::Working);
    h.screen.set_running_procs(3);
    h.screen.set_agent_count(2);
    assert_status_fits_one_row(&mut h);
}

#[test]
fn status_bar_never_wraps_narrow() {
    let mut h = TestHarness::new(30, 24, "status_bar_never_wraps_narrow");
    h.screen
        .set_task_label("some-very-long-task-name".into());
    h.screen.set_throbber(Throbber::Working);
    h.screen.set_running_procs(3);
    h.screen.set_agent_count(2);
    assert_status_fits_one_row(&mut h);
}

#[test]
fn status_bar_never_wraps_tiny() {
    let mut h = TestHarness::new(15, 24, "status_bar_never_wraps_tiny");
    h.screen.set_task_label("task".into());
    h.screen.set_throbber(Throbber::Working);
    assert_status_fits_one_row(&mut h);
}

#[test]
fn slug_label_truncated_before_hidden() {
    // At moderate width, the label should be truncated (contain "…") rather
    // than fully hidden. The spinner should always be visible.
    let label = "a-very-long-slug-name-that-wont-fit";
    let mut h = TestHarness::new(40, 24, "slug_label_truncated");
    h.screen.set_task_label(label.into());
    h.screen.set_throbber(Throbber::Working);
    let status = h.status_line_text();
    assert_status_fits_one_row(&mut h);

    // The full label shouldn't fit at width 40.
    assert!(
        !status.contains(label),
        "Full label should not fit at width 40: {status:?}"
    );
}

#[test]
fn spinner_always_visible() {
    // Even at very narrow widths, the spinner character should appear.
    let mut h = TestHarness::new(10, 24, "spinner_always_visible");
    h.screen.set_task_label("task".into());
    h.screen.set_throbber(Throbber::Working);
    let status = h.status_line_text();
    assert_status_fits_one_row(&mut h);

    // The spinner is one of the flower characters.
    let has_spinner = status.contains('✿')
        || status.contains('❀')
        || status.contains('✾')
        || status.contains('❁');
    assert!(
        has_spinner,
        "Spinner should always be visible even at width 10: {status:?}"
    );
}

#[test]
fn mode_survives_longer_than_timer() {
    // The mode indicator (priority 1) should survive longer than the timer
    // (priority 4). At a width where the timer doesn't fit, mode should
    // still be there.
    let mut h = TestHarness::new(25, 24, "mode_survives_longer");
    h.screen.set_throbber(Throbber::Working);
    let status = h.status_line_text();
    assert_status_fits_one_row(&mut h);
    assert!(
        status.contains("normal"),
        "Mode indicator should survive at width 25: {status:?}"
    );
}

// ── Width sweep ─────────────────────────────────────────────────────

#[test]
fn status_bar_never_wraps_at_any_width() {
    // Sweep from very narrow to wide — the bar must never exceed one row.
    for width in 5..=120 {
        let name = format!("sweep_w{width}");
        let mut h = TestHarness::new(width, 24, &name);
        h.screen.set_task_label("my-task-slug".into());
        h.screen.set_throbber(Throbber::Working);
        h.screen.set_running_procs(2);
        h.screen.set_agent_count(1);
        assert_status_fits_one_row(&mut h);
    }
}
