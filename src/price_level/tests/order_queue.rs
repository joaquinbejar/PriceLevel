#[cfg(test)]
mod tests {
    use crate::orders::{Hash32, Id, OrderType, Side, TimeInForce};
    use crate::price_level::order_queue::OrderQueue;
    use crate::utils::{Price, Quantity, TimestampMs};
    use std::str::FromStr;
    use std::sync::Arc;
    use tracing::info;

    fn create_test_order(id: u64, price: u128, quantity: u64) -> OrderType<()> {
        OrderType::<()>::Standard {
            id: Id::from_u64(id),
            price: Price::new(price),
            quantity: Quantity::new(quantity),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000000),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    #[test]
    fn test_display() {
        let queue = OrderQueue::new();
        queue.push(Arc::new(create_test_order(1, 1000u128, 10)));
        queue.push(Arc::new(create_test_order(2, 1100u128, 20)));

        let display_string = queue.to_string();
        info!("Display: {}", display_string);

        assert!(display_string.starts_with("OrderQueue:orders=["));
        assert!(display_string.contains("id=00000000-0000-0001-0000-000000000000"));
        assert!(display_string.contains("id=00000000-0000-0002-0000-000000000000"));
        assert!(display_string.contains("price=1000"));
        assert!(display_string.contains("price=1100"));
    }

    #[test]
    fn test_from_str() {
        // Create a queue directly for consistency check
        let queue = OrderQueue::new();
        queue.push(Arc::new(create_test_order(1, 1000u128, 10)));
        queue.push(Arc::new(create_test_order(2, 1100u128, 20)));

        // Get the display string
        let display_string = queue.to_string();

        // Verify display string format
        assert!(display_string.starts_with("OrderQueue:orders=["));
        assert!(display_string.contains("id=00000000-0000-0001-0000-000000000000"));
        assert!(display_string.contains("id=00000000-0000-0002-0000-000000000000"));
        assert!(display_string.contains("price=1000"));
        assert!(display_string.contains("price=1100"));

        // Example input string format (manually constructed to match expected format)
        let input = "OrderQueue:orders=[Standard:id=00000000-0000-0001-0000-000000000000;price=1000;quantity=10;side=BUY;timestamp=1616823000000;time_in_force=GTC,Standard:id=00000000-0000-0002-0000-000000000000;price=1100;quantity=20;side=BUY;timestamp=1616823000000;time_in_force=GTC]";

        // Try parsing
        let parsed_queue = match OrderQueue::from_str(input) {
            Ok(q) => q,
            Err(e) => {
                info!("Parse error: {:?}", e);
                info!("Input string: {}", input);
                panic!("Failed to parse OrderQueue from string");
            }
        };

        // Verify the parsed queue
        assert!(!parsed_queue.is_empty());
        let orders = parsed_queue.to_vec();

        // Should have both orders
        assert_eq!(orders.len(), 2, "Expected 2 orders in parsed queue");

        // Verify individual orders (order might not be preserved)
        let has_order1 = orders.iter().any(|o| {
            o.id() == Id::from_u64(1)
                && o.price() == Price::new(1000)
                && o.visible_quantity() == Quantity::new(10)
        });
        let has_order2 = orders.iter().any(|o| {
            o.id() == Id::from_u64(2)
                && o.price() == Price::new(1100)
                && o.visible_quantity() == Quantity::new(20)
        });

        assert!(has_order1, "First order not found or incorrect");
        assert!(has_order2, "Second order not found or incorrect");

        // Test round-trip parsing
        let round_trip_queue = OrderQueue::from_str(&display_string).unwrap();
        let round_trip_orders = round_trip_queue.to_vec();

        assert_eq!(
            round_trip_orders.len(),
            2,
            "Round-trip parsing should preserve order count"
        );

        let round_trip_has_order1 = round_trip_orders.iter().any(|o| {
            o.id() == Id::from_u64(1)
                && o.price() == Price::new(1000)
                && o.visible_quantity() == Quantity::new(10)
        });
        let round_trip_has_order2 = round_trip_orders.iter().any(|o| {
            o.id() == Id::from_u64(2)
                && o.price() == Price::new(1100)
                && o.visible_quantity() == Quantity::new(20)
        });

        assert!(
            round_trip_has_order1,
            "First order not preserved in round-trip"
        );
        assert!(
            round_trip_has_order2,
            "Second order not preserved in round-trip"
        );
    }

    #[test]
    fn test_serialize_deserialize() {
        let queue = OrderQueue::new();
        queue.push(Arc::new(create_test_order(1, 1000u128, 10)));
        queue.push(Arc::new(create_test_order(2, 1100u128, 20)));

        // Serialize to JSON
        let serialized = serde_json::to_string(&queue).unwrap();
        info!("Serialized: {}", serialized);

        // Deserialize back
        let deserialized: OrderQueue = serde_json::from_str(&serialized).unwrap();

        // Verify
        let original_orders = queue.to_vec();
        let deserialized_orders = deserialized.to_vec();

        assert_eq!(original_orders.len(), deserialized_orders.len());

        // Since the order of orders might not be preserved, compare individual orders
        for order in original_orders {
            let found = deserialized_orders.iter().any(|o| o.id() == order.id());
            assert!(
                found,
                "Order with ID {} not found after deserialization",
                order.id()
            );
        }
    }

    #[test]
    fn test_round_trip() {
        let queue = OrderQueue::new();
        queue.push(Arc::new(create_test_order(1, 1000, 10)));

        // Convert to string
        let string_rep = queue.to_string();

        // Parse back from string
        let parsed_queue = match OrderQueue::from_str(&string_rep) {
            Ok(q) => q,
            Err(e) => {
                info!("Parse error: {:?}", e);
                panic!("Failed to parse: {string_rep}");
            }
        };

        // Verify
        let original_orders = queue.to_vec();
        let parsed_orders = parsed_queue.to_vec();

        assert_eq!(original_orders.len(), parsed_orders.len());
        assert_eq!(original_orders[0].id(), parsed_orders[0].id());
        assert_eq!(original_orders[0].price(), parsed_orders[0].price());
    }

    // In price_level/order_queue.rs test module or in a separate test file

    #[test]
    fn test_order_queue_to_vec_empty() {
        let queue = OrderQueue::new();

        // test_to_vec on empty queue
        let vec = queue.to_vec();
        assert!(vec.is_empty());

        // Verify queue is still empty after to_vec
        assert!(queue.is_empty());
    }

    #[test]
    fn test_order_queue_from_str_complex() {
        // Test with a complex order string format
        let complex_order = "Standard:id=00000000-0000-0001-0000-000000000000;price=10000;quantity=100;side=BUY;timestamp=1616823000000;time_in_force=GTD-1617000000000";

        let input = format!("OrderQueue:orders=[{complex_order}]");
        let queue = OrderQueue::from_str(&input).unwrap();

        assert_eq!(queue.len(), 1);

        // Verify the order's details
        let order = &queue.to_vec()[0];

        if let OrderType::<()>::Standard {
            id,
            price,
            quantity,
            time_in_force,
            ..
        } = **order
        {
            assert_eq!(id, Id::from_u64(1));
            assert_eq!(price, Price::new(10000));
            assert_eq!(quantity, Quantity::new(100));
            assert!(matches!(time_in_force, TimeInForce::Gtd(1617000000000)));
        } else {
            panic!("Expected Standard order");
        }
    }

    #[test]
    fn test_order_queue_from_str_invalid_order() {
        // Test with an invalid order format
        let input = "OrderQueue:orders=[InvalidOrder:id=1]";
        let result = OrderQueue::from_str(input);

        assert!(result.is_err());
    }

    #[test]
    fn test_order_queue_serialization() {
        fn create_standard_order(id: u64, price: u128, quantity: u64) -> OrderType<()> {
            OrderType::Standard {
                id: Id::from_u64(id),
                price: Price::new(price),
                quantity: Quantity::new(quantity),
                side: Side::Buy,
                user_id: Hash32::zero(),
                timestamp: TimestampMs::new(1616823000000),
                time_in_force: TimeInForce::Gtc,
                extra_fields: (),
            }
        }
        let queue = OrderQueue::new();

        // Add an order
        let order = create_standard_order(1, 10000u128, 100);
        queue.push(Arc::new(order));

        // Serialize
        let serialized = serde_json::to_string(&queue).unwrap();

        // Check that it contains the expected order data
        assert!(serialized.contains("\"Standard\""));
        assert!(serialized.contains("\"id\":\"00000000-0000-0001-0000-000000000000\""));
        assert!(serialized.contains("\"price\":10000"));
        assert!(serialized.contains("\"quantity\":100"));

        // Deserialize and verify
        let deserialized: OrderQueue = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.len(), 1);

        let deserialized_order = &deserialized.to_vec()[0];

        if let OrderType::Standard {
            id,
            price,
            quantity,
            ..
        } = **deserialized_order
        {
            assert_eq!(id, Id::from_u64(1));
            assert_eq!(price, Price::new(10000));
            assert_eq!(quantity, Quantity::new(100));
        } else {
            panic!("Expected Standard order");
        }
    }

    #[test]
    fn test_order_queue_empty_check() {
        // Test lines 123-124
        let queue = OrderQueue::new();

        // Queue should be empty initially
        assert!(queue.is_empty());

        // Add an order and check again
        let order = OrderType::Standard {
            id: Id::from_u64(1),
            price: Price::new(1000),
            quantity: Quantity::new(10),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000000),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        };
        queue.push(Arc::new(order));

        // Queue should not be empty after adding an order
        assert!(!queue.is_empty());

        // Remove the order and check again
        let _ = queue.pop();
        assert!(queue.is_empty());

        // Push the order back and then try a different approach to check emptiness
        queue.push(Arc::new(order));
        assert!(!queue.is_empty());
    }

    #[test]
    fn test_order_queue_from_vec() {
        // Test lines 170, 178
        // Create a vector of orders
        let order1 = Arc::new(OrderType::Standard {
            id: Id::from_u64(1),
            price: Price::new(1000),
            quantity: Quantity::new(10),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000000),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        });

        let order2 = Arc::new(OrderType::Standard {
            id: Id::from_u64(2),
            price: Price::new(1000),
            quantity: Quantity::new(20),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000001),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        });

        let orders = vec![order1.clone(), order2.clone()];

        // Create a queue from the vector
        let queue = OrderQueue::from_vec(orders.clone());

        // Verify the queue contains the orders
        assert_eq!(queue.to_vec().len(), 2);
        assert!(queue.to_vec().contains(&order1));
        assert!(queue.to_vec().contains(&order2));

        // Test the From implementation
        let queue_from_trait: OrderQueue = orders.clone().into();
        assert_eq!(queue_from_trait.to_vec().len(), 2);

        // Test the Into implementation
        let orders_from_queue: Vec<Arc<OrderType<()>>> = queue.into();
        assert_eq!(orders_from_queue.len(), 2);
        assert!(orders_from_queue.contains(&order1));
        assert!(orders_from_queue.contains(&order2));
    }

    #[test]
    fn test_order_queue_from_str_parsing_with_complex_content() {
        // Test lines 196-198, 200-202, 241, 266-267

        // Create a complex string with nested delimiters
        let complex_input = "OrderQueue:orders=[Standard:id=00000000-0000-0001-0000-000000000000;price=1000;quantity=10;side=BUY;timestamp=1616823000000;time_in_force=GTC,IcebergOrder:id=00000000-0000-0002-0000-000000000000;price=1000;visible_quantity=5;hidden_quantity=15;side=SELL;timestamp=1616823000001;time_in_force=GTC]";

        // Parse the complex input
        let result = OrderQueue::from_str(complex_input);
        assert!(result.is_ok());

        let queue = result.unwrap();
        assert_eq!(queue.to_vec().len(), 2);

        // Verify the parsed orders have the expected IDs
        let order_ids: Vec<Id> = queue.to_vec().iter().map(|order| order.id()).collect();
        assert!(order_ids.contains(&Id::from_u64(1)));
        assert!(order_ids.contains(&Id::from_u64(2)));

        // Test parsing with empty orders
        let empty_orders = "OrderQueue:orders=[]";
        let result = OrderQueue::from_str(empty_orders);
        assert!(result.is_ok());
        let queue = result.unwrap();
        assert!(queue.is_empty());

        // Test parsing with invalid format (no "OrderQueue:" prefix)
        let invalid_input = "orders=[Standard:id=1;price=1000;quantity=10;side=BUY;timestamp=1616823000000;time_in_force=GTC]";
        let result = OrderQueue::from_str(invalid_input);
        assert!(result.is_err());

        // Test parsing with malformed content (missing closing bracket)
        let malformed_input = "OrderQueue:orders=[Standard:id=00000000-0000-0001-0000-000000000000;price=1000;quantity=10;side=BUY;timestamp=1616823000000;time_in_force=GTC";
        let result = OrderQueue::from_str(malformed_input);
        assert!(result.is_err());

        // Test parsing with invalid order type
        let invalid_order = "OrderQueue:orders=[InvalidOrder:id=00000000-0000-0001-0000-000000000000;price=1000;quantity=10;side=BUY;timestamp=1616823000000;time_in_force=GTC]";
        let result = OrderQueue::from_str(invalid_order);
        assert!(result.is_err());
    }

    #[test]
    fn test_order_queue_serialization_deserialization() {
        // Create a queue with orders
        let queue = OrderQueue::new();

        let order1 = OrderType::Standard {
            id: Id::from_u64(1),
            price: Price::new(1000),
            quantity: Quantity::new(10),
            side: Side::Buy,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000000),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        };

        let order2 = OrderType::IcebergOrder {
            id: Id::from_u64(2),
            price: Price::new(1000),
            visible_quantity: Quantity::new(5),
            hidden_quantity: Quantity::new(15),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1616823000001),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        };

        queue.push(Arc::new(order1));
        queue.push(Arc::new(order2));

        // Serialize the queue
        let serialized = serde_json::to_string(&queue).unwrap();

        // Verify the serialized format contains the orders
        assert!(serialized.contains("\"id\":\"00000000-0000-0001-0000-000000000000\""));
        assert!(serialized.contains("\"id\":\"00000000-0000-0002-0000-000000000000\""));

        // Deserialize back to OrderQueue
        let deserialized: OrderQueue = serde_json::from_str(&serialized).unwrap();

        // Verify the deserialized queue has the same orders
        assert_eq!(deserialized.to_vec().len(), 2);

        // Verify the order IDs
        let order_ids: Vec<Id> = deserialized
            .to_vec()
            .iter()
            .map(|order| order.id())
            .collect();
        assert!(order_ids.contains(&Id::from_u64(1)));
        assert!(order_ids.contains(&Id::from_u64(2)));
    }

    #[test]
    fn test_order_queue_reinsert_keeps_front_priority() {
        // A arrives before B. Popping A (the head), then re-inserting it at its
        // original sequence must keep A ahead of B — modelling a partial fill
        // that preserves price-time priority.
        let queue = OrderQueue::new();
        queue.push(Arc::new(create_test_order(1, 1000u128, 10)));
        queue.push(Arc::new(create_test_order(2, 1000u128, 20)));

        let (seq_a, order_a) = match queue.pop_entry() {
            Some(entry) => entry,
            None => panic!("expected to pop order A"),
        };
        assert_eq!(order_a.id(), Id::from_u64(1));

        // Re-insert A's residual at its original sequence.
        queue.reinsert(seq_a, order_a);

        // The very next pop must still be A, not the later-arriving B.
        match queue.pop_entry() {
            Some((_, order)) => assert_eq!(
                order.id(),
                Id::from_u64(1),
                "re-inserted residual must keep front priority"
            ),
            None => panic!("expected to pop the re-inserted order A"),
        }
        match queue.pop() {
            Some(order) => assert_eq!(order.id(), Id::from_u64(2)),
            None => panic!("expected to pop order B"),
        }
        assert!(queue.pop().is_none());
    }

    #[test]
    fn test_order_queue_push_assigns_tail_priority() {
        // A re-pushed order (new sequence) lands behind everything else.
        let queue = OrderQueue::new();
        queue.push(Arc::new(create_test_order(1, 1000u128, 10)));
        queue.push(Arc::new(create_test_order(2, 1000u128, 20)));

        let order_a = match queue.pop() {
            Some(order) => order,
            None => panic!("expected to pop order A"),
        };
        assert_eq!(order_a.id(), Id::from_u64(1));

        // Re-push A: it gets a fresh (highest) sequence and goes to the tail.
        queue.push(order_a);

        match queue.pop() {
            Some(order) => assert_eq!(
                order.id(),
                Id::from_u64(2),
                "B should now be ahead of re-pushed A"
            ),
            None => panic!("expected to pop order B"),
        }
        match queue.pop() {
            Some(order) => assert_eq!(order.id(), Id::from_u64(1)),
            None => panic!("expected to pop re-pushed order A"),
        }
        assert!(queue.pop().is_none());
    }

    #[test]
    fn test_order_queue_remove_cleans_index() {
        // Removing an order must drop it from both the map and the ordered
        // index, so it never resurfaces from a later pop.
        let queue = OrderQueue::new();
        queue.push(Arc::new(create_test_order(1, 1000u128, 10)));
        queue.push(Arc::new(create_test_order(2, 1000u128, 20)));

        match queue.remove(Id::from_u64(1)) {
            Some(order) => assert_eq!(order.id(), Id::from_u64(1)),
            None => panic!("expected to remove order A"),
        }
        assert_eq!(queue.len(), 1);

        match queue.pop() {
            Some(order) => assert_eq!(
                order.id(),
                Id::from_u64(2),
                "removed order must not resurface"
            ),
            None => panic!("expected to pop order B"),
        }
        assert!(queue.pop().is_none());
    }

    #[test]
    fn test_concurrent_try_push_cancel_readmit_keeps_map_index_one_to_one() {
        // Finding 1 (PR #125): `try_push` published the map entry, released the
        // shard lock, then inserted the index entry. A concurrent cancel +
        // same-id readmission landing in that window could leave two index
        // entries for one id (a FIFO jump). `try_push_with` now holds the shard
        // lock across BOTH publications, so the map and the index stay strictly
        // 1:1 under an admit / cancel / readmit storm on a tiny, heavily-reused
        // id set. Assert that invariant at quiescence over many iterations.
        use std::sync::{Arc as StdArc, Barrier};
        use std::thread;

        const THREADS: usize = 6;
        const OPS: usize = 300;
        const IDS: u64 = 4;

        for iter in 0..40 {
            let queue = StdArc::new(OrderQueue::new());
            let barrier = StdArc::new(Barrier::new(THREADS));

            let handles: Vec<_> = (0..THREADS)
                .map(|t| {
                    let queue = StdArc::clone(&queue);
                    let barrier = StdArc::clone(&barrier);
                    thread::spawn(move || {
                        barrier.wait();
                        for op in 0..OPS {
                            // A tiny id set shared across threads maximizes the
                            // chance an admission of `id` races a cancel of the
                            // same `id`.
                            let id = ((op as u64).wrapping_add(t as u64) % IDS) + 1;
                            let order = StdArc::new(create_test_order(id, 1_000, 10));
                            let _ = queue.try_push(order);
                            let _ = queue.remove(Id::from_u64(id));
                            let _ = queue.try_push(StdArc::new(create_test_order(id, 1_000, 10)));
                        }
                    })
                })
                .collect();
            for h in handles {
                h.join().expect("thread panicked");
            }

            // Quiescent: every started operation has completed, so the map and
            // the ordered index must be exactly 1:1. A split index entry from
            // the old publication race would make the index longer than the map.
            assert!(
                queue.debug_map_index_consistent(),
                "iter {iter}: id-keyed map and ordered index must be 1:1"
            );

            // Draining pops each resting id exactly once: a phantom second index
            // entry would resurface an id or desync the count.
            let mut popped: Vec<Id> = Vec::new();
            while let Some(order) = queue.pop() {
                popped.push(order.id());
            }
            let unique: std::collections::HashSet<Id> = popped.iter().copied().collect();
            assert_eq!(
                popped.len(),
                unique.len(),
                "iter {iter}: every id must be popped at most once"
            );
            assert!(queue.is_empty(), "iter {iter}: queue must fully drain");
        }
    }

    #[test]
    fn test_resequence_vs_front_scan_never_empty() {
        // Issue #127 🔴: the index re-key inserts the new key BEFORE removing the
        // old, so a resident maker is never transiently absent from the index. A
        // front scan (`match_front`) racing continuous demotions of that maker
        // must therefore NEVER return `Empty` — the liquidity is always resting.
        // Under the old remove-then-insert order the maker vanished from the
        // index between the two ops and a scan could miss it.
        use crate::price_level::order_queue::{FrontAction, FrontOutcome};
        use std::collections::HashSet;
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::thread;

        let queue = StdArc::new(OrderQueue::new());
        // The single resident maker (id 1) never leaves the queue.
        queue.push(StdArc::new(create_test_order(1, 1_000, 100)));
        let done = StdArc::new(AtomicBool::new(false));

        let demoter = {
            let queue = StdArc::clone(&queue);
            let done = StdArc::clone(&done);
            thread::spawn(move || {
                while !done.load(Ordering::Relaxed) {
                    // Demote the resident maker to a fresh tail sequence, over and
                    // over. It stays resident the whole time.
                    let _ = queue.resequence_to_tail(
                        Id::from_u64(1),
                        StdArc::new(create_test_order(1, 1_000, 100)),
                    );
                }
            })
        };

        for _ in 0..200_000 {
            let mut set_aside = HashSet::new();
            // A no-op probe: whatever the front is, park it (leaves it resting)
            // and report we found one. The maker always rests, so this must be
            // `Matched`, never `Empty`.
            let outcome =
                queue.match_front(&mut set_aside, |_seq, _order| (FrontAction::SetAside, ()));
            assert!(
                matches!(outcome, FrontOutcome::Matched { .. }),
                "front scan returned Empty while the resident maker rests (issue #127)"
            );
        }

        done.store(true, Ordering::Relaxed);
        demoter.join().expect("demoter thread panicked");
        assert_eq!(queue.len(), 1, "the resident maker still rests");
        assert!(
            queue.debug_map_index_consistent(),
            "map and index must be 1:1 after the demotions"
        );
    }

    #[test]
    fn test_pop_entry_after_resequence_orders_last_and_drains_clean() {
        // Issue #127 🔴: after a maker is demoted, `pop_entry` must return it
        // LAST (at its new tail sequence), never early via a stale old key, and a
        // full drain must leave BOTH the map and the ordered index empty.
        let queue = OrderQueue::new();
        queue.push(Arc::new(create_test_order(1, 1_000, 10))); // seq 0
        queue.push(Arc::new(create_test_order(2, 1_000, 20))); // seq 1 (to demote)
        queue.push(Arc::new(create_test_order(3, 1_000, 30))); // seq 2

        let replaced =
            queue.resequence_to_tail(Id::from_u64(2), Arc::new(create_test_order(2, 1_000, 25)));
        assert!(replaced.is_some(), "the demoted maker was resident");
        // New-before-old re-key leaves no stale old key: map and index are 1:1.
        assert!(queue.debug_map_index_consistent());

        let mut ids = Vec::new();
        while let Some((_, order)) = queue.pop_entry() {
            ids.push(order.id());
        }
        // FIFO by CURRENT sequence: 1, 3, then the demoted 2 LAST.
        assert_eq!(
            ids,
            vec![Id::from_u64(1), Id::from_u64(3), Id::from_u64(2)],
            "the demoted maker must pop last, never via its stale old key"
        );
        // Fully drained: map and index both empty (and consistent).
        assert!(queue.is_empty());
        assert!(
            queue.debug_map_index_consistent(),
            "no stale index entry may survive a full drain"
        );
    }

    #[test]
    fn test_pop_entry_vs_resequence_race_drains_each_once() {
        // Issue #127 🔴: `pop_entry` validates the stored sequence against the
        // popped key, so under continuous demotions racing a drain, every id
        // comes out EXACTLY once (never twice via a stale old key, never lost),
        // and the queue drains clean.
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::thread;

        const N: u64 = 50;

        for iter in 0..400 {
            let queue = StdArc::new(OrderQueue::new());
            for id in 1..=N {
                queue.push(StdArc::new(create_test_order(id, 1_000, 10)));
            }
            let done = StdArc::new(AtomicBool::new(false));
            let demoter = {
                let queue = StdArc::clone(&queue);
                let done = StdArc::clone(&done);
                thread::spawn(move || {
                    while !done.load(Ordering::Relaxed) {
                        // Continuously demote a mid maker; once it is popped this
                        // returns `None` (id gone) and simply spins.
                        let _ = queue.resequence_to_tail(
                            Id::from_u64(N / 2),
                            StdArc::new(create_test_order(N / 2, 1_000, 10)),
                        );
                    }
                })
            };

            let mut popped: Vec<Id> = Vec::new();
            while let Some((_, order)) = queue.pop_entry() {
                popped.push(order.id());
            }

            done.store(true, Ordering::Relaxed);
            demoter.join().expect("demoter thread panicked");

            let unique: std::collections::HashSet<Id> = popped.iter().copied().collect();
            assert_eq!(
                unique.len(),
                popped.len(),
                "iter {iter}: every id popped at most once (no stale-key double pop)"
            );
            assert_eq!(
                popped.len() as u64,
                N,
                "iter {iter}: all ids drained exactly once (none lost)"
            );
            assert!(
                queue.is_empty() && queue.debug_map_index_consistent(),
                "iter {iter}: clean drain, map and index both empty"
            );
        }
    }
}
