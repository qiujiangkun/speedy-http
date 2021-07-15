mod client;
mod pool;

pub use client::*;
pub use pool::*;

use std::sync::atomic::Ordering;

#[macro_export]
macro_rules! ensure {
    ($e: expr, $err: expr) => {
        $e.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, $err))?
    };
}
static HANDLE_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[derive(Copy, Clone, Debug)]
pub struct RequestHandle(usize);
impl RequestHandle {
    pub fn unique() -> Self {
        RequestHandle(HANDLE_COUNT.fetch_add(1, Ordering::Relaxed))
    }
}
