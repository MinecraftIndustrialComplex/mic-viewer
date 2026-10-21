// SPDX-License-Identifier: MPL-2.0
//! The page's files, compiled in (as Deep Time's deeptime-viewer does): no build step and nothing
//! read from disk at run time. Every script is a classic script that adds to `globalThis.MicViewer`
//! (docs/map-core.md), so a project's own page (Deep Time's workbench) can load `map.js` alone.

/// The viewer page.
pub const PAGE: &str = include_str!("../web/viewer.html");

/// A served file.
pub struct Asset {
    pub name: &'static str,
    pub content_type: &'static str,
    pub body: &'static str,
}

const JS: &str = "text/javascript; charset=utf-8";

/// Every file besides the page, by the name it is served under (`/<name>`).
pub const FILES: &[Asset] = &[
    Asset { name: "viewer.css", content_type: "text/css; charset=utf-8", body: include_str!("../web/viewer.css") },
    Asset { name: "map.js", content_type: JS, body: include_str!("../web/map.js") },
    Asset { name: "flat.js", content_type: JS, body: include_str!("../web/flat.js") },
    Asset { name: "marks.js", content_type: JS, body: include_str!("../web/marks.js") },
    Asset { name: "card.js", content_type: JS, body: include_str!("../web/card.js") },
    Asset { name: "timeline.js", content_type: JS, body: include_str!("../web/timeline.js") },
    Asset { name: "app.js", content_type: JS, body: include_str!("../web/app.js") },
];

/// The file served as `/<name>`.
pub fn file(name: &str) -> Option<&'static Asset> {
    FILES.iter().find(|a| a.name == name)
}

/// The map core alone, for a project's own page.
pub const MAP_JS: &str = include_str!("../web/map.js");

/// The environment variable that names a folder to serve the page's files from instead of the
/// compiled-in copies, read again on every request: for working on the page without a rebuild. Only
/// the names in [`FILES`] and the page are served from it.
pub const WEB_DIR_ENV: &str = "MIC_VIEWER_WEB_DIR";

/// The body of `name` (`viewer.html` for the page): from [`WEB_DIR_ENV`]'s folder when that is set
/// and holds it, else the compiled-in copy. None for a name that is not one of the page's files.
pub fn body(name: &str) -> Option<std::borrow::Cow<'static, str>> {
    let built: &'static str = if name == "viewer.html" { PAGE } else { file(name)?.body };
    if let Some(dir) = std::env::var_os(WEB_DIR_ENV)
        && let Ok(s) = std::fs::read_to_string(std::path::Path::new(&dir).join(name))
    {
        return Some(std::borrow::Cow::Owned(s));
    }
    Some(std::borrow::Cow::Borrowed(built))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_and_its_files_are_there_and_carry_no_em_dash() {
        assert!(PAGE.contains("<html"));
        for a in FILES {
            assert!(!a.body.is_empty(), "{}", a.name);
            assert!(!a.body.contains('\u{2014}'), "{} has an em dash", a.name);
        }
        assert!(!PAGE.contains('\u{2014}'));
        assert!(file("map.js").is_some());
        assert!(file("../Cargo.toml").is_none());
    }

    #[test]
    fn the_page_obeys_its_content_security_policy() {
        // No inline script, no inline style element, no style attributes (docs/api.md §8).
        let lower = PAGE.to_ascii_lowercase();
        assert!(!lower.contains("style=\""), "style attribute");
        assert!(!lower.contains("<style"), "style element");
        for (i, _) in lower.match_indices("<script") {
            let tag_end = lower[i..].find('>').unwrap() + i;
            assert!(lower[i..tag_end].contains("src="), "inline script at {i}");
        }
        // No inline handlers (` onclick="`, ` onload=` ...).
        for (i, _) in lower.match_indices(" on") {
            let name: String = lower[i + 3..].chars().take_while(|c| c.is_ascii_lowercase()).collect();
            let after = &lower[i + 3 + name.len()..];
            assert!(name.is_empty() || !after.starts_with('='), "inline handler on{name}");
        }
    }
}
