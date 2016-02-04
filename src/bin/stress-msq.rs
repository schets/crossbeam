extern crate crossbeam;

use crossbeam::sync::MsQueue;
use crossbeam::scope;

use std::sync::Arc;
// I know no better way in rust to prevent loop elimination
use std::sync::atomic::{AtomicBool, Ordering, fence};

const DUP: usize = 4;
const THREADS: u32 = 2;
const COUNT: u64 = 100000;
const THREADLOOP: u64 = 500;
const THREADCOUNT: u64 = 50;


fn main() {
    let DUMMY = AtomicBool::new(false);
    scope(|s| {
        for _i in 0..DUP {
            let q = Arc::new(MsQueue::new());
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
            for _ in 0..THREADCOUNT {
                s.spawn(|| {
                    for _ in 0..THREADLOOP {
                        fence(Ordering::SeqCst);
                        if DUMMY.load(Ordering::Relaxed) {
                            break
                        }
                    }
                });
            }
        }
    });
}
