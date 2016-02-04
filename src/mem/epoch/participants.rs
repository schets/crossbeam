// Manages the global participant list, which is an intrustive list in
// which items are lazily removed on traversal (after being
// "logically" deleted by becoming inactive.)

use std::mem;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::Ordering::{Relaxed, Acquire, Release, SeqCst};
use std::sync::atomic::{AtomicBool, fence};
use mem::epoch::{Atomic, Owned, Guard};
use mem::epoch::participant::Participant;
use mem::CachePadded;

/// Global, threadsafe list of threads participating in epoch management.
pub struct Participants {
    head: Atomic<ParticipantNode>,
    writable: AtomicBool,
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
            writable: AtomicBool::new(false),
        }
    }

    #[cfg(feature = "nightly")]
    pub const fn new() -> Participants {
        Participants {
            head: Atomic::null(),
            writable: AtomicBool::new(false),
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
            is_first: true,
            writable: &self.writable,
            can_write: false,
        }
    }
}

pub struct Iter<'a> {
    // pin to an epoch so that we can free inactive nodes
    guard: &'a Guard,
    next: &'a Atomic<ParticipantNode>,

    // an Acquire read is needed only for the first read, due to release
    // sequences
    is_first: bool,

    // The boolean lock variable
    writable: &'a AtomicBool,

    // Does this iterator have write privlidges
    can_write: bool,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Participant;
    fn next(&mut self) -> Option<&'a Participant> {
        let was_first = self.is_first;
        let mut cur = if self.is_first {
            self.is_first = false;
            self.next.load(Acquire, self.guard)
        } else {
            self.next.load(Relaxed, self.guard)
        };

        while let Some(n) = cur {
            // attempt to clean up inactive nodes
            if !n.active.load(Relaxed) {
                fence(Acquire);
                cur = n.next.load(Relaxed, self.guard);

                // Don't write to first since there are always many appenders
                if !was_first {
                    if !self.can_write {
                        //sanity check to avoid pointless cas
                        if self.writable.load(Relaxed) {
                            continue;
                        }
                        // gain write properties
                        // keep them since this thread likely to see rest of inactives
                        self.can_write = !self.writable.compare_and_swap(false, true, Relaxed);

                        if !self.can_write {
                            continue;
                        }
                    }

                    // Unlink the node! No cas shenanigans since we are only deleter
                    self.next.store_shared(cur, SeqCst);
                    unsafe { self.guard.unlinked(n); }
                }

            } else {
                self.next = &n.next;
                return Some(&n)
            }
        }

        if self.can_write {
            self.writable.store(false, Release);
        }
        None
    }
}
