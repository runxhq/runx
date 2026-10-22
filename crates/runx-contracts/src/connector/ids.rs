//! Validated identifiers for connectors, operations, credentials, instances
//! and digests. Each grammar is defined once here; the runtime, the parser,
//! the hosted route and the generated TypeScript all read it from this module.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Value, json};

use crate::schema::{RunxSchema, SchemaMapKey};

/// A connector's whole name: `<namespace>` or `<namespace>/<surface>`. The
/// namespace is a registry namespace, in the publisher slug grammar; a surface
/// is lowercase, starts with a letter and is 2 to 64 characters. `versions` is
/// reserved in either place, so `/v1/connectors/<name>/versions/<v>` reads one
/// way.
pub const CONNECTOR_NAME_PATTERN: &str = "^(?!versions(?:/|$))[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?(?:/(?!versions$)[a-z][a-z0-9-]{0,62}[a-z0-9])?$";
/// The word no namespace or surface may be: it names a connector's versions.
pub const RESERVED_CONNECTOR_SEGMENT: &str = "versions";
/// Dotted lowercase segments, underscores allowed, at least two segments.
pub const OPERATION_NAME_PATTERN: &str = r"^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$";
pub const OPERATION_NAME_MAX_LENGTH: usize = 100;
pub const CREDENTIAL_NAME_PATTERN: &str = "^[a-z][a-z0-9_]{0,63}$";
pub const ENV_NAME_PATTERN: &str = "^[A-Z][A-Z0-9_]{0,127}$";
pub const INSTANCE_ID_PATTERN: &str = "^[a-z0-9][a-z0-9._-]{0,127}$";
pub const DIGEST_PATTERN: &str = "^sha256:[0-9a-f]{64}$";
/// The secrets managers a [`SecretReference`] may name.
pub const SECRET_SCHEMES: [&str; 6] = ["op", "aws-sm", "aws-ps", "vault", "keychain", "file"];
/// `<scheme>://<locator>[#<field>]`, a scheme from [`SECRET_SCHEMES`]. A
/// locator may hold spaces between other characters, as 1Password items and
/// keychain services are named; no other whitespace.
pub const SECRET_REFERENCE_PATTERN: &str =
    r"^(op|aws-sm|aws-ps|vault|keychain|file)://[^\s#](([^\s#]| )*[^\s#])?(#[^\s#]+)?$";
/// What a refusal of a value that is not a reference names.
pub const SECRET_REFERENCE_EXPECTED: &str = "--from takes a secret reference: op://, aws-sm://, aws-ps://, vault://, keychain:// or file://";
/// A name the operator gives a secret: anything printable, with no space at
/// either end and no control character.
pub const SECRET_NAME_PATTERN: &str = r"^[^\s\x00-\x1f\x7f]([^\x00-\x1f\x7f]*[^\s\x00-\x1f\x7f])?$";

/// Whether `value` is a connector name: a namespace, or a namespace and one
/// surface, neither of them `versions`.
#[must_use]
pub fn is_connector_name(value: &str) -> bool {
    let (namespace, surface) = match value.split_once('/') {
        Some((namespace, surface)) => (namespace, Some(surface)),
        None => (value, None),
    };
    is_publisher_namespace(namespace)
        && namespace != RESERVED_CONNECTOR_SEGMENT
        && surface.is_none_or(|surface| {
            is_connector_surface(surface) && surface != RESERVED_CONNECTOR_SEGMENT
        })
}

/// A surface under a parent namespace, such as `calendar` in `google/calendar`.
fn is_connector_surface(value: &str) -> bool {
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

fn is_secret_name(value: &str) -> bool {
    value.len() <= 256
        && !value.is_empty()
        && value.trim() == value
        && !value.chars().any(char::is_control)
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
    /// A connector's name and its identity everywhere: `stripe`,
    /// `google/calendar`. Who published a version is provenance, never part
    /// of the name.
    ConnectorName,
    CONNECTOR_NAME_PATTERN,
    129,
    is_connector_name,
    "connector name"
);

/// The registry namespace grammar, which every publisher slug and every
/// connector name's first segment uses.
#[must_use]
pub fn is_publisher_namespace(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && bytes
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

impl ConnectorName {
    /// The registry namespace: the whole name of a leaf (`stripe`), the
    /// parent of a surface (`google` in `google/calendar`).
    #[must_use]
    pub fn namespace(&self) -> &str {
        self.0
            .split_once('/')
            .map_or(self.0.as_str(), |(namespace, _)| namespace)
    }

    /// The surface under a parent namespace, when the name has one.
    #[must_use]
    pub fn surface(&self) -> Option<&str> {
        self.0.split_once('/').map(|(_, surface)| surface)
    }
}
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
    /// Where a secrets manager holds a secret: `<scheme>://<locator>`, and
    /// `#<field>` for one field of a JSON secret. Never the secret itself.
    SecretReference,
    SECRET_REFERENCE_PATTERN,
    1024,
    is_secret_reference,
    "secret reference"
);
identifier!(
    /// The name of a secret the operator keeps in runx (`runx secret set`):
    /// what a binding, the managed agent and a host name it by. Never the
    /// secret itself.
    SecretName,
    SECRET_NAME_PATTERN,
    256,
    is_secret_name,
    "secret name"
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

#[must_use]
pub fn is_secret_reference(value: &str) -> bool {
    value.len() <= 1024 && secret_reference_parts(value).is_some()
}

fn secret_reference_parts(value: &str) -> Option<(&str, &str, Option<&str>)> {
    let (scheme, rest) = value.split_once("://")?;
    if !SECRET_SCHEMES.contains(&scheme) {
        return None;
    }
    let (locator, field) = match rest.split_once('#') {
        Some((locator, field)) => (locator, Some(field)),
        None => (rest, None),
    };
    let locator_ok = !locator.is_empty()
        && !locator.starts_with(' ')
        && !locator.ends_with(' ')
        && !locator.chars().any(|c| c.is_whitespace() && c != ' ');
    let field_ok = field.is_none_or(|field| {
        !field.is_empty() && !field.contains('#') && !field.chars().any(char::is_whitespace)
    });
    (locator_ok && field_ok).then_some((scheme, locator, field))
}

impl SecretReference {
    /// The manager: one of [`SECRET_SCHEMES`].
    #[must_use]
    pub fn scheme(&self) -> &str {
        secret_reference_parts(&self.0).map_or("", |(scheme, _, _)| scheme)
    }

    /// Where in the manager, as the manager names it.
    #[must_use]
    pub fn locator(&self) -> &str {
        secret_reference_parts(&self.0).map_or("", |(_, locator, _)| locator)
    }

    /// The field of a JSON secret, when the reference selects one.
    #[must_use]
    pub fn field(&self) -> Option<&str> {
        secret_reference_parts(&self.0).and_then(|(_, _, field)| field)
    }
}

impl Digest {
    /// The digest of `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(crate::sha256_prefixed(bytes))
    }
}
