pub fn choose_dispatch<'a>(value: &'a str, preferred: Option<&'a str>, attempt: u32) -> &'a str {
    if attempt == 0 {
        preferred.unwrap_or(value)
    } else {
        value
    }
}
