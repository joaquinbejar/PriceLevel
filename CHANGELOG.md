# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.9.1] - 2026-07-14

### Fixed

- **`MatchResult` round-trips non-self-describing serde formats again
  (#135).** 0.9.0's decode-time validation (#117) deserializes through a wire
  struct whose `outcome` is `Option<MatchOutcome>`, but `Serialize` still
  emitted a bare `MatchOutcome`. JSON tolerated the asymmetry; a positional
  decoder (bincode) read the enum variant index where the option tag was
  expected and failed with `UnexpectedVariant` on every 0.9.0 payload.
  `outcome` is now serialized as `Some(outcome)`: the JSON payload is
  byte-identical (serde flattens `Some`), and bincode encode/decode are
  symmetric. Bincode payloads written by 0.9.0 do not decode (they never
  did — 0.9.0 could not decode its own output); JSON payloads from any
  version are unaffected. Pinned by a bincode leg on the round-trip
  property test plus deterministic shape guards (`bincode` added as a
  dev-dependency only).

## [0.9.0] - 2026-07-14

Major hardening release: ten engine-correctness issues (#111–#120, PRs
#121–#130) plus five adversarial review rounds. Every fix ships with
regression tests; the full suite grew from ~440 to 526 tests.

### Changed (breaking)

- **`PriceLevel::add_order` returns `Result<Arc<OrderType<()>>, PriceLevelError>`.**
  Admission validates before mutating: the order's own visible + hidden total,
  the level's counters (checked CAS reservations), price and side topology,
  and id uniqueness — a rejected admission leaves the level byte-identical
  (#111, #113, #120). A duplicate id reports the new
  `PriceLevelError::DuplicateOrderId` and takes precedence over a
  counter-capacity error.
- **`PriceLevel::matchable_quantity(quantity, taker_id)`** — the fill-or-kill
  dry run applies the same self-match skip as the sweep so the two can never
  diverge (#120).
- **`OrderQueue::push` and `OrderQueue::from_vec` are no longer public**, and
  **`impl From<&PriceLevelSnapshot> for PriceLevel` is replaced by `TryFrom`**
  delegating to the validating `from_snapshot` — no public path can overwrite
  a live id or restore counters over silently-dropped duplicates (#113 +
  reviews).
- **Snapshot format v3.** Statistics carry a sticky `stats_degraded` flag
  (serialized only when `true`); v3 packages are written, v2 packages are
  still accepted on read (legacy 8-field statistics), v1 remains rejected
  (#117 + review). Pre-0.9 snapshots that captured a queue-priority demotion
  restore with the old (wrong) front priority — re-snapshot to pin the
  corrected order.
- **Concurrency contract wording:** the Gtc/Ioc/Day match path is lock-free;
  `add_order` / `update_order` (cancel included) are shared-lock mutators —
  normally uncontended, but they can block behind an O(depth) fill-or-kill
  writer (#112).

### Fixed

- Partial fills resize every matchable order variant (TrailingStop, Pegged,
  MarketToLimit previously kept their original size and could double-execute)
  (#118).
- `MatchResult::from_str` never panics on malformed UTF-8, and both decoders
  route through one invariant validator (outcome consistency, taker identity,
  filled-id ordering, checked sums) (#114, #116).
- Counter overflow is rejected before any state mutates, including reserve
  replenishment (which can no longer bypass FIFO or wrap the level counter —
  the sweep aborts in front of a replenishment the level cannot represent)
  (#111 + review).
- Duplicate-id admission is atomic (identity decided first, publication under
  one held entry lock), and the index re-key is new-before-old with a
  sequence-validated destructive pop — a demoted maker can neither vanish
  from a front scan nor drain out of FIFO order (#113, #119 + reviews).
- Level topology is pinned in a single side+count atomic word (opposite-side
  admissions into an empty level serialize on one CAS), snapshots retry on a
  topology epoch so they never capture a torn side transition, and a
  self-match attempt is rejected terminally with zero trades (#120 + review).
- Quantity updates derive from the live maker under the entry lock (no
  resurrection of executed quantity, no stale priority policy), with replenish
  counter transitions published inside the lock (#115, #119 + reviews).
- Execution statistics record all-or-nothing behind a seqlock (consistent
  clones/serialization, enforced reset serialization, checked aggregates,
  monotonic last-execution timestamp), and a dropped recording is observable
  via `stats_degraded` with a single WARN on transition (#117 + review).
- PostOnly can never trade (structural early return, linearized depth verdict
  via a mutation epoch) and FOK is all-or-nothing under every interleaving
  (level guard across an exact feasibility projection and the sweep; poisoned
  guards fail fast instead of reopening a half-mutated level) (#112 + review).
- The plain `PriceLevelData` serde round-trip preserves FIFO order (#131).

## [0.8.5] - 2026-07-14

Patch release: a **bug fix** to the snapshot round-trip's queue-priority
preservation. The snapshot JSON *shape* is unchanged, but the `orders` array
order — and therefore the package checksum — changes for levels whose
consumption order diverges from timestamp order (a demoted or
non-monotonically-timestamped maker).

### Fixed

- **A snapshot round-trip no longer undoes a queue-priority demotion.**
  `PriceLevel::snapshot()` sorted its orders by `(timestamp, sequence)` and
  did not serialize the insertion sequence, while `from_snapshot` re-enqueues
  in vector order. An order demoted to the back of the queue with its original
  admission timestamp intact — a quantity *increase* via `update_order`, or an
  iceberg / reserve replenishment — therefore sorted back to its old timestamp
  position on restore and wrongly regained front priority. The snapshot now
  materializes orders in **queue-consumption order** (ascending insertion
  sequence, exactly as `match_order` sweeps), so a restore reproduces the live
  queue's price-time priority in all cases, demotions included. Found via the
  queue-priority contract review in joaquinbejar/OrderBook-rs#204 (#109).

### Notes

- `PriceLevelSnapshot::orders()` / `iter_orders()` / `into_orders()` now
  yield consumption order, not timestamp order. A consumer that wants
  admission-time order should sort by `order.timestamp()` itself (or read
  `PriceLevel::snapshot_orders()` on a live level, which keeps the
  `(timestamp, sequence)` view).
- A snapshot serialized by ≤ 0.8.4 that captured a demotion restores with the
  old (wrong) front priority — the fix cannot repair data already persisted
  in timestamp order. Re-snapshot with 0.8.5 to pin the correct order.

## [0.8.4] - 2026-07-10

Patch release: a **documentation fix** to `TimeInForce::Gtd`'s payload unit. No
API, behavior, or wire-format change.

### Fixed

- **`TimeInForce::Gtd`'s doc no longer claims the payload is seconds — it is
  Unix MILLISECONDS since the epoch.** Every other timestamp in this crate is
  milliseconds (`TimestampMs`, trade timestamps, statistics), this crate's own
  tests always used 13-digit millisecond epochs for GTD, and `orderbook-rs`
  compares the payload against `Clock::now_millis`. A caller following the old
  doc and passing seconds got orders that appeared expired immediately. The
  contract is now pinned by the `gtd_payload_unit_is_milliseconds` test
  (found via joaquinbejar/OrderBook-rs#187).

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
