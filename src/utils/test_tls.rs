//! Non-panicking thread-local access for `cfg(test)` seams compiled into
//! production functions (pre-release hardening).
//!
//! The test hooks, injection plans and probes that production code consults
//! under `cfg(test)` live in `thread_local!` cells. `LocalKey::with` panics
//! when the key is accessed during or after its destruction, and
//! `RefCell::borrow` / `borrow_mut` panic on a conflicting borrow (a hook that
//! re-enters the seam). Those calls sit inside production functions, so the
//! Production Panic Policy applies to them. Every helper here uses
//! `LocalKey::try_with` plus `RefCell::try_borrow(_mut)` and treats a failure
//! as "no hook / not armed / nothing recorded", the pattern the update
//! decision hook already followed.

use std::cell::{Cell, RefCell};
use std::thread::LocalKey;

/// A `Cell`'s value, or `fallback` when the key is unavailable.
#[inline]
pub(crate) fn cell_get<T: Copy>(key: &'static LocalKey<Cell<T>>, fallback: T) -> T {
    key.try_with(Cell::get).unwrap_or(fallback)
}

/// Sets a `Cell`; a no-op when the key is unavailable.
#[inline]
pub(crate) fn cell_set<T>(key: &'static LocalKey<Cell<T>>, value: T) {
    let _ = key.try_with(|cell| cell.set(value));
}

/// Replaces a `Cell`'s value and returns the previous one, or `fallback` when
/// the key is unavailable (nothing is stored then).
#[inline]
pub(crate) fn cell_replace<T: Copy>(key: &'static LocalKey<Cell<T>>, value: T, fallback: T) -> T {
    key.try_with(|cell| cell.replace(value)).unwrap_or(fallback)
}

/// Overwrites an optional slot; a no-op when the key is unavailable or the
/// slot is borrowed (re-entry).
#[inline]
pub(crate) fn slot_set<T>(key: &'static LocalKey<RefCell<Option<T>>>, value: Option<T>) {
    let _ = key.try_with(|slot| {
        if let Ok(mut slot) = slot.try_borrow_mut() {
            *slot = value;
        }
    });
}

/// Takes the value out of an optional slot; `None` when the key is
/// unavailable or the slot is borrowed.
#[inline]
pub(crate) fn slot_take<T>(key: &'static LocalKey<RefCell<Option<T>>>) -> Option<T> {
    key.try_with(|slot| slot.try_borrow_mut().ok().and_then(|mut slot| slot.take()))
        .ok()
        .flatten()
}

/// Puts `value` back into an optional slot if the slot is still empty (a
/// hook fired meanwhile may have installed a replacement); a no-op when the
/// key is unavailable or the slot is borrowed.
#[inline]
pub(crate) fn slot_restore<T>(key: &'static LocalKey<RefCell<Option<T>>>, value: T) {
    let _ = key.try_with(|slot| {
        if let Ok(mut slot) = slot.try_borrow_mut()
            && slot.is_none()
        {
            *slot = Some(value);
        }
    });
}

/// A clone of the value in an optional slot; `None` when the key is
/// unavailable or the slot is mutably borrowed.
#[inline]
pub(crate) fn slot_clone<T: Clone>(key: &'static LocalKey<RefCell<Option<T>>>) -> Option<T> {
    key.try_with(|slot| slot.try_borrow().ok().and_then(|slot| slot.clone()))
        .ok()
        .flatten()
}
