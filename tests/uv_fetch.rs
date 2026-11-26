//! zem downloads, verifies and installs its own pinned uv. These tests serve a fake release
//! from a local HTTP server, so they need no network.
#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::thread;

use zem::ZemError;
use zem::home::ZemHome;
use zem::home::keys::sha256_hex;
use zem::store::uv::{UV_VERSION, UvRelease, UvStore};

const TARGET: &str = "test-target";

/// A `uv-<target>.tar.gz` holding `uv-<target>/<file>`.
fn archive(file: &str) -> Vec<u8> {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(gz);
    let body = b"#!/bin/sh\necho uv\n";
    let mut header = tar::Header::new_gnu();
    header.set_size(body.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    builder
        .append_data(&mut header, format!("uv-{TARGET}/{file}"), &body[..])
        .unwrap();
    builder.into_inner().unwrap().finish().unwrap()
}

fn sha256(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}

/// Serve `body` for `/<version>/uv-<target>.tar.gz` to the next `connections` requests and
/// answer anything else with 404. Returns the base URL.
fn serve(body: Vec<u8>, connections: usize) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let expected = format!("GET /{UV_VERSION}/uv-{TARGET}.tar.gz ");
    thread::spawn(move || {
        for _ in 0..connections {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            let ok = String::from_utf8_lossy(&request).starts_with(&expected);
            let response = if ok {
                let mut head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .into_bytes();
                head.extend_from_slice(&body);
                head
            } else {
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
            };
            let _ = stream.write_all(&response);
        }
    });
    base
}

fn release(base_url: String, sha256: String) -> UvRelease {
    UvRelease {
        version: UV_VERSION.to_string(),
        target: TARGET.to_string(),
        sha256,
        base_url,
    }
}

#[test]
fn downloads_verifies_and_reuses_uv() {
    let tmp = tempfile::tempdir().unwrap();
    let home = ZemHome::new(tmp.path().to_path_buf());
    let body = archive("uv");
    // One connection only: the second `ensure` must not touch the network.
    let base = serve(body.clone(), 1);
    let store = UvStore {
        home: &home,
        release: release(base, sha256(&body)),
    };
    assert!(!store.installed());

    let first = store.ensure().unwrap();
    assert!(first.fetched);
    assert_eq!(first.version, UV_VERSION);
    assert_eq!(first.path, store.binary());
    assert!(first.path.starts_with(home.tools_dir()));
    let mode = std::fs::metadata(&first.path).unwrap().permissions().mode();
    assert_eq!(mode & 0o111, 0o111, "uv must be executable");
    assert!(store.installed());

    let second = store.ensure().unwrap();
    assert!(!second.fetched);
    assert_eq!(second.path, first.path);

    // Neither the staging directory nor the downloaded archive is left behind.
    let leftovers: Vec<_> = std::fs::read_dir(home.tools_dir())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".zem-extract-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn checksum_mismatch_installs_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let home = ZemHome::new(tmp.path().to_path_buf());
    let body = archive("uv");
    let base = serve(body, 1);
    let store = UvStore {
        home: &home,
        release: release(base, "0".repeat(64)),
    };

    let err = store.ensure().unwrap_err();
    assert!(matches!(err, ZemError::ChecksumMismatch { .. }), "{err:?}");
    assert!(!store.installed());
    assert!(!store.dir().exists());
}

#[test]
fn archive_without_uv_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let home = ZemHome::new(tmp.path().to_path_buf());
    let body = archive("uvx");
    let base = serve(body.clone(), 1);
    let store = UvStore {
        home: &home,
        release: release(base, sha256(&body)),
    };

    let err = store.ensure().unwrap_err();
    assert!(matches!(err, ZemError::CorruptState { .. }), "{err:?}");
    assert!(!store.installed());
}

#[test]
fn unreachable_server_is_a_download_error() {
    let tmp = tempfile::tempdir().unwrap();
    let home = ZemHome::new(tmp.path().to_path_buf());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let store = UvStore {
        home: &home,
        release: release(base, "0".repeat(64)),
    };

    let err = store.ensure().unwrap_err();
    assert!(matches!(err, ZemError::Download { .. }), "{err:?}");
    assert!(!store.installed());
}
