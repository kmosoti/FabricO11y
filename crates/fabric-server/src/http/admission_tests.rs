//! Started blocking work retains admission when its async waiter is canceled.
//! Barriers release before assertions, including when a lifetime defect occurs.
use super::{WorkPermit, spawn_query_work};
use std::sync::{Arc, mpsc};
use std::time::Duration;
use tokio::sync::{Semaphore, oneshot};

const DEADLINE: Duration = Duration::from_secs(2);

fn admitted(pool: &Arc<Semaphore>) -> WorkPermit {
    Arc::new(Arc::clone(pool).try_acquire_owned().unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canceled_waiter_keeps_started_worker_admission_until_completion() {
    let pool = Arc::new(Semaphore::new(1));
    let permit = admitted(&pool);
    let (entered, started) = oneshot::channel();
    let (release, released) = mpsc::channel();
    let (finished, completed) = oneshot::channel();
    let waiter = tokio::spawn(async move {
        spawn_query_work(permit, move || {
            let _ = entered.send(());
            let released = released.recv_timeout(DEADLINE).is_ok();
            let _ = finished.send(released);
            42
        })
        .await
    });
    let started = tokio::time::timeout(DEADLINE, started).await;
    waiter.abort();
    let cancellation = tokio::time::timeout(DEADLINE, waiter).await;
    let still_owned =
        pool.available_permits() == 0 && Arc::clone(&pool).try_acquire_owned().is_err();
    // Release even when started/cancellation/ownership observations are wrong.
    let _ = release.send(());
    let completed = tokio::time::timeout(DEADLINE, completed).await;
    let recovered = tokio::time::timeout(DEADLINE, Arc::clone(&pool).acquire_owned()).await;
    assert!(
        matches!(started, Ok(Ok(()))),
        "worker did not reach its barrier"
    );
    assert!(matches!(cancellation, Ok(Err(error)) if error.is_cancelled()));
    assert!(
        matches!(completed, Ok(Ok(true))),
        "worker release/completion failed"
    );
    assert!(
        still_owned,
        "canceling the waiter released a still-running scan slot"
    );
    assert!(
        matches!(recovered, Ok(Ok(_))),
        "completed worker retained admission"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_worker_releases_admission() {
    let pool = Arc::new(Semaphore::new(1));
    let permit = admitted(&pool);
    let (entered, started) = oneshot::channel();
    let (release, released) = mpsc::channel();
    let worker = spawn_query_work(permit, move || {
        let _ = entered.send(());
        released.recv_timeout(DEADLINE).is_ok()
    });
    let started = tokio::time::timeout(DEADLINE, started).await;
    let busy = pool.available_permits() == 0;
    let _ = release.send(());
    let joined = tokio::time::timeout(DEADLINE, worker).await;
    let available = pool.available_permits();
    assert!(matches!(started, Ok(Ok(()))));
    assert!(
        matches!(joined, Ok(Ok(true))),
        "worker must complete after release"
    );
    assert!(busy, "started work did not own admission");
    assert_eq!(available, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn panicking_worker_releases_admission() {
    let pool = Arc::new(Semaphore::new(1));
    let permit = admitted(&pool);
    let (entered, started) = oneshot::channel();
    let (release, released) = mpsc::channel();
    let worker = spawn_query_work(permit, move || {
        let _ = entered.send(());
        let _ = released.recv_timeout(DEADLINE);
        panic!("injected query-worker panic after release");
    });
    let started = tokio::time::timeout(DEADLINE, started).await;
    let busy = pool.available_permits() == 0;
    let _ = release.send(());
    let joined = tokio::time::timeout(DEADLINE, worker).await;
    let available = pool.available_permits();
    assert!(matches!(started, Ok(Ok(()))));
    assert!(
        matches!(joined, Ok(Err(error)) if error.is_panic()),
        "expected injected panic"
    );
    assert!(busy, "started work did not own admission");
    assert_eq!(available, 1);
}
