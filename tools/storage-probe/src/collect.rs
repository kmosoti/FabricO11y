//! Strict, bounded adapter for the registered S5 OTLP/JSON Log profile.

use std::collections::HashSet;
use std::io;

use fabric_o11y::{
    Attribute, Event, EventId, EventTime, ObservedTime, Payload, ResourceId, Scalar, SourceId,
    TenantId,
};
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Number;

const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceConfig {
    pub tenant: u64,
    pub source: u64,
    pub resource: u64,
    pub first_event_id: u64,
}

#[derive(Clone, Debug)]
enum Json {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct JsonVisitor;

        impl<'de> Visitor<'de> for JsonVisitor {
            type Value = Json;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON value")
            }

            fn visit_unit<E>(self) -> Result<Json, E> {
                Ok(Json::Null)
            }

            fn visit_none<E>(self) -> Result<Json, E> {
                Ok(Json::Null)
            }

            fn visit_bool<E>(self, value: bool) -> Result<Json, E> {
                Ok(Json::Bool(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Json, E> {
                Ok(Json::Number(Number::from(value)))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Json, E> {
                Ok(Json::Number(Number::from(value)))
            }

            fn visit_f64<E>(self, value: f64) -> Result<Json, E>
            where
                E: serde::de::Error,
            {
                Number::from_f64(value)
                    .map(Json::Number)
                    .ok_or_else(|| E::custom("non-finite JSON number"))
            }

            fn visit_str<E>(self, value: &str) -> Result<Json, E> {
                Ok(Json::String(value.to_owned()))
            }

            fn visit_string<E>(self, value: String) -> Result<Json, E> {
                Ok(Json::String(value))
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Json, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element()? {
                    values.push(value);
                }
                Ok(Json::Array(values))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Json, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = Vec::new();
                let mut names = HashSet::new();
                while let Some((name, value)) = map.next_entry::<String, Json>()? {
                    if !names.insert(name.clone()) {
                        return Err(serde::de::Error::custom(format!(
                            "duplicate JSON object member {name:?}"
                        )));
                    }
                    values.push((name, value));
                }
                Ok(Json::Object(values))
            }
        }

        deserializer.deserialize_any(JsonVisitor)
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn object(value: Json, context: &str) -> io::Result<Vec<(String, Json)>> {
    match value {
        Json::Object(fields) => Ok(fields),
        _ => Err(invalid(format!("{context} must be an object"))),
    }
}

fn array(value: Json, context: &str) -> io::Result<Vec<Json>> {
    match value {
        Json::Array(values) => Ok(values),
        _ => Err(invalid(format!("{context} must be an array"))),
    }
}

fn string(value: Json, context: &str) -> io::Result<String> {
    match value {
        Json::String(value) => Ok(value),
        _ => Err(invalid(format!("{context} must be a string"))),
    }
}

fn only_fields(fields: &[(String, Json)], allowed: &[&str], context: &str) -> io::Result<()> {
    for (name, _) in fields {
        if !allowed.contains(&name.as_str()) {
            return Err(invalid(format!("unknown {context} field {name:?}")));
        }
    }
    Ok(())
}

fn required<'a>(fields: &'a [(String, Json)], name: &str, context: &str) -> io::Result<&'a Json> {
    fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
        .ok_or_else(|| invalid(format!("missing {context}.{name}")))
}

fn optional<'a>(fields: &'a [(String, Json)], name: &str) -> Option<&'a Json> {
    fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

fn unsigned_integer(value: &Json, context: &str) -> io::Result<u64> {
    match value {
        Json::Number(number) if number.is_u64() => number
            .as_u64()
            .ok_or_else(|| invalid(format!("{context} must be an unsigned integer"))),
        _ => Err(invalid(format!(
            "{context} must be an unsigned JSON integer"
        ))),
    }
}

fn dropped_count(fields: &[(String, Json)], context: &str) -> io::Result<()> {
    if let Some(value) = optional(fields, "droppedAttributesCount") {
        if unsigned_integer(value, context)? != 0 {
            return Err(invalid(format!("{context} reports dropped attributes")));
        }
    }
    Ok(())
}

fn attributes(value: &Json, prefix: &str) -> io::Result<Vec<Attribute>> {
    let values = array(value.clone(), "attributes")?;
    let mut result = Vec::with_capacity(values.len());
    for item in values {
        let fields = object(item, "KeyValue")?;
        only_fields(&fields, &["key", "value"], "KeyValue")?;
        let key = string(
            required(&fields, "key", "KeyValue")?.clone(),
            "KeyValue.key",
        )?;
        let value = scalar(required(&fields, "value", "KeyValue")?)?;
        result.push(Attribute {
            key: format!("{prefix}{key}"),
            value,
        });
    }
    Ok(result)
}

fn scalar(value: &Json) -> io::Result<Scalar> {
    let fields = object(value.clone(), "AnyValue")?;
    if fields.len() != 1 {
        return Err(invalid("AnyValue must contain exactly one scalar field"));
    }
    let (name, value) = &fields[0];
    match (name.as_str(), value) {
        ("stringValue", Json::String(value)) => Ok(Scalar::String(value.clone())),
        ("boolValue", Json::Bool(value)) => Ok(Scalar::Bool(*value)),
        ("intValue", Json::String(value)) => value
            .parse::<i64>()
            .ok()
            .filter(|_| valid_signed_decimal(value))
            .map(Scalar::I64)
            .ok_or_else(|| invalid("intValue string must be a signed i64 decimal")),
        ("intValue", Json::Number(value)) => value
            .as_i64()
            .map(Scalar::I64)
            .ok_or_else(|| invalid("intValue must fit a signed i64")),
        ("doubleValue", Json::Number(value)) => {
            let parsed = value
                .as_f64()
                .ok_or_else(|| invalid("doubleValue is outside the f64 range"))?;
            if parsed.is_finite() {
                Ok(Scalar::F64(parsed))
            } else {
                Err(invalid("doubleValue JSON number must be finite"))
            }
        }
        ("doubleValue", Json::String(value)) if value == "NaN" => Ok(Scalar::F64(f64::NAN)),
        ("doubleValue", Json::String(value)) if value == "Infinity" => {
            Ok(Scalar::F64(f64::INFINITY))
        }
        ("doubleValue", Json::String(value)) if value == "-Infinity" => {
            Ok(Scalar::F64(f64::NEG_INFINITY))
        }
        ("stringValue" | "boolValue" | "intValue" | "doubleValue", _) => {
            Err(invalid(format!("invalid type for AnyValue.{name}")))
        }
        _ => Err(invalid(format!("unsupported AnyValue field {name:?}"))),
    }
}

fn valid_signed_decimal(value: &str) -> bool {
    let digits = value.strip_prefix('-').unwrap_or(value);
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
}

fn unsigned_wire(value: &Json, context: &str) -> io::Result<u64> {
    match value {
        Json::Number(number) if number.is_u64() => number
            .as_u64()
            .ok_or_else(|| invalid(format!("{context} must be an unsigned integer"))),
        Json::String(value) if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) => {
            value
                .parse::<u64>()
                .map_err(|_| invalid(format!("{context} is out of range")))
        }
        _ => Err(invalid(format!(
            "{context} must be an unsigned integer or decimal string"
        ))),
    }
}

fn timestamp(fields: &[(String, Json)], name: &str) -> io::Result<(i64, bool)> {
    match optional(fields, name) {
        None => Ok((0, false)),
        Some(value) => {
            let parsed = unsigned_wire(value, name)?;
            let signed =
                i64::try_from(parsed).map_err(|_| invalid(format!("{name} exceeds i64::MAX")))?;
            Ok((signed, true))
        }
    }
}

fn append_optional_string(
    fields: &[(String, Json)],
    name: &str,
    output_key: &str,
    attributes: &mut Vec<Attribute>,
) -> io::Result<()> {
    if let Some(value) = optional(fields, name) {
        attributes.push(Attribute {
            key: output_key.to_owned(),
            value: Scalar::String(string(value.clone(), name)?),
        });
    }
    Ok(())
}

fn append_u32(
    fields: &[(String, Json)],
    name: &str,
    max: u32,
    attributes: &mut Vec<Attribute>,
) -> io::Result<()> {
    if let Some(value) = optional(fields, name) {
        let value = unsigned_integer(value, name)?;
        if value > u64::from(max) {
            return Err(invalid(format!("{name} exceeds its allowed range")));
        }
        attributes.push(Attribute {
            key: format!("otel.log.field.{name}"),
            value: Scalar::U64(value),
        });
    }
    Ok(())
}

fn validate_hex_id(value: &str, expected: usize, name: &str) -> io::Result<()> {
    if !value.is_empty()
        && (value.len() != expected || !value.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(invalid(format!(
            "{name} must be empty or {expected} hexadecimal digits"
        )));
    }
    Ok(())
}

fn parse_log(
    value: Json,
    resource_attrs: &[Attribute],
    resource_schema: Option<&str>,
    scope_attrs: &[Attribute],
    scope_name: Option<&str>,
    scope_version: Option<&str>,
    scope_schema: Option<&str>,
    config: &SourceConfig,
    position: u64,
) -> io::Result<Event> {
    let fields = object(value, "LogRecord")?;
    only_fields(
        &fields,
        &[
            "timeUnixNano",
            "observedTimeUnixNano",
            "body",
            "attributes",
            "severityNumber",
            "severityText",
            "traceId",
            "spanId",
            "flags",
            "droppedAttributesCount",
            "eventName",
        ],
        "LogRecord",
    )?;
    dropped_count(&fields, "LogRecord.droppedAttributesCount")?;

    let (event_time, time_present) = timestamp(&fields, "timeUnixNano")?;
    let (observed_time, observed_present) = timestamp(&fields, "observedTimeUnixNano")?;
    let body_fields = object(required(&fields, "body", "LogRecord")?.clone(), "body")?;
    only_fields(&body_fields, &["stringValue"], "body")?;
    let body = string(
        required(&body_fields, "stringValue", "body")?.clone(),
        "body.stringValue",
    )?;

    let mut mapped = Vec::new();
    mapped.extend(resource_attrs.iter().map(clone_attribute));
    if let Some(value) = resource_schema {
        mapped.push(Attribute {
            key: "otel.resource.schema_url".to_owned(),
            value: Scalar::String(value.to_owned()),
        });
    }
    mapped.extend(scope_attrs.iter().map(clone_attribute));
    for (value, key) in [
        (scope_name, "otel.scope.name"),
        (scope_version, "otel.scope.version"),
        (scope_schema, "otel.scope.schema_url"),
    ] {
        if let Some(value) = value {
            mapped.push(Attribute {
                key: key.to_owned(),
                value: Scalar::String(value.to_owned()),
            });
        }
    }
    if let Some(value) = optional(&fields, "attributes") {
        mapped.extend(attributes(value, "otel.log.attr.")?);
    }
    append_u32(&fields, "severityNumber", 24, &mut mapped)?;
    append_optional_string(
        &fields,
        "severityText",
        "otel.log.field.severityText",
        &mut mapped,
    )?;
    if let Some(value) = optional(&fields, "traceId") {
        let value = string(value.clone(), "traceId")?;
        validate_hex_id(&value, 32, "traceId")?;
        mapped.push(Attribute {
            key: "otel.log.field.traceId".to_owned(),
            value: Scalar::String(value),
        });
    }
    if let Some(value) = optional(&fields, "spanId") {
        let value = string(value.clone(), "spanId")?;
        validate_hex_id(&value, 16, "spanId")?;
        mapped.push(Attribute {
            key: "otel.log.field.spanId".to_owned(),
            value: Scalar::String(value),
        });
    }
    append_u32(&fields, "flags", u32::MAX, &mut mapped)?;
    append_optional_string(
        &fields,
        "eventName",
        "otel.log.field.eventName",
        &mut mapped,
    )?;
    mapped.push(Attribute {
        key: "otel.log.field.time_present".to_owned(),
        value: Scalar::Bool(time_present),
    });
    mapped.push(Attribute {
        key: "otel.log.field.observed_time_present".to_owned(),
        value: Scalar::Bool(observed_present),
    });

    let id = config
        .first_event_id
        .checked_add(position)
        .ok_or_else(|| invalid("EventId addition overflow"))?;
    Ok(Event {
        id: EventId(id),
        tenant: TenantId(config.tenant),
        source: SourceId(config.source),
        resource: ResourceId(config.resource),
        event_time: EventTime(event_time),
        observed_time: ObservedTime(observed_time),
        attributes: mapped,
        payload: Payload::Log { body },
    })
}

fn clone_attribute(attribute: &Attribute) -> Attribute {
    Attribute {
        key: attribute.key.clone(),
        value: match &attribute.value {
            Scalar::Bool(value) => Scalar::Bool(*value),
            Scalar::I64(value) => Scalar::I64(*value),
            Scalar::U64(value) => Scalar::U64(*value),
            Scalar::F64(value) => Scalar::F64(*value),
            Scalar::String(value) => Scalar::String(value.clone()),
        },
    }
}

fn optional_string(
    fields: &[(String, Json)],
    name: &str,
    context: &str,
) -> io::Result<Option<String>> {
    optional(fields, name)
        .map(|value| string(value.clone(), context))
        .transpose()
}

fn parse_attributes_field(fields: &[(String, Json)], prefix: &str) -> io::Result<Vec<Attribute>> {
    match optional(fields, "attributes") {
        Some(value) => attributes(value, prefix),
        None => Ok(Vec::new()),
    }
}

fn parse_scope_logs(
    values: Vec<Json>,
    resource_attrs: &[Attribute],
    resource_schema: Option<&str>,
    config: &SourceConfig,
    position: &mut u64,
    output: &mut Vec<Event>,
) -> io::Result<()> {
    for scope_log in values {
        let fields = object(scope_log, "scopeLogs")?;
        only_fields(&fields, &["scope", "logRecords", "schemaUrl"], "scopeLogs")?;
        let scope_schema = optional_string(&fields, "schemaUrl", "scopeLogs.schemaUrl")?;
        let (scope_attrs, scope_name, scope_version) = match optional(&fields, "scope") {
            None => (Vec::new(), None, None),
            Some(value) => {
                let fields = object(value.clone(), "scope")?;
                only_fields(
                    &fields,
                    &["name", "version", "attributes", "droppedAttributesCount"],
                    "scope",
                )?;
                dropped_count(&fields, "scope.droppedAttributesCount")?;
                (
                    parse_attributes_field(&fields, "otel.scope.attr.")?,
                    optional_string(&fields, "name", "scope.name")?,
                    optional_string(&fields, "version", "scope.version")?,
                )
            }
        };
        let records = array(
            required(&fields, "logRecords", "scopeLogs")?.clone(),
            "scopeLogs.logRecords",
        )?;
        for record in records {
            let event = parse_log(
                record,
                resource_attrs,
                resource_schema.as_deref(),
                &scope_attrs,
                scope_name.as_deref(),
                scope_version.as_deref(),
                scope_schema.as_deref(),
                config,
                *position,
            )?;
            *position = position
                .checked_add(1)
                .ok_or_else(|| invalid("physical input position overflow"))?;
            output.push(event);
        }
    }
    Ok(())
}

pub fn adapt_otlp(bytes: &[u8], config: &SourceConfig) -> io::Result<Vec<Event>> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(invalid("request exceeds the 64 MiB input limit"));
    }
    let root: Json = serde_json::from_slice(bytes).map_err(|error| invalid(error.to_string()))?;
    let fields = object(root, "request")?;
    only_fields(&fields, &["resourceLogs"], "request")?;
    let resource_logs = array(
        required(&fields, "resourceLogs", "request")?.clone(),
        "resourceLogs",
    )?;
    if resource_logs.len() > 1 {
        return Err(invalid(
            "this profile supports at most one resourceLogs item",
        ));
    }
    let mut output = Vec::new();
    let mut position = 0_u64;
    for resource_log in resource_logs {
        let fields = object(resource_log, "resourceLogs item")?;
        only_fields(
            &fields,
            &["resource", "scopeLogs", "schemaUrl"],
            "resourceLogs item",
        )?;
        let (resource_attrs, resource_schema) = match optional(&fields, "resource") {
            None => (Vec::new(), None),
            Some(value) => {
                let resource = object(value.clone(), "resource")?;
                only_fields(
                    &resource,
                    &["attributes", "droppedAttributesCount"],
                    "resource",
                )?;
                dropped_count(&resource, "resource.droppedAttributesCount")?;
                (
                    parse_attributes_field(&resource, "otel.resource.attr.")?,
                    None,
                )
            }
        };
        let resource_schema =
            optional_string(&fields, "schemaUrl", "resourceLogs.schemaUrl")?.or(resource_schema);
        let scopes = array(
            required(&fields, "scopeLogs", "resourceLogs item")?.clone(),
            "resourceLogs.scopeLogs",
        )?;
        parse_scope_logs(
            scopes,
            &resource_attrs,
            resource_schema.as_deref(),
            config,
            &mut position,
            &mut output,
        )?;
    }
    Ok(output)
}
