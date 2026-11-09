//! A fake Kubernetes API server for tests: fixed objects by path, and a short log for any pod.

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::cluster::{Connection, Contexts};

/// An exact request path and the JSON it answers with.
pub type Route = (&'static str, &'static str);

const VERSION_BODY: &str = r#"{"major":"1","minor":"35","gitVersion":"v1.35.0","gitCommit":"","gitTreeState":"","buildDate":"","goVersion":"","compiler":"","platform":""}"#;
const NOT_FOUND_BODY: &str = r#"{"kind":"Status","apiVersion":"v1","status":"Failure","message":"not found","reason":"NotFound","code":404}"#;

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

async fn serve_request(mut stream: TcpStream, routes: &[Route]) {
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
    let path = target.split('?').next().unwrap_or_default();
    if let Some((_, body)) = routes.iter().find(|(route, _)| *route == path) {
        respond(&mut stream, "200 OK", "application/json", body).await;
    } else if let Some(pod) = path
        .strip_suffix("/log")
        .and_then(|pod| pod.rsplit('/').next())
    {
        let body = format!("{}\n", log_line(pod));
        respond(&mut stream, "200 OK", "text/plain", &body).await;
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
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let routes: &'static [Route] = routes.leak();
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve_request(stream, routes));
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
