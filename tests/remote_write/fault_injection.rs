//! Fault injection tests for `RemoteWriteSender::push_once`.
//!
//! These tests do NOT require Docker — they use in-process TCP listeners
//! to simulate various failure modes.

use std::collections::HashMap;
use std::time::Duration;

use distributed_metrics::config::RemoteWriteDestination;
use distributed_metrics::remote_write::{parse_text_to_write_request, RemoteWriteSender};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

fn make_dest(url: &str) -> RemoteWriteDestination {
    RemoteWriteDestination {
        name: "fault-test".to_string(),
        url: url.to_string(),
        username: None,
        password: None,
        headers: HashMap::new(),
        interval: Duration::from_secs(15),
        timeout: Duration::from_secs(5),
    }
}

const VALID_PROM_TEXT: &str = "# TYPE test_gauge gauge\ntest_gauge 42\n";

// ---------------------------------------------------------------------------
// Connection failures
// ---------------------------------------------------------------------------

#[tokio::test]
async fn push_to_refused_port_returns_error() {
    // Arrange — bind and immediately drop to guarantee the port is refused
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind failed");
    let port = listener.local_addr().expect("local_addr failed").port();
    drop(listener);

    let dest = make_dest(&format!("http://127.0.0.1:{}/api/v1/write", port));
    let client = reqwest::Client::new();

    // Act
    let result = RemoteWriteSender::push_once(&dest, &client, VALID_PROM_TEXT).await;

    // Assert
    assert!(result.is_err());
    let err = result.expect_err("expected connection error");
    let msg = err.to_string();
    assert!(
        msg.contains("error sending request") || msg.contains("connection refused") || msg.contains("Connection refused"),
        "unexpected error: {}",
        msg
    );
}

#[tokio::test]
async fn push_to_invalid_hostname_returns_error() {
    // Arrange
    let dest = make_dest("http://this-host-does-not-exist.invalid:9999/api/v1/write");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("client build failed");

    // Act
    let result = RemoteWriteSender::push_once(&dest, &client, VALID_PROM_TEXT).await;

    // Assert
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// HTTP error responses
// ---------------------------------------------------------------------------

async fn start_mock_server(
    status_code: u16,
    body: &str,
) -> (tokio::task::JoinHandle<()>, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind failed");
    let port = listener.local_addr().expect("local_addr failed").port();
    let url = format!("http://127.0.0.1:{}/api/v1/write", port);

    let response_body = body.to_string();
    let handle = tokio::spawn(async move {
        // Accept one connection, read the request, send a canned response
        let (mut stream, _) = listener.accept().await.expect("accept failed");

        // Read until we get the double CRLF ending the headers
        let mut buf = vec![0u8; 8192];
        let mut total = 0;
        loop {
            let n = tokio::io::AsyncReadExt::read(&mut stream, &mut buf[total..])
                .await
                .expect("read failed");
            if n == 0 {
                break;
            }
            total += n;
            if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }

        let response = format!(
            "HTTP/1.1 {} Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            status_code,
            response_body.len(),
            response_body
        );
        stream
            .write_all(response.as_bytes())
            .await
            .expect("write failed");
        stream.shutdown().await.expect("shutdown failed");
    });

    (handle, url)
}

#[tokio::test]
async fn push_to_400_returns_error_with_status() {
    // Arrange
    let (server, url) = start_mock_server(400, "bad request: invalid protobuf").await;
    let dest = make_dest(&url);
    let client = reqwest::Client::new();

    // Act
    let result = RemoteWriteSender::push_once(&dest, &client, VALID_PROM_TEXT).await;
    server.await.expect("server task failed");

    // Assert
    let err = result.expect_err("expected 400 error");
    let msg = err.to_string();
    assert!(msg.contains("400"), "error should contain status code: {}", msg);
    assert!(
        msg.contains("bad request"),
        "error should contain response body: {}",
        msg
    );
}

#[tokio::test]
async fn push_to_500_returns_error() {
    // Arrange
    let (server, url) = start_mock_server(500, "internal server error").await;
    let dest = make_dest(&url);
    let client = reqwest::Client::new();

    // Act
    let result = RemoteWriteSender::push_once(&dest, &client, VALID_PROM_TEXT).await;
    server.await.expect("server task failed");

    // Assert
    let err = result.expect_err("expected 500 error");
    assert!(err.to_string().contains("500"));
}

#[tokio::test]
async fn push_to_429_returns_error() {
    // Arrange — rate limiting
    let (server, url) = start_mock_server(429, "rate limited").await;
    let dest = make_dest(&url);
    let client = reqwest::Client::new();

    // Act
    let result = RemoteWriteSender::push_once(&dest, &client, VALID_PROM_TEXT).await;
    server.await.expect("server task failed");

    // Assert
    let err = result.expect_err("expected 429 error");
    assert!(err.to_string().contains("429"));
}

#[tokio::test]
async fn error_body_truncated_to_1024_bytes() {
    // Arrange — server returns a 5KB error body of repeated 'x'
    let huge_body = "x".repeat(5000);
    let (server, url) = start_mock_server(500, &huge_body).await;
    let dest = make_dest(&url);
    let client = reqwest::Client::new();

    // Act
    let result = RemoteWriteSender::push_once(&dest, &client, VALID_PROM_TEXT).await;
    server.await.expect("server task failed");

    // Assert
    let err = result.expect_err("expected error");
    let msg = err.to_string();
    // The full message is "remote_write returned 500 Error: <body>"
    // The body portion is truncated to 1024 chars, so the total message
    // should be well under 5000 (the original body size).
    assert!(
        msg.len() < 1200,
        "error message should be truncated, got {} bytes",
        msg.len()
    );
    // Count the number of 'x' characters — should be exactly 1024 (truncated from 5000)
    let x_count = msg.chars().filter(|c| *c == 'x').count();
    assert_eq!(
        x_count, 1024,
        "body should be truncated to exactly 1024 chars of 'x', got {}",
        x_count
    );
}

#[tokio::test]
async fn error_body_under_1024_not_truncated() {
    // Arrange — server returns a small error body
    let small_body = "short error message";
    let (server, url) = start_mock_server(500, small_body).await;
    let dest = make_dest(&url);
    let client = reqwest::Client::new();

    // Act
    let result = RemoteWriteSender::push_once(&dest, &client, VALID_PROM_TEXT).await;
    server.await.expect("server task failed");

    // Assert — full body should be preserved
    let err = result.expect_err("expected error");
    let msg = err.to_string();
    assert!(
        msg.contains("short error message"),
        "small error body should not be truncated: {}",
        msg
    );
}

// ---------------------------------------------------------------------------
// Timeout
// ---------------------------------------------------------------------------

#[tokio::test]
async fn push_times_out_on_slow_server() {
    // Arrange — server that accepts connection but never responds
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind failed");
    let port = listener.local_addr().expect("local_addr failed").port();
    let url = format!("http://127.0.0.1:{}/api/v1/write", port);

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept failed");
        // Read the request but never respond
        let mut buf = vec![0u8; 8192];
        let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await;
        // Hold the connection open for longer than the client timeout
        tokio::time::sleep(Duration::from_secs(30)).await;
    });

    let mut dest = make_dest(&url);
    dest.timeout = Duration::from_secs(1); // Short timeout
    let client = reqwest::Client::new();

    // Act
    let start = std::time::Instant::now();
    let result = RemoteWriteSender::push_once(&dest, &client, VALID_PROM_TEXT).await;
    let elapsed = start.elapsed();

    // Assert
    assert!(result.is_err(), "should have timed out");
    assert!(
        elapsed < Duration::from_secs(5),
        "should have timed out in ~1s, took {:?}",
        elapsed
    );

    server.abort();
}

// ---------------------------------------------------------------------------
// Invalid input
// ---------------------------------------------------------------------------

#[tokio::test]
async fn push_empty_text_is_noop() {
    // Arrange — parse_text_to_write_request("") returns empty WriteRequest
    // which still encodes fine — it's a valid (empty) protobuf
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind failed");
    let port = listener.local_addr().expect("local_addr failed").port();

    // Server that accepts and returns 204
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept failed");
        let mut buf = vec![0u8; 8192];
        let mut total = 0;
        loop {
            let n = tokio::io::AsyncReadExt::read(&mut stream, &mut buf[total..])
                .await
                .expect("read failed");
            if n == 0 { break; }
            total += n;
            if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") { break; }
        }
        let response = "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n";
        stream.write_all(response.as_bytes()).await.expect("write failed");
        stream.shutdown().await.expect("shutdown failed");
    });

    let dest = make_dest(&format!("http://127.0.0.1:{}/api/v1/write", port));
    let client = reqwest::Client::new();

    // Act
    let result = RemoteWriteSender::push_once(&dest, &client, "").await;
    server.await.expect("server task failed");

    // Assert — empty text produces empty WriteRequest, server returns 204 = success
    assert!(result.is_ok());
}

#[tokio::test]
async fn parse_garbage_input_returns_empty() {
    // Arrange — completely invalid prometheus text
    let text = "this is not prometheus format at all!!!";

    // Act
    let result = parse_text_to_write_request(text);

    // Assert — prometheus-parse doesn't error on unrecognized lines, just ignores them
    assert!(result.is_ok());
    assert!(result.expect("parse failed").timeseries.is_empty());
}

#[tokio::test]
async fn parse_partial_line_does_not_panic() {
    // Arrange — truncated metric line
    let text = "# TYPE g gauge\ng{foo=";

    // Act
    let result = parse_text_to_write_request(text);

    // Assert — should either parse successfully (skipping bad line) or return error, not panic
    // We don't care which — just that it doesn't crash
    let _ = result;
}

#[tokio::test]
async fn parse_binary_garbage_does_not_panic() {
    // Arrange — random bytes
    let text = std::str::from_utf8(&[0x00, 0x01, 0x02, 0x7f, 0x20, 0x0a]).unwrap_or("");

    // Act
    let result = parse_text_to_write_request(text);

    // Assert — should not panic
    let _ = result;
}

// ---------------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------------

#[tokio::test]
async fn push_with_basic_auth_sends_authorization_header() {
    // Arrange — mock server that checks for Authorization header
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind failed");
    let port = listener.local_addr().expect("local_addr failed").port();
    let url = format!("http://127.0.0.1:{}/api/v1/write", port);

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept failed");
        let mut buf = vec![0u8; 8192];
        let mut total = 0;
        loop {
            let n = tokio::io::AsyncReadExt::read(&mut stream, &mut buf[total..])
                .await
                .expect("read failed");
            if n == 0 { break; }
            total += n;
            if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") { break; }
        }

        let request = String::from_utf8_lossy(&buf[..total]).to_lowercase();
        let has_auth = request.contains("authorization: basic");

        let (status, body) = if has_auth {
            ("204 No Content", "")
        } else {
            ("401 Unauthorized", "missing auth")
        };

        let response = format!(
            "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            status,
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).await.expect("write failed");
        stream.shutdown().await.expect("shutdown failed");
    });

    let mut dest = make_dest(&url);
    dest.username = Some("testuser".to_string());
    dest.password = Some("testpass".to_string());
    let client = reqwest::Client::new();

    // Act
    let result = RemoteWriteSender::push_once(&dest, &client, VALID_PROM_TEXT).await;
    server.await.expect("server task failed");

    // Assert
    assert!(result.is_ok(), "push with valid auth should succeed");
}

#[tokio::test]
async fn push_without_auth_to_auth_required_server_returns_401() {
    // Arrange
    let (server, url) = start_mock_server(401, "unauthorized").await;
    let dest = make_dest(&url); // no username/password
    let client = reqwest::Client::new();

    // Act
    let result = RemoteWriteSender::push_once(&dest, &client, VALID_PROM_TEXT).await;
    server.await.expect("server task failed");

    // Assert
    let err = result.expect_err("expected 401 error");
    assert!(err.to_string().contains("401"));
}

// ---------------------------------------------------------------------------
// Custom headers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn push_sends_custom_headers() {
    // Arrange — mock server that checks for custom header
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind failed");
    let port = listener.local_addr().expect("local_addr failed").port();
    let url = format!("http://127.0.0.1:{}/api/v1/write", port);

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept failed");
        let mut buf = vec![0u8; 8192];
        let mut total = 0;
        loop {
            let n = tokio::io::AsyncReadExt::read(&mut stream, &mut buf[total..])
                .await
                .expect("read failed");
            if n == 0 { break; }
            total += n;
            if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") { break; }
        }

        let request = String::from_utf8_lossy(&buf[..total]).to_lowercase();
        let has_custom = request.contains("x-custom-token: secret123");

        let (status, body) = if has_custom {
            ("204 No Content", "")
        } else {
            ("403 Forbidden", "missing custom header")
        };

        let response = format!(
            "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            status,
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).await.expect("write failed");
        stream.shutdown().await.expect("shutdown failed");
    });

    let mut dest = make_dest(&url);
    dest.headers
        .insert("x-custom-token".to_string(), "secret123".to_string());
    let client = reqwest::Client::new();

    // Act
    let result = RemoteWriteSender::push_once(&dest, &client, VALID_PROM_TEXT).await;
    server.await.expect("server task failed");

    // Assert
    assert!(result.is_ok(), "push with custom header should succeed");
}

// ---------------------------------------------------------------------------
// Server drops connection
// ---------------------------------------------------------------------------

#[tokio::test]
async fn push_to_server_that_closes_immediately_returns_error() {
    // Arrange — server accepts then immediately closes
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind failed");
    let port = listener.local_addr().expect("local_addr failed").port();
    let url = format!("http://127.0.0.1:{}/api/v1/write", port);

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept failed");
        drop(stream); // close immediately
    });

    let dest = make_dest(&url);
    let client = reqwest::Client::new();

    // Act
    let result = RemoteWriteSender::push_once(&dest, &client, VALID_PROM_TEXT).await;
    server.await.expect("server task failed");

    // Assert
    assert!(result.is_err(), "should error when server drops connection");
}
