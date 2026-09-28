//! Issue #214: the deferred removal events log the error the call returns.
//!
//! `DeferredEvents` stores no error payload. For `REMOVAL_REFUSED` and
//! `RELEASE_FAILED` the emitter logs the `Err` that `update_order` itself
//! returns, which is correct only while every path that records one of those
//! bits returns that error straight away. These tests pin that contract: the
//! `error` field of each event is exactly the `Display` of the returned
//! error.

#[cfg(test)]
mod tests {
    use crate::orders::{Hash32, Id, OrderType, OrderUpdate, Side, TimeInForce};
    use crate::price_level::PriceLevel;
    use crate::price_level::order_queue::set_remove_gap_hook;
    use crate::utils::{Price, Quantity, TimestampMs};
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing_subscriber::Layer;
    use tracing_subscriber::layer::{Context, SubscriberExt};
    use tracing_subscriber::registry;

    const PRICE: u128 = 10_000;

    fn maker(id: u64) -> OrderType<()> {
        OrderType::Standard {
            id: Id::from_u64(id),
            price: Price::new(PRICE),
            quantity: Quantity::new(5),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1_616_823_000_000),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    /// `(message, error field)` of one captured event.
    type Captured = Arc<Mutex<Vec<(String, Option<String>)>>>;

    #[derive(Default)]
    struct Fields {
        message: String,
        error: Option<String>,
    }

    impl Visit for Fields {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            match field.name() {
                "message" => self.message = format!("{value:?}"),
                "error" => self.error = Some(format!("{value:?}")),
                _ => {}
            }
        }
    }

    struct Capture(Captured);

    impl<S: tracing::Subscriber> Layer<S> for Capture {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            let mut fields = Fields::default();
            event.record(&mut fields);
            if let Ok(mut events) = self.0.lock() {
                events.push((fields.message, fields.error));
            }
        }
    }

    /// Runs `op` under a capturing subscriber (retrying, like the guard
    /// probes in `caller_boundaries`, in case a concurrent test resets the
    /// process-wide interest cache) and returns the returned error's
    /// `Display` with the `error` field of the event whose message contains
    /// `needle`.
    fn returned_and_logged(
        setup: impl Fn() -> Arc<PriceLevel>,
        needle: &str,
    ) -> (String, Option<String>) {
        for _ in 0..1_000 {
            let level = setup();
            let captured: Captured = Arc::default();
            let subscriber = registry().with(Capture(Arc::clone(&captured)));
            let returned = tracing::subscriber::with_default(subscriber, || {
                tracing::callsite::rebuild_interest_cache();
                level.update_order(OrderUpdate::Cancel {
                    order_id: Id::from_u64(1),
                })
            });
            let returned = returned.expect_err("the removal is reported as an error");
            let events = captured.lock().expect("capture lock").clone();
            if let Some((_, logged)) = events.into_iter().find(|(m, _)| m.contains(needle)) {
                return (returned.to_string(), logged);
            }
        }
        panic!("no `{needle}` event reached the capturing subscriber");
    }

    #[cfg(not(loom))]
    #[test]
    fn removal_refused_event_logs_the_returned_error() {
        let (returned, logged) = returned_and_logged(
            || {
                let level = Arc::new(PriceLevel::new(PRICE));
                level.add_order(maker(1)).expect("admit");
                // The count claims no resting order: the removal is refused
                // before any mutation.
                level.test_force_topology(Some(Side::Sell), 0);
                level
            },
            "removal rejected before mutation",
        );
        assert_eq!(logged.as_deref(), Some(returned.as_str()));
    }

    #[cfg(not(loom))]
    #[test]
    fn release_failed_event_logs_the_returned_error() {
        let level_slot: Arc<Mutex<Option<Arc<PriceLevel>>>> = Arc::default();
        let hook_slot = Arc::clone(&level_slot);
        // In the gap after the entry is removed and before its count is
        // released, drop the count to zero so the release fails.
        let _hook = set_remove_gap_hook(std::rc::Rc::new(move |_id| {
            if let Some(level) = hook_slot.lock().expect("slot lock").as_ref() {
                level.test_force_topology(Some(Side::Sell), 0);
            }
        }));
        let (returned, logged) = returned_and_logged(
            || {
                let level = Arc::new(PriceLevel::new(PRICE));
                level.add_order(maker(1)).expect("admit");
                *level_slot.lock().expect("slot lock") = Some(Arc::clone(&level));
                level
            },
            "underflow after a committed removal",
        );
        assert_eq!(logged.as_deref(), Some(returned.as_str()));
    }
}
