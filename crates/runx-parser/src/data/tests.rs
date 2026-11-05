#![allow(clippy::expect_used)]
use super::{DataDecodeError, decode_data};
use serde_json::json;

#[test]
fn decodes_the_same_data_across_formats() {
    let expected = json!({"name":"東京", "versions":["1.0.0", "2.0.0"], "active":true});
    for (format, source) in [
        (
            "json",
            r#"{"name":"東京","versions":["1.0.0","2.0.0"],"active":true}"#,
        ),
        (
            "yaml",
            "name: 東京\nversions: ['1.0.0', '2.0.0']\nactive: true\n",
        ),
        (
            "toml",
            "name = '東京'\nversions = ['1.0.0', '2.0.0']\nactive = true\n",
        ),
    ] {
        let bytes = serde_json::to_vec(&expected).expect("expected bytes").len();
        assert_eq!(decode_data(source, format, bytes), Ok(expected.clone()));
        assert_eq!(
            decode_data(source, format, bytes - 1),
            Err(DataDecodeError::ResourceLimit)
        );
        assert_eq!(
            decode_data(&format!("\u{feff}{source}"), format, bytes),
            Ok(expected.clone())
        );
    }
}

#[test]
fn rejects_ambiguity_and_never_echoes_source() {
    for (format, source) in [
        ("json", r#"{"secret":"do-not-expose","secret":1}"#),
        ("json", "{} {}"),
        ("json", "18446744073709551616"),
        ("json", "-9223372036854775809"),
        ("yaml", "secret: do-not-expose\nsecret: duplicate"),
        ("yaml", "1: do-not-expose"),
        ("yaml", "---\na: 1\n---\nb: 2"),
        ("yaml", "a: !private do-not-expose"),
        ("yaml", "a: !!secret do-not-expose"),
        ("yaml", "a: !<tag:example.com,2000:x> do-not-expose"),
        (
            "yaml",
            "%TAG !e! tag:example.com,2000:app/\n---\na: !e!x do-not-expose",
        ),
        ("yaml", "a: !!python/object/apply:os.system [do-not-expose]"),
        ("yaml", "a: 1234567890123456789012345678901234567890"),
        ("yaml", "a: 1e400"),
        ("yaml", "a: -1e400"),
        ("yaml", "base: &b {x: 1}\nd:\n  <<: *b\n  y: 2"),
        ("yaml", "a: .inf"),
        ("toml", "secret = 'do-not-expose'\nsecret = 1"),
        ("toml", "a = nan"),
    ] {
        let error = decode_data(source, format, 4096).expect_err("invalid data");
        assert_eq!(error, DataDecodeError::InvalidData);
        assert!(!error.to_string().contains("do-not-expose"));
    }
}

#[test]
fn keeps_explicit_strings_and_finite_floats_without_an_arbitrary_numeric_ceiling() {
    for (format, source) in [
        (
            "json",
            r#"{"float":1e100,"integer":"18446744073709551617","tag":"!!custom","<<":"literal"}"#,
        ),
        (
            "yaml",
            "float: 1e100\ninteger: '18446744073709551617'\ntag: '!!custom'\n'<<': literal\n",
        ),
    ] {
        let value = decode_data(source, format, 4096)
            .expect("explicit strings and representable floating point");
        assert_eq!(value["float"].as_f64(), Some(1e100));
        assert_eq!(value["integer"], "18446744073709551617");
        assert_eq!(value["tag"], "!!custom");
        assert_eq!(value["<<"], "literal");
    }
}

#[test]
fn yaml_radix_signs_do_not_reclassify_strings_or_refuse_large_strings() {
    let value = decode_data("hex: 0x10\noctal: 0o17\nnegative_hex: -0x10\npositive_octal: +0o17\nlarge_signed: -0xffffffffffffffffffffffffffffffff\n", "yaml", 4096).expect("YAML core scalar meanings");
    assert_eq!(value["hex"], 16);
    assert_eq!(value["octal"], 15);
    assert_eq!(value["negative_hex"], "-0x10");
    assert_eq!(value["positive_octal"], "+0o17");
    assert_eq!(value["large_signed"], "-0xffffffffffffffffffffffffffffffff");
    for spelling in ["0x-10", "0x+1f", "0o-7", "0o+7", "++12", "+-12", "-+12"] {
        let source = format!("value: {spelling}\n");
        let decoded = decode_data(&source, "yaml", 4096).expect("YAML core string");
        assert_eq!(decoded["value"], spelling);
    }
}

#[test]
fn charges_alias_expansion_and_escaped_unicode() {
    let source = "a: &a ['α', 'b']\nb: [*a, *a, *a]\n";
    let decoded = decode_data(source, "yaml", 4096).expect("bounded aliases");
    let size = serde_json::to_vec(&decoded).expect("bytes").len();
    assert_eq!(decode_data(source, "yaml", size), Ok(decoded));
    assert_eq!(
        decode_data(source, "yaml", size - 1),
        Err(DataDecodeError::ResourceLimit)
    );
    let quoted = r#"{"value":"\n\"\\東京"}"#;
    let decoded = decode_data(quoted, "json", 4096).expect("escaped text");
    let size = serde_json::to_vec(&decoded).expect("bytes").len();
    assert_eq!(
        decode_data(quoted, "json", size - 1),
        Err(DataDecodeError::ResourceLimit)
    );
}

#[test]
fn bounds_anchor_storage_and_refuses_cycles_without_changing_quoted_content() {
    let overlapping = "a: &a [b: &b [c: &c [d: &d [1, 2, 3]]]]";
    let decoded = decode_data(overlapping, "yaml", 4096).expect("small anchored tree");
    let output_bytes = serde_json::to_vec(&decoded).expect("bytes").len();
    assert_eq!(
        decode_data(overlapping, "yaml", output_bytes),
        Err(DataDecodeError::ResourceLimit)
    );
    assert_eq!(
        decode_data("a: &a [*a]", "yaml", 4096),
        Err(DataDecodeError::InvalidData)
    );
    let literal = decode_data(
        "value: |\n  !!tag 18446744073709551617\n  <<: 1e400\n",
        "yaml",
        4096,
    )
    .expect("literal content");
    assert_eq!(literal["value"], "!!tag 18446744073709551617\n<<: 1e400\n");
}

#[test]
fn handles_toml_tables_inline_values_and_dates() {
    let decoded = decode_data("version = 4\ncreated = 2026-11-03\n[[package]]\nname = 'example'\nversion = '1.0.0'\nmeta = { optional = false, tags = ['local'] }\n", "toml", 4096).expect("TOML");
    assert_eq!(decoded["created"], "2026-11-03");
    assert_eq!(decoded["package"][0]["meta"]["tags"], json!(["local"]));
}

#[test]
fn rejects_unknown_formats_and_excessive_depth() {
    assert_eq!(
        decode_data("{}", "xml", 4096),
        Err(DataDecodeError::UnsupportedFormat)
    );
    let nested = format!("{}0{}", "[".repeat(150), "]".repeat(150));
    assert!(decode_data(&nested, "json", 4096).is_err());
    assert!(decode_data(&nested, "yaml", 4096).is_err());
}
