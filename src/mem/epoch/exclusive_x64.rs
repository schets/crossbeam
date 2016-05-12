/// Like mem::epoch::AtomicPtr, but provides an ll/sc based api on x86, powerpc, arm, aarch64

use std::mem;
use std::sync::atomic;
use std::marker::PhantomData;

use std::sync::atomic::{Ordering, AtomicUsize};

#[repr(C)]
#[derive(Copy, Clone)]
struct Llsc {
    val: u64,
    counter: u64,
    extra: u64, //we adjust which is actually the real one due to alignment
}

#[inline(always)]
unsafe fn load_from(ptr: *const u64, ord: Ordering) -> u64 {
    let ptr: *const AtomicUsize = mem::transmute(ptr);
    (&*ptr).load(ord) as u64
}

#[inline(always)]
unsafe fn store_to(ptr: *const u64, n: u64, ord: Ordering) {
    let ptr: *const AtomicUsize = mem::transmute(ptr);
    (&*ptr).store(n as usize, ord)
}

// please add alignment specifications...
impl Llsc {
    pub unsafe fn get_ptr(&self) -> *const u64 {
        let addr: *const u64 = &self.counter;
        let addr64 = addr as u64;
        (addr64 & !15) as *const u64 // if &val is 16 byte aligned, will round down
        // otherwise, will keep address of counter
    }

    pub unsafe fn get_vals(&self, ord: Ordering) -> (u64, u64) {
        let adj_ptr = self.get_ptr();
        if adj_ptr == mem::transmute(self) {
            (load_from(&self.val, ord), self.counter)
        }
        else {
            (load_from(&self.counter, ord), self.extra)
        }
    }

    pub unsafe fn set_val(&self, val: u64, ord: Ordering) {
        let adj_ptr = self.get_ptr() as u64;
        let self_addr = (self as *const Llsc) as u64;
        if adj_ptr == self_addr {
            store_to(&self.val, val, ord);
        }
        else {
            store_to(&self.counter, val, ord);
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
unsafe fn cas_tagged(ptr: *const u64, old: (u64, u64), nval: u64) -> bool {
    let val: u64 = old.0;
    let counter: u64 = old.1;
    let ncounter: u64 = counter.wrapping_add(1);
    let new = nval;
    let succ: bool;
    asm!("lock cmpxchg16b ($5)\n\t
          sete $0\n\t"
         : "=r" (succ)
         : "{rax}"(val), "{rdx}"(counter), "{rbx}"(new), "{rcx}"(ncounter), "r"(ptr)
         : "memory"
         : "volatile");
    succ
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

    pub fn load(&self, ord: Ordering) -> *mut T {
        unsafe { self.data.get_vals(ord).0 as *mut T }
    }

    // Stores directly to the pointer without updating the counter
    //
    // This function can still leave one vulnerable to the ABA problem,
    // But is useful when only used to store to say a null value.
    pub fn store_direct(&self, val: *mut T, ord: Ordering) {
        unsafe { self.data.set_val(val as u64, ord) };
    }

    pub fn load_linked(&self, ord: Ordering) -> LinkedPtr<T> {
        unsafe {
            LinkedPtr {
                data: self.data.get_vals(ord),
                ptr: self.data.get_ptr(),
                borrowck: self,
            }
        }
    }
}

impl<'a, T> LinkedPtr<'a, T> {
    pub fn store_conditional(self, val: *mut T, _: Ordering) -> bool {
        unsafe { cas_tagged(self.ptr, self.data, val as u64) }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::ptr;
    use std::sync::atomic::Ordering::{Relaxed, Acquire, Release};
    #[test]
    fn test_cas () {
        let mut val: u64 = 0;
        let eptr = ExclusivePtr::<u64>::new(ptr::null_mut());
        let ll = eptr.load_linked(Relaxed);
        assert_eq!(eptr.load(Relaxed), ptr::null_mut());
        assert_eq!(ll.store_conditional(&mut val, Relaxed), true);
        assert_eq!(eptr.load(Relaxed), &mut val as *mut u64);
    }

    #[test]
    fn test_cas_fail () {
        let mut val: u64 = 0;
        let mut val2: u64 = 0;
        let eptr = ExclusivePtr::<u64>::new(ptr::null_mut());
        let ll = eptr.load_linked(Relaxed);
        assert_eq!(eptr.load(Relaxed), ptr::null_mut());
        eptr.store_direct(&mut val2, Relaxed);
        assert_eq!(ll.store_conditional(&mut val, Relaxed), false);
    }

}
