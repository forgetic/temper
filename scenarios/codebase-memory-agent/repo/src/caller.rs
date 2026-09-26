pub fn dispatch<'a>(value: &'a str, preferred: Option<&'a str>, attempt: u32) -> &'a str {
    crate::retry_worker_topic(value, preferred, attempt)
}
