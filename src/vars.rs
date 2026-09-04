//! Template variables: their declarations, types, and JSON Schema validation.
//!
//! A variable is a scalar value, a typed declaration or a dynamic variable, see [`VarsConfig`].
//! A typed declaration is a JSON Schema: its `type` plus any other keyword written next to it
//! (`enum`, `pattern`, `minimum`, `items`, ...). This module converts a value to the declared type
//! and delegates the validation to the `jsonschema` crate.

use crate::config::{absolute_or_join, deserialize_hash_map, ShellConfig};
use anyhow::Context;
use indexmap::IndexMap;
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{de, Deserialize, Deserializer, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Vars config
///
/// A variable is one of:
/// - a scalar value (`foo: bar`), whose type is inferred from the value
/// - a typed declaration (`foo: { type: array, default: [a, b] }`), see [`TypedVars`]
/// - a dynamic variable (`foo: { command: ... }`), see [`DynamicVars`]
///
/// Array and object values are only accepted as the `default` of a typed declaration, so that an
/// object is never ambiguous between a value and a declaration.
///
/// An object with `command` is dynamic (tried first, so `{ type, command }` is a typed dynamic
/// variable), otherwise an object with `type` is a typed declaration.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(
    untagged,
    expecting = "a scalar value, a typed declaration with `type` (and optionally `default`), or a dynamic variable with `command`"
)]
pub enum VarsConfig {
    Dynamic(Box<DynamicVars>),
    Typed(TypedVars),
    Static(
        #[serde(deserialize_with = "deserialize_scalar")]
        #[schemars(schema_with = "scalar_schema")]
        JsonValue,
    ),
}

impl VarsConfig {
    /// Returns whether the variable is declared without a value, ex: `foo:` or
    /// `foo: { type: string }`.
    /// Such a variable has no default, so it is required: it must be given a value before the
    /// task runs, by the `<name>=<value>` CLI argument or the dependent task's `depends_on.vars`.
    pub fn is_unset(&self) -> bool {
        match self {
            VarsConfig::Static(JsonValue::Null) => true,
            VarsConfig::Typed(t) => t.default.as_ref().is_none_or(JsonValue::is_null),
            _ => false,
        }
    }

    /// Returns the config to use when `value` overrides this declaration (from the CLI argument
    /// or `depends_on.vars`): a typed declaration keeps its type, so the value is interpreted
    /// according to it; otherwise the value replaces the declaration as is.
    pub fn with_value(&self, value: &VarsConfig) -> VarsConfig {
        let declared = match self {
            VarsConfig::Typed(t) => Some((t.r#type, &t.schema)),
            VarsConfig::Dynamic(d) => d.r#type.map(|t| (t, &d.schema)),
            VarsConfig::Static(_) => None,
        };
        match (declared, value) {
            (Some((r#type, schema)), VarsConfig::Static(v)) => VarsConfig::Typed(TypedVars {
                r#type,
                default: Some(v.clone()),
                schema: schema.clone(),
            }),
            _ => value.clone(),
        }
    }

    /// Checks the declaration itself: the JSON Schema keywords must be known, require `type`,
    /// and form a valid schema.
    ///
    /// # Errors
    ///
    /// Returns an error describing the offending keyword.
    pub fn validate(&self) -> anyhow::Result<()> {
        match self {
            VarsConfig::Typed(t) => validate_var_declaration(Some(t.r#type), &t.schema),
            VarsConfig::Dynamic(d) => validate_var_declaration(d.r#type, &d.schema),
            VarsConfig::Static(_) => Ok(()),
        }
    }
}

/// Accepts only scalar values; arrays and objects must be declared with `type`.
fn deserialize_scalar<'de, D: Deserializer<'de>>(deserializer: D) -> Result<JsonValue, D::Error> {
    let value = JsonValue::deserialize(deserializer)?;
    if value.is_array() || value.is_object() {
        return Err(de::Error::custom(
            "array and object values must be declared with `type`",
        ));
    }
    Ok(value)
}

fn scalar_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": ["string", "number", "boolean", "null"],
        "description": "Scalar value. Arrays and objects must be declared with `type`."
    })
}

/// Typed variable declaration
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TypedVars {
    /// Type of the variable, following JSON Schema
    #[serde(rename = "type")]
    pub r#type: VarType,

    /// Default value. Without it the variable is required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<JsonValue>,

    /// Any other JSON Schema keyword (`enum`, `pattern`, `minimum`, `items`, ...) validates the value.
    #[serde(flatten)]
    pub schema: VarSchema,
}

impl TypedVars {
    /// Checks `value` against the JSON Schema keywords of the declaration.
    pub fn check(&self, value: &JsonValue) -> anyhow::Result<()> {
        check_var_value(self.r#type, &self.schema, value)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct DynamicVars {
    /// Command
    #[schemars(extend("x-template" = true))]
    pub command: String,

    /// Type of the variable, following JSON Schema. The command output is interpreted as this
    /// type; without it, the type is inferred from the output.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<VarType>,

    /// Any other JSON Schema keyword (`enum`, `pattern`, `minimum`, `items`, ...) validates the
    /// output. Requires `type`.
    #[serde(flatten)]
    pub schema: VarSchema,

    /// Shell configuration
    pub shell: Option<ShellConfig>,

    /// Environment variables
    #[serde(default, deserialize_with = "deserialize_hash_map")]
    #[schemars(extend("x-template" = true))]
    pub env: IndexMap<String, String>,

    /// Dotenv files
    #[serde(default)]
    #[schemars(extend("x-template" = true))]
    pub env_files: Vec<String>,

    /// Working directory
    #[schemars(extend("x-template" = true))]
    pub working_dir: Option<String>,

    /// Whether the command output is reused by the other variables running the same command in
    /// the same working directory. A variable shared by several projects runs in each project
    /// directory, so sharing one run across them takes an explicit `working_dir`. Leave it off
    /// for a command that must run every time, ex: allocating a resource.
    #[serde(default)]
    pub cache: bool,

    #[serde(skip)]
    pub inner: Option<DynamicVarsInner>,
}

impl DynamicVars {
    pub fn env_file_paths(&self, dir: &Path) -> Vec<PathBuf> {
        self.env_files.iter().map(|f| absolute_or_join(f, dir)).collect()
    }

    pub fn working_dir_path(&self, dir: &Path) -> PathBuf {
        match self.working_dir.clone() {
            Some(wd) => absolute_or_join(&wd, dir),
            None => dir.to_path_buf(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DynamicVarsInner {
    pub name: String,
    pub command: String,
    pub shell: ShellConfig,
    pub working_dir: PathBuf,
    pub env: HashMap<String, String>,
    pub cache: bool,
}

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
