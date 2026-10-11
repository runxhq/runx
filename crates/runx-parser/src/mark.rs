//! The one check for a `mark.svg` that travels beside a `SKILL.md` or a
//! `connector.yaml`. A mark is drawn by every surface as an image, so the file
//! may hold only geometry and paint: shapes, groups, transforms and gradients
//! defined in the same file. Anything that can fetch, run, embed or style is
//! refused, and so is anything this list does not name.

use std::collections::BTreeSet;

/// The file name a mark takes beside the manifest it marks.
pub const MARK_FILE: &str = "mark.svg";
pub const MARK_MAX_BYTES: usize = 32 * 1024;
const MAX_ELEMENTS: usize = 256;
const MAX_DEPTH: usize = 16;
const MIN_VIEWBOX: u32 = 16;
const MAX_VIEWBOX: u32 = 1024;
const MAX_COORDINATE: f64 = 1_000_000.0;
const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";

const SHAPE_ATTRIBUTES: &[&str] = &[
    "d",
    "points",
    "x",
    "y",
    "x1",
    "y1",
    "x2",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "width",
    "height",
    "fill",
    "stroke",
    "stroke-width",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "fill-rule",
    "clip-rule",
    "opacity",
    "fill-opacity",
    "stroke-opacity",
    "transform",
];
const LINEAR_GRADIENT_ATTRIBUTES: &[&str] = &[
    "id",
    "x1",
    "y1",
    "x2",
    "y2",
    "gradientUnits",
    "gradientTransform",
];
const RADIAL_GRADIENT_ATTRIBUTES: &[&str] = &[
    "id",
    "cx",
    "cy",
    "r",
    "fx",
    "fy",
    "gradientUnits",
    "gradientTransform",
];
const STOP_ATTRIBUTES: &[&str] = &["offset", "stop-color", "stop-opacity"];

/// Checks a mark's bytes.
///
/// # Errors
///
/// Returns the first reason the file is not an admissible mark.
pub fn validate_mark_svg(contents: &[u8]) -> Result<(), String> {
    if contents.len() > MARK_MAX_BYTES {
        return Err(format!(
            "is {} bytes; a mark is at most {MARK_MAX_BYTES} bytes",
            contents.len()
        ));
    }
    let text = std::str::from_utf8(contents).map_err(|_| "is not UTF-8".to_owned())?;
    let trimmed = text.trim_start();
    if trimmed.starts_with("<?") {
        return Err(
            "starts with an XML declaration or processing instruction; a mark starts at <svg"
                .to_owned(),
        );
    }
    if text.contains("<![CDATA[") {
        return Err("contains CDATA".to_owned());
    }
    let document = roxmltree::Document::parse(text)
        .map_err(|error| format!("is not well-formed SVG: {error}"))?;
    let root = document.root_element();
    if root.tag_name().name() != "svg" || root.tag_name().namespace() != Some(SVG_NAMESPACE) {
        return Err(format!(
            "root element must be <svg xmlns=\"{SVG_NAMESPACE}\">"
        ));
    }
    if root
        .namespaces()
        .any(|namespace| namespace.uri() != SVG_NAMESPACE)
    {
        return Err("declares a namespace other than SVG".to_owned());
    }
    validate_viewbox(root)?;
    let gradient_ids = gradient_ids(&document)?;
    let mut elements = 0_usize;
    for node in root.descendants() {
        if node.is_comment() {
            return Err("contains a comment".to_owned());
        }
        if node.is_pi() {
            return Err("contains a processing instruction".to_owned());
        }
        if node.is_text() {
            if node.text().is_some_and(|text| !text.trim().is_empty()) {
                return Err("contains text; a mark is drawn with shapes only".to_owned());
            }
            continue;
        }
        if !node.is_element() {
            continue;
        }
        elements += 1;
        if elements > MAX_ELEMENTS {
            return Err(format!("has more than {MAX_ELEMENTS} elements"));
        }
        let depth = node.ancestors().filter(roxmltree::Node::is_element).count();
        if depth > MAX_DEPTH {
            return Err(format!("nests deeper than {MAX_DEPTH} levels"));
        }
        validate_element(node, &gradient_ids)?;
    }
    Ok(())
}

fn validate_viewbox(root: roxmltree::Node<'_, '_>) -> Result<(), String> {
    for attribute in root.attributes() {
        if attribute.namespace().is_some() || attribute.name() != "viewBox" {
            return Err(format!(
                "the <svg> element may carry only viewBox, not {}",
                attribute.name()
            ));
        }
    }
    let view_box = root
        .attribute("viewBox")
        .ok_or_else(|| "the <svg> element must carry viewBox=\"0 0 N N\"".to_owned())?;
    let parts = view_box.split_whitespace().collect::<Vec<_>>();
    let size = match parts.as_slice() {
        ["0", "0", width, height] if width == height => width.parse::<u32>().ok(),
        _ => None,
    };
    match size {
        Some(size) if (MIN_VIEWBOX..=MAX_VIEWBOX).contains(&size) => Ok(()),
        _ => Err(format!(
            "viewBox must be \"0 0 N N\": square, with an integer N from {MIN_VIEWBOX} to {MAX_VIEWBOX}"
        )),
    }
}

fn gradient_ids(document: &roxmltree::Document<'_>) -> Result<BTreeSet<String>, String> {
    let mut ids = BTreeSet::new();
    for node in document.descendants().filter(roxmltree::Node::is_element) {
        let Some(id) = node.attribute("id") else {
            continue;
        };
        if !matches!(node.tag_name().name(), "linearGradient" | "radialGradient") {
            return Err(format!(
                "<{}> may not carry an id; only gradients are named",
                node.tag_name().name()
            ));
        }
        if !is_identifier(id) {
            return Err(format!(
                "gradient id {id:?} must be a letter followed by letters, digits, '_' or '-'"
            ));
        }
        if !ids.insert(id.to_owned()) {
            return Err(format!("gradient id {id:?} is declared twice"));
        }
    }
    Ok(ids)
}

fn validate_element(
    node: roxmltree::Node<'_, '_>,
    gradient_ids: &BTreeSet<String>,
) -> Result<(), String> {
    let name = node.tag_name().name();
    if node.tag_name().namespace() != Some(SVG_NAMESPACE) {
        return Err(format!("<{name}> is outside the SVG namespace"));
    }
    let allowed: &[&str] = match name {
        "svg" if node.parent_element().is_none() => return Ok(()),
        "g" | "path" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon" => {
            SHAPE_ATTRIBUTES
        }
        "defs" => &[],
        "linearGradient" => LINEAR_GRADIENT_ATTRIBUTES,
        "radialGradient" => RADIAL_GRADIENT_ATTRIBUTES,
        "stop" => STOP_ATTRIBUTES,
        other => return Err(format!("<{other}> is not allowed in a mark")),
    };
    if name == "stop"
        && !node.parent_element().is_some_and(|parent| {
            matches!(
                parent.tag_name().name(),
                "linearGradient" | "radialGradient"
            )
        })
    {
        return Err("<stop> must sit inside a gradient".to_owned());
    }
    for attribute in node.attributes() {
        let attribute_name = attribute.name();
        if attribute.namespace().is_some() || !allowed.contains(&attribute_name) {
            return Err(format!("<{name}> may not carry {attribute_name}"));
        }
        validate_value(attribute_name, attribute.value(), gradient_ids)
            .map_err(|reason| format!("<{name}> {attribute_name}: {reason}"))?;
    }
    Ok(())
}

fn validate_value(name: &str, value: &str, gradient_ids: &BTreeSet<String>) -> Result<(), String> {
    match name {
        "id" => Ok(()),
        "fill" | "stroke" => paint(value, Some(gradient_ids)),
        "stop-color" => paint(value, None),
        "opacity" | "fill-opacity" | "stroke-opacity" | "stop-opacity" => unit_interval(value),
        "offset" => match value.strip_suffix('%') {
            Some(percent) => number(percent).and_then(|percent| {
                (0.0..=100.0)
                    .contains(&percent)
                    .then_some(())
                    .ok_or_else(|| "must be between 0% and 100%".to_owned())
            }),
            None => unit_interval(value),
        },
        "d" => path_data(value),
        "points" => number_list(value),
        "transform" | "gradientTransform" => transform(value),
        "stroke-linecap" => keyword(value, &["butt", "round", "square"]),
        "stroke-linejoin" => keyword(value, &["miter", "round", "bevel", "arcs", "miter-clip"]),
        "fill-rule" | "clip-rule" => keyword(value, &["nonzero", "evenodd"]),
        "gradientUnits" => keyword(value, &["userSpaceOnUse", "objectBoundingBox"]),
        _ => length(value),
    }
}

fn paint(value: &str, gradient_ids: Option<&BTreeSet<String>>) -> Result<(), String> {
    let value = value.trim();
    if matches!(value, "none" | "currentColor") || is_hex_color(value) {
        return Ok(());
    }
    if let (Some(ids), Some(reference)) = (
        gradient_ids,
        value
            .strip_prefix("url(#")
            .and_then(|rest| rest.strip_suffix(')')),
    ) {
        return if ids.contains(reference) {
            Ok(())
        } else {
            Err(format!("url(#{reference}) names no gradient in this file"))
        };
    }
    Err(format!(
        "{value:?} is not none, currentColor, a hex colour or url(#gradient)"
    ))
}

fn is_hex_color(value: &str) -> bool {
    value.strip_prefix('#').is_some_and(|hex| {
        matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn is_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
        && value.len() <= 64
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn number(value: &str) -> Result<f64, String> {
    let parsed = value
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("{value:?} is not a number"))?;
    if parsed.is_finite() && parsed.abs() <= MAX_COORDINATE {
        Ok(parsed)
    } else {
        Err(format!("{value:?} is out of range"))
    }
}

fn length(value: &str) -> Result<(), String> {
    number(value.trim().strip_suffix('%').unwrap_or(value)).map(drop)
}

fn unit_interval(value: &str) -> Result<(), String> {
    let parsed = number(value)?;
    (0.0..=1.0)
        .contains(&parsed)
        .then_some(())
        .ok_or_else(|| "must be between 0 and 1".to_owned())
}

fn keyword(value: &str, allowed: &[&str]) -> Result<(), String> {
    allowed
        .contains(&value.trim())
        .then_some(())
        .ok_or_else(|| format!("{value:?} is not one of {}", allowed.join(", ")))
}

fn number_list(value: &str) -> Result<(), String> {
    value
        .split(|character: char| character == ',' || character.is_whitespace())
        .filter(|part| !part.is_empty())
        .try_for_each(|part| number(part).map(drop))
}

fn path_data(value: &str) -> Result<(), String> {
    let allowed = |character: char| {
        character.is_ascii_digit()
            || character.is_whitespace()
            || matches!(character, '.' | ',' | '+' | '-' | 'e' | 'E')
            || "MmZzLlHhVvCcSsQqTtAa".contains(character)
    };
    if value.chars().all(allowed) {
        Ok(())
    } else {
        Err("path data may hold only commands, numbers, spaces and commas".to_owned())
    }
}

fn transform(value: &str) -> Result<(), String> {
    let mut rest = value.trim();
    if rest.is_empty() {
        return Err("is empty".to_owned());
    }
    while !rest.is_empty() {
        let open = rest
            .find('(')
            .ok_or_else(|| format!("{value:?} is not a list of transform functions"))?;
        let name = rest[..open].trim().trim_start_matches(',').trim();
        if !matches!(
            name,
            "matrix" | "translate" | "scale" | "rotate" | "skewX" | "skewY"
        ) {
            return Err(format!(
                "{name:?} is not matrix, translate, scale, rotate, skewX or skewY"
            ));
        }
        let close = rest[open..]
            .find(')')
            .map(|index| open + index)
            .ok_or_else(|| format!("{value:?} has an unclosed transform"))?;
        number_list(&rest[open + 1..close])?;
        rest = rest[close + 1..]
            .trim_start()
            .trim_start_matches(',')
            .trim_start();
    }
    Ok(())
}

#[cfg(test)]
mod tests;
