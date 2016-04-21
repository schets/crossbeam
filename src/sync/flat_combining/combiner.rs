use std::sync::atomic::{AtomicPtr, AtomicBool, AtomicUsize, fence};
use std::sync::atomic::Ordering::{Relaxed, Acquire, Release};
use std::sync::{Mutex, Condvar};
use std::cell::Cell;
use std::thread;
use std::mem;
use std::ptr;

const WAITING: usize = 0;
const IN_PROGRESS: usize = 1;
const TAKE_OVER = 2;
const COMPLETED: usize = 3;
const POISONED: usize = 4;

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

        // Prevent reordering of on_completed after completion?
        // Can't do just on client side, but would really, really like to not
        // have to
        self.on_completed();
    }

    fn awaken(&self) {}

    fn on_completed(&self) {}

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

    // Could really just use a cheaper mechanism
    wakeup: Condvar,

    wakeup_mut: Mutex<bool>,
}
 unsafe impl Send for FlatCombiner {}
 unsafe impl Sync for FlatCombiner {}

impl FlatCombiner {
    pub fn new() -> FlatCombiner {
        FlatCombiner {
            message_stack_head: AtomicPtr::new(ptr::null_mut()),
            local_messages: Cell::new(ptr::null_mut()),
            used: AtomicBool::new(false),
            poisoned: AtomicBool::new(false),
            wakeup: Condvar::new(),
            wakeup_mut: Mutex::new(false),
        }
    }

    fn load_messages(&self) -> Option<*mut Message> {
        // Make the local_messages check an invariant?
        if self.local_messages.get() == ptr::null_mut() &&
           self.message_stack_head.load(Relaxed) != ptr::null_mut() {
            let head = self.message_stack_head.swap(ptr::null_mut(), Acquire);
            self.local_messages.set(head);
            Some(head)
        }
        else { None }
    }

    fn get_a_message(&self) -> Option<*mut Message> {
        let mut mhead = self.local_messages.get();
        if mhead == ptr::null_mut() {
            match self.load_messages() {
                None => return None,
                Some(head) => mhead = head,
            };
        }
        Some(mhead)
    }

    fn get_and_process(&self) -> bool {
        if let Some(message) = self.get_a_message() {
            unsafe { (&*message).process() };
            true
        }
        else { false }
    }

    // No panic handling yet...
    fn read_messages(&self, n_max: usize) {
        for _ in 0..n_max {
            if !self.get_and_process() { break; }
        }
    }

    fn alert_next(&self) {
        self.wakeup.notify_all();
    }

    fn poison_self(&self) {}

    #[inline(always)]
    fn try_operation<F: Send + FnOnce()>(&self, op: F) -> Option<F> {
        let lock_val = self.used.load(Relaxed);
        if lock_val || self.used.compare_and_swap(false, true, Acquire) {
            Some(op)
        }
        else {
            op();
            self.read_messages(20);
            self.alert_next();
            None
        }
    }

    pub fn submit<F: Send + FnOnce() -> R, R: Send>(&self, _op: F) -> R {
        let mut rval: R = unsafe { mem::uninitialized() };
        {
            let rval_ref = &mut rval;
            let op = || *rval_ref = _op();
            if let Some(op) = self.try_operation(op) {
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

                // ewwwwwwwww
                // Move into dedicated function,  will satisfy owner shenanigans
                'goto: loop {
                    for _ in 0..50 {
                        if message.status.load(Relaxed) == COMPLETED {
                            break 'goto;
                        }
                    }

                    loop {
                        if message.status.load(Relaxed) == COMPLETED {
                            break 'goto;
                        }
                        thread::yield_now();
                    }

                    // fat, heavy waiting loop. Hopefully this is never reached
                    // Also, future schemes will let users specify the impl...
                    let mut waiting = self.wakeup_mut.lock().unwrap();
                    while message.status.load(Relaxed) != COMPLETED {
                        waiting = self.wakeup.wait(waiting).unwrap();
                    }
                    break;
                }
                fence(Acquire);
            }
        }
        rval
    }
}

#[cfg(test)]
mod test {

    use scope;
    use super::*;

    #[test]
    pub fn test_basic() {
        let comb = FlatCombiner::new();
        let mut x = 0;

        let rval = comb.submit(|| { x += 1; x+1});
        assert_eq!(rval, 2);
        assert_eq!(x, 1);
    }

    #[test]
    pub fn test_thread() {
        let comb = FlatCombiner::new();
        scope(|scope| {
            scope.spawn( || {
                for _ in 0..1000 {
                    let mut x = 0;

                    let rval = comb.submit(|| { x += 1; x+1});
                    assert_eq!(rval, 2);
                    assert_eq!(x, 1);
                }
            })
        });
    }
}
