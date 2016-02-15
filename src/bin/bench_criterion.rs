#![cfg_attr(feature = "nightly",
            feature(duration_span))]
#![feature(asm)]
extern crate crossbeam;
extern crate criterion;

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::mpsc::channel;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::{Relaxed, Release};
use std::time::Duration;
use std::cmp;

use crossbeam::scope;
use crossbeam::sync::MsQueue;
use crossbeam::sync::SegQueue;

use criterion::Criterion;

use extra_impls::mpsc_queue::Queue as MpscQueue;

mod extra_impls;

const COUNT: u64 = 10000000;
const THREADS: u64 = 3;


fn timer<F: Fn() -> bool>(f: &F) -> Option<u64> {
    let s = unsafe {
        let mut cycles_high = 0u32;
        let mut cycles_low = 0u32;
        asm!("mfence\n\t
              rdtsc\n\t
              mov %edx, $0\n\t
              mov %eax, $1\n\t"
             : "=r"(cycles_high), "=r"(cycles_low)
             :
             : "rax", "rcx", "rbx", "rdx"
             : "volatile");
        (cycles_low as u64) + ((cycles_high as u64) << 32)

    };
    if f() {
        let n = unsafe {
            let mut cycles_high = 0u32;
            let mut cycles_low = 0u32;
            asm!("rdtscp\n\t
                  mov %edx, $0\n\t
                  mov %eax, $1\n\t
                  mfence\n\t"
                 : "=r"(cycles_high), "=r"(cycles_low)
                 :
                 : "rax", "rcx", "rbx", "rdx"
                 : "volatile");
            (cycles_low as u64) + ((cycles_high as u64) << 32)

        };
        Some(n - s)
    } else {
        None
    }
}

fn get_n_samples_of<F: Fn() -> bool>(f: F, n: usize) -> Vec<u64> {
    let mut rv: Vec<u64> = (0..).map(|_| timer(&f)).filter(|x| x.is_some()).map(|x| x.unwrap()).take(n).collect();
    rv.sort();
    rv
}

//super lazy impl
fn get_quantile(v: &Vec<u64>, n : f64) -> u64 {
    assert!(n <= 100.0);
    v[(((n * (v.len() as f64)) as u64) / 100) as usize]
}

fn get_quantiles(v: &Vec<u64>, q: &Vec<f64>) -> Vec<u64> {
    q.iter().map(|x| get_quantile(v, *x)).collect()
}

fn get_mean_time_used() -> u64 {
    let s = get_n_samples_of(|| true, 10000000);
    get_quantile(&s, 50.0)
}

trait Queue<T> {
    fn push(&self, T);
    fn try_pop(&self) -> Option<T>;
}

impl<T> Queue<T> for MsQueue<T> {
    fn push(&self, t: T) { self.push(t) }
    fn try_pop(&self) -> Option<T> { self.try_pop() }
}

impl<T> Queue<T> for SegQueue<T> {
    fn push(&self, t: T) { self.push(t) }
    fn try_pop(&self) -> Option<T> { self.try_pop() }
}

impl<T> Queue<T> for MpscQueue<T> {
    fn push(&self, t: T) { self.push(t) }
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

fn bench_queue_mpsc<Q: Queue<u64> + Sync, F>(name: &str, ct: F) where F: Fn() -> Q {
    // Benchmark push times
    {
        let isgood = AtomicBool::new(true);
        let q = ct();
        scope(|scope| {
            let qr = &q;
            let ig = &isgood;
            for _i in 0..(THREADS-1) {
                scope.spawn(move || {
                    while ig.load(Relaxed) {
                        let _ = qr.push(_i);
                    }
                });
            }
            for _ in 0..THREADS {
                scope.spawn(move || {
                    while ig.load(Relaxed) {
                        qr.try_pop();
                    }
                });
            }

            println!("Benching {}", name);
            //warm this up
            let _ = get_n_samples_of(|| { q.push(1); true }, 10000);
            let results = get_n_samples_of(|| q.try_pop().is_some(), 100000000);
            isgood.store(false, Release);
            let quantiles: Vec<f64> = vec![50.0, 75.0, 90.0, 95.0, 99.0, 99.9, 99.99, 99.999];
            let qquantiles = get_quantiles(&results, &quantiles);
            let extra = get_mean_time_used();
            for (q, qr) in qquantiles.iter().zip(quantiles.iter()) {
                println!("Quantile for {}: {}", qr, q - cmp::min(extra, *q));
            }
            println!("\n\n")
        });
    }
}
/*
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
                    for _x in 0..COUNT {
                        qr.push(true);
                    }
                    if pcr.fetch_add(1, Relaxed) == (THREADS as usize) - 1 {
                        for _x in 0..THREADS {
                            qr.push(false)
                        }
                    }
                });
                scope.spawn(move || {u
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
}*/

fn main() {
//    bench_queue_mpsc("MSQ", || MsQueue::new());
//    bench_queue_mpsc("mpsc", || MpscQueue::new());
    bench_queue_mpsc("Seg", || SegQueue::new());
}
