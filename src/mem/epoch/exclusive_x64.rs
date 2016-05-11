/// Like mem::epoch::AtomicPtr, but provides an ll/sc based api on x86, powerpc, arm, aarch64

use std::mem;
use std::sync::atomic;
use std::marker::PhantomData;


#[repr(C)]
#[derive(Copy, Clone)]
struct Llsc {
    val: u64,
    counter: u64,
    extra: u64, //we adjust which is actually the real one due to alignment
}

// please add alignment specifications...
impl Llsc {
    pub unsafe fn get_ptr(&self) -> *const u64 {
        let addr: *const u64 = &self.counter;
        let addr64 = addr as u64;
        (addr64 & !15) as *const u64 // if &val is 16 byte aligned, will round down
        // otherwise, will keep address of counter
    }

    pub unsafe fn get_vals(&self) -> (u64, u64) {
        let adj_ptr = self.get_ptr();
        if adj_ptr == mem::transmute(self) {
            (self.val, self.counter)
        }
        else {
            (self.counter, self.extra)
        }
    }

    pub unsafe fn set_vals(&mut self, vals: (u64, u64)) {
        let adj_ptr = self.get_ptr() as u64;
        let self_addr = (self as *mut Llsc) as u64;
        if adj_ptr == self_addr {
            self.val = vals.0;
            self.counter = vals.1;
        }
        else {
            self.counter = vals.0;
            self.extra = vals.0;
        }
    }
}


#[repr(C)]
#[derive(Copy, Clone)]
struct LlscSmall {
    value: u32,
    counter: u32,
}


const BFLAG: u64 = 1 << 63;

#[repr(C)]
#[derive(Copy, Clone)]
struct LlscBool {
    combined: u64,
}

impl LlscBool {
    pub fn get_bool(&self) -> bool {
        (self.combined & BFLAG) != 0
    }
}

#[inline(always)]
unsafe fn cas_tagged(ptr: *const u64, old: (u64, u64), nval: u64) -> (u64, u64) {
    let val: u64 = old.0;
    let counter: u64 = old.1;
    let mut ncounter: u64 = counter.wrapping_add(1);
    let mut new = nval;
    println!("Here!");
    dummyfn();
    asm!("lock cmpxchg16b ($6)\n\t"
         : "={rbx}" (new), "={rcx}" (ncounter)
         : "{rax}"(val), "{rdx}"(counter), "0"(new), "1"(ncounter), "r"(ptr)
         : "memory"
         : "volatile");
    // either unchanged, or rbx/rcx are reloaded with the actual values
    (new, ncounter)
}


#[repr(C)]
pub struct ExclusivePtr<T> {
    data: Llsc,
    marker: PhantomData<T>,
}

pub struct LinkedPtr<'a, T: 'a> {
    data: (u64, u64),
    ptr: *const u64,
    borrowck: &'a ExclusivePtr<T>,
}

#[inline(never)]
pub fn dummyfn() {}
impl<T> ExclusivePtr<T> {

    pub fn new(ptr: *mut T) -> ExclusivePtr<T> {
        ExclusivePtr {
            data: Llsc {
                val: ptr as u64,
                counter: ptr as u64,
                extra: 0,
            },
            marker: PhantomData,
        }
    }

    pub fn load(&self) -> *mut T {
        unsafe { self.data.get_vals().0 as *mut T }
    }

    pub fn load_linked(&self) -> LinkedPtr<T> {
        unsafe {
            LinkedPtr {
                data: self.data.get_vals(),
                ptr: self.data.get_ptr(),
                borrowck: self,
            }
        }
    }
}

impl<'a, T> LinkedPtr<'a, T> {
    pub fn store_conditional(&self, val: *mut T) -> LinkedPtr<'a, T> {
        unsafe {
            LinkedPtr {
                data: cas_tagged(self.ptr, self.data, val as u64),
                ptr: self.ptr,
                borrowck: self.borrowck,
            }
        }
    }
}


#[cfg(test)]
mod test {
    use super::*;
    use std::ptr;
    #[test]
    fn test_cas () {
        let mut val: u64 = 0;
        let eptr = ExclusivePtr::<u64>::new(ptr::null_mut());
        let mut ll = eptr.load_linked();
        assert_eq!(eptr.load(), ptr::null_mut());
        ll = ll.store_conditional(&mut val);
        assert_eq!(eptr.load(), &mut val as *mut u64);
    }
}
