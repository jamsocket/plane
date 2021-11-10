pub fn whitespace(mut st: &[u8]) -> Option<(&[u8], ())> {
    if let Some(first) = st.first() {
        if *first != b' ' && *first != b'\t' {
            // Expected whitespace, got something else.
            return None;
        }
    } else {
        return None;
    }
    st = &st[1..];

    while st
        .first()
        .map(|d| d.is_ascii_whitespace())
        .unwrap_or_default()
    {
        st = &st[1..];
    }

    Some((st, ()))
}

pub fn hex_u8(st: &[u8]) -> Option<(&[u8], u8)> {
    if st.len() < 2 {
        return None;
    }

    let (byte, rest) = st.split_at(2);

    if let Ok(result) = u8::from_str_radix(std::str::from_utf8(&byte).ok()?, 16) {
        Some((rest, result))
    } else {
        None
    }
}

pub fn hex_u16(st: &[u8]) -> Option<(&[u8], u16)> {
    if st.len() < 4 {
        return None;
    }

    let (byte, rest) = st.split_at(4);

    if let Ok(result) = u16::from_str_radix(std::str::from_utf8(&byte).ok()?, 16) {
        Some((rest, result))
    } else {
        None
    }
}

pub fn colon(st: &[u8]) -> Option<(&[u8], ())> {
    if let Some(first) = st.first() {
        if *first == b':' {
            return Some((&st[1..], ()));
        }
    }

    None
}

pub fn consume_until_newline(mut st: &[u8]) -> Option<(&[u8], ())> {
    while !st.is_empty() {
        if st[0] == b'\n' {
            return Some((&st[1..], ()));
        }

        st = &st[1..];
    }

    Some((&[], ()))
}

pub fn decimal(st: &[u8]) -> Option<(&[u8], u32)> {
    let mut ix = 0;
    while st.get(ix).map(|d| d.is_ascii_digit()).unwrap_or_default() {
        ix += 1;
    }

    if ix == 0 {
        None
    } else {
        let result = u32::from_str_radix(std::str::from_utf8(&st[0..ix]).ok()?, 10).ok()?;
        Some((&st[ix..], result))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_parse_decimal() {
        assert_eq!(None, decimal(b",123"));
        assert_eq!(None, decimal(b" 0A f"));
        assert_eq!(None, decimal(b""));

        assert_eq!(Some((&b":456"[..], 123)), decimal(b"123:456"));
    }

    #[test]
    fn test_consume_until_newline() {
        assert_eq!(Some((&[][..], ())), consume_until_newline(b""));
        assert_eq!(
            Some((&b"bar\nbaz"[..], ())),
            consume_until_newline(b"foo\nbar\nbaz")
        );
    }

    #[test]
    fn test_colon() {
        assert_eq!(None, colon(b""));
        assert_eq!(None, colon(b"/"));
        assert_eq!(Some((&b"foo"[..], ())), colon(b":foo"));
    }

    #[test]
    fn test_hex_u8() {
        assert_eq!(None, hex_u8(b",0A f"));
        assert_eq!(None, hex_u8(b" 0A f"));
        assert_eq!(None, hex_u8(b""));

        assert_eq!(Some((&b"9999"[..], 0x0A)), hex_u8(b"0A9999"));
    }

    #[test]
    fn test_hex_u16() {
        assert_eq!(None, hex_u16(b",0A f"));
        assert_eq!(None, hex_u16(b" 0A f"));
        assert_eq!(None, hex_u16(b"0AA f"));
        assert_eq!(None, hex_u16(b""));

        assert_eq!(Some((&b"99"[..], 0x0A99)), hex_u16(b"0A9999"));
    }

    #[test]
    fn test_whitespace() {
        assert_eq!(None, whitespace(b""));

        assert_eq!(None, whitespace(b"blah"));

        assert_eq!(Some((&b"foo bar"[..], ())), whitespace(b" foo bar"));

        assert_eq!(Some((&b"foo bar"[..], ())), whitespace(b"  foo bar"));

        assert_eq!(Some((&b"foo bar"[..], ())), whitespace(b"\t \tfoo bar"));
    }
}
