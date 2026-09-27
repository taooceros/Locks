#[cfg(feature = "combiner_stat")]
use std::arch::x86_64::__rdtscp;

use std::{
    cell::SyncUnsafeCell,
    mem::MaybeUninit,
    ptr::{null_mut, NonNull},
    sync::atomic::{AtomicPtr, AtomicU32, Ordering::*},
};

use crossbeam::utils::{Backoff, CachePadded};
use lock_api::RawMutex;
use thread_local::ThreadLocal;

#[cfg(feature = "spin_park")]
use crate::dlock2::park::{self, Parked, SpinBudget};
use crate::dlock2::{DLock2, DLock2Delegate};

use super::node::Node;

const CLEAN_UP_AGE: u32 = 500;

#[derive(Debug)]
pub struct FC<T, I, F, L>
where
    T: Send + Sync,
    I: Send,
    F: Fn(&mut T, I) -> I,
    L: RawMutex,
{
    pass: AtomicU32,
    combiner_lock: CachePadded<L>,
    delegate: F,
    data: SyncUnsafeCell<T>,
    head: AtomicPtr<Node<I>>,
    local_node: ThreadLocal<SyncUnsafeCell<Node<I>>>,
    /// Waiters that published `PARKED` and are not yet resolved
    /// (`dlock2/park.rs`, handshake 2).
    #[cfg(feature = "spin_park")]
    parked: CachePadded<AtomicU32>,
}

impl<T, I, F, L> FC<T, I, F, L>
where
    T: Send + Sync,
    I: Send,
    F: DLock2Delegate<T, I>,
    L: RawMutex,
{
    pub fn new(data: T, delegate: F) -> Self {
        Self {
            pass: AtomicU32::new(0),
            combiner_lock: CachePadded::new(L::INIT),
            delegate,
            data: SyncUnsafeCell::new(data),
            head: AtomicPtr::new(std::ptr::null_mut()),
            local_node: ThreadLocal::new(),
            #[cfg(feature = "spin_park")]
            parked: CachePadded::new(AtomicU32::new(0)),
        }
    }

    /// Links an unlinked node at the head. Callers have already set
    /// `active`; the owner and (under `spin_park`) the cleaner use it.
    fn link_node(&self, node: &Node<I>) {
        // The list stores addresses, not exclusive borrows. Each ThreadLocal
        // allocation stays fixed until all calls finish and FC is dropped.
        let node_ptr = node as *const Node<I> as *mut Node<I>;
        let mut head = self.head.load(Acquire);
        loop {
            node.next.store(head, Relaxed);
            match self
                .head
                .compare_exchange_weak(head, node_ptr, Release, Acquire)
            {
                Ok(_) => {
                    break;
                }
                Err(x) => head = x,
            }
        }
    }

    #[cfg(not(feature = "spin_park"))]
    fn push_if_unactive(&self, node: &Node<I>) {
        if node.active.load(Acquire) {
            return;
        }
        node.active.store(true, Release);
        self.link_node(node);
    }

    /// Handshake 3 (`dlock2/park.rs`): the SeqCst load follows the owner's
    /// SeqCst `complete = false`, so this and the cleaner's
    /// `active = false; load complete` cannot both miss; the CAS lets
    /// exactly one side link the node.
    #[cfg(feature = "spin_park")]
    fn push_if_unactive(&self, node: &Node<I>) {
        if node.active.load(SeqCst) {
            return;
        }
        if node
            .active
            .compare_exchange(false, true, SeqCst, SeqCst)
            .is_ok()
        {
            self.link_node(node);
        }
    }

    fn combine(&self) {
        let mut current_ptr = NonNull::new(self.head.load(Acquire));

        let pass = self.pass.fetch_add(1, Relaxed);

        #[cfg(feature = "combiner_stat")]
        let mut aux: u32 = 0;
        #[cfg(feature = "combiner_stat")]
        let begin: u64;

        #[cfg(feature = "combiner_stat")]
        unsafe {
            begin = __rdtscp(&mut aux);
        }

        // Handshake 1, deferred: the park check for the previously served
        // node runs after the next delegate (or after the loop), once its
        // result stores have drained, so the fence does not stall the pass.
        #[cfg(feature = "spin_park")]
        let mut unwoken: Option<&Node<I>> = None;

        while let Some(current_nonnull) = current_ptr {
            // SAFETY: ThreadLocal nodes remain at stable addresses until this lock
            // is dropped, after all lock() calls have returned. Only atomics and
            // interior-mutable fields are accessed while a node is published.
            let current = unsafe { current_nonnull.as_ref() };

            if current.active.load(Acquire) && !current.complete.load(Acquire) {
                // SAFETY: acquire of complete=false observes the requester's
                // release publication. Combiner exclusion gives one reader/writer
                // of the payload and age; complete=true hands the result back.
                // No requester publishes another payload before taking this one.
                unsafe {
                    (*current.age.get()) = pass;
                    let result = (self.delegate)(
                        self.data.get().as_mut().unwrap_unchecked(),
                        current.data.get().read().assume_init(),
                    );
                    #[cfg(feature = "spin_park")]
                    if let Some(previous) = unwoken.take() {
                        previous.park.wake_if_parked(&self.parked);
                    }
                    current.data.get().write(MaybeUninit::new(result));
                }

                current.complete.store(true, Release);
                #[cfg(feature = "spin_park")]
                {
                    unwoken = Some(current);
                }
            }

            current_ptr = NonNull::new(current.next.load(Acquire));
        }

        #[cfg(feature = "spin_park")]
        if let Some(previous) = unwoken {
            previous.park.wake_if_parked(&self.parked);
        }

        #[cfg(feature = "combiner_stat")]
        unsafe {
            let end = __rdtscp(&mut aux);

            // SAFETY: only this thread writes/reads its ThreadLocal statistic;
            // other threads may hold &Node, hence the separate interior cell.
            // A thread that combines through `release_combiner` before ever
            // issuing a request has no node yet; its pass is not attributed.
            if let Some(node) = self.local_node.get() {
                *(*node.get()).combiner_time_stat.get() += end - begin;
            }
        }
    }

    /// One combining pass plus periodic cleanup. Caller holds `combiner_lock`.
    fn combine_locked(&self) {
        self.combine();
        unsafe {
            let pass = self.pass.load(Relaxed);

            if pass.is_multiple_of(CLEAN_UP_AGE) {
                self.clean_unactive_node(&self.head, pass);
            }
        }
    }

    /// Releases `combiner_lock`. Every release of the combiner lock, including
    /// one by a holder that did not combine, must go through here: under
    /// `spin_park` the release is followed by the fenced `parked` check of
    /// handshake 2, re-acquiring and combining while some waiter is parked
    /// (or until another holder takes over that obligation).
    fn release_combiner(&self) {
        unsafe { self.combiner_lock.unlock() };
        #[cfg(feature = "spin_park")]
        while park::parked_after_unlock(&self.parked) && self.combiner_lock.try_lock() {
            self.combine_locked();
            unsafe { self.combiner_lock.unlock() };
        }
    }

    // SAFETY: callers hold the combiner mutex. ThreadLocal nodes remain
    // allocated until quiescent drop; only the combiner touches age and list
    // links during traversal, clearing active only after unlinking.
    unsafe fn clean_unactive_node(&self, head: &AtomicPtr<Node<I>>, pass: u32) {
        let previous_ptr = NonNull::new(head.load(Acquire)).unwrap();

        let mut previous_nonnull = previous_ptr;

        let mut current_ptr = NonNull::new(previous_nonnull.as_ref().next.load(Acquire));

        while let Some(current_nonnull) = current_ptr {
            let current = current_nonnull.as_ref();
            let previous = previous_nonnull.as_ref();

            // assert!(current.active.load(Acquire));

            if pass - (*current.age.get()) > CLEAN_UP_AGE {
                previous.next.store(current.next.load(Acquire), Release);
                current.next.store(null_mut(), Release);
                self.retire_unlinked(current);
                current_ptr = NonNull::new(previous.next.load(Acquire));
                continue;
            }

            previous_nonnull = current_nonnull;
            current_ptr = NonNull::new(current.next.load(Acquire));
        }
    }

    #[cfg(not(feature = "spin_park"))]
    fn retire_unlinked(&self, node: &Node<I>) {
        node.active.store(false, Release);
    }

    /// Handshake 3 (`dlock2/park.rs`): `node` is unlinked. If its owner
    /// published a request meanwhile and could still miss `active = false`,
    /// win the CAS and re-link it here so a parked owner is never left with
    /// an unenrolled request. Re-linking at the head does not disturb the
    /// cleanup traversal, which never revisits nodes before its start.
    ///
    /// The window between the unlink and `active = false` is covered: an
    /// owner that reads `active = true` there did so before this SeqCst
    /// store in the total order, so its SeqCst `complete = false` (which it
    /// stored first) precedes the SeqCst load below, which then sees the
    /// pending request and re-links while the combiner lock is still held.
    #[cfg(feature = "spin_park")]
    fn retire_unlinked(&self, node: &Node<I>) {
        node.active.store(false, SeqCst);
        if !node.complete.load(SeqCst)
            && node
                .active
                .compare_exchange(false, true, SeqCst, SeqCst)
                .is_ok()
        {
            self.link_node(node);
        }
    }

    /// Test hook: hold the combiner lock idle (no combining) while `held`
    /// runs, then release it the way a non-combining holder must, e.g. the
    /// FC-PQ fast path.
    #[cfg(test)]
    pub(crate) fn hold_combiner_idle(&self, held: impl FnOnce()) {
        self.combiner_lock.lock();
        held();
        self.release_combiner();
    }
}

unsafe impl<'a, T, I, F, L> DLock2<I> for FC<T, I, F, L>
where
    T: Send + Sync,
    I: Send,
    F: DLock2Delegate<T, I>,
    L: RawMutex + Send + Sync,
{
    fn lock(&self, data: I) -> I {
        let node = self.local_node.get_or(|| SyncUnsafeCell::new(Node::new()));

        // SAFETY: the ThreadLocal allocation is stable until quiescent FC drop.
        // Published combiners can retain &Node across calls, so never make &mut
        // Node or replace its cell. This owner alone starts a new request after
        // taking the preceding result; release below publishes its input.
        let node = unsafe { &*node.get() };
        unsafe { node.data.get().write(MaybeUninit::new(data)) };
        #[cfg(not(feature = "spin_park"))]
        node.complete.store(false, Release);
        // Handshake 3: SeqCst pairs with the cleaner's SeqCst `active = false`.
        #[cfg(feature = "spin_park")]
        node.complete.store(false, SeqCst);
        #[cfg(feature = "spin_park")]
        let mut budget = SpinBudget::default();

        'outer: loop {
            self.push_if_unactive(node);

            if self.combiner_lock.try_lock() {
                self.combine_locked();
                self.release_combiner();

                if node.complete.load(Acquire) {
                    break 'outer;
                }
            } else {
                // Spin budget exhausted: the enrollment check and the failed
                // try_lock above are the last actions before the park attempt,
                // which re-checks completion and enrollment, tries the lock
                // once more, and only then sleeps (`dlock2/park.rs`).
                #[cfg(feature = "spin_park")]
                if budget.exhausted() {
                    match node.park.park_or_lock(
                        &self.parked,
                        &node.complete,
                        &node.active,
                        &*self.combiner_lock,
                    ) {
                        Parked::Complete => break 'outer,
                        Parked::Combiner => {
                            self.combine_locked();
                            self.release_combiner();
                            if node.complete.load(Acquire) {
                                break 'outer;
                            }
                        }
                        Parked::Retry => {}
                    }
                    continue 'outer;
                }

                let backoff = Backoff::new();
                let mut count: u32 = 8;
                loop {
                    if node.complete.load(Acquire) {
                        break 'outer;
                    }
                    backoff.spin();
                    count = count.wrapping_sub(1);
                    if count == 0 {
                        continue 'outer;
                    }
                }
            }
        }

        // SAFETY: acquire of complete=true transfers the initialized result
        // from the combiner; this owner reads it exactly once before republishing.
        unsafe { node.data.get().read().assume_init() }
    }

    #[cfg(feature = "combiner_stat")]
    fn get_combine_time(&self) -> Option<u64> {
        // SAFETY: this thread alone accesses its ThreadLocal statistic.
        unsafe {
            self.local_node
                .get()
                .map(|x| *(*x.get()).combiner_time_stat.get())
        }
    }
}
