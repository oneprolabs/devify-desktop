//! Account-scoped summary history exposed through native IPC.
use crate::{app_data, history, local_scope};
use tauri::AppHandle;

#[tauri::command]
pub(crate) async fn summary_history_list(
    app: AppHandle,
) -> Result<Vec<history::SummaryRecord>, String> {
    history::list(&local_scope::root(&app_data(&app)))
}

/// Persist a summary after it has been shown to the user. The backend owns the id and the
/// timestamp, and enforces the record cap, so the drawer cannot grow the file unbounded.
#[tauri::command]
pub(crate) async fn summary_history_add(
    app: AppHandle,
    record: history::NewRecord,
) -> Result<Vec<history::SummaryRecord>, String> {
    history::add(&local_scope::root(&app_data(&app)), record)
}

#[tauri::command]
pub(crate) async fn summary_history_delete(
    app: AppHandle,
    id: String,
) -> Result<Vec<history::SummaryRecord>, String> {
    history::remove(&local_scope::root(&app_data(&app)), &id)
}
