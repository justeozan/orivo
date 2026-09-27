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
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

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
/// The Unicode `Cf` category plus the two zero-width joiners, written out rather
/// than looked up: a table lookup would be a new dependency for a list that has
/// not moved in a decade, and the cost of missing one is only that a duplicate
/// title is reported where none was meant.
fn is_invisible(character: char) -> bool {
    matches!(character,
        '\u{00AD}'                  // soft hyphen
        | '\u{034F}'                // combining grapheme joiner
        | '\u{061C}'                // arabic letter mark
        | '\u{115F}'..='\u{1160}'   // hangul fillers
        | '\u{17B4}'..='\u{17B5}'   // khmer inherent vowels
        | '\u{180B}'..='\u{180F}'   // mongolian selectors and vowel separator
        | '\u{200B}'..='\u{200F}'   // zero-width space, joiners, bidi marks
        | '\u{202A}'..='\u{202E}'   // bidi embedding and override
        | '\u{2060}'..='\u{2064}'   // word joiner, invisible operators
        | '\u{2066}'..='\u{206F}'   // bidi isolates, deprecated formatting
        | '\u{3164}'                // hangul filler
        | '\u{FE00}'..='\u{FE0F}'   // variation selectors
        | '\u{FEFF}'                // zero-width no-break space
        | '\u{FFA0}'                // halfwidth hangul filler
        | '\u{1D173}'..='\u{1D17A}' // musical formatting
        | '\u{E0000}'..='\u{E007F}' // tags
        | '\u{E0100}'..='\u{E01EF}' // variation selectors supplement
    )
}

/// A path the host will hand an emulator, reduced to the one question every
/// caller has to ask about it.
///
/// Kept here because both runners send a *path* somewhere: Winlator to a Wine
/// container, RetroArch to a libretro core. Neither may send one that names
/// something inside an archive.
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

/// The pathnames a loader would silently read *beside* a file it was given.
///
/// RetroArch truncates the content path at its last dot and looks for
/// `<that>.ips`, `.bps`, `.ups` and `.xdelta` — `runloop_path_fill_names` in
/// `runloop.c` — then applies whichever it finds unless `--no-patch` was passed,
/// which an intent cannot pass. A patch dropped next to a ROM therefore changes
/// what runs without changing the file Orivo hashed.
pub const SOFT_PATCH_EXTENSIONS: [&str; 4] = ["ips", "bps", "ups", "xdelta"];

/// The four pathnames a loader would read beside this one.
pub fn soft_patch_siblings(file: &Path) -> Vec<PathBuf> {
    let Some(stem) = file.file_stem().and_then(|stem| stem.to_str()) else {
        return Vec::new();
    };
    let Some(directory) = file.parent() else {
        return Vec::new();
    };
    SOFT_PATCH_EXTENSIONS
        .iter()
        .map(|extension| directory.join(format!("{stem}.{extension}")))
        .collect()
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

    #[test]
    fn names_the_four_files_a_loader_would_read_beside_a_rom() {
        let siblings = soft_patch_siblings(Path::new("/roms/Alter Ego.nes"));
        assert_eq!(
            siblings,
            [
                PathBuf::from("/roms/Alter Ego.ips"),
                PathBuf::from("/roms/Alter Ego.bps"),
                PathBuf::from("/roms/Alter Ego.ups"),
                PathBuf::from("/roms/Alter Ego.xdelta"),
            ]
        );
    }
}
