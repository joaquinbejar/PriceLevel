//! Issue #150: `PriceLevel::from_snapshot` validates the orders in two walks
//! (aggregates, then ids and topology fused). These tests pin that it is observably identical to the pre-#150
//! three-walk restore (`PriceLevel::from_snapshot_legacy`, test-only): same
//! restored level for every valid snapshot, and the same error, with the same
//! precedence, for every invalid one.

#[cfg(test)]
mod tests {
    use crate::UuidGenerator;
    use crate::errors::{CapacityResource, PriceLevelError};
    use crate::execution::TakerKind;
    use crate::orders::{Hash32, Id, OrderType, PegReferenceType, Side, TimeInForce};
    use crate::price_level::level::PriceLevel;
    use crate::price_level::{PriceLevelSnapshot, PriceLevelSnapshotPackage, PriceLevelStatistics};
    use crate::utils::alloc::test_seam;
    use crate::utils::{Price, Quantity, TimestampMs};
    use std::num::NonZeroU64;
    use std::sync::Arc;
    use uuid::Uuid;

    const PRICE: u128 = 10_000;

    /// A compact order description the matrix mutates.
    #[derive(Debug, Clone, Copy)]
    struct Spec {
        id: u64,
        price: u128,
        side: Side,
        visible: u64,
        hidden: u64,
    }

    impl Spec {
        fn valid(id: u64) -> Self {
            Self {
                id,
                price: PRICE,
                side: Side::Buy,
                visible: 10,
                hidden: 0,
            }
        }

        fn build(self) -> Arc<OrderType<()>> {
            let common_ts = TimestampMs::new(1_716_000_000_000 + self.id);
            if self.hidden == 0 {
                Arc::new(OrderType::Standard {
                    id: Id::from_u64(self.id),
                    price: Price::new(self.price),
                    quantity: Quantity::new(self.visible),
                    side: self.side,
                    user_id: Hash32::zero(),
                    timestamp: common_ts,
                    time_in_force: TimeInForce::Gtc,
                    extra_fields: (),
                })
            } else {
                Arc::new(OrderType::IcebergOrder {
                    id: Id::from_u64(self.id),
                    price: Price::new(self.price),
                    visible_quantity: Quantity::new(self.visible),
                    hidden_quantity: Quantity::new(self.hidden),
                    side: self.side,
                    user_id: Hash32::zero(),
                    timestamp: common_ts,
                    time_in_force: TimeInForce::Gtc,
                    extra_fields: (),
                })
            }
        }
    }

    /// A snapshot carrying `specs` verbatim; the stored aggregates are
    /// deliberately wrong so the restore must recompute them.
    fn snapshot_of(specs: &[Spec]) -> PriceLevelSnapshot {
        PriceLevelSnapshot::from_raw_parts(
            Price::new(PRICE),
            Quantity::new(1),
            Quantity::new(2),
            3,
            specs.iter().map(|s| s.build()).collect(),
        )
    }

    /// Everything observable about a restored level: its package JSON
    /// (price, aggregates, orders in queue order, statistics, checksum), its
    /// live counters, and the trades a deterministic sweep emits.
    fn fingerprint(level: &PriceLevel) -> String {
        let before = level.snapshot_to_json().expect("snapshot_to_json");
        let counters = (
            level.visible_quantity(),
            level.hidden_quantity(),
            level.order_count(),
        );
        let generator = UuidGenerator::new(
            Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").expect("uuid"),
        );
        let result = level.match_order(
            25,
            Id::from_u64(u64::MAX),
            TimeInForce::Ioc,
            TakerKind::Standard,
            TimestampMs::new(1_800_000_000_000),
            &generator,
        );
        let trades: Vec<_> = result
            .trades()
            .as_vec()
            .iter()
            .map(|t| (t.maker_order_id(), t.price(), t.quantity(), t.taker_side()))
            .collect();
        let after = level.snapshot_to_json().expect("snapshot_to_json");
        format!(
            "{before}|{counters:?}|{trades:?}|{:?}|{after}",
            result.remaining_quantity()
        )
    }

    /// Restores `snapshot` through both paths and asserts identical outcomes.
    fn assert_equivalent(snapshot: &PriceLevelSnapshot) -> Result<(), PriceLevelError> {
        let new = PriceLevel::from_snapshot(snapshot.try_clone().expect("clone"));
        let old = PriceLevel::from_snapshot_legacy(snapshot.try_clone().expect("clone"));
        match (new, old) {
            (Ok(new), Ok(old)) => {
                assert_eq!(fingerprint(&new), fingerprint(&old), "{snapshot:?}");
                Ok(())
            }
            (Err(new), Err(old)) => {
                assert_eq!(new, old, "error precedence diverged for {snapshot:?}");
                Err(new)
            }
            (new, old) => panic!(
                "outcome diverged for {snapshot:?}: new={:?} old={:?}",
                new.map(|_| ()),
                old.map(|_| ())
            ),
        }
    }

    /// The injectable violations, one per rejection class.
    #[derive(Debug, Clone, Copy)]
    enum Violation {
        /// The order's own visible + hidden overflows `u64`.
        OrderTotal,
        /// The running visible sum overflows `u64`.
        VisibleSum,
        /// The running hidden sum overflows `u64`.
        HiddenSum,
        /// The order repeats the id of another order.
        DuplicateId,
        /// The order sits at another price.
        WrongPrice,
        /// The order is on the other side.
        MixedSide,
    }

    const VIOLATIONS: [Violation; 6] = [
        Violation::OrderTotal,
        Violation::VisibleSum,
        Violation::HiddenSum,
        Violation::DuplicateId,
        Violation::WrongPrice,
        Violation::MixedSide,
    ];

    fn inject(specs: &mut [Spec], violation: Violation, at: usize) {
        let n = specs.len();
        let spec = &mut specs[at];
        match violation {
            Violation::OrderTotal => {
                spec.visible = u64::MAX;
                spec.hidden = 1;
            }
            Violation::VisibleSum => {
                spec.visible = u64::MAX - 5;
                spec.hidden = 0;
            }
            Violation::HiddenSum => {
                spec.visible = 1;
                spec.hidden = u64::MAX - 1;
            }
            Violation::DuplicateId => {
                // Repeat an EARLIER id when there is one, else a later one.
                let other = if at > 0 { at - 1 } else { (at + 1) % n };
                spec.id = other as u64 + 1;
            }
            Violation::WrongPrice => spec.price = PRICE + 1,
            Violation::MixedSide => spec.side = Side::Sell,
        }
    }

    #[test]
    fn restore_matches_legacy_for_every_pair_of_violations_at_every_position() {
        const N: usize = 6;
        let positions = [0, 1, N / 2, N - 1];
        let mut cases = 0usize;
        for &first in &VIOLATIONS {
            for &p1 in &positions {
                for &second in &VIOLATIONS {
                    for &p2 in &positions {
                        let mut specs: Vec<Spec> = (1..=N as u64).map(Spec::valid).collect();
                        inject(&mut specs, first, p1);
                        inject(&mut specs, second, p2);
                        let _ = assert_equivalent(&snapshot_of(&specs));
                        cases += 1;
                    }
                }
            }
        }
        // Single violations and the valid baseline.
        for &violation in &VIOLATIONS {
            for &p in &positions {
                let mut specs: Vec<Spec> = (1..=N as u64).map(Spec::valid).collect();
                inject(&mut specs, violation, p);
                let outcome = assert_equivalent(&snapshot_of(&specs));
                // A single hidden-heavy order fits; it needs a second one to
                // overflow the hidden sum.
                if !matches!(violation, Violation::HiddenSum) {
                    assert!(outcome.is_err(), "{violation:?} at {p}");
                }
            }
        }
        let specs: Vec<Spec> = (1..=N as u64).map(Spec::valid).collect();
        assert!(assert_equivalent(&snapshot_of(&specs)).is_ok());
        assert_eq!(cases, 6 * 4 * 6 * 4);
    }

    #[test]
    fn restore_matches_legacy_for_triples_of_violations() {
        const N: usize = 5;
        let mut cases = 0usize;
        for &a in &VIOLATIONS {
            for &b in &VIOLATIONS {
                for &c in &VIOLATIONS {
                    for pa in 0..N {
                        for pb in 0..N {
                            for pc in 0..N {
                                let mut specs: Vec<Spec> =
                                    (1..=N as u64).map(Spec::valid).collect();
                                inject(&mut specs, a, pa);
                                inject(&mut specs, b, pb);
                                inject(&mut specs, c, pc);
                                let _ = assert_equivalent(&snapshot_of(&specs));
                                cases += 1;
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(cases, 216 * 125);
    }

    /// Deterministic xorshift64* stream for the randomized comparison.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    #[test]
    fn restore_matches_legacy_on_random_snapshots() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut ok = 0usize;
        let mut err = 0usize;
        for _ in 0..20_000 {
            let n = rng.below(12) as usize;
            let specs: Vec<Spec> = (0..n)
                .map(|i| {
                    let quantity = |rng: &mut Rng| match rng.below(20) {
                        0 => u64::MAX - rng.below(4),
                        1 => u64::MAX / 2 + rng.below(4),
                        2 => 0,
                        _ => 1 + rng.below(50),
                    };
                    Spec {
                        // Mostly unique ids, sometimes colliding.
                        id: if rng.below(15) == 0 {
                            rng.below(n as u64 + 1)
                        } else {
                            i as u64 + 1_000
                        },
                        price: if rng.below(25) == 0 { PRICE + 1 } else { PRICE },
                        side: if rng.below(25) == 0 {
                            Side::Sell
                        } else {
                            Side::Buy
                        },
                        visible: quantity(&mut rng),
                        hidden: if rng.below(3) == 0 {
                            quantity(&mut rng)
                        } else {
                            0
                        },
                    }
                })
                .collect();
            match assert_equivalent(&snapshot_of(&specs)) {
                Ok(()) => ok += 1,
                Err(_) => err += 1,
            }
        }
        // The stream exercises both outcomes substantially.
        assert!(ok > 1_000 && err > 1_000, "ok={ok} err={err}");
    }

    #[test]
    fn restore_scratch_refusal_keeps_its_rank() {
        let n = 6usize;
        // Refusal alone, and combined with each violation at each end.
        let mut cases: Vec<Vec<Spec>> = vec![(1..=n as u64).map(Spec::valid).collect()];
        for &violation in &VIOLATIONS {
            for at in [0, n - 1] {
                let mut specs: Vec<Spec> = (1..=n as u64).map(Spec::valid).collect();
                inject(&mut specs, violation, at);
                cases.push(specs);
            }
        }
        for specs in cases {
            let snapshot = snapshot_of(&specs);
            let new = {
                let _fail = test_seam::fail_after(CapacityResource::RestoreScratch, 0);
                PriceLevel::from_snapshot(snapshot.try_clone().expect("clone")).map(|_| ())
            };
            let old = {
                let _fail = test_seam::fail_after(CapacityResource::RestoreScratch, 0);
                PriceLevel::from_snapshot_legacy(snapshot.try_clone().expect("clone")).map(|_| ())
            };
            assert_eq!(new, old, "{specs:?}");
            let error = new.expect_err("a refused scratch set or a violation fails");
            // Only an aggregate failure outranks the refusal.
            if !matches!(error, PriceLevelError::InvalidOperation { .. }) {
                assert!(matches!(
                    error,
                    PriceLevelError::CapacityExceeded {
                        resource: CapacityResource::RestoreScratch,
                        ..
                    }
                ));
            }
        }
    }

    // ------------------------------------------------------------------
    // Explicit precedence contract, with failures near the end of a deep
    // snapshot.
    // ------------------------------------------------------------------

    const DEEP: usize = 10_000;

    fn deep() -> Vec<Spec> {
        (1..=DEEP as u64).map(Spec::valid).collect()
    }

    fn restore(specs: &[Spec]) -> Result<(), PriceLevelError> {
        assert_equivalent(&snapshot_of(specs))
    }

    #[test]
    fn deep_snapshot_valid_restores_identically() {
        restore(&deep()).expect("valid deep snapshot restores");
    }

    #[test]
    fn deep_snapshot_failure_at_the_last_order_is_reported() {
        let last = DEEP - 1;

        let mut specs = deep();
        inject(&mut specs, Violation::DuplicateId, last);
        assert_eq!(
            restore(&specs),
            Err(PriceLevelError::DuplicateOrderId(
                Id::from_u64(last as u64).to_string()
            ))
        );

        let mut specs = deep();
        inject(&mut specs, Violation::WrongPrice, last);
        assert!(matches!(
            restore(&specs),
            Err(PriceLevelError::InvalidOperation { message }) if message.contains("price")
        ));

        let mut specs = deep();
        inject(&mut specs, Violation::MixedSide, last);
        assert!(matches!(
            restore(&specs),
            Err(PriceLevelError::InvalidOperation { message }) if message.contains("side")
        ));

        let mut specs = deep();
        inject(&mut specs, Violation::OrderTotal, last);
        assert_eq!(
            restore(&specs),
            Err(PriceLevelError::InvalidOperation {
                message: "order total quantity overflows u64".to_string()
            })
        );
    }

    #[test]
    fn aggregate_failure_at_the_end_outranks_earlier_duplicate_and_topology() {
        let mut specs = deep();
        inject(&mut specs, Violation::MixedSide, 1);
        inject(&mut specs, Violation::DuplicateId, 2);
        inject(&mut specs, Violation::HiddenSum, DEEP - 2);
        inject(&mut specs, Violation::HiddenSum, DEEP - 1);
        assert_eq!(
            restore(&specs),
            Err(PriceLevelError::InvalidOperation {
                message: "snapshot hidden quantity overflow".to_string()
            })
        );
    }

    #[test]
    fn duplicate_at_the_end_outranks_earlier_topology_violation() {
        let mut specs = deep();
        inject(&mut specs, Violation::WrongPrice, 0);
        inject(&mut specs, Violation::DuplicateId, DEEP - 1);
        assert_eq!(
            restore(&specs),
            Err(PriceLevelError::DuplicateOrderId(
                Id::from_u64(DEEP as u64 - 1).to_string()
            ))
        );
    }

    #[test]
    fn first_topology_violation_wins_among_topology_violations() {
        let mut specs = deep();
        inject(&mut specs, Violation::MixedSide, 3);
        inject(&mut specs, Violation::WrongPrice, DEEP - 1);
        assert!(matches!(
            restore(&specs),
            Err(PriceLevelError::InvalidOperation { message }) if message.contains("side")
        ));
    }

    // ------------------------------------------------------------------
    // Package / JSON path: version and checksum still come first.
    // ------------------------------------------------------------------

    fn legacy_from_json(json: &str) -> Result<PriceLevel, PriceLevelError> {
        let package = PriceLevelSnapshotPackage::from_json(json)?;
        PriceLevel::from_snapshot_legacy(package.into_snapshot()?)
    }

    /// A correctly signed package whose content is invalid (duplicate id at
    /// the end): the checksum covers the bytes, not the invariants.
    fn signed_invalid_json() -> String {
        let mut specs = deep();
        inject(&mut specs, Violation::DuplicateId, DEEP - 1);
        let snapshot = PriceLevelSnapshot::with_orders(
            Price::new(PRICE),
            specs.iter().map(|s| s.build()).collect(),
        )
        .expect("aggregates fit");
        PriceLevelSnapshotPackage::new(snapshot)
            .expect("package")
            .to_json()
            .expect("json")
    }

    #[test]
    fn json_path_signed_invalid_content_reports_the_content_error() {
        let json = signed_invalid_json();
        let new = PriceLevel::from_snapshot_json(&json).map(|_| ());
        let old = legacy_from_json(&json).map(|_| ());
        assert_eq!(new, old);
        assert!(matches!(new, Err(PriceLevelError::DuplicateOrderId(_))));
    }

    #[test]
    fn json_path_checksum_mismatch_outranks_content_errors() {
        let json = signed_invalid_json();
        let tampered = json.replacen("\"price\":10000", "\"price\":10001", 1);
        assert_ne!(tampered, json);
        let new = PriceLevel::from_snapshot_json(&tampered).map(|_| ());
        let old = legacy_from_json(&tampered).map(|_| ());
        assert_eq!(new, old);
        assert!(matches!(new, Err(PriceLevelError::ChecksumMismatch { .. })));
    }

    #[test]
    fn json_path_unsupported_version_outranks_checksum_and_content() {
        let json = signed_invalid_json();
        let tampered = json.replacen("\"version\":4", "\"version\":1", 1).replacen(
            "\"price\":10000",
            "\"price\":10001",
            1,
        );
        let new = PriceLevel::from_snapshot_json(&tampered).map(|_| ());
        let old = legacy_from_json(&tampered).map(|_| ());
        assert_eq!(new, old);
        assert!(matches!(
            new,
            Err(PriceLevelError::InvalidOperation { message }) if message.contains("version")
        ));
    }

    // ------------------------------------------------------------------
    // Fixtures and every order type.
    // ------------------------------------------------------------------

    const FIXTURES: [&str; 3] = [
        include_str!("fixtures/snapshot_v2_pricelevel_0_8_4.json"),
        include_str!("fixtures/snapshot_v3_pricelevel_0_9_2.json"),
        include_str!("fixtures/snapshot_v3_degraded_pricelevel_0_9_2.json"),
    ];

    #[test]
    fn legacy_fixtures_restore_identically() {
        for fixture in FIXTURES {
            let new = PriceLevel::from_snapshot_json(fixture.trim()).expect("new restore");
            let old = legacy_from_json(fixture.trim()).expect("legacy restore");
            assert_eq!(fingerprint(&new), fingerprint(&old));
        }
    }

    fn every_order_type() -> Vec<Arc<OrderType<()>>> {
        let price = Price::new(PRICE);
        let side = Side::Sell;
        let user_id = Hash32::zero();
        let time_in_force = TimeInForce::Gtc;
        let ts = |i: u64| TimestampMs::new(1_716_000_000_000 + i);
        vec![
            Arc::new(OrderType::Standard {
                id: Id::from_u64(1),
                price,
                quantity: Quantity::new(5),
                side,
                user_id,
                timestamp: ts(1),
                time_in_force,
                extra_fields: (),
            }),
            Arc::new(OrderType::IcebergOrder {
                id: Id::from_u64(2),
                price,
                visible_quantity: Quantity::new(3),
                hidden_quantity: Quantity::new(9),
                side,
                user_id,
                timestamp: ts(2),
                time_in_force,
                extra_fields: (),
            }),
            Arc::new(OrderType::PostOnly {
                id: Id::from_u64(3),
                price,
                quantity: Quantity::new(4),
                side,
                user_id,
                timestamp: ts(3),
                time_in_force,
                extra_fields: (),
            }),
            Arc::new(OrderType::TrailingStop {
                id: Id::from_u64(4),
                price,
                quantity: Quantity::new(2),
                side,
                user_id,
                timestamp: ts(4),
                time_in_force,
                trail_amount: Quantity::new(1),
                last_reference_price: price,
                extra_fields: (),
            }),
            Arc::new(OrderType::PeggedOrder {
                id: Id::from_u64(5),
                price,
                quantity: Quantity::new(6),
                side,
                user_id,
                timestamp: ts(5),
                time_in_force,
                reference_price_offset: -1,
                reference_price_type: PegReferenceType::BestAsk,
                extra_fields: (),
            }),
            Arc::new(OrderType::MarketToLimit {
                id: Id::from_u64(6),
                price,
                quantity: Quantity::new(7),
                side,
                user_id,
                timestamp: ts(6),
                time_in_force,
                extra_fields: (),
            }),
            Arc::new(OrderType::ReserveOrder {
                id: Id::from_u64(7),
                price,
                visible_quantity: Quantity::new(2),
                hidden_quantity: Quantity::new(8),
                side,
                user_id,
                timestamp: ts(7),
                time_in_force,
                replenish_threshold: Quantity::new(1),
                replenish_amount: NonZeroU64::new(2),
                auto_replenish: true,
                extra_fields: (),
            }),
        ]
    }

    #[test]
    fn every_order_type_round_trips_identically_through_the_package() {
        let stats = PriceLevelStatistics::new();
        stats
            .record_execution(5, PRICE, 0, 1_000)
            .expect("record execution");
        let snapshot =
            PriceLevelSnapshot::with_orders_and_stats(Price::new(PRICE), every_order_type(), stats)
                .expect("snapshot");
        let json = PriceLevelSnapshotPackage::new(snapshot)
            .expect("package")
            .to_json()
            .expect("json");

        let new = PriceLevel::from_snapshot_json(&json).expect("new restore");
        let old = legacy_from_json(&json).expect("legacy restore");
        // Insertion priority (#109): queue order is the snapshot's order.
        let snapshot = new.snapshot().expect("snapshot");
        let ids: Vec<Id> = snapshot.orders().iter().map(|o| o.id()).collect();
        assert_eq!(ids, (1..=7).map(Id::from_u64).collect::<Vec<_>>());
        // Byte-identical re-encoding of the restored level.
        assert_eq!(new.snapshot_to_json().expect("json"), json);
        assert_eq!(fingerprint(&new), fingerprint(&old));
    }

    // ------------------------------------------------------------------
    // Review of #150: moved statistics restart their seqlock sequence.
    // ------------------------------------------------------------------

    /// Owned statistics with an exhausted-looking sequence, moved (not
    /// cloned) into a snapshot, then restored WITHOUT `try_clone` (which
    /// would itself restart the sequence and mask the move).
    fn snapshot_with_exhausted_stats_seq() -> PriceLevelSnapshot {
        let stats = PriceLevelStatistics::new_at(TimestampMs::new(7));
        stats.record_execution(3, PRICE, 0, 50).expect("record");
        // Even and above the entry limit: the next section is refused.
        stats.test_seed_stats_seq(u64::MAX - 1);
        assert!(stats.record_execution(1, PRICE, 0, 60).is_err());
        assert!(stats.stats_degraded());
        stats.test_seed_stats_seq(u64::MAX - 1);
        PriceLevelSnapshot::with_orders_and_stats(
            Price::new(PRICE),
            vec![Spec::valid(1).build(), Spec::valid(2).build()],
            stats,
        )
        .expect("snapshot")
    }

    #[test]
    fn restore_restarts_statistics_sequence_of_moved_statistics() {
        let restored =
            PriceLevel::from_snapshot(snapshot_with_exhausted_stats_seq()).expect("restore");
        let stats = restored.stats();
        assert_eq!(stats.test_stats_seq(), 0, "sequence restarted");
        // Counters and the degraded flag are preserved.
        assert_eq!(stats.orders_executed(), 1);
        assert_eq!(stats.quantity_executed(), 3);
        assert_eq!(stats.last_execution_time(), 50);
        assert!(stats.stats_degraded());
        // The documented rebuild recovery (#165): recording works again.
        assert_eq!(stats.record_execution(1, PRICE, 0, 70), Ok(()));
        assert_eq!(stats.orders_executed(), 2);
    }

    #[test]
    fn restore_and_legacy_restore_agree_on_moved_exhausted_statistics() {
        let new = PriceLevel::from_snapshot(snapshot_with_exhausted_stats_seq()).expect("new");
        let old =
            PriceLevel::from_snapshot_legacy(snapshot_with_exhausted_stats_seq()).expect("old");
        assert_eq!(new.stats().test_stats_seq(), old.stats().test_stats_seq());
        assert_eq!(
            new.stats().record_execution(1, PRICE, 0, 70),
            old.stats().record_execution(1, PRICE, 0, 70)
        );
        assert_eq!(fingerprint(&new), fingerprint(&old));
    }

    #[test]
    fn aggregate_failure_anywhere_reserves_no_scratch_set() {
        // The aggregate fold completes before the duplicate-id set is
        // reserved, so an aggregate rejection at any position never reaches
        // the reservation (the refusal seam would otherwise fire), exactly as
        // before #150.
        for at in [0, 1, DEEP - 1] {
            let mut specs: Vec<Spec> = (1..=DEEP as u64).map(Spec::valid).collect();
            inject(&mut specs, Violation::OrderTotal, at);
            let _fail = test_seam::fail_after(CapacityResource::RestoreScratch, 0);
            assert_eq!(
                PriceLevel::from_snapshot(snapshot_of(&specs)).map(|_| ()),
                Err(PriceLevelError::InvalidOperation {
                    message: "order total quantity overflows u64".to_string()
                })
            );
            assert_eq!(test_seam::injected(), 0, "no reservation attempted at {at}");
        }
    }
}
