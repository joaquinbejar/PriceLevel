//! Fallible collection growth (issue #164).
//!
//! Every owned collection-growth site of the engine and of the snapshot /
//! serialization paths reserves through these helpers instead of an
//! infallible `push` / `collect` / `with_capacity`. A refused reservation is
//! reported as the fixed-size [`PriceLevelError::CapacityExceeded`]: the
//! [`std::collections::TryReserveError`] is dropped and the typed error is
//! built from a `Copy` resource tag and a `usize`, so reporting an allocation
//! failure never allocates.
//!
//! Every helper leaves its collection unchanged on failure (the standard
//! `try_reserve*` contract), so a caller that reserves before mutating keeps
//! its own state untouched.
//!
//! What these helpers cannot cover is documented in
//! `doc/panic-boundaries.md` ("Allocation limits"): `DashMap` / `SkipMap`
//! node insertion and `Arc::new` have no stable fallible API, and an
//! allocator abort is a process-wide failure, not a Rust panic.
//!
//! Tests inject failures through the `cfg(test)`-only `test_seam`; there is
//! no production-visible knob.

use crate::errors::{CapacityResource, PriceLevelError};
use std::collections::HashSet;
use std::hash::{BuildHasher, Hash};

/// Builds the typed, allocation-free capacity error.
#[cold]
#[inline(never)]
#[must_use]
pub(crate) fn capacity_error(resource: CapacityResource, additional: usize) -> PriceLevelError {
    PriceLevelError::capacity_exceeded(resource, additional)
}

/// Reserves room for `additional` more elements (amortized growth).
///
/// # Errors
///
/// [`PriceLevelError::CapacityExceeded`] (`resource`, `additional`) on
/// capacity overflow or allocation failure; `vec` is unchanged.
#[inline]
pub(crate) fn try_reserve_vec<T>(
    vec: &mut Vec<T>,
    additional: usize,
    resource: CapacityResource,
) -> Result<(), PriceLevelError> {
    #[cfg(test)]
    test_seam::check(resource, additional)?;
    vec.try_reserve(additional)
        .map_err(|_| capacity_error(resource, additional))
}

/// Reserves room for exactly `additional` more elements.
///
/// # Errors
///
/// As [`try_reserve_vec`]; `vec` is unchanged.
#[inline]
pub(crate) fn try_reserve_exact_vec<T>(
    vec: &mut Vec<T>,
    additional: usize,
    resource: CapacityResource,
) -> Result<(), PriceLevelError> {
    #[cfg(test)]
    test_seam::check(resource, additional)?;
    vec.try_reserve_exact(additional)
        .map_err(|_| capacity_error(resource, additional))
}

/// Appends `item`, growing fallibly only when `vec` is full. With spare
/// capacity this is a length / capacity compare and a plain `push`.
///
/// # Errors
///
/// As [`try_reserve_vec`] with `additional == 1`; `vec` is unchanged and
/// `item` is dropped.
#[inline]
pub(crate) fn try_push_vec<T>(
    vec: &mut Vec<T>,
    item: T,
    resource: CapacityResource,
) -> Result<(), PriceLevelError> {
    if vec.len() == vec.capacity() {
        try_reserve_vec(vec, 1, resource)?;
    }
    vec.push(item);
    Ok(())
}

/// Reserves room for `additional` more elements in a hash set, so the next
/// `additional` insertions of new keys do not reallocate.
///
/// # Errors
///
/// As [`try_reserve_vec`]; `set` is unchanged.
#[inline]
pub(crate) fn try_reserve_set<K, S>(
    set: &mut HashSet<K, S>,
    additional: usize,
    resource: CapacityResource,
) -> Result<(), PriceLevelError>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    #[cfg(test)]
    test_seam::check(resource, additional)?;
    set.try_reserve(additional)
        .map_err(|_| capacity_error(resource, additional))
}

/// Reserves room for exactly `additional` more bytes in `s`.
///
/// # Errors
///
/// As [`try_reserve_vec`]; `s` is unchanged.
#[inline]
pub(crate) fn try_reserve_string(
    s: &mut String,
    additional: usize,
    resource: CapacityResource,
) -> Result<(), PriceLevelError> {
    #[cfg(test)]
    test_seam::check(resource, additional)?;
    s.try_reserve_exact(additional)
        .map_err(|_| capacity_error(resource, additional))
}

/// Copies `text` into a new, exactly sized `String` through the fallible
/// allocator API.
///
/// # Errors
///
/// As [`try_reserve_string`].
#[inline]
pub(crate) fn try_copy_str(
    text: &str,
    resource: CapacityResource,
) -> Result<String, PriceLevelError> {
    let mut out = String::new();
    try_reserve_string(&mut out, text.len(), resource)?;
    out.push_str(text);
    Ok(out)
}

/// An [`std::io::Write`] sink over a `Vec<u8>` whose every growth goes
/// through [`try_reserve_vec`] (resource
/// [`CapacityResource::SerializationBuffer`]).
///
/// A refused reservation records the byte count that could not be reserved
/// and returns an allocation-free `io::Error` built from
/// [`std::io::ErrorKind::OutOfMemory`]; the buffer is left as it was before
/// the refused write. The caller reads [`FallibleWriter::failure`] after the
/// serializer returns and reports the typed
/// [`PriceLevelError::CapacityExceeded`] instead of the serializer's own
/// error (which the dependency may have allocated to wrap the `io::Error`;
/// that allocation is outside this crate, see `doc/panic-boundaries.md`).
#[derive(Debug, Default)]
pub(crate) struct FallibleWriter {
    buf: Vec<u8>,
    failure: Option<PriceLevelError>,
}

impl FallibleWriter {
    /// An empty writer; allocates nothing until the first write.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The first reservation failure, if any write was refused.
    #[must_use]
    pub(crate) fn take_failure(&mut self) -> Option<PriceLevelError> {
        self.failure.take()
    }

    /// The bytes written so far.
    #[must_use]
    pub(crate) fn into_inner(self) -> Vec<u8> {
        self.buf
    }
}

impl std::io::Write for FallibleWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let needs_growth = self
            .buf
            .capacity()
            .checked_sub(self.buf.len())
            .is_none_or(|spare| bytes.len() > spare);
        if needs_growth
            && let Err(err) = try_reserve_vec(
                &mut self.buf,
                bytes.len(),
                CapacityResource::SerializationBuffer,
            )
        {
            if self.failure.is_none() {
                self.failure = Some(err);
            }
            return Err(std::io::Error::from(std::io::ErrorKind::OutOfMemory));
        }
        self.buf.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Test-only reservation-failure injection (issue #164).
///
/// A thread-local budget consulted by every helper above before it calls the
/// real `try_reserve*`: while armed for a resource, the first `allowed`
/// non-empty reservations of that resource on this thread succeed and every
/// later one fails with the same typed
/// [`PriceLevelError::CapacityExceeded`] a real allocator refusal produces.
/// Zero-element reservations never fail (they never allocate). Compiled only
/// under `cfg(test)`; there is no production-visible knob.
#[cfg(test)]
pub(crate) mod test_seam {
    use crate::errors::{CapacityResource, PriceLevelError};
    use std::cell::Cell;

    #[derive(Clone, Copy)]
    struct Plan {
        resource: CapacityResource,
        allowed: usize,
    }

    thread_local! {
        static PLAN: Cell<Option<Plan>> = const { Cell::new(None) };
        static INJECTED: Cell<usize> = const { Cell::new(0) };
    }

    /// Restores the previous plan on drop.
    pub(crate) struct FailGuard(Option<Plan>);

    impl Drop for FailGuard {
        fn drop(&mut self) {
            PLAN.with(|cell| cell.set(self.0));
        }
    }

    /// Lets the next `allowed` reservations of `resource` on this thread
    /// succeed and fails every later one, until the guard drops.
    pub(crate) fn fail_after(resource: CapacityResource, allowed: usize) -> FailGuard {
        INJECTED.with(|cell| cell.set(0));
        FailGuard(PLAN.with(|cell| cell.replace(Some(Plan { resource, allowed }))))
    }

    /// Number of failures injected on this thread since the last
    /// [`fail_after`].
    pub(crate) fn injected() -> usize {
        INJECTED.with(Cell::get)
    }

    pub(super) fn check(
        resource: CapacityResource,
        additional: usize,
    ) -> Result<(), PriceLevelError> {
        if additional == 0 {
            return Ok(());
        }
        let Some(plan) = PLAN.with(Cell::get) else {
            return Ok(());
        };
        if plan.resource != resource {
            return Ok(());
        }
        match plan.allowed.checked_sub(1) {
            Some(left) => {
                PLAN.with(|cell| {
                    cell.set(Some(Plan {
                        resource,
                        allowed: left,
                    }));
                });
                Ok(())
            }
            None => {
                INJECTED.with(|cell| cell.set(cell.get().wrapping_add(1)));
                Err(PriceLevelError::capacity_exceeded(resource, additional))
            }
        }
    }
}
