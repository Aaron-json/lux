use crate::inner::{CachePadded, LuxError, Slot, alloc_slots};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct SpscInner<T> {
    // padded to avoid false sharing.
    head: CachePadded<AtomicUsize>,
    tail: CachePadded<AtomicUsize>,

    // this is not padded since the 'fat pointer' is never changed, only
    // the data behind it.
    buf: Box<[Slot<T>]>,
}

impl<T> SpscInner<T> {
    fn new(cap_pow: u8) -> Self {
        let size: usize = 1 << cap_pow;

        SpscInner {
            buf: alloc_slots(size),
            head: CachePadded(AtomicUsize::new(0)),
            tail: CachePadded(AtomicUsize::new(0)),
        }
    }

    #[inline(always)]
    fn len(&self) -> usize {
        self.buf.len()
    }

    #[inline(always)]
    fn mask(&self, num: usize) -> usize {
        num & (self.buf.len() - 1)
    }
}

struct SpscProducer<T> {
    inner: Arc<SpscInner<T>>,
    // The consumer writes to the head. Doing an atomic load every time would
    // create cache coherence traffic and bouncing between the producer and
    // consumer threads. Since we know that the head only moves forward, we
    // can cache the last seen value and write up to it without loading the
    // actual head.
    head_cache: usize,
}

impl<T> SpscProducer<T> {
    fn try_push(&mut self, val: T) -> Result<(), LuxError<T>> {
        let inner = &self.inner;
        let t = inner.tail.0.load(Ordering::Relaxed);

        // Read the cached head, until it tells us the buffer is full then
        // load the actual value into it.
        if t.wrapping_sub(self.head_cache) == inner.len() {
            self.head_cache = inner.head.0.load(Ordering::Acquire);
            if t.wrapping_sub(self.head_cache) == inner.len() {
                return Err(LuxError::BufferFull(val));
            }
        }

        unsafe {
            (*inner.buf[inner.mask(t)].get()).write(val);
        }

        // publish the data
        inner.tail.0.store(t + 1, Ordering::Release);

        Ok(())
    }
}

struct SpscConsumer<T> {
    inner: Arc<SpscInner<T>>,
    // Cached tail to avoid cache coherence traffic and cache bouncing.
    // For more details, read the producer's type.
    tail_cache: usize,
}

impl<T> SpscConsumer<T> {
    fn try_pop(&mut self) -> Result<T, LuxError<T>> {
        let inner = &self.inner;
        let h = inner.head.0.load(Ordering::Relaxed);

        if self.tail_cache.wrapping_sub(h) == 0 {
            self.tail_cache = inner.tail.0.load(Ordering::Acquire);
            if self.tail_cache.wrapping_sub(h) == 0 {
                return Err(LuxError::BufferEmpty);
            }
        }

        let ret = unsafe { (*inner.buf[inner.mask(h)].get()).assume_init_read() };

        inner.head.0.store(h + 1, Ordering::Release);

        Ok(ret)
    }
}

// Both the Consumer and Producer can be sent from one thread to another
// but only one of each can exist. That is, you can not have shared references
// to any one of them.
unsafe impl<T: Send> Send for SpscConsumer<T> {}
unsafe impl<T: Send> Send for SpscProducer<T> {}

fn new_spsc<T>(cap_pow: u8) -> (SpscConsumer<T>, SpscProducer<T>) {
    let inner = Arc::new(SpscInner::<T>::new(cap_pow));
    let c = SpscConsumer {
        inner: inner.clone(),
        tail_cache: 0,
    };

    let p = SpscProducer {
        inner: inner,
        head_cache: 0,
    };

    (c, p)
}
