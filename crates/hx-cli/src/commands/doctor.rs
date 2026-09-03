//! Doctor command implementation.

use anyhow::Result;
use hx_config::find_project_root;
use hx_doctor::{print_report, run_checks};
use hx_ui::Output;

/// Run the doctor command.
pub async fn run(output: &Output) -> Result<i32> {
    output.status("Running", "doctor checks");

    // Try to find project root (but don't fail if not in a project)
    let project_dir = find_project_root(".").ok();

    let report = run_checks(project_dir.as_deref(), crate::cli::invoked_name()).await;

    print_report(&report, output);

    if report.has_errors() {
        Ok(4) // Toolchain error exit code
    } else {
        Ok(0)
    }
}
