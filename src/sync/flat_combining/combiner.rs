use std::sync::atomic::{AtomicPtr, AtomicBool, AtomicUsize, fence};
use std::sync::atomic::Ordering::{Relaxed, Acquire, Release};
use std::sync::{Mutex, Condvar};
use std::cell::Cell;
use std::thread;
use std::mem;
use std::ptr;

fn prefetch<T>(p: *const T) -> () {
    unsafe { mem::forget(ptr::read_volatile(p)); }
}

const WAITING: usize = 0;
const IN_PROGRESS: usize = 1;
const TAKE_OVER: usize = 2;
const RETRY: usize = 3;
const COMPLETED: usize = 4;
const POISONED: usize = 5;

struct Message {
    op: fn(*mut ()),
    data: *mut (),
    next: AtomicPtr<Message>,
    status: AtomicUsize,
}

struct PtrWrapper {
    pub val: *mut (),
}
unsafe impl Sync for PtrWrapper {}
unsafe impl Send for PtrWrapper {}

impl PtrWrapper {
    pub fn new<T>(p : *mut T) -> PtrWrapper {
        PtrWrapper {val : unsafe { mem::transmute(p) } }
    }

    pub fn get<T>(&self) -> *mut T {
        unsafe {mem::transmute(self.val) }
    }
}

fn run_operation<F: FnMut()>(p: *mut ()) {
    unsafe {
        let fncptr : *mut F() = mem::transmute(p);
        (&mut *fncptr)();
    }
}

impl Message {
    pub fn new<F: FnMut()>(data: *mut F) -> Message {
        Message {
            op: run_operation::<F>,
            data: unsafe { mem::transmute(data) },
            next: AtomicPtr::new(ptr::null_mut()),
            status: AtomicUsize::new(WAITING),
        }
    }

    pub fn process(&self) {
        self.status.store(IN_PROGRESS, Relaxed);
        unsafe { (self.op)(self.data) };
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
        let mnext = unsafe { (*mhead).next.load(Relaxed) };
        self.local_messages.set(mnext);
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
        let mut mhead = self.local_messages.get();
        if mhead == ptr::null_mut() {
            match self.load_messages() {
                Some(head) => mhead = head,
                None => return,
            }
        }
        unsafe { (*mhead).status.store(TAKE_OVER, Release) };
        self.wakeup.notify_all();
    }

    fn run_operation<F: Send + FnOnce()>(&self, op: F) {
        op();
        self.read_messages(20);
        self.alert_next();
    }

    fn try_operation<F: Send + FnOnce()>(&self, op: F) -> Option<F> {
        let lock_val = self.used.load(Relaxed);
        if lock_val || self.used.compare_and_swap(false, true, Acquire) {
            Some(op)
        }
        else {
            self.run_operation(op);
            self.used.store(false, Release);
            None
        }
    }

    fn wait_on(&self, status: &AtomicUsize) -> usize {
        for _ in 0..200 {
            let stat = status.load(Relaxed);
            if stat > IN_PROGRESS {
                return stat;
            }
        }

        loop {
            let stat = status.load(Relaxed);
            if stat > IN_PROGRESS {
                return stat;
            }
            thread::yield_now();
        }

        // fat, heavy waiting loop. Hopefully this is never reached
        // Also, future schemes will let users specify the impl...
        let mut waiting = self.wakeup_mut.lock().unwrap();
        while status.load(Relaxed) <= IN_PROGRESS {
            waiting = self.wakeup.wait(waiting).unwrap();
        }

        return status.load(Relaxed);
    }

    pub fn submit<F: Send + FnMut() -> R, R: Send>(&self, mut _op: F) -> R {
        let mut rval: R = unsafe { mem::uninitialized() };
        {
            let rval_ref = PtrWrapper::new(&mut rval as *mut R);
            let mut dirop = || unsafe { ptr::write(rval_ref.get::<R>(), _op()) };
            if let Some(mut op) = self.try_operation(dirop) {
                loop {
                    let mut message = Message::new(&mut op);
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

                    let status = self.wait_on(&message.status);
                    fence(Acquire);
                    match status {
                        TAKE_OVER => self.run_operation(op),
                        COMPLETED => continue,
                        _ => unreachable!(),
                    };
                    break;
                }
            }
        }
        rval
    }
}

#[cfg(test)]
mod test {

    use scope;
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering::Relaxed;

    #[test]
    pub fn test_basic() {
        let comb = FlatCombiner::new();
        let mut x = 0;

        let rval = comb.submit(|| { x += 1; x+1});
        assert_eq!(rval, 2);
        assert_eq!(x, 1);
        let rval = comb.submit(|| { x += 1; x+1});
        assert_eq!(rval, 3);
        assert_eq!(x, 2);

    }

    #[test]
    pub fn test_thread() {
        let num_loop = 10000;
        let nthread = 4;
        let _comb = FlatCombiner::new();
        let val = AtomicUsize::new(0);
        let in_area = AtomicUsize::new(0);
        scope(|scope| {
            for i in 0..nthread {
                scope.spawn( || {
                    let comb = &_comb;
                    for i in 0..num_loop {
                        comb.submit(|| {
                            let oldval = in_area.fetch_add(1, Relaxed);
                            //assert_eq!(oldval, 0);
                            let cval = val.load(Relaxed);
                            val.store(cval + 1, Relaxed);
                            in_area.fetch_sub(1, Relaxed);
                        });
                    }
                });
            }
        });
        assert_eq!(val.load(Relaxed), nthread*num_loop);
    }
}
