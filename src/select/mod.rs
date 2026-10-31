//! S3 Select (`SelectObjectContent`): a pragmatic subset. CSV and JSON objects, uncompressed, queried
//! with `SELECT ... FROM S3Object[...] [WHERE ...] [LIMIT n]`, answered as an event stream of
//! Records, Progress, Stats and End. The whole query runs before the first byte is sent, so a
//! failing query is an ordinary HTTP 400 and not an error in the middle of the stream. Nothing here
//! runs unless a SelectObjectContent request arrives. `docs/select.md` lists what is supported.

mod eval;
mod input;
mod output;
mod sql;

use std::borrow::Cow;

use bytes::{Bytes, BytesMut};
use s3s::dto::*;
use s3s::{S3Error, S3ErrorCode, S3Request, S3Response, S3Result};
use serde_json::Value as Json;

use self::eval::{Acc, Env, Rec, Value, from_json};
use self::input::{CsvIn, HeaderInfo};
use self::output::{Format, Sink};
use self::sql::{Expr, Query, Step};
use crate::objext;
use crate::s3impl::{FakeS3, obj};

/// An S3 Select error: HTTP 400 with the code AWS documents for it.
pub(crate) fn err(code: &'static str, msg: impl Into<Cow<'static, str>>) -> S3Error {
    let mut e = S3Error::with_message(S3ErrorCode::Custom(code.into()), msg);
    e.set_status_code(http::StatusCode::BAD_REQUEST);
    e
}

fn not_implemented(what: &str) -> S3Error {
    S3Error::with_message(
        S3ErrorCode::NotImplemented,
        format!("{what} is not supported by fakes3's S3 Select"),
    )
}

enum Input {
    Csv(CsvIn),
    Json { lines: bool },
}

struct Plan {
    query: Query,
    input: Input,
    output: Format,
    progress: bool,
    scan: Option<(Option<i64>, Option<i64>)>,
}

/// The one character of an option such as `FieldDelimiter`; `default` if it was not given.
fn byte(opt: &Option<String>, default: u8, what: &str) -> Result<u8, S3Error> {
    match opt.as_deref() {
        None => Ok(default),
        Some(s) if s.len() == 1 && s.is_ascii() => Ok(s.as_bytes()[0]),
        Some(_) => Err(err("InvalidRequestParameter", format!("{what} must be a single ASCII character"))),
    }
}

fn record_delimiter(opt: &Option<String>, what: &str) -> Result<String, S3Error> {
    match opt.as_deref() {
        None => Ok("\n".to_owned()),
        Some(s) if (1..=2).contains(&s.len()) && s.is_ascii() => Ok(s.to_owned()),
        Some(_) => Err(err(
            "InvalidRequestParameter",
            format!("{what} must be one or two ASCII characters"),
        )),
    }
}

impl Plan {
    fn new(req: &SelectObjectContentRequest) -> Result<Plan, S3Error> {
        if req.expression_type.as_str() != ExpressionType::SQL {
            return Err(err(
                "InvalidExpressionType",
                "The ExpressionType is invalid. Only SQL is supported.",
            ));
        }
        let ins = &req.input_serialization;
        match [ins.csv.is_some(), ins.json.is_some(), ins.parquet.is_some()]
            .iter()
            .filter(|b| **b)
            .count()
        {
            0 => return Err(err("MissingRequiredParameter", "InputSerialization needs CSV, JSON or Parquet")),
            1 => {}
            _ => {
                return Err(err(
                    "ObjectSerializationConflict",
                    "InputSerialization can only have one of CSV, JSON and Parquet",
                ));
            }
        }
        match ins.compression_type.as_ref().map(CompressionType::as_str) {
            None | Some("NONE") => {}
            Some("GZIP" | "BZIP2") => return Err(not_implemented("Compressed input")),
            Some(_) => return Err(err("InvalidCompressionFormat", "CompressionType must be NONE, GZIP or BZIP2")),
        }
        let input = if let Some(c) = &ins.csv {
            let header = match c.file_header_info.as_ref().map(FileHeaderInfo::as_str) {
                None => HeaderInfo::None,
                Some(h) if h.eq_ignore_ascii_case("NONE") => HeaderInfo::None,
                Some(h) if h.eq_ignore_ascii_case("IGNORE") => HeaderInfo::Ignore,
                Some(h) if h.eq_ignore_ascii_case("USE") => HeaderInfo::Use,
                Some(_) => return Err(err("InvalidFileHeaderInfo", "FileHeaderInfo must be NONE, IGNORE or USE")),
            };
            Input::Csv(CsvIn {
                field: byte(&c.field_delimiter, b',', "FieldDelimiter")?,
                record: record_delimiter(&c.record_delimiter, "RecordDelimiter")?.into_bytes(),
                quote: Some(byte(&c.quote_character, b'"', "QuoteCharacter")?),
                escape: Some(byte(&c.quote_escape_character, b'"', "QuoteEscapeCharacter")?),
                comment: c.comments.as_ref().map(|_| byte(&c.comments, b'#', "Comments")).transpose()?,
                header,
            })
        } else if let Some(j) = &ins.json {
            match j.type_.as_ref().map(JSONType::as_str) {
                Some("LINES") => Input::Json { lines: true },
                Some("DOCUMENT") | None => Input::Json { lines: false },
                Some(_) => return Err(err("InvalidJsonType", "The JsonType must be DOCUMENT or LINES")),
            }
        } else {
            return Err(not_implemented("Parquet input"));
        };
        let outs = &req.output_serialization;
        let output = match (&outs.csv, &outs.json) {
            (None, None) => return Err(err("MissingRequiredParameter", "OutputSerialization needs CSV or JSON")),
            (Some(_), Some(_)) => {
                return Err(err(
                    "ObjectSerializationConflict",
                    "OutputSerialization can only have one of CSV and JSON",
                ));
            }
            (Some(c), None) => {
                let always = match c.quote_fields.as_ref().map(QuoteFields::as_str) {
                    None | Some("ASNEEDED") => false,
                    Some("ALWAYS") => true,
                    Some(_) => return Err(err("InvalidQuoteFields", "QuoteFields must be ALWAYS or ASNEEDED")),
                };
                let quote = byte(&c.quote_character, b'"', "QuoteCharacter")? as char;
                Format::Csv {
                    field: (byte(&c.field_delimiter, b',', "FieldDelimiter")? as char).to_string(),
                    record: record_delimiter(&c.record_delimiter, "RecordDelimiter")?,
                    quote,
                    escape: if c.quote_escape_character.is_some() {
                        byte(&c.quote_escape_character, b'"', "QuoteEscapeCharacter")? as char
                    } else {
                        quote
                    },
                    always,
                }
            }
            (None, Some(j)) => Format::Json {
                record: record_delimiter(&j.record_delimiter, "RecordDelimiter")?,
            },
        };
        let query = sql::parse(&req.expression)?;
        let scan = req
            .scan_range
            .as_ref()
            .filter(|r| r.start.is_some() || r.end.is_some())
            .map(|r| (r.start, r.end));
        if scan.is_some() && matches!(input, Input::Json { lines: false }) {
            return Err(err("InvalidScanRange", "ScanRange is not supported for JSON documents; use LINES"));
        }
        Ok(Plan {
            query,
            input,
            output,
            progress: req.request_progress.as_ref().and_then(|p| p.enabled).unwrap_or(false),
            scan,
        })
    }

    /// The byte range ScanRange selects; records that start inside it are processed.
    fn range(&self, size: u64) -> Result<(u64, u64), S3Error> {
        let bad = || err("InvalidScanRange", "The ScanRange is invalid");
        Ok(match self.scan {
            None => (0, size),
            Some((Some(s), Some(e))) if s >= 0 && e >= s => (s as u64, (e as u64).saturating_add(1).min(size)),
            Some((Some(s), None)) if s >= 0 => (s as u64, size),
            Some((None, Some(e))) if e >= 0 => (size.saturating_sub(e as u64), size),
            Some(_) => return Err(bad()),
        })
    }

    fn run(self, data: Bytes) -> S3Result<Vec<S3Result<SelectObjectContentEvent>>> {
        let size = data.len() as u64;
        let (lo, hi) = self.range(size)?;
        let q = &self.query;
        let mut sink = Sink::new(self.output);

        let mut env = Env::new(
            matches!(self.input, Input::Json { .. }),
            None,
            q.source.alias.clone(),
            q.items
                .iter()
                .flatten()
                .filter_map(|i| i.alias.clone().map(|a| (a, i.expr.clone())))
                .collect(),
        );
        let names: Vec<String> = q.items.iter().flatten().enumerate().map(|(i, it)| item_name(it, i)).collect();
        let mut accs: Vec<Acc> = q.aggs.iter().map(|a| Acc::new(a.func)).collect();
        let mut rows = 0u64;
        let mut star_keys: Vec<String> = Vec::new();

        // One matching record: filter, then aggregate or project. `false` stops the scan.
        let mut record = |env: &Env, rec: Rec<'_>, sink: &mut Sink| -> S3Result<bool> {
            if let Some(w) = &q.filter
                && !matches!(env.eval(w, rec, &[])?, Value::Bool(true))
            {
                return Ok(true);
            }
            if q.aggregate {
                for (acc, spec) in accs.iter_mut().zip(&q.aggs) {
                    acc.feed(spec.arg.as_ref().map(|a| env.eval(a, rec, &[])).transpose()?)?;
                }
                return Ok(true);
            }
            match (&q.items, rec) {
                (Some(items), _) => {
                    let mut cells = Vec::with_capacity(items.len());
                    for (item, name) in items.iter().zip(&names) {
                        cells.push((name.as_str(), env.eval(&item.expr, rec, &[])?));
                    }
                    sink.row(&cells);
                }
                (None, Rec::Json(j)) if sink.is_json() => sink.json_record(j),
                (None, Rec::Json(j)) => match j {
                    Json::Object(m) => {
                        let cells: Vec<_> = m.iter().map(|(k, v)| (k.as_str(), from_json(v))).collect();
                        sink.row(&cells);
                    }
                    other => sink.row(&[("_1", from_json(other))]),
                },
                (None, Rec::Csv(fields)) => {
                    while star_keys.len() < fields.len() {
                        let i = star_keys.len();
                        star_keys.push(match env.header.as_ref().and_then(|h| h.get(i)) {
                            Some(h) => h.clone(),
                            None => format!("_{}", i + 1),
                        });
                    }
                    let cells: Vec<_> = fields
                        .iter()
                        .zip(&star_keys)
                        .map(|(f, k)| (k.as_str(), Value::Str(Cow::Borrowed(f))))
                        .collect();
                    sink.row(&cells);
                }
                (None, Rec::None) => {}
            }
            rows += 1;
            Ok(q.limit.is_none_or(|l| rows < l))
        };

        match &self.input {
            Input::Csv(o) => {
                let text = std::str::from_utf8(&data).map_err(|_| err("InvalidTextEncoding", "The object is not valid UTF-8"))?;
                // the header comes first, whatever the scan range
                let mut first = None;
                if o.header != HeaderInfo::None {
                    input::csv_records(text, o, |off, f| {
                        first = Some((off, f.iter().map(|c| c.to_string()).collect::<Vec<_>>()));
                        Ok(false)
                    })?;
                }
                let skip = first.as_ref().map(|(off, _)| *off);
                if o.header == HeaderInfo::Use {
                    env.header = first.map(|(_, h)| h);
                }
                input::csv_records(text, o, |off, fields| {
                    if Some(off) == skip || (off as u64) < lo || (off as u64) >= hi {
                        return Ok((off as u64) < hi);
                    }
                    record(&env, Rec::Csv(fields), &mut sink)
                })?;
            }
            Input::Json { .. } => {
                let (path, has_path) = (&q.source.path, !q.source.path.is_empty());
                input::json_values(&data, |off, v| {
                    if (off as u64) >= hi {
                        return Ok(false);
                    }
                    if (off as u64) < lo {
                        return Ok(true);
                    }
                    walk(v, path, has_path, &mut |r| record(&env, Rec::Json(r), &mut sink))
                })?;
            }
        }

        if q.aggregate {
            let results: Vec<Value<'static>> = accs.into_iter().map(Acc::finish).collect();
            let items = q.items.as_deref().unwrap_or_default();
            let mut cells = Vec::with_capacity(items.len());
            for (item, name) in items.iter().zip(&names) {
                cells.push((name.as_str(), env.eval(&item.expr, Rec::None, &results)?));
            }
            sink.row(&cells);
        }

        let scanned = (hi - lo) as i64;
        let (chunks, returned) = sink.finish();
        let mut events: Vec<S3Result<SelectObjectContentEvent>> = chunks
            .into_iter()
            .map(|payload| Ok(SelectObjectContentEvent::Records(RecordsEvent { payload: Some(payload) })))
            .collect();
        let (bytes_scanned, bytes_processed, bytes_returned) = (Some(scanned), Some(scanned), Some(returned as i64));
        if self.progress {
            events.push(Ok(SelectObjectContentEvent::Progress(ProgressEvent {
                details: Some(Progress {
                    bytes_scanned,
                    bytes_processed,
                    bytes_returned,
                }),
            })));
        }
        events.push(Ok(SelectObjectContentEvent::Stats(StatsEvent {
            details: Some(Stats {
                bytes_scanned,
                bytes_processed,
                bytes_returned,
            }),
        })));
        events.push(Ok(SelectObjectContentEvent::End(EndEvent {})));
        Ok(events)
    }
}

/// The key a projection gets in JSON output: its alias, the last name of its path, else `_N`.
fn item_name(item: &sql::Item, i: usize) -> String {
    if let Some(a) = &item.alias {
        return a.clone();
    }
    match &item.expr {
        Expr::Path { name, steps, .. } => match steps.last() {
            Some(Step::Field(f)) => f.clone(),
            None => name.clone(),
            _ => format!("_{}", i + 1),
        },
        _ => format!("_{}", i + 1),
    }
}

/// The records `FROM S3Object[path]` makes of one JSON value: the value the path leads to, or its
/// elements if that is an array. Returns `false` once `f` asked to stop.
fn walk(v: &Json, steps: &[Step], has_path: bool, f: &mut dyn FnMut(&Json) -> S3Result<bool>) -> S3Result<bool> {
    match steps.split_first() {
        None => match v {
            Json::Array(a) if has_path => {
                for x in a {
                    if !f(x)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            _ => f(v),
        },
        Some((Step::Field(n), rest)) => v.get(n).map_or(Ok(true), |c| walk(c, rest, has_path, f)),
        Some((Step::Index(i), rest)) => v.get(*i).map_or(Ok(true), |c| walk(c, rest, has_path, f)),
        Some((Step::Wildcard, rest)) => match v {
            Json::Array(a) => {
                for x in a {
                    if !walk(x, rest, has_path, f)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            other => walk(other, rest, has_path, f),
        },
    }
}

impl FakeS3 {
    pub(crate) async fn do_select_object_content(
        &self,
        req: S3Request<SelectObjectContentInput>,
    ) -> S3Result<S3Response<SelectObjectContentOutput>> {
        let input = req.input;
        let (v, _) = self.version(&input.bucket, &input.key, None)?;
        let o = obj(&v);
        objext::check_read_key(
            o.ext.as_deref(),
            input.sse_customer_algorithm.as_deref(),
            input.sse_customer_key.as_deref(),
            input.sse_customer_key_md5.as_deref(),
        )?;
        if let Some(a) = self.archive_state(&input.bucket, &input.key, o) {
            a.require_readable(false)?;
        }
        let plan = Plan::new(&input.request)?;
        let mut segments = o.data.slice(0..o.data.size);
        let data = match segments.len() {
            0 => Bytes::new(),
            1 => segments.remove(0),
            _ => {
                let mut b = BytesMut::with_capacity(o.data.size as usize);
                segments.iter().for_each(|s| b.extend_from_slice(s));
                b.freeze()
            }
        };
        let events = tokio::task::spawn_blocking(move || plan.run(data))
            .await
            .map_err(|_| S3Error::with_message(S3ErrorCode::InternalError, "The S3 Select query failed"))??;
        Ok(S3Response::new(SelectObjectContentOutput {
            payload: Some(SelectObjectContentEventStream::new(futures::stream::iter(events))),
        }))
    }
}
