//! Independent accounting observations over funding trees.

use skein_lib::List;

use crate::{Funder, Funding, Moved, Numbers, carve, charge, left, move_funding, settle};

fn add(a: u64, b: u64) -> u64 {
    a.checked_add(b).unwrap()
}

fn subtract(a: u64, b: u64) -> u64 {
    a.checked_sub(b).unwrap()
}

fn multiply(a: u64, b: u64) -> u64 {
    a.checked_mul(b).unwrap()
}

fn remainder(a: u64, b: u64) -> u64 {
    a.checked_rem(b).unwrap()
}

fn divide(a: u64, b: u64) -> u64 {
    a.checked_div(b).unwrap()
}

fn fresh(budget: u64) -> Numbers {
    Numbers { budget, spent: 0, spent_below: 0, reserved: 0 }
}

fn changed(moved: Moved) -> (Funding, Funding, Numbers) {
    match moved {
        Moved::Changed { old, new, task } => (old, new, task),
        Moved::Same { .. } => panic!("different funders must transfer"),
    }
}

#[test]
fn accounting_refusals_and_overruns_leave_exact_values() {
    let start = fresh(10);
    assert_eq!(carve(start, &[6, 5]), None, "a batch cannot exceed what is available");
    assert_eq!(carve(start, &[u64::MAX, 1]), None, "the sum of reservations must be representable");
    let reserved = carve(start, &[6, 4]).unwrap();
    assert_eq!(left(reserved), 0);
    let charged = charge(reserved, 3).unwrap();
    assert_eq!(charged.overrun, 3);
    assert_eq!(charged.numbers.spent, 3);
    assert_eq!(charged.numbers.reserved, 10);
    assert_eq!(left(charged.numbers), 0);
    assert_eq!(settle(start, fresh(1)), None);
    assert_eq!(settle(reserved, reserved), None);
    let huge = Numbers { spent: u64::MAX, reserved: u64::MAX, ..fresh(u64::MAX) };
    assert_eq!(left(huge), 0);
    assert_eq!(charge(huge, 1), None);
    let impossible = Numbers { spent: u64::MAX, spent_below: 1, ..fresh(u64::MAX) };
    assert_eq!(carve(impossible, &[]), None);
    assert_eq!(charge(impossible, 0), None);
    assert_eq!(settle(fresh(0), impossible), None);
    let parent = Numbers { spent: u64::MAX, reserved: 1, ..fresh(u64::MAX) };
    assert_eq!(settle(parent, charge(fresh(1), 1).unwrap().numbers), None);
    assert_eq!(start, fresh(10));
}

#[test]
fn every_expense_is_counted_once_up_generated_funding_trees() {
    let mut leaf_overruns = 0_u64;
    let mut subtree_overruns = 0_u64;
    for seed in 0..64_u64 {
        let mut root = fresh(10_000);
        let mut expenses = List::with_capacity(17);
        let mut funded = List::with_capacity(4);
        for child in 0..4_u64 {
            let budget = add(100, remainder(add(multiply(seed, 13), multiply(child, 19)), 97));
            root = carve(root, &[budget]).unwrap();
            let mut budgets = List::with_capacity(3);
            for leaf in 0..3_u64 {
                let amount = add(10, remainder(add(add(seed, child), leaf), 13));
                budgets.push(amount).unwrap();
            }
            let mut node = carve(fresh(budget), budgets.as_slice()).unwrap();
            for (leaf, amount) in budgets.iter().enumerate() {
                let actual = remainder(
                    add(add(multiply(seed, 7), multiply(child, 3)), multiply(u64::try_from(leaf).unwrap(), 17)),
                    add(multiply(*amount, 2), 1),
                );
                expenses.push(actual).unwrap();
                let charged = charge(fresh(*amount), actual).unwrap();
                if charged.overrun > 0 {
                    leaf_overruns = add(leaf_overruns, 1);
                }
                node = settle(node, charged.numbers).unwrap();
            }
            let actual = remainder(add(multiply(seed, 11), child), add(budget, 1));
            expenses.push(actual).unwrap();
            let charged = charge(node, actual).unwrap();
            if charged.overrun > 0 {
                subtree_overruns = add(subtree_overruns, 1);
            }
            node = charged.numbers;
            funded.push(node).unwrap();
        }
        for ended in &funded {
            root = settle(root, *ended).unwrap();
        }
        root = charge(root, 17).unwrap().numbers;
        expenses.push(17).unwrap();
        let mut expected = 0_u64;
        for expense in &expenses {
            expected = add(expected, *expense);
        }
        assert_eq!(add(root.spent, root.spent_below), expected, "seed {seed}");
        assert_eq!(root.reserved, 0, "seed {seed}");
        assert_eq!(left(root), 10_000_u64.saturating_sub(expected), "seed {seed}");
    }
    assert!(leaf_overruns > 0 && subtree_overruns > 0, "the generated trees exercise overruns at both levels");
}

#[test]
fn moving_live_funded_delegates_returns_old_spend_and_reserves_future_spend() {
    for seed in 0..48_u64 {
        let old_by = Funder::Task(7);
        let new_by = Funder::Pool { project: 2, person: 9, period: 4 };
        let old = Funding { by: old_by, numbers: carve(fresh(500), &[100]).unwrap() };
        let new = Funding { by: new_by, numbers: fresh(500) };
        let mut task = carve(fresh(100), &[30, 20]).unwrap();
        let own = remainder(seed, 11);
        task = charge(task, own).unwrap().numbers;
        let a_spent = remainder(seed, 19);
        let b_spent = remainder(seed, 13);
        let a = charge(fresh(30), a_spent).unwrap().numbers;
        let b = charge(fresh(20), b_spent).unwrap().numbers;
        let mut settled = settle(task, a).unwrap();
        settled = settle(settled, b).unwrap();
        let a_left = subtract(30, a_spent);
        let b_left = subtract(20, b_spent);
        let (old_after, mut new_after, mut reopened) =
            changed(move_funding(old, new, settled, &[a_left, b_left]).unwrap());
        let previous_expenses = add(add(own, a_spent), b_spent);
        assert_eq!(old_after.numbers.spent_below, previous_expenses);
        assert_eq!(old_after.numbers.reserved, 0);
        assert_eq!(new_after.numbers.reserved, subtract(100, previous_expenses));
        assert_eq!(reopened.spent, 0);
        assert_eq!(reopened.spent_below, 0);
        assert_eq!(left(reopened), left(task));
        let a_future = divide(a_left, 2);
        let b_future = divide(b_left, 2);
        reopened = settle(reopened, charge(fresh(a_left), a_future).unwrap().numbers).unwrap();
        reopened = settle(reopened, charge(fresh(b_left), b_future).unwrap().numbers).unwrap();
        let own_future = divide(left(reopened), 3);
        reopened = charge(reopened, own_future).unwrap().numbers;
        new_after.numbers = settle(new_after.numbers, reopened).unwrap();
        assert_eq!(new_after.numbers.reserved, 0);
        let future_expenses = add(add(a_future, b_future), own_future);
        assert_eq!(new_after.numbers.spent_below, future_expenses);
        assert_eq!(
            add(old_after.numbers.spent_below, new_after.numbers.spent_below),
            add(previous_expenses, future_expenses),
            "expenses before and after the move are counted once"
        );
    }
}

#[test]
fn moving_refuses_lost_reservations_and_unfunded_promises() {
    let old = Funding { by: Funder::Task(1), numbers: carve(fresh(100), &[100]).unwrap() };
    let new = Funding { by: Funder::Pool { project: 1, person: 2, period: 3 }, numbers: fresh(100) };
    let task = carve(fresh(100), &[50, 50]).unwrap();
    assert_eq!(move_funding(old, new, task, &[]), None, "live old reservations cannot disappear");
    let mut settled = settle(task, charge(fresh(50), 80).unwrap().numbers).unwrap();
    settled = settle(settled, fresh(50)).unwrap();
    assert_eq!(
        move_funding(old, new, settled, &[0, 50]),
        None,
        "root has 20 left but its surviving delegate is still promised 50"
    );
    assert_eq!(old.numbers.reserved, 100);
    assert_eq!(new.numbers.reserved, 0);
    let poor = Funding { numbers: fresh(19), ..new };
    assert_eq!(move_funding(old, poor, settled, &[]), None);
    let same = move_funding(old, old, task, &[]).unwrap();
    match same {
        Moved::Same { funding, task: kept } => {
            assert_eq!(funding, old);
            assert_eq!(kept, task);
        }
        Moved::Changed { .. } => panic!("same funder must keep its current allotment"),
    }
    assert_eq!(move_funding(old, Funding { numbers: fresh(100), ..old }, task, &[]), None);
    assert_eq!(move_funding(old, old, task, &[50, 50]), None, "same-funder moves skip normalization");

    let exhausted = charge(fresh(100), 110).unwrap().numbers;
    let (old_after, new_after, reopened) = changed(move_funding(old, new, exhausted, &[]).unwrap());
    assert_eq!(old_after.numbers.spent_below, 110, "old overrun history remains charged to the old funder");
    assert_eq!(new_after.numbers.reserved, 0);
    assert_eq!(reopened, fresh(0), "a fully spent task can move with no future budget");

    for seed in 0..32_u64 {
        let a_budget = add(20, remainder(seed, 11));
        let b_budget = add(20, remainder(seed, 13));
        let budget = add(a_budget, b_budget);
        let old = Funding { numbers: carve(fresh(100), &[budget]).unwrap(), ..old };
        let task = carve(fresh(budget), &[a_budget, b_budget]).unwrap();
        let a_expense = add(a_budget, add(1, remainder(seed, 17)));
        let task = settle(task, charge(fresh(a_budget), a_expense).unwrap().numbers).unwrap();
        let task = settle(task, fresh(b_budget)).unwrap();
        assert_eq!(
            move_funding(old, new, task, &[0, b_budget]),
            None,
            "seed {seed}: an old overrun cannot consume a live task's promised replacement"
        );
    }
}

#[test]
fn a_new_period_does_not_clear_the_reservations_of_its_predecessor() {
    let old = Funding { by: Funder::Period { project: 3, period: 7 }, numbers: carve(fresh(100), &[80]).unwrap() };
    let current = Funding { by: Funder::Period { project: 3, period: 8 }, numbers: fresh(100) };
    assert_eq!(left(old.numbers), 20);
    assert_eq!(left(current.numbers), 100);
    let ended = charge(fresh(80), 35).unwrap().numbers;
    let settled = settle(old.numbers, ended).unwrap();
    assert_eq!(settled.spent_below, 35);
    assert_eq!(left(settled), 65);
    assert_eq!(current.numbers, fresh(100));
    let overrunning = charge(fresh(80), 110).unwrap().numbers;
    let settled = settle(old.numbers, overrunning).unwrap();
    assert_eq!(settled.spent_below, 110);
    assert_eq!(left(settled), 0);
    assert_eq!(current.numbers.reserved, 0);

    let (old_after, new_after, task) = changed(move_funding(old, current, ended, &[]).unwrap());
    assert_eq!(old_after.by, old.by, "returned budget and historical spend keep their original period");
    assert_eq!(old_after.numbers.spent_below, 35);
    assert_eq!(old_after.numbers.reserved, 0);
    assert_eq!(new_after.by, current.by);
    assert_eq!(new_after.numbers.reserved, 45, "an explicit move is reserved in the new period");
    assert_eq!(task, fresh(45));
    let old_pool = Funding { by: Funder::Pool { project: 3, person: 8, period: 7 }, numbers: old.numbers };
    let new_pool = Funding { by: Funder::Pool { project: 3, person: 8, period: 8 }, numbers: current.numbers };
    assert!(
        move_funding(old_pool, new_pool, ended, &[]).is_some(),
        "same person's pools in different periods are different actual funders"
    );
}

#[test]
fn accepted_delegates_with_external_task_funders_need_separate_transfers() {
    // T is C's delegate; U is T's delegate, but an accepted proposal had C
    // fund U directly. Moving only T does not release C's reservation of U.
    let original_pool = carve(fresh(1_000), &[500]).unwrap();
    let old = Funding { by: Funder::Task(7), numbers: carve(fresh(500), &[100, 40]).unwrap() };
    let new = Funding { by: Funder::Pool { project: 2, person: 9, period: 4 }, numbers: fresh(500) };
    let t = charge(fresh(100), 20).unwrap().numbers;
    let u = charge(fresh(40), 5).unwrap().numbers;
    let (old_after_t, new_after_t, t) = changed(move_funding(old, new, t, &[]).unwrap());
    assert_eq!(old_after_t.numbers.reserved, 40, "the external delegate's old allocation remains live");
    assert_eq!(settle(original_pool, old_after_t.numbers), None, "the old chat cannot end yet");
    let (old_after_u, mut new_after_u, u) = changed(move_funding(old_after_t, new_after_t, u, &[]).unwrap());
    let original_pool = settle(original_pool, old_after_u.numbers).unwrap();
    assert_eq!(original_pool.spent_below, 25, "old allocations can now close exactly once up the old chain");
    assert_eq!(new_after_u.numbers.reserved, 115, "both independent future allotments are funded by the new pool");
    new_after_u.numbers = settle(new_after_u.numbers, charge(t, 10).unwrap().numbers).unwrap();
    new_after_u.numbers = settle(new_after_u.numbers, charge(u, 15).unwrap().numbers).unwrap();
    assert_eq!(new_after_u.numbers.reserved, 0);
    assert_eq!(new_after_u.numbers.spent_below, 25);
    assert_eq!(add(original_pool.spent_below, new_after_u.numbers.spent_below), 50, "all four expenses counted once");
}

#[test]
fn repeated_moves_keep_historical_run_expenses_out_of_new_allotments() {
    let first = Funding { by: Funder::Task(1), numbers: carve(fresh(100), &[100]).unwrap() };
    let second = Funding { by: Funder::Pool { project: 2, person: 3, period: 4 }, numbers: fresh(100) };
    let third = Funding { by: Funder::Pool { project: 2, person: 3, period: 5 }, numbers: fresh(100) };
    let committed_run_spend = 20_u64;
    let task = charge(fresh(100), committed_run_spend).unwrap().numbers;
    let (first, second, task) = changed(move_funding(first, second, task, &[]).unwrap());
    assert_eq!(task.spent, 0);
    let answer_whole = 30_u64;
    let delta = subtract(answer_whole, committed_run_spend);
    let task = charge(task, delta).unwrap().numbers;
    let (second, third, task) = changed(move_funding(second, third, task, &[]).unwrap());
    let third = settle(third.numbers, charge(task, 7).unwrap().numbers).unwrap();
    assert_eq!(first.numbers.spent_below, 20);
    assert_eq!(second.numbers.spent_below, 10, "the run's answer adds only its uncommitted difference");
    assert_eq!(third.spent_below, 7, "the final allotment charges only future expense");
    assert_eq!(add(add(first.numbers.spent_below, second.numbers.spent_below), third.spent_below), 37);
}
