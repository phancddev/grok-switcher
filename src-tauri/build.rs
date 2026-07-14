fn main() {
    tauri_build::build();

    let release_date = std::env::var("GROK_SWITCHER_RELEASE_DATE").unwrap_or_else(|_| {
        // Fallback for local builds. Release CI supplies one shared date to every platform.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // Civil date from Unix days (UTC).
        let days = (now / 86_400) as i64;
        // Algorithm from civil_from_days (Howard Hinnant) — days since 1970-01-01.
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = (z - era * 146_097) as u64;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
        let y = yoe as i64 + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        format!("{y:04}-{m:02}-{d:02}")
    });

    println!("cargo:rustc-env=GROK_SWITCHER_RELEASE_DATE={release_date}");
    println!("cargo:rerun-if-env-changed=GROK_SWITCHER_RELEASE_DATE");
}
