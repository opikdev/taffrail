//! The login shell environment, for an app started outside a terminal.
//!
//! The Dock, Finder and Linux desktop launchers start apps with a minimal `PATH` and no
//! `KUBECONFIG`, so exec auth plugins are not found and the user's kubeconfig is ignored.
//! On Windows, the Start menu passes the user's environment from the registry, so nothing
//! is read there.
//!
//! The process environment is never changed. [`crate::Contexts`] reads `KUBECONFIG` from
//! here and passes the other variables to exec auth plugins. Values can hold secrets, so
//! nothing here logs or prints them.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::OnceLock;

static LOGIN_SHELL: OnceLock<ShellEnvironment> = OnceLock::new();

/// Variables an exec auth plugin needs, or that choose the kubeconfig.
#[cfg(any(unix, test))]
const NAMES: &[&str] = &[
    // Finds the plugin, and the tools it calls.
    "PATH",
    // Lists the kubeconfig files to read.
    "KUBECONFIG",
    // Locates the plugin's credential files.
    "HOME",
    // Lets a plugin reach its token endpoint through a proxy.
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
];

/// Prefixes of the variables the common cloud plugins read, such as a profile or region.
#[cfg(any(unix, test))]
const PREFIXES: &[&str] = &[
    // `aws eks get-token` and `aws-iam-authenticator`.
    "AWS_",
    // `kubelogin` for Azure.
    "AZURE_",
    "AAD_",
    // `gke-gcloud-auth-plugin` and `gcloud`.
    "CLOUDSDK_",
    "GOOGLE_",
    // `doctl`.
    "DIGITALOCEAN_",
    // `oci ce cluster generate-token`.
    "OCI_",
];

/// The variables from [`NAMES`] and [`PREFIXES`] that a login shell set.
#[derive(Clone, Default)]
pub(crate) struct ShellEnvironment {
    variables: BTreeMap<String, String>,
}

impl ShellEnvironment {
    /// Keeps only the variables that kubeconfig loading and exec plugins need.
    #[cfg(any(unix, test))]
    pub(crate) fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let variables = pairs
            .into_iter()
            .filter(|(name, _)| {
                NAMES.contains(name) || PREFIXES.iter().any(|prefix| name.starts_with(prefix))
            })
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect();
        Self { variables }
    }

    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        self.variables.get(name).map(String::as_str)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.variables
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }
}

/// Shows names only, because values can hold secrets.
impl fmt::Debug for ShellEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_set()
            .entries(self.variables.keys())
            .finish()
    }
}

/// Reads the login shell environment if the app was started outside a terminal.
///
/// Call it once, before loading any kubeconfig. It waits a few seconds at most, and
/// keeps the current environment if the shell fails or is too slow.
pub fn use_login_shell_environment() {
    LOGIN_SHELL.get_or_init(|| read_outside_terminal().unwrap_or_default());
}

/// The environment read by [`use_login_shell_environment`], or an empty one.
pub(crate) fn login_shell() -> &'static ShellEnvironment {
    LOGIN_SHELL.get_or_init(ShellEnvironment::default)
}

#[cfg(unix)]
fn read_outside_terminal() -> Option<ShellEnvironment> {
    use std::io::IsTerminal;

    if std::io::stdin().is_terminal() {
        return None;
    }
    let shell = std::env::var_os("SHELL").filter(|shell| !shell.is_empty())?;
    shell::read(shell::login_command(shell.as_ref()), shell::TIMEOUT).ok()
}

#[cfg(not(unix))]
fn read_outside_terminal() -> Option<ShellEnvironment> {
    None
}

#[cfg(unix)]
mod shell {
    use std::io::{self, Read};
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::ShellEnvironment;

    /// Long enough for a slow profile, short enough that a broken one barely delays start-up.
    pub(super) const TIMEOUT: Duration = Duration::from_secs(5);
    /// Surrounds the environment, because an interactive shell may print a greeting.
    const MARKER: &str = "_TAFFRAIL_ENVIRONMENT_7C1E_";
    const MAX_OUTPUT: usize = 1 << 20;

    /// Prints the environment between two markers, separated by NUL bytes.
    pub(super) fn script() -> String {
        format!("printf %s {MARKER}; /usr/bin/env -0; printf %s {MARKER}")
    }

    /// Runs the shell as both login and interactive, like a new terminal tab, so it reads
    /// every profile file that might set `PATH`.
    pub(super) fn login_command(shell: &Path) -> Command {
        let mut command = Command::new(shell);
        command.args(["-l", "-i", "-c", &script()]);
        if let Some(home) = dirs::home_dir() {
            command.current_dir(home);
        }
        command
    }

    pub(super) fn read(mut command: Command, timeout: Duration) -> io::Result<ShellEnvironment> {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut stdout = child.stdout.take().expect("stdout is piped");
        let (sender, receiver) = mpsc::channel();
        // A process the profile starts in the background can hold stdout open forever, so
        // the reader stops at the second marker and is abandoned on timeout.
        thread::Builder::new()
            .name("login shell".into())
            .spawn(move || sender.send(read_output(&mut stdout)))?;
        let output = receiver.recv_timeout(timeout);
        let _ = child.kill();
        let _ = child.wait();
        let output = output.map_err(|_| {
            io::Error::new(io::ErrorKind::TimedOut, "the login shell did not finish")
        })??;
        parse(&output).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "the login shell printed no environment",
            )
        })
    }

    fn read_output(stdout: &mut impl Read) -> io::Result<Vec<u8>> {
        let mut output = Vec::new();
        let mut chunk = [0; 8192];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) => return Ok(output),
                Ok(read) => output.extend_from_slice(&chunk[..read]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
            if between_markers(&output).is_some() || output.len() > MAX_OUTPUT {
                return Ok(output);
            }
        }
    }

    fn between_markers(output: &[u8]) -> Option<&[u8]> {
        let marker = MARKER.as_bytes();
        let start = find(output, marker)? + marker.len();
        let length = find(&output[start..], marker)?;
        Some(&output[start..start + length])
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    /// Parses `NAME=value` entries separated by NUL bytes, skipping any that are not UTF-8.
    pub(super) fn parse(output: &[u8]) -> Option<ShellEnvironment> {
        let entries = between_markers(output)?;
        let pairs = entries.split(|byte| *byte == 0).filter_map(|entry| {
            let (name, value) = std::str::from_utf8(entry).ok()?.split_once('=')?;
            (!name.is_empty()).then_some((name, value))
        });
        Some(ShellEnvironment::from_pairs(pairs))
    }

    #[cfg(test)]
    mod tests {
        use std::time::Instant;

        use super::*;

        fn sh(script: &str) -> Command {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", script]);
            command
        }

        fn names(environment: &ShellEnvironment) -> Vec<&str> {
            environment.iter().map(|(name, _)| name).collect()
        }

        #[test]
        fn parses_the_environment_between_the_markers() {
            let output = format!(
                "Welcome back\n{MARKER}PATH=/opt/bin:/usr/bin\0EDITOR=vi\0AWS_PROFILE=dev\0GOOGLE_NOTE=a=b\nc\0=bad\0{MARKER}\nlogout\n"
            );

            let environment = parse(output.as_bytes()).unwrap();

            assert_eq!(names(&environment), ["AWS_PROFILE", "GOOGLE_NOTE", "PATH"]);
            assert_eq!(environment.get("PATH"), Some("/opt/bin:/usr/bin"));
            assert_eq!(environment.get("GOOGLE_NOTE"), Some("a=b\nc"));
        }

        #[test]
        fn skips_entries_that_are_not_utf8() {
            let mut output = format!("{MARKER}PATH=/usr/bin\0AWS_PROFILE=").into_bytes();
            output.extend_from_slice(&[0xff, 0]);
            output.extend_from_slice(MARKER.as_bytes());

            let environment = parse(&output).unwrap();

            assert_eq!(names(&environment), ["PATH"]);
        }

        #[test]
        fn output_without_both_markers_is_not_an_environment() {
            assert!(parse(b"PATH=/usr/bin\0").is_none());
            assert!(parse(format!("{MARKER}PATH=/usr/bin\0").as_bytes()).is_none());
        }

        #[test]
        fn runs_the_shell_as_login_and_interactive() {
            let command = login_command(Path::new("/bin/zsh"));

            let args: Vec<_> = command.get_args().collect();

            assert_eq!(command.get_program(), "/bin/zsh");
            assert_eq!(args[..3], ["-l", "-i", "-c"]);
            assert_eq!(args[3], script().as_str());
        }

        #[test]
        fn reads_the_environment_a_shell_prints() {
            let environment = read(sh(&format!("echo hello; {}", script())), TIMEOUT).unwrap();

            assert_eq!(
                environment.get("PATH"),
                std::env::var("PATH").ok().as_deref()
            );
        }

        #[test]
        fn stops_reading_at_the_marker_while_a_background_process_holds_the_output() {
            let started = Instant::now();

            let environment = read(sh(&format!("sleep 5 & {}", script())), TIMEOUT).unwrap();

            assert!(environment.get("PATH").is_some());
            assert!(started.elapsed() < Duration::from_secs(4));
        }

        #[test]
        fn gives_up_on_a_shell_that_hangs() {
            let started = Instant::now();

            let error = read(sh("sleep 30"), Duration::from_millis(200)).unwrap_err();

            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
            assert!(started.elapsed() < Duration::from_secs(4));
        }

        #[test]
        fn fails_when_the_shell_prints_no_environment() {
            let error = read(sh("echo broken profile; exit 1"), TIMEOUT).unwrap_err();

            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        }

        #[test]
        fn fails_when_the_shell_does_not_exist() {
            let command = login_command(Path::new("/nonexistent/taffrail-shell"));

            assert!(read(command, TIMEOUT).is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ShellEnvironment;

    #[test]
    fn keeps_only_the_variables_plugins_need() {
        let environment = ShellEnvironment::from_pairs([
            ("PATH", "/usr/bin"),
            ("KUBECONFIG", "/k"),
            ("AWS_REGION", "eu-west-1"),
            ("AZURE_CONFIG_DIR", "/a"),
            ("CLOUDSDK_CONFIG", "/c"),
            ("GITHUB_TOKEN", "secret"),
            ("PS1", "$ "),
            ("NOT_AWS_", "x"),
        ]);

        let names: Vec<_> = environment.iter().map(|(name, _)| name).collect();

        assert_eq!(
            names,
            [
                "AWS_REGION",
                "AZURE_CONFIG_DIR",
                "CLOUDSDK_CONFIG",
                "KUBECONFIG",
                "PATH"
            ]
        );
    }
}
