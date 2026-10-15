//! Run-scoped cancellation for in-flight tool executions (Cursor MCP).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

tokio::task_local! {
    static TOOL_CANCEL: Option<Arc<AtomicBool>>;
}

/// Install a cancel flag for the duration of `f` (typically one MCP `tools/call`).
pub async fn scope_tool_cancel<F, T>(cancel: Option<Arc<AtomicBool>>, f: F) -> T
where
    F: std::future::Future<Output = T>,
{
    TOOL_CANCEL.scope(cancel, f).await
}

/// Cancel flag for the current tool call, if any.
pub fn current_tool_cancel() -> Option<Arc<AtomicBool>> {
    TOOL_CANCEL.try_with(|c| c.clone()).ok().flatten()
}

pub fn is_cancelled(flag: &AtomicBool) -> bool {
    flag.load(Ordering::SeqCst)
}

pub fn request_cancel(flag: &AtomicBool) {
    flag.store(true, Ordering::SeqCst);
}
