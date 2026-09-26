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
///
/// The WebAuth SPA refuses a bare `/login`: with no `appId` it POSTs an empty
/// guid to `validateGuid`, marks the config invalid, and navigates itself to
/// `/error` — the Ubisoft ERROR screen Orivo's window used to show. `appId` is
/// the Connect web client's own id (the one Ubisoft's site links with).
/// `nexturl` stays on the Connect origin because the default (`ubisoft.com`)
/// or the bare host (redirects to the marketing site) would take the window
/// off `connect.ubisoft.com` after sign-in, where the sync never runs.
pub const LIBRARY_URL: &str = "https://connect.ubisoft.com/login?appId=412802ED-8163-4642-A931-8299F209FECB&nexturl=https%3A%2F%2Fconnect.ubisoft.com%2Flogin";

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
  // After sign-in the SPA writes the whole profile session to
  // `PRODloginData` (`ticket` + `sessionId` + …). GraphQL rejects a ticket
  // without `Ubi-SessionId` (HTTP 400 → `error` → SourceError::Network,
  // "could not be reached"), so both fields are required.
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
      const sessionId = parsed.sessionId || parsed.SessionId;
      if (typeof ticket === 'string' && ticket.length > 20 &&
          typeof sessionId === 'string' && sessionId.length > 0) {
        return { ticket, sessionId, key, storage };
      }
    }
    return null;
  };

  const session = findSession();
  // No session yet: the window is still on the sign-in form. Keep it open
  // (signed-out + wait_for_sign_in retries) instead of probing GraphQL.
  if (!session) { finish({ status: 'signed-out' }); return 'started'; }

  // 314d4fef… (club) is unauthorized on this GraphQL route and answers 401
  // UNAUTHORIZED_APPLICATION. 86263886… is Ubisoft Connect's own AppId and
  // reaches ticket validation (401 INVALID_TICKET when the ticket is bad).
  const query = '{ viewer { id ownedGames { totalCount nodes { id spaceId name boxArt } } } }';
  fetch('https://public-ubiservices.ubi.com/v1/profiles/me/uplay/graphql', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Ubi-AppId': '86263886-327a-4328-ac69-527f0d20a237',
      'Ubi-RequestedPlatformType': 'uplay',
      'Ubi-SessionId': session.sessionId,
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
      const code = typeof error === 'number' ? error : 0;
      if (code === 401 || code === 403 || code === 400) {
        // Stale or malformed session (previous attempt / mid-write). Drop it
        // so the next probe reports signed-out and the form stays usable
        // instead of failing the connect after three quick Network errors.
        try { session.storage.removeItem(session.key); } catch (ignored) {}
        finish({ status: 'signed-out' });
        return;
      }
      finish({
        status: 'error',
        detail: typeof error === 'string'
          ? error
          : String((error && error.message) || error)
      });
    });
  return 'started';
})()
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sign_in_url_carries_the_app_id_webauth_requires() {
        let url = reqwest::Url::parse(LIBRARY_URL).unwrap();
        assert_eq!(url.host_str(), Some("connect.ubisoft.com"));
        assert_eq!(url.path(), "/login");
        let params: Vec<(String, String)> = url
            .query_pairs()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        // Without appId the SPA lands on /error instead of the sign-in form.
        assert_eq!(
            params
                .iter()
                .find(|(key, _)| key == "appId")
                .map(|(_, value)| value.as_str()),
            Some("412802ED-8163-4642-A931-8299F209FECB")
        );
        // nexturl must stay on this origin or the in-page sync never runs.
        let nexturl = params
            .iter()
            .find(|(key, _)| key == "nexturl")
            .map(|(_, value)| value.as_str())
            .expect("the sign-in URL pins where sign-in returns to");
        let next = reqwest::Url::parse(nexturl).unwrap();
        assert!(is_library_page(&next));
    }

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

    #[test]
    fn the_start_script_sends_the_session_header_graphql_requires() {
        // Without Ubi-SessionId the route answers HTTP 400 → status `error` →
        // SourceError::Network ("could not be reached") after three failures.
        assert!(SYNC_START_SCRIPT.contains("Ubi-SessionId"));
        assert!(SYNC_START_SCRIPT.contains("session.sessionId"));
        // The club AppId is unauthorized on this GraphQL route (401
        // UNAUTHORIZED_APPLICATION); Connect's own AppId reaches ticket checks.
        assert!(SYNC_START_SCRIPT.contains("86263886-327a-4328-ac69-527f0d20a237"));
        assert!(!SYNC_START_SCRIPT.contains("314d4fef-e568-4f85-aa05-42c19632e58f"));
        // Session is only usable when both ticket and sessionId were found.
        assert!(SYNC_START_SCRIPT.contains("parsed.sessionId"));
        // A 400/401/403 on the probe means the stored session is stale: drop
        // the key and report signed-out so the form can be used again.
        assert!(SYNC_START_SCRIPT.contains("code === 400"));
        assert!(SYNC_START_SCRIPT.contains("removeItem(session.key)"));
        // Other failures carry a detail string for the Rust-side log.
        assert!(SYNC_START_SCRIPT.contains("detail:"));
    }
}
