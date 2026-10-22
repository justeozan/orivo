use quiky_host::scrape::{extract_links, filter_by_kind, LinkKind};
use std::fs;

#[test]
fn fixture_scrape() {
    let html = fs::read_to_string("../../tests/fixtures/sample-page.html").unwrap();
    let links = extract_links(&html, "https://fitgirl-repacks.site/game/");
    assert_eq!(links.len(), 3);
    assert_eq!(filter_by_kind(&links, LinkKind::Magnet).len(), 1);
    assert_eq!(filter_by_kind(&links, LinkKind::Torrent).len(), 1);
    assert_eq!(filter_by_kind(&links, LinkKind::Direct).len(), 1);
}
