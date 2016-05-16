/// Like mem::epoch::AtomicPtr, but provides an ll/sc based api on x86, powerpc, arm, aarch64

use std::mem;
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

#[inline(always)]
unsafe fn exchange_to(ptr: *const u64, n: u64, ord: Ordering) -> u64 {
    let ptr: *const AtomicUsize = mem::transmute(ptr);
    (&*ptr).swap(n as usize, ord) as u64
}


#[inline(always)]
unsafe fn cas_tagged(ptr: *const u64, old: (u64, u64), nval: u64) -> (bool, (u64, u64)) {
    let mut val: u64 = old.0;
    let mut counter: u64 = old.1;
    let ncounter: u64 = counter.wrapping_add(1);
    let new = nval;
    let succ: bool;
    asm!("lock cmpxchg16b ($7)\n\t
          sete $0\n\t"
         : "=r" (succ), "={rax}" (val), "={rdx}" (counter)
         : "1"(val), "2"(counter), "{rbx}"(new), "{rcx}"(ncounter), "r"(ptr)
         : "memory"
         : "volatile");
    // Returned values only matter if succ is false,
    // in which case thee right ones are loaded into memory
    (succ, (val, counter))
}

// All this junk is to ensure 16-byte alignment
// generally working alignment specifiers would solve this and some
// effecient issues, although those will be dominated by cas
impl Llsc {
    pub unsafe fn get_ptr(&self) -> *const u64 {
        let addr: *const u64 = &self.counter;
        let addr64 = addr as u64;
        (addr64 & !15) as *const u64 // if &val is 16 byte aligned, will round down
        // otherwise, will keep address of counter
    }

    pub unsafe fn get_vals(&self, ord: Ordering) -> (u64, u64) {
        let ptr = self.get_ptr();
        (load_from(ptr, ord), *ptr.offset(1))
    }

    pub unsafe fn set_val(&self, val: u64, ord: Ordering) {
        store_to(self.get_ptr(), val, ord);
    }

    pub unsafe fn xchg_val(&self, val: u64, ord: Ordering) -> u64 {
        exchange_to(self.get_ptr(), val, ord)
    }
}

pub trait IsU64 {
    fn from_u64(val: u64) -> Self;
    fn to_u64(&self) -> u64;
}

impl IsU64 for usize {
    fn from_u64(val: u64) -> usize {
        val as usize
    }

    fn to_u64(&self) -> u64 {
        *self as u64
    }
}

impl IsU64 for isize {
    fn from_u64(val: u64) -> isize {
        val as isize
    }

    fn to_u64(&self) -> u64 {
        *self as u64
    }
}

impl<T> IsU64 for *mut T {

    fn from_u64(val: u64) -> *mut T {
        val as *mut T
    }

    fn to_u64(&self) -> u64 {
        *self as u64
    }
}

impl IsU64 for bool {

    fn from_u64(val: u64) -> bool {
        val == 0
    }

    fn to_u64(&self) -> u64 {
        *self as u64
    }
}

pub struct ExclusiveData<T: IsU64> {
    data: Llsc,
    marker: PhantomData<T>,
}

pub struct LinkedData<'a, T: 'a + IsU64> {
    data: (u64, u64),
    ptr: *const u64,
    _borrowck: &'a ExclusiveData<T>,
}

impl<T: IsU64> ExclusiveData<T> {

    pub fn new(val: T) -> ExclusiveData<T> {
        ExclusiveData {
            data: Llsc {
                val: val.to_u64(),
                counter: val.to_u64(),
                extra: 0,
            },
            marker: PhantomData,
        }
    }

    /// Loads the value from the pointer with the given ordering
    pub fn load(&self, ord: Ordering) -> T {
        unsafe { T::from_u64(self.data.get_vals(ord).0) }
    }

    /// Stores directly to the pointer without updating the counter
    ///
    /// This function can still leave one vulnerable to the ABA problem,
    /// But is useful when only used to store to say a null value.
    /// Be careful when using, this must always cause a store_conditional to fail
    pub fn store_direct(&self, val: T, ord: Ordering) {
        unsafe { self.data.set_val(val.to_u64(), ord) };
    }

    /// Stores directly to the pointer without updating the counter
    ///
    /// This function can still leave one vulnerable to the ABA problem,
    /// But is useful when only used to store to say a null value.
    /// Be careful when using, this must always cause a store_conditional to fail
    pub fn exchange_direct(&self, val: T, ord: Ordering) -> T {
        unsafe { T::from_u64(self.data.xchg_val(val.to_u64(), ord)) }
    }

    /// Performs an exclusive load on the pointer
    ///
    /// If the pointer is modified by a different store_conditional in between the load_linked
    /// and store_conditional, this will always fail. This is stronger the cas
    /// since cas can succedd when modifications have occured as long as the end
    /// result is the same. However, this will always fail in a scenario where cas would fail.
    pub fn load_linked(&self, ord: Ordering) -> LinkedData<T> {
        unsafe {
            LinkedData {
                data: self.data.get_vals(ord),
                ptr: self.data.get_ptr(),
                _borrowck: self,
            }
        }
    }
}

impl<'a, T: IsU64> LinkedData<'a, T> {

    pub fn get(&self) -> T {
        T::from_u64(self.data.0)
    }

    /// Performs a conditional store on the pointer, conditional on no modifications occurring
    ///
    /// If the pointer is modified by a different store_conditional in between the load_linked
    /// and store_conditional, this will always fail. This is stronger the cas
    /// since cas can succedd when modifications have occured as long as the end
    /// result is the same. However, this will always fail in a scenario where cas would fail.
    pub fn store_conditional(self, val: T, _: Ordering) -> Option<LinkedData<'a, T>> {
        unsafe {
            let (succ, res) = cas_tagged(self.ptr, self.data, val.to_u64());
            match succ {
                true => None,
                false => Some(LinkedData {
                    data: res,
                    ptr: self.ptr,
                    _borrowck: self._borrowck,
                })
            }
        }
    }
}

unsafe impl<T: IsU64> Send for ExclusiveData<T> {}
unsafe impl<T: IsU64> Sync for ExclusiveData<T> {}

pub type ExclusivePtr<T> = ExclusiveData<*mut T>;
pub type ExclusiveUsize = ExclusiveData<usize>;
pub type ExclusiveIsize = ExclusiveData<isize>;

// This could be more efficient, by doing normal cas and packing
// as u64. BUT! That's code bloat for the time being
pub type ExclusiveBool = ExclusiveData<bool>;

pub type LinkedPtr<'a, T> = LinkedData<'a, *mut T>;
pub type LinkedUsize<'a> = LinkedData<'a, usize>;
pub type LinkedIsize<'a> = LinkedData<'a, isize>;
pub type LinkedBool<'a> = LinkedData<'a, bool>;

#[cfg(test)]
mod test {
    use scope;
    use super::*;
    use std::ptr;
    use std::sync::atomic::Ordering::{Relaxed};
    #[test]
    fn test_cas () {
        let mut val: u64 = 0;
        let eptr = ExclusivePtr::<u64>::new(ptr::null_mut());
        let ll = eptr.load_linked(Relaxed);
        assert_eq!(eptr.load(Relaxed), ptr::null_mut());
        assert_eq!(ll.store_conditional(&mut val, Relaxed).is_none(), true);
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
        assert_eq!(eptr.load(Relaxed), &mut val2 as *mut u64);
        assert_eq!(ll.store_conditional(&mut val, Relaxed).is_some(), true);
        assert_eq!(eptr.load(Relaxed), &mut val2 as *mut u64);
    }

    #[test]
    fn test_cas_fail_xchg () {
        let mut val: u64 = 0;
        let mut val2: u64 = 0;
        let eptr = ExclusivePtr::<u64>::new(ptr::null_mut());
        let ll = eptr.load_linked(Relaxed);
        assert_eq!(eptr.load(Relaxed), ptr::null_mut());
        assert_eq!(eptr.exchange_direct(&mut val2, Relaxed), ptr::null_mut());
        assert_eq!(eptr.load(Relaxed), &mut val2 as *mut u64);
        assert_eq!(ll.store_conditional(&mut val, Relaxed).is_some(), true);
        assert_eq!(eptr.load(Relaxed), &mut val2 as *mut u64);
    }

    #[test]
    fn test_mt_cas() {
        let num_run: usize = 1000000;
        let num_thread: usize = 4;
        let val = ExclusiveUsize::new(0);

        scope(|scope| {
            for _ in 0..num_thread {
                scope.spawn(||{
                    for _ in 0..num_run {
                        let mut ll = val.load_linked(Relaxed);
                        loop {
                            let next = ll.get() + 1;
                            match ll.store_conditional(next, Relaxed) {
                                None => break,
                                Some(nll) => ll = nll,
                            }
                        }
                    }
                });
            }
        });

        assert_eq!(val.load(Relaxed), num_run * num_thread);
    }
}
