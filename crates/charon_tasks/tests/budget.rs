use charon_tasks::*;
#[test]
fn budgets_cover_four_and_many_core_cpus() {
    for cpus in [1, 2, 4, 8, 16, 64] {
        let b = ThreadBudget::for_cpus(cpus, 1, 1);
        assert!(b.physics >= 1 && b.lighting >= 1);
        if cpus >= 4 {
            assert_eq!(b.reserved + b.physics + b.lighting, cpus);
        }
        if cpus == 4 {
            assert_eq!(b.helpers(), 0);
        }
        if cpus == 8 {
            assert_eq!((b.physics, b.lighting), (3, 3));
        }
    }
}
#[test]
fn reusable_pool_preserves_job_order_and_bounds_threads() {
    let pool = TaskPool::new(2);
    for _ in 0..3 {
        let values = pool.map((0..100).collect(), |i| i * i).unwrap();
        assert_eq!(values, (0..100).map(|i| i * i).collect::<Vec<_>>());
        assert!(pool.spawned_helpers() <= 2);
    }
}
#[test]
fn zero_helper_mode_runs_without_deadlock() {
    let pool = TaskPool::new(0);
    assert_eq!(pool.map(vec![1, 2, 3], |v| v + 1).unwrap(), vec![2, 3, 4]);
    assert_eq!(pool.spawned_helpers(), 0);
}

#[test]
fn nested_jobs_progress_and_shutdown_rejects_new_work() {
    let pool = std::sync::Arc::new(TaskPool::new(2));
    let nested = pool.clone();
    let values = pool
        .map(vec![1, 2, 3, 4], move |n| {
            nested.map(vec![n, n + 1], |v| v * 2).unwrap()
        })
        .unwrap();
    assert_eq!(
        values,
        vec![vec![2, 4], vec![4, 6], vec![6, 8], vec![8, 10]]
    );
    pool.shutdown();
    assert!(pool.map(vec![1], |v| v).is_err());
}
