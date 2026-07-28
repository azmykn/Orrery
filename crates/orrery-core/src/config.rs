//! Load and persist `AppConfig` as TOML under the XDG config dir
//! (`~/.config/orrery/config.toml`), with sensible PATH-detected defaults.

use std::fs;
use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};

use crate::model::AppConfig;

fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("orrery").join("config.toml"))
}

// Process-lifetime cache: `load()` is called many times per request cycle
// (scan, enrich, ai, watcher…). The config only changes through `save()` (the
// settings UI), which refreshes the cache, so we avoid re-reading/parsing the
// TOML on every call.
static CACHE: LazyLock<RwLock<Option<AppConfig>>> = LazyLock::new(|| RwLock::new(None));

/// First command on PATH from `candidates`, formatted into a `{path}` template.
fn detect(candidates: &[&str], template: &str) -> Option<String> {
    candidates
        .iter()
        .find(|c| which::which(c).is_ok())
        .map(|c| template.replace("{cmd}", c))
}

impl Default for AppConfig {
    fn default() -> Self {
        let home = dirs::home_dir()
            .map(|h| h.join("dev").to_string_lossy().into_owned())
            .unwrap_or_else(|| "~/dev".to_string());

        // Prefer an installed GUI editor; fall back to a sensible default.
        let ide_command = detect(&["code", "zed", "subl"], "{cmd} {path}")
            .or_else(|| detect(&["nvim", "vim"], "{cmd} {path}"))
            .unwrap_or_else(|| "xdg-open {path}".to_string());

        // Open the user's terminal at the repo and start a coding agent.
        let term = [
            "kitty",
            "alacritty",
            "foot",
            "wezterm",
            "konsole",
            "gnome-terminal",
        ]
        .iter()
        .find(|t| which::which(t).is_ok())
        .copied();
        let agent_command = match term {
            Some("konsole") => "konsole --workdir {path} -e claude".to_string(),
            Some("gnome-terminal") => {
                "gnome-terminal --working-directory={path} -- claude".to_string()
            }
            Some("wezterm") => "wezterm start --cwd {path} -- claude".to_string(),
            Some(t) => format!("{t} --working-directory {{path}} -e claude"),
            None => "xterm -e claude".to_string(),
        };

        Self {
            roots: vec![home],
            scan_depth: 3,
            ignore: ["node_modules", ".cache", "vendor", "target", "dist", ".git"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ide_command,
            agent_command,
            agent_dispatch_args: crate::model::default_agent_dispatch_args(),
            github_client_id: String::new(),
            github_allow_cli_token: true,
            gitlab_hosts: Vec::new(),
            ai_model: crate::model::default_ai_model(),
            ai_enabled: true,
            ai_backend: crate::model::default_ai_backend(),
            llama_server_path: String::new(),
            llama_model_path: String::new(),
            embed_model: crate::model::default_embed_model(),
            ollama_host: crate::model::default_ollama_host(),
            notify_enabled: true,
            notify_new_pr: true,
            notify_review_requested: true,
            notify_ci_failure: true,
            notify_attention: true,
            notify_agent_finished: true,
            sidebar_width: crate::model::default_sidebar_width(),
            sidebar_collapsed: false,
            workspace_groups: Vec::new(),
            active_workspace_group: None,
        }
    }
}

/// If `workspace_groups` is empty, seed Odoo-style groups (`core` / `digits` /
/// `custom`) under each configured root that has those directories. Returns true
/// when groups were added (caller should persist).
pub fn seed_odoo_groups_if_empty(cfg: &mut AppConfig) -> bool {
    if !cfg.workspace_groups.is_empty() {
        return false;
    }
    let mut groups = Vec::new();
    for root in &cfg.roots {
        let root_path = crate::scan::expand(root);
        let root_label = root_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(root.as_str());
        for (label, sub) in [("Core", "core"), ("Digits", "digits"), ("Custom", "custom")] {
            let p = root_path.join(sub);
            if p.is_dir() {
                groups.push(crate::model::WorkspaceGroup {
                    name: format!("{root_label} · {label}"),
                    prefixes: vec![p.to_string_lossy().into_owned()],
                });
            }
        }
    }
    if groups.is_empty() {
        return false;
    }
    cfg.workspace_groups = groups;
    true
}

/// Load config, falling back to (and writing) defaults if absent/invalid.
/// Cached after the first read; `save()` keeps the cache current.
pub fn load() -> AppConfig {
    if let Some(cfg) = CACHE.read().ok().and_then(|g| g.clone()) {
        return cfg;
    }
    let mut cfg = load_uncached();
    if seed_odoo_groups_if_empty(&mut cfg) {
        let _ = save(&cfg);
    }
    if let Ok(mut g) = CACHE.write() {
        *g = Some(cfg.clone());
    }
    cfg
}

fn load_uncached() -> AppConfig {
    let Some(path) = config_path() else {
        return AppConfig::default();
    };
    match fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).unwrap_or_else(|_| AppConfig::default()),
        Err(_) => {
            let cfg = AppConfig::default();
            let _ = save(&cfg);
            cfg
        }
    }
}

/// Persist config as TOML, creating the config directory if needed.
pub fn save(config: &AppConfig) -> Result<(), String> {
    let path = config_path().ok_or("no config directory")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = toml::to_string_pretty(config).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| e.to_string())?;
    // Keep the cache in step with what we just wrote.
    if let Ok(mut g) = CACHE.write() {
        *g = Some(config.clone());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_sane() {
        let cfg = AppConfig::default();
        assert!(!cfg.roots.is_empty(), "must have at least one root");
        assert_eq!(cfg.scan_depth, 3);
        assert!(cfg.ignore.iter().any(|i| i == "node_modules"));
        assert!(
            cfg.ide_command.contains("{path}"),
            "ide template needs {{path}}"
        );
        // The agent command launches the agent; the repo dir comes from the
        // launcher's working directory (launch::spawn sets current_dir), so
        // {path} isn't required — and the fallback terminal (xterm) has no
        // working-dir flag. Asserting {path} here would be env-dependent (it
        // only appears when a terminal with such a flag is detected).
        assert!(
            cfg.agent_command.contains("claude"),
            "agent command should launch the agent"
        );
        assert!(!cfg.agent_command.is_empty());
    }

    #[test]
    fn toml_round_trips() {
        let cfg = AppConfig::default();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: AppConfig = toml::from_str(&text).unwrap();
        assert_eq!(back.roots, cfg.roots);
        assert_eq!(back.scan_depth, cfg.scan_depth);
        assert_eq!(back.ignore, cfg.ignore);
        assert_eq!(back.ide_command, cfg.ide_command);
        assert_eq!(back.agent_command, cfg.agent_command);
        assert_eq!(back.agent_dispatch_args, cfg.agent_dispatch_args);
    }

    #[test]
    fn dispatch_args_default_passes_prompt() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.agent_dispatch_args, "{prompt}");
        // Older configs without the key must default the same way.
        let back: AppConfig = toml::from_str(
            "roots = []\nscanDepth = 3\nignore = []\nideCommand = \"c {path}\"\nagentCommand = \"claude\"",
        )
        .unwrap();
        assert_eq!(back.agent_dispatch_args, "{prompt}");
    }
}
