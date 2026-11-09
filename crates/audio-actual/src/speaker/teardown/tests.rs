use super::*;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct Probe(Arc<AtomicUsize>);
impl Drop for Probe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
struct Fake {
    calls: Arc<Mutex<Vec<Stage>>>,
    fail: Option<Stage>,
    panic: bool,
}
impl Fake {
    fn call(&self, stage: Stage) -> i32 {
        self.calls.lock().unwrap().push(stage);
        if self.fail == Some(stage) {
            assert!(!self.panic, "controlled native teardown panic");
            -123
        } else {
            0
        }
    }
}
impl Operations<u32> for Fake {
    fn stop(&self, device: u32, callback: u32) -> i32 {
        assert_eq!((device, callback), (20, 30));
        self.call(Stage::Stop)
    }
    fn destroy_callback(&self, device: u32, callback: u32) -> i32 {
        assert_eq!((device, callback), (20, 30));
        self.call(Stage::Callback)
    }
    fn destroy_aggregate(&self, device: u32) -> i32 {
        assert_eq!(device, 20);
        self.call(Stage::Aggregate)
    }
    fn destroy_tap(&self, tap: u32) -> i32 {
        assert_eq!(tap, 10);
        self.call(Stage::Tap)
    }
}
fn fixture(
    fail: Option<Stage>,
    panic: bool,
) -> (
    Owned<u32, Probe, Fake>,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<Stage>>>,
) {
    let dropped = Arc::new(AtomicUsize::new(0));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut owned = Owned::new(
        Fake {
            calls: calls.clone(),
            fail,
            panic,
        },
        Some(Arc::new(Probe(dropped.clone()))),
    );
    owned.tap = Some(10);
    owned.device = Some(20);
    owned.callback = Some(30);
    owned.started = true;
    owned.context = Some(Box::new(Probe(dropped.clone())));
    (owned, dropped, calls)
}

#[test]
fn successful_checked_teardown_closes_every_original_handle_once_before_releasing_context() {
    let (mut owned, dropped, calls) = fixture(None, false);
    owned.close().unwrap();
    owned.close().unwrap();
    assert!(!owned.cleanup_failed.load(Ordering::Acquire));
    assert_eq!(
        *calls.lock().unwrap(),
        [Stage::Stop, Stage::Callback, Stage::Aggregate, Stage::Tap]
    );
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert!(owned.device.is_none() && owned.callback.is_none() && owned.tap.is_none());
    drop(owned);
    assert_eq!(dropped.load(Ordering::SeqCst), 2);
}

#[test]
fn each_native_failure_or_panic_retains_context_owner_and_remaining_handles_without_retry() {
    let stages = [Stage::Stop, Stage::Callback, Stage::Aggregate, Stage::Tap];
    for (index, stage) in stages.iter().copied().enumerate() {
        for panic in [false, true] {
            let (mut owned, dropped, calls) = fixture(Some(stage), panic);
            let expected = Failure {
                stage,
                status: (!panic).then_some(-123),
            };
            assert_eq!(owned.close(), Err(expected));
            assert_eq!(owned.close(), Err(expected));
            assert_eq!(*calls.lock().unwrap(), stages[..=index]);
            assert!(owned.tap.is_some());
            assert_eq!(owned.device.is_some(), stage != Stage::Tap);
            assert_eq!(
                owned.callback.is_some(),
                matches!(stage, Stage::Stop | Stage::Callback)
            );
            let observer = owned.cleanup_failed.clone();
            drop(owned);
            assert!(observer.load(Ordering::Acquire));
            assert_eq!(*calls.lock().unwrap(), stages[..=index]);
            assert_eq!(dropped.load(Ordering::SeqCst), 0);
        }
    }
}

#[test]
fn partial_startup_preserves_first_error_and_cleans_only_acquired_resources() {
    for acquired in 1..=4 {
        let (mut owned, dropped, calls) = fixture(None, false);
        owned.started = acquired == 4;
        if acquired < 3 {
            owned.callback = None;
        }
        if acquired < 2 {
            owned.device = None;
        }
        assert_eq!(
            owned.startup_error("original startup error"),
            Ok("original startup error")
        );
        let expected = &[
            vec![Stage::Tap],
            vec![Stage::Aggregate, Stage::Tap],
            vec![Stage::Callback, Stage::Aggregate, Stage::Tap],
            vec![Stage::Stop, Stage::Callback, Stage::Aggregate, Stage::Tap],
        ][acquired - 1];
        assert_eq!(&*calls.lock().unwrap(), expected);
        drop(owned);
        assert_eq!(dropped.load(Ordering::SeqCst), 2);
    }
    let (mut owned, dropped, calls) = fixture(Some(Stage::Stop), false);
    assert_eq!(
        owned.startup_error("original start failed"),
        Err((
            "original start failed",
            Failure {
                stage: Stage::Stop,
                status: Some(-123)
            }
        ))
    );
    drop(owned);
    assert_eq!(*calls.lock().unwrap(), [Stage::Stop]);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
}

#[test]
fn unknown_registration_and_startup_unwind_never_free_a_live_callback_context() {
    let (mut owned, dropped, calls) = fixture(None, false);
    owned.uncertain(Failure {
        stage: Stage::Register,
        status: Some(-55),
    });
    owned.uncertain(Failure {
        stage: Stage::Tap,
        status: Some(-66),
    });
    assert_eq!(
        owned.close(),
        Err(Failure {
            stage: Stage::Register,
            status: Some(-55)
        })
    );
    drop(owned);
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    for fail in [None, Some(Stage::Callback)] {
        let (owned, dropped, _) = fixture(fail, false);
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                let _owned = owned;
                panic!("startup task unwound");
            }))
            .is_err()
        );
        assert_eq!(
            dropped.load(Ordering::SeqCst),
            if fail.is_none() { 2 } else { 0 }
        );
    }
}
