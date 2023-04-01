use crate::guard::Guard;

pub trait DLock<T> {
    fn lock<'b>(&self, f: &mut (dyn FnMut(&mut Guard<T>) + 'b));
}
