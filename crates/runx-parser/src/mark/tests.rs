use super::*;

fn svg(body: &str) -> String {
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">{body}</svg>"#)
}

#[test]
fn admits_shapes_transforms_and_gradients_defined_in_the_file() {
    let marks = [
        svg(r##"<path d="M4 4h16v16H4z" fill="#0f62fe"/>"##),
        svg(
            r#"<g transform="translate(2 2) scale(0.5)"><circle cx="12" cy="12" r="10" stroke="currentColor" stroke-width="1.5" fill="none"/></g>"#,
        ),
        svg(
            r##"<defs><linearGradient id="brand" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#ff5500"/><stop offset="100%" stop-color="#3300ff" stop-opacity="0.8"/></linearGradient></defs><rect width="24" height="24" rx="4" fill="url(#brand)"/>"##,
        ),
        "\n  ".to_owned() + &svg(r#"<polygon points="12,2 22,22 2,22" fill-rule="evenodd"/>"#),
    ];
    for mark in marks {
        assert_eq!(validate_mark_svg(mark.as_bytes()), Ok(()), "{mark}");
    }
}

#[test]
fn refuses_anything_that_can_run_fetch_embed_or_style() {
    let refused = [
        ("script", svg("<script>alert(1)</script>")),
        (
            "foreign object",
            svg("<foreignObject><div/></foreignObject>"),
        ),
        (
            "event attribute",
            svg(r#"<path d="M0 0" onload="alert(1)"/>"#),
        ),
        (
            "style attribute",
            svg(r#"<path d="M0 0" style="fill:red"/>"#),
        ),
        ("style element", svg("<style>path{fill:red}</style>")),
        ("class", svg(r#"<path d="M0 0" class="a"/>"#)),
        ("image", svg(r#"<image width="24" height="24"/>"#)),
        ("use", svg("<use/>")),
        ("filter", svg("<filter/>")),
        (
            "external paint",
            svg(r#"<path d="M0 0" fill="url(https://example.com/x.svg#a)"/>"#),
        ),
        (
            "data paint",
            svg(r#"<path d="M0 0" fill="url(data:image/svg+xml,x)"/>"#),
        ),
        (
            "missing gradient",
            svg(r#"<path d="M0 0" fill="url(#nope)"/>"#),
        ),
        ("id on a shape", svg(r#"<path id="p" d="M0 0"/>"#)),
        ("text", svg("hello")),
        ("comment", svg("<!-- hi --><path d=\"M0 0\"/>")),
        ("cdata", svg("<![CDATA[x]]>")),
        ("path injection", svg(r#"<path d="M0 0 url(x)"/>"#)),
        (
            "transform function",
            svg(r#"<g transform="perspective(1)"/>"#),
        ),
    ];
    for (case, mark) in refused {
        assert!(
            validate_mark_svg(mark.as_bytes()).is_err(),
            "{case} was admitted: {mark}"
        );
    }
}

#[test]
fn refuses_documents_that_are_not_one_plain_square_svg() {
    let refused = [
        ("xml declaration", r#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"/>"#.to_owned()),
        ("doctype", r#"<!DOCTYPE svg [<!ENTITY a "b">]><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"/>"#.to_owned()),
        ("no namespace", r#"<svg viewBox="0 0 24 24"/>"#.to_owned()),
        ("xlink namespace", r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 24 24"/>"#.to_owned()),
        ("non-square", r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 12"/>"#.to_owned()),
        ("offset viewBox", r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="1 0 24 24"/>"#.to_owned()),
        ("too small", r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"/>"#.to_owned()),
        ("too large", r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 4096 4096"/>"#.to_owned()),
        ("root size", r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="24"/>"#.to_owned()),
        ("not svg", r#"<html xmlns="http://www.w3.org/2000/svg"/>"#.to_owned()),
        ("too many elements", svg(&"<g/>".repeat(MAX_ELEMENTS + 1))),
        ("too deep", svg(&format!("{}{}", "<g>".repeat(MAX_DEPTH + 1), "</g>".repeat(MAX_DEPTH + 1)))),
        ("oversized", svg(&format!(r#"<path d="M0 0{}"/>"#, " 1".repeat(MARK_MAX_BYTES / 2)))),
    ];
    for (case, mark) in refused {
        assert!(
            validate_mark_svg(mark.as_bytes()).is_err(),
            "{case} was admitted"
        );
    }
    assert!(validate_mark_svg(&[0xff, 0xfe]).is_err());
}
