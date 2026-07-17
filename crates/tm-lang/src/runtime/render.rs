use super::*;
pub(super) struct BoundedDisplay {
    text: String,
    max_bytes: usize,
}

impl std::fmt::Write for BoundedDisplay {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        let remaining = self.max_bytes.saturating_sub(self.text.len());
        if value.len() <= remaining {
            self.text.push_str(value);
            return Ok(());
        }
        let mut end = remaining.min(value.len());
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&value[..end]);
        Err(std::fmt::Error)
    }
}

pub(super) fn bounded_display(value: &impl std::fmt::Display, max_bytes: usize) -> String {
    use std::fmt::Write as _;

    let mut output = BoundedDisplay {
        text: String::with_capacity(max_bytes.min(4096)),
        max_bytes,
    };
    let _ = write!(&mut output, "{value}");
    output.text
}

pub(super) struct BudgetWriter {
    bytes: Vec<u8>,
    written: usize,
    limit: usize,
    exceeded: bool,
}

impl BudgetWriter {
    pub(super) fn new(limit: usize, retain: bool) -> Self {
        Self {
            bytes: if retain {
                Vec::with_capacity(limit.min(4096))
            } else {
                Vec::new()
            },
            written: 0,
            limit,
            exceeded: false,
        }
    }

    pub(super) fn finish(self) -> (usize, String, bool) {
        let Self {
            mut bytes,
            written,
            exceeded,
            ..
        } = self;
        let valid_bytes = match std::str::from_utf8(&bytes) {
            Ok(_) => bytes.len(),
            Err(error) => error.valid_up_to(),
        };
        bytes.truncate(valid_bytes);
        (
            written,
            String::from_utf8(bytes).expect("validated UTF-8 prefix"),
            exceeded,
        )
    }
}

impl Write for BudgetWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let remaining = self.limit.saturating_sub(self.written);
        if !self.bytes.is_empty() || self.bytes.capacity() > 0 {
            self.bytes
                .extend_from_slice(&bytes[..bytes.len().min(remaining)]);
        }
        self.written = self.written.saturating_add(bytes.len());
        if bytes.len() > remaining {
            self.exceeded = true;
            return Err(io::Error::other("encoded value budget exceeded"));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) fn json_encoded_len_bounded(value: &JsonValue, limit: usize) -> Option<usize> {
    let mut writer = BudgetWriter::new(limit, false);
    let result = serde_json::to_writer(&mut writer, value);
    let (written, _, exceeded) = writer.finish();
    (result.is_ok() && !exceeded).then_some(written)
}

pub(super) fn json_string_encoded_len_bounded(value: &str, limit: usize) -> Option<usize> {
    let mut writer = BudgetWriter::new(limit, false);
    let result = serde_json::to_writer(&mut writer, value);
    let (written, _, exceeded) = writer.finish();
    (result.is_ok() && !exceeded).then_some(written)
}

pub(super) fn json_preview_bounded(value: &JsonValue, limit: usize) -> String {
    let mut writer = BudgetWriter::new(limit, true);
    let result = serde_json::to_writer(&mut writer, value);
    let (_, mut rendered, exceeded) = writer.finish();
    if result.is_err() || exceeded {
        if limit >= 3 {
            let mut end = limit.saturating_sub(3).min(rendered.len());
            while !rendered.is_char_boundary(end) {
                end -= 1;
            }
            rendered.truncate(end);
            rendered.push_str("...");
        } else {
            let mut end = limit.min(rendered.len());
            while !rendered.is_char_boundary(end) {
                end -= 1;
            }
            rendered.truncate(end);
        }
    }
    rendered
}

pub(super) fn write_value_json<W: Write>(
    writer: &mut W,
    value: &Value,
    depth: usize,
    max_depth: usize,
) -> io::Result<()> {
    if depth >= max_depth {
        return Err(io::Error::other("value nesting budget exceeded"));
    }
    match value {
        Value::Null => writer.write_all(b"null"),
        Value::Bool(value) => writer.write_all(if *value { b"true" } else { b"false" }),
        Value::Int(value) => write!(writer, "{value}"),
        Value::Decimal(value) => match serde_json::Number::from_f64(*value) {
            Some(value) => serde_json::to_writer(writer, &value).map_err(io::Error::other),
            None => writer.write_all(b"null"),
        },
        Value::String(value) | Value::Uri(value) => {
            serde_json::to_writer(writer, value).map_err(io::Error::other)
        }
        Value::List(values) => {
            writer.write_all(b"[")?;
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    writer.write_all(b",")?;
                }
                write_value_json(writer, value, depth + 1, max_depth)?;
            }
            writer.write_all(b"]")
        }
        Value::Record(fields) => {
            writer.write_all(b"{")?;
            for (index, (name, value)) in fields.iter().enumerate() {
                if index > 0 {
                    writer.write_all(b",")?;
                }
                serde_json::to_writer(&mut *writer, name).map_err(io::Error::other)?;
                writer.write_all(b":")?;
                write_value_json(writer, value, depth + 1, max_depth)?;
            }
            writer.write_all(b"}")
        }
        Value::Tagged { name, payload } => {
            writer.write_all(b"{\"tag\":")?;
            serde_json::to_writer(&mut *writer, name).map_err(io::Error::other)?;
            if let Some(payload) = payload {
                writer.write_all(b",\"value\":")?;
                write_value_json(writer, payload, depth + 1, max_depth)?;
            }
            writer.write_all(b"}")
        }
        Value::Callable(_) => writer.write_all(b"\"<function>\""),
    }
}

pub(super) fn value_json_bounded(
    value: &Value,
    limit: usize,
    retain: bool,
    max_depth: usize,
) -> Option<(usize, String)> {
    let mut writer = BudgetWriter::new(limit, retain);
    let result = write_value_json(&mut writer, value, 0, max_depth);
    let (written, rendered, exceeded) = writer.finish();
    (result.is_ok() && !exceeded).then_some((written, rendered))
}

pub(super) fn render_value_bounded(
    value: &Value,
    limit: usize,
    max_depth: usize,
) -> RuntimeResult<String> {
    match value {
        Value::String(value) | Value::Uri(value) if value.len() <= limit => Ok(value.clone()),
        Value::String(_) | Value::Uri(_) => Err(RuntimeError::Limit(
            "intermediate value budget exceeded".into(),
        )),
        _ => value_json_bounded(value, limit, true, max_depth)
            .map(|(_, rendered)| rendered)
            .ok_or_else(|| RuntimeError::Limit("intermediate value budget exceeded".into())),
    }
}
pub(super) fn compare_sort_keys(left: &Value, right: &Value) -> Ordering {
    match (left, right) {
        (Value::Int(left), Value::Int(right)) => left.cmp(right),
        (Value::Int(left), Value::Decimal(right)) => {
            (*left as f64).partial_cmp(right).unwrap_or(Ordering::Equal)
        }
        (Value::Decimal(left), Value::Int(right)) => left
            .partial_cmp(&(*right as f64))
            .unwrap_or(Ordering::Equal),
        (Value::Decimal(left), Value::Decimal(right)) => {
            left.partial_cmp(right).unwrap_or(Ordering::Equal)
        }
        (Value::Int(_) | Value::Decimal(_), _) => Ordering::Less,
        (_, Value::Int(_) | Value::Decimal(_)) => Ordering::Greater,
        _ => compare_values(left, right),
    }
}

pub(super) fn compare_values(left: &Value, right: &Value) -> Ordering {
    fn rank(value: &Value) -> u8 {
        match value {
            Value::Null => 0,
            Value::Bool(_) => 1,
            Value::Int(_) | Value::Decimal(_) => 2,
            Value::String(_) => 3,
            Value::Uri(_) => 4,
            Value::List(_) => 5,
            Value::Record(_) => 6,
            Value::Tagged { .. } => 7,
            Value::Callable(_) => 8,
        }
    }
    match (left, right) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Bool(left), Value::Bool(right)) => left.cmp(right),
        (Value::String(left), Value::String(right)) | (Value::Uri(left), Value::Uri(right)) => {
            left.cmp(right)
        }
        (Value::List(left), Value::List(right)) => left
            .iter()
            .zip(right)
            .map(|(left, right)| compare_sort_keys(left, right))
            .find(|ordering| *ordering != Ordering::Equal)
            .unwrap_or_else(|| left.len().cmp(&right.len())),
        (Value::Record(left), Value::Record(right)) => left
            .iter()
            .zip(right)
            .map(|((left_name, left), (right_name, right))| {
                left_name
                    .cmp(right_name)
                    .then_with(|| compare_sort_keys(left, right))
            })
            .find(|ordering| *ordering != Ordering::Equal)
            .unwrap_or_else(|| left.len().cmp(&right.len())),
        (
            Value::Tagged {
                name: left_name,
                payload: left_payload,
            },
            Value::Tagged {
                name: right_name,
                payload: right_payload,
            },
        ) => left_name.cmp(right_name).then_with(|| {
            left_payload
                .as_deref()
                .zip(right_payload.as_deref())
                .map(|(left, right)| compare_sort_keys(left, right))
                .unwrap_or_else(|| left_payload.is_some().cmp(&right_payload.is_some()))
        }),
        _ => rank(left).cmp(&rank(right)),
    }
}
