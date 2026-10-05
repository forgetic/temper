use temper_engine_domain_tasks::{
    self as tasks, Authorization, Balance, Event, Funder, Movement, Numbers, Party, Transfer,
};
use temper_engine_tasks_world::{LIMITS, World, task};
#[test]
fn tiny_actual_funding_trees_keep_unspent_and_cumulative_spend_over_moves() {
    for seed in 0..16 {
        let mut w = World::new(seed, LIMITS);
        w.open_period(2, 1000);
        w.carve_pool(2, 100);
        let mut root = task(1, &[]);
        root.numbers.budget = 1000;
        root.authority.budget.spend = 1000;
        w.make(Party::Person(1), vec![root]);
        let mut middle = task(2, &[]);
        middle.funder = Funder::Task(1);
        w.make(Party::Task(1), vec![middle]);
        let mut leaf = task(3, &[]);
        leaf.funder = Funder::Task(2);
        leaf.numbers.budget = 50;
        leaf.authority.budget.spend = 50;
        w.make(Party::Task(2), vec![leaf]);
        w.claim(2, 2);
        w.claim(3, 3);
        let middle_spent = seed % 11;
        let leaf_spent = (seed * 7) % 13;
        for (task, cumulative) in [(2, middle_spent), (3, leaf_spent)] {
            let reply_to = w.to();
            w.send(Event::Charge { reply_to, task, attempt: task, cumulative });
        }
        let spent = middle_spent + leaf_spent;
        let destination = Funder::Pool { project: 1, person: 9, period: 2 };
        let reply_to = w.to();
        w.send(Event::Move {
            reply_to,
            task: 2,
            authorization: Authorization::Person { person: 9, project: 1 },
            movement: Movement {
                to: Party::Person(9),
                transfers: Box::new([Transfer { task: 2, before: Funder::Task(1), after: destination }]),
                balances: Box::new([
                    Balance {
                        funder: Funder::Task(1),
                        before: w.record(1).numbers,
                        after: Numbers { budget: 1000, spent: 0, spent_below: spent, reserved: 0 },
                    },
                    Balance {
                        funder: destination,
                        before: Numbers { budget: 100, spent: 0, spent_below: 0, reserved: 0 },
                        after: Numbers { budget: 100, spent: 0, spent_below: 0, reserved: 100 - spent },
                    },
                ]),
                reason: Box::new([]),
            },
        });
        assert_eq!(w.record(2).numbers.budget, 100 - spent);
        assert_eq!(w.record(3).numbers.budget, 50 - leaf_spent);
        w.restart();
        let reply_to = w.to();
        w.send(Event::Charge { reply_to, task: 2, attempt: 2, cumulative: middle_spent + 1 });
        assert_eq!(w.record(2).numbers.spent, 1);
        w.cancel(1, b"end");
        w.settle(1);
        w.cancel(2, b"end");
        w.complete_cancel();
        assert!(w.records.contains_key(&tasks::Key::Closure { task: 2, generation: 2 }));
    }
}
