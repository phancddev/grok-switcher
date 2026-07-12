mod auth;
mod billing;
mod commands;
mod error;
mod login;
mod paths;
mod settings;
mod store;
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
            commands::check_github_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
