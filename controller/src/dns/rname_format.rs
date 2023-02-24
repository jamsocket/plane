use anyhow::{Context, Result};

pub fn format_rname(email: &str) -> Result<String> {
    let (pre, post) = email
        .split_once('@')
        .context("Email address must include '@'.")?;
    let pre = pre.replace('.', r"\.");

    Ok(format!("{}.{}", pre, post))
}

#[cfg(test)]
mod test {
    use super::format_rname;

    #[test]
    fn test_rname() {
        assert_eq!("foo.bar.com", &format_rname("foo@bar.com").unwrap());
        assert_eq!(
            r"foo\.bar.baz.com",
            &format_rname("foo.bar@baz.com").unwrap()
        );

        assert!(format_rname("foo.bar.com").is_err());
    }
}
