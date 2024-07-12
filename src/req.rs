use core::fmt;

use anyhow::Context;
use futures_lite::StreamExt;
use http_body_util::{BodyExt, Full};
use hyper::{body::Bytes, Request};
use tokio::net::TcpStream;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum Event {
    Retry(u64),
    Event(String),
    Data(String),
    Id(String),
    Comment(String),
    #[default]
    EOF,
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Event::Retry(s) => f.write_fmt(format_args!("retry({s})")),
            Event::Event(s) => f.write_fmt(format_args!("event({s})")),
            Event::Data(s) => f.write_fmt(format_args!("data({s})")),
            Event::Id(s) => f.write_fmt(format_args!("id({s})")),
            Event::Comment(s) => f.write_fmt(format_args!("comment({s})")),
            Event::EOF => write!(f, "EOF"),
        }
    }
}

// curl \
//    -H 'Content-Type: application/json' \
//    -H 'x-goog-api-key: AIzaSyBShBmCAqwejfhIoZdqf0EFiD_aqoYRsVA' \
//    -d @./data.json \
//    -X POST 'https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-pro-latest:streamGenerateContent?alt=sse'

pub async fn send(
    req: Request<Full<Bytes>>,
) -> anyhow::Result<impl futures_lite::Stream<Item = Event>> {
    let host = req.uri().host().context("host name not found!")?;

    let client_stream = TcpStream::connect((host, 443)).await?;

    let cx = native_tls::TlsConnector::builder().build()?;
    let cx = tokio_native_tls::TlsConnector::from(cx);

    let client_stream = cx.connect(host, client_stream).await?;

    let io = hyper_util::rt::TokioIo::new(client_stream);

    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;

    tokio::task::spawn(async move {
        if let Err(err) = conn.await {
            tracing::error!(name: "h2", "Connection failed: {:?}", err);
        }
    });

    let (_, res) = sender.send_request(req).await?.into_parts();

    let mut data_strm = res.into_data_stream();

    let res = data_strm.next().await.context("no data")??;

    let next = res.splitn(2, |&b| b == b':').next();

    if let Some(next) = next {
        if !next
            .iter()
            .all(|&c| <char as From<u8>>::from(c).is_alphanumeric())
        {
            let err = core::str::from_utf8(&res)?.to_string();
            anyhow::bail!(err);
        }
    }

    // NOTE: BOM is not handled in following stream.
    //       In the UTF-8 encoding, the presence of BOM is optional.
    Ok(futures_lite::stream::once(Ok(Bytes::from(res)))
        .chain(data_strm)
        .filter_map(move |res| {
            let res = res.ok()?;

            let mut parts = res.splitn(2, |&b| b == b':');

            let Some(label) = parts.next().filter(|s| {
                s.iter()
                    .map(|c| <char as From<u8>>::from(*c))
                    .all(char::is_alphanumeric)
            }) else {
                return Some(Event::Comment(std::str::from_utf8(&res).ok()?.to_string()));
            };

            let Some(value) = parts.next() else {
                return Some(Event::EOF);
            };

            let value = value.trim_ascii_end();

            match label {
                b"data" => Some(Event::Data(
                    std::str::from_utf8(value.strip_prefix(b" ")?)
                        .ok()?
                        .to_string(),
                )),

                b"retry" => Some(Event::Retry(
                    core::str::from_utf8(value).ok()?.parse::<u64>().ok()?,
                )),

                b"event" => Some(Event::Event(
                    std::str::from_utf8(value.strip_prefix(b" ")?)
                        .ok()?
                        .to_string(),
                )),

                b"id" => Some(Event::Id(
                    std::str::from_utf8(value.strip_prefix(b" ")?)
                        .ok()?
                        .to_string(),
                )),

                _ => Some(Event::Comment(std::str::from_utf8(&res).ok()?.to_string())),
            }
        }))
}
