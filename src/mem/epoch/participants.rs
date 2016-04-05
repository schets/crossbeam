// Manages the global participant list, which is an intrustive list in
// which items are lazily removed on traversal (after being
// "logically" deleted by becoming inactive.)

use std::mem;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::Ordering::{Relaxed, Acquire, Release};
use std::sync::atomic::AtomicUsize;

use mem::epoch::{Atomic, Owned, Guard};
use mem::epoch::participant::Participant;
use mem::CachePadded;

/// Global, threadsafe list of threads participating in epoch management.
pub struct Participants {
    cleaning: CachePadded<AtomicUsize>,
    head: Atomic<ParticipantNode>
}

pub struct ParticipantNode(CachePadded<Participant>);

impl ParticipantNode {
    pub fn new() -> ParticipantNode {
        ParticipantNode(CachePadded::new(Participant::new()))
    }
}

impl Deref for ParticipantNode {
    type Target = Participant;
    fn deref(&self) -> &Participant {
        &self.0
    }
}

impl DerefMut for ParticipantNode {
    fn deref_mut(&mut self) -> &mut Participant {
        &mut self.0
    }
}

impl Participants {
    #[cfg(not(feature = "nightly"))]
    pub fn new() -> Participants {
        Participants {
            head: Atomic::null(),
            cleaning: CachePadded::new(AtomicUsize::new(0))
        }
    }

    #[cfg(feature = "nightly")]
    pub const fn new() -> Participants {
        Participants {
            head: Atomic::null(),
            cleaning: CachePadded::new(AtomicUsize::new(0))
        }
    }

    /// Enroll a new thread in epoch management by adding a new `Particpant`
    /// record to the global list.
    pub fn enroll(&self) -> *const Participant {
        let mut participant = Owned::new(ParticipantNode::new());

        // we ultimately use epoch tracking to free Participant nodes, but we
        // can't actually enter an epoch here, so fake it; we know the node
        // can't be removed until marked inactive anyway.
        let fake_guard = ();
        let g: &'static Guard = unsafe { mem::transmute(&fake_guard) };
        loop {
            let head = self.head.load(Relaxed, g);
            participant.next.store_shared(head, Relaxed);
            match self.head.cas_and_ref(head, participant, Release, g) {
                Ok(shared) => {
                    let shared: &Participant = &shared;
                    return shared;
                }
                Err(owned) => {
                    participant = owned;
                }
            }
        }
    }

    pub fn iter<'a>(&'a self, g: &'a Guard) -> Iter<'a> {
        Iter {
            guard: g,
            next: &self.head,
            free_lock: &*self.cleaning,
            is_first: true,
        }
    }
}

pub struct Iter<'a> {
    // pin to an epoch so that we can free inactive nodes
    guard: &'a Guard,
    next: &'a Atomic<ParticipantNode>,
    free_lock: &'a AtomicUsize,

    // an Acquire read is needed only for the first read, due to release
    // sequences
    is_first: bool,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Participant;
    fn next(&mut self) -> Option<&'a Participant> {
        let on_head = self.is_first;
        let mut cur = if self.is_first {
            self.is_first = false;
            self.next.load(Acquire, self.guard)
        } else {
            self.next.load(Relaxed, self.guard)
        };

        while let Some(n) = cur {
            // attempt to clean up inactive nodes
            if !n.active.load(Relaxed) {
                cur = n.next.load(Relaxed, self.guard);

                if !on_head
                   && self.free_lock.load(Relaxed) == 0
                   && self.free_lock.swap(1, Acquire) == 0
                   && self.next.cas_shared(Some(n), cur, Relaxed) {
                    unsafe { self.guard.unlinked(n); }
                    self.free_lock.store(0, Release);
                }

            } else {
                self.next = &n.next;
                return Some(&n)
            }
        }

        None
    }
}

impl<'a> Drop for Iter<'a> {
    fn drop(&mut self) {
        if self.free_lock.load(Relaxed) != 0 {
        }
    }
}
