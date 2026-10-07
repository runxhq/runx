use std::collections::BTreeSet;
use std::sync::OnceLock;

use regex::Regex;
use runx_contracts::{JsonObject, JsonValue};
use url::Url;

use super::ExtractMode;

pub(super) fn extract_content(
    body: &str,
    mode: ExtractMode,
    base_url: &Url,
    content_type: &str,
) -> Result<JsonValue, String> {
    match mode {
        ExtractMode::Text => Ok(JsonValue::String(extract_text(body)?)),
        ExtractMode::Links => Ok(JsonValue::Array(extract_links(body, base_url)?)),
        ExtractMode::Json => serde_json::from_str::<JsonValue>(body)
            .map_err(|error| format!("the page is not one JSON document: {error}")),
        ExtractMode::Metadata => Ok(JsonValue::Object(JsonObject::from([
            (
                "title".to_owned(),
                optional_json(first_capture(title_regex()?, body)?),
            ),
            (
                "description".to_owned(),
                optional_json(meta_description(body)?),
            ),
            (
                "canonical".to_owned(),
                optional_json(canonical_url(body, base_url)?),
            ),
            (
                "declared_language".to_owned(),
                optional_json(first_capture(language_regex()?, body)?),
            ),
            (
                "content_type".to_owned(),
                JsonValue::String(content_type.to_owned()),
            ),
        ]))),
    }
}

fn extract_text(body: &str) -> Result<String, String> {
    let without_hidden = replace_all(hidden_regex()?, body, " ");
    let without_tags = replace_all(tag_regex()?, &without_hidden, " ");
    let normalized = replace_all(whitespace_regex()?, &without_tags, " ");
    decode_entities(normalized.trim())
}

fn extract_links(body: &str, base_url: &Url) -> Result<Vec<JsonValue>, String> {
    let mut seen = BTreeSet::new();
    let mut links = Vec::new();
    for captures in link_regex()?.captures_iter(body) {
        let Some(raw) = captures.get(1) else {
            continue;
        };
        let decoded = decode_entities(raw.as_str())?;
        let Ok(url) = base_url.join(&decoded) else {
            continue;
        };
        if seen.insert(url.to_string()) {
            links.push(JsonValue::String(url.to_string()));
        }
    }
    Ok(links)
}

fn meta_description(body: &str) -> Result<Option<String>, String> {
    let first = first_capture(meta_name_first_regex()?, body)?;
    if first.is_some() {
        Ok(first)
    } else {
        first_capture(meta_content_first_regex()?, body)
    }
}

fn canonical_url(body: &str, base_url: &Url) -> Result<Option<String>, String> {
    let Some(value) = first_capture(canonical_regex()?, body)? else {
        return Ok(None);
    };
    Ok(base_url.join(&value).ok().map(|url| url.to_string()))
}

fn first_capture(regex: &Regex, body: &str) -> Result<Option<String>, String> {
    let Some(value) = regex.captures(body).and_then(|captures| captures.get(1)) else {
        return Ok(None);
    };
    let normalized = replace_all(whitespace_regex()?, value.as_str(), " ");
    decode_entities(normalized.trim()).map(Some)
}

fn optional_json(value: Option<String>) -> JsonValue {
    value.map_or(JsonValue::Null, JsonValue::String)
}

fn decode_entities(value: &str) -> Result<String, String> {
    let named = value
        .replace("&nbsp;", " ")
        .replace("&NBSP;", " ")
        .replace("&amp;", "&")
        .replace("&AMP;", "&")
        .replace("&lt;", "<")
        .replace("&LT;", "<")
        .replace("&gt;", ">")
        .replace("&GT;", ">")
        .replace("&quot;", "\"")
        .replace("&QUOT;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'");
    Ok(numeric_entity_regex()?
        .replace_all(&named, |captures: &regex::Captures<'_>| {
            let raw = captures.get(1).map_or("", |value| value.as_str());
            let code = raw
                .strip_prefix(['x', 'X'])
                .map_or_else(|| raw.parse::<u32>(), |hex| u32::from_str_radix(hex, 16));
            code.ok()
                .and_then(char::from_u32)
                .map_or_else(|| captures[0].to_owned(), |character| character.to_string())
        })
        .into_owned())
}

fn replace_all(regex: &Regex, value: &str, replacement: &str) -> String {
    regex.replace_all(value, replacement).into_owned()
}

fn cached_regex(
    cell: &'static OnceLock<Result<Regex, String>>,
    pattern: &str,
) -> Result<&'static Regex, String> {
    match cell.get_or_init(|| Regex::new(pattern).map_err(|error| error.to_string())) {
        Ok(regex) => Ok(regex),
        Err(error) => Err(format!("an extraction pattern failed to compile: {error}")),
    }
}

fn title_regex() -> Result<&'static Regex, String> {
    static VALUE: OnceLock<Result<Regex, String>> = OnceLock::new();
    cached_regex(&VALUE, r"(?is)<title[^>]*>(.*?)</title>")
}

fn language_regex() -> Result<&'static Regex, String> {
    static VALUE: OnceLock<Result<Regex, String>> = OnceLock::new();
    cached_regex(&VALUE, r#"(?is)<html[^>]*\blang=["']([^"']+)["']"#)
}

fn meta_name_first_regex() -> Result<&'static Regex, String> {
    static VALUE: OnceLock<Result<Regex, String>> = OnceLock::new();
    cached_regex(
        &VALUE,
        r#"(?is)<meta[^>]+name=["']description["'][^>]+content=["']([^"']*)["']"#,
    )
}

fn meta_content_first_regex() -> Result<&'static Regex, String> {
    static VALUE: OnceLock<Result<Regex, String>> = OnceLock::new();
    cached_regex(
        &VALUE,
        r#"(?is)<meta[^>]+content=["']([^"']*)["'][^>]+name=["']description["']"#,
    )
}

fn canonical_regex() -> Result<&'static Regex, String> {
    static VALUE: OnceLock<Result<Regex, String>> = OnceLock::new();
    cached_regex(
        &VALUE,
        r#"(?is)<link[^>]+rel=["'][^"']*canonical[^"']*["'][^>]+href=["']([^"']+)["']"#,
    )
}

fn link_regex() -> Result<&'static Regex, String> {
    static VALUE: OnceLock<Result<Regex, String>> = OnceLock::new();
    cached_regex(&VALUE, r#"(?is)<a\b[^>]*\bhref=["']([^"']+)["']"#)
}

fn hidden_regex() -> Result<&'static Regex, String> {
    static VALUE: OnceLock<Result<Regex, String>> = OnceLock::new();
    cached_regex(
        &VALUE,
        r"(?is)<(?:script|style|noscript)\b[^>]*>.*?</(?:script|style|noscript)>",
    )
}

fn tag_regex() -> Result<&'static Regex, String> {
    static VALUE: OnceLock<Result<Regex, String>> = OnceLock::new();
    cached_regex(&VALUE, r"(?s)<[^>]+>")
}

fn whitespace_regex() -> Result<&'static Regex, String> {
    static VALUE: OnceLock<Result<Regex, String>> = OnceLock::new();
    cached_regex(&VALUE, r"\s+")
}

fn numeric_entity_regex() -> Result<&'static Regex, String> {
    static VALUE: OnceLock<Result<Regex, String>> = OnceLock::new();
    cached_regex(&VALUE, r"&#([xX][0-9a-fA-F]+|[0-9]+);")
}

#[cfg(test)]
mod tests {
    use super::{extract_links, extract_text};
    use runx_contracts::JsonValue;
    use url::Url;

    #[test]
    fn extraction_is_native_and_resolves_links() -> Result<(), Box<dyn std::error::Error>> {
        let body = r#"<html><body><script>no</script><p>A &amp; B</p><a href="/next">Next</a></body></html>"#;
        assert_eq!(extract_text(body)?, "A & B Next");
        assert_eq!(
            extract_links(body, &Url::parse("https://example.com/start")?)?,
            vec![JsonValue::String("https://example.com/next".to_owned())]
        );
        Ok(())
    }
}
