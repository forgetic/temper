//! A finished delegate remains readable through ctl, including its final tree.
use temper_client_world::fixtures;

#[test]
fn an_ended_tasks_tree_is_a_finished_read() {
    ended_tree(false);
}

#[test]
fn an_ended_tasks_tree_is_a_finished_json_read() {
    ended_tree(true);
}

fn ended_tree(json: bool) {
    let mut world = fixtures::world(fixtures::SEED);
    let tasks = fixtures::goal_with_a_finished_dependency(world.front());
    assert!(world.front().engine.task(tasks.first).is_none(), "ended delegate retired from live tasks");
    let agent = fixtures::agent(&mut world);
    let parent = tasks.goal.to_string();
    let child = tasks.first.to_string();

    let live = fixtures::give(&mut world, agent, &[b"task", parent.as_bytes(), b"--tree"]);
    let live = fixtures::answered(&mut world, agent, live);
    assert_eq!(live.status, 0, "live parent tree: {live:?}");
    let plain = fixtures::give(&mut world, agent, &[b"task", child.as_bytes()]);
    let plain = fixtures::answered(&mut world, agent, plain);
    assert_eq!(plain.status, 0, "ended delegate document: {plain:?}");
    assert!(plain.stderr.is_empty());

    let mut words = vec![b"task".as_slice(), child.as_bytes(), b"--tree"];
    if json {
        words.push(b"--json");
    }
    let read = fixtures::give(&mut world, agent, &words);
    let read = fixtures::answered(&mut world, agent, read);
    assert_eq!(
        (read.status, read.stderr.as_ref()),
        (0, b"".as_slice()),
        "ended delegate tree: stdout={:?}, stderr={:?}",
        String::from_utf8_lossy(&read.stdout),
        String::from_utf8_lossy(&read.stderr),
    );
    let stdout = String::from_utf8_lossy(&read.stdout);
    let root = if json {
        format!("\"tree\":{{\"root\":{{\"task\":{child},")
    } else {
        format!("tree:\n{child} · Investigate the change · Ended\n")
    };
    assert!(stdout.contains(&root), "final tree contains its ended root: {stdout}");
}
