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
pub struct RequestHandle<T = ()> {
    id: usize,
    data: T,
}
impl<T> RequestHandle<T> {
    pub fn unique(data: T) -> Self {
        RequestHandle {
            id: HANDLE_COUNT.fetch_add(1, Ordering::Relaxed),
            data,
        }
    }
    pub fn into_data(self) -> T {
        self.data
    }
}
