//! Canonical, run-local boundary for model-produced tool invocations.
//!
//! Providers do not agree on tool-call wire shapes, and the Anthropic OAuth
//! path additionally presents a small set of tools with Claude Code casing and
//! argument names. Tongs folds those wire shapes into [`ToolCall`], but this
//! boundary remains authoritative: it is derived from the finalized registry,
//! publishes closed schemas, applies only reviewed aliases, and validates a
//! call before conversation retention, policy, observability, or dispatch.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};
use tongs::model::{AssistantMessage, ContentBlock, ToolCall};
use tongs::provider::ToolDef;
use tongs::tools::{ToolEffects, ToolRegistry};

use crate::machine::{ToolFailureDiagnostic, ToolFailureReason};

/// Safe placeholder retained for a rejected call. Supplied names and argument
/// values never enter conversation history, previews, telemetry, or dispatch.
pub const REJECTED_TOOL_NAME: &str = "invalid_tool_invocation";

/// Failure to assemble one unambiguous catalog from the finalized registry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvocationCatalogError {
    DuplicateCanonicalName,
    ProviderAliasCollision,
}

impl std::fmt::Display for InvocationCatalogError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateCanonicalName => {
                formatter.write_str("final tool registry contains a duplicate canonical name")
            }
            Self::ProviderAliasCollision => {
                formatter.write_str("final tool registry contains an ambiguous provider tool alias")
            }
        }
    }
}

impl std::error::Error for InvocationCatalogError {}

#[derive(Clone, Debug)]
struct InvocationSpec {
    definition: ToolDef,
    effects: ToolEffects,
}

/// Result of canonicalizing one provider-decoded call.
pub struct CanonicalInvocation {
    pub call: ToolCall,
    /// Present when the call must settle locally without consulting the tool
    /// registry. The call itself has already been scrubbed in this case.
    pub rejection: Option<ToolFailureDiagnostic>,
}

/// Registry-derived invocation contract used for definitions, effects, and
/// every runtime call decision.
#[derive(Clone, Debug)]
pub struct ToolInvocationCatalog {
    entries: BTreeMap<String, InvocationSpec>,
    definitions: Vec<ToolDef>,
    effects: BTreeMap<String, ToolEffects>,
    /// Legacy machine constructors remain intentionally permissive for pure
    /// scheduler tests. Production always uses [`Self::from_registry`].
    enforce: bool,
}

impl ToolInvocationCatalog {
    /// Builds the production catalog from the exact finalized registry.
    pub fn from_registry(registry: &ToolRegistry) -> Result<Self, InvocationCatalogError> {
        let mut entries = BTreeMap::new();
        let mut definitions = Vec::new();
        let mut folded_names = BTreeSet::new();
        for tool in registry.tools() {
            let name = tool.name().to_string();
            if entries.contains_key(&name) {
                return Err(InvocationCatalogError::DuplicateCanonicalName);
            }
            // Anthropic OAuth maps its reviewed names case-insensitively. A
            // registry containing two case-fold-equivalent names would make
            // that provider mapping order-dependent, so reject it up front.
            if !folded_names.insert(name.to_ascii_lowercase()) {
                return Err(InvocationCatalogError::ProviderAliasCollision);
            }
            let definition = ToolDef {
                name: name.clone(),
                description: tool.description().to_string(),
                parameters: canonical_schema(&name, tool.parameters()),
            };
            definitions.push(definition.clone());
            entries.insert(
                name,
                InvocationSpec {
                    definition,
                    effects: tool.effects(),
                },
            );
        }
        let effects = entries
            .iter()
            .map(|(name, spec)| (name.clone(), spec.effects))
            .collect();
        Ok(Self {
            entries,
            definitions,
            effects,
            enforce: true,
        })
    }

    /// Compatibility catalog for direct `AgentMachine::with_effects` users.
    pub(crate) fn permissive(effects: BTreeMap<String, ToolEffects>) -> Self {
        Self {
            entries: BTreeMap::new(),
            definitions: Vec::new(),
            effects,
            enforce: false,
        }
    }

    pub fn definitions(&self) -> Vec<ToolDef> {
        self.definitions.clone()
    }

    pub fn effects(&self) -> &BTreeMap<String, ToolEffects> {
        &self.effects
    }

    pub fn schema(&self, name: &str) -> Option<&Value> {
        self.entries
            .get(name)
            .map(|spec| &spec.definition.parameters)
    }

    /// Returns the canonical name safe for live tool-call telemetry. Unknown,
    /// ambiguous, and unavailable names collapse to a constant placeholder.
    pub fn telemetry_name(&self, api: &str, supplied: &str) -> String {
        if !self.enforce {
            return supplied.to_string();
        }
        self.resolve_name(api, supplied)
            .unwrap_or(REJECTED_TOOL_NAME)
            .to_string()
    }

    /// Canonicalizes and validates one decoded provider call. No fuzzy
    /// matching is performed. The only name variants are the exact Claude Code
    /// spellings tongs publishes for Anthropic OAuth; the only key variants
    /// are the reviewed native filesystem keys below.
    pub fn canonicalize(&self, api: &str, mut call: ToolCall) -> CanonicalInvocation {
        if !self.enforce {
            return CanonicalInvocation {
                call,
                rejection: None,
            };
        }
        let Some(canonical_name) = self.resolve_name(api, &call.name).map(str::to_string) else {
            return rejected(call, ToolFailureReason::UnknownTool);
        };
        call.name = canonical_name.clone();
        if normalize_arguments(api, &canonical_name, &mut call.arguments).is_err()
            || !arguments_match(
                &self
                    .entries
                    .get(&canonical_name)
                    .expect("resolved catalog entry")
                    .definition
                    .parameters,
                &call.arguments,
            )
        {
            return rejected(call, ToolFailureReason::InvalidArguments);
        }
        CanonicalInvocation {
            call,
            rejection: None,
        }
    }

    /// Canonicalizes every tool block in one assistant turn and returns typed
    /// local failures keyed by provider call id.
    pub(crate) fn canonicalize_message(
        &self,
        assistant: &mut AssistantMessage,
    ) -> BTreeMap<String, ToolFailureDiagnostic> {
        let mut rejections = BTreeMap::new();
        let api = assistant.api.clone();
        for block in &mut assistant.content {
            let ContentBlock::ToolCall(call) = block else {
                continue;
            };
            let normalized = self.canonicalize(&api, call.clone());
            *call = normalized.call;
            if let Some(rejection) = normalized.rejection {
                rejections.insert(call.id.clone(), rejection);
            }
        }
        rejections
    }

    fn resolve_name(&self, api: &str, supplied: &str) -> Option<&str> {
        if let Some((canonical, _)) = self.entries.get_key_value(supplied) {
            return Some(canonical);
        }
        if api != "anthropic-messages" {
            return None;
        }
        // Exact spellings from tongs' pinned CLAUDE_CODE_TOOLS mapping. Do not
        // infer `Glob`→find, `Task`→a sub-agent, or case variants: those names
        // are not semantically equivalent or safely unambiguous.
        let canonical = match supplied {
            "Read" => "read",
            "Write" => "write",
            "Edit" => "edit",
            "Bash" => "bash",
            "Grep" => "grep",
            _ => return None,
        };
        self.entries.contains_key(canonical).then_some(canonical)
    }
}

fn rejected(mut call: ToolCall, reason: ToolFailureReason) -> CanonicalInvocation {
    call.name = REJECTED_TOOL_NAME.to_string();
    call.arguments = Value::Object(Map::new());
    CanonicalInvocation {
        call,
        rejection: Some(ToolFailureDiagnostic::schema(reason)),
    }
}

fn normalize_arguments(api: &str, name: &str, arguments: &mut Value) -> Result<(), ()> {
    let object = arguments.as_object_mut().ok_or(())?;
    // Codebase-memory deliberately documents `project` and `repo` as aliases;
    // supplying both is competing input rather than a value to prioritize.
    if object.contains_key("project") && object.contains_key("repo") {
        return Err(());
    }
    if api != "anthropic-messages" {
        return Ok(());
    }
    match name {
        "read" | "write" => rename_unambiguous(object, "file_path", "path"),
        "edit" => {
            rename_unambiguous(object, "file_path", "path")?;
            let old = object.remove("old_string");
            let new = object.remove("new_string");
            match (old, new) {
                (None, None) => Ok(()),
                (Some(old), Some(new))
                    if !object.contains_key("edits") && old.is_string() && new.is_string() =>
                {
                    object.insert(
                        "edits".to_string(),
                        serde_json::json!([{ "oldText": old, "newText": new }]),
                    );
                    Ok(())
                }
                _ => Err(()),
            }
        }
        _ => Ok(()),
    }
}

fn rename_unambiguous(
    object: &mut Map<String, Value>,
    alias: &str,
    canonical: &str,
) -> Result<(), ()> {
    let Some(value) = object.remove(alias) else {
        return Ok(());
    };
    if object.contains_key(canonical) {
        return Err(());
    }
    object.insert(canonical.to_string(), value);
    Ok(())
}

/// Produces the exact provider/runtime schema. Every object is closed,
/// including objects nested below arrays/combinators. The pinned tongs coding
/// tools advertise count fields as `number` even though their serde inputs are
/// unsigned integers; those known fields are corrected here at Temper's
/// boundary rather than changing unrelated provider schemas.
fn canonical_schema(name: &str, mut schema: Value) -> Value {
    if !schema.is_object() {
        schema = serde_json::json!({"type": "object", "properties": {}});
    }
    if let Some(root) = schema.as_object_mut() {
        root.insert("type".to_string(), Value::String("object".to_string()));
        root.entry("properties".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    for field in match name {
        "read" => &["offset", "limit"][..],
        "ls" | "find" => &["limit"][..],
        "grep" => &["context", "limit"][..],
        "bash" => &["timeout"][..],
        _ => &[],
    } {
        if let Some(field_schema) = schema.pointer_mut(&format!("/properties/{field}")) {
            if let Some(field_schema) = field_schema.as_object_mut() {
                field_schema.insert("type".to_string(), Value::String("integer".to_string()));
                field_schema.insert("minimum".to_string(), Value::from(0));
            }
        }
    }
    if name == "edit" {
        if let Some(edits) = schema
            .pointer_mut("/properties/edits")
            .and_then(Value::as_object_mut)
        {
            edits.insert("minItems".to_string(), Value::from(1));
        }
    }
    close_objects(&mut schema);
    schema
}

fn close_objects(schema: &mut Value) {
    let Some(object) = schema.as_object_mut() else {
        return;
    };
    let is_object = object.get("type").and_then(Value::as_str) == Some("object")
        || object.contains_key("properties");
    if is_object {
        object.insert("additionalProperties".to_string(), Value::Bool(false));
    }
    if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
        for property in properties.values_mut() {
            close_objects(property);
        }
    }
    if let Some(items) = object.get_mut("items") {
        close_objects(items);
    }
    for keyword in ["allOf", "anyOf", "oneOf"] {
        if let Some(branches) = object.get_mut(keyword).and_then(Value::as_array_mut) {
            for branch in branches {
                close_objects(branch);
            }
        }
    }
}

/// Closed JSON-schema subset used by every ordinary coding tool and the MCP
/// schemas admitted into the finalized registry. Unknown schema annotations
/// are ignored; all structural and scalar constraints used by these tools are
/// enforced.
pub fn arguments_match(schema: &Value, value: &Value) -> bool {
    if schema
        .get("enum")
        .and_then(Value::as_array)
        .is_some_and(|values| !values.contains(value))
        || schema
            .get("const")
            .is_some_and(|expected| expected != value)
    {
        return false;
    }
    if schema
        .get("allOf")
        .and_then(Value::as_array)
        .is_some_and(|branches| !branches.iter().all(|branch| arguments_match(branch, value)))
    {
        return false;
    }
    if schema
        .get("anyOf")
        .and_then(Value::as_array)
        .is_some_and(|branches| !branches.iter().any(|branch| arguments_match(branch, value)))
    {
        return false;
    }
    if schema
        .get("oneOf")
        .and_then(Value::as_array)
        .is_some_and(|branches| {
            branches
                .iter()
                .filter(|branch| arguments_match(branch, value))
                .count()
                != 1
        })
    {
        return false;
    }

    match schema.get("type").and_then(Value::as_str) {
        Some("object") => object_matches(schema, value),
        Some("array") => array_matches(schema, value),
        Some("string") => string_matches(schema, value),
        Some("integer") => integer_matches(schema, value),
        Some("number") => number_matches(schema, value),
        Some("boolean") => value.is_boolean(),
        Some("null") => value.is_null(),
        Some(_) => false,
        None => true,
    }
}

fn object_matches(schema: &Value, value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if schema
        .get("required")
        .and_then(Value::as_array)
        .is_some_and(|required| {
            required
                .iter()
                .any(|key| key.as_str().is_none_or(|key| !object.contains_key(key)))
        })
    {
        return false;
    }
    let properties = schema.get("properties").and_then(Value::as_object);
    object.iter().all(
        |(key, value)| match properties.and_then(|properties| properties.get(key)) {
            Some(property) => arguments_match(property, value),
            None => match schema.get("additionalProperties") {
                Some(Value::Bool(false)) => false,
                Some(additional) if additional.is_object() => arguments_match(additional, value),
                _ => true,
            },
        },
    )
}

fn array_matches(schema: &Value, value: &Value) -> bool {
    let Some(values) = value.as_array() else {
        return false;
    };
    let length = values.len() as u64;
    if schema
        .get("minItems")
        .and_then(Value::as_u64)
        .is_some_and(|min| length < min)
        || schema
            .get("maxItems")
            .and_then(Value::as_u64)
            .is_some_and(|max| length > max)
        || schema.get("uniqueItems").and_then(Value::as_bool) == Some(true)
            && values
                .iter()
                .enumerate()
                .any(|(index, value)| values[..index].contains(value))
    {
        return false;
    }
    schema
        .get("items")
        .is_none_or(|items| values.iter().all(|value| arguments_match(items, value)))
}

fn string_matches(schema: &Value, value: &Value) -> bool {
    let Some(value) = value.as_str() else {
        return false;
    };
    let length = value.chars().count() as u64;
    !schema
        .get("minLength")
        .and_then(Value::as_u64)
        .is_some_and(|min| length < min)
        && !schema
            .get("maxLength")
            .and_then(Value::as_u64)
            .is_some_and(|max| length > max)
}

fn integer_matches(schema: &Value, value: &Value) -> bool {
    let Some(value) = value
        .as_i64()
        .map(|value| value as f64)
        .or_else(|| value.as_u64().map(|value| value as f64))
    else {
        return false;
    };
    numeric_bounds_match(schema, value)
}

fn number_matches(schema: &Value, value: &Value) -> bool {
    value
        .as_f64()
        .is_some_and(|value| numeric_bounds_match(schema, value))
}

fn numeric_bounds_match(schema: &Value, value: f64) -> bool {
    !schema
        .get("minimum")
        .and_then(Value::as_f64)
        .is_some_and(|min| value < min)
        && !schema
            .get("maximum")
            .and_then(Value::as_f64)
            .is_some_and(|max| value > max)
        && !schema
            .get("exclusiveMinimum")
            .and_then(Value::as_f64)
            .is_some_and(|min| value <= min)
        && !schema
            .get("exclusiveMaximum")
            .and_then(Value::as_f64)
            .is_some_and(|max| value >= max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use tongs::tools::{Tool, ToolOutput, ToolUpdate};

    struct ContractTool(&'static str, Value, ToolEffects);

    #[async_trait]
    impl Tool for ContractTool {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            "contract fixture"
        }
        fn parameters(&self) -> Value {
            self.1.clone()
        }
        fn effects(&self) -> ToolEffects {
            self.2
        }
        async fn execute(
            &self,
            _: &str,
            _: Value,
            _: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
        ) -> tongs::Result<ToolOutput> {
            unreachable!("catalog tests never execute registry tools")
        }
    }

    fn registry(tools: Vec<ContractTool>) -> ToolRegistry {
        ToolRegistry::from_tools(
            tools
                .into_iter()
                .map(|tool| Box::new(tool) as Box<dyn Tool>)
                .collect(),
        )
    }

    #[test]
    fn closes_nested_schemas_and_enforces_constraints() {
        let registry = registry(vec![ContractTool(
            "edit",
            serde_json::json!({
                "type":"object", "properties":{"edits":{"type":"array", "items":{
                    "type":"object", "properties":{"oldText":{"type":"string"},"newText":{"type":"string"}},
                    "required":["oldText","newText"]}}}, "required":["edits"]
            }),
            ToolEffects::write(),
        )]);
        let catalog = ToolInvocationCatalog::from_registry(&registry).unwrap();
        let schema = catalog.schema("edit").unwrap();
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(
            schema["properties"]["edits"]["items"]["additionalProperties"],
            false
        );
        assert_eq!(schema["properties"]["edits"]["minItems"], 1);
        assert!(!arguments_match(schema, &serde_json::json!({"edits":[]})));
        assert!(!arguments_match(
            schema,
            &serde_json::json!({"edits":[{"oldText":"a","newText":"b","secret":"x"}]})
        ));
    }

    #[test]
    fn rejects_case_folded_registry_collisions() {
        let result = ToolInvocationCatalog::from_registry(&registry(vec![
            ContractTool(
                "read",
                serde_json::json!({"type":"object"}),
                ToolEffects::read(),
            ),
            ContractTool(
                "Read",
                serde_json::json!({"type":"object"}),
                ToolEffects::write(),
            ),
        ]));
        assert_eq!(
            result.unwrap_err(),
            InvocationCatalogError::ProviderAliasCollision
        );
    }
}
