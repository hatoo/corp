use futures::FutureExt;
use stap::{Cursor, Input, Parsing};

fn main() {
    let mut input = Input::new(Cursor {
        buf: Vec::<u8>::new(),
        index: 0,
    });

    let mut leak = None;

    let mut p = Parsing::new(&mut input, |iref| {
        let leak_mut = &mut leak;
        async move {
            *leak_mut = Some(iref);
        }
        .boxed_local()
    });

    p.poll();

    drop(leak);
}
