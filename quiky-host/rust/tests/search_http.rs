#![cfg(feature = "scraping")]

use quiky_host::search::{resolve_game_url, Hub};
use std::io::{Read, Write};
use std::net::TcpListener;

#[test]
fn resolves_first_candidate() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = std::thread::spawn(move || {
        for _ in 0..4 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buf = [0u8; 2048];
            let n = stream.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");
            let ok = path == "/dredge";
            let status = if ok { "200 OK" } else { "404 Not Found" };
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            if ok {
                return;
            }
        }
    });

    let hub = Hub::new("fitgirl", format!("http://{addr}"));
    let client = reqwest::blocking::Client::new();
    let found = resolve_game_url(&client, &hub, "Dredge").unwrap();
    assert_eq!(found, Some(format!("http://{addr}/dredge")));
    server.join().unwrap();
}
