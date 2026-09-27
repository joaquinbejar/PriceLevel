//! Issue #206: a looping fill-or-kill matcher neither breaks FIFO nor
//! starves the mutators waiting on the level's guard.
//!
//! The fill-or-kill guard hands the lock to announced mutators before it
//! retakes the exclusive side (see `price_level::fok_guard`). These tests
//! drive that protocol through the real `PriceLevel` with deterministic
//! seams instead of timing: the fill-or-kill-locked hook observes the level
//! while the exclusive side is held, the hand-off tallies count what the
//! protocol did, and every cross-thread step waits on an atomic condition,
//! never on a sleep.
//!
//! Hook order changed with the hand-off: the pre-fill-or-kill-lock hook
//! (`set_pre_fok_lock_hook`, issue #164) still fires before the exclusive
//! guard is requested, which now also means before the hand-off wait, so a
//! mutator announced by that hook is waited for by the same call.
//!
//! The looping test counts sections that run while a mutator is announced;
//! each announcement can overlap at most one (the one whose counter check
//! preceded it). The mutator's full wait, counted from its failed
//! `try_read`, can also include the section in progress at that moment,
//! hence the documented bound of two.

#[cfg(test)]
mod tests {
    use crate::UuidGenerator;
    use crate::execution::{MatchResult, TakerKind};
    use crate::orders::{Hash32, Id, OrderType, OrderUpdate, Side, TimeInForce};
    use crate::price_level::fok_guard::{announce_tally, handoff_tally, override_handoff_yields};
    use crate::price_level::level::{PriceLevel, set_fok_locked_hook};
    use crate::utils::{Price, Quantity, TimestampMs};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{Duration, Instant};
    use uuid::Uuid;

    const PRICE: u128 = 10_000;
    const TAKER: u64 = u64::MAX;
    /// Writer ids, disjoint from every matcher maker id.
    const WRITER_BASE: u64 = 1_000_000_000;
    /// A hand-off budget no test run exhausts, so the matcher waits until
    /// every announced mutator holds the shared side.
    const UNBOUNDED: u32 = u32::MAX;

    fn standard(id: u64) -> OrderType<()> {
        OrderType::Standard {
            id: Id::from_u64(id),
            price: Price::new(PRICE),
            quantity: Quantity::new(1),
            side: Side::Sell,
            user_id: Hash32::zero(),
            timestamp: TimestampMs::new(1),
            time_in_force: TimeInForce::Gtc,
            extra_fields: (),
        }
    }

    fn level_of(depth: u64) -> PriceLevel {
        let level = PriceLevel::new(PRICE);
        for id in 0..depth {
            level.add_order(standard(id)).expect("seed");
        }
        level
    }

    fn fok(level: &PriceLevel, quantity: u64, generator: &UuidGenerator) -> MatchResult {
        level.match_order(
            quantity,
            Id::from_u64(TAKER),
            TimeInForce::Fok,
            TakerKind::Standard,
            TimestampMs::new(2),
            generator,
        )
    }

    fn only_maker(result: &MatchResult) -> Id {
        let trades = result.trades().as_vec();
        assert_eq!(trades.len(), 1, "a qty-1 FOK fills exactly one maker");
        trades[0].maker_order_id()
    }

    fn resting(level: &PriceLevel, id: Id) -> bool {
        level.iter_orders().any(|order| order.id() == id)
    }

    /// Spin (no sleep) until `condition` holds.
    fn spin_until(mut condition: impl FnMut() -> bool) {
        while !condition() {
            std::hint::spin_loop();
        }
    }

    #[test]
    fn test_fok_handoff_uncontended_takes_no_detour() {
        let level = level_of(4);
        let generator = UuidGenerator::new(Uuid::nil());
        let before = (handoff_tally(), announce_tally());

        level.add_order(standard(10)).expect("add");
        let cancelled = level.update_order(OrderUpdate::Cancel {
            order_id: Id::from_u64(10),
        });
        assert!(matches!(cancelled, Ok(Some(_))));
        let result = fok(&level, 1, &generator);

        assert_eq!(only_maker(&result), Id::from_u64(0));
        assert_eq!(
            (handoff_tally(), announce_tally()),
            before,
            "no mutator blocked, so none announced and the FOK never waited"
        );
        assert_eq!(level.test_fok_waiting_mutators(), 0);
    }

    #[test]
    fn test_fok_handoff_budget_bounds_the_wait_on_a_mutator_that_never_arrives() {
        // A phantom announcement looks exactly like a mutator preempted
        // before it acquires: the FOK must give up after its budget and
        // still be all-or-nothing, never deadlock.
        let level = level_of(3);
        let generator = UuidGenerator::new(Uuid::nil());
        let (waited, exhausted) = handoff_tally();
        let result = {
            let _phantom = level.test_fok_announce();
            assert_eq!(level.test_fok_waiting_mutators(), 1);
            fok(&level, 2, &generator)
        };
        assert_eq!(handoff_tally(), (waited + 1, exhausted + 1));
        assert_eq!(
            level.test_fok_waiting_mutators(),
            0,
            "announcement withdrawn"
        );
        assert_eq!(result.executed_quantity(), Ok(Quantity::new(2)));
        assert_eq!(level.order_count(), 1);

        // A killed FOK behind the same phantom leaves the level untouched.
        let killed = {
            let _phantom = level.test_fok_announce();
            fok(&level, 5, &generator)
        };
        assert!(killed.was_killed());
        assert_eq!(level.order_count(), 1);
        assert!(resting(&level, Id::from_u64(2)));
    }

    #[test]
    fn test_fok_handoff_admits_blocked_writer_before_next_fok() {
        let level = Arc::new(level_of(8));
        let held = Arc::new(AtomicBool::new(false));
        let writer_id = Id::from_u64(WRITER_BASE);

        let writer = {
            let level = Arc::clone(&level);
            let held = Arc::clone(&held);
            thread::spawn(move || {
                spin_until(|| held.load(Ordering::SeqCst));
                // Blocks: the first FOK holds the exclusive side.
                level.add_order(standard(WRITER_BASE)).expect("writer add");
                announce_tally()
            })
        };

        let generator = UuidGenerator::new(Uuid::nil());
        let _budget = override_handoff_yields(UNBOUNDED);
        let observed = Rc::new(RefCell::new(Vec::new()));
        let _hook = {
            let level = Arc::clone(&level);
            let held = Arc::clone(&held);
            let observed = Rc::clone(&observed);
            set_fok_locked_hook(Box::new(move || {
                let call = observed.borrow().len();
                if call == 0 {
                    // Hold the exclusive side until the writer has tried the
                    // shared side, failed and announced itself.
                    held.store(true, Ordering::SeqCst);
                    spin_until(|| level.test_fok_waiting_mutators() == 1);
                }
                observed.borrow_mut().push((
                    resting(&level, writer_id),
                    level.test_fok_waiting_mutators(),
                ));
            }))
        };
        let (waited, exhausted) = handoff_tally();

        let first = fok(&level, 1, &generator);
        // Back-to-back, exactly the barging pattern of issue #206.
        let second = fok(&level, 1, &generator);
        let writer_announcements = writer.join().expect("writer");

        assert_eq!(only_maker(&first), Id::from_u64(0));
        assert_eq!(only_maker(&second), Id::from_u64(1));
        assert_eq!(writer_announcements, 1, "the writer blocked once");
        assert_eq!(
            *observed.borrow(),
            vec![(false, 1), (true, 0)],
            "the second FOK took the exclusive side only after the waiting writer's admission"
        );
        // The writer may win the lock between the two calls on its own (no
        // hand-off needed) or be waited for once; either way the budget is
        // never exhausted and the state checks above hold.
        let (waited_after, exhausted_after) = handoff_tally();
        assert!(waited_after - waited <= 1, "at most one hand-off");
        assert_eq!(exhausted_after, exhausted, "drained within budget");
        assert!(resting(&level, writer_id));
        assert_eq!(level.order_count(), 7);
    }

    /// What the fill-or-kill-locked hook saw per call: the true front and the
    /// number of announced mutators, both under the exclusive guard.
    type Observed = Rc<RefCell<Vec<(Option<(u64, Id)>, usize)>>>;

    /// One sample of the looping-matcher test: what the matcher saw under
    /// the exclusive guard, and what it then consumed.
    struct FokStep {
        front: Option<(u64, Id)>,
        consumed: Id,
    }

    #[test]
    fn test_looping_fok_consumes_true_front_and_bounds_writer_wait() {
        const DEPTH: u64 = 32;
        const WRITER_OPS: u64 = 400;

        let level = Arc::new(level_of(DEPTH));
        let start = Arc::new(Barrier::new(2));
        let stop = Arc::new(AtomicBool::new(false));
        let fok_done = Arc::new(AtomicU64::new(0));

        let matcher = {
            let level = Arc::clone(&level);
            let start = Arc::clone(&start);
            let stop = Arc::clone(&stop);
            let fok_done = Arc::clone(&fok_done);
            thread::spawn(move || {
                let generator = UuidGenerator::new(Uuid::nil());
                let _budget = override_handoff_yields(UNBOUNDED);
                // (front under the guard, mutators announced under the guard)
                let seen: Observed = Rc::new(RefCell::new(Vec::new()));
                let _hook = {
                    let level = Arc::clone(&level);
                    let seen = Rc::clone(&seen);
                    set_fok_locked_hook(Box::new(move || {
                        seen.borrow_mut()
                            .push((level.test_front(), level.test_fok_waiting_mutators()));
                    }))
                };
                let mut steps = Vec::new();
                let mut next = DEPTH;
                start.wait();
                while !stop.load(Ordering::SeqCst) {
                    let result = fok(&level, 1, &generator);
                    let consumed = only_maker(&result);
                    let (front, _) = *seen.borrow().last().expect("hook fired");
                    steps.push(FokStep { front, consumed });
                    fok_done.fetch_add(1, Ordering::SeqCst);
                    level.add_order(standard(next)).expect("replacement");
                    next += 1;
                }
                let overlapped = seen.borrow().iter().filter(|(_, n)| *n > 0).count();
                (steps, overlapped, handoff_tally().1)
            })
        };

        start.wait();
        let announced_before = announce_tally();
        let mut max_wait = Duration::ZERO;
        let mut max_span: u64 = 0;
        let mut cancelled_missing = Vec::new();
        for i in 0..WRITER_OPS {
            let id = WRITER_BASE + i;
            let before = fok_done.load(Ordering::SeqCst);
            let t0 = Instant::now();
            level.add_order(standard(id)).expect("writer add");
            let cancelled = level.update_order(OrderUpdate::Cancel {
                order_id: Id::from_u64(id),
            });
            max_wait = max_wait.max(t0.elapsed());
            max_span = max_span.max(fok_done.load(Ordering::SeqCst) - before);
            match cancelled {
                Ok(Some(_)) => {}
                Ok(None) => cancelled_missing.push(Id::from_u64(id)),
                Err(err) => panic!("writer cancel failed: {err}"),
            }
        }
        let announcements = announce_tally() - announced_before;
        stop.store(true, Ordering::SeqCst);
        let (steps, overlapped, exhausted) = matcher.join().expect("matcher");

        // FIFO: every FOK consumed the true front it saw under the exclusive
        // guard, and those fronts only ever move forward in sequence.
        assert!(!steps.is_empty());
        let mut last_seq = None;
        for (i, step) in steps.iter().enumerate() {
            let (seq, id) = step.front.expect("the level is never empty");
            assert_eq!(
                step.consumed, id,
                "FOK {i} consumed a maker other than the true front"
            );
            assert!(
                last_seq < Some(seq),
                "FOK {i}: front sequence went backwards"
            );
            last_seq = Some(seq);
        }
        // A writer order the matcher consumed was, by the loop above, the
        // true front; its late cancel is the only way a cancel finds nothing.
        for id in &cancelled_missing {
            assert!(
                steps.iter().any(|s| s.consumed == *id),
                "cancel of {id} found nothing, yet no FOK consumed it"
            );
        }

        // Writer wait, counted in exclusive sections rather than time: with
        // an unbounded budget, a FOK that checks after an announcement waits
        // until that mutator holds the shared side, so the only section that
        // can run while a mutator is announced is the single one whose check
        // preceded the announcement.
        assert_eq!(exhausted, 0, "unbounded budget never runs out");
        assert!(
            overlapped as u64 <= announcements,
            "{overlapped} exclusive sections ran while a mutator waited, \
             more than its {announcements} announcements"
        );
        // Wall-clock wait is scheduler-dependent, so it is only bounded
        // loosely here: the pre-#206 failure mode was seconds.
        assert!(
            max_wait < Duration::from_secs(5),
            "writer add+cancel waited {max_wait:?} across {max_span} FOK calls"
        );
    }
}
