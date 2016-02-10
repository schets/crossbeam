use std::sync::atomic::Ordering::{Acquire, Release, Relaxed};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::{ptr, mem};
use std::cmp;
use std::cell::UnsafeCell;

use mem::epoch::{self, Atomic, Owned, Guard};

const SEG_SIZE: usize = 32;

/// A Michael-Scott queue that allocates "segments" (arrays of nodes)
/// for efficiency.
///
/// Usable with any number of producers and consumers.
pub struct SegQueue<T> {
    head: Atomic<Segment<T>>,
    tail: Atomic<Segment<T>>,
}

struct Segment<T> {
    low: AtomicUsize,
    data: [UnsafeCell<(T, AtomicBool)>; SEG_SIZE],
    high: AtomicUsize,
    next: Atomic<Segment<T>>,
}

unsafe impl<T: Send> Sync for Segment<T> {}

impl<T> Segment<T> {
    fn new() -> Segment<T> {
        let rqueue = Segment {
            data: unsafe { mem::uninitialized() },
            low: AtomicUsize::new(0),
            high: AtomicUsize::new(0),
            next: Atomic::null(),
        };
        for val in rqueue.data.iter() {
            unsafe {
                (*val.get()).1 = AtomicBool::new(false);
            }
        }
        rqueue
    }
}

impl<T> SegQueue<T> {
    /// Create a new, empty queue.
    pub fn new() -> SegQueue<T> {
        let q = SegQueue {
            head: Atomic::null(),
            tail: Atomic::null(),
        };
        let sentinel = Owned::new(Segment::new());
        let guard = epoch::pin();
        let sentinel = q.head.store_and_ref(sentinel, Relaxed, &guard);
        q.tail.store_shared(Some(sentinel), Relaxed);
        q
    }

    /// Add `t` to the back of the queue.
    pub fn push(&self, t: T) {
        let guard = epoch::pin();
        loop {
            let tail = self.tail.load(Acquire, &guard).unwrap();
            if tail.high.load(Relaxed) >= SEG_SIZE { continue }
            let i = tail.high.fetch_add(1, Relaxed);
            unsafe {
                if i < SEG_SIZE {
                    let cell = (*tail).data.get_unchecked(i).get();
                    ptr::write(&mut (*cell).0, t);
                    (*cell).1.store(true, Release);

                    if i + 1 == SEG_SIZE {
                        let tail = tail.next.store_and_ref(Owned::new(Segment::new()), Release, &guard);
                        self.tail.store_shared(Some(tail), Release);
                    }

                    return
                }
            }
        }
    }

    /// Pushes all elements contained in the iterator to the back of the queue
    ///
    /// It's advised that this iterator shouldn't do too much heavy lifting
    /// between successful yields otherwise memory reclamation will be delayed
    #[inline(always)]
    pub fn push_bulk<I: ExactSizeIterator>(&self, i: &mut I) where I: ExactSizeIterator<Item=T> {
        let mut elems_left = i.len();
        while elems_left > 0 {
            let guard = epoch::pin();
            let try_to_push = cmp::min(elems_left, SEG_SIZE/2);
            let tail = self.tail.load(Acquire, &guard).unwrap();
            if tail.high.load(Relaxed) >= SEG_SIZE { continue }
            let j = tail.high.fetch_add(try_to_push, Relaxed);
            if j >= SEG_SIZE { continue }
            // Update the tail prior to pushing new elements
            // so that other can continue while we write
            if j + try_to_push >= SEG_SIZE {
                let tail = tail.next.store_and_ref(Owned::new(Segment::new()), Release, &guard);
                self.tail.store_shared(Some(tail), Release);
            }
            let topush = if j + try_to_push > SEG_SIZE {
SEG_SIZE - j} else {try_to_push};
            elems_left -= topush;
            // Theres an arm optimization here,
            // where we batch data writes, then have a single release fence,
            // then batch ready writes.
            for e in j..(j + topush) {
                unsafe {
match i.next() {
Some(val) => {
                    let cell = (*tail).data.get_unchecked(e).get();
                    ptr::write(&mut (*cell).0, val);
                    (*cell).1.store(true, Release);
},
None => {println!("Broke with elems_left of {} and try of {}", elems_left, e-j); return ()},
}
                }
            }
        }
    }

    /// Attempt to dequeue from the front.
    ///
    /// Returns `None` if the queue is observed to be empty.
    pub fn try_pop(&self) -> Option<T> {
        let guard = epoch::pin();
        loop {
            let head = self.head.load(Acquire, &guard).unwrap();
            loop {
                let low = head.low.load(Relaxed);
                if low >= cmp::min(head.high.load(Relaxed), SEG_SIZE) { break }
                if head.low.compare_and_swap(low, low+1, Relaxed) == low {
                    unsafe {
                        let cell = (*head).data.get_unchecked(low).get();
                        loop {
                            if (*cell).1.load(Acquire) { break }
                        }
                        if low + 1 == SEG_SIZE {
                            loop {
                                if let Some(next) = head.next.load(Acquire, &guard) {
                                    self.head.store_shared(Some(next), Release);
                                    break
                                }
                            }
                        }
                        return Some(ptr::read(&(*cell).0))
                    }
                }
            }
            if head.next.load(Relaxed, &guard).is_none() { return None }
        }
    }
}

#[cfg(test)]
mod test {
    const CONC_COUNT: i64 = 1000000;

    use scope;
    use super::*;

    #[test]
    fn push_pop_1() {
        let q: SegQueue<i64> = SegQueue::new();
        q.push(37);
        assert_eq!(q.try_pop(), Some(37));
    }

    #[test]
    fn push_pop_2() {
        let q: SegQueue<i64> = SegQueue::new();
        q.push(37);
        q.push(48);
        assert_eq!(q.try_pop(), Some(37));
        assert_eq!(q.try_pop(), Some(48));
    }

    #[test]
    fn push_pop_many_seq() {
        let q: SegQueue<i64> = SegQueue::new();
        for i in 0..200 {
            q.push(i)
        }
        for i in 0..200 {
            assert_eq!(q.try_pop(), Some(i));
        }
    }

    #[test]
    fn push_pop_many_spsc() {
        let q: SegQueue<i64> = SegQueue::new();

        scope(|scope| {
            scope.spawn(|| {
                let mut next = 0;

                while next < CONC_COUNT {
                    if let Some(elem) = q.try_pop() {
                        assert_eq!(elem, next);
                        next += 1;
                    }
                }
            });

            for i in 0..CONC_COUNT {
                q.push(i)
            }
        });
    }

    #[test]
    fn push_pop_many_spmc() {
        fn recv(_t: i32, q: &SegQueue<i64>) {
            let mut cur = -1;
            for _i in 0..CONC_COUNT {
                if let Some(elem) = q.try_pop() {
                    assert!(elem > cur);
                    cur = elem;

                    if cur == CONC_COUNT - 1 { break }
                }
            }
        }

        let q: SegQueue<i64> = SegQueue::new();
        let qr = &q;
        scope(|scope| {
            for i in 0..3 {
                scope.spawn(move || recv(i, qr));
            }

            scope.spawn(|| {
                for i in 0..CONC_COUNT {
                    q.push(i);
                }
            })
        });
    }

    #[test]
    fn push_pop_many_mpmc() {
        enum LR { Left(i64), Right(i64) }

        let q: SegQueue<LR> = SegQueue::new();

        scope(|scope| {
            for _t in 0..2 {
                scope.spawn(|| {
                    for i in CONC_COUNT-1..CONC_COUNT {
                        q.push(LR::Left(i))
                    }
                });
                scope.spawn(|| {
                    for i in CONC_COUNT-1..CONC_COUNT {
                        q.push(LR::Right(i))
                    }
                });
                scope.spawn(|| {
                    let mut vl = vec![];
                    let mut vr = vec![];
                    for _i in 0..CONC_COUNT {
                        match q.try_pop() {
                            Some(LR::Left(x)) => vl.push(x),
                            Some(LR::Right(x)) => vr.push(x),
                            _ => {}
                        }
                    }

                    let mut vl2 = vl.clone();
                    let mut vr2 = vr.clone();
                    vl2.sort();
                    vr2.sort();

                    assert_eq!(vl, vl2);
                    assert_eq!(vr, vr2);
                });
            }
        });
    }
}
