//! Instant Gaming account connector.
//!
//! Instant Gaming is a key reseller, not a launcher: what you own there is a
//! purchase that was redeemed on Steam, Ubisoft Connect or another store. So
//! this connector imports purchases as library entries that carry their art and
//! their name, and never claims they can be started from Orivo.
//!
//! There is no account API, so the sync runs inside the authenticated sign-in
//! window, reads the account's own order history from that same origin, and
//! hands Rust only the product ids and titles it found. No cookie or session
//! token is ever copied out of the window.
//!
//! The order page has moved before, so its address is **discovered** rather
//! than assumed: the script follows the account link the site itself renders
//! and falls back to a list of known paths. Crucially, it refuses to read any
//! page it cannot confirm is an order history — the shop front is wall-to-wall
//! product links, and scraping it would import the entire catalogue as
//! purchases.

/// The sign-in form. A signed-in visitor is redirected onwards by the site
/// itself, so this is also the right entry point for a re-sync.
pub const LIBRARY_URL: &str = "https://www.instant-gaming.com/en/login/";

pub fn is_library_page(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && matches!(
            url.host_str(),
            Some("www.instant-gaming.com") | Some("instant-gaming.com")
        )
}

/// Read the order history from the signed-in account. Every page is parsed with
/// the browser's own parser into a detached document, so nothing from a response
/// is executed or inserted into a live tree.
pub const SYNC_START_SCRIPT: &str = r#"
(() => {
  window.__orivoSourceSync = { status: 'pending' };
  const finish = (value) => { window.__orivoSourceSync = value; };

  const ORIGIN = 'https://www.instant-gaming.com';
  // Known and historical order-history paths, most specific first.
  const CANDIDATES = [
    '/en/account/orders/',
    '/en/myaccount/orders/',
    '/en/orders/',
    '/en/account/',
    '/en/myaccount/',
  ];
  // A path is only trusted when it names an order history. The shop front is
  // nothing but product links; reading it would import the whole catalogue.
  const isOrderPath = (path) => /(^|\/)(orders?|commandes?)(\/|$|\?)/i.test(path);
  const productLink = /\/(\d{1,9})-([a-z0-9-]{2,180})/i;

  const parse = (html) => new DOMParser().parseFromString(html, 'text/html');
  const isSignedOut = (page, url) =>
    /\/(login|connexion|signin)\b/i.test(url) ||
    !!page.querySelector('input[type="password"], form[action*="login"], form[action*="connexion"]');

  const readOrders = (page) => {
    const games = new Map();
    for (const anchor of page.querySelectorAll('a[href]')) {
      const href = anchor.getAttribute('href') || '';
      const match = productLink.exec(href);
      if (!match) { continue; }
      const id = match[1];
      if (games.has(id)) { continue; }

      const image = anchor.querySelector('img') ||
        (anchor.parentElement ? anchor.parentElement.querySelector('img') : null);
      let title = (anchor.getAttribute('title') || anchor.textContent || '').trim();
      if (!title && image) { title = (image.getAttribute('alt') || '').trim(); }
      if (!title) {
        // Fall back to the slug, minus the reseller wording it always carries,
        // so a purchase is still recognisable.
        title = match[2]
          .replace(/^(buy|acheter)-/i, '')
          .replace(/-?(key|cle|cd-key)-/i, '-')
          .replace(/-(pc|steam|uplay|origin|epic|gog|xbox|ubisoft-connect)$/i, '')
          .replace(/-/g, ' ')
          .trim();
      }
      if (!title || title.length > 200) { continue; }

      const cover = image ? (image.getAttribute('src') || image.getAttribute('data-src') || '') : '';
      games.set(id, { id, title, cover });
      if (games.size >= 2000) { break; }
    }
    return Array.from(games.values());
  };

  const fetchPage = (url) =>
    fetch(url, { credentials: 'include', headers: { Accept: 'text/html' } })
      .then((response) => (response.ok ? response.text().then((html) => ({
        html,
        url: response.url || url,
      })) : null))
      .catch(() => null);

  (async () => {
    // Start from whatever the account area links to, then fall back to the
    // known paths. The first page that is genuinely an order history wins.
    const paths = [];
    for (const anchor of document.querySelectorAll('a[href]')) {
      const href = anchor.getAttribute('href') || '';
      if (isOrderPath(href) && paths.length < 4) { paths.push(href); }
    }
    for (const candidate of CANDIDATES) { paths.push(candidate); }

    let sawSignedOut = false;
    let sawOrderPage = false;
    for (const path of paths) {
      const absolute = path.startsWith('http') ? path : ORIGIN + path;
      if (!absolute.startsWith(ORIGIN)) { continue; }
      const page = await fetchPage(absolute);
      if (!page) { continue; }
      if (isSignedOut(parse(page.html), page.url)) { sawSignedOut = true; continue; }
      // Only an order history may be read, however it was reached.
      if (!isOrderPath(new URL(page.url, ORIGIN).pathname)) { continue; }
      sawOrderPage = true;
      const games = readOrders(parse(page.html));
      if (games.length > 0) {
        finish({ status: 'ok', accountLabel: 'Instant Gaming', games });
        return;
      }
    }

    if (sawSignedOut) { finish({ status: 'signed-out' }); return; }
    // An empty order history and a page Orivo could not recognise look the
    // same from here, so say so rather than claiming the account owns nothing.
    finish({ status: 'unsupported', sawOrderPage });
  })();

  return 'started';
})()
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sync_only_runs_on_the_instant_gaming_origin() {
        assert!(is_library_page(
            &reqwest::Url::parse("https://www.instant-gaming.com/en/login/").unwrap()
        ));
        assert!(!is_library_page(
            &reqwest::Url::parse("https://www.instant-gaming.com.evil.example/").unwrap()
        ));
        assert!(!is_library_page(
            &reqwest::Url::parse("http://www.instant-gaming.com/").unwrap()
        ));
    }

    #[test]
    fn the_start_script_settles_the_slot_on_every_branch() {
        assert!(SYNC_START_SCRIPT.contains(crate::sources::SESSION_RESULT_PROPERTY));
        assert!(SYNC_START_SCRIPT.contains("'signed-out'"));
        assert!(SYNC_START_SCRIPT.contains("'unsupported'"));
        assert!(SYNC_START_SCRIPT.contains("status: 'ok'"));
    }

    #[test]
    fn the_order_page_is_discovered_rather_than_assumed() {
        // The path that 404'd is still tried, but it is no longer the only one,
        // and the account link the site renders is preferred over any guess.
        assert!(SYNC_START_SCRIPT.contains("'/en/account/orders/'"));
        assert!(SYNC_START_SCRIPT.contains("'/en/myaccount/orders/'"));
        assert!(SYNC_START_SCRIPT.contains("isOrderPath"));
    }

    #[test]
    fn only_a_confirmed_order_history_is_ever_read() {
        // The shop front is wall-to-wall product links. Reading a page that is
        // not an order history would import the whole catalogue as purchases,
        // so the guard runs on the URL the response actually settled on.
        assert!(SYNC_START_SCRIPT.contains("if (!isOrderPath(new URL(page.url, ORIGIN).pathname)) { continue; }"));
    }
}
