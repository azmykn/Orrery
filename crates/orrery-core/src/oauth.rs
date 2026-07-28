//! GitHub OAuth device flow (#18) + token resolution.
//!
//! Token resolution (see [`resolve_github_token`]):
//! 1. Stored Orrery OAuth token
//! 2. If Orrery **Sign out** is active → none (skip env/`gh`)
//! 3. If `github_allow_cli_token`: `$ORRERY_GITHUB_TOKEN`, then `gh auth token`
//!
//! Sign out never calls `gh auth logout`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
// `repo` (not just `public_repo`) so enrichment can read private repos — needed
// for the public/private filter and the lock badge — and so the Actions
// workflow-runs endpoint used by the CI pass can read private-repo CI.
// Classic OAuth with `repo` covers Actions; a fine-grained PAT used via
// `$ORRERY_GITHUB_TOKEN` / `gh` still needs an explicit Actions: Read grant.
const SCOPE: &str = "read:user repo";

/// Built-in OAuth app client id for the device flow, so sign-in works out of the
/// box with no configuration. The device flow has no client secret, so a client
/// id is not sensitive. A non-empty `github_client_id` in config overrides it
/// (e.g. to point at your own OAuth app).
const DEFAULT_GITHUB_CLIENT_ID: &str = "Ov23liQZt2ALfwxZbINW";

/// Where the token came from — shown in Settings so users know why the GitHub
/// Apps page may be empty and which SSO / scope path applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GithubTokenSource {
    /// Device-flow token written by Orrery.
    OrreryOAuth,
    /// `$ORRERY_GITHUB_TOKEN`.
    EnvVar,
    /// `gh auth token` (machine-wide CLI session).
    GhCli,
}

impl GithubTokenSource {
    /// Short English label for Settings.
    pub fn label(self) -> &'static str {
        match self {
            Self::OrreryOAuth => "Orrery OAuth",
            Self::EnvVar => "ORRERY_GITHUB_TOKEN",
            Self::GhCli => "`gh` CLI",
        }
    }

    /// Whether this source is the Orrery device-flow OAuth app (SSO via Apps).
    pub fn is_orrery_oauth(self) -> bool {
        matches!(self, Self::OrreryOAuth)
    }
}

/// The client id to use: the configured one if set, otherwise the built-in default.
pub fn github_client_id() -> String {
    let configured = crate::config::load().github_client_id;
    if configured.trim().is_empty() {
        DEFAULT_GITHUB_CLIENT_ID.to_string()
    } else {
        configured
    }
}

/// GitHub "Authorized OAuth Apps" page for the active client id (org SSO).
pub fn github_oauth_app_settings_url() -> String {
    format!(
        "https://github.com/settings/connections/applications/{}",
        github_client_id()
    )
}

/// Shared HTTP client for the device-flow calls (one connection pool, bounded
/// timeouts) instead of building a fresh client per request.
fn client() -> reqwest::Client {
    static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(8))
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .unwrap_or_default()
    });
    CLIENT.clone()
}

fn data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("orrery"))
}

fn token_path() -> Option<PathBuf> {
    data_dir().map(|d| d.join("github_token"))
}

fn signed_out_path() -> Option<PathBuf> {
    data_dir().map(|d| d.join("github_signed_out"))
}

pub fn stored_github_token() -> Option<String> {
    token_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// True after Orrery Sign out until Connect succeeds or CLI/env fallback is
/// explicitly re-enabled.
pub fn is_signed_out() -> bool {
    signed_out_path().is_some_and(|p| p.exists())
}

fn set_signed_out(on: bool) {
    let Some(path) = signed_out_path() else {
        return;
    };
    if on {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, b"1");
    } else {
        let _ = std::fs::remove_file(path);
    }
}

/// Clear the Orrery-only signed-out marker (e.g. after re-enabling CLI/env).
pub fn clear_signed_out() {
    set_signed_out(false);
}

fn save_token(token: &str) -> Result<(), String> {
    let path = token_path().ok_or("no data directory")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Create owner-only from the start (no umask race) — the token is a secret.
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| e.to_string())?;
        file.write_all(token.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&path, token).map_err(|e| e.to_string())?;
    }
    // A fresh Orrery login supersedes Sign out.
    set_signed_out(false);
    Ok(())
}

fn cli_token(bin: &str) -> Option<String> {
    let out = std::process::Command::new(bin)
        .args(["auth", "token"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!token.is_empty()).then_some(token)
}

fn allow_cli_token() -> bool {
    crate::config::load().github_allow_cli_token
}

/// Pure resolution used by [`github_token`] / [`github_token_source`] and tests.
pub fn resolve_github_token(
    stored: Option<String>,
    signed_out: bool,
    allow_cli: bool,
    env: Option<String>,
    cli: Option<String>,
) -> Option<(String, GithubTokenSource)> {
    if let Some(token) = stored.filter(|s| !s.is_empty()) {
        return Some((token, GithubTokenSource::OrreryOAuth));
    }
    // Orrery Sign out: stay disconnected until Connect or explicit re-enable.
    if signed_out {
        return None;
    }
    if !allow_cli {
        return None;
    }
    if let Some(token) = env.filter(|s| !s.is_empty()) {
        return Some((token, GithubTokenSource::EnvVar));
    }
    if let Some(token) = cli.filter(|s| !s.is_empty()) {
        return Some((token, GithubTokenSource::GhCli));
    }
    None
}

/// Resolve a GitHub token and where it came from.
pub fn github_token_source() -> Option<(String, GithubTokenSource)> {
    resolve_github_token(
        stored_github_token(),
        is_signed_out(),
        allow_cli_token(),
        std::env::var("ORRERY_GITHUB_TOKEN")
            .ok()
            .filter(|s| !s.is_empty()),
        cli_token("gh"),
    )
}

/// Resolve a GitHub token: stored OAuth → (unless signed out) env → `gh`.
pub fn github_token() -> Option<String> {
    github_token_source().map(|(t, _)| t)
}

/// Resolve a GitLab token: env → `glab auth token`.
pub fn gitlab_token() -> Option<String> {
    std::env::var("ORRERY_GITLAB_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| cli_token("glab"))
}

/// True if any GitHub token is available under current Sign-out / fallback rules.
pub fn github_authed() -> bool {
    github_token().is_some()
}

/// Source-aware hint when Actions returns 403 (not rate-limit).
pub fn ci_forbidden_hint() -> String {
    match github_token_source().map(|(_, s)| s) {
        Some(GithubTokenSource::OrreryOAuth) => format!(
            "GitHub CI 403 — authorize org SSO for Orrery at {} (or reconnect in Settings)",
            github_oauth_app_settings_url()
        ),
        Some(GithubTokenSource::EnvVar) | Some(GithubTokenSource::GhCli) => {
            "GitHub CI 403 — token lacks Actions read. Prefer Connect with Orrery in Settings, or grant Actions:read / classic `repo` on the PAT"
                .into()
        }
        None => {
            "GitHub CI 403 — can't read Actions (reconnect in Settings, authorize org SSO, or grant Actions:read on a PAT)"
                .into()
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStart {
    pub user_code: String,
    pub verification_uri: String,
    pub device_code: String,
    pub interval: u64,
}

/// Begin the device flow: returns the code the user enters at the URL.
pub async fn device_start(client_id: &str) -> Result<DeviceStart, String> {
    #[derive(Deserialize)]
    struct Resp {
        device_code: String,
        user_code: String,
        verification_uri: String,
        interval: u64,
    }
    let resp: Resp = client()
        .post(DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .form(&[("client_id", client_id), ("scope", SCOPE)])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    Ok(DeviceStart {
        user_code: resp.user_code,
        verification_uri: resp.verification_uri,
        device_code: resp.device_code,
        interval: resp.interval,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PollResult {
    /// "authorized" | "authorization_pending" | "slow_down" | "expired_token" | "access_denied" | "error"
    pub status: String,
}

/// Poll once for the token. On success, persists it.
pub async fn device_poll(client_id: &str, device_code: &str) -> Result<PollResult, String> {
    #[derive(Deserialize)]
    struct Resp {
        access_token: Option<String>,
        error: Option<String>,
    }
    let resp: Resp = client()
        .post(TOKEN_URL)
        .header("Accept", "application/json")
        .form(&[
            ("client_id", client_id),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    if let Some(token) = resp.access_token {
        let token = token.trim();
        if token.is_empty() || !token.is_ascii() || token.len() > 255 {
            return Err("received a malformed access token".into());
        }
        save_token(token)?;
        return Ok(PollResult {
            status: "authorized".into(),
        });
    }
    Ok(PollResult {
        status: resp.error.unwrap_or_else(|| "error".into()),
    })
}

/// Forget the stored OAuth token and stop using `gh`/env until Connect or the
/// user re-enables the CLI/env fallback. Never touches machine-wide `gh` auth.
pub fn sign_out() {
    if let Some(path) = token_path() {
        let _ = std::fs::remove_file(path);
    }
    set_signed_out(true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_prefers_stored_oauth() {
        let got = resolve_github_token(
            Some("oauth".into()),
            true, // signed out is ignored when a stored token exists
            true,
            Some("env".into()),
            Some("cli".into()),
        );
        assert_eq!(got, Some(("oauth".into(), GithubTokenSource::OrreryOAuth)));
    }

    #[test]
    fn resolve_signed_out_skips_fallbacks() {
        assert_eq!(
            resolve_github_token(None, true, true, Some("env".into()), Some("cli".into())),
            None
        );
    }

    #[test]
    fn resolve_allow_cli_off_skips_fallbacks() {
        assert_eq!(
            resolve_github_token(None, false, false, Some("env".into()), Some("cli".into())),
            None
        );
    }

    #[test]
    fn resolve_env_before_cli() {
        let got = resolve_github_token(None, false, true, Some("env".into()), Some("cli".into()));
        assert_eq!(got, Some(("env".into(), GithubTokenSource::EnvVar)));
    }

    #[test]
    fn resolve_cli_when_no_env() {
        let got = resolve_github_token(None, false, true, None, Some("cli".into()));
        assert_eq!(got, Some(("cli".into(), GithubTokenSource::GhCli)));
    }

    #[test]
    fn source_labels() {
        assert_eq!(GithubTokenSource::OrreryOAuth.label(), "Orrery OAuth");
        assert_eq!(GithubTokenSource::EnvVar.label(), "ORRERY_GITHUB_TOKEN");
        assert_eq!(GithubTokenSource::GhCli.label(), "`gh` CLI");
    }
}
