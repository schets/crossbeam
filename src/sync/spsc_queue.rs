//! SPSC ringbuffer

use std::sync::atomic::Ordering::{Acquire, Release, Relaxed};
use std::sync::atomic::{AtomicUsize, AtomicBool, AtomicPtr};
use std::sync::Arc;
use std::ptr;
use std::mem;
use std::cell::UnsafeCell;
use std::marker::PhantomData;
use mem::CachePadded;

const SEG_SIZE: usize = 64;

struct Segment<T> {
    data: [UnsafeCell<T>; SEG_SIZE],
    next: AtomicPtr<Segment<T>>,
}

impl<T> Segment<T> {
    pub fn new() -> Segment<T> {
        Segment {
            data: unsafe { mem::uninitialized() },
            next: AtomicPtr::new(ptr::null_mut()),
        }
    }
}

/// A single-producer, single consumer queue
pub struct SpscQueue<T: Send> {
    cache_stack: AtomicPtr<Segment<T>>,
    cache_size: AtomicUsize,
    _marker: PhantomData<T>,

    // These dummies result in a tremendous performance improvement, ~300%+
    _dummy_1: CachePadded<u64>,
    // data for the consumer
    head: AtomicUsize,
    head_block: AtomicPtr<Segment<T>>,
    tail_cache: AtomicUsize,
    prod_alive: AtomicBool, //seems weird, but consumer will read this

    _dummy_2: CachePadded<u64>,
    // data for the producer
    tail: AtomicUsize,
    tail_block: AtomicPtr<Segment<T>>,
    cons_alive: AtomicBool, //seems weird, but producer will read this
}

unsafe impl<T: Send> Send for SpscQueue<T> {}

impl<T: Send> SpscQueue<T> {
    pub fn new() -> (BoundedProducer<T>, BoundedConsumer<T>) {
        let first_block = Box::into_raw(Box::new(Segment::new()));
        let q = SpscQueue {
            cache_stack: AtomicPtr::new(ptr::null_mut()),
            cache_size: AtomicUsize::new(0),
            _marker: PhantomData,

            _dummy_1: CachePadded::zeroed(),
            head: AtomicUsize::new(1),
            head_block: AtomicPtr::new(first_block),
            tail_cache: AtomicUsize::new(1),
            prod_alive: AtomicBool::new(true),

            _dummy_2: CachePadded::zeroed(),
            tail: AtomicUsize::new(1),
            tail_block: AtomicPtr::new(first_block),
            cons_alive: AtomicBool::new(true),
        };
        let qarc = Arc::new(q);
        let rtuple = (BoundedProducer::new(qarc.clone()),
                      BoundedConsumer::new(qarc));
        rtuple
    }

    //#[inline(always)]
    fn acquire_segment(&self) -> *mut Segment<T> {
        let mut chead = self.cache_stack.load(Acquire);
        loop {
            if chead == ptr::null_mut() {
                return Box::into_raw(Box::new(Segment::new()));
            }
            let next = unsafe { (*chead).next.load(Relaxed) };
            let cas = self.cache_stack.compare_and_swap(chead, next, Acquire);
            if cas == chead {
                self.cache_size.fetch_sub(1, Relaxed);
                unsafe { (*chead).next.store(ptr::null_mut(), Relaxed); }
                return chead
            }
            chead = cas;
        }
    }

    //#[inline(always)]
    fn release_segment(&self, seg: *mut Segment<T>) {
        // Does this need to be acquire? Consume is definitely safe here...
        let mut chead = self.cache_stack.load(Relaxed);
        loop {
            if self.cache_size.load(Relaxed) > 3 {
                unsafe { Box::from_raw(seg); }
                return
            }
            unsafe { (*seg).next.store(chead, Relaxed); }
            let cas = self.cache_stack.compare_and_swap(chead, seg, Release);
            if cas == chead {
                self.cache_size.fetch_add(1, Relaxed);
                break;
            }
            chead = cas;
        }
    }

    /// Tries constructing the element and inserts into the queue
    ///
    /// Returns the closure if there isn't space
    //#[inline(always)]
    pub fn try_construct<F>(&self, ctor: F)
                            -> Result<(), F> where F: FnOnce() -> T {
        let ctail = self.tail.load(Relaxed);
        let next_tail = ctail.wrapping_add(1);
        //SEG_SIZE is a power of 2, so this is cheap
        let write_ind = ctail % SEG_SIZE;
        let mut tail_block = self.tail_block.load(Relaxed);
        if write_ind == 0 {
            // try to get another segment
            let next = self.acquire_segment();
            unsafe { (*tail_block).next.store(next, Relaxed); }
            tail_block = next;
            self.tail_block.store(next, Relaxed);
        }
        unsafe {
            let data_pos = (*tail_block).data[write_ind].get();
            ptr::write(data_pos, ctor());
        }
        self.tail.store(next_tail, Release);
        Ok(())
    }

    pub fn try_pop(&self) -> Option<T> {
        let chead = self.head.load(Relaxed);
        if chead == self.tail_cache.load(Relaxed) {
            let cur_tail = self.tail.load(Acquire);
            self.tail_cache.store(cur_tail, Relaxed);
            if chead == cur_tail {
                return None;
            }
        }

        let next_head = chead + 1;
        let read_ind = chead % SEG_SIZE;
        let mut head_block = self.head_block.load(Relaxed);
        if read_ind == 0 {
            // Acquire is not needed because this can only happen
            // once the head/tail have moved appropriately (and synchronized)
            let next = unsafe{ (*head_block).next.load(Relaxed) };
            if next == ptr::null_mut() {
                return None;
            }
            self.release_segment(head_block);
            head_block = next;
            self.head_block.store(next, Relaxed);
        }
        unsafe {
            let data_pos = (*head_block).data[read_ind].get();
            let rval = Some(ptr::read(data_pos));
            // Nothing synchronizes with the head! so the store can be relaxed
            self.head.store(next_head, Relaxed);
            rval
        }
    }

    pub fn capacity(&self) -> usize {0}
}


impl<T: Send> Drop for SpscQueue<T> {
    fn drop(&mut self) {
        loop {
            if let None = self.try_pop() {
                break;
            }
        }
        let head_block = self.head_block.load(Relaxed);
        unsafe { Box::from_raw(head_block); }
        let tail_block = self.tail_block.load(Relaxed);
        if tail_block != head_block {
            unsafe { Box::from_raw(tail_block); }
        }

        let mut cache_head = self.cache_stack.load(Relaxed);
        while cache_head != ptr::null_mut() {
            unsafe {
                let next = (*cache_head).next.load(Relaxed);
                Box::from_raw(cache_head);
                cache_head = next;
            }
        }
    }
}

/// The consumer proxy for the SpscQueue
pub struct BoundedConsumer<T: Send> {
    spsc: Arc<SpscQueue<T>>,
}

unsafe impl<T: Send> Send for BoundedConsumer<T> {}

impl<T: Send> Drop for BoundedConsumer<T> {
    fn drop(&mut self) {
        self.spsc.cons_alive.store(false, Release);
    }
}

impl<T: Send> BoundedConsumer<T> {
    pub fn new(queue: Arc<SpscQueue<T>>) -> BoundedConsumer<T> {
        BoundedConsumer {
            spsc: queue,
        }
    }

    /// Creates a new producer if the current one is dead
    pub fn create_producer(&self) -> Option<BoundedProducer<T>> {
        if self.spsc.prod_alive.load(Acquire) { return None };
        let rval = Some(BoundedProducer::new(self.spsc.clone()));
        self.spsc.prod_alive.store(true, Release);
        rval
    }

    /// Queries whether the producer is currently alive
    //#[inline(always)]
    pub fn is_producer_alive(&self) -> bool {
        self.spsc.prod_alive.load(Relaxed)
    }

    /// Attempts to pop an element from the queue
    //#[inline(always)]
    pub fn try_pop(&self) -> Option<T> {
        self.spsc.try_pop()
    }

    //#[inline(always)]
    pub fn capacity(&self) -> usize {
        self.spsc.capacity()
    }
}

/// The producer proxy for the SpscQueue
pub struct BoundedProducer<T: Send> {
    spsc: Arc<SpscQueue<T>>,
}

unsafe impl<T: Send> Send for BoundedProducer<T> {}

impl<T: Send> Drop for BoundedProducer<T> {
    fn drop(&mut self) {
        self.spsc.prod_alive.store(false, Release);
    }
}

impl<T: Send> BoundedProducer<T> {
    fn new(queue: Arc<SpscQueue<T>>) -> BoundedProducer<T> {
        BoundedProducer {
            spsc: queue,
        }
    }

    /// Creates a new consumer if the current one is dead
    pub fn create_consumer(&self) -> Option<BoundedConsumer<T>> {
        if self.spsc.cons_alive.load(Acquire) { return None }
        let rval = Some(BoundedConsumer::new(self.spsc.clone()));
        self.spsc.cons_alive.store(true, Release);
        rval
    }

    /// Queries whether the consumer is currently alive
    //#[inline(always)]
    pub fn is_consumer_alive(&self) -> bool {
        self.spsc.cons_alive.load(Relaxed)
    }

    /// Tries pushing the element onto the queue
    ///
    /// Returns an error with the element if the queue is full
    /// or consumer disconnected
    //#[inline(always)]
    pub fn try_push(&self, val: T) -> Result<(), T> {
        if !self.is_consumer_alive() {
            return Err(val);
        }
        self.try_construct(|| val).map_err(|f| f())
    }

    /// If there's room in the queue, constructs and inserts an element
    ///
    /// Returns an error with the constructor if the queue is full
    /// or consumer disconnected
    //#[inline(always)]
    pub fn try_construct<F>(&self, ctor: F) -> Result<(), F>
        where F: FnOnce() -> T {
        if !self.is_consumer_alive() {
            return Err(ctor);
        }
        self.spsc.try_construct(ctor)
    }

    //#[inline(always)]
    pub fn capacity(&self) -> usize {
        self.spsc.capacity()
    }
}

#[allow(unused_must_use)]
#[cfg(test)]
mod test {

    use scope;
    use super::*;
    use std::sync::atomic::Ordering::{Relaxed};
    use std::sync::atomic::AtomicUsize;
    const CONC_COUNT: i64 = 10000000;

    #[test]
    fn push_pop_1_b() {
        let (prod, cons) = SpscQueue::<i64>::new();
        assert_eq!(prod.try_push(37), Ok(()));
        assert_eq!(cons.try_pop(), Some(37));
        assert_eq!(cons.try_pop(), None)
    }


    #[test]
    fn push_pop_2_b() {
        let (prod, cons) = SpscQueue::<i64>::new();
        assert_eq!(prod.try_push(37).is_ok(), true);
        assert_eq!(prod.try_construct(|| 48).is_ok(), true);
        assert_eq!(cons.try_pop(), Some(37));
        assert_eq!(cons.try_pop(), Some(48));
        assert_eq!(cons.try_pop(), None)
    }

    #[test]
    fn push_pop_many_seq() {
        let (prod, cons) = SpscQueue::<i64>::new();
        for i in 0..200 {
            assert_eq!(prod.try_push(i).is_ok(), true);
        }
        for i in 0..200 {
            assert_eq!(cons.try_pop(), Some(i));
        }
    }

    struct Dropper<'a> {
        aref: &'a AtomicUsize,
    }

    impl<'a> Drop for Dropper<'a> {
        fn drop(& mut self) {
            self.aref.fetch_add(1, Relaxed);
        }
    }

    #[test]
    fn drop_on_dtor() {
        let msize = 100;
        let drop_count = AtomicUsize::new(0);
        {
            let (prod, _) = SpscQueue::new();
            for _ in 0..msize {
                prod.try_push(Dropper{aref: &drop_count});
            };
        }
        assert_eq!(drop_count.load(Relaxed), msize);
    }

    #[test]
    fn push_pop_many_spsc() {
        for _ in 0..1000 {
            let (prod, cons) = SpscQueue::<i64>::new();

            scope(|scope| {
                scope.spawn(move || {
                    let mut next = 0;

                    while next < CONC_COUNT {
                        if let Some(elem) = cons.try_pop() {
                            assert_eq!(elem, next);
                            next += 1;
                        }
                    }
                });

                let mut i = 0;
                while i < CONC_COUNT {
                    match prod.try_push(i) {
                        Err(_) => continue,
                        Ok(_) => {i += 1;},
                    }
                }
            });
        }
    }
    #[test]
    fn test_life_queries() {
        let (prod, cons) = SpscQueue::<i64>::new();
        assert_eq!(prod.is_consumer_alive(), true);
        assert_eq!(cons.is_producer_alive(), true);
        assert_eq!(prod.try_push(1), Ok(()));
        {
            let _x = cons;
            assert_eq!(prod.is_consumer_alive(), true);
            assert_eq!(prod.create_consumer().is_none(), true);
        }
        assert_eq!(prod.is_consumer_alive(), false);
        assert_eq!(prod.try_push(1), Err(1));
        let new_cons_o = prod.create_consumer();
        assert_eq!(prod.is_consumer_alive(), true);
        assert_eq!(new_cons_o.is_some(), true);
        assert_eq!(prod.create_consumer().is_none(), true);
        let new_cons = new_cons_o.unwrap();
        {
            let _x = prod;
            assert_eq!(new_cons.is_producer_alive(), true);
            assert_eq!(new_cons.create_producer().is_none(), true);
        }
        assert_eq!(new_cons.is_producer_alive(), false);
        assert_eq!(new_cons.try_pop(), Some(1));
        let new_prod = new_cons.create_producer();
        assert_eq!(new_prod.is_some(), true);
        assert_eq!(new_cons.create_producer().is_none(), true);
    }
}
