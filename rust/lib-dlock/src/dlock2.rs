use std::cmp::Reverse;
use std::fmt::{Binary, Debug};
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use crate::{
    c_binding::{ccsynch::CCCSynch, flatcombining::CFlatCombining},
    dlock2::{cc::CCSynch, fc::FC},
    spin_lock::RawSpinLock,
};
use enum_dispatch::enum_dispatch;
use strum::Display;

use self::{
    cc_ban::CCBan, dsm::DSMSynch, fc_ban::FCBan, fc_pq::UsageNode, fc_sl::FCSL, mutex::DLock2Mutex, spinlock::DLock2Wrapper, uscl::DLock2USCL
};

pub mod cc;
pub mod cc_ban;
pub mod dsm;
pub mod fc;
pub mod fc_ban;
pub mod fc_sl;
pub mod rcl;

pub mod mutex;
pub mod spinlock;
pub mod uscl;
pub mod fc_pq;

pub trait DLock2Delegate<T, I>: Fn(&mut T, I) -> I + Send + Sync {}
impl<T, I, F> DLock2Delegate<T, I> for F where F: Fn(&mut T, I) -> I + Send + Sync {}

// We probably should have a slightly more restrictive bound on the trait
#[enum_dispatch(DLock2Impl<T, I, F>)]
pub unsafe trait DLock2<I>: Send + Sync {
    fn lock(&self, data: I) -> I;

    #[cfg(feature = "combiner_stat")]
    fn get_combine_time(&self) -> Option<u64>;
}

#[enum_dispatch]
#[derive(Debug, Display)]
pub enum DLock2Impl<T, I, F>
where
    T: Send + Sync + 'static,
    I: Send + Sync + Debug + 'static,
    F: DLock2Delegate<T, I> + 'static,
{
    FC(FC<T, I, F>),
    FCBan(FCBan<T, I, F>),
    CC(CCSynch<T, I, F>),
    DSM(DSMSynch<T, I, F>),
    CCBan(CCBan<T, I, F>),
    FC_SL(FCSL<T, I, F, RawSpinLock>),
    FC_PQ_BTree(fc_pq::FCPQ<T, I, BTreeSet<UsageNode<'static, I>>, F, RawSpinLock>),
    FC_PQ_BHeap(fc_pq::FCPQ<T, I, BinaryHeap<Reverse<UsageNode<'static, I>>>, F, RawSpinLock>),
    SpinLock(DLock2Wrapper<T, I, F, RawSpinLock>),
    Mutex(DLock2Mutex<T, I, F>),
    USCL(DLock2USCL<T, I, F>),
    C_FC(CFlatCombining<T, F, I>),
    C_CC(CCCSynch<T, F, I>),
}
