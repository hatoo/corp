use futures::FutureExt;
use stap::{Cursor, Input, ParsingInput};

fn main() {
    let input = Input::new(Cursor {
        buf: Vec::<u8>::new(),
        index: 0,
    });

    let mut leak = None;

    let mut pi = ParsingInput::new(Box::new(input), |iref| {
        let leak_mut = &mut leak;
        async move {
            *leak_mut = Some(iref);
        }
        .boxed_local()
    });

    pi.poll();

    drop(leak);
}
