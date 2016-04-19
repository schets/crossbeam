use std::sync::atomic::{AtomicPtr, AtomicBool, AtomicUsize, fence};
use std::sync::atomic::Ordering::{Relaxed, Acquire, Release};
use std::cell::UnsafeCell;
use std::thread;
use std::mem;
use std::ptr;

const WAITING: usize = 0;
const IN_PROGRESS: usize = 1;
const COMPLETED: usize = 2;
const POISONED: usize = 3;

struct Message {
    op: unsafe fn(),
    next: AtomicPtr<Message>,
    status: AtomicUsize,
}

impl Message {
    pub fn new(op: fn()) -> Message {
        Message {
            op: op,
            next: AtomicPtr::new(ptr::null_mut()),
            status: AtomicUsize::new(WAITING),
        }
    }

    pub fn process(&self) {
        self.status.store(IN_PROGRESS, Relaxed);

        unsafe { (self.op)() };
        self.status.store(COMPLETED, Release);
        self.awaken();
    }

    fn awaken(&self) {}

    pub fn propagate_panic(&self) {}
}

unsafe impl Send for Message {}

pub struct FlatCombiner {
    /// An atomic stackish structure
    message_stack_head: AtomicPtr<Message>,

    /// A queue of messages local to the datastructure
    local_messages: Cell<*mut Message>,

    used: AtomicBool,

    poisoned: AtomicBool,
}

impl FlatCombiner {
    pub fn new() -> FlatCombiner {
        FlatCombiner {
            message_stack_head: AtomicPtr::new(ptr::null_mut()),
            local_messages: Cell::new(ptr::null_mut()),
            used: AtomicBool::new(false),
            poisoned: AtomicBool::new(false),
        }
    }

    fn load_messages(&self) -> bool {
        // Make the local_messages check an invariant?
        if self.local_messages.get() == ptr::null_mut() &&
           self.message_stack_head.load(Relaxed) != ptr::null_mut() {
            let head = self.message_stack_head.swap(ptr::null_mut(), Acquire);
            self.local_messages.set(head);
            true
        }
        else { false }
    }

    fn get_and_process(&self) -> usize {
        0
    }

    // No panic handling yet...
    fn read_messages(&self, n_max: usize) {
        for i in 0..n_max {
            if !self.get_and_process() { break; }
        }
    }

    fn alert_next(&self) {
    }

    fn poison_self(&self) {}

    fn try_operation<F: Send + FnOnce()>(&self, op: *const F) -> bool {
        let lock_val = self.used.load(Relaxed);
        if lock_val || !self.used.compare_and_swap(false, true, Acquire) {
            false
        }
        else {
            self.read_messages(20);
            true
        }
    }

    pub fn submit<F: Send + FnOnce()>(&self, op: F) {
        if !self.try_operation(&op) {
            let mut message = Message::new(unsafe { mem::transmute(&op) });
            loop {
                let cur_head = self.message_stack_head.load(Relaxed);
                message.next.store(cur_head, Relaxed);
                let old_head = self.message_stack_head.compare_and_swap(cur_head,
                                                                        &mut message,
                                                                        Release);
                if old_head == cur_head {
                    break;
                }
            }
            while message.status.load(Relaxed) != COMPLETED { thread::yield_now(); }
            fence(Acquire)
        }
    }
}
