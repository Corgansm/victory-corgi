//! Our own blocks come from a heap of the thread that asks, not from the
//! process's one heap (`VICTORY_OWN_HEAPS=0`: the process heap, as before).
//!
//! Rust's allocator on Windows is the process heap, and that heap has one
//! lock. Nine threads of ours make and grow blocks all through a frame (the
//! eight that record the title's lists and the one that submits them), beside
//! the title's own, and they met at that lock: in the profile of run
//! 20261127-144318 (the boat, 37 ms a frame) the submitting thread stood in
//! `RtlEnterCriticalSection` under `realloc` and `alloc` for 4 % of the frame
//! and spent 12 % of it in the heap all told, and it is the thread every
//! frame waits for. A heap of its own has a lock of its own, which only a
//! block of this thread released by another thread ever meets.
//!
//! Each block has 16 bytes ahead of it that say which heap it is from, so any
//! thread can give it back there.

use std::alloc::Layout;
use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, AtomicU8, AtomicUsize, Ordering::Relaxed};

use windows_sys::Win32::System::Memory::{HEAP_ZERO_MEMORY, HeapAlloc, HeapCreate, HeapDestroy, HeapFree, HeapReAlloc};

/// The heaps: a thread takes the next one when it first asks, and more
/// threads than heaps share.
const HEAPS: usize = 64;
/// What the system's heap aligns blocks to, and the bytes we keep ahead of
/// each of ours.
const HEAD: usize = 16;

static MADE: [AtomicPtr<c_void>; HEAPS] = [const { AtomicPtr::new(std::ptr::null_mut()) }; HEAPS];

/// Whether blocks come from these heaps: 0 not asked yet, 1 yes, 2 no. Asked
/// once, at the first block of the process, since a block must go back the
/// way it came.
static ON: AtomicU8 = AtomicU8::new(0);

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetEnvironmentVariableW(name: *const u16, buffer: *mut u16, size: u32) -> u32;
}

/// Whether blocks come from these heaps.
#[inline]
pub(crate) fn on() -> bool {
    match ON.load(Relaxed) {
        1 => true,
        2 => false,
        _ => ask(),
    }
}

#[cold]
fn ask() -> bool {
    // (The allocator asks: nothing here may allocate, so not `std::env`.)
    const NAME: [u16; 18] = {
        let text = b"VICTORY_OWN_HEAPS\0";
        let mut wide = [0u16; 18];
        let mut i = 0;
        while i < text.len() {
            wide[i] = text[i] as u16;
            i += 1;
        }
        wide
    };
    let mut value = [0u16; 8];
    // SAFETY: a terminated name and a buffer of the length passed.
    let n = unsafe { GetEnvironmentVariableW(NAME.as_ptr(), value.as_mut_ptr(), value.len() as u32) };
    let off = n == 1 && value[0] == u16::from(b'0');
    // Two threads asking at once answer alike.
    ON.store(if off { 2 } else { 1 }, Relaxed);
    !off
}

/// This thread's heap, made if it is the first to want it.
#[inline]
fn mine() -> (usize, *mut c_void) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    thread_local!(static MINE: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) });
    // (A thread on its way out, whose locals are gone, shares the first.)
    let index = MINE
        .try_with(|mine| {
            if mine.get() == usize::MAX {
                mine.set(NEXT.fetch_add(1, Relaxed) % HEAPS);
            }
            mine.get()
        })
        .unwrap_or(0);
    let heap = MADE[index].load(Relaxed);
    if !heap.is_null() {
        return (index, heap);
    }
    (index, make(index))
}

#[cold]
fn make(index: usize) -> *mut c_void {
    // SAFETY: a heap of the process that grows as needed, with its own lock.
    let new = unsafe { HeapCreate(0, 0, 0) };
    if new.is_null() {
        return new;
    }
    match MADE[index].compare_exchange(std::ptr::null_mut(), new, Relaxed, Relaxed) {
        Ok(_) => new,
        Err(theirs) => {
            // SAFETY: ours, and nothing was taken from it.
            unsafe { HeapDestroy(new) };
            theirs
        }
    }
}

/// What stands ahead of a block: the heap it is from, and how far ahead of it
/// the heap's own block begins.
#[repr(C)]
struct Head {
    heap: u32,
    back: u32,
}

/// # Safety
/// `at` is a block of ours.
#[inline]
unsafe fn head(at: *mut u8) -> *mut Head {
    // SAFETY: the 8 bytes ahead of every block of ours are its head.
    unsafe { at.sub(size_of::<Head>()).cast() }
}

/// A block of `layout`, zeroed if asked; null if there is no memory.
///
/// # Safety
/// As `GlobalAlloc::alloc`.
#[inline]
pub(crate) unsafe fn alloc(layout: Layout, zeroed: bool) -> *mut u8 {
    let (index, heap) = mine();
    if heap.is_null() {
        return std::ptr::null_mut();
    }
    let flags = if zeroed { HEAP_ZERO_MEMORY } else { 0 };
    let align = layout.align();
    // The heap's own blocks are aligned to `HEAD`: the head fills that much
    // and the block follows. A stricter alignment takes its room as well.
    let room = if align <= HEAD { HEAD } else { HEAD + align };
    let Some(bytes) = layout.size().checked_add(room) else {
        return std::ptr::null_mut();
    };
    // SAFETY: a heap of ours.
    let base = unsafe { HeapAlloc(heap, flags, bytes) }.cast::<u8>();
    if base.is_null() {
        return base;
    }
    let back = if align <= HEAD { HEAD } else { (base as usize + HEAD).next_multiple_of(align) - base as usize };
    // SAFETY: `back` is at least `HEAD` and at most `room`, so the head and
    // the block lie inside what the heap gave.
    unsafe {
        let at = base.add(back);
        head(at).write(Head { heap: index as u32, back: back as u32 });
        at
    }
}

/// # Safety
/// As `GlobalAlloc::dealloc`, for a block of [`alloc`] or [`realloc`].
#[inline]
pub(crate) unsafe fn dealloc(at: *mut u8) {
    // SAFETY: a block of ours has its head ahead of it, and its heap lives
    // as long as the process.
    unsafe {
        let Head { heap, back } = head(at).read();
        HeapFree(MADE[heap as usize % HEAPS].load(Relaxed), 0, at.sub(back as usize).cast());
    }
}

/// # Safety
/// As `GlobalAlloc::realloc`, for a block of [`alloc`] or [`realloc`].
#[inline]
pub(crate) unsafe fn realloc(at: *mut u8, layout: Layout, size: usize) -> *mut u8 {
    if layout.align() > HEAD {
        // The heap moves a block without regard to where it began in it.
        // SAFETY: the caller's layout with the new size is one too.
        let new = unsafe { alloc(Layout::from_size_align_unchecked(size, layout.align()), false) };
        if !new.is_null() {
            // SAFETY: both blocks hold at least the smaller size, and are apart.
            unsafe {
                std::ptr::copy_nonoverlapping(at, new, layout.size().min(size));
                dealloc(at);
            }
        }
        return new;
    }
    let Some(bytes) = size.checked_add(HEAD) else {
        return std::ptr::null_mut();
    };
    // SAFETY: a block of ours of this alignment begins `HEAD` after the
    // heap's; the heap keeps the bytes, the head among them, where it moves it.
    unsafe {
        let heap = head(at).read().heap;
        let base = HeapReAlloc(MADE[heap as usize % HEAPS].load(Relaxed), 0, at.sub(HEAD).cast(), bytes).cast::<u8>();
        if base.is_null() { base } else { base.add(HEAD) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(at: *mut u8, n: usize, seed: u8) {
        for i in 0..n {
            // SAFETY: the test's own block of `n` bytes.
            unsafe { at.add(i).write(seed.wrapping_add(i as u8)) };
        }
    }

    fn holds(at: *mut u8, n: usize, seed: u8) -> bool {
        // SAFETY: as above.
        (0..n).all(|i| unsafe { at.add(i).read() } == seed.wrapping_add(i as u8))
    }

    /// Blocks of every alignment are aligned, hold their bytes through a
    /// growth and a shrinking, and come back zeroed when asked.
    #[test]
    fn blocks_are_aligned_and_keep_their_bytes() {
        for align in [1usize, 2, 8, 16, 32, 64, 4096] {
            for size in [1usize, 15, 16, 17, 255, 4096, 70_000, 600_000] {
                let layout = Layout::from_size_align(size, align).unwrap();
                // SAFETY: a layout with a size, and the block is released below.
                unsafe {
                    let at = alloc(layout, false);
                    assert!(!at.is_null());
                    assert_eq!(at as usize % align, 0, "align {align} size {size}");
                    filled(at, size, 7);
                    let more = realloc(at, layout, size * 3 + 5);
                    assert_eq!(more as usize % align, 0);
                    assert!(holds(more, size, 7), "grown: align {align} size {size}");
                    let grown = Layout::from_size_align(size * 3 + 5, align).unwrap();
                    let less = realloc(more, grown, size.div_ceil(2));
                    assert_eq!(less as usize % align, 0);
                    assert!(holds(less, size.div_ceil(2), 7), "shrunk: align {align} size {size}");
                    dealloc(less);
                    let zero = alloc(layout, true);
                    assert!((0..size).all(|i| zero.add(i).read() == 0));
                    dealloc(zero);
                }
            }
        }
    }

    /// A block goes back to the heap it came from, whichever thread gives it
    /// back, and each thread has a heap.
    #[test]
    fn a_block_is_released_and_grown_by_another_thread() {
        let layout = Layout::from_size_align(1000, 8).unwrap();
        let made: Vec<(usize, u32)> = (0..8)
            .map(|k| {
                std::thread::spawn(move || {
                    // SAFETY: released or grown by the test below.
                    let at = unsafe { alloc(layout, false) };
                    filled(at, 1000, k as u8);
                    // SAFETY: a block of ours.
                    (at as usize, unsafe { head(at).read().heap })
                })
                .join()
                .unwrap()
            })
            .collect();
        let heaps: std::collections::HashSet<u32> = made.iter().map(|m| m.1).collect();
        assert!(heaps.len() > 1, "{heaps:?}");
        for (k, (at, heap)) in made.into_iter().enumerate() {
            let at = at as *mut u8;
            assert!(holds(at, 1000, k as u8));
            // SAFETY: blocks of ours, made above.
            unsafe {
                let more = realloc(at, layout, 300_000);
                assert!(holds(more, 1000, k as u8));
                assert_eq!(head(more).read().heap, heap);
                dealloc(more);
            }
        }
    }

    /// Many threads making, growing and releasing each other's blocks at once
    /// leave every block's bytes as they were written.
    #[test]
    fn threads_trade_blocks() {
        let (tx, rx) = std::sync::mpsc::channel::<(usize, usize, u8)>();
        let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
        let takers: Vec<_> = (0..4)
            .map(|_| {
                let rx = rx.clone();
                std::thread::spawn(move || {
                    let mut seen = 0;
                    loop {
                        let got = rx.lock().unwrap().recv();
                        let Ok((at, n, seed)) = got else { break seen };
                        assert!(holds(at as *mut u8, n, seed));
                        // SAFETY: a block of ours of 8-byte alignment, sent whole.
                        unsafe {
                            let layout = Layout::from_size_align(n, 8).unwrap();
                            let more = realloc(at as *mut u8, layout, n * 2);
                            assert!(holds(more, n, seed));
                            dealloc(more);
                        }
                        seen += 1;
                    }
                })
            })
            .collect();
        let makers: Vec<_> = (0..4u8)
            .map(|k| {
                let tx = tx.clone();
                std::thread::spawn(move || {
                    for i in 0..5000usize {
                        let n = 1 + (i * 37 + usize::from(k) * 101) % 9000;
                        // SAFETY: released by a taker.
                        let at = unsafe { alloc(Layout::from_size_align(n, 8).unwrap(), false) };
                        filled(at, n, k.wrapping_add(i as u8));
                        tx.send((at as usize, n, k.wrapping_add(i as u8))).unwrap();
                    }
                })
            })
            .collect();
        drop(tx);
        makers.into_iter().for_each(|m| m.join().unwrap());
        assert_eq!(takers.into_iter().map(|t| t.join().unwrap()).sum::<usize>(), 20_000);
    }
}
