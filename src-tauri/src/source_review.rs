//! What a file found in a shared folder is allowed to say for itself.
//!
//! Two source flows put files from a folder the user granted in front of them and
//! ask whether those files should become library cards: Winlator's exported
//! shortcuts, and the ROM folders of the console emulators. In both, the name
//! shown came *out of the file* — a `Name=` line, or a file name — and any app can
//! create a file in a shared folder with no permission at all. So the question the
//! user answers has to carry something they can check, and has to say when a name
//! is already somebody else's.
//!
//! Those rules live here rather than in each flow, because a second copy of a
//! security-relevant rule is a second place for it to be wrong.

use serde::Serialize;
use std::{collections::BTreeSet, path::Path};

/// What a granted folder or a file is called when its own name cannot be shown.
const UNNAMED_FILE: &str = "an unnamed file";
const ELIDED_PATH_COMPONENT: &str = "…";

/// Whose name a found file is also using.
///
/// A planted file named after a game the user already has is the whole point of
/// naming it, so the collision is said out loud rather than left for the user to
/// notice. The library case is the louder one and wins when both hold.
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SourceTitleCollision {
    #[default]
    None,
    /// A game already in the library carries this title.
    Library,
    /// Another file in the connected folder carries it.
    Folder,
}

/// The form two titles are compared in.
///
/// Case-insensitive because it is a person reading a menu that this protects, and
/// whitespace-insensitive for the same reason: `"Celeste "` and `"Celeste"` are
/// one name to that reader. Characters that occupy no width go too — a title that
/// reads identically has to *compare* identically, and the titles on the other
/// side of this comparison are the library's, which came from anywhere at all and
/// never went through [`display_text`].
///
/// **What this does not fold, exactly.** Two names that differ only by Unicode
/// *normalisation* still compare different: `"Pokémon"` composed (`U+00E9`) and
/// decomposed (`e` + `U+0301`) are two strings here. Folding them would need
/// decomposition tables — a new dependency for a check that *warns* rather than
/// refuses — and it would still not catch a homoglyph, which is the same attack
/// with none of the machinery: a Cyrillic `С` is simply a different letter, and
/// no amount of normalising makes it equal to `C`. Both are stated in
/// `docs/console-emulators.md` rather than implied away. The user still sees the
/// file name and the folder on every row, which is the part that does not lie.
pub fn folded_title(title: &str) -> String {
    title
        .chars()
        .filter(|character| !character.is_whitespace() && !is_invisible(*character))
        .flat_map(char::to_lowercase)
        .collect()
}

/// Whose name this file is also using, among the library's titles and the other
/// files found beside it.
pub fn title_collision<'a>(
    title: &str,
    taken_titles: &BTreeSet<String>,
    found: impl Iterator<Item = &'a str>,
) -> SourceTitleCollision {
    let folded = folded_title(title);
    if taken_titles.contains(&folded) {
        return SourceTitleCollision::Library;
    }
    let sharing = found
        .filter(|other| folded_title(other) == folded)
        .take(2)
        .count();
    if sharing > 1 {
        SourceTitleCollision::Folder
    } else {
        SourceTitleCollision::None
    }
}

/// Every title already spoken for, in the form [`title_collision`] compares.
pub fn taken_titles<'a>(titles: impl Iterator<Item = &'a str>) -> BTreeSet<String> {
    titles.map(folded_title).collect()
}

/// How a file is named on the confirmation: the file itself, and the folders
/// between the connected one and it.
///
/// Relative to the grant, because the absolute path is host-private and naming it
/// would tell the WebView where shared storage is mounted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOrigin {
    pub file_name: String,
    /// The folders below the connected one, `/`-joined. Empty when the file sits
    /// directly in it.
    pub folder_path: String,
}

/// What a file is called, and where it sits inside the folder that was granted. A
/// component the host cannot display is elided rather than dropped: the shape of
/// the nesting is itself part of what the user is checking.
pub fn file_origin(root: &Path, file: &Path, max_chars: usize) -> FileOrigin {
    let file_name = file
        .file_name()
        .map(|name| name.to_string_lossy())
        .and_then(|name| display_text(&name, max_chars))
        .unwrap_or_else(|| UNNAMED_FILE.into());
    let folder_path = file
        .parent()
        .and_then(|directory| directory.strip_prefix(root).ok())
        .map(|relative| {
            relative
                .components()
                .map(|component| {
                    display_text(&component.as_os_str().to_string_lossy(), max_chars)
                        .unwrap_or_else(|| ELIDED_PATH_COMPONENT.into())
                })
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_default();
    FileOrigin {
        file_name,
        folder_path,
    }
}

/// The granted folder's own name, for the heading above the question.
pub fn folder_label(path: &Path, fallback: &str, max_chars: usize) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| display_text(name, max_chars))
        .unwrap_or_else(|| fallback.into())
}

/// Text out of a file, reduced to what may be shown.
///
/// Control characters refuse the whole string, as they always did. Characters
/// that occupy **no width** are removed instead: a zero-width space or a bidi
/// override does not make a name unreadable, it makes two different names look
/// identical — which is exactly how a planted file would dodge the duplicate check
/// while still reading as the game it is imitating.
pub fn display_text(value: &str, max_chars: usize) -> Option<String> {
    let visible = value
        .chars()
        .filter(|character| !is_invisible(*character))
        .collect::<String>();
    let trimmed = visible.trim();
    (!trimmed.is_empty() && !trimmed.chars().any(char::is_control))
        .then(|| trimmed.chars().take(max_chars).collect())
}

/// Characters that take up no width, so no reader can see them.
///
/// The whole Unicode `Cf` (format) category, plus the handful of code points that
/// render as nothing without being `Cf`: the width-zero fillers, the variation
/// selectors, and braille blank. Written out rather than looked up, because a
/// character-property table would be a new dependency for a list that moves once
/// a Unicode release and whose worst failure is a duplicate title reported where
/// none was meant.
///
/// The ranges are Unicode 15.1's; a code point assigned to `Cf` after that simply
/// stays visible to the comparison, which is the safe direction — it can make a
/// collision go unreported, never invent one.
fn is_invisible(character: char) -> bool {
    matches!(character,
        // Cf
        '\u{00AD}'                  // soft hyphen
        | '\u{0600}'..='\u{0605}'   // arabic number signs
        | '\u{061C}'                // arabic letter mark
        | '\u{06DD}'                // arabic end of ayah
        | '\u{070F}'                // syriac abbreviation mark
        | '\u{0890}'..='\u{0891}'   // arabic pound and piastre marks
        | '\u{08E2}'                // arabic disputed end of ayah
        | '\u{180E}'                // mongolian vowel separator
        | '\u{200B}'..='\u{200F}'   // zero-width space, joiners, bidi marks
        | '\u{202A}'..='\u{202E}'   // bidi embedding and override
        | '\u{2060}'..='\u{2064}'   // word joiner, invisible operators
        | '\u{2066}'..='\u{206F}'   // bidi isolates, deprecated formatting
        | '\u{FEFF}'                // zero-width no-break space
        | '\u{FFF9}'..='\u{FFFB}'   // interlinear annotation
        | '\u{110BD}'               // kaithi number sign
        | '\u{110CD}'               // kaithi number sign above
        | '\u{13430}'..='\u{1343F}' // egyptian hieroglyph format controls
        | '\u{1BCA0}'..='\u{1BCA3}' // shorthand format controls
        | '\u{1D173}'..='\u{1D17A}' // musical formatting
        | '\u{E0001}'               // language tag
        | '\u{E0020}'..='\u{E007F}' // tags
        // Not Cf, and still nothing a reader can see.
        | '\u{034F}'                // combining grapheme joiner
        | '\u{115F}'..='\u{1160}'   // hangul fillers
        | '\u{17B4}'..='\u{17B5}'   // khmer inherent vowels
        | '\u{180B}'..='\u{180D}'   // mongolian free variation selectors
        | '\u{180F}'                // mongolian free variation selector four
        | '\u{2800}'                // braille pattern blank
        | '\u{3164}'                // hangul filler
        | '\u{FE00}'..='\u{FE0F}'   // variation selectors
        | '\u{FFA0}'                // halfwidth hangul filler
        | '\u{E0100}'..='\u{E01EF}' // variation selectors supplement
    )
}

/// A path the host will hand an emulator, reduced to the one question every
/// caller has to ask about it.
///
/// Used by the console runner, and only by it: RetroArch is the loader that reads
/// an archive delimiter out of a content path. Winlator hands over a `.desktop`
/// file that Winlator itself parses, and nothing in that path is interpreted as a
/// container, so it does not call this. It lives here because it is a rule about
/// what a shared folder may put in front of a user, not because both flows run it.
pub fn is_plain_file_path(path: &Path) -> bool {
    path.to_str()
        .is_some_and(|path| !names_archive_member(path))
}

/// Does this path name a file *inside* an archive?
///
/// RetroArch reads `…/pack.zip#Game.nes` as "the entry `Game.nes` inside
/// `pack.zip`" — `path_get_archive_delim` in `libretro-common/file/file_path.c`
/// looks for the first `#` that directly follows `.zip`, `.apk` or `.7z`, case
/// insensitively. The suffix after it is what decides the extension, so such a
/// path passes every check Orivo makes about "is this a `.nes` file?" while the
/// bytes the host hashed are the decoy's, not the game's.
fn names_archive_member(path: &str) -> bool {
    let lowercase = path.to_ascii_lowercase();
    let mut search = lowercase.as_str();
    let mut consumed = 0;
    while let Some(index) = search.find('#') {
        let absolute = consumed + index;
        if [".zip", ".apk", ".7z"]
            .iter()
            .any(|extension| lowercase[..absolute].ends_with(extension))
        {
            return true;
        }
        consumed = absolute + 1;
        search = &lowercase[consumed..];
    }
    false
}

/// The extensions a loader would silently read *beside* a file it was given.
///
/// RetroArch truncates the content path at its last dot and looks for
/// `<that>.ips`, `.bps`, `.ups` and `.xdelta` — `runloop_path_fill_names` in
/// `runloop.c` — then applies whichever it finds unless `--no-patch` was passed,
/// which an intent cannot pass. A patch dropped next to a ROM therefore changes
/// what runs without changing the file Orivo hashed.
///
/// `.ips` additionally covers `ips1`…`ips9`: `patch_content` walks that series
/// off the same base name, so one `.ips` name is four files' worth of doorway.
pub const SOFT_PATCH_EXTENSIONS: [&str; 4] = ["ips", "bps", "ups", "xdelta"];

/// Would a loader read `sibling` as a patch for `file`?
///
/// Compared the way the filesystem resolves the names, not the way `==` does.
/// Shared storage on Android 11+ is case-insensitive, so `POKÉMON EMERALD.IPS`
/// *is* the file `Pokémon Emerald.ips` to everything that opens it — and an
/// ASCII-only fold would have let the accented half through. The same fold the
/// duplicate-title check uses is reused here, so the two cannot disagree about
/// what "the same name" means; what it does not fold is written down at
/// [`folded_title`].
pub fn is_soft_patch_for(file: &Path, sibling: &str) -> bool {
    let Some(stem) = file.file_stem().and_then(|stem| stem.to_str()) else {
        return false;
    };
    let folded_stem = folded_title(stem);
    let sibling = Path::new(sibling);
    let Some(sibling_stem) = sibling.file_stem().and_then(|stem| stem.to_str()) else {
        return false;
    };
    if folded_title(sibling_stem) != folded_stem {
        return false;
    }
    sibling
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            let extension = folded_title(extension);
            SOFT_PATCH_EXTENSIONS.iter().any(|known| {
                extension == *known
                    // `patch_content` reads `.ips1` … `.ips9` off the same base.
                    || (*known == "ips"
                        && extension.len() == 4
                        && extension.starts_with("ips")
                        && extension.ends_with(|last: char| last.is_ascii_digit()))
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: usize = 160;

    /// A zero-width space makes two names look identical and compare different,
    /// which is exactly what a planted file wants. It is removed, so the name is
    /// shown the way it reads and compared the way it reads.
    #[test]
    fn a_name_cannot_hide_behind_characters_nobody_can_see() {
        for hidden in [
            "Celeste\u{200B}",
            "\u{200E}Celeste",
            "Cel\u{FEFF}este",
            "Celeste\u{2069}",
            "Cele\u{00AD}ste",
        ] {
            assert_eq!(
                display_text(hidden, MAX).as_deref(),
                Some("Celeste"),
                "{hidden:?}"
            );
            assert_eq!(folded_title(hidden), folded_title("Celeste"), "{hidden:?}");
        }
    }

    /// Every `Cf` range plus the width-zero code points that are not `Cf`. The
    /// list is written out, so this is what holds it to what the doc comment
    /// claims — and to nothing more.
    #[test]
    fn every_width_zero_character_the_fold_claims_is_removed() {
        for hidden in [
            '\u{00AD}',
            '\u{0600}',
            '\u{0605}',
            '\u{061C}',
            '\u{06DD}',
            '\u{070F}',
            '\u{0890}',
            '\u{08E2}',
            '\u{180E}',
            '\u{200B}',
            '\u{200F}',
            '\u{202E}',
            '\u{2060}',
            '\u{2066}',
            '\u{206F}',
            '\u{FEFF}',
            '\u{FFF9}',
            '\u{FFFB}',
            '\u{110BD}',
            '\u{13430}',
            '\u{1343F}',
            '\u{1BCA0}',
            '\u{1BCA3}',
            '\u{1D173}',
            '\u{E0001}',
            '\u{E007F}',
            '\u{034F}',
            '\u{115F}',
            '\u{17B4}',
            '\u{180B}',
            '\u{2800}',
            '\u{3164}',
            '\u{FE0F}',
            '\u{FFA0}',
            '\u{E0100}',
        ] {
            let spoofed = format!("Cel{hidden}este");
            assert_eq!(
                display_text(&spoofed, MAX).as_deref(),
                Some("Celeste"),
                "{:04X} survived display",
                hidden as u32
            );
            assert_eq!(
                folded_title(&spoofed),
                folded_title("Celeste"),
                "{:04X} survived the fold",
                hidden as u32
            );
        }
    }

    /// What the fold does *not* do, held to as tightly as what it does: two names
    /// that differ only by Unicode normalisation compare different, and a
    /// homoglyph is simply another letter. Both are stated in the docs rather
    /// than implied away; this is what stops the claim from drifting.
    #[test]
    fn normalisation_and_homoglyphs_are_out_of_reach() {
        // "Pokémon" composed, and the same name decomposed.
        assert_ne!(folded_title("Pokémon"), folded_title("Poke\u{0301}mon"));
        // A Cyrillic `С` is not a Latin `C`, and no folding makes it one.
        assert_ne!(folded_title("\u{0421}eleste"), folded_title("Celeste"));
    }

    #[test]
    fn a_name_carrying_a_control_character_is_refused_whole() {
        assert_eq!(display_text("Qu\u{7}ake", MAX), None);
        assert_eq!(display_text("   ", MAX), None);
        assert_eq!(display_text("\u{200B}", MAX), None);
    }

    #[test]
    fn a_title_that_only_differs_by_invisible_or_spacing_is_a_duplicate() {
        let library = taken_titles(["Celeste"].into_iter());
        assert_eq!(
            title_collision("celeste", &library, std::iter::empty()),
            SourceTitleCollision::Library
        );
        assert_eq!(
            title_collision(" C e l e s t e ", &library, std::iter::empty()),
            SourceTitleCollision::Library
        );
        assert_eq!(
            title_collision("Braid", &library, std::iter::empty()),
            SourceTitleCollision::None
        );
    }

    #[test]
    fn two_files_claiming_one_name_are_both_flagged() {
        let found = ["Celeste", "Celeste", "Braid"];
        assert_eq!(
            title_collision("Celeste", &BTreeSet::new(), found.into_iter()),
            SourceTitleCollision::Folder
        );
        assert_eq!(
            title_collision("Braid", &BTreeSet::new(), found.into_iter()),
            SourceTitleCollision::None
        );
    }

    /// The library case is the louder one: a name the user already owns is worse
    /// news than a name shared inside the folder they just connected.
    #[test]
    fn a_name_that_is_taken_twice_reports_the_library() {
        let library = taken_titles(["Celeste"].into_iter());
        assert_eq!(
            title_collision("Celeste", &library, ["Celeste", "Celeste"].into_iter()),
            SourceTitleCollision::Library
        );
    }

    #[test]
    fn names_a_file_by_its_own_name_and_the_folders_under_the_grant() {
        let root = Path::new("/storage/emulated/0/Download/Roms");
        assert_eq!(
            file_origin(root, &root.join("Alter Ego.nes"), MAX),
            FileOrigin {
                file_name: "Alter Ego.nes".into(),
                folder_path: String::new(),
            }
        );
        assert_eq!(
            file_origin(root, &root.join("new/Free Coins.nes"), MAX),
            FileOrigin {
                file_name: "Free Coins.nes".into(),
                folder_path: "new".into(),
            }
        );
    }

    /// RetroArch reads `pack.zip#Game.nes` as an entry inside `pack.zip`, so such
    /// a path ends in `.nes`, passes every check about what kind of file it is,
    /// and makes the host hash the decoy while the core loads the archive.
    #[test]
    fn refuses_a_path_that_names_a_file_inside_an_archive() {
        for member in [
            "/roms/pack.zip#Alter Ego.nes",
            "/roms/pack.ZIP#Alter Ego.nes",
            "/roms/pack.7z#Alter Ego.nes",
            "/roms/pack.apk#Alter Ego.nes",
            "/roms/pack.zip#sub/Alter Ego.nes",
            // The first `#` is not after an archive extension; the second is.
            "/roms/Game #1.zip#Alter Ego.nes",
        ] {
            assert!(!is_plain_file_path(Path::new(member)), "accepted {member}");
        }
    }

    /// A `#` that follows no archive extension is just a character in a file
    /// name, and refusing those would cost a user their library for nothing.
    #[test]
    fn accepts_a_hash_that_is_only_part_of_a_name() {
        for ordinary in [
            "/roms/Game #1.nes",
            "/roms/#1.nes",
            "/roms/pack.zipper#Alter Ego.nes",
        ] {
            assert!(
                is_plain_file_path(Path::new(ordinary)),
                "refused {ordinary}"
            );
        }
    }

    /// Shared storage on Android 11+ is case-insensitive, so `POKÉMON EMERALD.IPS`
    /// *is* `Pokémon Emerald.ips` to everything that opens it. The ASCII-only
    /// comparison this replaced let the accented half of that through.
    #[test]
    fn a_patch_is_recognised_however_its_name_is_cased() {
        let rom = Path::new("/roms/Pokémon Emerald.gba");
        for sibling in [
            "Pokémon Emerald.ips",
            "POKÉMON EMERALD.IPS",
            "pokémon emerald.Ips",
            "Pokémon Emerald.bps",
            "Pokémon Emerald.UPS",
            "Pokémon Emerald.xdelta",
            // `patch_content` walks `.ips1` … `.ips9` off the same base name.
            "Pokémon Emerald.ips1",
            "Pokémon Emerald.IPS9",
        ] {
            assert!(is_soft_patch_for(rom, sibling), "missed {sibling}");
        }
    }

    #[test]
    fn a_file_that_is_not_a_patch_for_this_rom_is_left_alone() {
        let rom = Path::new("/roms/Pokémon Emerald.gba");
        for sibling in [
            "Pokémon Emerald.gba",
            "Pokémon Ruby.ips",
            "Pokémon Emerald.ips10",
            "Pokémon Emerald.ipsx",
            "Pokémon Emerald.txt",
            "Pokémon Emerald",
        ] {
            assert!(!is_soft_patch_for(rom, sibling), "flagged {sibling}");
        }
    }
}
