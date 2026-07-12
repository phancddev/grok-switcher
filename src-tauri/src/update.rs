use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};

/// Public GitHub repo used for release checks.
pub const GITHUB_OWNER: &str = "phancddev";
pub const GITHUB_REPO: &str = "grok-switcher";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubUpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub has_update: bool,
    pub release_url: String,
    pub release_notes: Option<String>,
    pub published_at: Option<String>,
    pub tag_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    #[allow(dead_code)]
    prerelease: bool,
}

pub fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Strip leading `v` and any build metadata (`+…`) / pre-release for comparison base.
fn normalize_version(raw: &str) -> String {
    let s = raw.trim().trim_start_matches('v').trim_start_matches('V');
    // drop +build
    let s = s.split('+').next().unwrap_or(s);
    // for pre-release tags like 0.1.0-main.abc keep base before first -
    // but only if it looks like semver with extra suffix from CI
    if let Some((base, rest)) = s.split_once('-') {
        // keep pre-release for 1.0.0-beta.1; strip CI tags like 0.1.0-main.sha
        if rest.starts_with("main.") || rest.starts_with("dev.") {
            return base.to_string();
        }
    }
    s.to_string()
}

/// Compare semver-ish strings: returns true if `latest` is newer than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let a = parse_semver(&normalize_version(latest));
    let b = parse_semver(&normalize_version(current));
    a > b
}

fn parse_semver(s: &str) -> (u64, u64, u64) {
    let mut parts = s.split('.');
    let major = parts
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    let minor = parts
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    let patch = parts
        .next()
        .and_then(|p| {
            // "1" or "1-beta"
            let digits: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse().ok()
        })
        .unwrap_or(0);
    (major, minor, patch)
}

/// Check GitHub Releases API for a newer non-draft release.
pub fn check_github_latest() -> AppResult<GithubUpdateInfo> {
    let current = app_version();
    let url = format!(
        "https://api.github.com/repos/{GITHUB_OWNER}/{GITHUB_REPO}/releases/latest"
    );

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(format!("grok-switcher/{current}"))
        .build()?;

    let resp = client.get(&url).send()?;
    if resp.status().as_u16() == 404 {
        // No releases yet
        return Ok(GithubUpdateInfo {
            current_version: current.clone(),
            latest_version: current.clone(),
            has_update: false,
            release_url: format!("https://github.com/{GITHUB_OWNER}/{GITHUB_REPO}/releases"),
            release_notes: None,
            published_at: None,
            tag_name: None,
        });
    }
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(AppError::msg(format!(
            "GitHub API {status}: {}",
            body.chars().take(180).collect::<String>()
        )));
    }

    let release: GhRelease = resp.json()?;
    if release.draft {
        return Ok(GithubUpdateInfo {
            current_version: current.clone(),
            latest_version: current.clone(),
            has_update: false,
            release_url: release.html_url,
            release_notes: None,
            published_at: release.published_at,
            tag_name: Some(release.tag_name),
        });
    }

    let latest = normalize_version(&release.tag_name);
    let has_update = is_newer(&latest, &current);

    Ok(GithubUpdateInfo {
        current_version: current,
        latest_version: latest,
        has_update,
        release_url: release.html_url,
        release_notes: release.body.map(|b| {
            // first ~400 chars for UI
            b.chars().take(400).collect()
        }),
        published_at: release.published_at,
        tag_name: Some(release.tag_name),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_compare() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(is_newer("0.1.1", "0.1.0-main.abc"));
    }
}
