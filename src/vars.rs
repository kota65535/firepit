//! Types and JSON Schema validation of typed variables.
//!
//! A typed variable declaration is a JSON Schema: its `type` plus any other keyword written next
//! to it (`enum`, `pattern`, `minimum`, `items`, ...). This module converts a value to the declared
//! type and delegates the validation to the `jsonschema` crate.

use anyhow::Context;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// Extra JSON Schema keywords of a variable declaration, as written in the config.
pub type VarSchema = serde_json::Map<String, JsonValue>;

/// Builds the JSON Schema of a variable: its `type` plus the extra keywords.
fn var_schema(ty: VarType, schema: &VarSchema) -> JsonValue {
    let mut full = schema.clone();
    full.insert("type".to_string(), JsonValue::from(ty.as_str()));
    JsonValue::Object(full)
}

/// Checks a variable declaration's schema at config load time.
///
/// # Errors
///
/// Returns an error when a keyword is unknown, `type` is missing, or the schema is not a valid
/// JSON Schema (ex: `minimum: "abc"`, an invalid `pattern`).
pub fn validate_var_declaration(ty: Option<VarType>, schema: &VarSchema) -> anyhow::Result<()> {
    if schema.is_empty() {
        return Ok(());
    }
    check_known_keywords(&JsonValue::Object(schema.clone()), "")?;
    let ty = ty.context("JSON Schema keywords require `type`")?;
    let full = var_schema(ty, schema);
    jsonschema::meta::validate(&full).map_err(|e| anyhow::anyhow!("invalid schema: {}", e))?;
    jsonschema::validator_for(&full).map_err(|e| anyhow::anyhow!("invalid schema: {}", e))?;
    Ok(())
}

/// Checks that every keyword of `schema`, and of the subschemas it applies (`items`, `properties`,
/// `anyOf`, ...), is one the validator knows (JSON Schema 2020-12), so that a typo does not pass
/// silently. Annotation keywords such as `title` or `description` are not accepted either.
/// `path` locates the schema in the declaration, for the error message.
fn check_known_keywords(schema: &JsonValue, path: &str) -> anyhow::Result<()> {
    let draft = jsonschema::Draft::Draft202012;
    for key in schema.as_object().into_iter().flat_map(|o| o.keys()) {
        anyhow::ensure!(
            draft.is_known_keyword(key),
            "unknown keyword {:?}",
            format!("{}{}", path, key)
        );
    }
    for sub in draft.subresources_of(schema) {
        let sub_path = subschema_path(schema, sub)
            .map(|p| format!("{}{}.", path, p))
            .unwrap_or_default();
        check_known_keywords(sub, &sub_path)?;
    }
    Ok(())
}

/// Locates `sub`, a subschema yielded by `subresources_of(schema)`, in `schema`: it is the value
/// of a keyword, or an element of an array or map that is. Returns the path such as `items`,
/// `anyOf.1` or `properties.name`.
fn subschema_path(schema: &JsonValue, sub: &JsonValue) -> Option<String> {
    for (key, value) in schema.as_object()? {
        if std::ptr::eq(value, sub) {
            return Some(key.clone());
        }
        if let Some(i) = value
            .as_array()
            .and_then(|a| a.iter().position(|v| std::ptr::eq(v, sub)))
        {
            return Some(format!("{}.{}", key, i));
        }
        if let Some((name, _)) = value
            .as_object()
            .and_then(|o| o.iter().find(|(_, v)| std::ptr::eq(*v, sub)))
        {
            return Some(format!("{}.{}", key, name));
        }
    }
    None
}

/// Checks a value (already converted to `ty`) against the variable's JSON Schema.
///
/// # Errors
///
/// Returns an error listing the violated constraints.
pub fn check_var_value(ty: VarType, schema: &VarSchema, value: &JsonValue) -> anyhow::Result<()> {
    if schema.is_empty() {
        return Ok(());
    }
    // ponytail: the schema is compiled on every check; cache it if vars get numerous
    let validator = jsonschema::validator_for(&var_schema(ty, schema)).map_err(|e| anyhow::anyhow!("{}", e))?;
    let errors = validator.iter_errors(value).map(|e| e.to_string()).collect::<Vec<_>>();
    anyhow::ensure!(errors.is_empty(), "{}", errors.join("; "));
    Ok(())
}

/// Variable types, following JSON Schema
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VarType {
    String,
    Number,
    Integer,
    Boolean,
    Array,
    Object,
}

impl VarType {
    pub fn as_str(&self) -> &'static str {
        match self {
            VarType::String => "string",
            VarType::Number => "number",
            VarType::Integer => "integer",
            VarType::Boolean => "boolean",
            VarType::Array => "array",
            VarType::Object => "object",
        }
    }

    fn matches(&self, value: &JsonValue) -> bool {
        match self {
            VarType::String => value.is_string(),
            VarType::Number => value.is_number(),
            VarType::Integer => value.is_i64() || value.is_u64(),
            VarType::Boolean => value.is_boolean(),
            VarType::Array => value.is_array(),
            VarType::Object => value.is_object(),
        }
    }

    /// Interprets `value` as this type: a string given for a non-string type (ex: the CLI
    /// argument `list="[a, b]"`) is parsed as YAML, then the value is checked against the type.
    ///
    /// # Errors
    ///
    /// Returns an error when the string cannot be parsed or the value does not match the type.
    pub fn coerce(&self, value: JsonValue) -> anyhow::Result<JsonValue> {
        let value = match value {
            JsonValue::String(s) if *self != VarType::String => serde_yaml::from_str::<JsonValue>(&s)
                .with_context(|| format!("failed to read {:?} as {}", s, self.as_str()))?,
            v => v,
        };
        anyhow::ensure!(
            self.matches(&value),
            "expected {}, got {}",
            self.as_str(),
            json_type_name(&value)
        );
        Ok(value)
    }
}

fn json_type_name(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "boolean",
        JsonValue::Number(_) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) => "array",
        JsonValue::Object(_) => "object",
    }
}
