/// Includes structures and functions for modifying local GC state

#[derive(Clone, Copy, Debug)]
pub struct Options {
    // GC status control options

    /// Will the thread run the local GC, default true
    pub local_gc: bool,

    /// Will the thread run the global GC, default true
    pub global_gc: bool,

    /// Migrates local garbage instead of collecting it, default true
    pub migrate_local: bool,

    /// Allows one to forcibly disable GC
    /// so that misbehaved libraries can't cause latency spikes, default false
    pub force_no_gc: bool,

    // GC collection threshholds

    /// Determines the number of items upon which the collector
    /// will try to do a collection
    /// Default 32
    pub gc_num: usize,

    /// Determines the maximum number of items that will
    /// be collected in a given GC cycle
    /// Default usize_max
    pub items_per_gc: usize,
}

impl Options {
    pub fn new() -> Options {
        Options {
            local_gc: true,
            global_gc: true,
            migrate_local: true,
            force_no_gc: false,

            gc_num: 32,
            items_per_gc: usize::max_value(),
        }
    }

    /// Sets the local gc flag to the specified value
    pub fn with_local_gc<'a>(&'a mut self, val: bool) -> &'a mut Options {
        self.local_gc = val;
        self
    }

    /// Sets the global gc flag to the specified value
    pub fn with_global_gc<'a>(&'a mut self, val: bool) -> &'a mut Options {
        self.global_gc = val;
        self
    }

    pub fn with_migrate_local<'a>(&'a mut self, val: bool) -> &'a mut Options {
        self.migrate_local = val;
        self
    }

    /// Prevents any form of GC and prevents nested scopes from re-enabling
    pub unsafe fn with_force_no_gc<'a>(&'a mut self, val: bool) -> &'a mut Options {
        if !self.force_no_gc { self.force_no_gc = val; }
        self
    }

    /// Sets global and local GC to the specified values
    pub fn set_gc<'a>(&'a mut self, val: bool) -> &'a mut Options {
        self.with_local_gc(val).with_global_gc(val)
    }

    /// Disables global and local GC
    pub fn disable_gc<'a>(&'a mut self) -> &'a mut Options {
        self.set_gc(false)
    }

    /// Enables global and local GC
    pub fn enable_gc<'a>(&'a mut self) -> &'a mut Options {
        self.set_gc(true)
    }

    pub fn will_run_local_gc(&self) -> bool {
        !self.force_no_gc & self.local_gc
    }

    pub fn will_run_global_gc(&self) -> bool {
        !self.force_no_gc & self.global_gc
    }



    pub fn with_gc_num<'a>(&'a mut self, val: usize) -> &'a mut Options {
        self.gc_num = val;
        self
    }

    pub fn with_items_per_gc<'a>(&'a mut self, val: usize) -> &'a mut Options {
        self.items_per_gc = val;
        self
    }

}
