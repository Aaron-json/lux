use std::{cell::UnsafeCell, mem::MaybeUninit, sync::atomic::AtomicUsize};

pub mod mpmc;
pub mod spsc;

/// Cache padding that enforces 64 byte alignment.
#[repr(align(64))]
pub struct CachePadded<T>(T);

type Slot<T> = UnsafeCell<MaybeUninit<T>>;

struct SequencedSlot<T> {
    seq: AtomicUsize,
    data: UnsafeCell<MaybeUninit<T>>,
}

/// Allocate `n` uninitialized slots.
fn alloc_slots<T>(size: usize) -> Box<[Slot<T>]> {
    let mut data = Vec::with_capacity(size);
    unsafe {
        // We tell the Vec that it is full to avoid initializing the whole
        // buffer since we will manually handle reads and writes.
        // This is only safe since we use UnsafeCell and MaybeUninit which do
        // not need to be dropped automatically.
        data.set_len(size);
    }

    data.into_boxed_slice()
}

fn alloc_sequenced_slots<T>(size: usize) -> Box<[SequencedSlot<T>]> {
    let mut data = Vec::<SequencedSlot<T>>::with_capacity(size);
    unsafe {
        // It is fine if data contains garbage here, read alloc_slots for
        // more details
        data.set_len(size);
    }

    // Initially, a slot's sequence is its index.
    for i in 0..size {
        data[i].seq = AtomicUsize::new(i);
    }

    data.into_boxed_slice()
}

#[derive(Debug)]
enum LuxError<T> {
    BufferEmpty,
    // When we cannot push, we return the value to avoid dropping it.
    BufferFull(T),
}
