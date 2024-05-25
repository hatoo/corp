use futures::FutureExt;
use stap::{Cursor, Input, Parsing};

fn main() {
    let mut input = Input::new(Cursor {
        buf: Vec::<u8>::new(),
        index: 0,
    });

    let mut p = Parsing::new(&mut input, |iref| async move { iref }.boxed_local());

    p.poll();

    let iref = p.into_result().unwrap();
    drop(input);
    iref;
}
