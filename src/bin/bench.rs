#![cfg_attr(feature = "nightly",
            feature(duration_span))]

#![feature(duration_span)]
extern crate crossbeam;

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::mpsc::channel;
use std::time::Duration;
use std::iter;

use crossbeam::scope;
use crossbeam::sync::MsQueue;
use crossbeam::sync::SegQueue;

use extra_impls::mpsc_queue::Queue as MpscQueue;

mod extra_impls;

const COUNT: usize = 10000000;
const THREADS: usize = 2;

fn time<F: FnOnce()>(f: F) -> Duration {
    Duration::span(f)
}


fn nanos(d: Duration) -> f64 {
    d.as_secs() as f64 * 1000000000f64 + (d.subsec_nanos() as f64)
}

trait Queue<T> {
    fn push(&self, T);
    fn push_bulk<I: ExactSizeIterator<Item=T>>(&self, i: &mut I);
    fn try_pop(&self) -> Option<T>;
}

impl<T> Queue<T> for MsQueue<T> {
    fn push(&self, t: T) { self.push(t) }
    fn push_bulk<I: ExactSizeIterator<Item=T>>(&self, i: &mut I) {
        for v in i {
            self.push(v);
        }
    }
    fn try_pop(&self) -> Option<T> { self.try_pop() }
}

impl<T> Queue<T> for SegQueue<T> {
    fn push(&self, t: T) { self.push(t) }
    fn push_bulk<I: ExactSizeIterator<Item=T>>(&self, i: &mut I) {
        self.push_bulk(i);
    }
    fn try_pop(&self) -> Option<T> { self.try_pop() }
}

impl<T> Queue<T> for MpscQueue<T> {
    fn push(&self, t: T) { self.push(t) }
    fn push_bulk<I: ExactSizeIterator<Item=T>>(&self, i: &mut I) {
        for v in i {
            self.push(v);
        }
    }
    fn try_pop(&self) -> Option<T> {
        use extra_impls::mpsc_queue::*;

        loop {
            match self.pop() {
                Data(t) => return Some(t),
                Empty => return None,
                Inconsistent => (),
            }
        }
    }
}

impl<T> Queue<T> for Mutex<VecDeque<T>> {
    fn push(&self, t: T) { self.lock().unwrap().push_back(t) }
    fn push_bulk<I: ExactSizeIterator<Item=T>>(&self, i: &mut I) {
        for v in i {
            self.push(v);
        }
    }
    fn try_pop(&self) -> Option<T> { self.lock().unwrap().pop_front() }
}

fn bench_queue_mpsc<Q: Queue<usize> + Sync>(q: Q) -> f64 {
    let d = time(|| {
        scope(|scope| {
            for _i in 0..THREADS {
                let qr = &q;
                scope.spawn(move || {
                    let mut numbers = 0..COUNT;
                    qr.push_bulk(&mut numbers);
                });
            }

            let mut count = 0;
            while count < COUNT*THREADS {
                if q.try_pop().is_some() {
                    count += 1;
                }
            }
        });
    });

    nanos(d) / ((COUNT * THREADS) as f64)
}

fn bench_queue_mpmc<Q: Queue<bool> + Sync>(q: Q) -> f64 {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering::Relaxed;

    let prod_count = AtomicUsize::new(0);

    let d = time(|| {
        scope(|scope| {
            for _i in 0..THREADS {
                let qr = &q;
                let pcr = &prod_count;
                scope.spawn(move || {
                    let mut x = (0..COUNT).map(|_x| true);
                    qr.push_bulk(&mut x);
                    if pcr.fetch_add(1, Relaxed) == (THREADS as usize) - 1 {
                        for _x in 0..THREADS {
                            qr.push(false)
                        }
                    }
                });
                scope.spawn(move || {
                    loop {
                        if let Some(false) = qr.try_pop() { break }
                    }
                });
            }


        });
    });

    nanos(d) / ((COUNT * THREADS) as f64)
}

fn bench_chan_mpsc() -> f64 {
    let (tx, rx) = channel();

    let d = time(|| {
        scope(|scope| {
            for _i in 0..THREADS {
                let my_tx = tx.clone();

                scope.spawn(move || {
                    for x in 0..COUNT {
                        let _ = my_tx.send(x);
                    }
                });
            }

            for _i in 0..COUNT*THREADS {
                let _ = rx.recv().unwrap();
            }
        });
    });

    nanos(d) / ((COUNT * THREADS) as f64)
}

fn main() {
    println!("MSQ mpsc: {}", bench_queue_mpsc(MsQueue::new()));
    println!("chan mpsc: {}", bench_chan_mpsc());
    println!("mpsc mpsc: {}", bench_queue_mpsc(MpscQueue::new()));
    println!("Seg mpsc: {}", bench_queue_mpsc(SegQueue::new()));

    println!("MSQ mpmc: {}", bench_queue_mpmc(MsQueue::new()));
    println!("Seg mpmc: {}", bench_queue_mpmc(SegQueue::new()));

//    println!("queue_mpsc: {}", bench_queue_mpsc());
//    println!("queue_mpmc: {}", bench_queue_mpmc());
//   println!("mutex_mpmc: {}", bench_mutex_mpmc());
}
