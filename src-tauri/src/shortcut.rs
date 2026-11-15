//! Shortcuts a player keeps instead of the games themselves: Windows shell
//! links (`.lnk`), internet shortcuts (`.url`, the files Steam and Epic put on
//! the desktop) and macOS Finder aliases.
//!
//! Everything here reads bytes. Nothing starts a process: a `.lnk` gives back
//! the program, arguments and folder it names, a `.url` gives back its address
//! only when the address belongs to a game launcher, and an alias gives back
//! the file it points to.

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

/// A shortcut is a few kilobytes. Anything larger is not one, and is not read.
const MAX_SHORTCUT_BYTES: u64 = 256 * 1024;
/// Longest address a `.url` may carry and still be handed to a launcher.
const MAX_URL_LENGTH: usize = 2048;

/// URI schemes of game launchers. A `.url` pointing anywhere else (a web page,
/// a file, a script host) is not a game, and is never opened as one.
const LAUNCHER_SCHEMES: &[&str] = &[
    "steam",
    "com.epicgames.launcher",
    "goggalaxy",
    "uplay",
    "origin",
    "origin2",
    "battlenet",
    "heroic",
    "itch",
    "amazon-games",
    "playnite",
];

fn read_bounded(path: &Path) -> Option<Vec<u8>> {
    let file = fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > MAX_SHORTCUT_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_SHORTCUT_BYTES).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

// ---------------------------------------------------------------------------
// Internet shortcuts (.url)
// ---------------------------------------------------------------------------

/// The launcher address a `.url` file holds, or `None` when the file is not an
/// internet shortcut or points at something other than a game launcher.
pub fn launcher_url(path: &Path) -> Option<String> {
    let bytes = read_bounded(path)?;
    let text = String::from_utf8_lossy(&bytes);
    parse_internet_shortcut(&text).filter(|url| is_launcher_url(url))
}

/// `URL=` in the `[InternetShortcut]` section.
fn parse_internet_shortcut(text: &str) -> Option<String> {
    let mut in_section = false;
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.starts_with('[') {
            in_section = line.eq_ignore_ascii_case("[InternetShortcut]");
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some((key, value)) = line.split_once('=')
            && key.trim().eq_ignore_ascii_case("URL")
        {
            let value = value.trim();
            return (!value.is_empty()).then(|| value.to_string());
        }
    }
    None
}

/// Whether `url` is a well-formed address for one of the known game launchers.
/// Whitespace, quotes and control characters are refused outright: the address
/// is handed to the operating system's opener, and none of them belongs in a
/// launcher URI.
pub fn is_launcher_url(url: &str) -> bool {
    if url.len() > MAX_URL_LENGTH
        || url.chars().any(|character| {
            character.is_whitespace() || character.is_control() || character == '"'
        })
    {
        return false;
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    !rest.is_empty()
        && LAUNCHER_SCHEMES
            .iter()
            .any(|known| scheme.eq_ignore_ascii_case(known))
}

/// A launcher address that names a game a connected store already lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreLink {
    Steam(u32),
    Epic(String),
}

/// The store game a launcher address starts, when it is one Orivo can match
/// against its own Steam and Epic records.
pub fn store_link(url: &str) -> Option<StoreLink> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme.eq_ignore_ascii_case("steam") {
        let rest = rest.split(['?', '#']).next().unwrap_or_default();
        let (verb, id) = rest.split_once('/')?;
        if !matches!(
            verb.to_ascii_lowercase().as_str(),
            "rungameid" | "run" | "launch"
        ) {
            return None;
        }
        let id = id.trim_end_matches('/');
        // A non-Steam game added to Steam gets a 64-bit id; only a real app id
        // fits in 32 bits.
        let app_id = id.parse::<u64>().ok()?;
        return u32::try_from(app_id)
            .ok()
            .filter(|app_id| *app_id > 0)
            .map(StoreLink::Steam);
    }
    if scheme.eq_ignore_ascii_case("com.epicgames.launcher") {
        let reference = rest.strip_prefix("apps/")?;
        let reference = reference.split(['?', '#']).next().unwrap_or_default();
        let decoded = percent_decode(reference)?;
        // `namespace:catalogItemId:appName` in recent shortcuts, the app name
        // alone in older ones. The catalog keys an Epic game by its app name.
        let app_name = decoded.rsplit(':').next().unwrap_or_default();
        return crate::catalog::is_valid_provider_reference(app_name)
            .then(|| StoreLink::Epic(app_name.to_string()));
    }
    None
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = value.get(index + 1..index + 3)?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

// ---------------------------------------------------------------------------
// Shell links (.lnk)
// ---------------------------------------------------------------------------

/// What a `.lnk` file says, once its target has been looked for on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellLink {
    /// The program the link starts, when it names a path that exists here.
    pub target: Option<PathBuf>,
    /// Whether the link names a path at all. A link to a Store app or another
    /// shell object names none; a link whose program was deleted names one
    /// that is no longer there.
    pub names_a_path: bool,
    pub arguments: Vec<String>,
    pub working_directory: Option<PathBuf>,
    /// The link asks Windows to start its program elevated. Only the shell can
    /// do that, so such a link is opened, never run directly.
    pub run_as_administrator: bool,
}

pub fn read_shell_link(path: &Path) -> Option<ShellLink> {
    let raw = parse_shell_link(&read_bounded(path)?)?;
    let link_directory = path.parent().unwrap_or(Path::new(""));
    let candidates = raw
        .local_path
        .iter()
        .chain(raw.environment_target.iter())
        .map(|candidate| PathBuf::from(expand_environment(candidate)))
        .chain(
            raw.relative_path
                .iter()
                .map(|relative| link_directory.join(expand_environment(relative))),
        )
        .collect::<Vec<_>>();
    let target = candidates
        .iter()
        .find(|candidate| candidate.exists())
        .cloned();
    let working_directory = raw
        .working_directory
        .as_deref()
        .map(expand_environment)
        .map(PathBuf::from)
        .filter(|directory| directory.is_dir());
    Some(ShellLink {
        target,
        names_a_path: !candidates.is_empty(),
        arguments: raw
            .arguments
            .as_deref()
            .map(split_command_line)
            .unwrap_or_default(),
        working_directory,
        run_as_administrator: raw.run_as_administrator,
    })
}

/// The fields of a `.lnk` file, before anything is looked up on disk.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct RawShellLink {
    local_path: Option<String>,
    environment_target: Option<String>,
    relative_path: Option<String>,
    working_directory: Option<String>,
    arguments: Option<String>,
    run_as_administrator: bool,
}

const LINK_HEADER_SIZE: usize = 0x4C;
const LINK_CLSID: [u8; 16] = [
    0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
];
const HAS_LINK_TARGET_ID_LIST: u32 = 0x0000_0001;
const HAS_LINK_INFO: u32 = 0x0000_0002;
const HAS_NAME: u32 = 0x0000_0004;
const HAS_RELATIVE_PATH: u32 = 0x0000_0008;
const HAS_WORKING_DIR: u32 = 0x0000_0010;
const HAS_ARGUMENTS: u32 = 0x0000_0020;
const HAS_ICON_LOCATION: u32 = 0x0000_0040;
const IS_UNICODE: u32 = 0x0000_0080;
const RUN_AS_USER: u32 = 0x0000_2000;
const VOLUME_ID_AND_LOCAL_BASE_PATH: u32 = 0x1;
const COMMON_NETWORK_RELATIVE_LINK: u32 = 0x2;
const ENVIRONMENT_VARIABLE_BLOCK: u32 = 0xA000_0001;

fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// A NUL-terminated string in the system code page. Without the code page,
/// each byte is read as Latin-1: exact for ASCII paths, and a link that carries
/// anything else nearly always carries the Unicode copy too.
fn ansi_string_at(bytes: &[u8], offset: usize) -> Option<String> {
    let tail = bytes.get(offset..)?;
    let end = tail.iter().position(|byte| *byte == 0)?;
    Some(tail[..end].iter().map(|byte| char::from(*byte)).collect())
}

fn unicode_string_at(bytes: &[u8], offset: usize) -> Option<String> {
    let tail = bytes.get(offset..)?;
    let units = tail
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|unit| *unit != 0)
        .collect::<Vec<_>>();
    // A string that runs off the end of its structure has no terminator.
    if units.len() * 2 + 2 > tail.len() {
        return None;
    }
    Some(String::from_utf16_lossy(&units))
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

/// Reads the parts of the [MS-SHLLINK] format that say what a link starts:
/// the header, the LinkInfo paths, the StringData fields and the environment
/// variable block. Every offset is bounds-checked; malformed input gives
/// `None` or an empty field, never a panic.
fn parse_shell_link(bytes: &[u8]) -> Option<RawShellLink> {
    if u32_at(bytes, 0)? as usize != LINK_HEADER_SIZE || bytes.get(4..20)? != LINK_CLSID {
        return None;
    }
    let flags = u32_at(bytes, 20)?;
    let mut link = RawShellLink {
        run_as_administrator: flags & RUN_AS_USER != 0,
        ..RawShellLink::default()
    };
    let mut cursor = LINK_HEADER_SIZE;

    if flags & HAS_LINK_TARGET_ID_LIST != 0 {
        let size = u16_at(bytes, cursor)? as usize;
        cursor = cursor.checked_add(2 + size)?;
    }

    if flags & HAS_LINK_INFO != 0 {
        let size = u32_at(bytes, cursor)? as usize;
        let info = bytes.get(cursor..cursor.checked_add(size)?)?;
        link.local_path = non_empty(link_info_path(info));
        cursor += size;
    }

    let unicode = flags & IS_UNICODE != 0;
    let mut string_data = |present: bool| -> Option<Option<String>> {
        if !present {
            return Some(None);
        }
        let count = u16_at(bytes, cursor)? as usize;
        let length = if unicode { count * 2 } else { count };
        let raw = bytes.get(cursor + 2..cursor + 2 + length)?;
        cursor += 2 + length;
        Some(Some(if unicode {
            let units = raw
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>();
            String::from_utf16_lossy(&units)
        } else {
            raw.iter().map(|byte| char::from(*byte)).collect()
        }))
    };
    let _name = string_data(flags & HAS_NAME != 0)?;
    link.relative_path = non_empty(string_data(flags & HAS_RELATIVE_PATH != 0)?);
    link.working_directory = non_empty(string_data(flags & HAS_WORKING_DIR != 0)?);
    link.arguments = non_empty(string_data(flags & HAS_ARGUMENTS != 0)?);
    let _icon = string_data(flags & HAS_ICON_LOCATION != 0)?;

    // ExtraData: a run of sized blocks ended by one smaller than four bytes.
    while let Some(size) = u32_at(bytes, cursor) {
        let size = size as usize;
        if size < 8 {
            break;
        }
        let Some(block) = bytes.get(cursor..cursor + size) else {
            break;
        };
        if u32_at(block, 4) == Some(ENVIRONMENT_VARIABLE_BLOCK) {
            // TargetAnsi is 260 bytes at +8, TargetUnicode 520 bytes at +268.
            link.environment_target = non_empty(
                block
                    .get(268..788)
                    .and_then(|unicode| unicode_string_at(unicode, 0))
                    .or_else(|| block.get(8..268).and_then(|ansi| ansi_string_at(ansi, 0))),
            );
        }
        cursor += size;
    }
    Some(link)
}

/// The full path a LinkInfo structure records: a local base path, or a network
/// share name, followed by the common path suffix.
fn link_info_path(info: &[u8]) -> Option<String> {
    let header_size = u32_at(info, 4)? as usize;
    let flags = u32_at(info, 8)?;
    let unicode_offsets = header_size >= 0x24;
    let suffix = if unicode_offsets && u32_at(info, 32)? != 0 {
        unicode_string_at(info, u32_at(info, 32)? as usize)
    } else {
        ansi_string_at(info, u32_at(info, 24)? as usize)
    }
    .unwrap_or_default();

    if flags & VOLUME_ID_AND_LOCAL_BASE_PATH != 0 {
        let base = if unicode_offsets && u32_at(info, 28)? != 0 {
            unicode_string_at(info, u32_at(info, 28)? as usize)
        } else {
            ansi_string_at(info, u32_at(info, 16)? as usize)
        }?;
        return Some(join_windows_path(&base, &suffix));
    }
    if flags & COMMON_NETWORK_RELATIVE_LINK != 0 {
        let network_offset = u32_at(info, 20)? as usize;
        let network = info.get(network_offset..)?;
        let name_offset = u32_at(network, 8)? as usize;
        let share = if name_offset > 0x14 && u32_at(network, 20).is_some_and(|offset| offset != 0) {
            unicode_string_at(network, u32_at(network, 20)? as usize)
        } else {
            ansi_string_at(network, name_offset)
        }?;
        return Some(join_windows_path(&share, &suffix));
    }
    None
}

fn join_windows_path(base: &str, suffix: &str) -> String {
    if suffix.is_empty() {
        base.to_string()
    } else if base.ends_with('\\') {
        format!("{base}{suffix}")
    } else {
        format!("{base}\\{suffix}")
    }
}

/// `%NAME%` replaced by the variable's value, the way the shell expands a
/// link's paths. A variable that is not set is left as written.
fn expand_environment(value: &str) -> String {
    let mut expanded = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find('%') {
        expanded.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => {
                let name = &after[..end];
                match std::env::var(name) {
                    Ok(value) => expanded.push_str(&value),
                    Err(_) => {
                        expanded.push('%');
                        expanded.push_str(name);
                        expanded.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            _ => {
                expanded.push('%');
                rest = after;
            }
        }
    }
    expanded.push_str(rest);
    expanded
}

/// Splits a Windows command line the way the C runtime does
/// (`CommandLineToArgvW`), so each argument reaches the program as it would
/// from the shortcut itself: whitespace separates arguments outside quotes,
/// `2n` backslashes before a quote are `n` backslashes, `2n+1` are `n` and a
/// literal quote, and `""` inside quotes is a literal quote.
pub fn split_command_line(line: &str) -> Vec<String> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut in_argument = false;
    let mut quoted = false;
    let mut characters = line.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\\' => {
                let mut backslashes = 1;
                while characters.peek() == Some(&'\\') {
                    characters.next();
                    backslashes += 1;
                }
                in_argument = true;
                if characters.peek() == Some(&'"') {
                    current.extend(std::iter::repeat_n('\\', backslashes / 2));
                    if backslashes % 2 == 1 {
                        characters.next();
                        current.push('"');
                    }
                } else {
                    current.extend(std::iter::repeat_n('\\', backslashes));
                }
            }
            '"' => {
                in_argument = true;
                if quoted && characters.peek() == Some(&'"') {
                    characters.next();
                    current.push('"');
                } else {
                    quoted = !quoted;
                }
            }
            ' ' | '\t' if !quoted => {
                if in_argument {
                    arguments.push(std::mem::take(&mut current));
                    in_argument = false;
                }
            }
            _ => {
                in_argument = true;
                current.push(character);
            }
        }
    }
    if in_argument {
        arguments.push(current);
    }
    arguments
}

// ---------------------------------------------------------------------------
// Finder aliases (macOS)
// ---------------------------------------------------------------------------

/// The first bytes of a Finder alias file: bookmark data.
const ALIAS_MAGIC: &[u8; 16] = b"book\0\0\0\0mark\0\0\0\0";

/// Whether `path` is a Finder alias. An alias is an ordinary file, usually
/// without an extension, whose content is bookmark data.
pub fn is_finder_alias(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut magic = [0_u8; 16];
    file.read_exact(&mut magic).is_ok() && &magic == ALIAS_MAGIC
}

/// The file or folder a Finder alias points to, resolved without showing any
/// interface and without mounting a volume.
#[cfg(target_os = "macos")]
pub fn resolve_finder_alias(path: &Path) -> Option<PathBuf> {
    use objc2_foundation::{NSString, NSURL, NSURLBookmarkResolutionOptions};

    let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str()?));
    let resolved = NSURL::URLByResolvingAliasFileAtURL_options_error(
        &url,
        NSURLBookmarkResolutionOptions::WithoutUI | NSURLBookmarkResolutionOptions::WithoutMounting,
    )
    .ok()?;
    let target = PathBuf::from(resolved.path()?.to_string());
    (target != path && target.exists()).then_some(target)
}

#[cfg(not(target_os = "macos"))]
pub fn resolve_finder_alias(_path: &Path) -> Option<PathBuf> {
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal, well-formed `.lnk`: header, LinkInfo with a local base path,
    /// then the StringData fields the flags announce, in Unicode.
    pub(crate) fn shell_link_bytes(
        target: &str,
        working_directory: Option<&str>,
        arguments: Option<&str>,
        run_as_administrator: bool,
    ) -> Vec<u8> {
        let mut flags = HAS_LINK_INFO | IS_UNICODE;
        if working_directory.is_some() {
            flags |= HAS_WORKING_DIR;
        }
        if arguments.is_some() {
            flags |= HAS_ARGUMENTS;
        }
        if run_as_administrator {
            flags |= RUN_AS_USER;
        }
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(LINK_HEADER_SIZE as u32).to_le_bytes());
        bytes.extend_from_slice(&LINK_CLSID);
        bytes.extend_from_slice(&flags.to_le_bytes());
        bytes.resize(LINK_HEADER_SIZE, 0);

        // LinkInfo with a 0x1C header: ANSI base path then an empty suffix.
        let base = target.as_bytes();
        let header_size = 0x1C_u32;
        let base_offset = header_size;
        let suffix_offset = base_offset + base.len() as u32 + 1;
        let size = suffix_offset + 1;
        let mut info = Vec::new();
        info.extend_from_slice(&size.to_le_bytes());
        info.extend_from_slice(&header_size.to_le_bytes());
        info.extend_from_slice(&VOLUME_ID_AND_LOCAL_BASE_PATH.to_le_bytes());
        info.extend_from_slice(&0_u32.to_le_bytes()); // VolumeID, unused here
        info.extend_from_slice(&base_offset.to_le_bytes());
        info.extend_from_slice(&0_u32.to_le_bytes()); // no network link
        info.extend_from_slice(&suffix_offset.to_le_bytes());
        info.extend_from_slice(base);
        info.push(0);
        info.push(0);
        bytes.extend_from_slice(&info);

        for value in [working_directory, arguments].into_iter().flatten() {
            let units = value.encode_utf16().collect::<Vec<_>>();
            bytes.extend_from_slice(&(units.len() as u16).to_le_bytes());
            for unit in units {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
        }
        bytes.extend_from_slice(&0_u32.to_le_bytes()); // terminal block
        bytes
    }

    #[test]
    fn a_shell_link_gives_its_target_folder_and_arguments() {
        let bytes = shell_link_bytes(
            r"C:\Games\Hades\Hades.exe",
            Some(r"C:\Games\Hades"),
            Some(r#"-windowed "--profile=Player One""#),
            false,
        );
        let link = parse_shell_link(&bytes).unwrap();
        assert_eq!(
            link.local_path.as_deref(),
            Some(r"C:\Games\Hades\Hades.exe")
        );
        assert_eq!(link.working_directory.as_deref(), Some(r"C:\Games\Hades"));
        assert_eq!(
            split_command_line(link.arguments.as_deref().unwrap()),
            vec!["-windowed", "--profile=Player One"]
        );
        assert!(!link.run_as_administrator);
    }

    #[test]
    fn an_elevated_link_says_so() {
        let bytes = shell_link_bytes(r"C:\Game.exe", None, None, true);
        assert!(parse_shell_link(&bytes).unwrap().run_as_administrator);
    }

    #[test]
    fn truncated_or_foreign_bytes_are_not_a_link() {
        let bytes = shell_link_bytes(r"C:\Games\Hades\Hades.exe", Some(r"C:\Games"), None, false);
        for length in 0..bytes.len() - 4 {
            // Must never panic; a cut inside a field is simply not a link.
            let _ = parse_shell_link(&bytes[..length]);
        }
        assert!(parse_shell_link(b"[InternetShortcut]\nURL=steam://run/1\n").is_none());
        assert!(parse_shell_link(&[]).is_none());
    }

    #[test]
    fn a_shell_link_to_a_program_on_disk_is_resolved() {
        let directory = std::env::temp_dir().join(format!(
            "orivo-shortcut-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos())
        ));
        fs::create_dir_all(&directory).unwrap();
        let program = directory.join("Game.exe");
        fs::write(&program, b"MZ").unwrap();
        let link = directory.join("Game.lnk");
        fs::write(
            &link,
            shell_link_bytes(
                program.to_str().unwrap(),
                Some(directory.to_str().unwrap()),
                Some("-a b"),
                false,
            ),
        )
        .unwrap();
        let resolved = read_shell_link(&link).unwrap();
        assert_eq!(resolved.target, Some(program));
        assert_eq!(resolved.working_directory, Some(directory.clone()));
        assert_eq!(resolved.arguments, vec!["-a", "b"]);
        assert!(resolved.names_a_path);

        let missing = directory.join("Missing.lnk");
        fs::write(
            &missing,
            shell_link_bytes(r"Z:\Nowhere\Gone.exe", None, None, false),
        )
        .unwrap();
        let resolved = read_shell_link(&missing).unwrap();
        assert_eq!(resolved.target, None);
        assert!(resolved.names_a_path);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn command_lines_split_like_the_c_runtime() {
        assert_eq!(split_command_line(""), Vec::<String>::new());
        assert_eq!(split_command_line("  a  b\tc "), vec!["a", "b", "c"]);
        assert_eq!(split_command_line(r#""a b" c"#), vec!["a b", "c"]);
        assert_eq!(split_command_line(r#"a\\\"b"#), vec![r#"a\"b"#]);
        assert_eq!(split_command_line(r#""a\\" b"#), vec![r"a\", "b"]);
        assert_eq!(
            split_command_line(r"C:\Path\To\ x"),
            vec![r"C:\Path\To\", "x"]
        );
        assert_eq!(split_command_line(r#""say ""hi""""#), vec![r#"say "hi""#]);
        assert_eq!(split_command_line(r#""""#), vec![""]);
    }

    #[test]
    fn an_internet_shortcut_gives_its_launcher_address_only() {
        let steam =
            "[InternetShortcut]\r\nIDList=\r\nURL=steam://rungameid/1145360\r\nIconIndex=0\r\n";
        assert_eq!(
            parse_internet_shortcut(steam).as_deref(),
            Some("steam://rungameid/1145360")
        );
        assert!(is_launcher_url("steam://rungameid/1145360"));
        assert!(is_launcher_url(
            "com.epicgames.launcher://apps/fn%3A4fe75bbc5a674f4f9b356b5c90567da5%3AFortnite?action=launch&silent=true"
        ));
        for refused in [
            "https://example.com/game",
            "file:///C:/Windows/System32/cmd.exe",
            "steam://",
            "steam://run/1 --evil",
            "javascript://alert(1)",
        ] {
            assert!(!is_launcher_url(refused), "{refused}");
        }
        assert_eq!(
            parse_internet_shortcut("[Other]\nURL=steam://run/1\n"),
            None
        );
    }

    #[test]
    fn steam_and_epic_addresses_name_their_store_game() {
        assert_eq!(
            store_link("steam://rungameid/1145360"),
            Some(StoreLink::Steam(1145360))
        );
        assert_eq!(store_link("steam://run/570/"), Some(StoreLink::Steam(570)));
        // A non-Steam game added to Steam: launchable, but no store record.
        assert_eq!(store_link("steam://rungameid/12345678901234567890"), None);
        assert_eq!(
            store_link(
                "com.epicgames.launcher://apps/fn%3A4fe75bbc5a674f4f9b356b5c90567da5%3AFortnite?action=launch&silent=true"
            ),
            Some(StoreLink::Epic("Fortnite".into()))
        );
        assert_eq!(
            store_link("com.epicgames.launcher://apps/Sugar?action=launch"),
            Some(StoreLink::Epic("Sugar".into()))
        );
        assert_eq!(store_link("goggalaxy://openGameView/1207658924"), None);
    }

    #[test]
    fn environment_variables_expand_and_unknown_ones_stay() {
        // SAFETY: test-local variable name, not read by any other test.
        unsafe { std::env::set_var("ORIVO_SHORTCUT_TEST_ROOT", r"D:\Games") };
        assert_eq!(
            expand_environment(r"%ORIVO_SHORTCUT_TEST_ROOT%\Hades\Hades.exe"),
            r"D:\Games\Hades\Hades.exe"
        );
        assert_eq!(
            expand_environment("%ORIVO_UNSET_VARIABLE%\\x"),
            "%ORIVO_UNSET_VARIABLE%\\x"
        );
        assert_eq!(expand_environment("100% sure"), "100% sure");
    }
}
