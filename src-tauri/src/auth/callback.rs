//! Loopback listener that receives the browser redirect at the end of sign-in.

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::brand::APP_NAME;

/// How long the user has to finish signing in before the wait gives up.
pub const PRODUCTION_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Longest request line we are willing to read.
const MAX_REQUEST_LINE: u64 = 8 * 1024;
/// Most connections handled at once; further ones wait in the OS backlog.
const MAX_CONNECTIONS: usize = 8;
/// How long a single connection may take to send its request line.
const READ_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, PartialEq, Eq)]
pub enum CallbackError {
    TimedOut,
    Cancelled,
    Denied,
    Io(String),
}

pub struct Listener {
    pub callback_url: String,
    listener: TcpListener,
}

/// Binds an ephemeral port on the loopback interface only.
pub async fn bind() -> std::io::Result<Listener> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    Ok(Listener {
        callback_url: format!("http://localhost:{port}/callback"),
        listener,
    })
}

enum Outcome {
    Code(String),
    Denied,
    Ignored,
}

impl Listener {
    /// Waits for the redirect carrying `expected_state`. Consuming `self`
    /// closes the port on every exit path.
    pub async fn wait_for_code(
        self,
        expected_state: &str,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> Result<String, CallbackError> {
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        // Each connection runs in its own task so a stalled socket cannot
        // delay the real callback. The JoinSet aborts every task when it is
        // dropped, so none outlives the wait.
        let serve = async {
            let mut conns: JoinSet<Outcome> = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = self.listener.accept(), if conns.len() < MAX_CONNECTIONS => {
                        let (stream, _) =
                            accepted.map_err(|e| CallbackError::Io(e.to_string()))?;
                        let state = expected_state.to_owned();
                        conns.spawn(async move { handle(stream, &state).await });
                    }
                    Some(done) = conns.join_next(), if !conns.is_empty() => {
                        match done {
                            Ok(Outcome::Code(code)) => return Ok(code),
                            Ok(Outcome::Denied) => return Err(CallbackError::Denied),
                            Ok(Outcome::Ignored) | Err(_) => {}
                        }
                    }
                }
            }
        };
        tokio::select! {
            _ = cancel.cancelled() => Err(CallbackError::Cancelled),
            _ = &mut deadline => Err(CallbackError::TimedOut),
            r = serve => r,
        }
    }
}

async fn handle(mut stream: TcpStream, expected_state: &str) -> Outcome {
    let line = match tokio::time::timeout(READ_TIMEOUT, read_request_line(&mut stream)).await {
        Ok(Some(line)) => line,
        _ => return Outcome::Ignored,
    };
    let (status, outcome, body) = route(&line, expected_state);
    let _ = write_response(&mut stream, status, body).await;
    outcome
}

async fn read_request_line(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut reader = BufReader::new(stream.take(MAX_REQUEST_LINE));
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line).await.ok()?;
    Some(line)
}

fn route(line: &[u8], expected_state: &str) -> (&'static str, Outcome, String) {
    let bad = || {
        (
            "400 Bad Request",
            Outcome::Ignored,
            page(
                "Sign-in failed",
                "This sign-in link is not valid. Go back to the app and try again.",
            ),
        )
    };
    let not_found = || {
        (
            "404 Not Found",
            Outcome::Ignored,
            page("Not found", "Nothing here."),
        )
    };

    let Ok(line) = std::str::from_utf8(line) else {
        return bad();
    };
    let mut parts = line.split_whitespace();
    let (Some("GET"), Some(target)) = (parts.next(), parts.next()) else {
        return bad();
    };
    let Ok(url) = Url::parse("http://localhost").and_then(|base| base.join(target)) else {
        return bad();
    };
    if url.path() != "/callback" {
        return not_found();
    }
    let mut code = None;
    let mut state = None;
    let mut error = false;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            "error" => error = true,
            _ => {}
        }
    }
    if state.as_deref() != Some(expected_state) {
        return bad();
    }
    if error {
        return (
            "200 OK",
            Outcome::Denied,
            page(
                "Sign-in cancelled",
                "You can close this tab and go back to the app.",
            ),
        );
    }
    match code {
        Some(code) if !code.is_empty() => (
            "200 OK",
            Outcome::Code(code),
            page(
                "You're signed in",
                "You can close this tab and go back to the app.",
            ),
        ),
        _ => bad(),
    }
}

/// Static pages: nothing from the request is ever placed in the HTML.
fn page(title: &str, message: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>{APP_NAME}</title>\
         <style>body{{font-family:system-ui,sans-serif;display:grid;place-items:center;min-height:100vh;margin:0}}\
         main{{text-align:center}}</style></head><body><main><h1>{title}</h1><p>{message}</p></main></body></html>"
    )
}

async fn write_response(stream: &mut TcpStream, status: &str, body: String) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio_util::sync::CancellationToken;

    fn client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }

    const LONG: Duration = Duration::from_secs(10);

    #[tokio::test]
    async fn returns_code_when_state_matches() {
        let l = bind().await.unwrap();
        let url = l.callback_url.clone();
        let wait = tokio::spawn(l.wait_for_code("ST", LONG, CancellationToken::new()));
        let resp = client()
            .get(format!("{url}?code=abc&state=ST"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = resp.text().await.unwrap();
        assert!(body.contains("You can close this tab"));
        assert_eq!(wait.await.unwrap().unwrap(), "abc");
    }

    #[tokio::test]
    async fn ignores_wrong_state_and_keeps_waiting() {
        let l = bind().await.unwrap();
        let url = l.callback_url.clone();
        let wait = tokio::spawn(l.wait_for_code("ST", LONG, CancellationToken::new()));
        let bad = client()
            .get(format!("{url}?code=x&state=BAD"))
            .send()
            .await
            .unwrap();
        assert_eq!(bad.status(), 400);
        assert!(!wait.is_finished());
        client()
            .get(format!("{url}?code=good&state=ST"))
            .send()
            .await
            .unwrap();
        assert_eq!(wait.await.unwrap().unwrap(), "good");
    }

    #[tokio::test]
    async fn ignores_other_paths() {
        let l = bind().await.unwrap();
        let base = l.callback_url.trim_end_matches("/callback").to_string();
        let url = l.callback_url.clone();
        let wait = tokio::spawn(l.wait_for_code("ST", LONG, CancellationToken::new()));
        let resp = client()
            .get(format!("{base}/favicon.ico"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
        assert!(!wait.is_finished());
        client()
            .get(format!("{url}?code=c&state=ST"))
            .send()
            .await
            .unwrap();
        assert_eq!(wait.await.unwrap().unwrap(), "c");
    }

    #[tokio::test]
    async fn error_param_is_denied() {
        let l = bind().await.unwrap();
        let url = l.callback_url.clone();
        let wait = tokio::spawn(l.wait_for_code("ST", LONG, CancellationToken::new()));
        client()
            .get(format!("{url}?error=access_denied&state=ST"))
            .send()
            .await
            .unwrap();
        assert!(matches!(wait.await.unwrap(), Err(CallbackError::Denied)));
    }

    #[tokio::test]
    async fn times_out() {
        let l = bind().await.unwrap();
        let r = l
            .wait_for_code("ST", Duration::from_millis(50), CancellationToken::new())
            .await;
        assert!(matches!(r, Err(CallbackError::TimedOut)));
    }

    #[tokio::test]
    async fn cancel_ends_wait_and_frees_port() {
        let l = bind().await.unwrap();
        let port = l.listener.local_addr().unwrap().port();
        let cancel = CancellationToken::new();
        let wait = tokio::spawn(l.wait_for_code("ST", LONG, cancel.clone()));
        cancel.cancel();
        assert!(matches!(wait.await.unwrap(), Err(CallbackError::Cancelled)));
        tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("port should be free after cancel");
    }

    #[tokio::test]
    async fn survives_garbage_bytes() {
        let l = bind().await.unwrap();
        let port = l.listener.local_addr().unwrap().port();
        let url = l.callback_url.clone();
        let wait = tokio::spawn(l.wait_for_code("ST", LONG, CancellationToken::new()));
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        s.write_all(b"\xff\xfe\r\n\r\n").await.unwrap();
        drop(s);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!wait.is_finished());
        client()
            .get(format!("{url}?code=ok&state=ST"))
            .send()
            .await
            .unwrap();
        assert_eq!(wait.await.unwrap().unwrap(), "ok");
    }

    #[tokio::test]
    async fn idle_connections_do_not_block_the_real_callback() {
        let l = bind().await.unwrap();
        let port = l.listener.local_addr().unwrap().port();
        let url = l.callback_url.clone();
        let wait = tokio::spawn(l.wait_for_code("ST", LONG, CancellationToken::new()));
        let mut idle = Vec::new();
        for _ in 0..3 {
            idle.push(
                tokio::net::TcpStream::connect(("127.0.0.1", port))
                    .await
                    .unwrap(),
            );
        }
        let mut partial = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        partial.write_all(b"GET /callb").await.unwrap();
        let r = tokio::time::timeout(Duration::from_millis(1000), async {
            client()
                .get(format!("{url}?code=fast&state=ST"))
                .send()
                .await
                .unwrap();
            wait.await
        })
        .await
        .expect("callback was starved by idle connections");
        assert_eq!(r.unwrap().unwrap(), "fast");
    }
}
