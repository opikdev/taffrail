use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

const VERSION_BODY: &str = r#"{"major":"1","minor":"35","gitVersion":"v1.35.0","gitCommit":"","gitTreeState":"","buildDate":"","goVersion":"","compiler":"","platform":""}"#;

/// What the fake Kubernetes API server answers with.
pub struct Fixture {
    pub pod_list: String,
    pub pod: String,
    pub events: String,
    pub logs: String,
}

/// A kubeconfig whose only context, `dev`, points at `server`.
pub fn kubeconfig(server: &str, current: &str) -> String {
    format!(
        r#"
current-context: {current}
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
    )
}

fn respond(mut stream: &TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).unwrap();
}

fn serve_request(stream: TcpStream, fixture: &Fixture) {
    // Closing with unread data resets the connection on Windows, which loses the response.
    let mut request = BufReader::new(&stream);
    let mut request_line = String::new();
    request.read_line(&mut request_line).unwrap();
    let mut header = String::new();
    while request.read_line(&mut header).unwrap() > "\r\n".len() {
        header.clear();
    }
    if request_line.contains("watch=true") {
        thread::park();
    } else if request_line.contains("/api/v1/pods") {
        respond(&stream, &fixture.pod_list);
    } else if request_line.contains("/log?") {
        respond(&stream, &fixture.logs);
    } else if request_line.contains("/events") {
        respond(&stream, &fixture.events);
    } else if request_line.contains("/pods/") {
        respond(&stream, &fixture.pod);
    } else {
        respond(&stream, VERSION_BODY);
    }
}

/// Starts a server on a free port and returns its URL.
pub fn serve(fixture: Fixture) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    let fixture = Arc::new(fixture);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let stream = stream.unwrap();
            let fixture = fixture.clone();
            thread::spawn(move || serve_request(stream, &fixture));
        }
    });
    server
}
