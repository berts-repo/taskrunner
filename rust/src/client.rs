//! HTTP GET over the daemon's unix socket, for the CLI and the shim.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;

pub struct Response {
    pub status: u16,
    pub body: String,
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Fetches `path_and_query` (e.g. `/status`) from the daemon on `socket`.
pub async fn get(
    socket: &Path,
    path_and_query: &str,
    timeout: Duration,
) -> anyhow::Result<Response> {
    tokio::time::timeout(timeout, fetch(socket, path_and_query))
        .await
        .with_context(|| format!("GET {path_and_query} timed out"))?
}

async fn fetch(socket: &Path, path_and_query: &str) -> anyhow::Result<Response> {
    let stream = UnixStream::connect(socket).await?;
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
    tokio::spawn(connection);
    let request = hyper::Request::builder()
        .uri(path_and_query)
        .header(hyper::header::HOST, "taskrunner")
        .body(Empty::<Bytes>::new())?;
    let response = sender.send_request(request).await?;
    let status = response.status().as_u16();
    let body = response.into_body().collect().await?.to_bytes();
    Ok(Response { status, body: String::from_utf8_lossy(&body).into_owned() })
}
