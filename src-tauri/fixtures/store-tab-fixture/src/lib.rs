//! The reference Store tab component: a plugin that adds pages to the Store,
//! and that hands the host, beside what it should show, everything it must not.
//!
//! `tabs` takes no argument and the world imports nothing that could vary, so
//! there is no selector to make it misbehave on request the way the runner
//! fixture does. Instead one fixed answer carries one of each fault the host
//! drops field by field. The faults that refuse a whole answer — too many tabs,
//! a duplicate key, an id outside its grammar — are tested on WIT records in
//! `plugin_runtime.rs`, where no component is needed to build them.
//!
//! | tab / game        | what it carries                                         |
//! | ----------------- | ------------------------------------------------------- |
//! | `deals`           | an uppercase `#rrggbb` accent, lowercased by the host    |
//! | `deals` / `alpha` | everything well-formed: https art, a discount, a page    |
//! | `deals` / `beta`  | a title with a direction override and a bell; an `http` |
//! |                   | cover, a hero with credentials, a landscape on an IP,   |
//! |                   | a logo that would close a CSS `url(…)`, a "discount"    |
//! |                   | above the regular price, a `javascript:` page            |
//! | `deals` / `gamma` | no price, and a page on a non-default port              |
//! | `free`            | a colour name instead of `#rrggbb`, a NUL in its label   |
//! | (no key)          | no games at all, so no tab                              |

wit_bindgen::generate!({
    world: "store-tab-plugin",
    path: "../../../wit",
});

use exports::orivo::plugin::{
    plugin_core::{Guest as CoreGuest, Health, Identity},
    store_tabs::{Guest as StoreTabsGuest, StorePrice, StoreTab, StoreTabGame},
};
use orivo::plugin::{
    host_journal::{self, JournalLevel},
    types::PluginError,
};

const PLUGIN_ID: &str = "com.orivo.fixture-store";
const PLUGIN_VERSION: &str = "1.0.0";
/// An image host the application's content policy already loads from, so the
/// well-formed pictures are ones the Store could really show.
const ART: &str = "https://cdn.cloudflare.steamstatic.com/steam/apps/620";

struct Fixture;

impl CoreGuest for Fixture {
    fn get_identity() -> Identity {
        // `extension-kind` has no case for a Store tab, so a plugin that only
        // adds one reports none: its manifest's `store_tab` is the declaration.
        Identity {
            id: PLUGIN_ID.into(),
            version: PLUGIN_VERSION.into(),
            extensions: Vec::new(),
        }
    }

    fn health_check() -> Result<Health, PluginError> {
        Ok(Health {
            ready: true,
            message: None,
        })
    }
}

impl StoreTabsGuest for Fixture {
    fn tabs() -> Result<Vec<StoreTab>, PluginError> {
        host_journal::log(JournalLevel::Info, "fixture store: three tabs, one empty");
        Ok(vec![deals(), free(), empty()])
    }
}

fn game(id: &str, title: &str) -> StoreTabGame {
    StoreTabGame {
        id: id.into(),
        title: title.into(),
        short_description: None,
        cover_url: None,
        hero_url: None,
        landscape_url: None,
        logo_url: None,
        genres: Vec::new(),
        price: None,
        page_url: None,
    }
}

fn deals() -> StoreTab {
    let alpha = StoreTabGame {
        short_description: Some("A short trip\nthrough a long level.".into()),
        cover_url: Some(format!("{ART}/library_600x900.jpg")),
        hero_url: Some(format!("{ART}/library_hero.jpg")),
        landscape_url: Some(format!("{ART}/header.jpg")),
        genres: vec!["Puzzle".into(), "Co-op".into()],
        price: Some(StorePrice {
            currency: "EUR".into(),
            amount_minor: 499,
            regular_amount_minor: Some(1999),
        }),
        page_url: Some("https://store.example.com/games/alpha".into()),
        ..game("alpha", "Alpha Quest")
    };
    let beta = StoreTabGame {
        cover_url: Some("http://store.example.com/beta.jpg".into()),
        hero_url: Some("https://player:secret@store.example.com/beta.jpg".into()),
        landscape_url: Some("https://127.0.0.1/beta.jpg".into()),
        logo_url: Some("https://store.example.com/beta.png') ; background: url(x".into()),
        price: Some(StorePrice {
            currency: "eur".into(),
            amount_minor: 500,
            regular_amount_minor: Some(400),
        }),
        page_url: Some("javascript:alert(1)".into()),
        ..game("beta", "Beta \u{202E}Run\u{7}")
    };
    let gamma = StoreTabGame {
        page_url: Some("https://store.example.com:8443/games/gamma".into()),
        ..game("gamma", "Gamma")
    };
    StoreTab {
        key: Some("deals".into()),
        label: "Fixture Deals".into(),
        accent: Some("#E4572E".into()),
        games: vec![alpha, beta, gamma],
    }
}

fn free() -> StoreTab {
    StoreTab {
        key: Some("free".into()),
        label: "Free\u{0} this week".into(),
        accent: Some("orange".into()),
        games: vec![StoreTabGame {
            price: Some(StorePrice {
                currency: "USD".into(),
                amount_minor: 0,
                regular_amount_minor: None,
            }),
            ..game("delta", "Delta")
        }],
    }
}

fn empty() -> StoreTab {
    StoreTab {
        key: None,
        label: "Coming soon".into(),
        accent: None,
        games: Vec::new(),
    }
}

export!(Fixture);
