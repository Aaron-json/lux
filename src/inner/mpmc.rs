use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use crate::inner::{CachePadded, SequencedSlot, alloc_sequenced_slots};

// Used to implement a Multi-producer Multi-consumer queue. The design
// implements Dmitry vyukov's MPMC Queue ideas.
struct MpmcInner<T> {
    buf: Box<[SequencedSlot<T>]>,

    head: CachePadded<AtomicUsize>,
    tail: CachePadded<AtomicUsize>,
}

impl<T> MpmcInner<T> {
    fn new(cap_pow: u8) -> Self {
        let size: usize = 1 << cap_pow;

        MpmcInner {
            buf: alloc_sequenced_slots(size),
            head: CachePadded(AtomicUsize::new(0)),
            tail: CachePadded(AtomicUsize::new(0)),
        }
    }

    #[inline(always)]
    fn mask(&self, num: usize) -> usize {
        num & (self.buf.len() - 1)
    }

    #[inline(always)]
    fn len(&self) -> usize {
        self.buf.len()
    }
}

impl<T> Drop for MpmcInner<T> {
    fn drop(&mut self) {
        let h = self.head.0.load(Ordering::Relaxed);
        let t = self.tail.0.load(Ordering::Relaxed);

        for i in 0..t.wrapping_sub(h) {
            let idx = h.wrapping_add(i);

            unsafe {
                (*self.buf[self.mask(idx)].data.get()).assume_init_drop();
            }
        }
    }
}

struct MpmcProducer<T> {
    inner: Arc<MpmcInner<T>>,
}

impl<T> MpmcProducer<T> {
    fn try_push(&self, val: T) -> Result<(), T> {
        let inner = &self.inner;

        let mut idx = inner.tail.0.load(Ordering::Relaxed);

        loop {
            let slot = &inner.buf[inner.mask(idx)];
            let seq = slot.seq.load(Ordering::Acquire);

            let diff = seq.wrapping_sub(idx) as isize;
            if diff == 0 {
                match inner.tail.0.compare_exchange_weak(
                    idx,
                    idx + 1,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    // we sucessfully claimed the slot
                    Ok(_) => unsafe {
                        (*slot.data.get()).write(val);
                        // update to t + 1 to signal consumers this slot is ready
                        // for reading
                        slot.seq.store(idx + 1, Ordering::Release);
                        return Ok(());
                    },
                    Err(new_idx) => {
                        // Another producer won the CAS and updated it before us.
                        // Advance to the new tail and try again
                        idx = new_idx;
                        // we do not use hint::spin_loop here since we just
                        // got fresh data which is likely to win the next CAS
                        // unless we are under very heavy contention.
                    }
                }
            } else if diff < 0 {
                // this slot contains data from the previous lap's write. has not been
                // read yet
                return Err(val);
            } else if diff > 0 {
                // another producer claimed the slot and updated tail and
                // our copy is stale so we reload it.

                // TODO: a more robust solution pending. would probably also include
                // things like waiting strategies and park/yield and some state in
                // the case of multiple threads
                std::hint::spin_loop();
                idx = inner.tail.0.load(Ordering::Relaxed);
            }
        }
    }
}

struct MpmcConsumer<T> {
    inner: Arc<MpmcInner<T>>,
}

impl<T> MpmcConsumer<T> {
    fn try_pop(&self) -> Option<T> {
        let inner = &self.inner;

        let mut idx = inner.head.0.load(Ordering::Relaxed);

        loop {
            let slot = &inner.buf[inner.mask(idx)];

            let seq = slot.seq.load(Ordering::Acquire);

            // consumers look for sequence number `position + 1` (set by the
            // producer) to read this slot
            // this may also underflow
            let diff = seq.wrapping_sub(idx + 1) as isize;

            if diff == 0 {
                match inner.head.0.compare_exchange_weak(
                    idx,
                    idx + 1,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        let data = unsafe { (*slot.data.get()).assume_init_read() };

                        slot.seq.store(idx + inner.len(), Ordering::Release);

                        return Some(data);
                    }
                    Err(new_idx) => {
                        // another reader won the CAS
                        idx = new_idx;
                    }
                }
            } else if diff < 0 {
                // producer has not written here yet
                return None;
            } else {
                // another consumer has read this position and advanced
                // the sequence number but the head is stale
                std::hint::spin_loop();
                idx = inner.head.0.load(Ordering::Relaxed);
            }
        }
    }
}

// Implement clone so users do not have to wrap in another Arc, increasing
// indirections.
impl<T> Clone for MpmcProducer<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}
impl<T> Clone for MpmcConsumer<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

// Both the Consumer and Producer can be sent and shared from one thread to
// another
unsafe impl<T: Send> Send for MpmcProducer<T> {}
unsafe impl<T: Send> Send for MpmcConsumer<T> {}
unsafe impl<T: Send> Sync for MpmcProducer<T> {}
unsafe impl<T: Send> Sync for MpmcConsumer<T> {}
