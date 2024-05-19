use std::{
    ops::{Deref, DerefMut, Range},
    pin::pin,
    task::{Context, Poll},
};

use futures::{task::noop_waker_ref, Future};

/// stream and index
pub struct Cursor<T> {
    /// stream of items. You must only grow this vector.
    pub buf: Vec<T>,
    pub index: usize,
}

impl<T> Cursor<T> {
    #[inline]
    pub fn remaining(&self) -> &[T] {
        &self.buf[self.index..]
    }
}

#[cfg(debug_assertions)]
pub struct Input<T>(std::cell::RefCell<Cursor<T>>);

#[cfg(not(debug_assertions))]
pub struct Input<T>(std::cell::UnsafeCell<Cursor<T>>);

impl<T> Input<T> {
    #[cfg(debug_assertions)]
    pub fn new(cursor: Cursor<T>) -> Self {
        Self(std::cell::RefCell::new(cursor))
    }
    #[cfg(not(debug_assertions))]
    pub fn new(cursor: Cursor<T>) -> Self {
        Self(std::cell::UnsafeCell::new(cursor))
    }

    /// Don't call .await while holding a borrow of the cursor.
    unsafe fn cursor(&self) -> impl Deref<Target = Cursor<T>> + '_ {
        #[cfg(debug_assertions)]
        {
            self.0.borrow()
        }
        #[cfg(not(debug_assertions))]
        unsafe {
            &*self.0.get()
        }
    }

    /// Don't call .await while holding a borrow of the cursor.
    unsafe fn cursor_mut(&self) -> impl DerefMut<Target = Cursor<T>> + '_ {
        #[cfg(debug_assertions)]
        {
            self.0.borrow_mut()
        }
        #[cfg(not(debug_assertions))]
        unsafe {
            &mut *self.0.get()
        }
    }

    /// Relatively safe way to access the cursor.
    /// Safety: DO NOT use self inside the FnOnce.
    #[inline]
    fn scope_cursor<O>(&self, f: impl FnOnce(&Cursor<T>) -> O) -> O {
        f(unsafe { &self.cursor() })
    }

    /// Relatively safe way to access the cursor.
    /// Safety: DO NOT use self inside the FnOnce.
    #[inline]
    fn scope_cursor_mut<O>(&self, f: impl FnOnce(&mut Cursor<T>) -> O) -> O {
        f(unsafe { &mut self.cursor_mut() })
    }

    pub fn into_inner(self) -> Cursor<T> {
        self.0.into_inner()
    }

    pub fn borrow(&self) -> InputRef<'_, T> {
        InputRef(self)
    }

    fn read(&self) -> impl Future<Output = ()> + '_ {
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
                self.input.scope_cursor(|c| {
                    if c.buf.len() > self.start_len {
                        std::task::Poll::Ready(())
                    } else {
                        std::task::Poll::Pending
                    }
                })
            }
        }

        Read {
            input: self,
            start_len: self.scope_cursor(|c| c.buf.len()),
        }
    }

    fn read_n(&self, at_least: usize) -> impl Future<Output = ()> + '_ {
        struct ReadAtLeast<'a, T> {
            input: &'a Input<T>,
            start_len: usize,
            at_least: usize,
        }

        impl<T> Future for ReadAtLeast<'_, T> {
            type Output = ();

            fn poll(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Self::Output> {
                self.input.scope_cursor(|c| {
                    if c.buf.len() >= self.start_len + self.at_least {
                        std::task::Poll::Ready(())
                    } else {
                        std::task::Poll::Pending
                    }
                })
            }
        }

        ReadAtLeast {
            input: self,
            start_len: self.scope_cursor(|c| c.buf.len()),
            at_least,
        }
    }
}

pub struct InputRef<'a, T>(&'a Input<T>);

impl<'a, T> InputRef<'a, T> {
    pub fn scope_cursor_mut<O>(&mut self, f: impl FnOnce(&mut Cursor<T>) -> O) -> O {
        self.0.scope_cursor_mut(f)
    }

    pub fn scope_cursor<O>(&self, f: impl FnOnce(&Cursor<T>) -> O) -> O {
        self.0.scope_cursor(f)
    }

    pub fn read(&mut self) -> impl Future<Output = ()> + '_ {
        self.0.read()
    }

    pub fn read_n(&mut self, at_least: usize) -> impl Future<Output = ()> + '_ {
        self.0.read_n(at_least)
    }
}

pub async fn tag<'a, T>(input: &mut InputRef<'a, T>, tag: &[T]) -> Result<Range<usize>, ()>
where
    T: PartialEq,
{
    input.read_n(tag.len()).await;

    input.scope_cursor_mut(|cursor| {
        if cursor.remaining().starts_with(tag) {
            let start = cursor.index;
            cursor.index += tag.len();
            Ok(start..cursor.index)
        } else {
            Err(())
        }
    })
}

pub async fn many0<'a, T>(
    input: &mut InputRef<'a, T>,
    mut cond: impl FnMut(&T) -> bool,
) -> Range<usize> {
    let start = input.scope_cursor(|c| c.index);

    loop {
        if let Some(r) = input.scope_cursor_mut(|c| {
            for (i, item) in c.buf[c.index..].iter().enumerate() {
                if !cond(item) {
                    c.index += i;
                    return Some(start..c.index);
                }
            }

            c.index = c.buf.len();
            None
        }) {
            return r;
        }

        input.read().await;
    }
}

pub trait PollNoop: Future + Unpin {
    // better name?
    fn poll_noop(&mut self) -> Poll<<Self as Future>::Output> {
        let mut cx = Context::from_waker(noop_waker_ref());

        pin!(self).poll(&mut cx)
    }
}

impl<T: Future + Unpin> PollNoop for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read() {
        let input = Input::new(Cursor {
            buf: vec![1, 2, 3],
            index: 0,
        });

        let mut read = input.read();

        assert!(read.poll_noop().is_pending());

        input.scope_cursor_mut(|c| {
            c.buf.push(4);
        });

        assert!(read.poll_noop().is_ready());
    }

    #[test]
    fn test_get3() {
        async fn get3<T>(input: &Input<T>) {
            input.read_n(3).await;
        }

        let input = Input::new(Cursor {
            buf: Vec::new(),
            index: 0,
        });
        let mut get3 = pin!(get3(&input));

        assert!(get3.poll_noop().is_pending());

        input.scope_cursor_mut(|c| {
            c.buf.push(1);
        });
        assert!(get3.poll_noop().is_pending());

        input.scope_cursor_mut(|c| {
            c.buf.push(2);
        });
        assert!(get3.poll_noop().is_pending());

        input.scope_cursor_mut(|c| {
            c.buf.push(3);
        });
        assert!(get3.poll_noop().is_ready());
    }

    #[test]
    fn test_many0() {
        let input = Input::new(Cursor {
            buf: Vec::new(),
            index: 0,
        });

        let cond = move |x: &i32| *x % 2 == 0;

        let mut iref = InputRef(&input);

        let mut p = pin!(many0(&mut iref, cond));

        input.scope_cursor_mut(|c| c.buf.push(0));
        assert!(p.poll_noop().is_pending());

        input.scope_cursor_mut(|c| c.buf.push(2));
        assert!(p.poll_noop().is_pending());

        input.scope_cursor_mut(|c| c.buf.push(4));
        assert!(p.poll_noop().is_pending());

        input.scope_cursor_mut(|c| c.buf.push(1));
        assert_eq!(p.poll_noop(), Poll::Ready(0..3));
    }

    #[test]
    fn test_combined() {
        let input = Input::new(Cursor {
            buf: Vec::new(),
            index: 0,
        });

        let mut iref = input.borrow();

        let p = async {
            let alpha0 = many0(&mut iref, |x: &u8| x.is_ascii_alphabetic()).await;
            dbg!(&alpha0);
            let digit = many0(&mut iref, |x: &u8| x.is_ascii_digit()).await;
            dbg!(&digit);
            let alpha2 = many0(&mut iref, |x: &u8| x.is_ascii_alphabetic()).await;

            (alpha0, digit, alpha2)
        };

        let mut p = pin!(p);

        input.scope_cursor_mut(|c| {
            c.buf.extend(b"abc");
        });

        assert!(p.poll_noop().is_pending());

        input.scope_cursor_mut(|c| {
            c.buf.extend(b"123");
        });

        assert!(p.poll_noop().is_pending());
        input.scope_cursor_mut(|c| {
            c.buf.extend(b"def");
        });

        assert!(p.poll_noop().is_pending());
        input.scope_cursor_mut(|c| {
            c.buf.push(b';');
        });

        assert_eq!(p.poll_noop(), Poll::Ready((0..3, 3..6, 6..9)));
    }

    #[test]
    #[should_panic]
    #[cfg(debug_assertions)]
    fn test_bad_borrow() {
        async fn bad(input: &Input<u8>) -> usize {
            let cursor = unsafe { input.cursor() };
            // You must not call read().await while borrowing cursor.
            // Can we ensure that it can't happen?
            input.read().await;
            cursor.remaining().len()
        }

        let input = Input::new(Cursor {
            buf: Vec::new(),
            index: 0,
        });

        let mut p = pin!(bad(&input));

        assert!(p.poll_noop().is_pending());

        unsafe { input.cursor_mut() }.buf.push(1);
    }
}
