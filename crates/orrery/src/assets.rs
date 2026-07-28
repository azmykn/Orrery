//! Embedded SVG assets served to GPUI.
//!
//! Two icon packs share one [`AssetSource`]:
//! - **Orrery** icons under `assets/icons/` (`lucide/…`, `brand/…`, `devicon/…`)
//! - **gpui-component** icons (`icons/….svg`) for TitleBar window controls,
//!   `Button` icons, menus, etc.
//!
//! Without the gpui-component pack, CSD minimize/maximize/close and the header
//! "+" render as empty (invisible) controls against the dark chrome.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets/icons"]
struct Icons;

/// True if an Orrery asset is embedded at `path` (e.g. `"devicon/rust.svg"`).
pub fn has_icon(path: &str) -> bool {
    Icons::get(path).is_some()
}

/// Combined asset source registered on the `Application`.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }
        // Orrery lucide/brand/devicon paths first (no `icons/` prefix).
        if let Some(f) = Icons::get(path) {
            return Ok(Some(f.data));
        }
        // gpui-component IconName paths look like `icons/window-minimize.svg`.
        gpui_component_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut out: Vec<SharedString> = Icons::iter()
            .filter(|p| p.starts_with(path))
            .map(|p| SharedString::from(p.to_string()))
            .collect();
        if let Ok(extra) = gpui_component_assets::Assets.list(path) {
            out.extend(extra);
        }
        out.sort();
        out.dedup();
        Ok(out)
    }
}
