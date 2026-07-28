//! Local AI summaries (#23). A `Backend` seam selects the inference engine:
//! the Ollama HTTP path (default, GPU-accelerated) or the bundled llama.cpp
//! sidecar (#21). The public entry points (`available`, `installed_models`,
//! `generate`, `embed`, `pull`) dispatch on the configured backend; everything
//! backend-agnostic (prompts, `pick_model`) stays free-standing; vector
//! similarity lives in `crate::semantic`.
//!
//! Everything degrades gracefully: if the active backend isn't reachable,
//! summaries are simply unavailable and the UI shows nothing.

use serde::Deserialize;

use crate::model::Repo;

/// The inference engine serving AI features, from `config.ai_backend`.
enum Backend {
    Ollama,
    LlamaCpp,
}

fn active_backend() -> Backend {
    match crate::config::load().ai_backend.as_str() {
        "llamaCpp" | "llama_cpp" | "llamacpp" => Backend::LlamaCpp,
        _ => Backend::Ollama,
    }
}

// ── Backend-dispatching entry points ───────────────────────────────────────
// Each delegates to the active backend. The llama.cpp arms drive the bundled
// `llama-server` sidecar (see `crate::llama`); embeddings stay Ollama-only, and
// model "pull" is a GGUF download from Settings rather than an Ollama registry
// pull. All degrade to "unavailable" when the backend isn't reachable.

/// Is the active backend reachable?
pub async fn available() -> bool {
    match active_backend() {
        Backend::Ollama => ollama_available().await,
        Backend::LlamaCpp => crate::llama::available().await,
    }
}

/// Installed/available models as (name, size_bytes) for the active backend.
pub async fn installed_models() -> Vec<(String, u64)> {
    match active_backend() {
        Backend::Ollama => ollama_installed_models().await,
        Backend::LlamaCpp => crate::llama::installed_models(),
    }
}

/// Default token budget for short generations (summaries, ping, resume, …).
const DEFAULT_NUM_PREDICT: u32 = 120;
/// Token budget for Conventional Commit messages (subject + real body).
const COMMIT_NUM_PREDICT: u32 = 384;
/// Max diff characters fed into the commit prompt (keeps context bounded).
const COMMIT_DIFF_CLAMP: usize = 10_000;

/// Generate text from `prompt` using `model` on the active backend. The
/// llama.cpp backend serves the configured GGUF, so it ignores `model`.
pub async fn generate(model: &str, prompt: &str) -> Result<String, String> {
    generate_limited(model, prompt, DEFAULT_NUM_PREDICT).await
}

async fn generate_limited(model: &str, prompt: &str, num_predict: u32) -> Result<String, String> {
    match active_backend() {
        Backend::Ollama => ollama_generate(model, prompt, num_predict).await,
        Backend::LlamaCpp => crate::llama::generate(prompt, num_predict).await,
    }
}

/// Whether the active backend can embed at all. Embeddings are Ollama-only, so
/// semantic indexing/recall stays dormant (not broken) on the llama.cpp
/// backend — callers skip the work instead of erroring per call.
pub fn embeddings_supported() -> bool {
    matches!(active_backend(), Backend::Ollama)
}

/// Embed `text` with `model` on the active backend. Embeddings are Ollama-only
/// for now — semantic search stays hidden on the llama.cpp backend.
pub async fn embed(model: &str, text: &str) -> Result<Vec<f32>, String> {
    match active_backend() {
        Backend::Ollama => ollama_embed(model, text).await,
        Backend::LlamaCpp => Err("embeddings are not supported on the llama.cpp backend".into()),
    }
}

/// Download/prepare `model` on the active backend, reporting progress. The
/// llama.cpp model download has its own command (`download_llama_model`).
pub async fn pull(model: &str, on_progress: impl FnMut(&str, u64, u64)) -> Result<(), String> {
    match active_backend() {
        Backend::Ollama => ollama_pull(model, on_progress).await,
        Backend::LlamaCpp => {
            Err("use the model download in Settings for the llama.cpp backend".into())
        }
    }
}

/// Base URL of the Ollama server, from config (default http://localhost:11434).
/// config::load() is cached, so this is cheap to call per request.
fn base() -> String {
    crate::config::load().ollama_host
}

/// Shared HTTP client so the many Ollama calls (status, per-repo summaries and
/// embeddings) reuse one connection pool. reqwest::Client is Arc-backed.
///
/// Only a `connect_timeout` is set — a dead/unreachable host fails fast — but no
/// overall request timeout, because generation and model pulls legitimately
/// stream for minutes and must not be cut off.
fn client() -> reqwest::Client {
    static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(8))
            .build()
            .unwrap_or_default()
    });
    CLIENT.clone()
}

/// Is a local Ollama server reachable?
async fn ollama_available() -> bool {
    client()
        .get(format!("{}/api/version", base()))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// Installed Ollama models as (name, size_bytes).
async fn ollama_installed_models() -> Vec<(String, u64)> {
    #[derive(Deserialize)]
    struct Tags {
        #[serde(default)]
        models: Vec<Model>,
    }
    #[derive(Deserialize)]
    struct Model {
        name: String,
        #[serde(default)]
        size: u64,
    }
    let resp = client().get(format!("{}/api/tags", base())).send().await;
    match resp {
        Ok(r) => match r.json::<Tags>().await {
            Ok(t) => t.models.into_iter().map(|m| (m.name, m.size)).collect(),
            Err(_) => Vec::new(),
        },
        Err(_) => Vec::new(),
    }
}

/// Heuristic: is this an embedding-only model? Such models reject `/api/generate`
/// (Ollama 400), so they must never be picked as a chat fallback.
pub fn is_embedding_model(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("embed") || n.contains("minilm") || n.starts_with("bge") || n.contains("/bge")
}

/// Choose the chat model: the preferred one if installed, otherwise the smallest
/// installed model that can actually generate (embedding models are excluded —
/// picking one would 400 on /api/generate). Pure for testing.
pub fn pick_model(preferred: &str, available: &[(String, u64)]) -> Option<String> {
    if available.iter().any(|(name, _)| name == preferred) {
        return Some(preferred.to_string());
    }
    available
        .iter()
        .filter(|(name, _)| !is_embedding_model(name))
        .min_by_key(|(_, size)| *size)
        .map(|(name, _)| name.clone())
}

/// Build the summarization prompt from repo metadata. Pure for testing.
pub fn summary_prompt(repo: &Repo) -> String {
    let git = &repo.git;
    let changes = if git.dirty > 0 {
        format!("{} uncommitted change(s)", git.dirty)
    } else {
        "a clean tree".to_string()
    };
    format!(
        "You summarize a code repository in ONE concise, factual sentence for a developer dashboard. \
No preamble, no markdown, max 24 words.\n\n\
Name: {name}\n\
Language: {lang}\n\
Description: {desc}\n\
State: branch {branch}, {changes}, {ahead} ahead / {behind} behind upstream.\n\n\
Summary:",
        name = repo.display_name,
        lang = repo.language.as_deref().unwrap_or("unknown"),
        desc = repo.description.as_deref().unwrap_or("(none)"),
        branch = git.branch,
        changes = changes,
        ahead = git.ahead,
        behind = git.behind,
    )
}

/// Heuristic: models known to "think" into a hidden channel and often empty
/// `response` when `num_predict` is small (Orrery's default chat budget).
fn likely_thinking_model(model: &str) -> bool {
    let n = model.to_lowercase();
    n.starts_with("qwen3")
        || n.contains("qwen3:")
        || n.starts_with("gemma3")
        || n.contains("gemma3:")
}

/// Generate a summary via Ollama.
///
/// Known thinking models (qwen3, gemma3, …) get `think:false` on the first
/// attempt so short prompts (commit messages, summaries) don't burn the whole
/// token budget on hidden reasoning. Other models try without the field first;
/// if the response is empty, we retry once with `think:false` — that way plain
/// models that might reject the field are never hit with it unnecessarily.
async fn ollama_generate(model: &str, prompt: &str, num_predict: u32) -> Result<String, String> {
    if likely_thinking_model(model) {
        return generate_once(model, prompt, true, num_predict).await;
    }
    let first = generate_once(model, prompt, false, num_predict).await?;
    if !first.is_empty() {
        return Ok(first);
    }
    generate_once(model, prompt, true, num_predict).await
}

async fn generate_once(
    model: &str,
    prompt: &str,
    suppress_think: bool,
    num_predict: u32,
) -> Result<String, String> {
    #[derive(Deserialize)]
    struct GenResp {
        #[serde(default)]
        response: String,
    }
    let mut body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
        "options": { "temperature": 0.2, "num_predict": num_predict }
    });
    if suppress_think {
        body["think"] = serde_json::Value::Bool(false);
    }
    let resp = client()
        .post(format!("{}/api/generate", base()))
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(ollama_error(resp).await);
    }
    let parsed: GenResp = resp.json().await.map_err(|e| e.to_string())?;
    Ok(parsed.response.trim().to_string())
}

/// Pull a model via Ollama (`/api/pull`), streaming NDJSON progress. `on_progress`
/// is called with (status, completed_bytes, total_bytes) for each update — kept
/// as a callback so this module stays free of Tauri's event machinery.
async fn ollama_pull(
    model: &str,
    mut on_progress: impl FnMut(&str, u64, u64),
) -> Result<(), String> {
    use futures_util::StreamExt;

    // Ollama's /api/pull needs an explicit tag; default to :latest when none.
    let model = if model.contains(':') {
        model.to_string()
    } else {
        format!("{model}:latest")
    };
    let model = model.as_str();

    #[derive(Deserialize)]
    struct Line {
        #[serde(default)]
        status: String,
        #[serde(default)]
        completed: u64,
        #[serde(default)]
        total: u64,
        #[serde(default)]
        error: Option<String>,
    }

    let body = serde_json::json!({ "model": model, "stream": true });
    let resp = client()
        .post(format!("{}/api/pull", base()))
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(ollama_error(resp).await);
    }

    // Ollama streams newline-delimited JSON objects; buffer partial lines.
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        buf.extend_from_slice(&chunk.map_err(|e| e.to_string())?);
        while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=nl).collect();
            let trimmed = &line[..line.len().saturating_sub(1)];
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(l) = serde_json::from_slice::<Line>(trimmed) {
                if let Some(err) = l.error {
                    return Err(err);
                }
                on_progress(&l.status, l.completed, l.total);
            }
        }
    }
    Ok(())
}

/// Build a readable error from a non-2xx Ollama response, surfacing its
/// `{"error": "..."}` body (e.g. "… does not support generate") instead of a
/// bare status code.
async fn ollama_error(resp: reqwest::Response) -> String {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let detail = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or_else(|| body.trim().to_string());
    if detail.is_empty() {
        format!("Ollama API {status}")
    } else {
        format!("Ollama API {status}: {detail}")
    }
}

/// Embed `text` with an embedding model via Ollama (`/api/embed`).
async fn ollama_embed(model: &str, text: &str) -> Result<Vec<f32>, String> {
    #[derive(Deserialize)]
    struct Resp {
        #[serde(default)]
        embeddings: Vec<Vec<f32>>,
    }
    let body = serde_json::json!({ "model": model, "input": text });
    let resp = client()
        .post(format!("{}/api/embed", base()))
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(ollama_error(resp).await);
    }
    let parsed: Resp = resp.json().await.map_err(|e| e.to_string())?;
    parsed
        .embeddings
        .into_iter()
        .next()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| "model returned no embedding".to_string())
}

fn clamp_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

fn is_odoo_manifest_path(path: &str) -> bool {
    let base = path.rsplit('/').next().unwrap_or(path);
    base == "__manifest__.py" || base == "__openerp__.py"
}

/// Relative paths of Odoo manifests touched by a unified diff (`diff --git` /
/// `+++ b/` headers).
pub fn manifest_paths_in_diff(diff: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            for part in rest.split_whitespace() {
                let p = part
                    .strip_prefix("a/")
                    .or_else(|| part.strip_prefix("b/"))
                    .unwrap_or(part);
                if is_odoo_manifest_path(p) && !paths.iter().any(|x| x == p) {
                    paths.push(p.to_string());
                }
            }
        } else if let Some(rest) = line.strip_prefix("+++ b/") {
            let p = rest.split('\t').next().unwrap_or(rest).trim();
            if is_odoo_manifest_path(p) && !paths.iter().any(|x| x == p) {
                paths.push(p.to_string());
            }
        }
    }
    paths
}

/// Parse an Odoo-style `version` assignment from a single line
/// (`'version': '19.0.1.0.0'`, `"version": "…"`, optional leading diff `+/-`).
pub fn parse_odoo_version_line(line: &str) -> Option<String> {
    let raw = line.trim();
    let stripped = raw
        .strip_prefix('+')
        .or_else(|| raw.strip_prefix('-'))
        .unwrap_or(raw)
        .trim()
        .trim_end_matches(',')
        .trim();

    for key in ["'version'", "\"version\""] {
        let Some(idx) = stripped.find(key) else {
            continue;
        };
        let after_key = stripped[idx + key.len()..].trim_start();
        let after_sep = after_key
            .strip_prefix(':')
            .or_else(|| after_key.strip_prefix('='))?
            .trim_start();
        let quote = after_sep.chars().next()?;
        if quote != '\'' && quote != '"' {
            continue;
        }
        let inner = &after_sep[quote.len_utf8()..];
        let end = inner.find(quote)?;
        let val = inner[..end].trim();
        if !val.is_empty() && val.chars().any(|c| c.is_ascii_digit()) {
            return Some(val.to_string());
        }
    }
    None
}

fn version_from_added_diff_lines(diff: &str) -> Option<String> {
    // Prefer the new (+) value when a version line changed.
    for line in diff.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            if let Some(v) = parse_odoo_version_line(line) {
                return Some(v);
            }
        }
    }
    // Fall back to any version line in the hunk (context / deletions).
    for line in diff.lines() {
        if let Some(v) = parse_odoo_version_line(line) {
            return Some(v);
        }
    }
    None
}

fn version_from_manifest_file(path: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    content.lines().find_map(parse_odoo_version_line)
}

/// Resolve an Odoo module version for commit prompts: prefer on-disk
/// `__manifest__.py` / `__openerp__.py` when `repo_path` is set (survives diff
/// truncation), else parse from the diff hunk. Returns a short hint line or
/// `None` when the change set does not touch a manifest.
pub fn odoo_manifest_version_hint(diff: &str, repo_path: Option<&str>) -> Option<String> {
    let paths = manifest_paths_in_diff(diff);
    let touches_manifest =
        !paths.is_empty() || diff.contains("__manifest__.py") || diff.contains("__openerp__.py");
    if !touches_manifest {
        return None;
    }

    let from_disk = repo_path.and_then(|root| {
        let root = std::path::Path::new(root);
        if !paths.is_empty() {
            paths
                .iter()
                .find_map(|rel| version_from_manifest_file(&root.join(rel)))
        } else {
            // Headers missed but the text mentions a manifest — walk common
            // single-module layouts is out of scope; stick to diff parse.
            None
        }
    });
    let version = from_disk.or_else(|| version_from_added_diff_lines(diff))?;
    Some(format!(
        "Odoo module version (from __manifest__.py): {version}"
    ))
}

/// Prompt to write a Conventional Commit message from a staged/working diff.
/// When `repo_path` is set and the diff touches an Odoo manifest, the current
/// module version is injected so it survives the diff character clamp.
/// Output shape (subject, blank line, body) is parsed by [`split_commit_message`].
pub fn commit_prompt(diff: &str) -> String {
    commit_prompt_with_context(diff, None)
}

/// Like [`commit_prompt`], with an optional repo root for on-disk manifest reads.
pub fn commit_prompt_with_context(diff: &str, repo_path: Option<&str>) -> String {
    let hint = odoo_manifest_version_hint(diff, repo_path)
        .map(|h| format!("\n{h}\n"))
        .unwrap_or_default();
    format!(
        "Write a Conventional Commit message for these staged changes.\n\n\
Requirements:\n\
- Subject line in Conventional Commit form (e.g. `feat(scope): summary`), under ~72 characters.\n\
- Then a blank line, then a real body of 2–5 short sentences or bullets explaining WHAT changed and WHY — not a subject-only one-liner.\n\
- If the diff touches `__manifest__.py` or `__openerp__.py` and shows a `version` change, you MUST mention the module version in the subject or body.\n\
- Output ONLY the commit message — no code fences, no preamble, no quotes around the whole message.\n\
- Write in English.\n\
{hint}\n\
Diff:\n{}\n\nCommit message:",
        clamp_chars(diff, COMMIT_DIFF_CLAMP)
    )
}

/// Split an AI commit message into (subject, body): the first non-empty line
/// (stripped of wrapping quotes/backticks) is the subject, everything after it
/// is the body. Both come back trimmed; either may be empty.
pub fn split_commit_message(text: &str) -> (String, String) {
    let mut lines = text.trim().lines();
    let subject = lines
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches(['"', '`'])
        .to_string();
    let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    (subject, body)
}

/// Prompt to summarize commits into a changelog / PR description.
pub fn changelog_prompt(commits: &[String]) -> String {
    format!(
        "Summarize these commits into a concise changelog as markdown bullet points, grouping related \
changes. No preamble.\n\nCommits:\n{}\n\nChangelog:",
        commits.join("\n")
    )
}

/// Prompt to draft a pull-request title + body from a branch's commit range.
/// The first output line is the title; the rest (after a blank line) is the
/// markdown body — [`split_pr_draft`] parses that shape back apart.
pub fn pr_prompt(branch: &str, commits: &[String]) -> String {
    format!(
        "Draft a pull request for branch \"{branch}\" from these commits (newest first). \
Output the PR title on the FIRST line (imperative, under 72 chars, no quotes), then a blank \
line, then a concise markdown body summarizing the changes as bullet points. No preamble, \
no code fences.\n\nCommits:\n{}\n\nPull request:",
        commits.join("\n")
    )
}

/// Split an AI PR draft into (title, body): first non-empty line is the
/// title, the rest is the body. `None` when there's no usable title.
pub fn split_pr_draft(text: &str) -> Option<(String, String)> {
    let mut lines = text.trim().lines();
    let title = lines.next()?.trim().trim_matches('"').to_string();
    if title.is_empty() {
        return None;
    }
    let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    Some((title, body))
}

/// Non-AI PR draft from a commit range (newest first): the latest commit's
/// subject as the title, the commit list as the body. Works with an empty
/// range too, so "Open PR" never depends on AI being reachable.
pub fn fallback_pr_draft(branch: &str, commits: &[String]) -> (String, String) {
    let title = commits
        .first()
        .cloned()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| branch.to_string());
    let body = commits
        .iter()
        .map(|c| format!("- {c}"))
        .collect::<Vec<_>>()
        .join("\n");
    (title, body)
}

/// Prompt to catch the user up on what changed in a repo since they last looked.
pub fn resume_prompt(repo_name: &str, commits: &[String]) -> String {
    format!(
        "In 2–3 short sentences, catch me up on what changed in the \"{repo_name}\" repository since I \
last looked, based on these commits (newest first). Be specific and factual, no preamble, no markdown.\n\n\
Commits:\n{}\n\nWhat changed:",
        commits.join("\n")
    )
}

/// Run `prompt` through the configured chat model on the active backend.
/// Picks the configured model if installed, else the smallest non-embed —
/// returns an error when AI is unreachable or no model is installed, so callers
/// degrade gracefully (the `aiReady` contract).
async fn generate_with_model(prompt: &str) -> Result<String, String> {
    generate_with_budget(prompt, DEFAULT_NUM_PREDICT).await
}

async fn generate_with_budget(prompt: &str, num_predict: u32) -> Result<String, String> {
    let cfg = crate::config::load();
    let models = installed_models().await;
    let model = pick_model(&cfg.ai_model, &models).ok_or("no AI model installed")?;
    generate_limited(&model, prompt, num_predict).await
}

/// Generate a Conventional Commit message for a staged/working diff.
///
/// `repo_path` is the repo root used to read `__manifest__.py` when the diff
/// touches it (so the version survives the diff character clamp). Pass `""`
/// or any path when unknown — version still falls back to parsing the diff.
pub async fn commit_message(repo_path: &str, diff: &str) -> Result<String, String> {
    let path = (!repo_path.is_empty()).then_some(repo_path);
    generate_with_budget(&commit_prompt_with_context(diff, path), COMMIT_NUM_PREDICT).await
}

/// Summarize commit subjects into a markdown changelog.
pub async fn changelog(commits: &[String]) -> Result<String, String> {
    generate_with_model(&changelog_prompt(commits)).await
}

/// A 2–3 sentence catch-up on what changed in a repo since the last look.
pub async fn resume(repo_name: &str, commits: &[String]) -> Result<String, String> {
    generate_with_model(&resume_prompt(repo_name, commits)).await
}

/// Draft a PR title + body from a branch's commit range (first line = title —
/// see [`pr_prompt`] / [`split_pr_draft`]). Callers fall back to
/// [`fallback_pr_draft`] when AI is unavailable (the `aiReady` contract).
pub async fn pr_description(branch: &str, commits: &[String]) -> Result<String, String> {
    generate_with_model(&pr_prompt(branch, commits)).await
}

/// Prompt for a short daily briefing across recently-active repos.
pub fn briefing_prompt(lines: &[String]) -> String {
    format!(
        "You are a dev's morning briefing. In 2–4 short sentences, summarize what changed across these \
repositories. Be specific and factual, no preamble.\n\n{}\n\nBriefing:",
        lines.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Activity, GitStatus};

    fn repo() -> Repo {
        Repo {
            id: "/x".into(),
            display_name: "Orrery".into(),
            slug: Some("Hankanman/Orrery".into()),
            path: "~/dev/Orrery".into(),
            description: Some("repo dashboard".into()),
            language: Some("Rust".into()),
            git: GitStatus {
                branch: "main".into(),
                ahead: 2,
                behind: 0,
                dirty: 7,
                ..Default::default()
            },
            last_commit_unix: 0,
            activity: Activity::Active,
            root: "~/dev".into(),
            host: None,
            remote_host: None,
            stars: 0,
            topics: vec![],
            open_issues: 0,
            latest_release: None,
            private: false,
            favorite: false,
            ai_summary: None,
            parent_id: None,
            submodule_path: None,
        }
    }

    #[test]
    fn pick_model_prefers_configured_then_smallest() {
        let avail = vec![
            ("big:70b".to_string(), 40_000),
            ("small:1b".to_string(), 1_000),
        ];
        assert_eq!(pick_model("big:70b", &avail).as_deref(), Some("big:70b"));
        // preferred absent → smallest
        assert_eq!(pick_model("missing", &avail).as_deref(), Some("small:1b"));
        assert_eq!(pick_model("x", &[]), None);
    }

    #[test]
    fn likely_thinking_model_matches_qwen3_and_gemma3() {
        assert!(likely_thinking_model("qwen3:0.6b"));
        assert!(likely_thinking_model("gemma3:1b"));
        assert!(!likely_thinking_model("qwen2.5:1.5b"));
        assert!(!likely_thinking_model("llama3.2:1b"));
    }

    #[test]
    fn pick_model_skips_embedding_models_in_fallback() {
        // nomic-embed-text is the smallest, but it can't generate — must be
        // skipped so the chat fallback never 400s on /api/generate.
        let avail = vec![
            ("nomic-embed-text".to_string(), 270),
            ("gemma:2b".to_string(), 1_600),
            ("qwen:9b".to_string(), 9_000),
        ];
        assert_eq!(pick_model("missing", &avail).as_deref(), Some("gemma:2b"));
        // A preferred embedding model is still honoured if explicitly chosen.
        assert_eq!(
            pick_model("nomic-embed-text", &avail).as_deref(),
            Some("nomic-embed-text")
        );
        // Only embedding models installed → no chat model.
        assert_eq!(pick_model("x", &[("all-minilm".to_string(), 50)]), None);
        assert!(is_embedding_model("nomic-embed-text"));
        assert!(!is_embedding_model("gemma:2b"));
    }

    #[test]
    fn summary_prompt_includes_key_facts() {
        let p = summary_prompt(&repo());
        assert!(p.contains("Orrery"));
        assert!(p.contains("Rust"));
        assert!(p.contains("7 uncommitted"));
        assert!(p.contains("branch main"));
    }

    #[test]
    fn pr_prompt_and_draft_split_round_trip() {
        let commits = vec!["feat: two".to_string(), "feat: one".to_string()];
        let p = pr_prompt("feat/x", &commits);
        assert!(p.contains("feat/x"));
        assert!(p.contains("feat: two"));
        assert!(p.contains("FIRST line"));

        // A well-formed draft splits into title + body.
        let (title, body) = split_pr_draft("feat: add x\n\n- one\n- two").unwrap();
        assert_eq!(title, "feat: add x");
        assert_eq!(body, "- one\n- two");
        // Title-only drafts still work; empty drafts don't.
        assert_eq!(
            split_pr_draft("\"feat: quoted\"").unwrap(),
            ("feat: quoted".to_string(), String::new())
        );
        assert!(split_pr_draft("   \n").is_none());
    }

    #[test]
    fn split_commit_message_separates_subject_and_body() {
        let (s, b) = split_commit_message("feat(x): add y\n\nLonger reasoning.\nSecond line.");
        assert_eq!(s, "feat(x): add y");
        assert_eq!(b, "Longer reasoning.\nSecond line.");
        // Subject-only messages give an empty body; wrapping quotes/backticks drop.
        assert_eq!(
            split_commit_message("`fix: subject only`\n"),
            ("fix: subject only".to_string(), String::new())
        );
        // A body directly after the subject (no blank line) still splits.
        let (s, b) = split_commit_message("fix: a\nbody line");
        assert_eq!((s.as_str(), b.as_str()), ("fix: a", "body line"));
        assert_eq!(split_commit_message("  "), (String::new(), String::new()));
    }

    #[test]
    fn fallback_pr_draft_uses_latest_subject_and_lists_commits() {
        let commits = vec!["feat: two".to_string(), "feat: one".to_string()];
        let (title, body) = fallback_pr_draft("feat/x", &commits);
        assert_eq!(title, "feat: two");
        assert_eq!(body, "- feat: two\n- feat: one");
        // No commits (shouldn't happen, but): the branch name still titles it.
        let (title, body) = fallback_pr_draft("feat/x", &[]);
        assert_eq!(title, "feat/x");
        assert!(body.is_empty());
    }

    #[test]
    fn commit_prompt_includes_diff_and_clamps() {
        let p = commit_prompt("diff --git a/x b/x\n+hello");
        assert!(p.contains("Conventional Commit"));
        assert!(p.contains("WHAT changed and WHY"));
        assert!(p.contains("+hello"));
        // very long diffs are clamped (budget leaves room for the prompt wrapper)
        let big = "x".repeat(20_000);
        let clamped = commit_prompt(&big);
        assert!(clamped.len() < COMMIT_DIFF_CLAMP + 2_000);
        assert!(!clamped.contains(&"x".repeat(COMMIT_DIFF_CLAMP + 1)));
    }

    #[test]
    fn parse_odoo_version_line_accepts_common_forms() {
        assert_eq!(
            parse_odoo_version_line("    'version': '19.0.1.2.0',"),
            Some("19.0.1.2.0".into())
        );
        assert_eq!(
            parse_odoo_version_line("+    \"version\": \"18.0.1.0.1\""),
            Some("18.0.1.0.1".into())
        );
        assert_eq!(parse_odoo_version_line("name = 'foo'"), None);
        assert_eq!(parse_odoo_version_line("--- a/__manifest__.py"), None);
    }

    #[test]
    fn odoo_manifest_version_hint_from_diff_and_disk() {
        let diff = "\
diff --git a/addons/foo/__manifest__.py b/addons/foo/__manifest__.py
--- a/addons/foo/__manifest__.py
+++ b/addons/foo/__manifest__.py
@@ -1,5 +1,5 @@
 {
-    'version': '19.0.1.1.0',
+    'version': '19.0.1.2.0',
     'name': 'Foo',
 }
";
        let hint = odoo_manifest_version_hint(diff, None).expect("hint");
        assert!(hint.contains("19.0.1.2.0"), "{hint}");
        assert!(hint.contains("Odoo module version"));

        // No manifest in the change set → no hint.
        assert!(odoo_manifest_version_hint("diff --git a/x.rs b/x.rs\n+hi", None).is_none());

        // On-disk read wins when repo_path points at a real manifest.
        let dir = tempfile::tempdir().unwrap();
        let mod_dir = dir.path().join("addons/foo");
        std::fs::create_dir_all(&mod_dir).unwrap();
        std::fs::write(
            mod_dir.join("__manifest__.py"),
            "{\n    'version': '19.0.9.9.9',\n    'name': 'Foo',\n}\n",
        )
        .unwrap();
        let hint = odoo_manifest_version_hint(diff, Some(dir.path().to_str().unwrap())).unwrap();
        assert!(hint.contains("19.0.9.9.9"), "{hint}");

        let prompt = commit_prompt_with_context(diff, None);
        assert!(prompt.contains("19.0.1.2.0"));
        assert!(prompt.contains("__manifest__.py"));
    }

    #[test]
    fn manifest_paths_in_diff_collects_headers() {
        let diff = "diff --git a/m/__manifest__.py b/m/__manifest__.py\n+++ b/m/__openerp__.py\n";
        let paths = manifest_paths_in_diff(diff);
        assert!(paths.iter().any(|p| p == "m/__manifest__.py"));
        assert!(paths.iter().any(|p| p == "m/__openerp__.py"));
    }
}
