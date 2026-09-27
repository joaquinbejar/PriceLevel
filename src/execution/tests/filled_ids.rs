//! Pre-release hardening: `MatchResult::validated`'s filled-id check is
//! hasher-free. Pins that the sort-based check returns exactly the outcome
//! (including which error, and its message) of the former `HashSet` walk.

#[cfg(test)]
// Scoped to this co-located test module only (issue #173): raw arithmetic and
// narrowing casts are permitted inside `mod tests` per the Testing section of
// `rules/global_rules.md`.
#[allow(clippy::arithmetic_side_effects, clippy::cast_possible_truncation)]
mod tests {
    use crate::errors::{CapacityResource, PriceLevelError};
    use crate::execution::match_result::check_filled_ids;
    use crate::orders::Id;
    use std::collections::HashSet;

    /// The former step-5 body, verbatim in behaviour (test-only reference).
    fn legacy(filled: &[Id], makers: &[Id]) -> Result<(), PriceLevelError> {
        if !filled.is_empty() {
            let mut seen = HashSet::new();
            seen.try_reserve(filled.len()).map_err(|_| {
                PriceLevelError::capacity_exceeded(
                    CapacityResource::ValidationScratch,
                    filled.len(),
                )
            })?;
            let mut makers = makers.iter().copied();
            for id in filled {
                if !seen.insert(*id) {
                    return Err(PriceLevelError::InvalidOperation {
                        message: format!("filled order id {id} appears more than once"),
                    });
                }
                if !makers.by_ref().any(|maker| maker == *id) {
                    return Err(PriceLevelError::InvalidOperation {
                        message: format!(
                            "filled order id {id} is not an in-order maker of the trades"
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    fn check(filled: &[Id], makers: &[Id]) {
        assert_eq!(
            check_filled_ids(filled, makers.iter().copied()),
            legacy(filled, makers),
            "filled {filled:?} makers {makers:?}"
        );
    }

    #[test]
    fn precedence_duplicate_vs_subsequence() {
        let id = Id::from_u64;
        // Subsequence failure (index 1) precedes the repeat (index 2).
        check(&[id(1), id(2), id(1)], &[id(1), id(3)]);
        // Repeat (index 1) precedes the subsequence failure (index 2).
        check(&[id(1), id(1), id(9)], &[id(1), id(1)]);
        // Valid.
        check(&[id(1), id(3)], &[id(1), id(2), id(3)]);
        check(&[], &[id(1)]);
    }

    #[test]
    fn matches_legacy_on_random_inputs() {
        let mut state: u64 = 0xD1B5_4A32_D192_ED03;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..5_000 {
            let alphabet = 1 + next() % 8;
            let makers: Vec<Id> = (0..next() % 16)
                .map(|_| Id::from_u64(next() % alphabet))
                .collect();
            // Mostly subsequences of the makers (so the duplicate branch is
            // exercised past index 0), sometimes arbitrary ids.
            let filled: Vec<Id> = if next() % 3 == 0 {
                (0..next() % 10)
                    .map(|_| Id::from_u64(next() % alphabet))
                    .collect()
            } else {
                makers.iter().copied().filter(|_| next() % 2 == 0).collect()
            };
            check(&filled, &makers);
        }
    }
}
