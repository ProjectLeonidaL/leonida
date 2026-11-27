//! Where the game folder is when leonida.toml does not say: the one an
//! earlier build used, or one found by a short search of this PC's folders.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use leonida_hle::runtime::Title;

/// `Game/TitleId` in the game's MicrosoftGame.config.
pub const TITLE_ID: u32 = 0x7952_987A;

const TIME_LIMIT: Duration = Duration::from_secs(2);
/// While nothing is found: a disk that sleeps takes seconds to answer.
const SLOW_DISK_LIMIT: Duration = Duration::from_secs(6);
/// For each drive, and for Downloads and Desktop together.
const FOLDER_LIMIT: usize = 2000;
/// A manifest is a few KB; a bigger file is not read.
const MANIFEST_LIMIT: u64 = 1 << 20;

const SKIPPED: &[&str] = &[
    "windows",
    "program files",
    "program files (x86)",
    "programdata",
    "$recycle.bin",
    "system volume information",
    "windowsapps",
];

/// In %LOCALAPPDATA%\leonida, not next to the exe (every build is unpacked to
/// a new folder) nor in the data folder, which a leonida.toml can move.
fn memory_file() -> Result<PathBuf> {
    Ok(leonida_core::config::user_dir()?.join("game-folder.txt"))
}

/// The last game folder used, if it still has a MicrosoftGame.config.
pub fn remembered() -> Option<PathBuf> {
    read_memory(&memory_file().ok()?)
}

/// Keeps `dir` for the builds that come after this one.
pub fn remember(dir: &Path) -> Result<()> {
    write_memory(&memory_file()?, dir)
}

fn read_memory(file: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(file).ok()?;
    let dir = PathBuf::from(text.trim());
    (!dir.as_os_str().is_empty() && dir.join("MicrosoftGame.config").is_file()).then_some(dir)
}

fn write_memory(file: &Path, dir: &Path) -> Result<()> {
    leonida_core::ensure_outside_repo(file)?;
    let dir = std::path::absolute(dir)?;
    let text = dir
        .to_str()
        .with_context(|| format!("{} is not valid UTF-8", dir.display()))?;
    if std::fs::read_to_string(file).is_ok_and(|old| old == text) {
        return Ok(());
    }
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(file, text).with_context(|| format!("writing {}", file.display()))
}

/// The folders holding the game with `title_id` that a search finds in about
/// two seconds, or up to six while it finds none.
pub fn game_dirs(title_id: u32) -> Vec<PathBuf> {
    let start = Instant::now();
    let (sender, receiver) = mpsc::channel();
    // A thread for each drive: one call on a sleeping disk or an offline
    // network drive can block for seconds, and then holds up only that drive.
    for roots in roots() {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let deadline = start + SLOW_DISK_LIMIT;
            let mut search = Search::new(title_id, deadline, FOLDER_LIMIT, |dir| {
                let _ = sender.send(dir);
            });
            search.run(&roots);
        });
    }
    drop(sender);
    let mut found = Vec::new();
    loop {
        let limit = if found.is_empty() {
            SLOW_DISK_LIMIT
        } else {
            TIME_LIMIT
        };
        let Some(left) = (start + limit).checked_duration_since(Instant::now()) else {
            break;
        };
        match receiver.recv_timeout(left) {
            Ok(dir) if !found.contains(&dir) => found.push(dir),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    found
}

/// A folder whose subfolders are searched `depth` levels down. Each
/// subfolder, or its `suffix` subfolder, may be the game folder.
struct Root {
    dir: PathBuf,
    depth: usize,
    suffix: Option<&'static str>,
}

fn root(dir: PathBuf, depth: usize, suffix: Option<&'static str>) -> Root {
    Root { dir, depth, suffix }
}

/// What each search thread walks, in order. A drive's top level comes before
/// its second level, so a game folder there is found early.
fn roots() -> Vec<Vec<Root>> {
    let mut roots: Vec<Vec<Root>> = (b'C'..=b'Z')
        .map(|letter| {
            let drive = PathBuf::from(format!("{}:\\", letter as char));
            vec![
                root(drive.join("XboxGames"), 1, Some("Content")),
                root(drive.clone(), 1, None),
                root(drive, 2, None),
            ]
        })
        .collect();
    if let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        roots.push(vec![
            root(home.join("Downloads"), 1, None),
            root(home.join("Desktop"), 1, None),
        ]);
    }
    roots
}

struct Search<F> {
    title_id: u32,
    deadline: Instant,
    folders_left: usize,
    seen: HashSet<String>,
    found: F,
}

impl<F: FnMut(PathBuf)> Search<F> {
    fn new(title_id: u32, deadline: Instant, folders: usize, found: F) -> Self {
        Search {
            title_id,
            deadline,
            folders_left: folders,
            seen: HashSet::new(),
            found,
        }
    }

    fn run(&mut self, roots: &[Root]) {
        for root in roots {
            if !self.walk(&root.dir, root.depth, root.suffix) {
                return;
            }
        }
    }

    /// False once the time or the folder count has run out.
    fn walk(&mut self, dir: &Path, depth: usize, suffix: Option<&str>) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return true;
        };
        for entry in entries.flatten() {
            if self.folders_left == 0 || Instant::now() >= self.deadline {
                return false;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if !(kind.is_dir() || kind.is_symlink()) || skipped(&entry.file_name()) {
                continue;
            }
            self.folders_left -= 1;
            let path = entry.path();
            let candidate = suffix.map_or_else(|| path.clone(), |s| path.join(s));
            if is_game(&candidate, self.title_id)
                && self.seen.insert(candidate.to_string_lossy().to_lowercase())
            {
                (self.found)(candidate);
            }
            // A link or junction may be the game folder, but is not walked
            // into: it can loop.
            if kind.is_dir() && depth > 1 && !self.walk(&path, depth - 1, suffix) {
                return false;
            }
        }
        true
    }
}

fn skipped(name: &OsStr) -> bool {
    let name = name.to_string_lossy().to_lowercase();
    name.starts_with(['.', '$']) || SKIPPED.contains(&name.as_str())
}

fn is_game(dir: &Path, title_id: u32) -> bool {
    let manifest = dir.join("MicrosoftGame.config");
    std::fs::metadata(&manifest).is_ok_and(|m| m.is_file() && m.len() < MANIFEST_LIMIT)
        && std::fs::read_to_string(&manifest)
            .is_ok_and(|text| Title::from_manifest(&text).id == title_id)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use super::{Root, Search, TITLE_ID, read_memory, root, write_memory};

    fn game(dir: &Path, title_id: u32) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let manifest = format!("<Game>\n  <TitleId>{title_id:08X}</TitleId>\n</Game>\n");
        std::fs::write(dir.join("MicrosoftGame.config"), manifest).unwrap();
        dir.to_path_buf()
    }

    fn search(roots: &[Root], folders: usize) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(60);
        Search::new(TITLE_ID, deadline, folders, |dir| found.push(dir)).run(roots);
        found
    }

    fn under(dir: &Path, depth: usize) -> Root {
        root(dir.to_path_buf(), depth, None)
    }

    #[test]
    fn a_matching_manifest_is_found() {
        let temp = tempfile::tempdir().unwrap();
        let first = game(&temp.path().join("Games").join("The Game"), TITLE_ID);
        let second = game(&temp.path().join("Copy"), TITLE_ID);
        let mut found = search(&[under(temp.path(), 2)], 1000);
        found.sort();
        let mut wanted = vec![first, second];
        wanted.sort();
        assert_eq!(found, wanted);
    }

    #[test]
    fn another_title_id_is_ignored() {
        let temp = tempfile::tempdir().unwrap();
        game(&temp.path().join("Other"), 0x1234_5678);
        std::fs::create_dir(temp.path().join("Empty")).unwrap();
        assert!(search(&[under(temp.path(), 2)], 1000).is_empty());
    }

    #[test]
    fn skipped_folders_are_not_searched() {
        let temp = tempfile::tempdir().unwrap();
        for name in [
            "Windows",
            "Program Files",
            "PROGRAMDATA",
            ".hidden",
            "$Stash",
        ] {
            game(&temp.path().join(name), TITLE_ID);
            game(&temp.path().join(name).join("inner"), TITLE_ID);
        }
        assert!(search(&[under(temp.path(), 2)], 1000).is_empty());
    }

    #[test]
    fn the_depth_limit_holds() {
        let temp = tempfile::tempdir().unwrap();
        let near = game(&temp.path().join("a").join("b"), TITLE_ID);
        game(&temp.path().join("x").join("y").join("z"), TITLE_ID);
        assert_eq!(search(&[under(temp.path(), 2)], 1000), [near]);
        assert!(search(&[under(temp.path(), 1)], 1000).is_empty());
    }

    #[test]
    fn a_suffix_names_the_game_folder() {
        let temp = tempfile::tempdir().unwrap();
        let content = game(&temp.path().join("The Game").join("Content"), TITLE_ID);
        let roots = [root(temp.path().to_path_buf(), 1, Some("Content"))];
        assert_eq!(search(&roots, 1000), [content]);
    }

    #[test]
    fn the_folder_limit_stops_the_search() {
        let temp = tempfile::tempdir().unwrap();
        game(&temp.path().join("Game"), TITLE_ID);
        assert!(search(&[under(temp.path(), 1)], 0).is_empty());
        assert_eq!(search(&[under(temp.path(), 1)], 1).len(), 1);
    }

    #[test]
    fn the_time_limit_stops_the_search() {
        let temp = tempfile::tempdir().unwrap();
        game(&temp.path().join("Game"), TITLE_ID);
        let mut found = Vec::new();
        Search::new(TITLE_ID, Instant::now(), 1000, |dir| found.push(dir))
            .run(&[under(temp.path(), 1)]);
        assert!(found.is_empty());
    }

    #[test]
    fn the_same_folder_is_found_once() {
        let temp = tempfile::tempdir().unwrap();
        game(&temp.path().join("Game"), TITLE_ID);
        let roots = [under(temp.path(), 1), under(temp.path(), 2)];
        assert_eq!(search(&roots, 1000).len(), 1);
    }

    #[test]
    fn a_junction_is_checked_but_not_walked() {
        let temp = tempfile::tempdir().unwrap();
        let target = game(&temp.path().join("elsewhere").join("Game"), TITLE_ID);
        game(&target.join("inner"), TITLE_ID);
        let link = temp.path().join("games").join("Game");
        std::fs::create_dir(temp.path().join("games")).unwrap();
        // A junction, unlike a symbolic link, needs no Developer Mode.
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
        let roots = [under(&temp.path().join("games"), 2)];
        assert_eq!(search(&roots, 1000), [link]);
    }

    #[test]
    fn a_remembered_folder_needs_its_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("leonida").join("game-folder.txt");
        assert_eq!(read_memory(&file), None);
        let dir = game(&temp.path().join("Game"), TITLE_ID);
        write_memory(&file, &dir).unwrap();
        write_memory(&file, &dir).unwrap();
        assert_eq!(read_memory(&file), Some(dir.clone()));
        std::fs::remove_file(dir.join("MicrosoftGame.config")).unwrap();
        assert_eq!(read_memory(&file), None);
    }
}
