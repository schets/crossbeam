// Definition of global epoch state. The `get` function is the way to
// access this data externally (until const fn is stabilized...).

use std::sync::atomic::{AtomicUsize, AtomicBool};

use mem::CachePadded;
use mem::epoch::garbage;
use mem::epoch::participants::Participants;

/// Global epoch state
pub struct EpochState {
    /// Current global epoch
    pub epoch: CachePadded<AtomicUsize>,

    /// Flag to pressure threads into doing global GC
    pub do_global: CachePadded<AtomicBool>,

    // FIXME: move this into the `garbage` module, rationalize API
    /// Global garbage bags
    pub garbage: [CachePadded<garbage::ConcBag>; 3],

    /// Participant list
    pub participants: Participants,
}

unsafe impl Send for EpochState {}
unsafe impl Sync for EpochState {}

pub use self::imp::get;

#[cfg(not(feature = "nightly"))]
mod imp {
    use std::mem;
    use std::sync::atomic::{self, AtomicUsize};
    use std::sync::atomic::Ordering::Relaxed;

    use super::EpochState;
    use mem::CachePadded;
    use mem::epoch::participants::Participants;

    impl EpochState {
        fn new() -> EpochState {
            EpochState {
                epoch: CachePadded::zeroed(),
                do_global: CachePadded::zeroed(),
                garbage: [CachePadded::zeroed(),
                          CachePadded::zeroed(),
                          CachePadded::zeroed()],
                participants: Participants::new(),
            }
        }

        pub fn has_garbage(&self) -> bool {
            self.garbage.iter().any(|x| x.has_garbage())
        }

        /// Sets the global garbage flag if needed by the program
        ///
        /// Is racy, but that's ok - "bad" calls will result in an extra GC
        /// check or a delayed GC check - since there shouldn't be too much
        /// contention this won't be a big deal and each "bad" case will be
        /// resolved fairly quickly anyways
        pub fn set_garbage_flag(&self) {
            let has_garbage = self.has_garbage();
            let garbage_flag = self.do_global.load(Relaxed);
            if has_garbage && !garbage_flag {
                self.do_global.store(true, Relaxed);
            }
            else if !has_garbage && garbage_flag {
                self.do_global.store(false, Relaxed);
            }
        }
    }

    static EPOCH: AtomicUsize = atomic::ATOMIC_USIZE_INIT;

    pub fn get() -> &'static EpochState {
        let mut addr = EPOCH.load(Relaxed);

        if addr == 0 {
            let boxed = Box::new(EpochState::new());
            let raw = Box::into_raw(boxed);

            addr = EPOCH.compare_and_swap(0, raw as usize, Relaxed);
            if addr != 0 {
                let boxed = unsafe { Box::from_raw(raw) };
                mem::drop(boxed);
            } else {
                addr = raw as usize;
            }
        }

        unsafe {
            &*(addr as *mut EpochState)
        }
    }
}

#[cfg(feature = "nightly")]
mod imp {
    use super::EpochState;
    use mem::CachePadded;
    use mem::epoch::participants::Participants;

    impl EpochState {
        const fn new() -> EpochState {
            EpochState {
                do_global: CachePadded::zeroed(),
                epoch: CachePadded::zeroed(),
                garbage: [CachePadded::zeroed(),
                          CachePadded::zeroed(),
                          CachePadded::zeroed()],
                participants: Participants::new(),
            }
        }
    }

    static EPOCH: EpochState = EpochState::new();
    pub fn get() -> &'static EpochState { &EPOCH }
}
