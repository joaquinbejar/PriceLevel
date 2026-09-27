//! Issue #149: the streaming snapshot encoders are byte-equivalent to the
//! buffered ones they replaced.
//!
//! The production path serializes the orders borrowed (`BorrowedOrders`, no
//! `Vec<&OrderType<()>>`) and hashes the canonical JSON by streaming it into
//! a SHA-256 `io::Write` adapter (no `serde_json::to_vec` payload). This
//! module keeps a **test-only** copy of the old path (collect the reference
//! vector, `to_vec` the payload, hash it, `to_string` the package) and checks
//! that both produce identical JSON bytes and checksums, over deterministic
//! pseudo-random snapshots covering every order variant, every id kind and
//! wide / degraded statistics, and over every pinned legacy fixture.

#[cfg(test)]
// Scoped to this co-located test module only (issue #173): the deterministic
// PRNG fixture generator below does raw arithmetic and a sign-changing
// `u64`-to-`i64` cast on generated offsets, permitted inside `mod tests` per
// the Testing section of `rules/global_rules.md`. Production code outside
// this module keeps the full deny list.
#[allow(clippy::arithmetic_side_effects, clippy::cast_possible_wrap)]
mod tests {
    use crate::errors::PriceLevelError;
    use crate::orders::{Hash32, Id, OrderType, PegReferenceType, Side, TimeInForce};
    use crate::price_level::snapshot::SNAPSHOT_FORMAT_VERSION;
    use crate::price_level::{
        PriceLevel, PriceLevelSnapshot, PriceLevelSnapshotPackage, PriceLevelStatistics,
    };
    use crate::utils::{Price, Quantity, TimestampMs};
    use serde::ser::{Serialize, SerializeStruct, Serializer};
    use sha2::{Digest, Sha256};
    use std::num::NonZeroU64;
    use std::sync::Arc;

    // ------------------------------------------------------------------
    // Test-only reference: the pre-#149 (pre-#164) buffered encoders.
    // ------------------------------------------------------------------

    /// The old `impl Serialize for PriceLevelSnapshot`: collects the order
    /// references into a vector before serializing them.
    struct LegacySnapshot<'a>(&'a PriceLevelSnapshot);

    impl Serialize for LegacySnapshot<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let snapshot = self.0;
            let orders: Vec<&OrderType<()>> = snapshot.orders().iter().map(Arc::as_ref).collect();
            let mut state = serializer.serialize_struct("PriceLevelSnapshot", 6)?;
            state.serialize_field("price", &snapshot.price())?;
            state.serialize_field("visible_quantity", &snapshot.visible_quantity())?;
            state.serialize_field("hidden_quantity", &snapshot.hidden_quantity())?;
            state.serialize_field("order_count", &snapshot.order_count())?;
            state.serialize_field("orders", &orders)?;
            state.serialize_field("statistics", snapshot.statistics())?;
            state.end()
        }
    }

    /// The old derived package envelope, over the legacy snapshot encoder.
    struct LegacyPackage<'a> {
        version: u32,
        snapshot: &'a PriceLevelSnapshot,
        checksum: &'a str,
    }

    impl Serialize for LegacyPackage<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut state = serializer.serialize_struct("PriceLevelSnapshotPackage", 3)?;
            state.serialize_field("version", &self.version)?;
            state.serialize_field("snapshot", &LegacySnapshot(self.snapshot))?;
            state.serialize_field("checksum", self.checksum)?;
            state.end()
        }
    }

    /// Old canonical payload: a fully materialized `serde_json::to_vec`.
    fn legacy_payload(snapshot: &PriceLevelSnapshot) -> Vec<u8> {
        serde_json::to_vec(&LegacySnapshot(snapshot)).expect("legacy payload encodes")
    }

    /// Old checksum: SHA-256 over the buffered payload, lowercase hex.
    fn legacy_checksum(snapshot: &PriceLevelSnapshot) -> String {
        use std::fmt::Write as _;
        let digest = Sha256::digest(legacy_payload(snapshot));
        let mut hex = String::new();
        for byte in digest {
            write!(hex, "{byte:02x}").expect("writing to a String is infallible");
        }
        hex
    }

    /// Old package JSON: `serde_json::to_string` of the envelope.
    fn legacy_package_json(version: u32, snapshot: &PriceLevelSnapshot, checksum: &str) -> String {
        serde_json::to_string(&LegacyPackage {
            version,
            snapshot,
            checksum,
        })
        .expect("legacy package encodes")
    }

    // ------------------------------------------------------------------
    // Deterministic pseudo-random snapshots
    // ------------------------------------------------------------------

    /// xorshift64*: deterministic, dependency-free.
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

        fn u128(&mut self) -> u128 {
            (u128::from(self.next()) << 64) | u128::from(self.next())
        }
    }

    fn random_id(rng: &mut Rng, index: u64) -> Id {
        // Distinct ids per snapshot: the index is folded into every kind.
        match rng.below(3) {
            0 => Id::from_u64(index),
            1 => Id::from_uuid(uuid::Uuid::from_u128(
                (rng.u128() << 32) | u128::from(index),
            )),
            _ => Id::from_ulid(ulid::Ulid((rng.u128() << 32) | u128::from(index))),
        }
    }

    fn random_tif(rng: &mut Rng) -> TimeInForce {
        match rng.below(5) {
            0 => TimeInForce::Gtc,
            1 => TimeInForce::Ioc,
            2 => TimeInForce::Fok,
            3 => TimeInForce::Gtd(rng.next()),
            _ => TimeInForce::Day,
        }
    }

    fn random_order(rng: &mut Rng, index: u64, price: Price) -> OrderType<()> {
        let id = random_id(rng, index);
        let side = if rng.below(2) == 0 {
            Side::Buy
        } else {
            Side::Sell
        };
        let mut user = [0u8; 32];
        for chunk in user.chunks_mut(8) {
            chunk.copy_from_slice(&rng.next().to_le_bytes());
        }
        let user_id = if rng.below(4) == 0 {
            Hash32::zero()
        } else {
            Hash32::new(user)
        };
        let timestamp = TimestampMs::new(rng.next() >> 20);
        let time_in_force = random_tif(rng);
        // Keep each order's quantities small enough that the level's summed
        // aggregates cannot overflow `u64`.
        let quantity = Quantity::new(rng.below(1 << 40) + 1);
        let hidden = Quantity::new(rng.below(1 << 40));
        match rng.below(7) {
            0 => OrderType::Standard {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields: (),
            },
            1 => OrderType::IcebergOrder {
                id,
                price,
                visible_quantity: quantity,
                hidden_quantity: hidden,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields: (),
            },
            2 => OrderType::PostOnly {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields: (),
            },
            3 => OrderType::TrailingStop {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                trail_amount: Quantity::new(rng.below(1 << 20)),
                last_reference_price: Price::new(rng.u128()),
                extra_fields: (),
            },
            4 => OrderType::PeggedOrder {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                reference_price_offset: rng.next() as i64,
                reference_price_type: match rng.below(4) {
                    0 => PegReferenceType::BestBid,
                    1 => PegReferenceType::BestAsk,
                    2 => PegReferenceType::MidPrice,
                    _ => PegReferenceType::LastTrade,
                },
                extra_fields: (),
            },
            5 => OrderType::MarketToLimit {
                id,
                price,
                quantity,
                side,
                user_id,
                timestamp,
                time_in_force,
                extra_fields: (),
            },
            _ => OrderType::ReserveOrder {
                id,
                price,
                visible_quantity: quantity,
                hidden_quantity: hidden,
                side,
                user_id,
                timestamp,
                time_in_force,
                replenish_threshold: Quantity::new(rng.below(1 << 20)),
                replenish_amount: NonZeroU64::new(rng.below(1 << 20)),
                auto_replenish: rng.below(2) == 0,
                extra_fields: (),
            },
        }
    }

    fn random_stats(rng: &mut Rng) -> PriceLevelStatistics {
        let stats = PriceLevelStatistics::new_at(TimestampMs::new(rng.below(1 << 30)));
        for _ in 0..rng.below(4) {
            stats.record_order_added().expect("orders_added");
        }
        if rng.below(2) == 0 {
            stats.record_order_removed().expect("orders_removed");
        }
        for _ in 0..rng.below(4) {
            // Occasionally a value above `u64::MAX` (the v4 widening).
            let price = if rng.below(3) == 0 {
                rng.u128() >> 40
            } else {
                u128::from(rng.below(1 << 32))
            };
            let order_ts = rng.below(1 << 30);
            stats
                .record_execution(rng.below(1 << 20) + 1, price, order_ts, order_ts + 7)
                .expect("record_execution");
        }
        if rng.below(5) == 0 {
            stats.mark_degraded();
        }
        stats
    }

    fn random_snapshot(rng: &mut Rng) -> PriceLevelSnapshot {
        let price = Price::new(rng.u128() >> rng.below(128));
        let count = match rng.below(4) {
            0 => 0,
            1 => 1,
            _ => rng.below(64),
        };
        let orders: Vec<Arc<OrderType<()>>> = (0..count)
            .map(|index| Arc::new(random_order(rng, index, price)))
            .collect();
        PriceLevelSnapshot::with_orders_and_stats(price, orders, random_stats(rng))
            .expect("random snapshot builds")
    }

    /// Asserts every encoder agrees with its legacy reference for `snapshot`.
    fn assert_equivalent(snapshot: PriceLevelSnapshot) {
        let legacy_bytes = legacy_payload(&snapshot);
        let streamed_bytes = serde_json::to_vec(&snapshot).expect("snapshot encodes");
        assert_eq!(streamed_bytes, legacy_bytes, "snapshot JSON bytes differ");

        let expected_checksum = legacy_checksum(&snapshot);
        let package = PriceLevelSnapshotPackage::new(snapshot).expect("package");
        assert_eq!(package.checksum(), expected_checksum, "checksum differs");

        let json = package.to_json().expect("package encodes");
        assert_eq!(
            json,
            legacy_package_json(package.version(), package.snapshot(), package.checksum()),
            "package JSON bytes differ"
        );

        // The streamed package validates, and tampering is still detected.
        let decoded = PriceLevelSnapshotPackage::from_json(&json).expect("decodes");
        decoded.validate().expect("streamed package validates");
        let tampered = json.replacen("\"order_count\":", "\"order_count\":1", 1);
        let tampered = PriceLevelSnapshotPackage::from_json(&tampered).expect("parses");
        assert!(matches!(
            tampered.validate(),
            Err(PriceLevelError::ChecksumMismatch { .. })
        ));
    }

    #[test]
    fn test_streaming_matches_legacy_on_random_snapshots() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..256 {
            assert_equivalent(random_snapshot(&mut rng));
        }
    }

    #[test]
    fn test_streaming_matches_legacy_on_empty_snapshot() {
        assert_equivalent(PriceLevelSnapshot::new(Price::new(0)));
        assert_equivalent(PriceLevelSnapshot::new(Price::new(u128::MAX)));
    }

    #[test]
    fn test_streaming_matches_legacy_on_captured_level() {
        // A snapshot taken from a live level (the `snapshot_to_json` path).
        let mut rng = Rng(0xD1B5_4A32_D192_ED03);
        let price = Price::new(10_000);
        let level = PriceLevel::new(price.as_u128());
        for index in 0..200 {
            let order = OrderType::Standard {
                id: random_id(&mut rng, index),
                price,
                quantity: Quantity::new(rng.below(1 << 30) + 1),
                side: Side::Sell,
                user_id: Hash32::new([7; 32]),
                timestamp: TimestampMs::new(index),
                time_in_force: TimeInForce::Gtc,
                extra_fields: (),
            };
            level.add_order(order).expect("add_order");
        }
        let json = level.snapshot_to_json().expect("snapshot_to_json");
        let package = PriceLevelSnapshotPackage::from_json(&json).expect("decodes");
        assert_eq!(package.checksum(), legacy_checksum(package.snapshot()));
        assert_eq!(
            json,
            legacy_package_json(package.version(), package.snapshot(), package.checksum())
        );
        let restored = PriceLevel::from_snapshot_json(&json).expect("restores");
        assert_eq!(restored.snapshot_to_json().expect("re-encodes"), json);
    }

    // ------------------------------------------------------------------
    // Pinned fixtures: every supported version re-encodes byte-identically.
    // ------------------------------------------------------------------

    const FIXTURES: [(&str, u32, &str); 4] = [
        (
            include_str!("fixtures/snapshot_v2_pricelevel_0_8_4.json"),
            2,
            "f1cb277735453e6778a1b53ef9badf8f3e9896e577b97fc0ecace724925342c6",
        ),
        (
            include_str!("fixtures/snapshot_v3_pricelevel_0_9_2.json"),
            3,
            "a2af7207e6afe8ac17d4698ad7253427c36b26bc843a783a08d69ab691aceeb5",
        ),
        (
            include_str!("fixtures/snapshot_v3_degraded_pricelevel_0_9_2.json"),
            3,
            "bb09e16ae1b5b929b25bf6e4ba6d2828ae3cd59186af8034b22657fca1cdbe74",
        ),
        (
            include_str!("fixtures/snapshot_v4_pricelevel_0_10_0.json"),
            4,
            "91b6a69003e82b6321b8dc050f1efa84d835d193f7bffc5490a61395465e039d",
        ),
    ];

    #[test]
    fn test_fixtures_reencode_byte_identically() {
        for (fixture, version, checksum) in FIXTURES {
            let fixture = fixture.trim();
            let package = PriceLevelSnapshotPackage::from_json(fixture).expect("fixture parses");
            assert_eq!(package.version(), version);
            assert_eq!(package.checksum(), checksum, "fixture checksum is pinned");
            package.validate().expect("fixture validates");

            // The stored checksum is the legacy one, and the streaming path
            // re-encodes the exact stored bytes.
            assert_eq!(legacy_checksum(package.snapshot()), checksum);
            assert_eq!(package.to_json().expect("re-encodes"), fixture);
            assert_eq!(
                legacy_package_json(version, package.snapshot(), package.checksum()),
                fixture
            );
        }
    }

    /// The snapshot pinned as `fixtures/snapshot_v4_pricelevel_0_10_0.json`:
    /// every order variant and id kind, a wide (`> u64::MAX`) executed value.
    fn v4_fixture_snapshot() -> PriceLevelSnapshot {
        let mut rng = Rng(0x0149_0149_0149_0149);
        let price = Price::new(1_000_000);
        let mut orders: Vec<Arc<OrderType<()>>> = (0..14)
            .map(|index| Arc::new(random_order(&mut rng, index, price)))
            .collect();
        // The seed draws every variant but `MarketToLimit`; pin one too.
        orders.push(Arc::new(OrderType::MarketToLimit {
            id: Id::from_u64(14),
            price,
            quantity: Quantity::new(5),
            side: Side::Buy,
            user_id: Hash32::new([0xAB; 32]),
            timestamp: TimestampMs::new(1_700_000_000_000),
            time_in_force: TimeInForce::Ioc,
            extra_fields: (),
        }));
        let stats = PriceLevelStatistics::new_at(TimestampMs::new(1_000));
        stats.record_order_added().expect("orders_added");
        stats
            .record_execution(100_000_000, 4_999_500_000_000, 1_000, 2_000)
            .expect("wide execution");
        PriceLevelSnapshot::with_orders_and_stats(price, orders, stats).expect("fixture snapshot")
    }

    #[test]
    fn test_v4_fixture_matches_its_generator() {
        let package = PriceLevelSnapshotPackage::new(v4_fixture_snapshot()).expect("package");
        assert_eq!(package.version(), SNAPSHOT_FORMAT_VERSION);
        assert!(package.snapshot().statistics().value_executed() > u128::from(u64::MAX));
        let (fixture, _, checksum) = FIXTURES[3];
        assert_eq!(package.checksum(), checksum);
        assert_eq!(package.to_json().expect("encodes"), fixture.trim());
    }

    /// Regenerates the v4 fixture: `cargo test print_v4_fixture -- --ignored
    /// --nocapture`. The committed file was written by `origin/main` at
    /// `a597851` (before #149), i.e. by the path under test's predecessor.
    #[test]
    #[ignore = "fixture generator"]
    fn print_v4_fixture() {
        let package = PriceLevelSnapshotPackage::new(v4_fixture_snapshot()).expect("package");
        println!("{}", package.to_json().expect("encodes"));
    }
}
