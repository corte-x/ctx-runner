// curl \
//    -H 'Content-Type: application/json' \
//    -H 'x-goog-api-key: AIzaSyBShBmCAqwejfhIoZdqf0EFiD_aqoYRsVA' \
//    -d @./data.json \
//    -X POST 'https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-pro-latest:streamGenerateContent?alt=sse'

use anyhow::Context;
use futures_lite::StreamExt;
use http_body_util::{BodyExt, Full};
use hyper::{body::Bytes, Request};
use native_tls::TlsConnector as NativeTlsConnector;
use tokio::net::TcpStream;
use tokio_native_tls::TlsConnector;
use tracing::instrument;

use crate::google::{ChatCompletionRequest, ChatCompletionResponse, Content, Part, Tool};

#[instrument]
pub async fn send(
    req: Request<Full<Bytes>>,
) -> anyhow::Result<impl futures_lite::Stream<Item = anyhow::Result<ChatCompletionResponse>>> {
    let host = req.uri().host().context("host name not found!")?;

    let client_stream = TcpStream::connect((host, 443)).await?;

    let cx = NativeTlsConnector::builder().build()?;
    let cx = TlsConnector::from(cx);

    let client_stream = cx.connect(host, client_stream).await?;

    let io = hyper_util::rt::TokioIo::new(client_stream);

    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;

    tokio::task::spawn(async move {
        if let Err(err) = conn.await {
            tracing::error!(name: "h2", "Connection failed: {:?}", err);
        }
    });

    let (parts, res) = sender.send_request(req).await?.into_parts();

    let s = parts.status;
    tracing::info!(name: "res", status=?s);

    let data_strm = res.into_data_stream();

    // NOTE: BOM is not handled in following stream.
    //       In the UTF-8 encoding, the presence of BOM is optional.
    Ok(data_strm.then(|data| async move {
        let data = data?;

        match core::str::from_utf8(&data)?.split_once(|b| b == ':') {
            Some((_, val)) => Ok(serde_json::from_str(val)?),
            _ => Ok(serde_json::from_slice(&data)?),
        }
    }))
}

pub async fn make(
    ref req: &'_ ChatCompletionRequest,
) -> anyhow::Result<impl futures_lite::Stream<Item = anyhow::Result<ChatCompletionResponse>>> {
    use anyhow::Context;
    let req = Request::builder()
            .method(hyper::Method::POST)
            .uri("https://generativelanguage.googleapis.com/v1beta/models/gemini-1.5-pro-latest:streamGenerateContent?alt=sse")
            .header(
                hyper::header::CONTENT_TYPE,
                "application/json",
            )
            .header("x-goog-api-key", std::env::var_os("GOOGLE_API_KEY").context("`GOOGLE_API_KEY` not found!")?.to_string_lossy().as_ref())
            .body(Full::new(Bytes::from(serde_json::to_vec(&req)?)))?;

    Ok(send(req).await?)
}
