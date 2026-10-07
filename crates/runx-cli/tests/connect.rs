use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use crate::support::isolated_runx_command_with_inherited_cwd;

#[test]
fn packaged_native_connect_authenticates_and_lists_grants() -> Result<(), Box<dyn std::error::Error>>
{
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let server = thread::spawn(move || -> Result<(), String> {
        for (path, body) in [
            (
                "/v1/me",
                r#"{"status":"success","principal":{"principal_id":"fixture-user","role":"user"}}"#,
            ),
            ("/v1/grants", r#"{"status":"success","grants":[]}"#),
        ] {
            let (mut stream, _) = listener.accept().map_err(|error| error.to_string())?;
            let mut request = [0u8; 8192];
            let length = stream
                .read(&mut request)
                .map_err(|error| error.to_string())?;
            let request = String::from_utf8_lossy(&request[..length]);
            if !request.starts_with(&format!("GET {path} HTTP/1.1")) {
                return Err(format!("unexpected request for {path}: {request}"));
            }
            if !request
                .to_ascii_lowercase()
                .contains("authorization: bearer rxk_fixture")
            {
                return Err(format!("request for {path} omitted the bearer token"));
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .map_err(|error| error.to_string())?;
        }
        Ok(())
    });

    let output = isolated_runx_command_with_inherited_cwd("connect-package-test")
        .env("RUNX_PUBLIC_API_TOKEN", "rxk_fixture")
        .args([
            "connect",
            "list",
            "--api-base-url",
            &format!("http://{address}"),
            "--allow-local-api",
            "--json",
        ])
        .output()?;
    let server_result = server
        .join()
        .map_err(|_| "connect fixture server panicked")?;
    server_result.map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(body["environment"]["principal_id"], "fixture-user");
    assert_eq!(body["connect"]["grants"], serde_json::json!([]));
    Ok(())
}
