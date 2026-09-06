pub fn dispatch<'a>(value: &'a str, preferred: Option<&'a str>, attempt: u32) -> &'a str {
    crate::choose_dispatch(value, preferred, attempt)
}
