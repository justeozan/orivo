//! Search helpers for configurable hubs (FitGirl, ...).
//!
//! The pure helpers (slug, candidate URLs, filtering) are always available.
//! The HTTP resolver is gated behind the `scraping` feature so the crate can
//! still be built for targets without a TLS stack.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hub {
    pub name: String,
    pub base_url: String,
}

impl Default for Hub {
    fn default() -> Self {
        Self {
            name: "fitgirl".into(),
            base_url: "https://fitgirl-repacks.site".into(),
        }
    }
}

impl Hub {
    pub fn new(name: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            base_url: base_url.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("http error: {0}")]
    Http(String),
    #[error("no game page found for {0}")]
    NotFound(String),
}

/// Convert a title to a URL slug the way QuikyTest does.
pub fn convert_to_slug(title: &str) -> String {
    let slug = title.to_lowercase();
    let mut out = String::new();
    for c in slug.chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else {
            out.push('-');
        }
    }
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    out.trim_matches('-').to_string()
}

/// Build candidate game-page URLs for a title on a hub.
pub fn candidate_urls(hub: &Hub, title: &str) -> Vec<String> {
    let slug = convert_to_slug(title);
    let base = hub.base_url.trim_end_matches('/');
    let mut urls = vec![format!("{}/{}", base, slug)];
    for suffix in [
        "-pc",
        "-repack",
        "-v",
        "-deluxe",
        "-goty",
        "-complete",
        "-edition",
    ] {
        urls.push(format!("{}/{}{}", base, slug, suffix));
    }
    urls.push(format!("{}/?s={}", base, title.replace(' ', "+")));
    urls
}

/// Filter a list of `(href, text)` links returned by a search page.
pub fn filter_search_results<'a>(
    links: &'a [(String, String)],
    title: &str,
) -> Vec<&'a (String, String)> {
    let needle = title.to_lowercase();
    links
        .iter()
        .filter(|(href, text)| {
            href.contains("fitgirl-repacks.site")
                && !href.contains("/tag/")
                && !href.contains("/category/")
                && !href.contains("/author/")
                && !href.contains("/page/")
                && !href.contains('#')
                && text.to_lowercase().contains(&needle)
        })
        .take(1)
        .collect()
}

/// Extract `(href, text)` pairs from a search results page.
pub fn extract_search_links(html: &str) -> Vec<(String, String)> {
    let re = regex::Regex::new(r#"(?is)<a[^>]+href\s*=\s*["']([^"']+)["'][^>]*>(.*?)</a>"#)
        .expect("valid regex");
    let tag = regex::Regex::new(r"(?is)<[^>]+>").expect("valid regex");
    re.captures_iter(html)
        .map(|cap| {
            let href = cap.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
            let text = tag
                .replace_all(cap.get(2).map(|m| m.as_str()).unwrap_or(""), "")
                .trim()
                .to_string();
            (href, text)
        })
        .collect()
}

/// Search pages rarely link the bare game title: the anchor text carries the
/// site name or a trailing dash. Keep the part a user would recognise.
pub fn clean_result_title(text: &str) -> String {
    let mut title = text.trim().to_string();
    for suffix in [
        " - FitGirl Repacks",
        " – FitGirl Repacks",
        " — FitGirl Repacks",
        " | FitGirl Repacks",
        " » FitGirl Repacks",
        " - FitGirl",
        " | FitGirl",
    ] {
        if title.to_lowercase().ends_with(&suffix.to_lowercase()) {
            title.truncate(title.len() - suffix.len());
            break;
        }
    }
    title.trim().to_string()
}

fn is_game_page(url: &url::Url, base: &url::Url) -> bool {
    if url.scheme() != "https" || url.host_str() != base.host_str() {
        return false;
    }
    let path = url.path();
    if path == "/" || path.is_empty() || url.query().is_some() || url.fragment().is_some() {
        return false;
    }
    const RESERVED: &[&str] = &[
        "/tag/",
        "/category/",
        "/author/",
        "/page/",
        "/feed/",
        "/comments/",
        "/wp-",
        "/go/",
        "/search/",
        "/about",
        "/contact",
        "/dmca",
        "/privacy",
        "/disclaimer",
        "/donate",
        "/faq",
        "/request",
        "/updates",
        "/news",
    ];
    !RESERVED.iter().any(|part| path.starts_with(part))
}

/// Turn the anchors of a search results page into deduplicated game pages.
///
/// Only same-host HTTPS article URLs are kept. A WordPress search page also
/// links categories, pagination and its own nav; letting those through would
/// ask the installer to scrape a non-game page and report "no magnet" later.
pub fn search_result_links(
    links: &[(String, String)],
    base_url: &str,
    limit: usize,
) -> Vec<SearchHit> {
    let Ok(base) = url::Url::parse(base_url) else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    let mut hits = Vec::new();
    for (raw_href, raw_text) in links {
        let Ok(url) = base.join(raw_href) else {
            continue;
        };
        if !is_game_page(&url, &base) {
            continue;
        }
        let title = clean_result_title(raw_text);
        if title.is_empty() || title.eq_ignore_ascii_case("fitgirl repacks") {
            continue;
        }
        let normalized = url.to_string();
        if !seen.insert(normalized.clone()) {
            continue;
        }
        hits.push(SearchHit {
            title,
            url: normalized,
        });
        if hits.len() >= limit {
            break;
        }
    }
    hits
}

/// Search a hub and return every game page on the first results page.
#[cfg(feature = "scraping")]
pub fn search_game_pages(
    client: &reqwest::blocking::Client,
    hub: &Hub,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchHit>, SearchError> {
    let encoded: String = url::form_urlencoded::byte_serialize(query.trim().as_bytes()).collect();
    let url = format!("{}/?s={}", hub.base_url.trim_end_matches('/'), encoded);
    let response = client
        .get(&url)
        .send()
        .map_err(|e| SearchError::Http(e.to_string()))?;
    if !response.status().is_success() {
        return Err(SearchError::Http(format!("search returned {}", response.status())));
    }
    let html = response
        .text()
        .map_err(|e| SearchError::Http(e.to_string()))?;
    Ok(search_result_links(
        &extract_search_links(&html),
        &hub.base_url,
        limit.clamp(1, 24),
    ))
}

/// Resolve the most likely game page URL for `title`.
#[cfg(feature = "scraping")]
pub fn resolve_game_url(
    client: &reqwest::blocking::Client,
    hub: &Hub,
    title: &str,
) -> Result<Option<String>, SearchError> {
    let urls = candidate_urls(hub, title);
    // Direct candidate pages first (all but the final search URL).
    for url in urls.iter().take(urls.len() - 1) {
        let response = client
            .head(url)
            .send()
            .map_err(|e| SearchError::Http(e.to_string()))?;
        if response.status().is_success() {
            return Ok(Some(url.clone()));
        }
    }

    // Search page fallback.
    let search_url = urls.last().expect("search url");
    let html = client
        .get(search_url)
        .send()
        .map_err(|e| SearchError::Http(e.to_string()))?
        .text()
        .map_err(|e| SearchError::Http(e.to_string()))?;
    let links = extract_search_links(&html);
    let found = filter_search_results(&links, title);
    Ok(found.first().map(|(href, _)| href.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_basic() {
        assert_eq!(convert_to_slug("Hello World!!"), "hello-world");
        assert_eq!(convert_to_slug("Dredge"), "dredge");
    }

    #[test]
    fn candidate_urls_include_variations() {
        let hub = Hub::default();
        let urls = candidate_urls(&hub, "Dredge");
        assert!(urls[0].ends_with("/dredge"));
        assert!(urls.iter().any(|u| u.ends_with("/dredge-pc")));
        assert!(urls.iter().any(|u| u.contains("?s=Dredge")));
    }

    #[test]
    fn filter_search_result() {
        let links = vec![
            (
                "https://fitgirl-repacks.site/tag/x".to_string(),
                "Dredge".to_string(),
            ),
            (
                "https://fitgirl-repacks.site/dredge/".to_string(),
                "Dredge".to_string(),
            ),
        ];
        let found = filter_search_results(&links, "Dredge");
        assert_eq!(found.len(), 1);
        assert!(found[0].0.ends_with("/dredge/"));
    }

    #[test]
    fn search_results_keep_only_game_pages() {
        let links = vec![
            (
                "https://fitgirl-repacks.site/mini-madness/".to_string(),
                "Mini Madness - FitGirl Repacks".to_string(),
            ),
            (
                "https://fitgirl-repacks.site/category/action/".to_string(),
                "Action".to_string(),
            ),
            (
                "https://fitgirl-repacks.site/page/2/?s=mini".to_string(),
                "Next".to_string(),
            ),
            (
                "https://fitgirl-repacks.site/mini-madness/".to_string(),
                "Mini Madness".to_string(),
            ),
            (
                "https://example.com/mini-madness/".to_string(),
                "Mini Madness".to_string(),
            ),
            (
                "https://fitgirl-repacks.site/stick-it-to-the-stickman/".to_string(),
                "Stick It to the Stickman".to_string(),
            ),
        ];
        let hits = search_result_links(&links, "https://fitgirl-repacks.site", 8);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "Mini Madness");
        assert!(hits[0].url.ends_with("/mini-madness/"));
        assert_eq!(hits[1].title, "Stick It to the Stickman");
    }

    #[test]
    fn clean_title_strips_site_suffix() {
        assert_eq!(
            clean_result_title("Mini Madness - FitGirl Repacks"),
            "Mini Madness"
        );
        assert_eq!(clean_result_title("  Dredge  "), "Dredge");
    }
}
