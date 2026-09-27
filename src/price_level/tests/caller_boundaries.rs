//! Caller-supplied code boundaries (issue #172).
//!
//! These tests deliberately run panicking / re-entrant caller code (a
//! `tracing` subscriber, a `fmt::Write` destination) and use test-only
//! `catch_unwind` to check the documented state behaviour. Production code
//! installs no panic hook and catches nothing.

#[cfg(test)]
mod tests {
    use crate::UuidGenerator;
    use crate::execution::{MatchOutcome, TakerKind};
    use crate::orders::{Hash32, Id, OrderType, OrderUpdate, Side, TimeInForce};
    use crate::price_level::PriceLevel;
    use crate::utils::{Price, Quantity, TimestampMs};
    use std::fmt::Write as _;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use tracing_subscriber::Layer;
    use tracing_subscriber::layer::{Context, SubscriberExt};
    use tracing_subscriber::registry;
    use uuid::Uuid;

    const PRICE: u128 = 10_000;

    fn maker(id: u64, quantity: u64) -> OrderType<()> {
        OrderType::Standard {
            id: Id::from_u64(id),
            price: Price::new(PRICE),
            quantity: Quantity::new(quantity),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_616_823_000_000 + id),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    /// A subscriber layer whose every event panics: stands in for a faulty
    /// process-installed subscriber (caller-supplied code).
    struct PanickingLayer;

    impl<S: tracing::Subscriber> Layer<S> for PanickingLayer {
        fn on_event(&self, _event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            panic!("deliberately panicking subscriber (test)");
        }
    }

    /// A fill-or-kill kill logs its `debug!` only after releasing the exclusive
    /// guard, so a panicking subscriber neither poisons the guard nor leaves
    /// the level refusing work: the kill mutated nothing and the level stays
    /// fully usable.
    #[test]
    fn fok_kill_with_panicking_subscriber_does_not_poison_level() {
        let level = PriceLevel::new(PRICE);
        level.add_order(maker(1, 10)).expect("admit maker");
        let ids = UuidGenerator::new(Uuid::new_v4());

        // `tracing` caches callsite interest process-wide, so a concurrent
        // test thread that (re)registers dispatchers can transiently hide the
        // kill event from this thread's scoped subscriber. A killed FOK that
        // emitted nothing mutated nothing, so retrying is harmless; the test
        // requires that the panicking subscriber is reached at least once.
        let mut panicked = false;
        for attempt in 0..1_000u64 {
            let subscriber = registry().with(PanickingLayer);
            let unwound = tracing::subscriber::with_default(subscriber, || {
                tracing::callsite::rebuild_interest_cache();
                catch_unwind(AssertUnwindSafe(|| {
                    level.match_order(
                        100,
                        Id::from_u64(10_000 + attempt),
                        TimeInForce::Fok,
                        TakerKind::Standard,
                        TimestampMs::new(1),
                        &ids,
                    )
                }))
            });
            if unwound.is_err() {
                panicked = true;
                break;
            }
        }
        assert!(panicked, "the subscriber panic must propagate");

        // Nothing was mutated by the killed FOK.
        assert_eq!(level.order_count(), 1);
        assert_eq!(level.visible_quantity(), 10);

        // Not poisoned: admission and a normal match still work.
        level
            .add_order(maker(2, 5))
            .expect("level must not be poisoned");
        let result = level.match_order(
            15,
            Id::from_u64(100),
            TimeInForce::Fok,
            TakerKind::Standard,
            TimestampMs::new(2),
            &ids,
        );
        assert_eq!(result.outcome(), MatchOutcome::Filled);
        assert_eq!(level.order_count(), 0);
        assert_eq!(level.visible_quantity(), 0);
    }

    /// A formatting destination that cancels every resting order the moment
    /// an order BODY is being written (the `Standard` variant name). With the
    /// old derived `Debug`, that write happened inside `DashMap`'s `Debug`,
    /// under the shard read lock of the entry being formatted, so cancelling
    /// that same order (shard write lock, same thread) deadlocked.
    struct CancelOnOrderBody<'a> {
        level: &'a PriceLevel,
        ids: Vec<Id>,
        fired: bool,
        out: String,
    }

    impl std::fmt::Write for CancelOnOrderBody<'_> {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            if !self.fired && s.contains("Standard") {
                self.fired = true;
                for id in self.ids.drain(..) {
                    let _ = self
                        .level
                        .update_order(OrderUpdate::Cancel { order_id: id });
                }
            }
            self.out.push_str(s);
            Ok(())
        }
    }

    #[test]
    fn debug_does_not_hold_shard_locks_while_writing_order_bodies() {
        let level = PriceLevel::new(PRICE);
        let ids: Vec<Id> = (1..=64).map(Id::from_u64).collect();
        for n in 1..=64 {
            level.add_order(maker(n, 1)).expect("admit maker");
        }

        let mut writer = CancelOnOrderBody {
            level: &level,
            ids,
            fired: false,
            out: String::new(),
        };
        write!(writer, "{level:?}").expect("formatting succeeds");

        assert!(writer.fired, "the re-entrant cancel must have been armed");
        assert!(writer.out.starts_with("PriceLevel"));
        // The text is the pre-cancel materialization (all 64 bodies); the
        // level reflects the destination's re-entrant cancels.
        assert_eq!(writer.out.matches("Standard").count(), 64);
        assert_eq!(level.order_count(), 0);
        assert_eq!(level.visible_quantity(), 0);
    }

    /// A formatting destination that runs a fill-or-kill match on EVERY write.
    /// On an empty level the old derived `Debug` held only one guard while
    /// writing: the `fok_guard` read guard (inside `RwLock`'s `Debug`), so the
    /// re-entrant FOK's exclusive acquisition on the same thread deadlocked.
    /// The hand-written `Debug` omits the guard, so every re-entry succeeds.
    struct FokOnEveryWrite<'a> {
        level: &'a PriceLevel,
        ids: &'a UuidGenerator,
        matches: u64,
    }

    impl std::fmt::Write for FokOnEveryWrite<'_> {
        fn write_str(&mut self, _s: &str) -> std::fmt::Result {
            self.matches += 1;
            let result = self.level.match_order(
                1,
                Id::from_u64(1_000 + self.matches),
                TimeInForce::Fok,
                TakerKind::Standard,
                TimestampMs::new(1),
                self.ids,
            );
            if result.outcome() == MatchOutcome::Killed {
                Ok(())
            } else {
                Err(std::fmt::Error)
            }
        }
    }

    #[test]
    fn debug_does_not_hold_fok_guard_while_writing() {
        let level = PriceLevel::new(PRICE);
        let ids = UuidGenerator::new(Uuid::new_v4());
        let mut writer = FokOnEveryWrite {
            level: &level,
            ids: &ids,
            matches: 0,
        };
        write!(writer, "{level:?}").expect("every re-entrant FOK is killed, none blocks");
        assert!(writer.matches > 0);
        assert_eq!(level.order_count(), 0);
    }

    /// A formatting destination that panics mid-write leaves the level
    /// unchanged and usable: `Debug` is read-only and holds no guard.
    struct PanickingWriter;

    impl std::fmt::Write for PanickingWriter {
        fn write_str(&mut self, _s: &str) -> std::fmt::Result {
            panic!("deliberately panicking fmt::Write destination (test)");
        }
    }

    #[test]
    fn panicking_debug_destination_leaves_level_intact() {
        let level = PriceLevel::new(PRICE);
        level.add_order(maker(1, 7)).expect("admit maker");

        let unwound = catch_unwind(AssertUnwindSafe(|| {
            let _ = write!(PanickingWriter, "{level:?}");
        }));
        assert!(unwound.is_err());

        assert_eq!(level.order_count(), 1);
        assert_eq!(level.visible_quantity(), 7);
        level
            .update_order(OrderUpdate::Cancel {
                order_id: Id::from_u64(1),
            })
            .expect("cancel succeeds");
        assert_eq!(level.order_count(), 0);
    }
}
