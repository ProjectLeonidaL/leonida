//! What `leonida` does with no command, as when leonida.exe is double-clicked:
//! what `run-gta6.ps1` does after its build step, for a player who has never
//! opened a terminal. The console window stays open until they have read how
//! it ended.
//!
//! The warm-up and the game run as `leonida translate --warm all` and
//! `leonida run --execute` in this window, so they take the same paths as
//! from the script, a title that relaunches itself gets a command line it can
//! use again, and a crash still leaves this process to say what happened.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use anyhow::{Context, Result};
use windows_sys::Win32::Foundation::{FALSE, TRUE};
use windows_sys::Win32::System::Console::{
    CTRL_BREAK_EVENT, CTRL_C_EVENT, FlushConsoleInputBuffer, GetStdHandle, STD_INPUT_HANDLE,
    SetConsoleCtrlHandler,
};
use windows_sys::core::BOOL;

/// How a process that Ctrl+C or Ctrl+Break ended exits (STATUS_CONTROL_C_EXIT).
const CTRL_C_EXIT: u32 = 0xC000_013A;

const WARM_UP_QUESTION: &str = "\
Warm up the shader cache first? It translates every shader before the game starts:
about 1 to 2 minutes the first time (and after an update), about 30 s when the cache is full.
WARNING: without it, the game translates shaders while you play, so it stutters and runs
slower until the cache fills up.";

/// Never returns: waits for Enter, then ends with the game's exit code (1 if
/// it could not be started).
pub fn run() -> ! {
    println!("{}", title(crate::BUILD));
    use_exe_folder();
    let code = match play() {
        Ok(status) if status.success() => {
            println!();
            println!("The game has closed.");
            0
        }
        Ok(status) if stopped_by_ctrl_c(status) => {
            println!();
            println!("The game was stopped with Ctrl+C.");
            status.code().unwrap_or(1)
        }
        Ok(status) => {
            println!();
            println!("The game ended with {}.", describe(status));
            println!("If it did not end on purpose, the lines above say why.");
            if let Ok(cfg) = leonida_core::Config::load() {
                println!(
                    "A crash also leaves a report in crashes.log under {}.",
                    cfg.data_dir.display()
                );
            }
            println!("Start leonida.exe again to retry.");
            status.code().unwrap_or(1)
        }
        Err(e) => {
            println!();
            println!("Could not start the game: {e:#}");
            println!("Fix that, then start leonida.exe again.");
            1
        }
    };
    wait_for_enter();
    std::process::exit(code)
}

fn play() -> Result<ExitStatus> {
    let dir = game_dir()?;

    // Shaders not in the cache are translated the first time the game uses
    // them, mid-play.
    println!();
    println!("{WARM_UP_QUESTION}");
    let warm_up = ask_yes("Warm up now?");
    outlive_ctrl_c();
    if warm_up {
        match leonida(&["translate", "--warm", "all"], &dir, false) {
            Ok(status) if status.success() => {}
            Ok(status) if stopped_by_ctrl_c(status) => {
                println!("WARNING: the warm-up was stopped with Ctrl+C; starting anyway")
            }
            Ok(status) => {
                println!(
                    "WARNING: the warm-up failed ({}); starting anyway",
                    describe(status)
                )
            }
            Err(e) => println!("WARNING: the warm-up did not start ({e:#}); starting anyway"),
        }
    } else {
        println!(
            "WARNING: starting without a warm-up: expect stutter until the shader cache fills up"
        );
    }

    println!();
    println!("Starting the game. Keep this window open: it shows warnings and errors.");
    leonida(&["run", "--execute"], &dir, true)
}

/// The configured game folder if it has a MicrosoftGame.config, else the one
/// an earlier build used, else one found on this PC or given by the player
/// (saved to leonida.toml). The folder used is remembered.
fn game_dir() -> Result<PathBuf> {
    let cfg = leonida_core::Config::load().context(
        "leonida.toml in this folder could not be read; correct it, or delete it to be \
         asked for the game folder again",
    )?;
    if let Some(dir) = &cfg.game_dir {
        if dir.join("MicrosoftGame.config").is_file() {
            println!("Game folder: {}", dir.display());
            crate::remember_game_dir(dir);
            return Ok(dir.clone());
        }
        println!(
            "The game folder has no MicrosoftGame.config: {}",
            dir.display()
        );
    }
    if let Some(dir) = crate::find::remembered() {
        println!("Game folder: {}", dir.display());
        if cfg.game_dir.is_some()
            && let Err(e) = crate::save_game_dir(&dir)
        {
            println!("WARNING: {e:#}");
        }
        return Ok(dir);
    }
    println!("Looking for the game on this PC...");
    let found: Vec<PathBuf> = crate::find::game_dirs(crate::find::TITLE_ID)
        .into_iter()
        .filter(|dir| crate::check_game_dir(dir).is_ok())
        .collect();
    if found.is_empty() {
        println!("The game was not found.");
    }
    if let Some(dir) = choose(&found) {
        crate::save_game_dir(&dir)?;
        return Ok(dir);
    }
    println!(
        "Enter the folder of your copy of the game (the one with MicrosoftGame.config in it)."
    );
    println!("You can drag the folder from File Explorer into this window, then press Enter:");
    crate::read_game_dir()
}

/// The found folder the player agrees to use, if any.
fn choose(found: &[PathBuf]) -> Option<PathBuf> {
    match found {
        [] => None,
        [dir] => {
            ask_yes(&format!("Found the game in {}. Use it?", dir.display())).then(|| dir.clone())
        }
        _ => {
            println!("Found the game in more than one folder:");
            for (i, dir) in found.iter().enumerate() {
                println!("  {}. {}", i + 1, dir.display());
            }
            loop {
                print!("Enter the number of the one to use, or 0 to enter a folder: ");
                let _ = std::io::stdout().flush();
                let mut line = String::new();
                if std::io::stdin().lock().read_line(&mut line).unwrap_or(0) == 0 {
                    return None;
                }
                match pick(&line, found.len()) {
                    Some(0) => return None,
                    Some(n) => return Some(found[n - 1].clone()),
                    None => println!("Type a number from 0 to {}.", found.len()),
                }
            }
        }
    }
}

/// The number typed, if it is 0 to `count`.
fn pick(answer: &str, count: usize) -> Option<usize> {
    answer.trim().parse().ok().filter(|n| *n <= count)
}

/// Runs this program again in this window with `args`, for the game in
/// `game_dir`. `quiet` keeps the console to warnings and errors unless
/// `LEONIDA_LOG` asks for more (`LEONIDA_LOG=info` shows the GPU layer's
/// status lines).
fn leonida(args: &[&str], game_dir: &Path, quiet: bool) -> Result<ExitStatus> {
    let exe = std::env::current_exe().context("finding leonida.exe")?;
    let mut command = std::process::Command::new(exe);
    command.args(args).env("LEONIDA_GAME_DIR", game_dir);
    if quiet && std::env::var_os("LEONIDA_LOG").is_none_or(|v| v.is_empty()) {
        command.env("LEONIDA_LOG", "warn");
    }
    command
        .status()
        .with_context(|| format!("starting leonida {}", args.join(" ")))
}

fn title(build: &str) -> String {
    if build == "local" {
        "Leonida Legacy (local build)".to_owned()
    } else {
        format!("Leonida Legacy, build {build}")
    }
}

/// An exit code; a Windows exception code (negative as an `i32`) in hex.
fn describe(status: ExitStatus) -> String {
    match status.code() {
        Some(code) if code < 0 => format!("exit code {:#010x}", code as u32),
        Some(code) => format!("exit code {code}"),
        None => status.to_string(),
    }
}

/// Asks a question whose default is yes. A closed input counts as Enter.
fn ask_yes(question: &str) -> bool {
    print!("{question} [Y/n]: ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    is_yes(&line)
}

fn is_yes(answer: &str) -> bool {
    matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes"
    )
}

fn wait_for_enter() {
    println!();
    print!("Press Enter to close this window.");
    let _ = std::io::stdout().flush();
    // Nobody reads the console while the warm-up or the game runs: an Enter
    // pressed then would close the window before this can be read.
    // SAFETY: GetStdHandle takes no pointers, and FlushConsoleInputBuffer
    // only fails if the handle is not a console.
    unsafe { FlushConsoleInputBuffer(GetStdHandle(STD_INPUT_HANDLE)) };
    let _ = std::io::stdin().lock().read_line(&mut String::new());
}

/// Moves to leonida.exe's folder when there is no config file to use, so a
/// start from elsewhere (the Run dialog, a launcher) finds and saves
/// leonida.toml next to the exe, as a double-click in File Explorer does.
fn use_exe_folder() {
    let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    else {
        return;
    };
    if is_temporary(&dir) {
        println!(
            "WARNING: leonida.exe runs from a temporary folder, which Windows empties: {}",
            dir.display()
        );
        println!("Unpack the zip to a folder of its own first, or your settings are lost.");
    }
    if std::env::var_os("LEONIDA_CONFIG").is_some() || Path::new("leonida.toml").is_file() {
        return;
    }
    if let Err(e) = std::env::set_current_dir(&dir) {
        println!("WARNING: could not move to {} ({e})", dir.display());
    }
}

/// Whether `dir` is under %TEMP%, where File Explorer puts an exe opened
/// from inside a zip. Canonical paths, since %TEMP% may use short names.
fn is_temporary(dir: &Path) -> bool {
    match (
        std::fs::canonicalize(dir),
        std::fs::canonicalize(std::env::temp_dir()),
    ) {
        (Ok(dir), Ok(temp)) => dir.starts_with(temp),
        _ => false,
    }
}

/// From here on Ctrl+C and Ctrl+Break end only the warm-up or the game, which
/// keep the default handler, and this process stays to say how they ended.
/// Not `SetConsoleCtrlHandler(None, TRUE)`: children inherit that and would
/// ignore Ctrl+C too.
fn outlive_ctrl_c() {
    // SAFETY: the handler is a plain function that lives as long as the
    // process and touches no state.
    unsafe { SetConsoleCtrlHandler(Some(ignore_ctrl_c), TRUE) };
}

unsafe extern "system" fn ignore_ctrl_c(event: u32) -> BOOL {
    if event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT {
        TRUE
    } else {
        FALSE
    }
}

fn stopped_by_ctrl_c(status: ExitStatus) -> bool {
    status.code().is_some_and(|code| code as u32 == CTRL_C_EXIT)
}

#[cfg(test)]
mod tests {
    use std::os::windows::process::ExitStatusExt;
    use std::path::Path;
    use std::process::ExitStatus;

    use super::{is_temporary, is_yes, pick, stopped_by_ctrl_c, title};

    #[test]
    fn ctrl_c_is_its_exit_code() {
        assert!(stopped_by_ctrl_c(ExitStatus::from_raw(0xC000_013A)));
        for code in [0, 1, 0xC000_0005] {
            assert!(!stopped_by_ctrl_c(ExitStatus::from_raw(code)), "{code:#x}");
        }
    }

    #[test]
    fn the_temp_folder_is_temporary() {
        let dir = tempfile::tempdir().unwrap();
        assert!(is_temporary(dir.path()));
        assert!(!is_temporary(Path::new(env!("CARGO_MANIFEST_DIR"))));
    }

    #[test]
    fn enter_and_yes_mean_yes() {
        for answer in ["", "\r\n", "y", "Y\r\n", " yes ", "YES"] {
            assert!(is_yes(answer), "{answer:?}");
        }
        for answer in ["n", "no\r\n", "N", "yep", "x"] {
            assert!(!is_yes(answer), "{answer:?}");
        }
    }

    #[test]
    fn a_pick_is_a_listed_number_or_zero() {
        assert_eq!(pick("0\r\n", 2), Some(0));
        assert_eq!(pick(" 2 ", 2), Some(2));
        for answer in ["", "3", "-1", "one"] {
            assert_eq!(pick(answer, 2), None, "{answer:?}");
        }
    }

    #[test]
    fn the_title_names_the_build() {
        assert_eq!(title("123"), "Leonida Legacy, build 123");
        assert_eq!(title("local"), "Leonida Legacy (local build)");
    }
}
