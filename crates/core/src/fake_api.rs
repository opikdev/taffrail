//! A fake Kubernetes API server for tests: fixed objects by path, a short log for any
//! pod, and a handler a test can answer any request with.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::cluster::{Connection, Contexts};

/// An exact request path and the JSON it answers with.
pub type Route = (&'static str, &'static str);

const VERSION_BODY: &str = r#"{"major":"1","minor":"35","gitVersion":"v1.35.0","gitCommit":"","gitTreeState":"","buildDate":"","goVersion":"","compiler":"","platform":""}"#;
const NOT_FOUND_BODY: &str = r#"{"kind":"Status","apiVersion":"v1","status":"Failure","message":"not found","reason":"NotFound","code":404}"#;

/// A request the fake server received.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    pub path: String,
    /// Query parameters, decoded.
    pub query: HashMap<String, String>,
}

/// How the fake server answers a request.
#[derive(Clone)]
pub enum Reply {
    Json(String),
    /// A whole plain-text body, as a log that ends.
    Text(String),
    /// Sends the body, then drops the connection mid-response, as a lost connection.
    Cut(String),
    /// A Kubernetes error status with this code.
    Status(u16),
    /// A 403 with this message, as the API refuses a role.
    Forbidden(&'static str),
    /// Never answers, as a watch with no changes.
    Hang,
}

/// Answers a request, or leaves it to the fixed routes with `None`.
pub type Handler = Arc<dyn Fn(&Request) -> Option<Reply> + Send + Sync>;

/// The line every pod's log holds, so a test can tell sources apart.
pub fn log_line(pod: &str) -> String {
    format!("2026-01-01T00:00:00Z hello from {pod}")
}

async fn respond(stream: &mut TcpStream, status: &str, kind: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await.ok();
}

async fn respond_cut(stream: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ntransfer-encoding: chunked\r\n\r\n{:x}\r\n{body}\r\n",
        body.len()
    );
    stream.write_all(response.as_bytes()).await.ok();
    stream.flush().await.ok();
}

async fn reply(stream: &mut TcpStream, reply: Reply) {
    match reply {
        Reply::Json(body) => respond(stream, "200 OK", "application/json", &body).await,
        Reply::Text(body) => respond(stream, "200 OK", "text/plain", &body).await,
        Reply::Cut(body) if body.is_empty() => {}
        Reply::Cut(body) => respond_cut(stream, &body).await,
        Reply::Status(code) => {
            let body = format!(
                r#"{{"kind":"Status","apiVersion":"v1","status":"Failure","message":"status {code}","reason":"","code":{code}}}"#
            );
            respond(stream, &format!("{code} Failed"), "application/json", &body).await;
        }
        Reply::Forbidden(message) => {
            let body = format!(
                r#"{{"kind":"Status","apiVersion":"v1","status":"Failure","message":"{message}","reason":"Forbidden","code":403}}"#
            );
            respond(stream, "403 Forbidden", "application/json", &body).await;
        }
        Reply::Hang => std::future::pending().await,
    }
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = bytes
            .get(index + 1..index + 3)
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (bytes[index], hex) {
            (b'%', Some(byte)) => {
                decoded.push(byte);
                index += 3;
            }
            (b'+', _) => {
                decoded.push(b' ');
                index += 1;
            }
            (byte, _) => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn parse_request(target: &str) -> Request {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    Request {
        path: path.to_owned(),
        query: query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| {
                let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                (decode(key), decode(value))
            })
            .collect(),
    }
}

/// A pod with no status, for a path that names one.
fn pod_body(path: &str) -> Option<String> {
    let mut parts = path.strip_prefix("/api/v1/namespaces/")?.split('/');
    let (namespace, kind, name) = (parts.next()?, parts.next()?, parts.next()?);
    if kind != "pods" || parts.next().is_some() {
        return None;
    }
    Some(format!(
        r#"{{"apiVersion":"v1","kind":"Pod","metadata":{{"name":"{name}","namespace":"{namespace}","uid":"uid-{name}"}}}}"#
    ))
}

async fn serve_request(mut stream: TcpStream, routes: &[Route], handler: Option<Handler>) {
    let mut reader = BufReader::new(&mut stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).await.is_err() {
        return;
    }
    let mut header = String::new();
    while reader
        .read_line(&mut header)
        .await
        .is_ok_and(|read| read > 2)
    {
        header.clear();
    }
    let target = request_line.split(' ').nth(1).unwrap_or_default();
    let request = parse_request(target);
    let path = request.path.as_str();
    if let Some(answer) = handler.as_ref().and_then(|handler| handler(&request)) {
        reply(&mut stream, answer).await;
    } else if let Some((_, body)) = routes.iter().find(|(route, _)| *route == path) {
        respond(&mut stream, "200 OK", "application/json", body).await;
    } else if let Some(pod) = path
        .strip_suffix("/log")
        .and_then(|pod| pod.rsplit('/').next())
    {
        let body = format!("{}\n", log_line(pod));
        respond(&mut stream, "200 OK", "text/plain", &body).await;
    } else if let Some(body) = pod_body(path) {
        respond(&mut stream, "200 OK", "application/json", &body).await;
    } else if path == "/version" {
        respond(&mut stream, "200 OK", "application/json", VERSION_BODY).await;
    } else {
        respond(
            &mut stream,
            "404 Not Found",
            "application/json",
            NOT_FOUND_BODY,
        )
        .await;
    }
}

/// Starts a server and connects to it as context `dev`.
pub async fn connect(routes: Vec<Route>) -> Connection {
    serve(routes, None).await
}

/// Starts a server that asks `handler` first, and connects to it as context `dev`.
pub async fn connect_with(handler: Handler) -> Connection {
    serve(Vec::new(), Some(handler)).await
}

async fn serve(routes: Vec<Route>, handler: Option<Handler>) -> Connection {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let routes: &'static [Route] = routes.leak();
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve_request(stream, routes, handler.clone()));
        }
    });
    let kubeconfig = format!(
        r#"
current-context: dev
clusters:
  - name: dev
    cluster:
      server: {server}
contexts:
  - name: dev
    context:
      cluster: dev
      user: dev
users:
  - name: dev
    user: {{}}
"#
    );
    Contexts::from_yaml(&kubeconfig)
        .unwrap()
        .connect("dev".into())
        .await
        .unwrap()
}
