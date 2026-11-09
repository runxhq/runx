//! Running a step's admitted calls apart from its driver, at most so many at
//! once. Each item is started through `start`, which runs it apart and sends
//! its result when it ends; each result is handed to `finish` on the driver,
//! in the order the items finished. After `finish` fails, nothing more is
//! started, and what is already running still finishes and is handed over,
//! since its effect may have happened.

use std::sync::mpsc;

use crate::RuntimeError;

pub(super) fn run_bounded<I, R>(
    items: impl IntoIterator<Item = I>,
    limit: usize,
    mut start: impl FnMut(I, mpsc::Sender<R>),
    mut finish: impl FnMut(R) -> Result<(), RuntimeError>,
) -> Result<(), RuntimeError> {
    let (done, finished) = mpsc::channel();
    let mut items = items.into_iter();
    let mut running = 0_usize;
    let mut failure = None;
    loop {
        while running < limit.max(1) && failure.is_none() {
            let Some(item) = items.next() else {
                break;
            };
            start(item, done.clone());
            running += 1;
        }
        if running == 0 {
            break;
        }
        // Every started item sends exactly once, so a result is coming.
        let result = finished.recv().map_err(|_| RuntimeError::EngineInvariant {
            context: "running a step's calls",
            message: "a call ended without saying how".to_owned(),
        })?;
        running -= 1;
        if let Err(error) = finish(result)
            && failure.is_none()
        {
            failure = Some(error);
        }
    }
    failure.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::run_bounded;
    use crate::RuntimeError;

    /// N items that each wait until N have started can only finish if N run
    /// at once: a limit below N would leave them waiting.
    #[test]
    fn as_many_as_the_limit_run_at_once() -> Result<(), RuntimeError> {
        let started_together = Arc::new(Barrier::new(3));
        let mut finished = 0;
        std::thread::scope(|threads| {
            run_bounded(
                0..3,
                3,
                |item, done| {
                    let started_together = Arc::clone(&started_together);
                    threads.spawn(move || {
                        started_together.wait();
                        let _ = done.send(item);
                    });
                },
                |_| {
                    finished += 1;
                    Ok(())
                },
            )
        })?;
        assert_eq!(finished, 3);
        Ok(())
    }

    /// Starting and finishing are both the driver's, so counting what has
    /// started and not finished there sees every start: never past the limit.
    #[test]
    fn no_more_than_the_limit_ever_start_together() -> Result<(), RuntimeError> {
        let running = std::cell::Cell::new(0_usize);
        let mut most = 0;
        let mut finished = 0;
        std::thread::scope(|threads| {
            run_bounded(
                0..7,
                2,
                |item, done| {
                    running.set(running.get() + 1);
                    most = most.max(running.get());
                    assert!(running.get() <= 2, "a third started with two running");
                    threads.spawn(move || {
                        let _ = done.send(item);
                    });
                },
                |_| {
                    running.set(running.get() - 1);
                    finished += 1;
                    Ok(())
                },
            )
        })?;
        assert_eq!(finished, 7);
        assert_eq!(most, 2);
        Ok(())
    }

    /// Results reach the driver in the order the items finished: the first
    /// item waits until the second has been handed over.
    #[test]
    fn results_are_handed_over_in_the_order_items_finish() -> Result<(), RuntimeError> {
        let (second_handed, wait_for_second) = std::sync::mpsc::channel::<()>();
        let wait_for_second = std::sync::Mutex::new(Some(wait_for_second));
        let mut order = Vec::new();
        std::thread::scope(|threads| {
            run_bounded(
                0..2,
                2,
                |item, done| {
                    let waits = (item == 0)
                        .then(|| wait_for_second.lock().ok().and_then(|mut wait| wait.take()))
                        .flatten();
                    threads.spawn(move || {
                        if let Some(wait) = waits {
                            let _ = wait.recv();
                        }
                        let _ = done.send(item);
                    });
                },
                |item| {
                    order.push(item);
                    if item == 1 {
                        let _ = second_handed.send(());
                    }
                    Ok(())
                },
            )
        })?;
        assert_eq!(order, [1, 0]);
        Ok(())
    }

    /// After a failure nothing more starts, and what was running is still
    /// handed over.
    #[test]
    fn a_failure_starts_nothing_more_and_settles_what_runs() {
        let mut handed = Vec::new();
        let result = std::thread::scope(|threads| {
            run_bounded(
                0..5,
                2,
                |item, done| {
                    threads.spawn(move || {
                        let _ = done.send(item);
                    });
                },
                |item| {
                    handed.push(item);
                    if handed.len() == 1 {
                        Err(RuntimeError::EngineInvariant {
                            context: "test",
                            message: "the first failed".to_owned(),
                        })
                    } else {
                        Ok(())
                    }
                },
            )
        });
        assert!(result.is_err());
        // The two started before the failure are both handed over; nothing
        // after them started.
        assert_eq!(handed.len(), 2);
    }
}
