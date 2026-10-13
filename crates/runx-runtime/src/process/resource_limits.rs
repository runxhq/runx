#[cfg(unix)]
use rustix::process::{Resource, Rlimit, getrlimit};

#[cfg(unix)]
use super::ProcessSpec;

#[cfg(unix)]
const RESOURCE_LIMIT_SHELL: &str = "/bin/sh";
#[cfg(unix)]
const RESOURCE_LIMIT_ARG0: &str = "runx-resource-limits";
#[cfg(unix)]
const MISSING_COMMAND_CHECK: &str = "{ command -v \"$1\" >/dev/null 2>&1 || { printf '%s is not installed or not on PATH\\n' \"$1\" >&2; exit 127; }; }";
#[cfg(unix)]
const RESOURCE_LIMIT_FILE_BLOCK_BYTES: u64 = 512;
#[cfg(any(target_os = "linux", target_os = "android"))]
const RESOURCE_LIMIT_MEMORY_KIB_BYTES: u64 = 1024;
#[cfg(unix)]
const CHILD_MAX_OPEN_FILES: u64 = 256;
#[cfg(unix)]
const CHILD_MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
#[cfg(unix)]
const CHILD_MAX_CPU_SECONDS: u64 = 60;
#[cfg(any(target_os = "linux", target_os = "android"))]
// Memory is bounded by what a tool commits, not by the address space it
// reserves: V8 reserves large unbacked regions for pointer compression and for
// every WebAssembly memory, so an address-space ceiling refuses a wasm-using
// tool with free memory to spare. The data limit counts committed private
// writable memory only; CPU, file, descriptor, process, and output bounds
// still apply.
const CHILD_MAX_DATA_BYTES: u64 = 16 * 1024 * 1024 * 1024;

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ChildResourceLimit {
    flag: &'static str,
    value: u64,
}

#[cfg(unix)]
pub(super) fn resource_limit_shell() -> &'static str {
    RESOURCE_LIMIT_SHELL
}

#[cfg(unix)]
fn child_resource_limits() -> Vec<ChildResourceLimit> {
    let mut limits = Vec::with_capacity(5);
    push_count_limit(&mut limits, "-n", Resource::Nofile, CHILD_MAX_OPEN_FILES);
    push_scaled_limit(
        &mut limits,
        "-f",
        Resource::Fsize,
        CHILD_MAX_FILE_BYTES,
        RESOURCE_LIMIT_FILE_BLOCK_BYTES,
    );
    push_count_limit(&mut limits, "-t", Resource::Cpu, CHILD_MAX_CPU_SECONDS);
    // POSIX sh does not guarantee a process-count ulimit flag, and Ubuntu dash
    // rejects `ulimit -u`. Keep this wrapper to flags supported by /bin/sh.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    push_scaled_limit(
        &mut limits,
        "-d",
        Resource::Data,
        CHILD_MAX_DATA_BYTES,
        RESOURCE_LIMIT_MEMORY_KIB_BYTES,
    );
    limits
}

#[cfg(unix)]
fn push_count_limit(
    limits: &mut Vec<ChildResourceLimit>,
    flag: &'static str,
    resource: Resource,
    target: u64,
) {
    limits.push(ChildResourceLimit {
        flag,
        value: shell_limit_value(getrlimit(resource), target, 1),
    });
}

#[cfg(unix)]
fn push_scaled_limit(
    limits: &mut Vec<ChildResourceLimit>,
    flag: &'static str,
    resource: Resource,
    target: u64,
    unit_bytes: u64,
) {
    limits.push(ChildResourceLimit {
        flag,
        value: shell_limit_value(getrlimit(resource), target, unit_bytes),
    });
}

#[cfg(unix)]
fn shell_limit_value(current: Rlimit, target: u64, unit: u64) -> u64 {
    let hard_limit = current.maximum.unwrap_or(target);
    target.min(hard_limit) / unit
}

#[cfg(unix)]
pub(super) fn resource_limit_shell_args(spec: &ProcessSpec) -> Vec<String> {
    resource_limit_shell_args_for_limits(&child_resource_limits(), spec)
}

#[cfg(unix)]
fn resource_limit_shell_args_for_limits(
    limits: &[ChildResourceLimit],
    spec: &ProcessSpec,
) -> Vec<String> {
    let mut script = String::new();
    for (index, limit) in limits.iter().enumerate() {
        if index > 0 {
            script.push_str(" && ");
        }
        script.push_str("ulimit ");
        script.push_str(limit.flag);
        script.push_str(" \"$");
        script.push_str(&(index + 1).to_string());
        script.push('"');
    }
    if !limits.is_empty() {
        script.push_str(" && shift ");
        script.push_str(&limits.len().to_string());
        script.push_str(" && ");
    }
    // A command that cannot be found is named as the tool's own failure, not
    // as this wrapper's shell error.
    script.push_str(MISSING_COMMAND_CHECK);
    script.push_str(" && exec \"$@\"");

    let mut args = vec!["-c".to_owned(), script, RESOURCE_LIMIT_ARG0.to_owned()];
    args.extend(limits.iter().map(|limit| limit.value.to_string()));
    args.push(spec.command.clone());
    args.extend(spec.args.iter().cloned());
    args
}

#[cfg(all(test, unix))]
pub(super) fn child_resource_limit_value(flag: &str) -> Option<u64> {
    child_resource_limits()
        .into_iter()
        .find(|limit| limit.flag == flag)
        .map(|limit| limit.value)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn shell_limit_value_clamps_to_inherited_hard_limit() {
        let unlimited = Rlimit {
            current: None,
            maximum: None,
        };
        assert_eq!(shell_limit_value(unlimited, 128, 1), 128);

        let stricter_parent = Rlimit {
            current: Some(64),
            maximum: Some(64),
        };
        assert_eq!(shell_limit_value(stricter_parent, 128, 1), 64);

        let byte_limit = Rlimit {
            current: Some(1536),
            maximum: Some(1536),
        };
        assert_eq!(
            shell_limit_value(byte_limit, 4096, RESOURCE_LIMIT_FILE_BLOCK_BYTES),
            3
        );
    }

    #[test]
    fn resource_limit_shell_args_do_not_interpolate_requested_command() {
        let spec = ProcessSpec::new("test", "echo $(touch should-not-run)", 128)
            .args(vec!["hello; rm -rf /".to_owned()]);
        let limits = vec![
            ChildResourceLimit {
                flag: "-n",
                value: 256,
            },
            ChildResourceLimit {
                flag: "-t",
                value: 60,
            },
        ];

        let args = resource_limit_shell_args_for_limits(&limits, &spec);

        assert_eq!(
            args,
            vec![
                "-c".to_owned(),
                format!(
                    "ulimit -n \"$1\" && ulimit -t \"$2\" && shift 2 && {MISSING_COMMAND_CHECK} && exec \"$@\""
                ),
                RESOURCE_LIMIT_ARG0.to_owned(),
                "256".to_owned(),
                "60".to_owned(),
                "echo $(touch should-not-run)".to_owned(),
                "hello; rm -rf /".to_owned(),
            ]
        );
    }
}
