//! Fixed-slot, allocation-free async/await experiment.
//!
//! The executor is deliberately tiny: a no-op waker, eager polling, and no
//! I/O. It measures whether `async`/`await` state machines fit a no-alloc
//! server before committing the real reactor to them.

#![cfg_attr(not(test), no_std)]

use core::future::Future;
use core::pin::Pin;
use core::ptr::null;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

unsafe fn waker_clone(_: *const ()) -> RawWaker {
    RawWaker::new(null(), &WAKER_VTABLE)
}
unsafe fn waker_wake(_: *const ()) {}
unsafe fn waker_wake_by_ref(_: *const ()) {}
unsafe fn waker_drop(_: *const ()) {}

static WAKER_VTABLE: RawWakerVTable =
    RawWakerVTable::new(waker_clone, waker_wake, waker_wake_by_ref, waker_drop);

fn noop_waker() -> Waker {
    // SAFETY: the vtable's functions never dereference the null data pointer.
    unsafe { Waker::from_raw(RawWaker::new(null(), &WAKER_VTABLE)) }
}

pub fn block_on<F: Future>(future: F) -> F::Output {
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);
    let mut future = future;
    loop {
        // The future is stack-owned and never moved while pinned.
        let pinned = unsafe { Pin::new_unchecked(&mut future) };
        if let Poll::Ready(value) = pinned.poll(&mut context) {
            return value;
        }
    }
}

pub fn run_slots<const N: usize, F: Future<Output = u32>>(futures: [F; N]) -> [u32; N] {
    let mut futures = futures;
    let mut done = [false; N];
    let mut output = [0u32; N];
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);

    loop {
        let mut all_done = true;
        for index in 0..N {
            if done[index] {
                continue;
            }
            all_done = false;
            let pinned = unsafe { Pin::new_unchecked(&mut futures[index]) };
            if let Poll::Ready(value) = pinned.poll(&mut context) {
                output[index] = value;
                done[index] = true;
            }
        }
        if all_done {
            return output;
        }
    }
}

pub struct Yield {
    remaining: u8,
    value: u32,
}

impl Yield {
    pub const fn new(remaining: u8, value: u32) -> Self {
        Self { remaining, value }
    }
}

impl Future for Yield {
    type Output = u32;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<u32> {
        let this = self.get_mut();
        if this.remaining == 0 {
            Poll::Ready(this.value)
        } else {
            this.remaining -= 1;
            context.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

pub async fn demo() -> u32 {
    let a = Yield::new(2, 1).await;
    let b = Yield::new(1, 2).await;
    a + b
}

pub fn run() -> i32 {
    let results = run_slots([demo(), demo(), demo()]);
    if results == [3, 3, 3] {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::{block_on, demo, run_slots};

    #[test]
    fn block_on_works() {
        assert_eq!(block_on(demo()), 3);
    }

    #[test]
    fn slots_run_to_completion() {
        assert_eq!(run_slots([demo(), demo()]), [3, 3]);
    }

    #[test]
    fn future_size_is_recorded() {
        let future = demo();
        let size = core::mem::size_of_val(&future);
        std::println!("async demo future size: {size} bytes");
        assert!(size <= 256);
    }
}
