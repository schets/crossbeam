
#[cfg(target_arch = "x86_64")]
mod exclusive_target {
    mod x86_64;
    pub use self::x86_64::{ExclusivePtr, ExclusiveUsize, ExclusiveIsize, ExclusiveBool};
    pub use self::x86_64::{LinkedPtr, LinkedUsize, LinkedIsize, LinkedBool};
    pub const IS_LOCK_FREE: bool = true;
}

#[cfg(not(target_arch = "x86_64"))]
mod exclusive_target {
    pub const IS_LOCK_FREE: bool = false;
}

pub use self::exclusive_target::{ExclusivePtr, ExclusiveUsize, ExclusiveIsize, ExclusiveBool};
pub use self::exclusive_target::{LinkedPtr, LinkedUsize, LinkedIsize, LinkedBool};

#[inline(always)]
pub fn is_lock_free() -> bool {
    self::exclusive_target::IS_LOCK_FREE
}
