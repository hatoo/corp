use std::{
    future::Future,
    ops::{Deref, DerefMut, Range},
    pin::Pin,
    task::{Context, Poll},
};

use futures::task::noop_waker_ref;

/// buf and index
#[derive(Debug)]
pub struct Cursor<T> {
    /// Sequence of items, you may append items to this when you want.
    /// I think reducing the items of `buf` isn't introduce an unsoundness (only may panics) but you don't want to do that.
    pub buf: Vec<T>,
    /// Some of parsers in this crate assumes index <= buf.len().
    pub index: usize,
}

impl<T> Cursor<T> {
    fn sanity_check(&self) -> bool {
        self.index <= self.buf.len()
    }
}

#[cfg(debug_assertions)]
#[repr(transparent)]
#[derive(Debug)]
pub struct Input<T>(std::cell::RefCell<Cursor<T>>);

#[cfg(not(debug_assertions))]
#[repr(transparent)]
#[derive(Debug)]
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
    pub fn start_parsing<'a, O, F, P>(&'a mut self, parser: P) -> Parsing<'a, T, O, F>
    where
        P: Parser<'a, T, O, F>,
        F: Future<Output = O> + Unpin + 'a,
    {
        Parsing::new(self, parser)
    }

    #[inline]
    pub fn into_parsing_input<O, F>(self) -> ParsingInput<T, O, F> {
        ParsingInput::<T, O, F>::new(self)
    }
}

#[derive(Debug)]
pub struct CursorRef<'a, T> {
    cursor: &'a mut Cursor<T>,
}

// you can't get &mut buf
impl<'a, T> CursorRef<'a, T> {
    #[inline]
    pub fn index(&self) -> usize {
        self.cursor.index
    }

    #[inline]
    pub fn index_mut(&mut self) -> &mut usize {
        &mut self.cursor.index
    }

    #[inline]
    pub fn remaining(&self) -> &[T] {
        &self.cursor.buf[self.cursor.index..]
    }

    #[inline]
    pub fn buf(&self) -> &[T] {
        &self.cursor.buf
    }
}

#[repr(transparent)]
#[derive(Debug)]
pub struct InputRef<'a, T>(&'a Input<T>);

impl<'a, T> InputRef<'a, T> {
    /// This crate is safe if you can't bring arguments to outside. I believe it is true.
    #[inline]
    pub fn scope_cursor_mut<O>(&mut self, jail: impl FnOnce(&mut CursorRef<T>) -> O) -> O {
        let mut cursor = unsafe { self.0.cursor_mut_unsafe() };
        debug_assert!(cursor.sanity_check());
        jail(&mut CursorRef {
            cursor: &mut cursor,
        })
    }

    /// This crate is safe if you can't bring arguments to outside. I believe it is true.
    #[inline]
    pub fn scope_cursor<O>(&self, jail: impl FnOnce(&CursorRef<T>) -> O) -> O {
        let mut cursor = unsafe { self.0.cursor_mut_unsafe() };
        debug_assert!(cursor.sanity_check());
        jail(&CursorRef {
            cursor: &mut cursor,
        })
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

#[derive(Debug)]
pub struct Parsing<'a, T, O, F> {
    input: &'a Input<T>,
    result: Option<O>,
    future: F,
}

impl<'a, T, O, F> Parsing<'a, T, O, F>
where
    F: Future<Output = O> + Unpin + 'a,
{
    #[inline]
    pub fn new<P: Parser<'a, T, O, F>>(input: &'a mut Input<T>, parser: P) -> Self {
        // Dupe mutable ref
        Self {
            future: parser(InputRef(unsafe {
                std::mem::transmute::<&mut Input<T>, &Input<T>>(&mut *input)
            })),
            result: None,
            input,
        }
    }

    #[inline]
    pub fn poll(&mut self) -> bool {
        let mut cx = Context::from_waker(noop_waker_ref());
        match Pin::new(&mut self.future).poll(&mut cx) {
            Poll::Ready(result) => {
                self.result = Some(result);
                true
            }
            Poll::Pending => false,
        }
    }
}

impl<'a, T, O, F> Parsing<'a, T, O, F> {
    #[inline]
    pub fn cursor(&self) -> impl Deref<Target = Cursor<T>> + '_ {
        self.input.cursor()
    }

    #[inline]
    pub fn cursor_mut(&mut self) -> impl DerefMut<Target = Cursor<T>> + '_ {
        unsafe { self.input.cursor_mut_unsafe() }
    }

    #[inline]
    pub fn into_result(self) -> Option<O> {
        self.result
    }
}

#[derive(Debug)]
pub struct ParsingInput<T, O, F> {
    input: Input<T>,
    result: Option<O>,
    future: Option<F>,
}

impl<T, O, F> ParsingInput<T, O, F> {
    pub fn new(input: Input<T>) -> Self {
        Self {
            input,
            result: None,
            future: None,
        }
    }

    #[inline]
    pub fn cursor(&self) -> impl Deref<Target = Cursor<T>> + '_ {
        self.input.cursor()
    }

    #[inline]
    pub fn cursor_mut(&mut self) -> impl DerefMut<Target = Cursor<T>> + '_ {
        self.input.cursor_mut()
    }

    #[inline]
    pub fn result_mut(&mut self) -> Option<&mut O> {
        self.result.as_mut()
    }

    #[inline]
    pub fn into_input(self) -> Input<T> {
        self.input
    }
}

impl<T, O, F> ParsingInput<T, O, F>
where
    F: Future<Output = O>,
{
    #[inline]
    pub fn start_parsing<'a, P: Parser<'a, T, O, F>>(&mut self, parser: P)
    where
        T: 'a,
        F: 'a,
    {
        self.future = Some(parser(InputRef(unsafe {
            std::mem::transmute::<&Input<T>, &Input<T>>(&self.input)
        })));
    }

    #[inline]
    pub fn poll(&mut self) -> bool
    where
        F: Unpin,
    {
        if let Some(future) = self.future.as_mut() {
            let mut cx = Context::from_waker(noop_waker_ref());
            match Pin::new(future).poll(&mut cx) {
                Poll::Ready(result) => {
                    self.result = Some(result);
                    self.future = None;
                    true
                }
                Poll::Pending => false,
            }
        } else {
            false
        }
    }
}

pub async fn just<T>(input: &mut InputRef<'_, T>, t: T) -> Result<usize, ()>
where
    T: PartialEq,
{
    input.read_n(1).await;

    input.scope_cursor_mut(|cursor| {
        if cursor.remaining()[0] == t {
            *cursor.index_mut() += 1;
            Ok(cursor.index())
        } else {
            Err(())
        }
    })
}

pub async fn tag<T>(input: &mut InputRef<'_, T>, tag: &[T]) -> Result<Range<usize>, ()>
where
    T: PartialEq,
{
    input.read_n(tag.len()).await;

    input.scope_cursor_mut(|cursor| {
        if cursor.remaining().starts_with(tag) {
            let start = cursor.index();
            *cursor.index_mut() += tag.len();
            Ok(start..cursor.index())
        } else {
            Err(())
        }
    })
}

pub async fn many0<T>(
    input: &mut InputRef<'_, T>,
    mut cond: impl FnMut(&T) -> bool,
) -> Range<usize> {
    let start = input.scope_cursor(|c| c.index());

    loop {
        if let Some(r) = input.scope_cursor_mut(|c| {
            for (i, item) in c.remaining().iter().enumerate() {
                if !cond(item) {
                    *c.index_mut() += i;
                    return Some(start..c.index());
                }
            }

            let len = c.buf().len();
            *c.index_mut() = len;
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

    #[test]
    fn test_parsing_input() {
        let input = Input::new(Cursor {
            buf: Vec::new(),
            index: 0,
        });

        let mut parsing_input = input.into_parsing_input();

        parsing_input.start_parsing(move |mut iref: InputRef<u8>| {
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

        parsing_input.cursor_mut().buf.extend(b"abc");
        assert!(!parsing_input.poll());
        parsing_input.cursor_mut().buf.extend(b"123");
        assert!(!parsing_input.poll());
        parsing_input.cursor_mut().buf.extend(b"abc");
        assert!(!parsing_input.poll());
        parsing_input.cursor_mut().buf.extend(b";");
        assert!(parsing_input.poll());
        assert_eq!(parsing_input.result_mut(), Some(&mut (0..3, 3..6, 6..9)));
    }
}
