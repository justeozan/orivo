#![cfg(feature = "scraping")]

use quiky_host::download::download_http;
use std::io::{Read, Write};
use std::net::TcpListener;

#[test]
fn downloads_bytes_and_reports_progress() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let payload = b"hello quiky".to_vec();
    let body = payload.clone();

    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request).unwrap();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(&body).unwrap();
    });

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("file.bin");
    let client = reqwest::blocking::Client::new();
    let mut seen = Vec::new();
    let written = download_http(
        &client,
        &format!("http://{addr}/file.bin"),
        &dest,
        None,
        |done, total| seen.push((done, total)),
    )
    .unwrap();

    server.join().unwrap();
    assert_eq!(written, payload.len() as u64);
    assert_eq!(std::fs::read(&dest).unwrap(), payload);
    assert!(seen
        .iter()
        .any(|(done, total)| *done == written && *total == written));
}
