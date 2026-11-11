//! What a failed request tells the user. The client prints an API error with a debug dump of
//! its status, so every error that reaches the user is put into words here instead.

use std::error::Error;

use kube::client::AuthError;
use kube::config::KubeconfigError;
use kube::core::Status;
use kube::runtime::watcher;

/// The API's own reason for a failed request, such as an RBAC refusal, or for a request that
/// never reached the API, the client's error and its causes.
pub(crate) fn reason(error: &kube::Error) -> String {
    match error {
        kube::Error::Api(status) => status_reason(status),
        kube::Error::HyperError(source) => causes(source),
        kube::Error::Service(source) => causes(source.as_ref()),
        kube::Error::ReadEvents(source) => causes(source),
        error => causes(error),
    }
}

/// The reason a watch failed, without the watcher's wrapping.
pub(crate) fn watch_reason(error: &watcher::Error) -> String {
    match error {
        watcher::Error::InitialListFailed(error)
        | watcher::Error::WatchStartFailed(error)
        | watcher::Error::WatchFailed(error) => reason(error),
        watcher::Error::WatchError(status) => status_reason(status),
        error => causes(error),
    }
}

/// The status's message, or its code and reason when the API sent no message.
pub(crate) fn status_reason(status: &Status) -> String {
    match (status.message.is_empty(), status.reason.is_empty()) {
        (false, _) => status.message.clone(),
        (true, false) => format!("the API answered {} {}", status.code, status.reason),
        (true, true) => format!("the API answered {}", status.code),
    }
}

/// Why a kubeconfig could not be read or used.
pub(crate) fn kubeconfig_reason(error: &KubeconfigError) -> String {
    causes(error)
}

/// The words for an error whose text quotes credentials, or `None` if it quotes none. A
/// login plugin's command line holds its environment, and its output can hold a token. A
/// YAML error quotes the lines around it, which in a kubeconfig can hold keys and tokens.
fn redacted(error: &(dyn Error + 'static)) -> Option<String> {
    match error.downcast_ref::<AuthError>() {
        Some(AuthError::AuthExecRun { out, .. }) => {
            let ended = match out.status.code() {
                Some(code) => format!("the login plugin exited with code {code}"),
                None => "the login plugin stopped before it finished".to_owned(),
            };
            return Some(format!(
                "{ended}. Run kubectl with this context in a terminal to see its message"
            ));
        }
        Some(AuthError::AuthExecParse(_)) => {
            return Some("the login plugin printed a credential that could not be read".to_owned());
        }
        _ => {}
    }
    match error.downcast_ref::<KubeconfigError>() {
        Some(KubeconfigError::Parse(error)) => Some(match error.location() {
            Some(at) => format!(
                "the YAML is not valid at line {}, column {}",
                at.line(),
                at.column()
            ),
            None => "the YAML is not valid".to_owned(),
        }),
        _ => None,
    }
}

/// An error and each of its causes that adds something, joined with colons. Each error's
/// text repeats its causes, so one that quotes credentials replaces the whole chain.
fn causes(error: &(dyn Error + 'static)) -> String {
    let mut link = Some(error);
    while let Some(cause) = link {
        if let Some(text) = redacted(cause) {
            return text;
        }
        link = cause.source();
    }
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let next = cause.to_string();
        if !text.contains(&next) {
            text.push_str(": ");
            text.push_str(&next);
        }
        source = cause.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use std::io;

    use kube::core::Status;
    use kube::runtime::watcher;

    use super::{reason, status_reason, watch_reason};

    fn api_error(code: u16, message: &str, why: &str) -> kube::Error {
        kube::Error::Api(Status::failure(message, why).with_code(code).boxed())
    }

    const REFUSED: &str = r#"events is forbidden: User "jane" cannot list resource "events" in API group "" in the namespace "team-a""#;

    #[test]
    fn an_api_error_says_the_apis_message_alone() {
        let error = api_error(403, REFUSED, "Forbidden");

        assert!(error.to_string().contains("Status {"), "{error}");
        assert_eq!(reason(&error), REFUSED);
    }

    #[test]
    fn an_api_error_without_a_message_says_its_code() {
        assert_eq!(
            reason(&api_error(401, "", "Unauthorized")),
            "the API answered 401 Unauthorized"
        );
        assert_eq!(
            status_reason(&Status::failure("", "").with_code(500)),
            "the API answered 500"
        );
    }

    #[test]
    fn a_watch_error_drops_the_watchers_wrapping() {
        let listed = watcher::Error::InitialListFailed(api_error(403, REFUSED, "Forbidden"));
        let during = watcher::Error::WatchError(Status::failure("too old", "Expired").boxed());

        assert_eq!(watch_reason(&listed), REFUSED);
        assert_eq!(watch_reason(&during), "too old");
        assert_eq!(
            watch_reason(&watcher::Error::NoResourceVersion),
            watcher::Error::NoResourceVersion.to_string()
        );
    }

    #[test]
    fn a_stream_error_says_its_cause_without_the_clients_prefix() {
        let error = kube::Error::ReadEvents(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "connection reset by peer",
        ));

        assert_eq!(reason(&error), "connection reset by peer");
    }

    #[tokio::test]
    async fn an_unreachable_server_says_why_without_debug_output() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let config = kube::Config::new(format!("http://{address}").parse().unwrap());
        let client = kube::Client::try_from(config).unwrap();

        let error = client.apiserver_version().await.unwrap_err();

        let reason = reason(&error);
        assert!(!reason.contains("Error:"), "{reason}");
        assert!(!reason.contains('{'), "{reason}");
        assert!(reason.to_lowercase().contains("connect"), "{reason}");
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_login_plugin_behind_other_errors_shows_no_secrets() {
        use std::os::unix::process::ExitStatusExt;
        use std::process::{ExitStatus, Output};

        let failed = kube::client::AuthError::AuthExecRun {
            cmd: r#"AWS_SECRET_ACCESS_KEY="env-secret" "aws""#.to_owned(),
            out: Box::new(Output {
                status: ExitStatus::from_raw(255 << 8),
                stdout: b"out-token".to_vec(),
                stderr: b"err-secret".to_vec(),
            }),
        };
        let error = kube::Error::Service(Box::new(kube::Error::Auth(failed)));

        let reason = reason(&error);

        assert!(
            reason.starts_with("the login plugin exited with code 255"),
            "{reason}"
        );
        for secret in ["env-secret", "out-token", "err-secret"] {
            assert!(!reason.contains(secret), "{reason}");
        }
    }
}
