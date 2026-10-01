//! Orivo's Ryujinx runner: a real third-party plugin, written against the
//! published `orivo-plugin@1` contract and loaded by the host exactly like
//! anybody else's.
//!
//! It knows one thing — what a Nintendo Switch game file looks like — and it is
//! deliberately unable to know anything else. It calls `host-files.list-directory`
//! on the single grant the user approved and nothing more: no `read-file`, so the
//! user's `prod.keys`, firmware dump and save folder are unreachable *by
//! construction* rather than by promise, and the host's own `bytes_read` counter
//! is the proof (see `src-tauri/src/ryujinx_plugin.rs`). There is no WASI here at
//! all — no clock, no random source, no socket — because the host would refuse
//! to instantiate a component that asked for one.
//!
//! ## Why a file name is hex
//!
//! The host resolves the game file itself, by matching the opaque external
//! reference this component returns against the names it finds in the granted
//! folder. That reference has to pass the catalogue's opaque-id grammar
//! (`[A-Za-z0-9._\-:]`, `plugin_manifest::valid_opaque_id`), and a Switch dump is
//! conventionally named `Title [0100…][v0].nsp` — spaces and brackets, neither
//! of which the grammar allows. So a reference here is `x:` followed by the entry
//! name in **lower-case hexadecimal**: grammar-safe, and one spelling per file.
//! Both halves of that matter, and neither is decoration. `x:` is a namespace no
//! plain name can enter, because the host never lists a name containing `:`, so a
//! hex reference and a name can never describe one file between them; lower case
//! is the single canonical spelling, and a reference is the key of a library card,
//! so a second spelling would have been a second card for one file.
//! `runner_host::GrantedLibrary` knows that encoding; it still resolves the file
//! from its own listing and never joins anything this component said onto a path.
//!
//! ## What Ryujinx accepts, and where that list comes from
//!
//! `GAME_SUFFIXES` is Ryujinx's own library scanner, not a guess — see
//! `docs/ryujinx-runner.md` for the file and lines it was read from. Orivo never
//! ships Ryujinx, a key, a firmware dump or a game.

wit_bindgen::generate!({
    world: "runner-plugin",
    path: "../../wit",
});

use exports::orivo::plugin::{
    plugin_core::{Guest as CoreGuest, Health, Identity},
    runner::{
        Guest as RunnerGuest, LaunchIntent, ProfileValidation, RunnerGamePage, RunnerProfile,
    },
};
use orivo::plugin::{
    host_files::{self, DirectoryEntry},
    host_journal::{self, JournalLevel},
    types::{
        ExtensionKind, ExternalReference, GameCandidate, PageInfo, PageRequest, PluginError,
        PluginErrorCode,
    },
};

const PLUGIN_ID: &str = "com.orivo.ryujinx";
/// Must equal `package/manifest.json`'s `version` and `Cargo.toml`'s: the
/// registry's pre-install gate asks the component who it is and refuses a
/// package that disagrees with its own manifest.
const PLUGIN_VERSION: &str = "1.0.0";
/// The one directory grant this component asks for. `games` is deliberately the
/// slot `runner_commands::DEFAULT_DIRECTORY_SLOT` already defaults to, so the
/// "Add a folder" button needs no per-plugin knowledge to name it — the v1
/// manifest has no field where a package could declare its slots.
const GAMES_GRANT: &str = "games";

/// Every extension Ryujinx's own library scanner accepts, in its order.
/// Copied from `src/Ryujinx.UI.Common/App/ApplicationLibrary.cs` and
/// `src/Ryujinx.UI.Common/Configuration/FileTypes.cs`; `docs/ryujinx-runner.md`
/// records where those were read and what the macOS bundle declares beside
/// them.
const GAME_SUFFIXES: [&str; 6] = [".nsp", ".pfs0", ".xci", ".nca", ".nro", ".nso"];

/// What puts a reference in a namespace no plain file name can reach: the host
/// refuses to list a name containing `:`, so nothing it could ever show this
/// component collides with one of these.
const REFERENCE_PREFIX: &str = "x:";
const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

const PLATFORM: &str = "Nintendo Switch";

/// A Switch title id is sixteen hexadecimal digits, and a dump conventionally
/// carries it in square brackets. Sixteen is specific enough that `[v0]` and
/// `[US]` cannot be mistaken for one.
const TITLE_ID_DIGITS: usize = 16;

/// Hex spends two digits per byte inside the 256 an external reference may be,
/// minus the prefix, so a name past this cannot be addressed at all. It is
/// reported in the journal rather than silently dropped: a library that came
/// back short should say why. The host bounds a decoded reference at the same
/// number for the same reason.
const MAX_NAME_BYTES: usize = (256 - REFERENCE_PREFIX.len()) / 2;

/// More candidates than one page will ever carry — the host's own ceiling is
/// 100 — kept here so a caller asking for `u32::MAX` cannot make this component
/// allocate for it.
const MAX_PAGE_GAMES: usize = 100;

struct Ryujinx;

impl CoreGuest for Ryujinx {
    fn get_identity() -> Identity {
        Identity {
            id: PLUGIN_ID.into(),
            version: PLUGIN_VERSION.into(),
            extensions: vec![ExtensionKind::Runner],
        }
    }

    fn health_check() -> Result<Health, PluginError> {
        // Health is asked under the probe budget with no grants at all, between
        // install and configuration. There is nothing honest to check here: this
        // component holds no state, opens no file and reaches no network, and
        // whether Ryujinx itself is installed is a question about the profile's
        // application, which only the host can see.
        Ok(Health {
            ready: true,
            message: None,
        })
    }
}

impl RunnerGuest for Ryujinx {
    /// The v1 `runner-profile` record carries an id and a display name, so those
    /// are the only two things there are to judge. The emulation application and
    /// the folders are the host's, and it validates them itself.
    ///
    /// This deliberately does not look at the granted folder. A profile is
    /// created *before* its first folder is allowed, and nothing revalidates it
    /// afterwards, so a verdict that depended on a listing would leave every
    /// new profile permanently rejected.
    fn validate_profile(profile: RunnerProfile) -> Result<ProfileValidation, PluginError> {
        if profile.id.trim().is_empty() || profile.display_name.trim().is_empty() {
            return Ok(ProfileValidation {
                valid: false,
                message: Some("This runner profile has no name to be told apart by.".into()),
            });
        }
        Ok(ProfileValidation {
            valid: true,
            message: None,
        })
    }

    fn discover_page(
        profile_id: String,
        request: PageRequest,
    ) -> Result<RunnerGamePage, PluginError> {
        if profile_id.trim().is_empty() {
            return Err(error(
                PluginErrorCode::InvalidInput,
                "Orivo asked about a runner profile with no name.",
            ));
        }
        let entries = host_files::list_directory(GAMES_GRANT)?;
        // The cursor is a position in that listing rather than a name. The
        // listing is sorted and bounded by the host, so a position is stable
        // across calls — and unlike a name it cannot be mistaken for a path, nor
        // outgrow the 256 bytes a cursor is allowed. What it costs is stated in
        // `docs/ryujinx-runner.md`: a file that appears earlier in the alphabet
        // between two pages of one import is seen by the next import, not by
        // this one.
        let start = match request.cursor.as_deref() {
            None => 0,
            Some(cursor) => parse_cursor(cursor)?,
        };
        let limit = (request.limit.max(1) as usize).min(MAX_PAGE_GAMES);

        let mut games = Vec::new();
        let mut next_cursor = None;
        let mut unsupported = 0_usize;
        let mut unaddressable = 0_usize;
        for (index, entry) in entries.iter().enumerate().skip(start) {
            let Some(suffix) = game_suffix(entry) else {
                unsupported += 1;
                continue;
            };
            if entry.name.len() > MAX_NAME_BYTES {
                unaddressable += 1;
                continue;
            }
            if games.len() == limit {
                next_cursor = Some(index.to_string());
                break;
            }
            games.push(candidate(&entry.name, suffix));
        }

        // One bounded line per page, not one per entry: this is what the
        // Settings → Plugins log has to show when a folder of ROMs imported
        // fewer games than the user can see in Finder.
        host_journal::log(
            JournalLevel::Info,
            &format!(
                "{} Switch game(s) on this page; skipped {unsupported} entries Ryujinx does \
                 not open and {unaddressable} name(s) Orivo cannot refer to",
                games.len()
            ),
        );

        Ok(RunnerGamePage {
            page: PageInfo {
                complete: next_cursor.is_none(),
                next_cursor,
            },
            games,
        })
    }

    /// Opaque ids only, echoed back exactly as received, under the one launch
    /// mode the v1 contract declares. The host resolves the application, the
    /// working directory and the single argument from the profile — never from
    /// anything returned here.
    ///
    /// No listing is taken: `prepare-launch` runs under the interactive budget
    /// and the plan asks it to stay short and do no disk scan. The check below
    /// is arithmetic on the reference itself, and the host re-verifies the file
    /// immediately before it starts a process anyway.
    fn prepare_launch(
        profile_id: String,
        game_reference: String,
    ) -> Result<LaunchIntent, PluginError> {
        let offerable = decode_name(&game_reference).is_some_and(|name| {
            name.len() <= MAX_NAME_BYTES
                && listable_name(&name)
                && recognised_suffix(&name).is_some()
        });
        if !offerable {
            return Err(error(
                PluginErrorCode::InvalidInput,
                "That is not a Nintendo Switch game file this runner offered.",
            ));
        }
        Ok(LaunchIntent {
            runner_id: PLUGIN_ID.into(),
            profile_id,
            game_reference,
            mode: "default".into(),
        })
    }
}

/// One library card, built from a file name and nothing else. No byte of the
/// file itself is read: a title id printed in the name is the only identity a
/// runner needs, and reading an `.nsp` header would mean opening a file the user
/// allowed this plugin to *list*.
///
fn candidate(name: &str, suffix_len: usize) -> GameCandidate {
    let stem = &name[..name.len() - suffix_len];
    let title = readable_title(name, stem);
    let title_id = title_id(stem);
    GameCandidate {
        reference: ExternalReference {
            provider_id: PLUGIN_ID.into(),
            external_id: encode_name(name),
        },
        title,
        sort_title: None,
        // The v1 `game-candidate` has no field for an external title id, and
        // `platform` is the one that lands on the card's own metadata line, so
        // the id travels there where a user can read and search it.
        platform: Some(match title_id {
            Some(id) => format!("{PLATFORM} · {id}"),
            None => PLATFORM.to_string(),
        }),
        installed: true,
    }
}

/// The recognised suffix's byte length, or `None` for anything else in the
/// folder. Matched case-insensitively because Ryujinx's own scanner lowercases
/// before it compares, so `GAME.NSP` is a game there and has to be one here.
///
/// Hidden entries are skipped, and that *is* parity with Ryujinx rather than a
/// departure from it: its `EnumerationOptions` leaves `AttributesToSkip` at the
/// default `Hidden | System`, so a dot-file never reaches its game list.
/// `host-files` has no such default and reports them, which is why the rule has
/// to live here. What it costs to get wrong is concrete: on an exFAT or FAT
/// volume macOS writes an AppleDouble sidecar `._<name>` beside every file, it
/// carries the same `.nsp` suffix as the dump it shadows, and it would earn a
/// second card with the same title and the same title id whose four kilobytes
/// Play would hand to the emulator.
fn game_suffix(entry: &DirectoryEntry) -> Option<usize> {
    if entry.directory || !listable_name(&entry.name) {
        return None;
    }
    recognised_suffix(&entry.name)
}

/// A name the host could have shown this component, and therefore one it could
/// have offered.
///
/// The host asks the same question of a decoded reference and is the authority;
/// this is the plugin declining to say something it could never have meant. A
/// single ordinary component, not hidden: no separator, no `:` (which a platform
/// reads as a drive or a stream), no control character, and no leading dot — see
/// `game_suffix` for what the dot costs on an exFAT volume.
fn listable_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(char::is_control)
}

fn recognised_suffix(name: &str) -> Option<usize> {
    GAME_SUFFIXES.iter().find_map(|suffix| {
        let start = name.len().checked_sub(suffix.len())?;
        // A file called exactly `.nsp` has no title in it, and treating it as
        // one would put a nameless card in the library.
        (start > 0 && name.as_bytes()[start..].eq_ignore_ascii_case(suffix.as_bytes()))
            .then_some(suffix.len())
    })
}

/// One reference per file, spelled one way. The digit table is explicit because
/// the canonical spelling is the whole point: the host accepts lower case only,
/// and a card is keyed by its reference, so a second spelling of one name would
/// be a second card for one file.
fn encode_name(name: &str) -> String {
    let mut encoded = String::with_capacity(REFERENCE_PREFIX.len() + name.len() * 2);
    encoded.push_str(REFERENCE_PREFIX);
    for byte in name.as_bytes() {
        encoded.push(HEX_DIGITS[usize::from(byte >> 4)] as char);
        encoded.push(HEX_DIGITS[usize::from(byte & 0x0f)] as char);
    }
    encoded
}

/// The inverse, used only to judge a reference the host handed back. It refuses
/// anything that is not exactly what `encode_name` produces — a missing prefix, an
/// odd length, an upper-case or non-hex digit, or bytes that are not UTF-8 —
/// because a reference that decodes to something else is not one this component
/// issued.
fn decode_name(reference: &str) -> Option<String> {
    let digits = reference.strip_prefix(REFERENCE_PREFIX)?.as_bytes();
    if digits.is_empty() || !digits.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(digits.len() / 2);
    for pair in digits.chunks(2) {
        let high = lower_hex_digit(pair[0])?;
        let low = lower_hex_digit(pair[1])?;
        bytes.push((high << 4) | low);
    }
    String::from_utf8(bytes).ok()
}

fn parse_cursor(cursor: &str) -> Result<usize, PluginError> {
    cursor.parse::<usize>().map_err(|_| {
        error(
            PluginErrorCode::InvalidInput,
            "This runner did not issue that page cursor.",
        )
    })
}

/// The sixteen-hex-digit title id a dump conventionally prints in brackets,
/// upper-cased. Only a bracketed group counts: sixteen hex digits loose in a
/// title would just as likely be a release group's checksum.
fn title_id(stem: &str) -> Option<String> {
    let bytes = stem.as_bytes();
    let mut index = 0;
    while let Some(open) = bytes[index..].iter().position(|byte| *byte == b'[') {
        let start = index + open + 1;
        let Some(close) = bytes[start..].iter().position(|byte| *byte == b']') else {
            return None;
        };
        let group = &stem[start..start + close];
        if group.len() == TITLE_ID_DIGITS && group.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Some(group.to_ascii_uppercase());
        }
        index = start + close + 1;
        if index >= bytes.len() {
            break;
        }
    }
    None
}

/// A name a player would recognise. Square-bracket groups are where a dump puts
/// its title id and version, so they come off; parentheses stay, because that is
/// where the region usually is and a region is part of what tells two dumps
/// apart.
fn readable_title(name: &str, stem: &str) -> String {
    let mut title = String::with_capacity(stem.len());
    let mut depth = 0_usize;
    for character in stem.chars() {
        match character {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => title.push(character),
            _ => {}
        }
    }
    let collapsed = title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(['-', '_', '.', ' '])
        .to_string();
    // Three fallbacks, in order, because the host rejects an empty title and
    // takes the whole page down with it. A file called only
    // `[0100000000010000].nsp` has nothing left once the brackets come off, so
    // its stem is the label; a file called ` .nsp` — or one named with a
    // no-break space, or U+3000 — has a stem that is blank as well, and its own
    // file name is all there is.
    //
    // The last one cannot be blank, so there is no fourth case and no card left
    // unmade: this is only ever called for a name that matched a suffix of at
    // least four printable characters, and `trim` removes none of them.
    [collapsed.as_str(), stem.trim(), name.trim()]
        .into_iter()
        .find(|candidate| !candidate.is_empty())
        .unwrap_or(name)
        .to_owned()
}

/// Lower case only, so one file has one reference. `char::to_digit(16)` takes
/// both cases, which would have given a name of *k* hex-significant bytes 2^k of
/// them.
fn lower_hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn error(code: PluginErrorCode, message: &str) -> PluginError {
    PluginError {
        code,
        message: message.into(),
        retryable: false,
    }
}

export!(Ryujinx);
