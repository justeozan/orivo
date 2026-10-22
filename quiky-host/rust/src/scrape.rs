//! HTML scraping helpers for game pages.

use regex::Regex;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkKind {
    Magnet,
    Torrent,
    Direct,
    Other,
}

#[derive(Debug, Clone)]
pub struct ScrapedLink {
    pub kind: LinkKind,
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DownloadLinks {
    pub magnet: Option<String>,
    pub torrent: Option<String>,
    pub direct: Option<String>,
}

impl DownloadLinks {
    pub fn is_empty(&self) -> bool {
        self.magnet.is_none() && self.torrent.is_none() && self.direct.is_none()
    }

    /// Priority used by QuikyTest: magnet > torrent > direct.
    pub fn preferred_source(&self) -> Option<&str> {
        self.magnet
            .as_deref()
            .or(self.torrent.as_deref())
            .or(self.direct.as_deref())
    }
}

pub fn classify(url: &str) -> LinkKind {
    let lower = url.to_lowercase();
    if lower.starts_with("magnet:") {
        LinkKind::Magnet
    } else if lower.ends_with(".torrent")
        || lower.contains(".torrent?")
        || lower.contains(".torrent#")
    {
        LinkKind::Torrent
    } else if is_direct_host(&lower) || has_file_extension(&lower) {
        LinkKind::Direct
    } else {
        LinkKind::Other
    }
}

fn is_direct_host(lower: &str) -> bool {
    [
        "1fichier.com",
        "mega.nz",
        "mediafire.com",
        "uploadhaven.com",
        "fuckingfast.co",
        "fuckingfast.net",
        "datanodes.to",
        "gofile.io",
        "buzzheavier.com",
        "pixeldrain.com",
        "krakenfiles.com",
        "qiwi.gg",
        "ufile.io",
        "filecrypt.cc",
        "mixdrop.co",
        "anonfiles.com",
    ]
    .iter()
    .any(|host| lower.contains(host))
}

fn has_file_extension(lower: &str) -> bool {
    let path = lower.split(['?', '#']).next().unwrap_or(lower);
    [".rar", ".zip", ".7z", ".iso", ".bin", ".exe", ".tar", ".gz"]
        .iter()
        .any(|ext| path.ends_with(ext))
}

/// Decode the handful of HTML entities that appear in href attributes.
///
/// FitGirl emits magnet links as `...&#038;dn=...`; passing that to aria2 keeps
/// the literal `&#038;` and breaks the tracker list.
fn unescape_html(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&#038;", "&")
        .replace("&#38;", "&")
        .replace("&#x26;", "&")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

/// Extract all links from an HTML page, resolving relative URLs.
pub fn extract_links(html: &str, base_url: &str) -> Vec<ScrapedLink> {
    let base = Url::parse(base_url).ok();
    let re = Regex::new(r#"href\s*=\s*["']([^"']+)["']"#).unwrap();
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for cap in re.captures_iter(html) {
        let raw = unescape_html(cap.get(1).map(|m| m.as_str()).unwrap_or(""));
        if raw.is_empty() || raw.starts_with('#') || raw.starts_with("javascript:") {
            continue;
        }
        let resolved = if let Some(b) = &base {
            b.join(&raw)
                .map(|u| u.to_string())
                .unwrap_or_else(|_| raw.clone())
        } else {
            raw.clone()
        };
        if seen.insert(resolved.clone()) {
            out.push(ScrapedLink {
                kind: classify(&resolved),
                url: resolved,
            });
        }
    }
    out
}

pub fn filter_by_kind(links: &[ScrapedLink], kind: LinkKind) -> Vec<String> {
    links
        .iter()
        .filter(|l| l.kind == kind)
        .map(|l| l.url.clone())
        .collect()
}

/// Group scraped links into magnet/torrent/direct buckets.
pub fn group_links(links: &[ScrapedLink]) -> DownloadLinks {
    DownloadLinks {
        magnet: filter_by_kind(links, LinkKind::Magnet).into_iter().next(),
        torrent: filter_by_kind(links, LinkKind::Torrent).into_iter().next(),
        direct: filter_by_kind(links, LinkKind::Direct).into_iter().next(),
    }
}

/// Read the repack size printed on a FitGirl page (`Repack Size: 1.4 GB`).
///
/// The value is a display approximation, not a byte count: it is used to size
/// the progress bar before aria2 has metadata, never to accept or reject a
/// download. Parsing it is therefore deliberately forgiving.
pub fn parse_repack_size(html: &str) -> Option<u64> {
    let re = Regex::new(
        r"(?is)Repack\s+Size\s*:?\s*(?:<[^>]+>\s*)*(?:from\s*)?([0-9]+(?:[.,][0-9]+)?)\s*(TB|GB|MB|GiB|MiB)",
    )
    .ok()?;
    let caps = re.captures(html)?;
    let value = caps.get(1)?.as_str().replace(',', ".").parse::<f64>().ok()?;
    let multiplier = match caps.get(2)?.as_str().to_ascii_lowercase().as_str() {
        "tb" => 1024_u64.pow(4),
        "gb" | "gib" => 1024_u64.pow(3),
        "mb" | "mib" => 1024_u64.pow(2),
        _ => return None,
    };
    let bytes = (value * multiplier as f64).round() as u64;
    (bytes > 0).then_some(bytes)
}

/// Fetch a game page and return its download links.
#[cfg(feature = "scraping")]
pub fn scrape_game_page(
    client: &reqwest::blocking::Client,
    url: &str,
) -> Result<DownloadLinks, String> {
    let html = client
        .get(url)
        .send()
        .map_err(|e| e.to_string())?
        .text()
        .map_err(|e| e.to_string())?;
    Ok(group_links(&extract_links(&html, url)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_magnet() {
        assert_eq!(classify("magnet:?xt=urn:btih:abc"), LinkKind::Magnet);
    }

    #[test]
    fn classify_torrent() {
        assert_eq!(classify("https://x/y.torrent"), LinkKind::Torrent);
    }

    #[test]
    fn classify_direct() {
        assert_eq!(classify("https://1fichier.com/x"), LinkKind::Direct);
        assert_eq!(classify("https://mega.nz/x"), LinkKind::Direct);
        assert_eq!(
            classify("https://fuckingfast.co/p3j7da5fapi1"),
            LinkKind::Direct
        );
        assert_eq!(
            classify("https://example.com/game.rar?token=x"),
            LinkKind::Direct
        );
    }

    #[test]
    fn classify_ignores_banner_and_torrent_page() {
        assert_eq!(
            classify("https://www.internetdownloadmanager.com/download.html"),
            LinkKind::Other
        );
        assert_eq!(
            classify("https://1337x.to/torrent/6738793/Stick-It-to-the-Stickman/"),
            LinkKind::Other
        );
    }

    #[test]
    fn extract_resolves_relative() {
        let html = r#"<a href="magnet:?xt=urn:btih:abc">m</a><a href="/files/x.torrent">t</a><a href="https://1fichier.com/dl">d</a>"#;
        let links = extract_links(html, "https://fitgirl-repacks.site/game/");
        assert_eq!(links.len(), 3);
        assert!(links
            .iter()
            .any(|l| l.url == "https://fitgirl-repacks.site/files/x.torrent"));
    }

    #[test]
    fn extract_decodes_html_entities_in_magnet() {
        let html = r#"<a href="magnet:?xt=urn:btih:abc&#038;dn=Game&#038;tr=udp://x:80">m</a>"#;
        let links = extract_links(html, "https://fitgirl-repacks.site/game/");
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].url,
            "magnet:?xt=urn:btih:abc&dn=Game&tr=udp://x:80"
        );
        assert_eq!(links[0].kind, LinkKind::Magnet);
    }

    #[test]
    fn parse_repack_size_from_page_copy() {
        let html = "<p><strong>Repack Size:</strong> from 1.4 GB</p>";
        assert_eq!(parse_repack_size(html), Some(1_503_238_554));
        assert_eq!(
            parse_repack_size("<p>Repack Size: 780 MB</p>"),
            Some(817_889_280)
        );
        assert_eq!(parse_repack_size("<p>no size here</p>"), None);
    }

    #[test]
    fn group_prefers_magnet() {
        let html = r#"<a href="magnet:?xt=urn:btih:abc">m</a><a href="/x.torrent">t</a><a href="https://1fichier.com/x">d</a>"#;
        let groups = group_links(&extract_links(html, "https://example.com/"));
        assert!(groups.magnet.is_some());
        assert!(groups.preferred_source().unwrap().starts_with("magnet:"));
    }
}
