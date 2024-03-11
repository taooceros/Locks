use std::sync::atomic::*;

pub trait AtomicExtension {
    type T;
    fn load_acquire(&self) -> Self::T;
    fn store_release(&self, val: Self::T);
}

macro_rules! impl_atomic_extension {
    ($t:ty, $atomicType: ty) => {
        impl AtomicExtension for $atomicType {
            type T = $t;
            fn load_acquire(&self) -> $t {
                self.load(std::sync::atomic::Ordering::Acquire)
            }
            fn store_release(&self, val: $t) {
                self.store(val, std::sync::atomic::Ordering::Release);
            }
        }
    };
}

impl_atomic_extension!(u8, AtomicU8);
impl_atomic_extension!(u16, AtomicU16);
impl_atomic_extension!(u32, AtomicU32);
impl_atomic_extension!(u64, AtomicU64);
impl_atomic_extension!(usize, AtomicUsize);
impl_atomic_extension!(i8, AtomicI8);
impl_atomic_extension!(i16, AtomicI16);
impl_atomic_extension!(i32, AtomicI32);
impl_atomic_extension!(i64, AtomicI64);
impl_atomic_extension!(isize, AtomicIsize);
impl_atomic_extension!(bool, AtomicBool);

impl<T> AtomicExtension for AtomicPtr<T> {
    type T = *mut T;
    fn load_acquire(&self) -> *mut T {
        self.load(std::sync::atomic::Ordering::Acquire)
    }
    fn store_release(&self, val: *mut T) {
        self.store(val, std::sync::atomic::Ordering::Release);
    }
}
