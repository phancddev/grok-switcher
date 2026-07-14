mod auth;
mod billing;
mod commands;
mod error;
mod login;
mod paths;
mod settings;
mod store;
mod token_refresh;
mod types;
mod update;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            #[cfg(desktop)]
            {
                app.handle()
                    .plugin(tauri_plugin_updater::Builder::new().build())?;
            }
            // Refresh near-expiry tokens on startup and every five minutes.
            token_refresh::spawn_background_refresh(app.handle().clone());
            Ok(())
        })
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
            commands::get_app_version,
            commands::get_app_info,
            commands::check_github_update,
            commands::refresh_all_tokens,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
