use std::{
    cell::RefCell,
    ops::Deref,
    pin::pin,
    task::{Context, Poll},
};

use futures::{task::noop_waker_ref, Future};

pub struct Cursor<T> {
    pub stream: Vec<T>,
    pub index: usize,
}

impl<T> Cursor<T> {
    pub fn remaining_len(&self) -> usize {
        self.stream.len() - self.index
    }
}

pub struct Input<T>(pub RefCell<Cursor<T>>);

impl<T> Input<T> {
    pub fn cursor(&self) -> impl Deref<Target = Cursor<T>> + '_ {
        self.0.borrow()
    }

    pub fn read(&self) -> impl Future<Output = ()> + '_ {
        struct Read<'a, T> {
            input: &'a Input<T>,
            start_len: usize,
        }

        impl<T> Future for Read<'_, T> {
            type Output = ();

            fn poll(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Self::Output> {
                let borrow = self.input.0.borrow();

                if borrow.stream.len() > self.start_len {
                    std::task::Poll::Ready(())
                } else {
                    std::task::Poll::Pending
                }
            }
        }

        Read {
            input: self,
            start_len: self.0.borrow().stream.len(),
        }
    }
}

pub trait PollNoop: Future + Unpin {
    fn poll_noop(&mut self) -> Poll<<Self as Future>::Output> {
        let mut cx = Context::from_waker(noop_waker_ref());

        pin!(self).poll(&mut cx)
    }
}

impl<T: Future + Unpin> PollNoop for T {}

async fn get3<T>(input: &Input<T>) {
    while input.cursor().remaining_len() < 3 {
        input.read().await;
    }
}

#[cfg(test)]
mod tests {
    use futures::{Future, FutureExt};

    use super::*;
    use std::cell::RefCell;

    #[test]
    fn test_read() {
        let input = Input(RefCell::new(Cursor {
            stream: vec![1, 2, 3],
            index: 0,
        }));

        let mut read = input.read();

        assert!(read.poll_noop().is_pending());

        input.0.borrow_mut().stream.push(4);

        assert!(read.poll_noop().is_ready());
    }

    #[test]
    fn test_get3() {
        let input = Input(RefCell::new(Cursor {
            stream: Vec::new(),
            index: 0,
        }));

        let mut get3 = get3(&input).boxed_local();

        let mut cx = Context::from_waker(noop_waker_ref());
        assert!(get3.poll_unpin(&mut cx).is_pending());

        input.0.borrow_mut().stream.push(1);
        assert!(get3.poll_unpin(&mut cx).is_pending());

        input.0.borrow_mut().stream.push(2);
        assert!(get3.poll_unpin(&mut cx).is_pending());

        input.0.borrow_mut().stream.push(3);
        assert!(get3.poll_unpin(&mut cx).is_ready());
    }

    #[test]
    fn test() {
        struct F<'a> {
            v: &'a RefCell<Vec<u8>>,
        }

        impl Future for F<'_> {
            type Output = ();

            fn poll(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Self::Output> {
                if self.v.borrow().len() == 0 {
                    std::task::Poll::Pending
                } else {
                    std::task::Poll::Ready(())
                }
            }
        }

        let v = RefCell::new(Vec::new());

        let mut f = F { v: &v };

        let mut cx = std::task::Context::from_waker(futures::task::noop_waker_ref());

        assert!(f.poll_unpin(&mut cx).is_pending());

        f.v.borrow_mut().push(1);

        assert!(f.poll_unpin(&mut cx).is_ready());
    }
}
