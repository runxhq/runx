//! `<ref>#<method>`: what `runx skill`, `runx skill inspect` and `runx mcp`
//! take. The reference is a skill reference, a `publisher/name@version`, a
//! package directory, or a file in one, and the method follows its first `#`,
//! as a `skill` step and a turn's `calls.skills` name one. A reference with no
//! `#method` runs the package's `default_method`, or its only method.

use std::ffi::OsStr;
use std::path::PathBuf;

/// A reference and the method it names, if it names one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MethodRef {
    pub skill_path: PathBuf,
    pub method: Option<String>,
}

impl MethodRef {
    /// Split `token` at its first `#`. The reference is resolved without the
    /// method, so a package directory named with `#method` is that directory,
    /// never a name to look up in a registry.
    ///
    /// # Errors
    ///
    /// Returns why `token` names no reference, or names an empty or non-UTF-8
    /// method.
    pub fn parse(token: &OsStr, command: &str) -> Result<Self, String> {
        let refuse = || {
            format!(
                "runx {command} names a method as <ref>#<method>: {}",
                token.to_string_lossy()
            )
        };
        let Some((skill, method)) = split_at_hash(token) else {
            if token.is_empty() {
                return Err(refuse());
            }
            return Ok(Self {
                skill_path: PathBuf::from(token),
                method: None,
            });
        };
        let method = method.to_str().ok_or_else(refuse)?;
        if skill.is_empty() || method.is_empty() {
            return Err(refuse());
        }
        Ok(Self {
            skill_path: PathBuf::from(skill),
            method: Some(method.to_owned()),
        })
    }
}

/// The text before and after the first `#`, which a filesystem path need not
/// spell in UTF-8.
#[cfg(unix)]
fn split_at_hash(token: &OsStr) -> Option<(&OsStr, &OsStr)> {
    use std::os::unix::ffi::OsStrExt;

    let bytes = token.as_bytes();
    let at = bytes.iter().position(|byte| *byte == b'#')?;
    Some((
        OsStr::from_bytes(&bytes[..at]),
        OsStr::from_bytes(&bytes[at + 1..]),
    ))
}

/// The text before and after the first `#`. Off unix a path that isn't UTF-8
/// has no method to split.
#[cfg(not(unix))]
fn split_at_hash(token: &OsStr) -> Option<(&OsStr, &OsStr)> {
    let (skill, method) = token.to_str()?.split_once('#')?;
    Some((OsStr::new(skill), OsStr::new(method)))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::PathBuf;

    use super::MethodRef;

    #[test]
    fn a_reference_names_its_method_after_its_first_hash() -> Result<(), String> {
        for (token, skill, method) in [
            ("skills/nws-weather", "skills/nws-weather", None),
            (
                "skills/nws-weather#normalize",
                "skills/nws-weather",
                Some("normalize"),
            ),
            (
                "acme/echo@1.0.0#default",
                "acme/echo@1.0.0",
                Some("default"),
            ),
            ("./echo/SKILL.md#shout", "./echo/SKILL.md", Some("shout")),
            ("echo#a#b", "echo", Some("a#b")),
        ] {
            assert_eq!(
                MethodRef::parse(OsStr::new(token), "skill")?,
                MethodRef {
                    skill_path: PathBuf::from(skill),
                    method: method.map(str::to_owned),
                },
                "{token}"
            );
        }
        for refused in ["", "#normalize", "skills/nws-weather#"] {
            let error = MethodRef::parse(OsStr::new(refused), "skill")
                .err()
                .ok_or(format!("{refused:?} parsed"))?;
            assert!(error.contains("<ref>#<method>"), "{error}");
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_utf8_still_names_a_method() -> Result<(), String> {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let token = OsString::from_vec(b"skill-\xff#review".to_vec());
        assert_eq!(
            MethodRef::parse(&token, "skill")?,
            MethodRef {
                skill_path: PathBuf::from(OsString::from_vec(b"skill-\xff".to_vec())),
                method: Some("review".to_owned()),
            }
        );
        let refused = OsString::from_vec(b"skill#\xff".to_vec());
        assert!(MethodRef::parse(&refused, "skill").is_err());
        Ok(())
    }
}
