//! A tiny std-only HTTP/1.1 server for tests (feature `test-util`).
//!
//! Responses are matched by method and exact path (including the query
//! string); unmatched requests get a JSON 404. Every request is recorded.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

/// A canned response.
#[derive(Debug, Clone)]
pub struct MockResponse {
    method: String,
    path: String,
    status: u16,
    content_type: String,
    body: Vec<u8>,
}

impl MockResponse {
    /// JSON response for `method path`.
    pub fn json(method: &str, path: &str, status: u16, body: &str) -> Self {
        Self::with_type(method, path, status, "application/json", body.as_bytes())
    }

    /// Plain text response for `method path`.
    pub fn text(method: &str, path: &str, status: u16, body: &str) -> Self {
        Self::with_type(method, path, status, "text/plain", body.as_bytes())
    }

    /// Response with an explicit content type.
    pub fn with_type(
        method: &str,
        path: &str,
        status: u16,
        content_type: &str,
        body: &[u8],
    ) -> Self {
        Self {
            method: method.to_string(),
            path: path.to_string(),
            status,
            content_type: content_type.to_string(),
            body: body.to_vec(),
        }
    }
}

/// A recorded request.
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    /// HTTP method.
    pub method: String,
    /// Path including query string.
    pub path: String,
    /// Headers (names lowercased).
    pub headers: Vec<(String, String)>,
    /// Body bytes.
    pub body: Vec<u8>,
}

impl RecordedRequest {
    /// First header value with this (lowercase) name.
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    }
}

/// A running mock server; stops accepting when dropped (the thread is detached).
pub struct MockServer {
    port: u16,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

impl MockServer {
    /// Start serving `responses` on an ephemeral localhost port.
    pub fn start(responses: Vec<MockResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let port = listener.local_addr().expect("local addr").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                handle(stream, &responses, &recorded);
            }
        });
        Self { port, requests }
    }

    /// Base URL, e.g. `http://127.0.0.1:41234`.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Requests received so far.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("lock").clone()
    }
}

fn handle(
    mut stream: TcpStream,
    responses: &[MockResponse],
    recorded: &Mutex<Vec<RecordedRequest>>,
) -> Option<()> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        reader.read_line(&mut h).ok()?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    let len = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; len];
    reader.read_exact(&mut body).ok()?;

    let (status, ctype, out) = match responses
        .iter()
        .find(|r| r.method == method && r.path == path)
    {
        Some(r) => (r.status, r.content_type.as_str(), r.body.clone()),
        None => (
            404,
            "application/json",
            br#"{"error":{"code":"not_found","message":"no mock for this route"}}"#.to_vec(),
        ),
    };
    // Record before answering, so a test that asserts right after the call sees it.
    recorded.lock().expect("lock").push(RecordedRequest {
        method,
        path,
        headers,
        body,
    });
    let head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        out.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&out);
    let _ = stream.flush();
    Some(())
}
