extern crate crossbeam;

use crossbeam::sync::MsQueue;
use crossbeam::scope;

use std::sync::Arc;

const DUP: usize = 4;
const THREADS: u32 = 2;
const COUNT: u64 = 10000;
const EXTRA_COUNT: u64 = 20;
const EXTRA_THREAD: u64 = 50;

fn _main() {
    scope(|s| {
        for _i in 0..DUP {
            let q = Arc::new(MsQueue::new());
            let q_extra = Arc::new(MsQueue::new());
            let qs = q.clone();

            s.spawn(move || {
                for i in 1..COUNT { qs.push(i) }
            });

            for _i in 0..THREADS {
                let qr = q.clone();
                s.spawn(move || {
                    let mut cur: u64 = 0;
                    for _j in 0..COUNT {
                        if let Some(new) = qr.try_pop() {
                            assert!(new > cur);
                            cur = new;
                        }
                    }
                });
            }

            for _ in 0..EXTRA_THREAD {
                let q = q_extra.clone();
                s.spawn(move || {
                    for i in 0..EXTRA_COUNT {
                        if i % 2 == 0 {
                            q.push(i);
                        }
                        else {
                            q.try_pop();
                        }
                    }
                });
            }
        }
    });
}

fn main() {
    loop {
        _main();
    }
}
