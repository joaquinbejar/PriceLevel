# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.8.3] - 2026-06-25

Patch release: a **performance fix** to `match_order`'s transient allocation. No
API, behavior, or wire-format change.

### Fixed

- **`PriceLevel::match_order` no longer pre-allocates the `MatchResult` buffers
  to the whole level depth.** It reserved `trades` / `filled_order_ids` for
  `order_count()` entries on every match, so a qty-1 taker against a deep level
  reserved a multi-MB buffer (~176 B × depth) it immediately freed — pure
  allocator pressure, not a leak. The pre-size is now bounded by
  `min(incoming_quantity, order_count)`, a tight upper bound on the number of
  fills (each trade consumes ≥1 unit of the taker). A qty-1 match against a
  100k-deep level now allocates KB, not MB; large sweeps are unaffected. Matching
  semantics, FIFO/price-time priority, and trade output are unchanged.

## [0.8.2] - 2026-06-24

Small, **non-breaking** release exposing two primitives an order book needs to
compose this level without re-deriving its internal sweep order.

### Added

- **`PriceLevel::matchable_quantity(incoming_quantity: u64) -> u64`** is now
  `pub`. It was already the deterministic fill-or-kill dry run — a no-mutation
  replay of the FIFO sweep (including iceberg / reserve replenishment) that
  returns exactly what `match_order` would consume. Making it public lets a
  composing order book delegate per-level all-or-nothing feasibility to this
  single upstream source of truth instead of re-implementing the sweep and
  risking drift.
- **`PriceLevel::snapshot_by_seq_into(&self, out: &mut Vec<Arc<OrderType<()>>>)`**
  — buffer-reuse variant of `snapshot_by_insertion_seq()`. Clears `out` and
  refills it in ascending insertion sequence (the order `match_order` consumes
  orders), so a consumer that walks every level repeatedly (e.g. a
  self-trade-prevention pre-scan) can reuse one pooled scratch buffer and avoid
  the per-call allocation the owned-`Vec` variant pays.

No breaking changes, no new dependencies, no change to matching semantics or the
snapshot wire format.

## [0.8.1] - 2026-06-24

### Added

- **`PriceLevel::snapshot_by_insertion_seq() -> Vec<Arc<OrderType<()>>>`** —
  returns the resting orders in ascending insertion sequence, i.e. the exact
  order `match_order` consumes them. Unlike `snapshot_orders()` (sorted by
  `(timestamp, sequence)`, which equals the sweep only when timestamps are
  monotonic with insertion) and `iter_orders()` (no stable order), this faithfully
  predicts the sweep, giving a downstream consumer a public primitive to walk
  orders in consumption order.

No breaking changes, no new dependencies, snapshot wire format unchanged.

## [0.8.0] - 2026-06-23

Roadmap hardening release: a sweep of correctness, robustness, and tooling
improvements across the price level. Highlights:

### Added

- Taker time-in-force handling inside `match_order` (`Gtc` / `Ioc` / `Fok` /
  `Gtd` / `Day`), with a `TakerKind` (`Standard` / `PostOnly` / `MarketToLimit`)
  and a `MatchOutcome` (`Filled` / `PartiallyFilled` / `NotFilled` / `Killed` /
  `Rejected`).
- A property-test harness (`proptest`) covering the nine price-level invariants.
- A `loom` linearization model for the cancel-vs-partial-fill protocol.

### Changed

- Unified order matchability behind `OrderType::is_matchable`, shared by the
  post-only pre-check and the fill-or-kill dry run so they can never disagree.
- Bumped `sha2` to `0.11`.

### Fixed

- A zero-visible iceberg / auto-replenishing reserve backed by hidden quantity is
  now correctly treated as matchable depth, closing a fill-or-kill no-progress
  loop.
- Lost-cancel race on the order queue.

[0.8.3]: https://github.com/joaquinbejar/PriceLevel/compare/v0.8.2...v0.8.3
[0.8.2]: https://github.com/joaquinbejar/PriceLevel/compare/v0.8.1...v0.8.2
[0.8.1]: https://github.com/joaquinbejar/PriceLevel/compare/v0.8.0...v0.8.1
[0.8.0]: https://github.com/joaquinbejar/PriceLevel/releases/tag/v0.8.0
