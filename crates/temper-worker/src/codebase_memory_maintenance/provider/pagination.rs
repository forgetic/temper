use serde_json::{Map, Value, json};

pub(super) fn arguments(cursor: Option<&str>, limit: u32) -> Result<Value, String> {
    let offset = cursor
        .map_or(Ok(0), |value| value.parse::<u64>())
        .map_err(|_| "provider inventory offset was invalid")?;
    Ok(json!({"offset": offset, "limit": limit, "include_details": true}))
}

pub(super) fn next_cursor(object: &Map<String, Value>) -> Result<Option<String>, String> {
    let more = object
        .get("has_more")
        .and_then(Value::as_bool)
        .ok_or("provider inventory omitted has_more")?;
    if !more {
        return Ok(None);
    }
    let offset = object
        .get("offset")
        .and_then(Value::as_u64)
        .ok_or("provider inventory omitted a valid offset")?;
    let limit = object
        .get("limit")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or("provider inventory omitted a positive limit")?;
    let next = offset
        .checked_add(limit)
        .ok_or("provider inventory offset overflow")?;
    Ok(Some(next.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_pages_advance_even_when_unreadable_projects_are_omitted() {
        let page = json!({"offset": 50, "limit": 50, "returned": 48, "has_more": true});
        assert_eq!(
            next_cursor(page.as_object().unwrap()).unwrap(),
            Some("100".to_string())
        );
        assert_eq!(
            arguments(Some("100"), 50).unwrap(),
            json!({"offset":100,"limit":50,"include_details":true})
        );
        assert!(arguments(Some("page-2"), 50).is_err());
    }

    #[test]
    fn ambiguous_pagination_cannot_claim_a_complete_inventory() {
        for page in [
            json!({}),
            json!({"has_more":true,"offset":0,"limit":0}),
            json!({"has_more":true,"offset":u64::MAX,"limit":1}),
        ] {
            assert!(next_cursor(page.as_object().unwrap()).is_err());
        }
    }
}
