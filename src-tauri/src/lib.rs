mod auth;
mod billing;
mod commands;
mod error;
mod login;
mod paths;
mod settings;
mod store;
mod types;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::list_accounts,
            commands::get_active,
            commands::add_account,
            commands::import_current_account,
            commands::switch_account,
            commands::remove_account,
            commands::set_account_label,
            commands::refresh_quota,
            commands::refresh_all_quotas,
            commands::get_settings,
            commands::save_settings,
            commands::resolve_grok_binary,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
