/// ```compile_fail
/// use futures::FutureExt;
/// use stap::{Input, Cursor, Parsing};
///
/// let mut input = Input::new(Cursor {
///     buf: Vec::<u8>::new(),
///     index: 0,
/// });
///
/// let mut leak = None;
///
/// let mut parsing = Parsing::new(&mut input, move |mut iref| async {
///     leak = Some(iref);
/// }.boxed_local());
///
/// ```
use std::{
    ops::{Deref, DerefMut, Range},
    pin::pin,
    task::{Context, Poll},
};

use futures::{task::noop_waker_ref, Future};
use pin_project_lite::pin_project;

/// buf and index
pub struct Cursor<T> {
    /// Sequence of items, you may append items to this when you want.
    /// I think reducing the items of `buf` isn't introduce an unsoundness (only panics) but you don't want to do that.
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
    #[inline]
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

    #[inline]
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
    #[inline]
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
    #[inline]
    unsafe fn cursor_mut_unsafe(&self) -> impl DerefMut<Target = Cursor<T>> + '_ {
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
                let c = self.input.cursor();
                if c.buf.len() > self.start_len {
                    std::task::Poll::Ready(())
                } else {
                    std::task::Poll::Pending
                }
            }
        }

        Read {
            input: self,
            start_len: self.cursor().buf.len(),
        }
    }

    #[inline]
    fn read_n(&self, at_least: usize) -> impl Future<Output = ()> + '_ {
        struct ReadAtLeast<'a, T> {
            input: &'a Input<T>,
            start_index: usize,
            at_least: usize,
        }

        impl<T> Future for ReadAtLeast<'_, T> {
            type Output = ();

            fn poll(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Self::Output> {
                let c = self.input.cursor();
                if c.buf.len() >= self.start_index + self.at_least {
                    std::task::Poll::Ready(())
                } else {
                    std::task::Poll::Pending
                }
            }
        }

        ReadAtLeast {
            input: self,
            start_index: self.cursor().index,
            at_least,
        }
    }

    #[inline]
    pub fn start_parsing<'a, O, F, P>(&'a mut self, parser: P) -> Parsing<'a, T, F, O>
    where
        P: Parser<'a, T, O, F>,
        F: Future<Output = O> + Unpin + 'a,
    {
        Parsing::new(self, parser)
    }
}

#[repr(transparent)]
pub struct InputRef<'a, T>(&'a Input<T>);

impl<'a, T> InputRef<'a, T> {
    /// This crate is safe if you can't bring arguments to outside. I believe it is true.
    #[inline]
    pub fn scope_cursor_mut<O>(&mut self, jail: impl FnOnce(&mut Cursor<T>) -> O) -> O {
        let mut cursor = unsafe { self.0.cursor_mut_unsafe() };
        jail(&mut cursor)
    }

    /// This crate is safe if you can't bring arguments to outside. I believe it is true.
    #[inline]
    pub fn scope_cursor<O>(&self, jail: impl FnOnce(&Cursor<T>) -> O) -> O {
        let cursor = self.0.cursor();
        jail(&cursor)
    }

    #[inline]
    pub fn read(&mut self) -> impl Future<Output = ()> + '_ {
        self.0.read()
    }

    #[inline]
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
    #[inline]
    pub fn new<P: Parser<'a, T, O, F>>(input: &'a mut Input<T>, parser: P) -> Self {
        // Dupe mutable ref
        Self {
            future: parser(InputRef(unsafe { std::mem::transmute(&mut *input) })),
            result: None,
            input,
        }
    }

    #[inline]
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
    #[inline]
    pub fn cursor(&self) -> impl Deref<Target = Cursor<T>> + '_ {
        self.input.cursor()
    }

    #[inline]
    pub fn cursor_mut(&mut self) -> impl DerefMut<Target = Cursor<T>> + '_ {
        self.input.cursor_mut()
    }

    #[inline]
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

    #[test]
    fn test_read() {
        let mut input = Input::new(Cursor {
            buf: vec![1, 2, 3],
            index: 0,
        });

        let mut p = input.start_parsing(|mut iref| {
            async move {
                iref.read().await;
            }
            .boxed_local()
        });

        assert!(!p.poll());
        p.cursor_mut().buf.push(4);
        assert!(p.poll());
    }

    #[test]
    fn test_get3() {
        let mut input = Input::new(Cursor {
            buf: Vec::new(),
            index: 0,
        });

        let mut p = input.start_parsing(|mut iref| {
            async move {
                iref.read_n(3).await;
            }
            .boxed_local()
        });

        assert!(!p.poll());
        p.cursor_mut().buf.push(1);
        assert!(!p.poll());
        p.cursor_mut().buf.push(2);
        assert!(!p.poll());
        p.cursor_mut().buf.push(3);
        assert!(p.poll());
    }

    #[test]
    fn test_many0() {
        let mut input = Input::new(Cursor {
            buf: Vec::new(),
            index: 0,
        });

        let mut p = input.start_parsing(|mut iref| {
            async move { many0(&mut iref, |x| *x % 2 == 0).await }.boxed_local()
        });

        p.cursor_mut().buf.push(0);
        assert!(!p.poll());

        p.cursor_mut().buf.push(2);
        assert!(!p.poll());

        p.cursor_mut().buf.push(4);
        assert!(!p.poll());

        p.cursor_mut().buf.push(1);
        assert!(p.poll());

        assert_eq!(p.into_result(), Some(0..3));
    }

    #[test]
    fn test_parsing() {
        let mut input = Input::new(Cursor {
            buf: Vec::new(),
            index: 0,
        });

        let mut parsing = input.start_parsing(move |mut iref: InputRef<u8>| {
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
}
