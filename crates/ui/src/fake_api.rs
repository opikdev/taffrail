use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const VERSION_BODY: &str = r#"{"major":"1","minor":"35","gitVersion":"v1.35.0","gitCommit":"","gitTreeState":"","buildDate":"","goVersion":"","compiler":"","platform":""}"#;

/// What the fake Kubernetes API server answers with.
pub struct Fixture {
    pub pod_list: String,
    pub pod: String,
    pub events: String,
    pub logs: String,
    /// When set, each log request streams these messages forever instead of `logs`, so tails
    /// stay open and their sources interleave.
    pub live_logs: Option<&'static [&'static str]>,
    /// Objects read by exact path, such as a pod's ReplicaSet and Deployment.
    pub objects: Vec<(String, String)>,
    /// When set, a log request after a pod's first gets this in place of `logs`, as a log
    /// that rotated while the stream was down.
    pub reconnect_logs: Option<String>,
    /// Paths the API refuses with a 403, as it does a role that may not list them.
    pub forbidden: Vec<&'static str>,
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

/// The message Kubernetes RBAC gives for a list it refuses at `path`.
pub fn forbidden_message(path: &str) -> String {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let (group, rest) = match parts.as_slice() {
        ["api", _, rest @ ..] => ("", rest),
        ["apis", group, _, rest @ ..] => (*group, rest),
        _ => ("", &[][..]),
    };
    let (scope, resource) = match rest {
        ["namespaces", namespace, resource] => {
            (format!("in the namespace \"{namespace}\""), *resource)
        }
        [resource] => ("at the cluster scope".to_owned(), *resource),
        _ => (String::new(), ""),
    };
    let qualified = if group.is_empty() {
        resource.to_owned()
    } else {
        format!("{resource}.{group}")
    };
    format!(
        r#"{qualified} is forbidden: User "jane" cannot list resource "{resource}" in API group "{group}" {scope}"#
    )
}

fn respond_forbidden(mut stream: &TcpStream, path: &str) {
    let message = forbidden_message(path).replace('"', "\\\"");
    let body = format!(
        r#"{{"kind":"Status","apiVersion":"v1","status":"Failure","message":"{message}","reason":"Forbidden","code":403}}"#
    );
    let response = format!(
        "HTTP/1.1 403 Forbidden\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).unwrap();
}

fn respond(mut stream: &TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).unwrap();
}

/// Writes one message at a time, stamped with the time since `started` after 10:42, at a pace
/// and starting point that differ by pod. Returns when the client disconnects.
fn stream_logs(mut stream: &TcpStream, request_line: &str, messages: &[&str], started: Instant) {
    let seed = request_line.bytes().map(usize::from).sum::<usize>();
    let pace = Duration::from_millis(150 + (seed % 7) as u64 * 50);
    let head = "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ntransfer-encoding: chunked\r\n\r\n";
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    for message in messages.iter().cycle().skip(seed % messages.len()) {
        let millis = started.elapsed().as_millis();
        let (seconds, millis) = (42 * 60 + millis / 1000, millis % 1000);
        let line = format!(
            "2026-11-07T10:{:02}:{:02}.{millis:03}Z {message}\n",
            seconds / 60 % 60,
            seconds % 60
        );
        let chunk = format!("{:x}\r\n{line}\r\n", line.len());
        if stream.write_all(chunk.as_bytes()).is_err() {
            return;
        }
        thread::sleep(pace);
    }
}

fn serve_request(
    stream: TcpStream,
    fixture: &Fixture,
    logs_read: &Mutex<HashSet<String>>,
    started: Instant,
) {
    // Closing with unread data resets the connection on Windows, which loses the response.
    let mut request = BufReader::new(&stream);
    let mut request_line = String::new();
    request.read_line(&mut request_line).unwrap();
    let mut header = String::new();
    while request.read_line(&mut header).unwrap() > "\r\n".len() {
        header.clear();
    }
    let path = request_line
        .split(' ')
        .nth(1)
        .and_then(|target| target.split('?').next())
        .unwrap_or_default();
    if request_line.contains("watch=true") {
        thread::park();
    } else if fixture.forbidden.contains(&path) {
        respond_forbidden(&stream, path);
    } else if let Some((_, body)) = fixture.objects.iter().find(|(object, _)| object == path) {
        respond(&stream, body);
    } else if request_line.contains("/api/v1/pods") {
        respond(&stream, &fixture.pod_list);
    } else if request_line.contains("/log?") {
        match fixture.live_logs {
            Some(messages) => stream_logs(&stream, &request_line, messages, started),
            None => {
                let first = logs_read.lock().unwrap().insert(path.to_owned());
                match &fixture.reconnect_logs {
                    Some(logs) if !first => respond(&stream, logs),
                    _ => respond(&stream, &fixture.logs),
                }
            }
        }
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
    let logs_read = Arc::new(Mutex::new(HashSet::new()));
    let started = Instant::now();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let stream = stream.unwrap();
            let fixture = fixture.clone();
            let logs_read = logs_read.clone();
            thread::spawn(move || serve_request(stream, &fixture, &logs_read, started));
        }
    });
    server
}
