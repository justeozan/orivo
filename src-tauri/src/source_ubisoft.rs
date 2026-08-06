//! Ubisoft Connect account connector.
//!
//! Ubisoft publishes no account API and no OAuth client an application may use,
//! so this connector deliberately never tries to hold a Ubisoft credential. The
//! sign-in window stays signed in, and each sync runs *inside* that
//! authenticated origin: the page reads its own session ticket out of its own
//! storage, calls Ubisoft's own library endpoint, and hands Rust nothing but a
//! list of game ids and names.
//!
//! That is a real constraint, not a shortcut. It means Ubisoft can change its
//! web client and break this, which is why the script reports `unsupported`
//! rather than guessing when the answer no longer has the shape it expects.

/// Open the sign-in form directly. Landing on the Connect home instead showed
/// the marketing site with no visible way in, which is what made the window
/// look like it had failed to load anything useful.
pub const LIBRARY_URL: &str = "https://connect.ubisoft.com/login";

/// Sign-in may roam across Ubisoft's identity hosts; the sync only ever runs on
/// the Connect origin itself.
pub fn is_library_page(url: &reqwest::Url) -> bool {
    url.scheme() == "https" && url.host_str() == Some("connect.ubisoft.com")
}

/// Start the in-page sync. The result lands on `window.__orivoSourceSync`; see
/// `sources::SESSION_POLL_SCRIPT` for how Rust reads it back.
pub const SYNC_START_SCRIPT: &str = r#"
(() => {
  window.__orivoSourceSync = { status: 'pending' };
  const finish = (value) => { window.__orivoSourceSync = value; };

  // The web client keeps its session under a key whose name has changed more
  // than once. Searching for the shape instead of the name is what keeps this
  // working across Ubisoft's own renames.
  const findSession = () => {
    let storage;
    try { storage = window.localStorage; } catch (error) { return null; }
    if (!storage) { return null; }
    for (let index = 0; index < storage.length; index += 1) {
      const key = storage.key(index);
      if (!key) { continue; }
      const raw = storage.getItem(key);
      if (!raw || raw.length < 24 || raw[0] !== '{') { continue; }
      let parsed;
      try { parsed = JSON.parse(raw); } catch (error) { continue; }
      if (!parsed || typeof parsed !== 'object') { continue; }
      const ticket = parsed.ticket || parsed.Ticket;
      if (typeof ticket === 'string' && ticket.length > 20) {
        return { ticket };
      }
    }
    return null;
  };

  const session = findSession();
  if (!session) { finish({ status: 'signed-out' }); return 'started'; }

  const query = '{ viewer { id ownedGames { totalCount nodes { id spaceId name boxArt } } } }';
  fetch('https://public-ubiservices.ubi.com/v1/profiles/me/uplay/graphql', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Ubi-AppId': '314d4fef-e568-4f85-aa05-42c19632e58f',
      'Ubi-RequestedPlatformType': 'uplay',
      'Authorization': 'Ubi_v1 t=' + session.ticket
    },
    body: JSON.stringify({ query })
  })
    .then((response) => (response.ok ? response.json() : Promise.reject(response.status)))
    .then((payload) => {
      const nodes =
        payload && payload.data && payload.data.viewer && payload.data.viewer.ownedGames
          ? payload.data.viewer.ownedGames.nodes
          : null;
      if (!Array.isArray(nodes)) { finish({ status: 'unsupported' }); return; }
      const games = [];
      for (const node of nodes.slice(0, 2000)) {
        if (!node || node.id === undefined || node.id === null || !node.name) { continue; }
        games.push({
          id: String(node.id),
          title: String(node.name),
          cover: typeof node.boxArt === 'string' ? node.boxArt : ''
        });
      }
      finish({ status: 'ok', accountLabel: 'Ubisoft Connect', games });
    })
    .catch((error) => {
      finish({ status: error === 401 || error === 403 ? 'signed-out' : 'error' });
    });
  return 'started';
})()
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sync_only_runs_on_the_connect_origin() {
        assert!(is_library_page(
            &reqwest::Url::parse("https://connect.ubisoft.com/games").unwrap()
        ));
        assert!(!is_library_page(
            &reqwest::Url::parse("https://account.ubisoft.com/login").unwrap()
        ));
        assert!(!is_library_page(
            &reqwest::Url::parse("https://connect.ubisoft.com.evil.example/").unwrap()
        ));
        assert!(!is_library_page(
            &reqwest::Url::parse("http://connect.ubisoft.com/").unwrap()
        ));
    }

    #[test]
    fn the_start_script_publishes_to_the_slot_rust_polls() {
        assert!(SYNC_START_SCRIPT.contains(crate::sources::SESSION_RESULT_PROPERTY));
        // Every terminal branch has to settle the slot, or a sync would hang
        // until its deadline instead of reporting something actionable.
        assert!(SYNC_START_SCRIPT.contains("'signed-out'"));
        assert!(SYNC_START_SCRIPT.contains("'unsupported'"));
        assert!(SYNC_START_SCRIPT.contains("'error'"));
        assert!(SYNC_START_SCRIPT.contains("status: 'ok'"));
    }
}
