//! Readers for the input formats: CSV records and JSON values.

use std::borrow::Cow;

use s3s::S3Error;
use serde_json::Value as Json;

use super::err;

#[derive(Clone, Copy, PartialEq)]
pub enum HeaderInfo {
    None,
    Ignore,
    Use,
}

pub struct CsvIn {
    pub field: u8,
    /// One or two bytes.
    pub record: Vec<u8>,
    pub quote: Option<u8>,
    pub escape: Option<u8>,
    pub comment: Option<u8>,
    pub header: HeaderInfo,
}

/// Calls `f(offset, fields)` for every non-empty record, until `f` returns `false`. A field in quotes
/// may contain delimiters and newlines; inside it the escape character (the quote character by
/// default) takes a quote or itself literally. A quote in the middle of a field is an ordinary character.
pub fn csv_records<'a>(
    data: &'a str,
    o: &CsvIn,
    mut f: impl FnMut(usize, &[Cow<'a, str>]) -> Result<bool, S3Error>,
) -> Result<(), S3Error> {
    let b = data.as_bytes();
    let n = b.len();
    let rd = &o.record;
    let is_rd = |i: usize| b[i] == rd[0] && (rd.len() == 1 || b.get(i + 1) == Some(&rd[1]));
    let mut fields: Vec<Cow<'a, str>> = Vec::new();
    let mut i = 0;
    while i < n {
        let start = i;
        if Some(b[i]) == o.comment {
            while i < n && !is_rd(i) {
                i += 1;
            }
            i = (i + rd.len()).min(n);
            continue;
        }
        fields.clear();
        loop {
            let fs = i;
            if i < n && Some(b[i]) == o.quote {
                i += 1;
                let mut buf = Vec::new();
                while i < n {
                    let c = b[i];
                    if Some(c) == o.escape && i + 1 < n && (Some(b[i + 1]) == o.quote || Some(b[i + 1]) == o.escape) {
                        buf.push(b[i + 1]);
                        i += 2;
                    } else if Some(c) == o.quote {
                        i += 1;
                        break;
                    } else {
                        buf.push(c);
                        i += 1;
                    }
                }
                while i < n && b[i] != o.field && !is_rd(i) {
                    buf.push(b[i]);
                    i += 1;
                }
                let s = String::from_utf8(buf).map_err(|_| err("InvalidTextEncoding", "The object is not valid UTF-8"))?;
                fields.push(Cow::Owned(s));
            } else {
                while i < n && b[i] != o.field && !is_rd(i) {
                    i += 1;
                }
                fields.push(Cow::Borrowed(&data[fs..i]));
            }
            if i >= n {
                break;
            }
            if b[i] == o.field {
                i += 1;
                if i >= n {
                    fields.push(Cow::Borrowed(""));
                    break;
                }
                continue;
            }
            i += rd.len();
            break;
        }
        if rd.as_slice() == b"\n"
            && let Some(last) = fields.last_mut()
            && last.ends_with('\r')
        {
            match last {
                Cow::Borrowed(s) => *s = &s[..s.len() - 1],
                Cow::Owned(s) => {
                    s.pop();
                }
            }
        }
        if fields.len() == 1 && fields[0].is_empty() {
            continue;
        }
        if !f(start, &fields)? {
            return Ok(());
        }
    }
    Ok(())
}

/// Calls `f(offset, value)` for every JSON value of the input (a stream of values, whether one per
/// line or a pretty-printed document), until `f` returns `false`.
pub fn json_values(data: &[u8], mut f: impl FnMut(usize, &Json) -> Result<bool, S3Error>) -> Result<(), S3Error> {
    let mut it = serde_json::Deserializer::from_slice(data).into_iter::<Json>();
    loop {
        let off = it.byte_offset();
        match it.next() {
            None => return Ok(()),
            Some(Ok(v)) => {
                if !f(off, &v)? {
                    return Ok(());
                }
            }
            Some(Err(e)) => return Err(err("JSONParsingError", format!("Invalid JSON input: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn csv(data: &str, field: u8, record: &[u8]) -> Vec<Vec<String>> {
        let o = CsvIn {
            field,
            record: record.to_vec(),
            quote: Some(b'"'),
            escape: Some(b'"'),
            comment: None,
            header: HeaderInfo::None,
        };
        let mut out = Vec::new();
        csv_records(data, &o, |_, f| {
            out.push(f.iter().map(|c| c.to_string()).collect());
            Ok(true)
        })
        .unwrap();
        out
    }

    #[test]
    fn csv_shapes() {
        assert_eq!(csv("a,b\nc,d\n", b',', b"\n"), [["a", "b"], ["c", "d"]]);
        assert_eq!(csv("a,b,\r\n\r\nc", b',', b"\n"), [vec!["a", "b", ""], vec!["c"]]);
        assert_eq!(csv("1|2|\t3|4|\t", b'|', b"\t"), [["1", "2", ""], ["3", "4", ""]]);
        assert_eq!(
            csv("\"x,y\",\"say \"\"hi\"\"\",z=\"q\"\n", b',', b"\n"),
            [["x,y", "say \"hi\"", "z=\"q\""]]
        );
        assert_eq!(csv("\"a\nb\",c", b',', b"\n"), [["a\nb", "c"]]);
        assert_eq!(csv(",,x,", b',', b"\n"), [["", "", "x", ""]]);
    }

    #[test]
    fn json_stream() {
        let mut n = 0;
        json_values(b"{\"a\":1}\n{\"a\":2}  [3]", |_, _| {
            n += 1;
            Ok(true)
        })
        .unwrap();
        assert_eq!(n, 3);
        assert!(json_values(b"{\"a\":", |_, _| Ok(true)).is_err());
    }
}
