use std::fs;
use std::io;
use std::path::PathBuf;

use runx_runtime::native_capability_snapshot;

struct Options {
    out: PathBuf,
    check: bool,
}

fn main() -> Result<(), io::Error> {
    let options = parse_args()?;
    let mut rendered =
        serde_json::to_string_pretty(&native_capability_snapshot()).map_err(io::Error::other)?;
    rendered.push('\n');

    if options.check {
        let current = fs::read_to_string(&options.out)?;
        if current != rendered {
            return Err(io::Error::other(format!(
                "native capability snapshot is stale: {}",
                options.out.display()
            )));
        }
        return Ok(());
    }

    if let Some(parent) = options.out.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&options.out, rendered)?;
    Ok(())
}

fn parse_args() -> Result<Options, io::Error> {
    let mut out = None;
    let mut check = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::other("--out requires a file path"))?;
                out = Some(PathBuf::from(value));
            }
            "--check" => check = true,
            other => {
                return Err(io::Error::other(format!("unsupported argument: {other}")));
            }
        }
    }
    Ok(Options {
        out: out.ok_or_else(|| io::Error::other("--out is required"))?,
        check,
    })
}
