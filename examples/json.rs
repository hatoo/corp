use std::{collections::HashMap, io::Read};

use futures::FutureExt;
use stap::{copy1, just, many0, many1, tag, Anchor, Cursor, Input, InputRef, Parsing};

async fn skip_whitespace(iref: &mut InputRef<'_, u8>) {
    many0(iref, |&c| c.is_ascii_whitespace()).await;
}

async fn null(iref: &mut InputRef<'_, u8>) -> Result<(), ()> {
    tag(iref, b"null").await.map(|_| ())
}

async fn bool(iref: &mut InputRef<'_, u8>) -> Result<bool, ()> {
    if tag(iref, b"true").await.is_ok() {
        Ok(true)
    } else if tag(iref, b"false").await.is_ok() {
        Ok(false)
    } else {
        Err(())
    }
}

async fn number(iref: &mut InputRef<'_, u8>) -> Result<f64, ()> {
    let mut anchor = Anchor::new(iref);

    let _ = just(&mut anchor, b'-').await;
    many1(&mut anchor, |&c| c.is_ascii_digit()).await?;

    if just(&mut anchor, b'.').await.is_ok() {
        many0(&mut anchor, |&c| c.is_ascii_digit()).await;
    }

    if just(&mut anchor, b'e').await.is_ok() || just(&mut anchor, b'E').await.is_ok() {
        let _ = just(&mut anchor, b'-').await;
        let _ = many1(&mut anchor, |&c| c.is_ascii_digit()).await;
    }

    let range = anchor.range();
    anchor.forget();

    iref.scope_cursor(move |c| {
        let s = std::str::from_utf8(&c.buf()[range]).unwrap();
        let n: f64 = s.parse().unwrap();
        Ok(n)
    })
}

async fn string(iref: &mut InputRef<'_, u8>) -> Result<String, ()> {
    just(iref, b'"').await?;

    let mut buf = Vec::new();

    loop {
        let x = copy1(iref).await;

        if x == b'\\' {
            let e = copy1(iref).await;

            match e {
                b'"' => buf.push(b'"'),
                b'\\' => buf.push(b'\\'),
                b'/' => buf.push(b'/'),
                b'b' => buf.push(b'\x08'),
                b'f' => buf.push(b'\x0c'),
                b'n' => buf.push(b'\n'),
                b'r' => buf.push(b'\r'),
                b't' => buf.push(b'\t'),
                b'u' => {
                    let mut code = 0u32;

                    for _ in 0..4 {
                        let x = copy1(iref).await;

                        code = code * 16
                            + match x {
                                b'0'..=b'9' => (x - b'0') as u32,
                                b'a'..=b'f' => (x - b'a' + 10) as u32,
                                b'A'..=b'F' => (x - b'A' + 10) as u32,
                                _ => unreachable!(),
                            };
                    }

                    buf.extend_from_slice(&code.to_be_bytes());
                }
                _ => unreachable!(),
            }
        } else if x == b'"' {
            break;
        } else {
            buf.push(x);
        }
    }

    let s = String::from_utf8(buf).unwrap();
    Ok(s)
}

async fn array(iref: &mut InputRef<'_, u8>) -> Result<Vec<Json>, ()> {
    just(iref, b'[').await?;

    let mut array = Vec::new();

    let mut first = true;

    loop {
        skip_whitespace(iref).await;

        if just(iref, b']').await.is_ok() {
            break;
        }

        if !first {
            just(iref, b',').await?;
            skip_whitespace(iref).await;
        }

        first = false;

        let value = Box::pin(json(iref)).await?;

        array.push(value);
    }

    Ok(array)
}

async fn object(iref: &mut InputRef<'_, u8>) -> Result<HashMap<String, Json>, ()> {
    just(iref, b'{').await?;

    let mut map = HashMap::new();

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

async fn json(iref: &mut InputRef<'_, u8>) -> Result<Json, ()> {
    if let Ok(()) = null(iref).await {
        return Ok(Json::Null);
    }

    if let Ok(b) = bool(iref).await {
        return Ok(Json::Bool(b));
    }

    if let Ok(n) = number(iref).await {
        return Ok(Json::Number(n));
    }

    if let Ok(s) = string(iref).await {
        return Ok(Json::String(s));
    }

    if let Ok(a) = array(iref).await {
        return Ok(Json::Array(a));
    }

    if let Ok(o) = object(iref).await {
        return Ok(Json::Object(o));
    }

    Err(())
}

#[derive(Debug, PartialEq)]
#[allow(dead_code)]
enum Json {
    Null,
    Bool(bool),
    String(String),
    Number(f64),
    Array(Vec<Json>),
    Object(HashMap<String, Json>),
}

fn main() {
    println!("Please input a JSON string\nIt will exit when parsing has succeeded or failed:");

    let mut input = Input::new(Cursor {
        buf: Vec::new(),
        index: 0,
    });

    let mut parsing = Parsing::new(&mut input, |mut iref| {
        async move { json(&mut iref).await }.boxed_local()
    });

    let mut buf = [0; 4096];
    let mut stdin = std::io::stdin().lock();
    while !parsing.poll() {
        let n = stdin.read(&mut buf).unwrap();

        if n == 0 {
            break;
        }

        parsing.cursor_mut().buf.extend_from_slice(&buf[..n]);
    }

    dbg!(parsing.into_result());
}

#[test]
fn test_json() {
    let example =
        r#"{"a": "b", "c": {"d": "e"}, "f": 114514, "g": [1, "2"], "h": true, "i": null}"#;

    let mut input = Input::new(Cursor {
        buf: example.as_bytes().to_vec(),
        index: 0,
    });

    let mut parsing = Parsing::new(&mut input, |mut iref| {
        async move { json(&mut iref).await }.boxed_local()
    });

    assert!(parsing.poll());

    let expected = Json::Object(
        vec![
            ("a".to_string(), Json::String("b".to_string())),
            (
                "c".to_string(),
                Json::Object(
                    vec![("d".to_string(), Json::String("e".to_string()))]
                        .into_iter()
                        .collect(),
                ),
            ),
            ("f".to_string(), Json::Number(114514f64)),
            (
                "g".to_string(),
                Json::Array(vec![Json::Number(1f64), Json::String("2".to_string())]),
            ),
            ("h".to_string(), Json::Bool(true)),
            ("i".to_string(), Json::Null),
        ]
        .into_iter()
        .collect(),
    );

    assert_eq!(parsing.into_result().unwrap(), Ok(expected));
}
