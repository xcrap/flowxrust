//! The app's end of the bridge.
//!
//! A socket in a directory of its own, made fresh for each launch:
//!
//! - **The directory is private.** `mkdtemp` makes it `0700` with a random
//!   name, under the user's own temporary directory, and it is checked to be
//!   the user's and `0700` before anything is bound in it. Nobody else can
//!   reach the socket at all; the socket itself is `0600` besides.
//! - **Not under the application-support directory**, because a Unix socket's
//!   path has to fit in `sun_path`, 104 bytes on macOS, and
//!   `~/Library/Application Support/com.flowx.FlowX/FlowX-Dev/…` leaves a
//!   long user name no room. A fresh directory per launch also means two
//!   running instances never meet, and nothing stale is left to clean.
//! - **A caller proves which thread it speaks for** with that thread's token
//!   (see [`crate::token`]), and its peer credentials must be this user's.
//!   Anything else is refused before the app is asked.
//! - **The app is asked off its UI thread.** Connections are served on a
//!   thread of their own; the handler decides how to reach app state.
//! - **Quit removes it.** [`Bridge::shutdown`] deletes the socket and its
//!   directory and stops accepting; dropping the bridge does the same.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::lines::{Line, read_line};
use crate::token::Secret;
use crate::tools::{CallError, ToolCall};
use crate::wire::{MAX_REQUEST_BYTES, Reply, Request};

/// The longest socket path `sun_path` holds on macOS, its NUL aside.
pub const MAX_SOCKET_PATH: usize = 103;
/// How many calls are served at once. More are closed unanswered.
pub const MAX_CONNECTIONS: usize = 16;
/// How long a caller has to send its request.
pub const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the app has to answer one.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(20);

/// The socket's name inside its directory.
const SOCKET_NAME: &str = "flowx.sock";

/// A request that proved its thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub thread: uuid::Uuid,
    pub call: ToolCall,
}

/// The answer to a call, from wherever the app finds it.
pub type Answer = Pin<Box<dyn Future<Output = Reply> + Send>>;

/// What the app does with a call. Called on the bridge's own thread.
pub type Handler = Arc<dyn Fn(Call) -> Answer + Send + Sync>;

/// A [`Handler`] from an async function of a call.
pub fn handler<F, Fut>(answer: F) -> Handler
where
    F: Fn(Call) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |call| Box::pin(answer(call)) as Answer)
}

pub const WRONG_TOKEN: &str = "FlowX refused this request: its token does not match the \
    thread it names. These tools work only from the thread FlowX started them for, and \
    only until FlowX quits.";

/// A listening bridge.
pub struct Bridge {
    directory: PathBuf,
    socket: PathBuf,
    secret: Arc<Secret>,
    stop: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl std::fmt::Debug for Bridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bridge")
            .field("socket", &self.socket)
            .finish_non_exhaustive()
    }
}

impl Bridge {
    /// Listen in a fresh private directory under `parent`, answering with
    /// `handler`.
    pub fn start(parent: &Path, handler: Handler) -> std::io::Result<Self> {
        let directory = private_directory(parent)?;
        let socket = directory.join(SOCKET_NAME);
        let bound = bind(&socket);
        let listener = match bound {
            Ok(listener) => listener,
            Err(error) => {
                let _ = std::fs::remove_dir(&directory);
                return Err(error);
            }
        };
        let secret = Arc::new(Secret::generate()?);
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let serving = secret.clone();
        std::thread::Builder::new()
            .name("flowx-bridge".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        tracing::error!(%error, "the FlowX bridge could not start");
                        return;
                    }
                };
                runtime.block_on(accept(listener, serving, handler, stopped));
            })?;
        Ok(Self {
            directory,
            socket,
            secret,
            stop: Mutex::new(Some(stop)),
        })
    }

    /// Where the server connects.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// The token a thread's server presents.
    pub fn token_for(&self, thread: &str) -> String {
        self.secret.token_for(thread)
    }

    /// Stop accepting and remove the socket and its directory. Idempotent.
    pub fn shutdown(&self) {
        if let Some(stop) = self
            .stop
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = stop.send(());
        }
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_dir(&self.directory);
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// A new directory only this user can enter.
fn private_directory(parent: &Path) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let directory = tempfile::Builder::new()
        .prefix("flowx-bridge-")
        .tempdir_in(parent)?
        .keep();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    let metadata = std::fs::symlink_metadata(&directory)?;
    // SAFETY: `geteuid` has no preconditions and cannot fail.
    let me = unsafe { libc::geteuid() };
    if !metadata.is_dir() || metadata.uid() != me || metadata.mode() & 0o077 != 0 {
        let _ = std::fs::remove_dir(&directory);
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{} is not a private directory", directory.display()),
        ));
    }
    Ok(directory)
}

fn bind(socket: &Path) -> std::io::Result<std::os::unix::net::UnixListener> {
    use std::os::unix::fs::PermissionsExt;
    if socket.as_os_str().len() > MAX_SOCKET_PATH {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "{} is longer than a socket path may be ({MAX_SOCKET_PATH} bytes)",
                socket.display()
            ),
        ));
    }
    let listener = std::os::unix::net::UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

async fn accept(
    listener: std::os::unix::net::UnixListener,
    secret: Arc<Secret>,
    handler: Handler,
    mut stopped: tokio::sync::oneshot::Receiver<()>,
) {
    let listener = match tokio::net::UnixListener::from_std(listener) {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!(%error, "the FlowX bridge could not listen");
            return;
        }
    };
    let permits = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
    loop {
        tokio::select! {
            _ = &mut stopped => return,
            accepted = listener.accept() => {
                let stream = match accepted {
                    Ok((stream, _)) => stream,
                    Err(error) => {
                        // Out of descriptors, most likely: wait rather than spin.
                        tracing::warn!(%error, "the FlowX bridge could not accept a call");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let Ok(permit) = permits.clone().try_acquire_owned() else {
                    continue;
                };
                let secret = secret.clone();
                let handler = handler.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    serve(stream, &secret, &handler).await;
                });
            }
        }
    }
}

/// Answer one connection's one call.
async fn serve(stream: UnixStream, secret: &Secret, handler: &Handler) {
    let same_user = stream
        .peer_cred()
        // SAFETY: `geteuid` has no preconditions and cannot fail.
        .is_ok_and(|peer| peer.uid() == unsafe { libc::geteuid() });
    let (read, mut write) = stream.into_split();
    let reply = if same_user {
        respond(BufReader::new(read), secret, handler).await
    } else {
        Some(Reply::refused("FlowX answers only its own user."))
    };
    if let Some(reply) = reply {
        let _ = write.write_all(&reply.to_line()).await;
    }
    let _ = write.shutdown().await;
}

async fn respond(
    mut reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    secret: &Secret,
    handler: &Handler,
) -> Option<Reply> {
    let line = tokio::time::timeout(READ_TIMEOUT, read_line(&mut reader, MAX_REQUEST_BYTES))
        .await
        .ok()?
        .ok()?;
    let request = match line {
        Line::Data(bytes) => match serde_json::from_slice::<Request>(&bytes) {
            Ok(request) => request,
            Err(_) => return Some(Reply::refused("FlowX could not read this request.")),
        },
        Line::TooLong => return Some(Reply::refused("This request is too large.")),
        Line::End => return None,
    };
    // The token first: nothing about a request is looked at, or answered
    // differently, until it is known to come from the thread it names.
    if !secret.verify(&request.thread, &request.token) {
        return Some(Reply::refused(WRONG_TOKEN));
    }
    let Ok(thread) = uuid::Uuid::parse_str(&request.thread) else {
        return Some(Reply::refused(WRONG_TOKEN));
    };
    let call = match ToolCall::parse(&request.tool, &request.arguments) {
        Ok(call) => call,
        Err(CallError::UnknownTool(name)) => {
            return Some(Reply::refused(format!("FlowX has no tool named {name}.")));
        }
        Err(CallError::InvalidArguments(why)) => return Some(Reply::refused(why)),
    };
    Some(
        tokio::time::timeout(ANSWER_TIMEOUT, handler(Call { thread, call }))
            .await
            .unwrap_or_else(|_| {
                Reply::refused(
                    "FlowX did not answer in time. It may be busy; try again in a moment.",
                )
            }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::io::AsyncReadExt;

    /// A bridge whose app answers every call by naming it, and counts them.
    fn bridge(parent: &Path) -> (Bridge, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let answer = handler(move |call: Call| {
            counted.fetch_add(1, Ordering::SeqCst);
            async move { Reply::ok(format!("{} for {}", call.call.name(), call.thread)) }
        });
        (Bridge::start(parent, answer).unwrap(), calls)
    }

    async fn send(socket: &Path, line: &[u8]) -> Option<Reply> {
        let mut stream = UnixStream::connect(socket).await.unwrap();
        stream.write_all(line).await.unwrap();
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).await.unwrap();
        if answer.is_empty() {
            return None;
        }
        Some(serde_json::from_slice(&answer[..answer.len() - 1]).unwrap())
    }

    fn request(thread: &str, token: String, tool: &str) -> Vec<u8> {
        Request {
            thread: thread.into(),
            token,
            tool: tool.into(),
            arguments: serde_json::json!({}),
        }
        .to_line()
    }

    #[tokio::test]
    async fn the_socket_is_alone_in_a_private_directory() {
        let parent = tempfile::tempdir().unwrap();
        let (bridge, _) = bridge(parent.path());
        let directory = bridge.socket().parent().unwrap();
        assert!(directory.starts_with(parent.path()));
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(directory), 0o700);
        assert_eq!(mode(bridge.socket()), 0o600);
        assert!(bridge.socket().as_os_str().len() <= MAX_SOCKET_PATH);
    }

    #[tokio::test]
    async fn a_call_with_its_thread_s_token_reaches_the_app() {
        let parent = tempfile::tempdir().unwrap();
        let (bridge, calls) = bridge(parent.path());
        let thread = uuid::Uuid::new_v4().to_string();
        let reply = send(
            bridge.socket(),
            &request(&thread, bridge.token_for(&thread), "list_threads"),
        )
        .await
        .unwrap();
        assert_eq!(reply, Reply::ok(format!("list_threads for {thread}")));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_wrong_token_is_refused_before_the_app_is_asked() {
        let parent = tempfile::tempdir().unwrap();
        let (bridge, calls) = bridge(parent.path());
        let (mine, theirs) = (
            uuid::Uuid::new_v4().to_string(),
            uuid::Uuid::new_v4().to_string(),
        );
        for line in [
            // Made up.
            request(&mine, "0".repeat(64), "list_threads"),
            // Another thread's: it cannot speak for this one.
            request(&mine, bridge.token_for(&theirs), "list_threads"),
            // Right, but for an unknown tool: still nothing reaches the app.
            request(&mine, String::new(), "nothing"),
        ] {
            let reply = send(bridge.socket(), &line).await.unwrap();
            assert!(!reply.ok);
            assert_eq!(reply.text, WRONG_TOKEN);
        }
        // Another launch's token for the very same thread.
        let other_parent = tempfile::tempdir().unwrap();
        let (other, _) = self::bridge(other_parent.path());
        let reply = send(
            bridge.socket(),
            &request(&mine, other.token_for(&mine), "list_threads"),
        )
        .await
        .unwrap();
        assert_eq!(reply.text, WRONG_TOKEN);
        assert_eq!(calls.load(Ordering::SeqCst), 0, "the app was asked");
    }

    #[tokio::test]
    async fn garbage_and_oversized_requests_are_refused_without_the_app() {
        let parent = tempfile::tempdir().unwrap();
        let (bridge, calls) = bridge(parent.path());
        let reply = send(bridge.socket(), b"hello\n").await.unwrap();
        assert!(!reply.ok);
        let mut huge = vec![b'x'; MAX_REQUEST_BYTES + 10];
        huge.push(b'\n');
        let reply = send(bridge.socket(), &huge).await.unwrap();
        assert_eq!(reply, Reply::refused("This request is too large."));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn an_authenticated_call_with_bad_arguments_is_refused_by_the_bridge() {
        let parent = tempfile::tempdir().unwrap();
        let (bridge, calls) = bridge(parent.path());
        let thread = uuid::Uuid::new_v4().to_string();
        let reply = send(
            bridge.socket(),
            &request(&thread, bridge.token_for(&thread), "read_thread"),
        )
        .await
        .unwrap();
        assert!(!reply.ok);
        assert!(reply.text.contains("thread_id"), "{}", reply.text);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn shutting_down_removes_the_socket_and_its_directory() {
        let parent = tempfile::tempdir().unwrap();
        let (bridge, _) = bridge(parent.path());
        let socket = bridge.socket().to_path_buf();
        let directory = socket.parent().unwrap().to_path_buf();
        bridge.shutdown();
        assert!(!socket.exists());
        assert!(!directory.exists());
        let refused = UnixStream::connect(&socket).await;
        assert!(refused.is_err());
        // Again, and on drop: nothing left to remove is not an error.
        bridge.shutdown();
        drop(bridge);
    }

    #[test]
    fn a_socket_path_too_long_for_the_platform_is_refused() {
        let parent = tempfile::tempdir().unwrap();
        let deep = parent.path().join("d".repeat(120));
        std::fs::create_dir_all(&deep).unwrap();
        let error = Bridge::start(&deep, handler(|_| async { Reply::ok("") })).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        // The directory made for it is gone again.
        assert_eq!(std::fs::read_dir(&deep).unwrap().count(), 0);
    }
}
