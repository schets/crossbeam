use std::sync::atomic::Ordering::{Acquire, Release, Relaxed};
use std::ptr;
use std::mem;

use mem::epoch::{self, Atomic, Owned, Shared};
use mem::CachePadded;

const ELIMINATION_SIZE: usize = 8;

/// Treiber's lock-free stack.
///
/// Usable with any number of producers and consumers.
pub struct EliminationStack<T> {
    head: CachePadded<Atomic<Node<T>>>,
    elimination: [Elimination; ELIMINATION_SIZE],
}

struct Node<T> {
    data: T,
    next: Atomic<Node<T>>,
}

struct Elimination {
    node: CachePadded<Atomic<Node<T>>>,
    finished: AtomicBool,
}

enum EliminationType<'a> {
    Push(Shared<'a, Node>),
    Pop(Shared<'a, Node>),
    Empty(),
}

// each pointer is atomically stored along with push/pop state in the bits
fn as_push(orig: Shared<Node>) -> Shared<Node> {
    unsafe { Shared::from_raw(1 | (orig as usize)) }
}

fn as_pop(orig: Shared<Node>) -> Shared<Node> {
    orig
}

fn which_type<'a>(orig: Shared<Node>) -> EliminationType {
    let orig_ptr = orig.as_raw()
    let ptr_usize = orig_ptr as usize;
    let is_push = (1 & ptr_usize) != 0;
    if is_push {
        Push(unsafe { Shared::from_raw((ptr_usize ^ 1) as *mut Node) });
    }
    else if orig_ptr != ptr::null_mut() {
        Pop(orig)
    }
    else {
        Empty()
    }
}

fn ptr_to_rng(val: *mut Node) -> usize {
    let usize_val = val as usize;
    (usize_val * 2862933555777941757 + 3037000493) % ELIMINATION_SIZE
}

impl<T> EliminationStack<T> {
    /// Create a new, empty stack.
    pub fn new() -> EliminationStack<T> {
        let mut rval = EliminationStack {
            head: CachePadded::new(Atomic::null()),
            elimination: unsafe { mem::uninitialized() },
        };
        for e in rval.elimination.iter_mut() {
            *e = CachePadded::new(Atomic::null());
        }
        rval
    }

    fn try_push_elim(&self, node: Shared<Node>, g: &Guard) {
        let index = ptr_to_rng(node.as_raw());
        match self.elimination[index].node.load(Acquire, g) {

        };
    }

    /// Push `t` on top of the stack.
    pub fn push(&self, t: T) {
        let mut n = Owned::new(Node {
            data: t,
            next: Atomic::null()
        });
        let guard = epoch::pin();
        loop {
            let head = self.head.load(Relaxed, &guard);
            n.next.store_shared(head, Relaxed);
            match self.head.cas_and_ref(head, n, Release, &guard) {
                Ok(_) => break,
                Err(owned) => n = owned,
            }
        }
    }

    /// Attempt to pop the top element of the stack.
    ///
    /// Returns `None` if the stack is observed to be empty.
    pub fn pop(&self) -> Option<T> {
        let guard = epoch::pin();
        loop {
            match self.head.load(Acquire, &guard) {
                Some(head) => {
                    let next = head.next.load(Relaxed, &guard);
                    if self.head.cas_shared(Some(head), next, Release) {
                        unsafe {
                            guard.unlinked(head);
                            return Some(ptr::read(&(*head).data))
                        }
                    }
                }
                None => return None
            }
        }
    }

    /// Check if this queue is empty.
    pub fn is_empty(&self) -> bool {
        let guard = epoch::pin();
        self.head.load(Acquire, &guard).is_none()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn is_empty() {
        let q: EliminationStack<i64> = EliminationStack::new();
        assert!(q.is_empty());
        q.push(20);
        q.push(20);
        assert!(!q.is_empty());
        assert!(!q.is_empty());
        assert!(q.pop().is_some());
        assert!(q.pop().is_some());
        assert!(q.is_empty());
        q.push(25);
        assert!(!q.is_empty());
    }
}
