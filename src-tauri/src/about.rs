//! Settings › About: the pages Orivo links to.
//!
//! The WebView sends which page, never a URL, so nothing it renders can turn
//! this command into a way to open an arbitrary link.

use serde::Deserialize;

use crate::store::{SystemUrlOpener, UrlOpener};

const REPOSITORY: &str = "https://github.com/justeozan/orivo";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OrivoPage {
    /// Every release, with what each one changed.
    Releases,
    /// A new issue on the repository.
    NewIssue,
}

fn page_url(page: OrivoPage) -> String {
    match page {
        OrivoPage::Releases => format!("{REPOSITORY}/releases"),
        OrivoPage::NewIssue => format!("{REPOSITORY}/issues/new"),
    }
}

#[tauri::command]
pub fn open_orivo_page(page: OrivoPage) -> Result<(), String> {
    SystemUrlOpener.open(&page_url(page))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_page_is_a_fixed_https_address_on_the_repository() {
        for page in [OrivoPage::Releases, OrivoPage::NewIssue] {
            assert!(page_url(page).starts_with("https://github.com/justeozan/orivo/"));
        }
        assert_eq!(
            page_url(OrivoPage::NewIssue),
            "https://github.com/justeozan/orivo/issues/new"
        );
    }

    #[test]
    fn the_webview_names_a_page_and_nothing_else() {
        let page: OrivoPage = serde_json::from_value(serde_json::json!("releases")).unwrap();
        assert_eq!(page, OrivoPage::Releases);
        assert!(serde_json::from_value::<OrivoPage>(serde_json::json!("https://example.com")).is_err());
    }
}
