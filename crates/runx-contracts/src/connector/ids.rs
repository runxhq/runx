//! Validated identifiers for connectors, operations, credentials, instances
//! and digests. Each grammar is defined once here; the runtime, the parser,
//! the hosted route and the generated TypeScript all read it from this module.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Value, json};

use crate::schema::{RunxSchema, SchemaMapKey};

/// `^[a-z][a-z0-9-]{0,62}[a-z0-9]$`: equal to the provider slug grants and the
/// catalog use.
pub const CONNECTOR_ID_PATTERN: &str = "^[a-z][a-z0-9-]{0,62}[a-z0-9]$";
/// Dotted lowercase segments, underscores allowed, at least two segments.
pub const OPERATION_NAME_PATTERN: &str = r"^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$";
pub const OPERATION_NAME_MAX_LENGTH: usize = 100;
pub const CREDENTIAL_NAME_PATTERN: &str = "^[a-z][a-z0-9_]{0,63}$";
pub const ENV_NAME_PATTERN: &str = "^[A-Z][A-Z0-9_]{0,127}$";
pub const INSTANCE_ID_PATTERN: &str = "^[a-z0-9][a-z0-9._-]{0,127}$";
pub const DIGEST_PATTERN: &str = "^sha256:[0-9a-f]{64}$";

#[must_use]
pub fn is_connector_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    let (Some(first), Some(last)) = (bytes.first(), bytes.last()) else {
        return false;
    };
    (2..=64).contains(&bytes.len())
        && first.is_ascii_lowercase()
        && (last.is_ascii_lowercase() || last.is_ascii_digit())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

#[must_use]
pub fn is_operation_name(value: &str) -> bool {
    if value.is_empty() || value.len() > OPERATION_NAME_MAX_LENGTH {
        return false;
    }
    let mut segments = 0_usize;
    for segment in value.split('.') {
        let mut bytes = segment.bytes();
        let valid = bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
            && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
        if !valid {
            return false;
        }
        segments += 1;
    }
    segments >= 2
}

fn is_credential_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
}

fn is_env_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=128).contains(&bytes.len())
        && bytes[0].is_ascii_uppercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'_')
}

fn is_instance_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=128).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn is_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

macro_rules! identifier {
    ($(#[$meta:meta])* $name:ident, $pattern:expr, $max:expr, $valid:path, $what:literal) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Returns `None` when the value is outside the grammar.
            pub fn new(value: impl Into<String>) -> Option<Self> {
                let value = value.into();
                $valid(&value).then_some(Self(value))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            #[must_use]
            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.0 == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value.clone()).ok_or_else(|| {
                    de::Error::custom(format!(concat!("{:?} is not a valid ", $what), value))
                })
            }
        }

        impl RunxSchema for $name {
            fn json_schema() -> Value {
                json!({ "type": "string", "pattern": $pattern, "maxLength": $max })
            }
        }

        impl SchemaMapKey for $name {
            const PATTERN: Option<&'static str> = Some($pattern);
        }
    };
}

identifier!(
    /// A connector id.
    ConnectorId,
    CONNECTOR_ID_PATTERN,
    64,
    is_connector_id,
    "connector id"
);
identifier!(
    /// A connector operation name such as `channel.post` or `reports.run_realtime`.
    OperationName,
    OPERATION_NAME_PATTERN,
    OPERATION_NAME_MAX_LENGTH,
    is_operation_name,
    "operation name"
);
identifier!(
    /// A named credential on a connector, or a field of one.
    CredentialName,
    CREDENTIAL_NAME_PATTERN,
    64,
    is_credential_name,
    "credential name"
);
identifier!(
    /// A process environment variable name.
    EnvName,
    ENV_NAME_PATTERN,
    128,
    is_env_name,
    "environment variable name"
);
identifier!(
    /// A connector instance id.
    InstanceId,
    INSTANCE_ID_PATTERN,
    128,
    is_instance_id,
    "instance id"
);
identifier!(
    /// `sha256:` followed by 64 lowercase hex digits.
    Digest,
    DIGEST_PATTERN,
    71,
    is_digest,
    "sha256 digest"
);

impl CredentialName {
    /// The conventional single-field credential name.
    #[must_use]
    pub fn token() -> Self {
        Self("token".to_owned())
    }
}

impl Digest {
    /// The digest of `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(crate::sha256_prefixed(bytes))
    }
}
