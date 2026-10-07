pub(crate) fn truncate_utf8_bytes(text: &str, maximum: usize) -> String {
    if text.len() <= maximum {
        return text.to_owned();
    }
    let mut end = maximum;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::truncate_utf8_bytes;

    #[test]
    fn utf8_truncation_never_splits_a_character() {
        assert_eq!(truncate_utf8_bytes("ab🙂cd", 5), "ab");
        assert_eq!(truncate_utf8_bytes("ab🙂cd", 6), "ab🙂");
    }
}
