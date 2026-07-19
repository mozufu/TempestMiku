use std::collections::BTreeSet;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use url::Url;

use crate::{McpBounds, McpError, Result};

const RESERVED_SERVER_ALIASES: &[&str] = &[
    "admin",
    "catalog",
    "internal",
    "prompts",
    "resources",
    "system",
    "tm",
    "tools",
];

const RESERVED_TOOL_NAMESPACES: &[&str] = &["catalog", "internal", "prompts", "resources"];

pub(crate) fn validate_server_alias(alias: &str) -> Result<()> {
    if alias.is_empty() || alias.len() > 48 {
        return Err(McpError::InvalidConfig(
            "server alias must contain 1..=48 bytes".to_string(),
        ));
    }
    if !alias.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
    }) || !alias
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        || !alias
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
    {
        return Err(McpError::InvalidConfig(format!(
            "server alias {alias:?} must be lowercase ASCII, start/end alphanumeric, and contain only [a-z0-9_-]"
        )));
    }
    if RESERVED_SERVER_ALIASES.contains(&alias) {
        return Err(McpError::InvalidConfig(format!(
            "server alias {alias:?} is reserved"
        )));
    }
    Ok(())
}

pub(crate) fn validate_name(name: &str, bounds: &McpBounds, context: &str) -> Result<()> {
    if name.is_empty() || name.len() > bounds.max_name_bytes {
        return Err(McpError::InvalidConfig(format!(
            "{context} name must contain 1..={} bytes",
            bounds.max_name_bytes
        )));
    }
    let first_ok = name
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric);
    let last_ok = name
        .as_bytes()
        .last()
        .is_some_and(u8::is_ascii_alphanumeric);
    let body_ok = name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'));
    if !first_ok || !last_ok || !body_ok {
        return Err(McpError::InvalidConfig(format!(
            "{context} name {name:?} is not a strict MCP name"
        )));
    }
    Ok(())
}

pub(crate) fn validate_remote_name(
    server: &str,
    name: &str,
    bounds: &McpBounds,
    context: &str,
) -> Result<()> {
    validate_name(name, bounds, context).map_err(|error| McpError::InvalidRemote {
        server: server.to_string(),
        message: error.to_string(),
    })
}

pub(crate) fn validate_tool_namespace(name: &str) -> Result<()> {
    let head = name.split('.').next().unwrap_or(name);
    if RESERVED_TOOL_NAMESPACES.contains(&head) {
        return Err(McpError::Collision(format!(
            "tool name {name:?} occupies reserved imported namespace {head:?}"
        )));
    }
    Ok(())
}

pub(crate) fn validate_uri(uri: &str, bounds: &McpBounds, context: &str) -> Result<()> {
    if uri.is_empty() || uri.len() > bounds.max_uri_bytes {
        return Err(McpError::InvalidConfig(format!(
            "{context} URI must contain 1..={} bytes",
            bounds.max_uri_bytes
        )));
    }
    let parsed = Url::parse(uri)
        .map_err(|error| McpError::InvalidConfig(format!("invalid {context} URI: {error}")))?;
    if parsed.scheme().is_empty() || uri.chars().any(char::is_control) {
        return Err(McpError::InvalidConfig(format!("invalid {context} URI")));
    }
    Ok(())
}

pub(crate) fn value_len(value: &Value) -> Result<usize> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|error| McpError::Unavailable(format!("JSON encoding failed: {error}")))
}

pub(crate) fn ensure_value_bound(value: &Value, max: usize, target: &str) -> Result<usize> {
    let bytes = value_len(value)?;
    if bytes > max {
        return Err(McpError::Bounds {
            target: target.to_string(),
            limit: format!("{bytes} bytes exceeds {max}"),
        });
    }
    Ok(bytes)
}

pub(crate) fn validate_schema(
    server: &str,
    schema: &Value,
    bounds: &McpBounds,
    label: &str,
) -> Result<Value> {
    ensure_value_bound(schema, bounds.max_schema_bytes, label)?;
    if !schema.is_object() {
        return Err(McpError::InvalidRemote {
            server: server.to_string(),
            message: format!("{label} must be a JSON Schema object"),
        });
    }
    let mut nodes = 0usize;
    validate_json_shape(
        server,
        schema,
        0,
        &mut nodes,
        bounds.max_schema_depth,
        bounds.max_schema_nodes,
        label,
    )?;
    let stripped = strip_schema_annotations(schema);
    validate_supported_schema(server, &stripped, label, true)?;
    Ok(stripped)
}

/// Produce the model-visible subset of a schema. Literal values supplied by a remote server are
/// deliberately withheld: `enum` and `const` still constrain validation internally, but cannot be
/// used as a catalog-time instruction channel.
pub(crate) fn schema_for_disclosure(schema: &Value) -> Value {
    const DROP: &[&str] = &["enum", "const", "$schema"];
    rewrite_schema_keywords(schema, DROP, false)
}

/// Validate one JSON value against the deliberately small, audited schema subset accepted above.
/// Unsupported JSON Schema vocabulary is rejected at catalog import instead of being silently
/// ignored and creating a validation gap.
pub(crate) fn validate_schema_instance(
    server: &str,
    schema: &Value,
    value: &Value,
    label: &str,
) -> Result<()> {
    validate_instance_at(server, schema, value, label, "$")
}

fn validate_supported_schema(server: &str, schema: &Value, label: &str, root: bool) -> Result<()> {
    let object = schema.as_object().ok_or_else(|| McpError::InvalidRemote {
        server: server.to_string(),
        message: format!("{label} contains a non-object subschema"),
    })?;
    const SUPPORTED: &[&str] = &[
        "$schema",
        "type",
        "properties",
        "required",
        "additionalProperties",
        "items",
        "minItems",
        "maxItems",
        "minLength",
        "maxLength",
        "minProperties",
        "maxProperties",
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "enum",
        "const",
    ];
    if object.keys().any(|key| !SUPPORTED.contains(&key.as_str())) {
        return Err(McpError::InvalidRemote {
            server: server.to_string(),
            message: format!("{label} uses an unsupported JSON Schema keyword"),
        });
    }
    if let Some(dialect) = object.get("$schema") {
        let supported = matches!(
            dialect.as_str(),
            Some(
                "https://json-schema.org/draft/2020-12/schema"
                    | "https://json-schema.org/draft/2020-12/schema#"
                    | "http://json-schema.org/draft-07/schema#"
                    | "https://json-schema.org/draft-07/schema#"
            )
        );
        if !supported {
            return Err(McpError::InvalidRemote {
                server: server.to_string(),
                message: format!("{label} declares an unsupported JSON Schema dialect"),
            });
        }
    }
    let schema_type =
        object
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| McpError::InvalidRemote {
                server: server.to_string(),
                message: format!("{label} schema must declare one supported type"),
            })?;
    if !matches!(
        schema_type,
        "null" | "boolean" | "object" | "array" | "number" | "integer" | "string"
    ) {
        return Err(McpError::InvalidRemote {
            server: server.to_string(),
            message: format!("{label} schema type is unsupported"),
        });
    }
    if root && schema_type != "object" {
        return Err(McpError::InvalidRemote {
            server: server.to_string(),
            message: format!("{label} root type must be object"),
        });
    }

    let properties = match object.get("properties") {
        None => None,
        Some(Value::Object(properties)) if schema_type == "object" => Some(properties),
        Some(_) => return invalid_schema(server, label, "properties requires object type"),
    };
    if let Some(properties) = properties {
        for (name, child) in properties {
            if !is_schema_property_name(name) {
                return invalid_schema(server, label, "property name is invalid or too long");
            }
            validate_supported_schema(server, child, label, false)?;
        }
    }
    if let Some(required) = object.get("required") {
        let required = required.as_array().ok_or_else(|| McpError::InvalidRemote {
            server: server.to_string(),
            message: format!("{label} required must be an array"),
        })?;
        if schema_type != "object" {
            return invalid_schema(server, label, "required requires object type");
        }
        let mut seen = BTreeSet::new();
        for name in required {
            let name = name.as_str().ok_or_else(|| McpError::InvalidRemote {
                server: server.to_string(),
                message: format!("{label} required entries must be strings"),
            })?;
            if !is_schema_property_name(name)
                || !seen.insert(name)
                || properties.is_none_or(|properties| !properties.contains_key(name))
            {
                return invalid_schema(
                    server,
                    label,
                    "required entries must be unique declared properties",
                );
            }
        }
    }
    if let Some(additional) = object.get("additionalProperties")
        && (schema_type != "object" || !additional.is_boolean())
    {
        return invalid_schema(
            server,
            label,
            "additionalProperties must be boolean on an object schema",
        );
    }
    match object.get("items") {
        None if schema_type == "array" => {
            return invalid_schema(server, label, "array schema requires items");
        }
        Some(items) if schema_type == "array" => {
            validate_supported_schema(server, items, label, false)?;
        }
        Some(_) => return invalid_schema(server, label, "items requires array type"),
        None => {}
    }

    validate_usize_pair(
        server,
        object,
        label,
        schema_type,
        "minItems",
        "maxItems",
        "array",
    )?;
    validate_usize_pair(
        server,
        object,
        label,
        schema_type,
        "minLength",
        "maxLength",
        "string",
    )?;
    validate_usize_pair(
        server,
        object,
        label,
        schema_type,
        "minProperties",
        "maxProperties",
        "object",
    )?;
    validate_number_bounds(server, object, label, schema_type)?;

    if let Some(values) = object.get("enum") {
        let values = values.as_array().ok_or_else(|| McpError::InvalidRemote {
            server: server.to_string(),
            message: format!("{label} enum must be an array"),
        })?;
        if values.is_empty() || values.len() > 128 {
            return invalid_schema(server, label, "enum must contain 1..=128 values");
        }
        let mut seen = BTreeSet::new();
        for value in values {
            validate_literal(server, label, schema_type, value)?;
            let encoded = serde_json::to_string(value).map_err(|error| {
                McpError::Unavailable(format!("schema literal encoding failed: {error}"))
            })?;
            if !seen.insert(encoded) {
                return invalid_schema(server, label, "enum values must be unique");
            }
        }
    }
    if let Some(value) = object.get("const") {
        validate_literal(server, label, schema_type, value)?;
    }
    Ok(())
}

fn is_schema_property_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && name
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn invalid_schema<T>(server: &str, label: &str, message: &str) -> Result<T> {
    Err(McpError::InvalidRemote {
        server: server.to_string(),
        message: format!("{label} {message}"),
    })
}

fn validate_usize_pair(
    server: &str,
    object: &Map<String, Value>,
    label: &str,
    schema_type: &str,
    minimum: &str,
    maximum: &str,
    required_type: &str,
) -> Result<()> {
    let minimum_value = optional_usize(server, object, label, minimum)?;
    let maximum_value = optional_usize(server, object, label, maximum)?;
    if (minimum_value.is_some() || maximum_value.is_some()) && schema_type != required_type {
        return invalid_schema(
            server,
            label,
            &format!("{minimum}/{maximum} require {required_type} type"),
        );
    }
    if minimum_value
        .zip(maximum_value)
        .is_some_and(|(min, max)| min > max)
    {
        return invalid_schema(server, label, &format!("{minimum} exceeds {maximum}"));
    }
    Ok(())
}

fn optional_usize(
    server: &str,
    object: &Map<String, Value>,
    label: &str,
    key: &str,
) -> Result<Option<usize>> {
    object
        .get(key)
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| McpError::InvalidRemote {
                    server: server.to_string(),
                    message: format!("{label} {key} must be a non-negative integer"),
                })
        })
        .transpose()
}

fn validate_number_bounds(
    server: &str,
    object: &Map<String, Value>,
    label: &str,
    schema_type: &str,
) -> Result<()> {
    let keys = ["minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum"];
    let mut values = Map::new();
    for key in keys {
        if let Some(value) = object.get(key) {
            if !matches!(schema_type, "number" | "integer") || !value.is_number() {
                return invalid_schema(server, label, "numeric bounds require number/integer type");
            }
            values.insert(key.to_string(), value.clone());
        }
    }
    if numeric(&values, "minimum")
        .zip(numeric(&values, "maximum"))
        .is_some_and(|(minimum, maximum)| minimum > maximum)
        || numeric(&values, "exclusiveMinimum")
            .zip(numeric(&values, "exclusiveMaximum"))
            .is_some_and(|(minimum, maximum)| minimum >= maximum)
    {
        return invalid_schema(server, label, "numeric minimum exceeds maximum");
    }
    Ok(())
}

fn numeric(object: &Map<String, Value>, key: &str) -> Option<f64> {
    object.get(key).and_then(Value::as_f64)
}

fn validate_literal(server: &str, label: &str, schema_type: &str, value: &Value) -> Result<()> {
    if matches!(value, Value::Array(_) | Value::Object(_))
        || !value_matches_type(value, schema_type)
        || value.as_str().is_some_and(|value| value.len() > 256)
    {
        return invalid_schema(
            server,
            label,
            "enum/const literals must be bounded scalars matching the schema type",
        );
    }
    Ok(())
}

fn validate_instance_at(
    server: &str,
    schema: &Value,
    value: &Value,
    label: &str,
    path: &str,
) -> Result<()> {
    let object = schema.as_object().expect("validated schema object");
    let schema_type = object
        .get("type")
        .and_then(Value::as_str)
        .expect("validated schema type");
    if !value_matches_type(value, schema_type) {
        return invalid_instance(server, label, path, &format!("expected {schema_type}"));
    }
    if let Some(expected) = object.get("const")
        && value != expected
    {
        return invalid_instance(server, label, path, "does not match const");
    }
    if let Some(values) = object.get("enum").and_then(Value::as_array)
        && !values.contains(value)
    {
        return invalid_instance(server, label, path, "is outside enum");
    }
    match schema_type {
        "object" => {
            let value = value.as_object().expect("object type checked");
            let properties = object
                .get("properties")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            if let Some(required) = object.get("required").and_then(Value::as_array) {
                for name in required.iter().filter_map(Value::as_str) {
                    if !value.contains_key(name) {
                        return invalid_instance(
                            server,
                            label,
                            path,
                            &format!("missing required property {name}"),
                        );
                    }
                }
            }
            if object.get("additionalProperties").and_then(Value::as_bool) == Some(false)
                && value.keys().any(|key| !properties.contains_key(key))
            {
                return invalid_instance(server, label, path, "has an additional property");
            }
            validate_len_bounds(
                server,
                label,
                path,
                value.len(),
                object,
                "minProperties",
                "maxProperties",
            )?;
            for (name, child_schema) in &properties {
                if let Some(child) = value.get(name) {
                    validate_instance_at(
                        server,
                        child_schema,
                        child,
                        label,
                        &format!("{path}/{}", escape_pointer(name)),
                    )?;
                }
            }
        }
        "array" => {
            let value = value.as_array().expect("array type checked");
            validate_len_bounds(
                server,
                label,
                path,
                value.len(),
                object,
                "minItems",
                "maxItems",
            )?;
            let items = object.get("items").expect("validated array items");
            for (index, child) in value.iter().enumerate() {
                validate_instance_at(server, items, child, label, &format!("{path}/{index}"))?;
            }
        }
        "string" => {
            let length = value.as_str().expect("string type checked").chars().count();
            validate_len_bounds(
                server,
                label,
                path,
                length,
                object,
                "minLength",
                "maxLength",
            )?;
        }
        "number" | "integer" => {
            let number = value.as_f64().expect("number type checked");
            if numeric(object, "minimum").is_some_and(|minimum| number < minimum)
                || numeric(object, "maximum").is_some_and(|maximum| number > maximum)
                || numeric(object, "exclusiveMinimum").is_some_and(|minimum| number <= minimum)
                || numeric(object, "exclusiveMaximum").is_some_and(|maximum| number >= maximum)
            {
                return invalid_instance(server, label, path, "is outside numeric bounds");
            }
        }
        _ => {}
    }
    Ok(())
}

fn invalid_instance<T>(server: &str, label: &str, path: &str, message: &str) -> Result<T> {
    Err(McpError::InvalidRemote {
        server: server.to_string(),
        message: format!("{label} {path} {message}"),
    })
}

fn validate_len_bounds(
    server: &str,
    label: &str,
    path: &str,
    length: usize,
    schema: &Map<String, Value>,
    minimum: &str,
    maximum: &str,
) -> Result<()> {
    if schema
        .get(minimum)
        .and_then(Value::as_u64)
        .is_some_and(|minimum| length < minimum as usize)
        || schema
            .get(maximum)
            .and_then(Value::as_u64)
            .is_some_and(|maximum| length > maximum as usize)
    {
        return invalid_instance(server, label, path, "length is outside schema bounds");
    }
    Ok(())
}

fn value_matches_type(value: &Value, schema_type: &str) -> bool {
    match schema_type {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "string" => value.is_string(),
        _ => false,
    }
}

fn escape_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn validate_json_shape(
    server: &str,
    value: &Value,
    depth: usize,
    nodes: &mut usize,
    max_depth: usize,
    max_nodes: usize,
    label: &str,
) -> Result<()> {
    *nodes = nodes.saturating_add(1);
    if depth > max_depth || *nodes > max_nodes {
        return Err(McpError::Bounds {
            target: format!("{server} {label}"),
            limit: format!("JSON depth/nodes exceeds {max_depth}/{max_nodes}"),
        });
    }
    match value {
        Value::Array(values) => {
            for value in values {
                validate_json_shape(server, value, depth + 1, nodes, max_depth, max_nodes, label)?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                if key.len() > 256 {
                    return Err(McpError::Bounds {
                        target: format!("{server} {label}"),
                        limit: "schema key exceeds 256 bytes".to_string(),
                    });
                }
                validate_json_shape(server, value, depth + 1, nodes, max_depth, max_nodes, label)?;
            }
        }
        Value::String(text) if text.len() > 16 * 1024 => {
            return Err(McpError::Bounds {
                target: format!("{server} {label}"),
                limit: "schema string exceeds 16384 bytes".to_string(),
            });
        }
        _ => {}
    }
    Ok(())
}

/// Remove annotation-only text before schema disclosure. Constraints and property names remain,
/// while remote descriptions/comments/examples cannot become ambient model instructions.
fn strip_schema_annotations(value: &Value) -> Value {
    const DROP: &[&str] = &[
        "$comment",
        "description",
        "examples",
        "default",
        "title",
        "deprecated",
        "readOnly",
        "writeOnly",
    ];
    rewrite_schema_keywords(value, DROP, false)
}

/// Remove schema keywords without treating entries inside a `properties` map as keywords. JSON
/// Schema permits ordinary fields literally named `title`, `description`, `enum`, or `const`; those
/// names must survive while annotations/constraints on their child schemas are transformed.
fn rewrite_schema_keywords(value: &Value, drop: &[&str], properties_map: bool) -> Value {
    match value {
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| rewrite_schema_keywords(value, drop, false))
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .filter(|(key, _)| properties_map || !drop.contains(&key.as_str()))
                .map(|(key, value)| {
                    (
                        key.clone(),
                        rewrite_schema_keywords(
                            value,
                            drop,
                            !properties_map && key == "properties",
                        ),
                    )
                })
                .collect::<Map<_, _>>(),
        ),
        _ => value.clone(),
    }
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub(crate) fn value_digest(value: &Value) -> Result<String> {
    serde_json::to_vec(value)
        .map(|bytes| sha256_hex(&bytes))
        .map_err(|error| McpError::Unavailable(format!("JSON encoding failed: {error}")))
}

pub(crate) fn local_resource_uri(alias: &str, source_uri: &str) -> String {
    format!(
        "mcp://{alias}/resources/{}",
        sha256_hex(source_uri.as_bytes())
    )
}
