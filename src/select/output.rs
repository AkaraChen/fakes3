//! Serialisation of result rows as CSV or JSON, collected into event-sized chunks.

use bytes::Bytes;
use serde_json::Value as Json;

use super::eval::{Value, to_text};

/// A Records event carries at most this much.
const CHUNK: usize = 128 * 1024;

pub enum Format {
    Csv {
        field: String,
        record: String,
        quote: char,
        escape: char,
        always: bool,
    },
    Json {
        record: String,
    },
}

pub struct Sink {
    fmt: Format,
    buf: Vec<u8>,
    chunks: Vec<Bytes>,
    pub returned: u64,
}

impl Sink {
    pub fn new(fmt: Format) -> Self {
        Sink {
            fmt,
            buf: Vec::new(),
            chunks: Vec::new(),
            returned: 0,
        }
    }

    /// One result row; a JSON row is keyed by the names in `cells`, a missing value is left out.
    pub fn row(&mut self, cells: &[(&str, Value<'_>)]) {
        match &self.fmt {
            Format::Csv {
                field,
                record,
                quote,
                escape,
                always,
            } => {
                for (i, (_, v)) in cells.iter().enumerate() {
                    if i > 0 {
                        self.buf.extend_from_slice(field.as_bytes());
                    }
                    let text = to_text(v);
                    let needs = *always && !v.is_null()
                        || text.contains(field.as_str())
                        || text.contains(*quote)
                        || text.contains(record.as_str())
                        || text.contains(['\n', '\r']);
                    if needs {
                        let mut tmp = [0u8; 4];
                        self.buf.extend_from_slice(quote.encode_utf8(&mut tmp).as_bytes());
                        for c in text.chars() {
                            if c == *quote || c == *escape {
                                self.buf.extend_from_slice(escape.encode_utf8(&mut tmp).as_bytes());
                            }
                            self.buf.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
                        }
                        self.buf.extend_from_slice(quote.encode_utf8(&mut tmp).as_bytes());
                    } else {
                        self.buf.extend_from_slice(text.as_bytes());
                    }
                }
                self.buf.extend_from_slice(record.as_bytes());
            }
            Format::Json { record } => {
                self.buf.push(b'{');
                let mut first = true;
                for (k, v) in cells {
                    if matches!(v, Value::Missing) {
                        continue;
                    }
                    if !first {
                        self.buf.push(b',');
                    }
                    first = false;
                    let _ = serde_json::to_writer(&mut self.buf, k);
                    self.buf.push(b':');
                    let _ = serde_json::to_writer(&mut self.buf, &json(v));
                }
                self.buf.push(b'}');
                self.buf.extend_from_slice(record.as_bytes());
            }
        }
        self.flush(CHUNK);
    }

    /// A JSON record as it is, for `SELECT *` over JSON.
    pub fn json_record(&mut self, v: &Json) {
        if let Format::Json { record } = &self.fmt {
            let _ = serde_json::to_writer(&mut self.buf, v);
            self.buf.extend_from_slice(record.as_bytes());
            self.flush(CHUNK);
        }
    }

    fn flush(&mut self, at_least: usize) {
        if !self.buf.is_empty() && self.buf.len() >= at_least {
            self.returned += self.buf.len() as u64;
            self.chunks.push(Bytes::from(std::mem::take(&mut self.buf)));
        }
    }

    pub fn finish(mut self) -> (Vec<Bytes>, u64) {
        self.flush(1);
        (self.chunks, self.returned)
    }

    pub fn is_json(&self) -> bool {
        matches!(self.fmt, Format::Json { .. })
    }
}

fn json(v: &Value<'_>) -> Json {
    match v {
        Value::Null | Value::Missing => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Int(n) => Json::from(*n),
        Value::Float(f) => serde_json::Number::from_f64(*f).map_or(Json::Null, Json::Number),
        Value::Str(s) => Json::String(s.to_string()),
        Value::Json(j) => (*j).clone(),
    }
}
