use std::sync::atomic::AtomicUsize;

use crate::inner::{CachePadded, SequencedSlot};

struct MpmcInner<T> {
    data: Box<[SequencedSlot<T>]>,

    head: CachePadded<AtomicUsize>,
    tail: CachePadded<AtomicUsize>,
}
