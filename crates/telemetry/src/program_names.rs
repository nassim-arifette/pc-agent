//! Readable program names ("Google Chrome" for `chrome.exe`), as Task Manager shows them: the
//! `FileDescription`, else the `ProductName`, of the executable's version resource. They are for
//! the local window only: never part of a snapshot, a model payload or a preview (contract C5,
//! note of 26 October 2026), so nothing in this module is serialized.
//!
//! A name is read once per executable and cached by full path; paths stay in this process's
//! bounded memory and are never returned. Reading needs no administrator rights: a process that
//! refuses a query-only handle (protected processes, some services) keeps its executable name.
#![cfg_attr(not(windows), allow(dead_code))]
use std::{
    collections::{HashMap, VecDeque},
    hash::Hash,
    sync::{Arc, Mutex, MutexGuard},
};

/// Longest readable name kept, like the executable names sent to the model.
const MAX_PROGRAM_NAME_CHARS: usize = 64;
/// Executables whose name was read, or found missing.
const MAX_PATHS: usize = 512;
/// Process instances already resolved, so a refresh does not open them again.
const MAX_INSTANCES: usize = 2048;
/// String tables tried after the file's first declared translation: U.S. English in Unicode,
/// U.S. English in Windows-1252, then language-neutral Unicode.
const FALLBACK_TABLES: [(u16, u16); 3] = [(0x0409, 0x04B0), (0x0409, 0x04E4), (0x0000, 0x04B0)];
/// The fields read, in order.
const FIELDS: [&str; 2] = ["FileDescription", "ProductName"];

/// Bidirectional controls (marks, embeddings, overrides and isolates) and the byte-order mark:
/// they could reorder or hide what the window shows around a name.
fn is_bidi_or_invisible(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}

/// A name as the window shows it: control, bidirectional-formatting and byte-order characters
/// removed, each run of whitespace turned into one space, trimmed and cut to 64 characters;
/// `None` when nothing is left.
fn sanitize_program_name(raw: &str) -> Option<String> {
    let mut out = String::new();
    let mut count = 0;
    let mut pending_space = false;
    for c in raw.chars() {
        if c.is_whitespace() {
            pending_space = count > 0;
            continue;
        }
        if c.is_control() || is_bidi_or_invisible(c) {
            continue;
        }
        if pending_space {
            // A space is kept only with a character after it.
            if count + 2 > MAX_PROGRAM_NAME_CHARS {
                break;
            }
            out.push(' ');
            count += 1;
            pending_space = false;
        }
        if count == MAX_PROGRAM_NAME_CHARS {
            break;
        }
        out.push(c);
        count += 1;
    }
    (count > 0).then_some(out)
}

/// FileDescription, else ProductName, from the first string table that has either: the file's
/// first declared translation, then the fallbacks. `lookup` answers a `VerQueryValueW` sub-block
/// such as `\StringFileInfo\040904b0\FileDescription`.
pub(crate) fn name_from_tables(
    translations: &[(u16, u16)],
    mut lookup: impl FnMut(&str) -> Option<String>,
) -> Option<String> {
    let mut tables: Vec<(u16, u16)> = translations.iter().take(1).copied().collect();
    for fallback in FALLBACK_TABLES {
        if !tables.contains(&fallback) {
            tables.push(fallback);
        }
    }
    tables.into_iter().find_map(|(language, code_page)| {
        FIELDS.into_iter().find_map(|field| {
            lookup(&format!(
                "\\StringFileInfo\\{language:04x}{code_page:04x}\\{field}"
            ))
            .as_deref()
            .and_then(sanitize_program_name)
        })
    })
}

/// `\VarFileInfo\Translation`: language and code page pairs, two little-endian words each.
pub(crate) fn translations(bytes: &[u8]) -> Vec<(u16, u16)> {
    bytes
        .chunks_exact(4)
        .map(|c| {
            (
                u16::from_le_bytes([c[0], c[1]]),
                u16::from_le_bytes([c[2], c[3]]),
            )
        })
        .collect()
}

/// A version string: little-endian UTF-16 up to its terminating NUL.
pub(crate) fn utf16_text(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&unit| unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// The drive letter of an absolute path on a lettered drive (`C:\…`). Nothing else is read: the
/// version API would search the DLL search path for a bare name, and UNC or device paths can
/// reach the network.
pub(crate) fn drive_letter(path: &str) -> Option<char> {
    let mut chars = path.chars();
    let letter = chars.next().filter(char::is_ascii_alphabetic)?;
    (chars.next() == Some(':') && chars.next() == Some('\\')).then_some(letter)
}

/// A map that forgets its oldest entry once full.
#[derive(Debug)]
pub(crate) struct Bounded<K, V> {
    capacity: usize,
    entries: HashMap<K, V>,
    order: VecDeque<K>,
}
impl<K: Eq + Hash + Clone, V> Bounded<K, V> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }
    pub(crate) fn get(&self, key: &K) -> Option<&V> {
        self.entries.get(key)
    }
    pub(crate) fn insert(&mut self, key: K, value: V) {
        if let Some(slot) = self.entries.get_mut(&key) {
            *slot = value;
            return;
        }
        if self.entries.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
        self.order.push_back(key.clone());
        self.entries.insert(key, value);
    }
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

type ImagePathReader = dyn Fn(u32, u64) -> Option<String> + Send + Sync;
type VersionNameReader = dyn Fn(&str) -> Option<String> + Send + Sync;

struct Caches {
    /// Readable name by process instance (PID plus creation time); `None` when it is unknown.
    instances: Bounded<(u32, u64), Option<Arc<str>>>,
    /// Readable name by lower-case executable path; `None` when it has no usable resource.
    paths: Bounded<String, Option<Arc<str>>>,
}

/// Readable names of running programs, shared by the threads that measure: reads happen on the
/// caller's thread, outside the lock, once per instance and per executable while cached.
pub struct ProgramNames {
    caches: Mutex<Caches>,
    image_path: Box<ImagePathReader>,
    version_name: Box<VersionNameReader>,
}
impl Default for ProgramNames {
    fn default() -> Self {
        Self::new()
    }
}
impl ProgramNames {
    /// Native reads on Windows; other systems know no readable name.
    pub fn new() -> Self {
        #[cfg(windows)]
        {
            Self::with_readers(
                crate::windows::version_info::image_path,
                crate::windows::version_info::version_name,
            )
        }
        #[cfg(not(windows))]
        {
            Self::with_readers(|_, _| None, |_| None)
        }
    }
    fn with_readers(
        image_path: impl Fn(u32, u64) -> Option<String> + Send + Sync + 'static,
        version_name: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            caches: Mutex::new(Caches {
                instances: Bounded::new(MAX_INSTANCES),
                paths: Bounded::new(MAX_PATHS),
            }),
            image_path: Box::new(image_path),
            version_name: Box::new(version_name),
        }
    }
    fn caches(&self) -> MutexGuard<'_, Caches> {
        self.caches.lock().unwrap_or_else(|p| p.into_inner())
    }
    /// The readable name of one process instance, or `None` when it is unknown: the process
    /// refused a query-only handle, exited, its PID now names another instance, or its
    /// executable has no usable version resource.
    pub fn name(&self, pid: u32, created: u64) -> Option<String> {
        if let Some(known) = self.caches().instances.get(&(pid, created)) {
            return known.as_deref().map(str::to_owned);
        }
        let name = (self.image_path)(pid, created).and_then(|path| self.path_name(&path));
        self.caches().instances.insert((pid, created), name.clone());
        name.as_deref().map(str::to_owned)
    }
    fn path_name(&self, path: &str) -> Option<Arc<str>> {
        let key = path.to_lowercase();
        if let Some(known) = self.caches().paths.get(&key) {
            return known.clone();
        }
        let name: Option<Arc<str>> = (self.version_name)(path).map(Arc::from);
        self.caches().paths.insert(key, name.clone());
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn names_are_trimmed_collapsed_stripped_and_bounded() {
        assert_eq!(
            sanitize_program_name("  Google \t\n Chrome  ").as_deref(),
            Some("Google Chrome")
        );
        // Bidirectional overrides, isolates, marks, controls and a byte-order mark disappear.
        assert_eq!(
            sanitize_program_name("\u{FEFF}Bad\u{202E}exe.\u{2066}cod\u{200F}e\u{7}\u{0}")
                .as_deref(),
            Some("Badexe.code")
        );
        assert_eq!(
            sanitize_program_name("Microsoft® Windows® Operating System").as_deref(),
            Some("Microsoft® Windows® Operating System")
        );
        for empty in ["", "   ", "\u{202E}\u{200E}", "\t\r\n", "\u{1}\u{2}"] {
            assert_eq!(sanitize_program_name(empty), None, "{empty:?}");
        }
        // 64 characters at most, counted in characters, never ending on a space.
        let long = "é".repeat(80);
        assert_eq!(sanitize_program_name(&long).unwrap().chars().count(), 64);
        let spaced = format!("{} tail", "a".repeat(63));
        assert_eq!(sanitize_program_name(&spaced).unwrap(), "a".repeat(63));
        let exact = format!("{} b", "a".repeat(62));
        assert_eq!(sanitize_program_name(&exact).unwrap(), exact);
    }

    fn table(entries: &[(&str, &str)]) -> impl FnMut(&str) -> Option<String> {
        let map: HashMap<String, String> = entries
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |sub_block: &str| map.get(sub_block).cloned()
    }

    #[test]
    fn the_description_comes_first_then_the_product_then_fallback_tables() {
        let english = [
            (
                "\\StringFileInfo\\040904b0\\FileDescription",
                "Antimalware Service Executable",
            ),
            (
                "\\StringFileInfo\\040904b0\\ProductName",
                "Microsoft® Windows® Operating System",
            ),
        ];
        assert_eq!(
            name_from_tables(&[(0x0409, 0x04B0)], table(&english)).as_deref(),
            Some("Antimalware Service Executable")
        );
        // An empty or blank description falls back to the product name of the same table.
        let blank = [
            ("\\StringFileInfo\\040c04b0\\FileDescription", "  "),
            (
                "\\StringFileInfo\\040c04b0\\ProductName",
                "Lecteur multimédia",
            ),
            (
                "\\StringFileInfo\\040904b0\\FileDescription",
                "Media Player",
            ),
        ];
        assert_eq!(
            name_from_tables(&[(0x040C, 0x04B0)], table(&blank)).as_deref(),
            Some("Lecteur multimédia")
        );
        // Only the first declared translation is tried before the fallbacks.
        let second = [
            (
                "\\StringFileInfo\\040704b0\\FileDescription",
                "Zweite Sprache",
            ),
            ("\\StringFileInfo\\040904e4\\ProductName", "Legacy Tool"),
        ];
        assert_eq!(
            name_from_tables(&[(0x0411, 0x04B0), (0x0407, 0x04B0)], table(&second)).as_deref(),
            Some("Legacy Tool")
        );
        // Without any translation, the neutral table is the last resort.
        let neutral = [("\\StringFileInfo\\000004b0\\FileDescription", "Neutral")];
        assert_eq!(
            name_from_tables(&[], table(&neutral)).as_deref(),
            Some("Neutral")
        );
        assert_eq!(name_from_tables(&[(0x0409, 0x04B0)], table(&[])), None);
    }

    #[test]
    fn translation_pairs_text_and_drive_paths_are_parsed_defensively() {
        assert_eq!(
            translations(&[0x09, 0x04, 0xB0, 0x04, 0x0C, 0x04, 0xE4, 0x04, 0xFF]),
            [(0x0409, 0x04B0), (0x040C, 0x04E4)]
        );
        let text: Vec<u8> = "Chrome\0junk"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .chain([0x41])
            .collect();
        assert_eq!(utf16_text(&text), "Chrome");
        assert_eq!(utf16_text(&[0x41]), "");
        assert_eq!(drive_letter(r"C:\Program Files\app.exe"), Some('C'));
        for refused in [
            "app.exe",
            r"\\server\share\app.exe",
            r"\\?\C:\app.exe",
            r"\Device\HarddiskVolume3\app.exe",
            "C:app.exe",
            "Registry",
            "",
        ] {
            assert_eq!(drive_letter(refused), None, "{refused}");
        }
    }

    #[test]
    fn a_bounded_map_forgets_its_oldest_entries_first() {
        let mut map = Bounded::new(3);
        for key in 0..3 {
            map.insert(key, key * 10);
        }
        // Updating a key keeps its age.
        map.insert(0, 1);
        assert_eq!(map.get(&0), Some(&1));
        map.insert(3, 30);
        assert_eq!(map.len(), 3);
        assert_eq!(map.get(&0), None);
        assert_eq!(map.get(&1), Some(&10));
        for key in 4..1000 {
            map.insert(key, key);
        }
        assert_eq!(map.len(), 3);
        assert_eq!(map.get(&999), Some(&999));
        assert_eq!(map.get(&996), None);
    }

    #[test]
    fn names_are_read_once_per_executable_and_once_per_instance() {
        let opened = Arc::new(AtomicUsize::new(0));
        let read = Arc::new(AtomicUsize::new(0));
        let (o, r) = (opened.clone(), read.clone());
        let names = ProgramNames::with_readers(
            move |pid, created| {
                o.fetch_add(1, Ordering::SeqCst);
                match (pid, created) {
                    (10 | 11, 1) => Some(r"C:\Apps\Browser\browser.exe".to_owned()),
                    // Same file, other case: one cached read.
                    (12, 1) => Some(r"c:\apps\browser\BROWSER.EXE".to_owned()),
                    (20, 1) => Some(r"C:\Tools\bare.exe".to_owned()),
                    _ => None,
                }
            },
            move |path| {
                r.fetch_add(1, Ordering::SeqCst);
                path.to_lowercase()
                    .ends_with("browser.exe")
                    .then(|| "Example Browser".to_owned())
            },
        );
        for _ in 0..3 {
            assert_eq!(names.name(10, 1).as_deref(), Some("Example Browser"));
        }
        assert_eq!(names.name(11, 1).as_deref(), Some("Example Browser"));
        assert_eq!(names.name(12, 1).as_deref(), Some("Example Browser"));
        // No version resource, and a process that cannot be opened: unknown, and remembered.
        assert_eq!(names.name(20, 1), None);
        assert_eq!(names.name(20, 1), None);
        assert_eq!(names.name(30, 1), None);
        assert_eq!(names.name(30, 1), None);
        // A reused PID is another instance: it is resolved again.
        assert_eq!(names.name(10, 2), None);
        assert_eq!(opened.load(Ordering::SeqCst), 6);
        assert_eq!(read.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn the_caches_stay_bounded() {
        let names = ProgramNames::with_readers(
            |pid, _| Some(format!(r"C:\Apps\app-{pid}.exe")),
            |_| Some("App".to_owned()),
        );
        for pid in 0..(MAX_INSTANCES as u32 + 100) {
            names.name(pid, 1);
        }
        let caches = names.caches();
        assert_eq!(caches.instances.len(), MAX_INSTANCES);
        assert_eq!(caches.paths.len(), MAX_PATHS);
    }

    #[cfg(not(windows))]
    #[test]
    fn other_systems_know_no_readable_name() {
        assert_eq!(ProgramNames::new().name(std::process::id(), 1), None);
    }
}
