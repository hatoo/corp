use std::{
    ops::{Deref, DerefMut, Range},
    pin::pin,
    task::{Context, Poll},
};

use futures::{task::noop_waker_ref, Future};
use pin_project_lite::pin_project;

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
#[repr(transparent)]
pub struct Input<T>(std::cell::RefCell<Cursor<T>>);

#[cfg(not(debug_assertions))]
#[repr(transparent)]
pub struct Input<T>(std::cell::UnsafeCell<Cursor<T>>);

impl<T> Input<T> {
    pub fn new(cursor: Cursor<T>) -> Self {
        #[cfg(debug_assertions)]
        {
            Self(std::cell::RefCell::new(cursor))
        }
        #[cfg(not(debug_assertions))]
        {
            Self(std::cell::UnsafeCell::new(cursor))
        }
    }

    pub fn cursor(&self) -> impl Deref<Target = Cursor<T>> + '_ {
        #[cfg(debug_assertions)]
        {
            self.0.borrow()
        }
        #[cfg(not(debug_assertions))]
        unsafe {
            &*self.0.get()
        }
    }
    pub fn cursor_mut(&mut self) -> impl DerefMut<Target = Cursor<T>> + '_ {
        #[cfg(debug_assertions)]
        {
            self.0.borrow_mut()
        }
        #[cfg(not(debug_assertions))]
        unsafe {
            &mut *self.0.get()
        }
    }

    /// Don't call .await while holding a borrow of the cursor.
    pub unsafe fn cursor_mut_unsafe(&self) -> impl DerefMut<Target = Cursor<T>> + '_ {
        #[cfg(debug_assertions)]
        {
            self.0.borrow_mut()
        }
        #[cfg(not(debug_assertions))]
        unsafe {
            &mut *self.0.get()
        }
    }

    pub fn into_inner(self) -> Cursor<T> {
        self.0.into_inner()
    }

    #[inline]
    fn read(&self) -> impl Future<Output = ()> + '_ {
        self.read_n(1)
    }

    #[inline]
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
                let c = self.input.cursor();
                if c.buf.len() >= self.start_len + self.at_least {
                    std::task::Poll::Ready(())
                } else {
                    std::task::Poll::Pending
                }
            }
        }

        ReadAtLeast {
            input: self,
            start_len: self.cursor().buf.len(),
            at_least,
        }
    }
}

#[repr(transparent)]
pub struct InputRef<'a, T>(&'a Input<T>);

impl<'a, T> InputRef<'a, T> {
    /// This crate is safe if you can't bring arguments to outside. I believe it is true.
    pub fn scope_cursor_mut<O>(&mut self, jail: impl FnOnce(&mut Cursor<T>) -> O) -> O {
        let mut cursor = unsafe { self.0.cursor_mut_unsafe() };
        jail(&mut cursor)
    }

    /// This crate is safe if you can't bring arguments to outside. I believe it is true.
    pub fn scope_cursor<O>(&self, jail: impl FnOnce(&Cursor<T>) -> O) -> O {
        let cursor = self.0.cursor();
        jail(&cursor)
    }

    pub fn read(&mut self) -> impl Future<Output = ()> + '_ {
        self.0.read()
    }

    pub fn read_n(&mut self, at_least: usize) -> impl Future<Output = ()> + '_ {
        self.0.read_n(at_least)
    }
}

pub trait Parser<'a, T, O, F>: FnOnce(InputRef<'a, T>) -> F
where
    T: 'a,
    F: Future<Output = O> + 'a,
{
}

impl<'a, T, O, F, P> Parser<'a, T, O, F> for P
where
    T: 'a,
    P: FnOnce(InputRef<'a, T>) -> F,
    F: Future<Output = O> + 'a,
{
}

pin_project! {
    pub struct Parsing<'a, T, F, O> {
        input: &'a mut Input<T>,
        result: Option<O>,
        #[pin]
        future: F,
    }
}

impl<'a, T, O, F> Parsing<'a, T, F, O>
where
    F: Future<Output = O> + Unpin + 'a,
{
    pub fn new<P: Parser<'a, T, O, F>>(input: &'a mut Input<T>, parser: P) -> Self {
        // Dupe mutable ref
        Self {
            future: parser(InputRef(unsafe { std::mem::transmute(&mut *input) })),
            result: None,
            input,
        }
    }

    pub fn poll(&mut self) -> bool {
        let mut cx = Context::from_waker(noop_waker_ref());
        match pin!(&mut self.future).poll(&mut cx) {
            Poll::Ready(result) => {
                self.result = Some(result);
                true
            }
            Poll::Pending => false,
        }
    }
}

impl<'a, T, F, O> Parsing<'a, T, F, O> {
    pub fn cursor(&self) -> impl Deref<Target = Cursor<T>> + '_ {
        self.input.cursor()
    }

    pub fn cursor_mut(&mut self) -> impl DerefMut<Target = Cursor<T>> + '_ {
        self.input.cursor_mut()
    }

    pub fn into_result(self) -> Option<O> {
        self.result
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

#[cfg(test)]
mod tests {
    use futures::FutureExt;

    use super::*;

    /*
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
    */

    #[test]
    fn test_parsing() {
        let mut input = Input::new(Cursor {
            buf: Vec::new(),
            index: 0,
        });

        let mut parsing = Parsing::new(&mut input, move |mut iref: InputRef<u8>| {
            async move {
                let alpha0 = many0(&mut iref, |x: &u8| x.is_ascii_alphabetic()).await;
                dbg!(&alpha0);
                let digit = many0(&mut iref, |x: &u8| x.is_ascii_digit()).await;
                dbg!(&digit);
                let alpha2 = many0(&mut iref, |x: &u8| x.is_ascii_alphabetic()).await;

                (alpha0, digit, alpha2)
            }
            .boxed_local()
        });

        parsing.cursor_mut().buf.extend(b"abc");
        assert!(!parsing.poll());
        parsing.cursor_mut().buf.extend(b"123");
        assert!(!parsing.poll());
        parsing.cursor_mut().buf.extend(b"abc");
        assert!(!parsing.poll());
        parsing.cursor_mut().buf.extend(b";");
        assert!(parsing.poll());
        assert_eq!(parsing.into_result(), Some((0..3, 3..6, 6..9)));
    }

    /*
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic]
    fn test_bad2() {
        let mut input = Input::new(Cursor {
            buf: Vec::new(),
            index: 0,
        });

        let mut p = Parsing::new(&mut input, move |iref: InputRef<u8>| {
            async move { iref }.boxed_local()
        });

        assert!(p.poll());

        let _r = p.into_result().unwrap();
        // _r must dropped here
        input.scope_cursor_mut(move |c| {
            c.buf.push(1);
        });
    }
    */
}
