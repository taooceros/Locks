pub mod lock;
mod node;

pub type FCBan<'a, T, F, L> = lock::FCBan<'a, T, F, L>;
