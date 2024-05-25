use futures::FutureExt;
use stap::{Cursor, Input, ParsingInput};

fn main() {
    let input = Input::new(Cursor {
        buf: Vec::<u8>::new(),
        index: 0,
    });

    let mut pi = ParsingInput::new(Box::new(input), |iref| async move { iref }.boxed_local());

    pi.poll();

    let iref = pi.result_mut();

    drop(pi);

    iref;
}
