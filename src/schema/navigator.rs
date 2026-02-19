use crate::position::PathSegment;
use serde_json::Value;
use std::collections::HashSet;

/// Information extracted from a schema node for hover display.
#[derive(Debug, Default)]
pub struct HoverInfo {
    pub description: Option<String>,
    pub type_info: Option<String>,
    pub default: Option<String>,
    pub examples: Vec<String>,
    pub enum_values: Vec<String>,
}

impl HoverInfo {
    pub fn to_markdown(&self) -> String {
        let mut parts = Vec::new();

        if let Some(desc) = &self.description {
            parts.push(desc.clone());
        }

        if let Some(ty) = &self.type_info {
            parts.push(format!("**Type:** `{ty}`"));
        }

        if let Some(default) = &self.default {
            parts.push(format!("**Default:** `{default}`"));
        }

        if !self.enum_values.is_empty() {
            let vals = self
                .enum_values
                .iter()
                .map(|v| format!("`{v}`"))
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("**Allowed values:** {vals}"));
        }

        if !self.examples.is_empty() {
            let exs = self
                .examples
                .iter()
                .map(|e| format!("`{e}`"))
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("**Examples:** {exs}"));
        }

        parts.join("\n\n")
    }
}

/// A reference into a JSON Schema document that supports navigation.
pub struct SchemaNode<'a> {
    /// The current schema sub-object.
    pub schema: &'a Value,
    /// The document root (for resolving `$ref`).
    pub root: &'a Value,
}

impl<'a> SchemaNode<'a> {
    pub fn new(schema: &'a Value, root: &'a Value) -> Self {
        Self { schema, root }
    }

    fn resolved(&self) -> &'a Value {
        resolve_ref(self.schema, self.root, &mut HashSet::new()).unwrap_or(self.schema)
    }

    /// Navigate to the schema node at the given JSON path.
    pub fn navigate(&self, path: &[PathSegment]) -> Option<SchemaNode<'a>> {
        let mut visited: HashSet<usize> = HashSet::new();
        navigate_inner(self.schema, self.root, path, &mut visited)
    }

    /// Return the names of all directly defined properties (for completion).
    pub fn property_names(&self) -> Vec<String> {
        let schema = self.resolved();
        let mut names = Vec::new();

        if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
            names.extend(props.keys().cloned());
        }

        for key in &["allOf", "anyOf", "oneOf"] {
            if let Some(arr) = schema.get(key).and_then(|v| v.as_array()) {
                for sub in arr {
                    let node = SchemaNode::new(sub, self.root);
                    names.extend(node.property_names());
                }
            }
        }

        names.sort();
        names.dedup();
        names
    }

    /// Extract hover information from this schema node.
    pub fn hover_info(&self) -> HoverInfo {
        extract_hover_info(self.resolved())
    }

    /// Return enum values if the schema has an `enum` keyword.
    pub fn enum_values(&self) -> Vec<String> {
        self.resolved()
            .get("enum")
            .and_then(|e| e.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|v| match v {
                        Value::String(s) => format!("\"{}\"", s),
                        other => other.to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Return the `type` field if present.
    pub fn schema_type(&self) -> Option<&str> {
        self.resolved().get("type").and_then(|t| t.as_str())
    }
}

fn navigate_inner<'a>(
    schema: &'a Value,
    root: &'a Value,
    path: &[PathSegment],
    visited: &mut HashSet<usize>,
) -> Option<SchemaNode<'a>> {
    // Cycle guard
    let ptr = schema as *const Value as usize;
    if visited.contains(&ptr) {
        return None;
    }
    visited.insert(ptr);

    let schema = resolve_ref(schema, root, visited).unwrap_or(schema);

    if path.is_empty() {
        return Some(SchemaNode { schema, root });
    }

    let segment = &path[0];
    let rest = &path[1..];

    // Try direct resolution for current segment
    if let Some(node) = try_navigate_segment(schema, root, segment, visited) {
        return navigate_inner(node.schema, root, rest, visited);
    }

    // Try allOf / anyOf / oneOf sub-schemas
    for key in &["allOf", "anyOf", "oneOf"] {
        if let Some(arr) = schema.get(key).and_then(|v| v.as_array()) {
            for sub in arr {
                if let Some(node) = navigate_inner(sub, root, path, visited) {
                    return Some(node);
                }
            }
        }
    }

    None
}

fn try_navigate_segment<'a>(
    schema: &'a Value,
    root: &'a Value,
    segment: &PathSegment,
    _visited: &mut HashSet<usize>,
) -> Option<SchemaNode<'a>> {
    match segment {
        PathSegment::Key(key) => {
            // Check properties
            if let Some(prop) = schema.get("properties").and_then(|p| p.get(key.as_str())) {
                return Some(SchemaNode { schema: prop, root });
            }

            // Check patternProperties (find first matching pattern)
            if let Some(pattern_props) = schema.get("patternProperties").and_then(|p| p.as_object())
            {
                for (pattern, sub) in pattern_props {
                    if let Ok(re) = regex_lite_match(pattern, key) {
                        if re {
                            return Some(SchemaNode { schema: sub, root });
                        }
                    }
                }
            }

            // Fall back to additionalProperties
            if let Some(ap) = schema.get("additionalProperties") {
                if ap.is_object() {
                    return Some(SchemaNode { schema: ap, root });
                }
            }

            None
        }

        PathSegment::Index(idx) => {
            // items as object (applies to all)
            if let Some(items) = schema.get("items") {
                if items.is_object() || items.get("$ref").is_some() {
                    return Some(SchemaNode {
                        schema: items,
                        root,
                    });
                }
                // items as array (tuple validation — deprecated in draft 2020-12)
                if let Some(item) = items.as_array().and_then(|a| a.get(*idx)) {
                    return Some(SchemaNode { schema: item, root });
                }
            }

            // prefixItems (draft 2020-12)
            if let Some(item) = schema
                .get("prefixItems")
                .and_then(|pi| pi.as_array())
                .and_then(|a| a.get(*idx))
            {
                return Some(SchemaNode { schema: item, root });
            }

            None
        }
    }
}

/// Resolve a `$ref` JSON Pointer fragment within the root document.
/// Returns `None` if no `$ref` is present or resolution fails.
fn resolve_ref<'a>(
    schema: &'a Value,
    root: &'a Value,
    visited: &mut HashSet<usize>,
) -> Option<&'a Value> {
    let ref_str = schema.get("$ref")?.as_str()?;

    // Only support fragment-only JSON Pointers: "#/path/to/def"
    let pointer = ref_str.strip_prefix('#')?;

    let ptr = root as *const Value as usize;
    if visited.contains(&ptr) {
        return None;
    }
    visited.insert(ptr);

    root.pointer(pointer)
}

/// Minimal pattern matching — just literal string containment for patternProperties.
/// A full regex engine would be overkill here; we fall through to `additionalProperties`
/// for unmatched patterns.
fn regex_lite_match(pattern: &str, value: &str) -> Result<bool, ()> {
    // Very simple: check if value contains the pattern as literal substring
    // This covers the most common cases (e.g., "^x-" for extension properties)
    if pattern.starts_with('^') {
        let trimmed = pattern.trim_start_matches('^');
        return Ok(value.starts_with(trimmed));
    }
    Ok(value.contains(pattern))
}

fn extract_hover_info(schema: &Value) -> HoverInfo {
    let description = schema
        .get("description")
        .and_then(|d| d.as_str())
        .map(str::to_owned)
        .or_else(|| {
            schema
                .get("title")
                .and_then(|t| t.as_str())
                .map(str::to_owned)
        });

    let type_info = match schema.get("type") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Array(arr)) => {
            let types: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
            if types.is_empty() {
                None
            } else {
                Some(types.join(" | "))
            }
        }
        _ => None,
    };

    let default = schema.get("default").map(|v| v.to_string());

    let examples = schema
        .get("examples")
        .and_then(|e| e.as_array())
        .map(|arr| arr.iter().map(|v| v.to_string()).collect())
        .unwrap_or_default();

    let enum_values = schema
        .get("enum")
        .and_then(|e| e.as_array())
        .map(|arr| {
            arr.iter()
                .map(|v| match v {
                    Value::String(s) => format!("\"{}\"", s),
                    other => other.to_string(),
                })
                .collect()
        })
        .unwrap_or_default();

    HoverInfo {
        description,
        type_info,
        default,
        examples,
        enum_values,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_schema() -> Value {
        json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The name of the thing"
                },
                "count": {
                    "type": "integer",
                    "default": 0,
                    "description": "How many"
                },
                "tags": {
                    "type": "array",
                    "items": {
                        "type": "string"
                    }
                },
                "nested": {
                    "type": "object",
                    "properties": {
                        "inner": {
                            "type": "boolean"
                        }
                    }
                }
            }
        })
    }

    #[test]
    fn test_navigate_to_property() {
        let schema = make_schema();
        let node = SchemaNode::new(&schema, &schema);

        let path = vec![PathSegment::Key("name".into())];
        let result = node.navigate(&path);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(
            result.schema.get("type").and_then(|v| v.as_str()),
            Some("string")
        );
    }

    #[test]
    fn test_navigate_nested() {
        let schema = make_schema();
        let node = SchemaNode::new(&schema, &schema);

        let path = vec![
            PathSegment::Key("nested".into()),
            PathSegment::Key("inner".into()),
        ];
        let result = node.navigate(&path);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(
            result.schema.get("type").and_then(|v| v.as_str()),
            Some("boolean")
        );
    }

    #[test]
    fn test_navigate_array_items() {
        let schema = make_schema();
        let node = SchemaNode::new(&schema, &schema);

        let path = vec![PathSegment::Key("tags".into()), PathSegment::Index(0)];
        let result = node.navigate(&path);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(
            result.schema.get("type").and_then(|v| v.as_str()),
            Some("string")
        );
    }

    #[test]
    fn test_property_names() {
        let schema = make_schema();
        let node = SchemaNode::new(&schema, &schema);
        let names = node.property_names();
        assert!(names.contains(&"name".to_owned()));
        assert!(names.contains(&"count".to_owned()));
        assert!(names.contains(&"tags".to_owned()));
        assert!(names.contains(&"nested".to_owned()));
    }

    #[test]
    fn test_hover_info() {
        let schema = make_schema();
        let node = SchemaNode::new(&schema, &schema);
        let path = vec![PathSegment::Key("count".into())];
        let result = node.navigate(&path).unwrap();
        let info = result.hover_info();
        assert_eq!(info.description.as_deref(), Some("How many"));
        assert_eq!(info.type_info.as_deref(), Some("integer"));
        assert_eq!(info.default.as_deref(), Some("0"));
    }

    #[test]
    fn test_ref_resolution() {
        let schema = json!({
            "definitions": {
                "MyType": {
                    "type": "string",
                    "description": "A referenced type"
                }
            },
            "type": "object",
            "properties": {
                "value": {
                    "$ref": "#/definitions/MyType"
                }
            }
        });

        let node = SchemaNode::new(&schema, &schema);
        let path = vec![PathSegment::Key("value".into())];
        let result = node.navigate(&path);
        assert!(result.is_some());
        let result = result.unwrap();
        let info = result.hover_info();
        assert_eq!(info.description.as_deref(), Some("A referenced type"));
    }

    #[test]
    fn test_enum_values() {
        let schema = json!({
            "type": "object",
            "properties": {
                "status": {
                    "type": "string",
                    "enum": ["active", "inactive", "pending"]
                }
            }
        });

        let node = SchemaNode::new(&schema, &schema);
        let path = vec![PathSegment::Key("status".into())];
        let result = node.navigate(&path).unwrap();
        let vals = result.enum_values();
        assert_eq!(vals, vec!["\"active\"", "\"inactive\"", "\"pending\""]);
    }

    #[test]
    fn test_cycle_detection() {
        // A schema with a $ref that points to itself — should not infinite-loop
        let schema = json!({
            "type": "object",
            "properties": {
                "child": {
                    "$ref": "#"
                }
            }
        });

        let node = SchemaNode::new(&schema, &schema);
        let path = vec![
            PathSegment::Key("child".into()),
            PathSegment::Key("child".into()),
            PathSegment::Key("child".into()),
        ];
        // Should return Some or None, but NOT panic/stack-overflow
        let _ = node.navigate(&path);
    }
}
