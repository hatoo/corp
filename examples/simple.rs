use futures::FutureExt;
use stap::{many1, Anchor, Cursor, Input, Parsing};

fn main() {
    let mut input = Input::new(Cursor {
        buf: Vec::<u8>::new(),
        index: 0,
    });

    let mut parsing = Parsing::new(&mut input, |mut iref| {
        async move {
            // anchoring the current index
            // restoring the index if parsing fails (= when dropped)
            let mut anchor = Anchor::new(&mut iref);

            let num = many1(&mut anchor, |b| b.is_ascii_digit()).await?;
            // Since the parser holds state, this output is showed up only once.
            dbg!(&num);
            let alpha = many1(&mut anchor, |b| b.is_ascii_alphabetic()).await?;
            dbg!(&alpha);

            // parsing is successful, so we can forget the anchor in other words, current index of the input is valid.
            anchor.forget();

            Ok::<_, ()>((num, alpha))
        }
        // This makes the future `Unpin`, currently this is required but any workaround is welcome.
        .boxed_local()
    });

    // There is no input to parse, so parsing should fail.
    assert!(!parsing.poll());

    parsing.cursor_mut().buf.extend_from_slice(b"123a");
    // The parser recognizing the input as a number followed by an alphabet. But because of there may be more alphabets, it should fail.
    assert!(!parsing.poll());

    parsing.cursor_mut().buf.extend_from_slice(b"bc;");
    // the parser should ends.
    assert!(parsing.poll());

    dbg!(parsing.into_result());
}
