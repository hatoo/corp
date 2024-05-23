use std::io::Read;

use futures::FutureExt;
use serde_json::{Map, Number, Value};
use stap::{just, many0, many1, Cursor, Input, InputRef, Parsing};

async fn skip_whitespace(iref: &mut InputRef<'_, u8>) {
    many0(iref, |&c| c.is_ascii_whitespace()).await;
}

async fn number(iref: &mut InputRef<'_, u8>) -> Result<f64, ()> {
    let is_minus = just(iref, b'-').await.is_ok();
    let range = many1(iref, |&c| c.is_ascii_digit() || c == b'.').await?;

    dbg!(&range);

    iref.scope_cursor(move |c| {
        let s = std::str::from_utf8(&c.buf()[range]).unwrap();
        let n: f64 = s.parse().unwrap();
        let n = if is_minus { -n } else { n };
        Ok(n)
    })
}

async fn string(iref: &mut InputRef<'_, u8>) -> Result<String, ()> {
    just(iref, b'"').await?;

    let range = many0(iref, |&c| c != b'"').await;

    just(iref, b'"').await?;

    iref.scope_cursor(move |c| {
        let s = std::str::from_utf8(&c.buf()[range]).unwrap();
        let s = s.to_string();
        Ok(s)
    })
}

async fn object(iref: &mut InputRef<'_, u8>) -> Result<Map<String, Value>, ()> {
    just(iref, b'{').await?;

    let mut map = Map::new();

    let mut first = true;

    loop {
        skip_whitespace(iref).await;

        if just(iref, b'}').await.is_ok() {
            break;
        }

        if !first {
            just(iref, b',').await?;
            skip_whitespace(iref).await;
        }

        first = false;

        let key = string(iref).await?;

        skip_whitespace(iref).await;

        just(iref, b':').await?;

        skip_whitespace(iref).await;

        let value = Box::pin(json(iref)).await?;

        map.insert(key, value);
    }

    Ok(map)
}

async fn json(iref: &mut InputRef<'_, u8>) -> Result<Value, ()> {
    if let Ok(n) = number(iref).await {
        return Ok(Value::Number(Number::from_f64(n).unwrap()));
    }

    if let Ok(s) = string(iref).await {
        return Ok(Value::String(s));
    }

    if let Ok(o) = object(iref).await {
        return Ok(Value::Object(o));
    }

    Err(())
}

fn main() {
    let mut input = Input::new(Cursor {
        buf: Vec::new(),
        index: 0,
    });

    let mut parsing = Parsing::new(&mut input, |mut iref| {
        async move { json(&mut iref).await }.boxed_local()
    });

    while !parsing.poll() {
        let mut buf = [0; 4096];

        let n = std::io::stdin().read(&mut buf).unwrap();

        if n == 0 {
            break;
        }

        parsing.cursor_mut().buf.extend_from_slice(&buf[..n]);
    }

    dbg!(parsing.into_result());
}

#[test]
fn test_json() {
    use serde_json::json;

    let example = r#"{"a": "b", "c": {"d": "e"}, "f": 114514}"#;

    let mut input = Input::new(Cursor {
        buf: example.as_bytes().to_vec(),
        index: 0,
    });

    let mut parsing = Parsing::new(&mut input, |mut iref| {
        async move { json(&mut iref).await }.boxed_local()
    });

    assert!(parsing.poll());

    let expected = json!(
    {
        "a": "b",
        "c": {
            "d": "e"
        },
        "f": 114514.0
    });

    assert_eq!(parsing.into_result().unwrap(), Ok(expected));
}
