//! `monitorx` / `mwaitx` sites patched in place at their first exception
//! (`LEONIDA_MWAITX_PATCH=1`).
//!
//! The title waits for a fence in a loop of `monitorx`, a look at the value and `mwaitx`. On
//! an Intel processor each of the two is an exception, about a microsecond each in Windows'
//! dispatch, and the emulation does no more than skip the first and spin once on the second.
//! While the graphics side is behind, that loop runs for 150 ms at a time: 110,000 to 200,000
//! exceptions a second on average, over 400,000 at peaks (five runs on an i7-13620H).
//!
//! With the switch the three bytes of the site the exception came from are rewritten to what
//! the emulation does: `monitorx` a three-byte `nop`, `mwaitx` a `pause` and a `nop`. The loop
//! then spins with `pause` without leaving the title's code. Only a site that faulted is
//! rewritten, so no data or bytes in the middle of an instruction are taken for code.
//!
//! The three bytes change in one locked write of the aligned 8 or 16 bytes around them:
//! another thread running the site sees either the old instruction (which faults, and the
//! handler finds the new bytes: [`is_replacement`]) or the new one, never a mix. A site in the
//! last two bytes of 16 is left to the emulation.
//!
//! shortcut: on the console `mwaitx` lets the core rest until the value is written or its
//! timer runs out; here it spins, as the emulation did.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

/// `LEONIDA_MWAITX_PATCH=1`: wait hint sites are patched at their first exception.
pub(super) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("LEONIDA_MWAITX_PATCH").is_some_and(|v| v == "1"))
}

/// What a wait hint's three bytes become.
pub(super) fn replacement(code: [u8; 3]) -> Option<[u8; 3]> {
    match code {
        [0x0f, 0x01, 0xfa] => Some([0x0f, 0x1f, 0x00]),
        [0x0f, 0x01, 0xfb] => Some([0xf3, 0x90, 0x90]),
        _ => None,
    }
}

/// Whether `code` starts with a replacement: the faulting thread fetched the site before
/// another thread patched it, and runs it again.
pub(super) fn is_replacement(code: &[u8]) -> bool {
    code.len() >= 3 && [[0x0f, 0x1f, 0x00], [0xf3, 0x90, 0x90]].iter().any(|r| code[..3] == *r)
}

/// The aligned word the three bytes at `at` lie in: its address and its size (8 or 16).
fn word_of(at: usize) -> Option<(usize, usize)> {
    if at & 7 <= 5 {
        Some((at & !7, 8))
    } else if at & 15 <= 13 {
        Some((at & !15, 16))
    } else {
        None
    }
}

/// `word` (little-endian) with the bytes `old` at `offset` replaced by `new`; None if they
/// are not `old`.
fn splice(word: u128, offset: usize, old: [u8; 3], new: [u8; 3]) -> Option<u128> {
    let mut bytes = word.to_le_bytes();
    if bytes[offset..offset + 3] != old {
        return None;
    }
    bytes[offset..offset + 3].copy_from_slice(&new);
    Some(u128::from_le_bytes(bytes))
}

/// A locked compare-and-exchange of the 16 aligned bytes at `at`; whether `old` was there.
///
/// # Safety
/// `at` is 16-byte aligned, and its 16 bytes are readable and writable.
unsafe fn exchange16(at: *mut u128, old: u128, new: u128) -> bool {
    let ok: u8;
    // SAFETY: as the caller says; rbx is saved and restored around the instruction, which
    // needs it and which the compiler does not let us name.
    unsafe {
        std::arch::asm!(
            "xchg {low}, rbx",
            "lock cmpxchg16b [{at}]",
            "sete {ok}",
            "mov rbx, {low}",
            at = in(reg) at,
            low = inout(reg) new as u64 => _,
            ok = out(reg_byte) ok,
            in("rcx") (new >> 64) as u64,
            inout("rax") old as u64 => _,
            inout("rdx") (old >> 64) as u64 => _,
            options(nostack),
        );
    }
    ok != 0
}

/// Replaces the bytes `old` at `at` by `new` in one locked write; whether they were `old`.
///
/// # Safety
/// The aligned word around `at` ([`word_of`]) is readable and writable.
unsafe fn exchange(at: usize, old: [u8; 3], new: [u8; 3]) -> bool {
    let Some((word, size)) = word_of(at) else {
        return false;
    };
    let offset = at - word;
    if size == 8 {
        // SAFETY: aligned, readable and writable, as the caller says.
        let cell = unsafe { &*(word as *const AtomicU64) };
        let was = cell.load(Relaxed);
        let Some(now) = splice(u128::from(was), offset, old, new) else {
            return false;
        };
        return cell.compare_exchange(was, now as u64, Relaxed, Relaxed).is_ok();
    }
    // SAFETY: aligned, readable; a torn read only makes the exchange fail.
    let was = unsafe { (word as *const u128).read_volatile() };
    let Some(now) = splice(was, offset, old, new) else {
        return false;
    };
    // SAFETY: aligned, readable and writable, as the caller says.
    unsafe { exchange16(word as *mut u128, was, now) }
}

/// Sites patched, sites left to the emulation (no room for one write, or their page could not
/// be made writable), exceptions on sites already patched, and the wait hints emulated since
/// the last line.
static COUNTS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
const PATCHED: usize = 0;
const LEFT: usize = 1;
const STALE: usize = 2;
const HINTS: usize = 3;
/// When the last line was written, in milliseconds.
static LINE_AT: AtomicU64 = AtomicU64::new(0);
/// Taken while a site is patched: two threads changing the protection of one page at once
/// would leave it read-only under the other's write. A thread that finds it taken leaves its
/// site to the emulation this time.
static PATCHING: AtomicBool = AtomicBool::new(false);

/// A wait hint emulated (counted for the line).
pub(super) fn emulated() {
    COUNTS[HINTS].fetch_add(1, Relaxed);
}

/// An exception on a site already patched.
pub(super) fn stale() {
    COUNTS[STALE].fetch_add(1, Relaxed);
}

/// Patches the wait hint at `at` whose three bytes are `code`; whether it did.
///
/// # Safety
/// `at` is the address of a wait hint the processor just refused, in the title's code.
pub(super) unsafe fn site(at: usize, code: [u8; 3]) -> bool {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let (Some(new), Some((word, size))) = (replacement(code), word_of(at)) else {
        COUNTS[LEFT].fetch_add(1, Relaxed);
        return false;
    };
    if PATCHING.swap(true, Relaxed) {
        return false;
    }
    let mut old = 0u32;
    // SAFETY: the site's own page (an aligned word never crosses one), made writable for the
    // write and given back its protection; the word lies in it, so it is readable.
    let done = unsafe {
        let done = VirtualProtect(word as *const _, size, PAGE_EXECUTE_READWRITE, &mut old) != 0
            && exchange(at, code, new);
        if old != 0 {
            VirtualProtect(word as *const _, size, old, &mut old);
        }
        FlushInstructionCache(GetCurrentProcess(), word as *const _, size);
        done
    };
    PATCHING.store(false, Relaxed);
    COUNTS[if done { PATCHED } else { LEFT }].fetch_add(1, Relaxed);
    done
}

/// A line a second while the switch is on: wait hints emulated a second, and the sites. A
/// patched site runs in the title's code and is not counted.
pub(super) fn line(now_us: u64) {
    let now = now_us / 1000;
    let last = LINE_AT.load(Relaxed);
    if now < last + 1_000 || LINE_AT.compare_exchange(last, now, Relaxed, Relaxed).is_err() {
        return;
    }
    let hints = COUNTS[HINTS].swap(0, Relaxed);
    if last == 0 {
        return;
    }
    tracing::info!(
        "console wait hints (LEONIDA_MWAITX_PATCH=1): monitorx and mwaitx exceptions {:.0} a second; sites patched {} (they run in \
         the title's code since, uncounted), left to the emulation {}, exceptions on patched sites {}",
        hints as f64 * 1e3 / (now - last) as f64,
        COUNTS[PATCHED].load(Relaxed),
        COUNTS[LEFT].load(Relaxed),
        COUNTS[STALE].load(Relaxed),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_become_what_their_emulation_does() {
        assert_eq!(replacement([0x0f, 0x01, 0xfa]), Some([0x0f, 0x1f, 0x00]));
        assert_eq!(replacement([0x0f, 0x01, 0xfb]), Some([0xf3, 0x90, 0x90]));
        assert_eq!(replacement([0x0f, 0x01, 0xfc]), None, "clzero is not a hint");
        assert!(is_replacement(&[0xf3, 0x90, 0x90, 0xcc]));
        assert!(is_replacement(&[0x0f, 0x1f, 0x00]));
        assert!(!is_replacement(&[0x0f, 0x01, 0xfb]));
        assert!(!is_replacement(&[0xf3, 0x90]));
    }

    #[test]
    fn the_word_holds_all_three_bytes() {
        assert_eq!(word_of(0x1000), Some((0x1000, 8)));
        assert_eq!(word_of(0x1005), Some((0x1000, 8)));
        assert_eq!(word_of(0x1006), Some((0x1000, 16)));
        assert_eq!(word_of(0x1007), Some((0x1000, 16)));
        assert_eq!(word_of(0x100d), Some((0x1008, 8)));
        assert_eq!(word_of(0x100e), None);
        assert_eq!(word_of(0x100f), None);
        assert_eq!(word_of(0x1016), Some((0x1010, 16)));
    }

    #[test]
    fn only_the_three_bytes_change() {
        let word = u128::from_le_bytes(std::array::from_fn(|i| i as u8));
        let now = splice(word, 4, [4, 5, 6], [0xaa, 0xbb, 0xcc]).expect("the bytes are there");
        let mut want: [u8; 16] = std::array::from_fn(|i| i as u8);
        want[4..7].copy_from_slice(&[0xaa, 0xbb, 0xcc]);
        assert_eq!(now.to_le_bytes(), want);
        assert_eq!(splice(word, 4, [4, 5, 7], [0; 3]), None);
    }

    #[test]
    fn locked_writes_of_8_and_16_bytes() {
        #[repr(align(16))]
        struct Block([u8; 32]);
        for at in [0usize, 3, 5, 6, 9, 13, 16, 21] {
            let mut block = Block([0x90; 32]);
            block.0[at..at + 3].copy_from_slice(&[0x0f, 0x01, 0xfb]);
            let base = block.0.as_mut_ptr() as usize;
            // SAFETY: our own block, 16-byte aligned, readable and writable.
            assert!(unsafe { exchange(base + at, [0x0f, 0x01, 0xfb], [0xf3, 0x90, 0x90]) }, "at {at}");
            let mut want = [0x90u8; 32];
            want[at] = 0xf3;
            assert_eq!(block.0, want, "at {at}");
            // Bytes that are not the old ones any more: nothing is written.
            // SAFETY: as above.
            assert!(!unsafe { exchange(base + at, [0x0f, 0x01, 0xfb], [0; 3]) });
            assert_eq!(block.0, want);
        }
        let mut block = Block([0x90; 32]);
        block.0[14..17].copy_from_slice(&[0x0f, 0x01, 0xfb]);
        // SAFETY: as above.
        assert!(!unsafe { exchange(block.0.as_mut_ptr() as usize + 14, [0x0f, 0x01, 0xfb], [0; 3]) });
    }

    /// A wait loop in executable memory of our own: patched in a read-only, executable page,
    /// its protection given back, and the patched code runs on any processor.
    #[test]
    fn a_site_is_patched_in_executable_code_and_runs() {
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, VirtualAlloc,
            VirtualFree, VirtualProtect, VirtualQuery,
        };
        // monitorx; mwaitx; ret
        let code = [0x0f, 0x01, 0xfa, 0x0f, 0x01, 0xfb, 0xc3];
        // SAFETY: a page of our own, written while writable, then made read-only and
        // executable; the patched code is two hints and a return, called without arguments.
        unsafe {
            let page = VirtualAlloc(std::ptr::null(), 4096, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as usize;
            assert_ne!(page, 0);
            std::ptr::copy_nonoverlapping(code.as_ptr(), page as *mut u8, code.len());
            let mut old = 0u32;
            assert_ne!(VirtualProtect(page as *const _, 4096, PAGE_EXECUTE_READ, &mut old), 0);
            assert!(site(page, [0x0f, 0x01, 0xfa]));
            assert!(site(page + 3, [0x0f, 0x01, 0xfb]));
            let bytes = std::slice::from_raw_parts(page as *const u8, code.len());
            assert_eq!(bytes, [0x0f, 0x1f, 0x00, 0xf3, 0x90, 0x90, 0xc3]);
            let mut m: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
            assert_ne!(VirtualQuery(page as *const _, &mut m, size_of_val(&m)), 0);
            assert_eq!(m.Protect, PAGE_EXECUTE_READ, "the page's protection is given back");
            let run: extern "C" fn() = std::mem::transmute(page);
            run();
            VirtualFree(page as *mut _, 0, MEM_RELEASE);
        }
    }
}
