use std::future::Future;
use std::io;
use std::time::Duration;

use kube::config::{KubeConfigOptions, Kubeconfig, KubeconfigError};
use kube::{Client, Config};
use tokio::runtime::{self, Runtime};

use crate::details::{self, PodDetails};
use crate::logs::{self, LogTail, LogTarget};
use crate::pods::{self, PodWatch};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const WORKER_THREADS: usize = 2;

#[derive(Debug, thiserror::Error)]
pub enum ClusterError {
    #[error("Failed to read kubeconfig: {0}")]
    ReadKubeconfig(#[source] KubeconfigError),
    #[error("Failed to load context \"{context}\": {source}")]
    LoadContext {
        context: String,
        source: KubeconfigError,
    },
    #[error("Failed to reach context \"{context}\": {source}")]
    Connect {
        context: String,
        source: kube::Error,
    },
    #[error("Context \"{0}\" did not respond within {CONNECT_TIMEOUT:?}")]
    Timeout(String),
    #[error("Connection task for context \"{context}\" failed: {source}")]
    Task {
        context: String,
        source: tokio::task::JoinError,
    },
    #[error("Failed to read pod {pod}: {error}")]
    PodDetails { pod: String, error: String },
}

/// The contexts in the user's kubeconfig.
#[derive(Clone)]
pub struct Contexts {
    kubeconfig: Kubeconfig,
}

impl Contexts {
    /// Reads `KUBECONFIG`, falling back to `~/.kube/config`.
    pub fn load() -> Result<Self, ClusterError> {
        Kubeconfig::read()
            .map(|kubeconfig| Self { kubeconfig })
            .map_err(ClusterError::ReadKubeconfig)
    }

    pub fn from_yaml(yaml: &str) -> Result<Self, ClusterError> {
        Kubeconfig::from_yaml(yaml)
            .map(|kubeconfig| Self { kubeconfig })
            .map_err(ClusterError::ReadKubeconfig)
    }

    pub fn names(&self) -> Vec<String> {
        self.kubeconfig
            .contexts
            .iter()
            .map(|context| context.name.clone())
            .collect()
    }

    /// The current context, or `None` if it names no known context.
    pub fn current(&self) -> Option<&str> {
        let current = self.kubeconfig.current_context.as_deref()?;
        self.kubeconfig
            .contexts
            .iter()
            .any(|context| context.name == current)
            .then_some(current)
    }

    /// `saved` if it still exists, otherwise the current context.
    pub fn preferred<'a>(&'a self, saved: Option<&'a str>) -> Option<&'a str> {
        saved
            .filter(|saved| {
                self.kubeconfig
                    .contexts
                    .iter()
                    .any(|context| context.name == *saved)
            })
            .or_else(|| self.current())
    }

    async fn connect(self, context: String) -> Result<Connection, ClusterError> {
        let options = KubeConfigOptions {
            context: Some(context.clone()),
            ..KubeConfigOptions::default()
        };
        let config = Config::from_custom_kubeconfig(self.kubeconfig, &options)
            .await
            .map_err(|source| ClusterError::LoadContext {
                context: context.clone(),
                source,
            })?;
        let request = async {
            let client = Client::try_from(config)?;
            let version = client.apiserver_version().await?;
            Ok::<_, kube::Error>((client, version))
        };
        let (client, version) = tokio::time::timeout(CONNECT_TIMEOUT, request)
            .await
            .map_err(|_| ClusterError::Timeout(context.clone()))?
            .map_err(|source| ClusterError::Connect {
                context: context.clone(),
                source,
            })?;
        Ok(Connection {
            context,
            server_version: version.git_version,
            client,
        })
    }
}

/// A context that answered a request.
#[derive(Clone)]
pub struct Connection {
    pub context: String,
    pub server_version: String,
    client: Client,
}

/// Runs cluster work on its own threads, away from the UI executor.
pub struct Backend {
    runtime: Runtime,
}

impl Backend {
    pub fn new() -> io::Result<Self> {
        let runtime = runtime::Builder::new_multi_thread()
            .worker_threads(WORKER_THREADS)
            .enable_all()
            .build()?;
        Ok(Self { runtime })
    }

    /// Connects to `context`, running any exec auth plugin it configures.
    pub fn connect(
        &self,
        contexts: Contexts,
        context: String,
    ) -> impl Future<Output = Result<Connection, ClusterError>> + use<> {
        let task = self.runtime.spawn(contexts.connect(context.clone()));
        async move {
            task.await
                .map_err(|source| ClusterError::Task { context, source })?
        }
    }

    /// Watches every pod the connection can see, until the returned watch is dropped.
    pub fn watch_pods(&self, connection: &Connection) -> PodWatch {
        let _runtime = self.runtime.enter();
        pods::watch(connection.client.clone())
    }

    /// Reads one pod and its recent events.
    pub fn pod_details(
        &self,
        connection: &Connection,
        namespace: String,
        name: String,
    ) -> impl Future<Output = Result<PodDetails, ClusterError>> + use<> {
        let pod = format!("{namespace}/{name}");
        let task = self
            .runtime
            .spawn(details::load(connection.client.clone(), namespace, name));
        async move {
            task.await.map_err(|error| ClusterError::PodDetails {
                pod,
                error: error.to_string(),
            })?
        }
    }

    /// Follows `targets`, until the returned tail is dropped.
    pub fn tail_logs(
        &self,
        connection: &Connection,
        targets: Vec<LogTarget>,
    ) -> io::Result<LogTail> {
        let _runtime = self.runtime.enter();
        logs::start(connection.client.clone(), targets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const VERSION_BODY: &str = r#"{"major":"1","minor":"35","gitVersion":"v1.35.0","gitCommit":"","gitTreeState":"","buildDate":"","goVersion":"","compiler":"","platform":""}"#;

    fn kubeconfig(server: &str, user: &str) -> String {
        format!(
            r#"
apiVersion: v1
kind: Config
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
  - name: broken
    context:
      cluster: missing
      user: dev
users:
  - name: dev
    user: {user}
"#
        )
    }

    /// Serves one `/version` request and returns the raw request text.
    async fn serve_version(listener: TcpListener) -> String {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let read = stream.read(&mut request).await.unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{VERSION_BODY}",
            VERSION_BODY.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        String::from_utf8_lossy(&request[..read]).into_owned()
    }

    #[test]
    fn lists_contexts_and_the_current_one() {
        let contexts = Contexts::from_yaml(&kubeconfig("http://127.0.0.1:1", "{}")).unwrap();

        assert_eq!(contexts.names(), ["dev", "broken"]);
        assert_eq!(contexts.current(), Some("dev"));
    }

    #[test]
    fn ignores_a_current_context_that_does_not_exist() {
        let yaml = kubeconfig("http://127.0.0.1:1", "{}")
            .replace("current-context: dev", "current-context: gone");

        let contexts = Contexts::from_yaml(&yaml).unwrap();

        assert_eq!(contexts.current(), None);
    }

    #[test]
    fn prefers_a_saved_context_that_still_exists() {
        let contexts = Contexts::from_yaml(&kubeconfig("http://127.0.0.1:1", "{}")).unwrap();

        assert_eq!(contexts.preferred(Some("broken")), Some("broken"));
    }

    #[test]
    fn falls_back_to_the_current_context_without_a_usable_saved_one() {
        let contexts = Contexts::from_yaml(&kubeconfig("http://127.0.0.1:1", "{}")).unwrap();

        assert_eq!(contexts.preferred(Some("gone")), Some("dev"));
        assert_eq!(contexts.preferred(None), Some("dev"));
    }

    #[test]
    fn rejects_invalid_yaml() {
        let Err(error) = Contexts::from_yaml("clusters: [") else {
            panic!("invalid YAML should not load");
        };

        assert!(error.to_string().starts_with("Failed to read kubeconfig"));
    }

    #[tokio::test]
    async fn connect_reports_a_context_with_an_unknown_cluster() {
        let contexts = Contexts::from_yaml(&kubeconfig("http://127.0.0.1:1", "{}")).unwrap();

        let Err(error) = contexts.connect("broken".into()).await else {
            panic!("a missing cluster should not connect");
        };

        assert!(error.to_string().contains("\"broken\""), "{error}");
    }

    #[tokio::test]
    async fn connect_reports_an_unreachable_server() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let server = format!("http://127.0.0.1:{port}");
        let contexts = Contexts::from_yaml(&kubeconfig(&server, "{}")).unwrap();

        let Err(error) = contexts.connect("dev".into()).await else {
            panic!("a closed port should not connect");
        };

        assert!(
            error
                .to_string()
                .starts_with("Failed to reach context \"dev\""),
            "{error}"
        );
    }

    #[tokio::test]
    async fn connect_returns_the_server_version() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let contexts = Contexts::from_yaml(&kubeconfig(&server, "{}")).unwrap();
        let served = tokio::spawn(serve_version(listener));

        let connection = contexts.connect("dev".into()).await.unwrap();

        assert_eq!(connection.context, "dev");
        assert_eq!(connection.server_version, "v1.35.0");
        assert!(served.await.unwrap().starts_with("GET /version"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn connect_sends_the_token_from_an_exec_plugin() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let exec = r#"
      exec:
        apiVersion: client.authentication.k8s.io/v1
        command: sh
        args:
          - -c
          - 'echo "{\"apiVersion\":\"client.authentication.k8s.io/v1\",\"kind\":\"ExecCredential\",\"status\":{\"token\":\"exec-token\"}}"'
        interactiveMode: Never
"#;
        let contexts = Contexts::from_yaml(&kubeconfig(&server, exec)).unwrap();
        let served = tokio::spawn(serve_version(listener));

        contexts.connect("dev".into()).await.unwrap();

        let request = served.await.unwrap().to_lowercase();
        assert!(
            request.contains("authorization: bearer exec-token"),
            "{request}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn exec_plugin_failure_names_the_context() {
        let exec = r#"
      exec:
        apiVersion: client.authentication.k8s.io/v1
        command: taffrail-missing-exec-plugin
        interactiveMode: Never
"#;
        let contexts = Contexts::from_yaml(&kubeconfig("http://127.0.0.1:1", exec)).unwrap();

        let Err(error) = contexts.connect("dev".into()).await else {
            panic!("a missing exec plugin should not connect");
        };

        assert!(error.to_string().contains("\"dev\""), "{error}");
    }
}
