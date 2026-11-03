//! An ordered, bounded, panic-safe parallel pipeline.
//!
//! Every heavy operation in Kiln has the same shape:
//!
//! ```text
//!   Source (1 thread)  ->  Workers (N threads)  ->  Sink (calling thread)
//!   read + hash files      compress / decompress    write in original order
//! ```
//!
//! [`run_ordered`] implements that shape once:
//!
//! * **Order is preserved.** Workers finish out of order; the sink always sees results in the
//!   order the source produced the items, so output is byte-for-byte reproducible no matter how
//!   many threads are used.
//! * **Memory is bounded.** A fixed number of *credits* circulates. The source needs a credit to
//!   produce an item; the sink returns it once the item has been consumed. At most
//!   [`Config::inflight`] items exist at any moment, however slow any stage is.
//! * **Failures are clean.** The first error *in item order* wins. On an error or a worker panic
//!   every stage is told to stop and the call returns; nothing deadlocks or leaks threads.

use std::any::Any;
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use crossbeam_channel::{bounded, unbounded};

/// Why a pipeline run stopped early.
#[derive(Debug)]
pub enum PipelineError<E> {
    /// A stage returned an error.
    Task(E),
    /// A stage panicked; the payload message is preserved.
    Panic(String),
}

/// Produces the items that flow through the pipeline. Runs on its own thread.
pub trait Source: Send {
    /// What is produced.
    type Item: Send;
    /// The error type shared by every stage.
    type Error: Send;
    /// Produce the next item, or `None` when the input is exhausted.
    fn next_item(&mut self) -> Result<Option<Self::Item>, Self::Error>;
}

/// Pipeline sizing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Number of worker threads (at least 1).
    pub threads: usize,
    /// Maximum number of items alive at once (at least 1).
    pub inflight: usize,
}

/// Turn a requested thread count into a real one (`0` means "all available cores").
pub fn resolve_threads(requested: usize) -> usize {
    let n = if requested == 0 {
        thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    } else {
        requested
    };
    n.max(1)
}

impl Config {
    /// A configuration for `requested` worker threads (`0` = all cores) with a default
    /// in-flight window of 1.5 x threads + 2, enough to keep every worker busy.
    pub fn new(requested: usize) -> Self {
        let threads = resolve_threads(requested);
        Self {
            threads,
            inflight: threads * 3 / 2 + 2,
        }
    }

    /// Override the in-flight window.
    pub fn with_inflight(mut self, inflight: usize) -> Self {
        self.inflight = inflight.max(1);
        self
    }
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_owned()
    }
}

/// Run `source -> work -> sink` with `config.threads` workers, delivering results to `sink` in
/// source order. Returns the source (so callers can read state it accumulated) on success.
pub fn run_ordered<S, Out, W, K>(
    config: Config,
    source: S,
    work: W,
    mut sink: K,
) -> Result<S, PipelineError<S::Error>>
where
    S: Source,
    Out: Send,
    W: Fn(S::Item) -> Result<Out, S::Error> + Sync,
    K: FnMut(Out) -> Result<(), S::Error>,
{
    let threads = config.threads.max(1);
    let inflight = config.inflight.max(1);
    let abort = AtomicBool::new(false);

    thread::scope(|scope| {
        let abort = &abort;
        let work = &work;
        let (work_tx, work_rx) = bounded::<(u64, S::Item)>(inflight);
        let (res_tx, res_rx) = unbounded::<(u64, Result<Out, PipelineError<S::Error>>)>();
        let (credit_tx, credit_rx) = bounded::<()>(inflight);
        for _ in 0..inflight {
            credit_tx
                .send(())
                .expect("credit channel has room for every credit");
        }

        // Stage 1: the source.
        let producer = scope.spawn(move || {
            let mut source = source;
            let mut seq = 0u64;
            let outcome = loop {
                if abort.load(Ordering::Relaxed) || credit_rx.recv().is_err() {
                    break Ok(());
                }
                match catch_unwind(AssertUnwindSafe(|| source.next_item())) {
                    Ok(Ok(Some(item))) => {
                        if work_tx.send((seq, item)).is_err() {
                            break Ok(());
                        }
                        seq += 1;
                    }
                    Ok(Ok(None)) => break Ok(()),
                    Ok(Err(e)) => break Err(PipelineError::Task(e)),
                    Err(p) => break Err(PipelineError::Panic(panic_message(p))),
                }
            };
            (source, outcome) // dropping `work_tx` here lets the workers drain and exit
        });

        // Stage 2: the workers.
        for _ in 0..threads {
            let work_rx = work_rx.clone();
            let res_tx = res_tx.clone();
            scope.spawn(move || {
                for (seq, item) in &work_rx {
                    if abort.load(Ordering::Relaxed) {
                        break;
                    }
                    let result = match catch_unwind(AssertUnwindSafe(|| work(item))) {
                        Ok(r) => r.map_err(PipelineError::Task),
                        Err(p) => Err(PipelineError::Panic(panic_message(p))),
                    };
                    if res_tx.send((seq, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(work_rx);
        drop(res_tx);

        // Stage 3: the sink, on the calling thread, strictly in item order.
        let mut pending: BTreeMap<u64, Result<Out, PipelineError<S::Error>>> = BTreeMap::new();
        let mut next = 0u64;
        let mut failure = None;
        'receive: for (seq, result) in &res_rx {
            pending.insert(seq, result);
            while let Some(result) = pending.remove(&next) {
                let step = result.and_then(|out| sink(out).map_err(PipelineError::Task));
                if let Err(e) = step {
                    failure = Some(e);
                    abort.store(true, Ordering::Relaxed);
                    break 'receive;
                }
                next += 1;
                // Tokens are conserved, so the channel always has room for this one.
                let _ = credit_tx.send(());
            }
        }
        // Closing both ends wakes a source blocked on a credit or a worker blocked on a send.
        drop(res_rx);
        drop(credit_tx);

        let joined = producer.join();
        if let Some(e) = failure {
            return Err(e);
        }
        let (source, outcome) = joined.map_err(|p| PipelineError::Panic(panic_message(p)))?;
        outcome?;
        if !pending.is_empty() {
            return Err(PipelineError::Panic("pipeline lost a work item".into()));
        }
        Ok(source)
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use super::*;

    /// Yields 0..limit, optionally failing at `fail_at`, and records how many it produced.
    #[derive(Debug)]
    struct Counter {
        next: u32,
        limit: u32,
        fail_at: Option<u32>,
        alive: Option<Arc<(AtomicUsize, AtomicUsize)>>,
    }

    impl Counter {
        fn new(limit: u32) -> Self {
            Self {
                next: 0,
                limit,
                fail_at: None,
                alive: None,
            }
        }
    }

    impl Source for Counter {
        type Item = u32;
        type Error = String;
        fn next_item(&mut self) -> Result<Option<u32>, String> {
            if self.next >= self.limit {
                return Ok(None);
            }
            if self.fail_at == Some(self.next) {
                return Err(format!("source failed at {}", self.next));
            }
            if let Some(a) = &self.alive {
                let now = a.0.fetch_add(1, Ordering::SeqCst) + 1;
                a.1.fetch_max(now, Ordering::SeqCst);
            }
            self.next += 1;
            Ok(Some(self.next - 1))
        }
    }

    fn jitter(i: u32) {
        thread::sleep(Duration::from_micros(
            u64::from(i.wrapping_mul(7919) % 5) * 300,
        ));
    }

    #[test]
    fn results_arrive_in_source_order_despite_uneven_work() {
        let mut seen = Vec::new();
        let src = run_ordered(
            Config::new(4),
            Counter::new(300),
            |i| {
                jitter(i);
                Ok(i * 2)
            },
            |v| {
                seen.push(v);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(seen, (0..300).map(|i| i * 2).collect::<Vec<_>>());
        assert_eq!(
            src.next, 300,
            "the source is handed back with its final state"
        );
    }

    #[test]
    fn empty_input_is_fine() {
        let mut calls = 0;
        run_ordered(Config::new(3), Counter::new(0), Ok, |_: u32| {
            calls += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(calls, 0);
    }

    #[test]
    fn source_error_is_reported_after_earlier_items_are_delivered() {
        let mut seen = Vec::new();
        let mut src = Counter::new(100);
        src.fail_at = Some(50);
        let err = run_ordered(Config::new(4), src, Ok, |v| {
            seen.push(v);
            Ok(())
        })
        .unwrap_err();
        assert!(matches!(err, PipelineError::Task(ref m) if m == "source failed at 50"));
        assert_eq!(seen, (0..50).collect::<Vec<_>>());
    }

    #[test]
    fn earliest_worker_error_wins_and_later_items_are_not_delivered() {
        let mut seen = Vec::new();
        let err = run_ordered(
            Config::new(4),
            Counter::new(200),
            |i| {
                jitter(i);
                if i == 20 || i == 120 {
                    Err(format!("bad {i}"))
                } else {
                    Ok(i)
                }
            },
            |v| {
                seen.push(v);
                Ok(())
            },
        )
        .unwrap_err();
        assert!(matches!(err, PipelineError::Task(ref m) if m == "bad 20"));
        assert_eq!(seen, (0..20).collect::<Vec<_>>());
    }

    #[test]
    fn sink_error_stops_an_endless_source() {
        #[derive(Debug)]
        struct Endless;
        impl Source for Endless {
            type Item = u64;
            type Error = String;
            fn next_item(&mut self) -> Result<Option<u64>, String> {
                Ok(Some(1))
            }
        }
        let mut n = 0;
        let err = run_ordered(Config::new(2), Endless, Ok, |_| {
            n += 1;
            if n == 5 {
                Err("enough".to_owned())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert!(matches!(err, PipelineError::Task(ref m) if m == "enough"));
    }

    #[test]
    fn worker_panic_becomes_an_error_instead_of_a_deadlock() {
        let err = run_ordered(
            Config::new(3),
            Counter::new(100),
            |i| {
                if i == 7 {
                    panic!("boom at 7");
                }
                Ok(i)
            },
            |_| Ok(()),
        )
        .unwrap_err();
        assert!(matches!(err, PipelineError::Panic(ref m) if m.contains("boom at 7")));
    }

    #[test]
    fn items_in_flight_never_exceed_the_window() {
        let stats = Arc::new((AtomicUsize::new(0), AtomicUsize::new(0)));
        let mut src = Counter::new(400);
        src.alive = Some(stats.clone());
        let cfg = Config::new(3).with_inflight(5);
        let s2 = stats.clone();
        run_ordered(
            cfg,
            src,
            |i| {
                jitter(i);
                Ok(i)
            },
            |_| {
                s2.0.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .unwrap();
        let max = stats.1.load(Ordering::SeqCst);
        assert!(max <= 5, "{max} items were alive at once, window is 5");
        assert!(
            max >= 2,
            "pipeline should actually overlap work (saw {max})"
        );
    }

    #[test]
    fn single_thread_single_credit_still_completes() {
        let mut total = 0u64;
        run_ordered(Config::new(1).with_inflight(1), Counter::new(50), Ok, |v| {
            total += u64::from(v);
            Ok(())
        })
        .unwrap();
        assert_eq!(total, (0..50u64).sum());
    }

    #[test]
    fn thread_count_resolution() {
        assert_eq!(resolve_threads(3), 3);
        assert!(resolve_threads(0) >= 1);
        let c = Config::new(4);
        assert_eq!((c.threads, c.inflight), (4, 8));
    }
}
