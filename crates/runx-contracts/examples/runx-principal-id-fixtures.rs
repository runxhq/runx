use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use runx_contracts::{PrincipalReference, RunxPrincipalId};
use serde_json::{Value, json};

struct Options {
    out_dir: PathBuf,
    check: bool,
}

struct Case {
    name: &'static str,
    input: String,
    expected_valid: bool,
}

fn main() -> io::Result<()> {
    let options = parse_args()?;
    let document = fixture_document()?;
    reconcile_file(
        &options.out_dir.join("principal-id-v1.vectors.json"),
        canonical_bytes(&document)?,
        options.check,
    )?;
    reject_orphans(&options.out_dir, options.check)
}

fn parse_args() -> io::Result<Options> {
    let mut out_dir = None;
    let mut check = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out_dir = args.next().map(PathBuf::from),
            "--check" => check = true,
            other => return Err(io::Error::other(format!("unsupported argument: {other}"))),
        }
    }
    Ok(Options {
        out_dir: out_dir.ok_or_else(|| io::Error::other("--out is required"))?,
        check,
    })
}

fn fixture_document() -> io::Result<Value> {
    let cases = vec![
        Case {
            name: "simple-user",
            input: "user_1".to_owned(),
            expected_valid: true,
        },
        Case {
            name: "namespaced-edge-key",
            input: "edge-key:sha256:abcdef0123456789".to_owned(),
            expected_valid: true,
        },
        Case {
            name: "maximum-length",
            input: "a".repeat(RunxPrincipalId::MAX_LENGTH),
            expected_valid: true,
        },
        Case {
            name: "empty",
            input: String::new(),
            expected_valid: false,
        },
        Case {
            name: "invalid-leading-punctuation",
            input: ".user".to_owned(),
            expected_valid: false,
        },
        Case {
            name: "surrounding-whitespace",
            input: " user_1 ".to_owned(),
            expected_valid: false,
        },
        Case {
            name: "internal-whitespace",
            input: "user name".to_owned(),
            expected_valid: false,
        },
        Case {
            name: "control-character",
            input: "user\nname".to_owned(),
            expected_valid: false,
        },
        Case {
            name: "unicode",
            input: "usér".to_owned(),
            expected_valid: false,
        },
        Case {
            name: "slash",
            input: "user/name".to_owned(),
            expected_valid: false,
        },
        Case {
            name: "overlength",
            input: "a".repeat(RunxPrincipalId::MAX_LENGTH + 1),
            expected_valid: false,
        },
    ];

    let cases = cases
        .into_iter()
        .map(|case| {
            let parsed = RunxPrincipalId::new(case.input.clone());
            if parsed.is_some() != case.expected_valid {
                return Err(io::Error::other(format!(
                    "principal-id fixture expectation disagrees with Rust: {}",
                    case.name
                )));
            }
            let reference = parsed.map(PrincipalReference::from_runx_principal_id);
            Ok(json!({
                "expectation": if case.expected_valid { "valid" } else { "invalid" },
                "input": case.input,
                "name": case.name,
                "reference": reference,
            }))
        })
        .collect::<io::Result<Vec<_>>>()?;

    Ok(json!({
        "cases": cases,
        "grammar": "^[A-Za-z0-9][A-Za-z0-9._:-]{0,255}$",
        "schema": "runx.principal_id.fixtures.v1",
    }))
}

fn canonical_bytes(value: &Value) -> io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn reconcile_file(path: &Path, expected: Vec<u8>, check: bool) -> io::Result<()> {
    if check {
        let actual = fs::read(path).map_err(|error| {
            io::Error::other(format!(
                "missing generated fixture {}: {error}",
                path.display()
            ))
        })?;
        if actual != expected {
            return Err(io::Error::other(format!(
                "generated fixture is stale: {}",
                path.display()
            )));
        }
    } else {
        fs::create_dir_all(
            path.parent()
                .ok_or_else(|| io::Error::other("fixture path has no parent"))?,
        )?;
        fs::write(path, expected)?;
    }
    Ok(())
}

fn reject_orphans(out_dir: &Path, check: bool) -> io::Result<()> {
    let expected = BTreeSet::from(["principal-id-v1.vectors.json"]);
    if !out_dir.exists() {
        return if check {
            Err(io::Error::other(format!(
                "missing generated fixture directory: {}",
                out_dir.display()
            )))
        } else {
            Ok(())
        };
    }
    for entry in fs::read_dir(out_dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if path.extension().and_then(|value| value.to_str()) != Some("json")
            || expected.contains(name)
        {
            continue;
        }
        if check {
            return Err(io::Error::other(format!(
                "orphan principal-id fixture: {}",
                path.display()
            )));
        }
        fs::remove_file(path)?;
    }
    Ok(())
}
