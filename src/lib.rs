use std::{
    cell::RefCell,
    ops::{Deref, DerefMut, Range},
    pin::pin,
    task::{Context, Poll},
};

use futures::{task::noop_waker_ref, Future};

/// stream and index
pub struct Cursor<T> {
    /// stream of items. You must only grow this vector.
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

    pub fn cursor_mut(&self) -> impl DerefMut<Target = Cursor<T>> + '_ {
        self.0.borrow_mut()
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

    pub fn peek_copied(&self) -> impl Future<Output = T> + '_
    where
        T: Copy,
    {
        struct PeekCopied<'a, T> {
            input: &'a Input<T>,
        }

        impl<T> Future for PeekCopied<'_, T>
        where
            T: Copy,
        {
            type Output = T;

            fn poll(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Self::Output> {
                let borrow = self.input.0.borrow();

                if borrow.index < borrow.stream.len() {
                    std::task::Poll::Ready(borrow.stream[borrow.index])
                } else {
                    std::task::Poll::Pending
                }
            }
        }

        PeekCopied { input: self }
    }
}

pub async fn many0<T>(input: &Input<T>, mut cond: impl FnMut(&T) -> bool) -> Range<usize> {
    let start = input.cursor().index;

    loop {
        while input.cursor().remaining_len() == 0 {
            input.read().await;
        }

        // TODO: this can be more efficient
        let mut cursor = input.cursor_mut();
        if !cond(&cursor.stream[cursor.index]) {
            return start..cursor.index;
        }

        cursor.index += 1;
    }
}

pub trait PollNoop: Future + Unpin {
    fn poll_noop(&mut self) -> Poll<<Self as Future>::Output> {
        let mut cx = Context::from_waker(noop_waker_ref());

        pin!(self).poll(&mut cx)
    }
}

impl<T: Future + Unpin> PollNoop for T {}

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
        async fn get3<T>(input: &Input<T>) {
            while input.cursor().remaining_len() < 3 {
                input.read().await;
            }
        }

        let input = Input(RefCell::new(Cursor {
            stream: Vec::new(),
            index: 0,
        }));

        let mut get3 = get3(&input).boxed_local();

        assert!(get3.poll_noop().is_pending());

        input.0.borrow_mut().stream.push(1);
        assert!(get3.poll_noop().is_pending());

        input.0.borrow_mut().stream.push(2);
        assert!(get3.poll_noop().is_pending());

        input.0.borrow_mut().stream.push(3);
        assert!(get3.poll_noop().is_ready());
    }

    #[test]
    fn test_many0() {
        let input = Input(RefCell::new(Cursor {
            stream: Vec::new(),
            index: 0,
        }));

        let cond = move |x: &i32| *x % 2 == 0;

        let mut p = many0(&input, cond).boxed_local();

        input.0.borrow_mut().stream.push(0);
        assert!(p.poll_noop().is_pending());

        input.0.borrow_mut().stream.push(2);
        assert!(p.poll_noop().is_pending());

        input.0.borrow_mut().stream.push(4);
        assert!(p.poll_noop().is_pending());

        input.0.borrow_mut().stream.push(1);
        assert_eq!(p.poll_noop(), Poll::Ready(0..3));
    }

    #[test]
    fn test_combined() {
        let input = Input(RefCell::new(Cursor {
            stream: Vec::new(),
            index: 0,
        }));

        let p = async {
            let alpha0 = many0(&input, |x: &u8| x.is_ascii_alphabetic()).await;
            dbg!(&alpha0);
            let digit = many0(&input, |x: &u8| x.is_ascii_digit()).await;
            dbg!(&digit);
            let alpha2 = many0(&input, |x: &u8| x.is_ascii_alphabetic()).await;

            (alpha0, digit, alpha2)
        };

        let mut p = p.boxed_local();

        input.0.borrow_mut().stream.push(b'a');
        input.0.borrow_mut().stream.push(b'b');
        input.0.borrow_mut().stream.push(b'c');

        assert!(p.poll_noop().is_pending());

        input.0.borrow_mut().stream.push(b'1');
        input.0.borrow_mut().stream.push(b'2');
        input.0.borrow_mut().stream.push(b'3');

        assert!(p.poll_noop().is_pending());
        input.0.borrow_mut().stream.push(b'a');
        input.0.borrow_mut().stream.push(b'b');
        input.0.borrow_mut().stream.push(b'c');

        assert!(p.poll_noop().is_pending());
        input.0.borrow_mut().stream.push(b';');

        assert_eq!(p.poll_noop(), Poll::Ready((0..3, 3..6, 6..9)));
    }

    #[test]
    #[should_panic]
    fn test_bad_borrow() {
        async fn bad(input: &Input<u8>) -> usize {
            let cursor = input.cursor();
            // You must not call read().await while borrowing cursor.
            // Can we ensure that it can't happen?
            input.read().await;
            cursor.remaining_len()
        }

        let input = Input(RefCell::new(Cursor {
            stream: Vec::new(),
            index: 0,
        }));

        let mut p = bad(&input).boxed_local();

        assert!(p.poll_noop().is_pending());

        input.0.borrow_mut().stream.push(1);
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
