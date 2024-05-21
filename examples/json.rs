use std::{collections::HashMap, io::Read};

use futures::FutureExt;
use stap::{just, many0, Cursor, Input, InputRef, Parsing};

async fn skip_whitespace(iref: &mut InputRef<'_, u8>) {
    many0(iref, |&c| c.is_ascii_whitespace()).await;
}

async fn string(iref: &mut InputRef<'_, u8>) -> Result<String, ()> {
    just(iref, b'"').await?;

    let range = many0(iref, |&c| c != b'"').await;

    iref.scope_cursor_mut(move |c| {
        *c.index_mut() += 1;
    });

    iref.scope_cursor(move |c| {
        let s = std::str::from_utf8(&c.buf()[range]).unwrap();
        let s = s.to_string();
        Ok(s)
    })
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
    if let Ok(s) = string(iref).await {
        return Ok(Json::String(s));
    }

    if let Ok(o) = object(iref).await {
        return Ok(Json::Object(o));
    }

    Err(())
}

#[derive(Debug)]
enum Json {
    String(String),
    Object(HashMap<String, Json>),
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
