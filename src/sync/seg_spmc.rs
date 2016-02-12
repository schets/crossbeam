use std::sync::atomic::Ordering::{Acquire, Release, Relaxed};
use std::sync::atomic::AtomicUsize;
use std::{ptr, mem};
use std::cmp;
use std::cell::UnsafeCell;

use mem::epoch::{self, Atomic, Owned, Shared, Guard};

const SEG_SIZE: usize = 256;

/// A spmc queue that allocates "segments" (arrays of nodes)
/// for efficiency.
///
/// Usable with one producer and any number of consumers
pub struct SegSpmc<T> {
    head: Atomic<Segment<T>>,
    tail: Atomic<Segment<T>>,
}

// This could easily be abtracted into a central area
//#[repr(C)]
struct Segment<T> {
    low: AtomicUsize,
    _dummy: [usize; 8],
    high: AtomicUsize,
    _dummy2: [usize; 8],
    next: Atomic<Segment<T>>,
    data: [UnsafeCell<T>; SEG_SIZE],
}

unsafe impl<T: Send> Sync for Segment<T> {}

impl<T> Segment<T> {
    fn new() -> Segment<T> {
        Segment {
            data: unsafe { mem::uninitialized() },
            low: AtomicUsize::new(0),
            _dummy: unsafe { mem::uninitialized() },
            _dummy2: unsafe { mem::uninitialized() },
            high: AtomicUsize::new(0),
            next: Atomic::null(),
        }
    }
}

impl<T> SegSpmc<T> {
    /// Create a new, empty queue.
    pub fn new() -> SegSpmc<T> {
        let q = SegSpmc {
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
        // The epoch doesn't need to be pinned here since the segment pointed
        // to by tail can only be unlinked once this has left the current tail
        unsafe {
            let guard = epoch::fake_pin();

            let tail = self.tail.load(Relaxed, &guard).unwrap();
            let i = tail.high.load(Relaxed);
            let cell = (*tail).data.get_unchecked(i).get();
            ptr::write(cell, t);

            if i + 1 == SEG_SIZE {
                let ntail = Owned::new(Segment::new());
                let tail = tail.next.store_and_ref(ntail, Relaxed, &guard);
                self.tail.store_shared(Some(tail), Relaxed);
            }
            // Only once this advances to a new tail can the old one
            // even be unlinked, so we don't have to ever pin the epoch
            tail.high.store(i + 1, Release);
        }
    }

    fn try_advance_head(&self, head: Shared<Segment<T>>, guard: &Guard) {
        if let Some(next) = head.next.load(Acquire, &guard) {
            if head.as_raw() == self.head.load(Relaxed, &guard).unwrap().as_raw() {
                if self.head.cas_shared(Some(head), Some(next), Release) {
                    unsafe { guard.unlinked(head) };
                }
            }
        }
    }

    /// Attempt to dequeue from the front.
    ///
    /// Returns `None` if the queue is observed to be empty.
    pub fn try_pop(&self) -> Option<T> {
        loop {
            let guard = epoch::pin();
            let head = self.head.load(Acquire, &guard).unwrap();
            loop {
                let curhigh = head.high.load(Relaxed);
                let low = head.low.load(Relaxed);
                if low >= cmp::min(curhigh, SEG_SIZE) { break; }
                // There's a version that uses fetch_add and should be faster
                // but I can't get it to work...
                let attempt = head.low.fetch_add(1, Acquire);
                if attempt < curhigh {
                    unsafe {
                        let cell = (*head).data.get_unchecked(attempt).get();
                        if attempt + 1 == SEG_SIZE {
                            self.try_advance_head(head, &guard);
                        }
                        return Some(ptr::read(cell))
                    }
                }
                if curhigh < SEG_SIZE {
                    head.low.fetch_sub(1, Relaxed);
                }
            }
            if head.next.load(Relaxed, &guard).is_none() { return None }
            if head.low.load(Relaxed) >= SEG_SIZE {
                self.try_advance_head(head, &guard);
            }
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
        let q: SegSpmc<i64> = SegSpmc::new();
        q.push(37);
        assert_eq!(q.try_pop(), Some(37));
    }

    #[test]
    fn push_pop_2() {
        let q: SegSpmc<i64> = SegSpmc::new();
        q.push(37);
        q.push(48);
        assert_eq!(q.try_pop(), Some(37));
        assert_eq!(q.try_pop(), Some(48));
    }

    #[test]
    fn push_pop_many_seq() {
        let q: SegSpmc<i64> = SegSpmc::new();
        for i in 0..200 {
            q.push(i)
        }
        for i in 0..200 {
            assert_eq!(q.try_pop(), Some(i));
        }
    }

    #[test]
    fn push_pop_many_spsc() {
        let q: SegSpmc<i64> = SegSpmc::new();

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
        fn recv(_t: i32, q: &SegSpmc<i64>) {
            let mut cur = -1;
            for _i in 0..CONC_COUNT {
                if let Some(elem) = q.try_pop() {
                    assert!(elem > cur);
                    cur = elem;

                    if cur == CONC_COUNT - 1 { break }
                }
            }
        }

        let q: SegSpmc<i64> = SegSpmc::new();
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
}
