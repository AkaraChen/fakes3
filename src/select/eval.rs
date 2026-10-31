//! Expression evaluation: values, SQL three-valued logic, the built-in functions and the aggregates.

use std::borrow::Cow;
use std::cell::Cell;
use std::cmp::Ordering;

use s3s::S3Error;
use serde_json::Value as Json;

use super::err;
use super::sql::{AggFn, Arith, Cmp, Expr, Func, Step, TrimSide, Ty};

#[derive(Debug, Clone)]
pub enum Value<'a> {
    Null,
    /// A field the record does not have (JSON); it is left out of JSON output.
    Missing,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Cow<'a, str>),
    Json(&'a Json),
}

impl Value<'_> {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null | Value::Missing)
    }

    pub fn into_owned(self) -> Value<'static> {
        match self {
            Value::Null => Value::Null,
            Value::Missing => Value::Missing,
            Value::Bool(b) => Value::Bool(b),
            Value::Int(n) => Value::Int(n),
            Value::Float(f) => Value::Float(f),
            Value::Str(s) => Value::Str(Cow::Owned(s.into_owned())),
            Value::Json(j) => Value::Str(Cow::Owned(j.to_string())),
        }
    }
}

pub fn from_json(v: &Json) -> Value<'_> {
    match v {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => n.as_i64().map_or_else(|| Value::Float(n.as_f64().unwrap_or(f64::NAN)), Value::Int),
        Json::String(s) => Value::Str(Cow::Borrowed(s)),
        other => Value::Json(other),
    }
}

/// The record a row is evaluated against.
#[derive(Clone, Copy)]
pub enum Rec<'a> {
    /// Finishing aggregates: no record, so a column reference is an error.
    None,
    Csv(&'a [Cow<'a, str>]),
    Json(&'a Json),
}

/// Gives names to the things an expression can refer to.
pub struct Env {
    /// Column names of a CSV with `FileHeaderInfo=USE`.
    pub header: Option<Vec<String>>,
    pub json: bool,
    pub from_alias: Option<String>,
    /// `expr AS name` of the projections; they can be used in later projections and in WHERE.
    pub aliases: Vec<(String, Expr)>,
    depth: Cell<u32>,
}

type R<T> = Result<T, S3Error>;

const MAX_ALIAS_DEPTH: u32 = 64;
const MAX_COLUMN: usize = 1_048_576;

impl Env {
    pub fn new(json: bool, header: Option<Vec<String>>, from_alias: Option<String>, aliases: Vec<(String, Expr)>) -> Self {
        Env {
            header,
            json,
            from_alias,
            aliases,
            depth: Cell::new(0),
        }
    }

    pub fn eval<'a>(&'a self, e: &'a Expr, rec: Rec<'a>, aggs: &'a [Value<'static>]) -> R<Value<'a>> {
        Ok(match e {
            Expr::Null => Value::Null,
            Expr::Bool(b) => Value::Bool(*b),
            Expr::Int(n) => Value::Int(*n),
            Expr::Float(f) => Value::Float(*f),
            Expr::Str(s) => Value::Str(Cow::Borrowed(s)),
            Expr::Agg(i) => aggs.get(*i).cloned().unwrap_or(Value::Null),
            Expr::Path { name, quoted, steps } => self.path(name, *quoted, steps, rec, aggs)?,
            Expr::Not(a) => match tri(&self.eval(a, rec, aggs)?) {
                Some(b) => Value::Bool(!b),
                None => Value::Null,
            },
            Expr::Neg(a) => match num(&self.eval(a, rec, aggs)?)? {
                Some(Num::I(n)) => n.checked_neg().map_or_else(|| Value::Float(-(n as f64)), Value::Int),
                Some(Num::F(f)) => Value::Float(-f),
                None => Value::Null,
            },
            Expr::And(a, b) => {
                let l = tri(&self.eval(a, rec, aggs)?);
                if l == Some(false) {
                    return Ok(Value::Bool(false));
                }
                match (l, tri(&self.eval(b, rec, aggs)?)) {
                    (_, Some(false)) => Value::Bool(false),
                    (Some(true), Some(true)) => Value::Bool(true),
                    _ => Value::Null,
                }
            }
            Expr::Or(a, b) => {
                let l = tri(&self.eval(a, rec, aggs)?);
                if l == Some(true) {
                    return Ok(Value::Bool(true));
                }
                match (l, tri(&self.eval(b, rec, aggs)?)) {
                    (_, Some(true)) => Value::Bool(true),
                    (Some(false), Some(false)) => Value::Bool(false),
                    _ => Value::Null,
                }
            }
            Expr::Cmp(op, a, b) => {
                let (l, r) = (self.eval(a, rec, aggs)?, self.eval(b, rec, aggs)?);
                match compare(&l, &r) {
                    None => Value::Null,
                    Some(o) => Value::Bool(match op {
                        Cmp::Eq => o == Ordering::Equal,
                        Cmp::Ne => o != Ordering::Equal,
                        Cmp::Lt => o == Ordering::Less,
                        Cmp::Le => o != Ordering::Greater,
                        Cmp::Gt => o == Ordering::Greater,
                        Cmp::Ge => o != Ordering::Less,
                    }),
                }
            }
            Expr::Arith(op, a, b) => arith(*op, self.eval(a, rec, aggs)?, self.eval(b, rec, aggs)?)?,
            Expr::Like { e, pattern, escape, neg } => {
                let (v, p) = (self.eval(e, rec, aggs)?, self.eval(pattern, rec, aggs)?);
                let esc = match escape {
                    Some(x) => match self.eval(x, rec, aggs)? {
                        Value::Str(s) if s.chars().count() == 1 => s.chars().next(),
                        v if v.is_null() => None,
                        _ => return Err(err("EvaluatorInvalidArguments", "ESCAPE must be a single character")),
                    },
                    None => None,
                };
                if v.is_null() || p.is_null() {
                    Value::Null
                } else {
                    let (v, p) = (to_text(&v), to_text(&p));
                    let hit = like(&p.chars().collect::<Vec<_>>(), &v.chars().collect::<Vec<_>>(), esc);
                    Value::Bool(hit != *neg)
                }
            }
            Expr::In { e, list, neg } => {
                let v = self.eval(e, rec, aggs)?;
                if v.is_null() {
                    return Ok(Value::Null);
                }
                let mut unknown = false;
                for item in list {
                    match compare(&v, &self.eval(item, rec, aggs)?) {
                        Some(Ordering::Equal) => return Ok(Value::Bool(!*neg)),
                        None => unknown = true,
                        _ => {}
                    }
                }
                if unknown { Value::Null } else { Value::Bool(*neg) }
            }
            Expr::Between { e, lo, hi, neg } => {
                let v = self.eval(e, rec, aggs)?;
                let (lo, hi) = (self.eval(lo, rec, aggs)?, self.eval(hi, rec, aggs)?);
                match (compare(&v, &lo), compare(&v, &hi)) {
                    (Some(a), Some(b)) => Value::Bool((a != Ordering::Less && b != Ordering::Greater) != *neg),
                    _ => Value::Null,
                }
            }
            Expr::IsNull { e, neg, missing } => {
                let v = self.eval(e, rec, aggs)?;
                let hit = if *missing { matches!(v, Value::Missing) } else { v.is_null() };
                Value::Bool(hit != *neg)
            }
            Expr::Case { operand, whens, otherwise } => {
                let op = operand.as_ref().map(|o| self.eval(o, rec, aggs)).transpose()?;
                for (w, t) in whens {
                    let w = self.eval(w, rec, aggs)?;
                    let hit = match &op {
                        Some(o) => compare(o, &w) == Some(Ordering::Equal),
                        None => tri(&w) == Some(true),
                    };
                    if hit {
                        return self.eval(t, rec, aggs);
                    }
                }
                match otherwise {
                    Some(o) => self.eval(o, rec, aggs)?,
                    None => Value::Null,
                }
            }
            Expr::Cast(a, ty) => cast(self.eval(a, rec, aggs)?, *ty)?,
            Expr::Func(f, args) => self.func(*f, args, rec, aggs)?,
        })
    }

    fn func<'a>(&'a self, f: Func, args: &'a [Expr], rec: Rec<'a>, aggs: &'a [Value<'static>]) -> R<Value<'a>> {
        match f {
            Func::Coalesce => {
                for a in args {
                    let v = self.eval(a, rec, aggs)?;
                    if !v.is_null() {
                        return Ok(v);
                    }
                }
                Ok(Value::Null)
            }
            Func::NullIf => {
                let (a, b) = (self.eval(&args[0], rec, aggs)?, self.eval(&args[1], rec, aggs)?);
                Ok(if compare(&a, &b) == Some(Ordering::Equal) { Value::Null } else { a })
            }
            Func::Lower | Func::Upper | Func::CharLength => {
                let v = self.eval(&args[0], rec, aggs)?;
                if v.is_null() {
                    return Ok(Value::Null);
                }
                let s = to_text(&v);
                Ok(match f {
                    Func::Lower => Value::Str(Cow::Owned(s.to_lowercase())),
                    Func::Upper => Value::Str(Cow::Owned(s.to_uppercase())),
                    _ => Value::Int(s.chars().count() as i64),
                })
            }
            Func::Substring => {
                let v = self.eval(&args[0], rec, aggs)?;
                let start = int_arg(&self.eval(&args[1], rec, aggs)?)?;
                let len = args.get(2).map(|a| self.eval(a, rec, aggs).and_then(|v| int_arg(&v))).transpose()?;
                if v.is_null() || start.is_none() || (args.len() == 3 && len.flatten().is_none()) {
                    return Ok(Value::Null);
                }
                let (start, len) = (start.unwrap_or(1), len.flatten());
                if len.is_some_and(|l| l < 0) {
                    return Err(err("EvaluatorInvalidArguments", "SUBSTRING length must not be negative"));
                }
                let chars: Vec<char> = to_text(&v).chars().collect();
                let lo = start.max(1);
                let hi = len.map_or(i64::MAX, |l| start.saturating_add(l)).min(chars.len() as i64 + 1);
                Ok(Value::Str(Cow::Owned(if hi <= lo {
                    String::new()
                } else {
                    chars[(lo - 1) as usize..(hi - 1) as usize].iter().collect()
                })))
            }
            Func::Trim(side) => {
                let v = self.eval(&args[0], rec, aggs)?;
                let set = args.get(1).map(|a| self.eval(a, rec, aggs)).transpose()?;
                if v.is_null() || set.as_ref().is_some_and(Value::is_null) {
                    return Ok(Value::Null);
                }
                let (s, set) = (
                    to_text(&v),
                    set.map_or(Cow::Borrowed(" "), |s| Cow::Owned(to_text(&s).into_owned())),
                );
                let m = |c: char| set.contains(c);
                let t = match side {
                    TrimSide::Both => s.trim_matches(m),
                    TrimSide::Leading => s.trim_start_matches(m),
                    TrimSide::Trailing => s.trim_end_matches(m),
                };
                Ok(Value::Str(Cow::Owned(t.to_owned())))
            }
        }
    }

    fn path<'a>(&'a self, name: &'a str, quoted: bool, steps: &'a [Step], rec: Rec<'a>, aggs: &'a [Value<'static>]) -> R<Value<'a>> {
        let head_is_record = self.from_alias.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(name))
            || name.eq_ignore_ascii_case("s3object")
            // the implicit alias of an unaliased FROM is `_1`; in CSV `_1` is the first column
            || (self.json && self.from_alias.is_none() && name == "_1");
        if head_is_record {
            let Rec::Json(j) = rec else {
                // CSV: `s._3`, the record itself has no value
                return match steps.split_first() {
                    Some((Step::Field(f), rest)) => {
                        let v = self.field(f, rec, true)?.unwrap_or(Value::Missing);
                        Ok(follow(v, rest))
                    }
                    _ => Err(err("UnsupportedSqlOperation", "A CSV record can only be used through its columns")),
                };
            };
            return Ok(follow(Value::Json(j), steps));
        }
        // `"_3"` in double quotes is a string, not the third column
        if let Some(v) = self.field(name, rec, !quoted)? {
            return Ok(follow(v, steps));
        }
        if let Some((_, ex)) = self.aliases.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
            let d = self.depth.get();
            if d >= MAX_ALIAS_DEPTH {
                return Err(err(
                    "ParseUnsupportedAlias",
                    "number of calls exceed maximum size, probably a cyclic reference to alias",
                ));
            }
            self.depth.set(d + 1);
            let r = self.eval(ex, rec, aggs);
            self.depth.set(d);
            return Ok(follow(r?, steps));
        }
        if quoted && steps.is_empty() {
            return Ok(Value::Str(Cow::Borrowed(name)));
        }
        if self.json {
            return Ok(Value::Missing);
        }
        Err(match self.header {
            Some(_) => err(
                "MissingHeaders",
                format!("Some headers in the query are missing from the file: {name}. Please check the file and try again."),
            ),
            None => err(
                "EvaluatorBindingDoesNotExist",
                format!("Binding {name} does not exist: a CSV without a header has only the columns _1, _2, ..."),
            ),
        })
    }

    /// A column of the record by name; `None` if the record has no such thing.
    fn field<'a>(&'a self, name: &str, rec: Rec<'a>, positional: bool) -> R<Option<Value<'a>>> {
        match rec {
            Rec::None => Err(err(
                "UnsupportedSqlOperation",
                "A column cannot be used next to an aggregate function",
            )),
            Rec::Json(j) => Ok(j.get(name).map(from_json)),
            Rec::Csv(fields) => {
                if let Some(n) = name
                    .strip_prefix('_')
                    .filter(|d| positional && !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
                {
                    return match n.parse::<usize>() {
                        Ok(n) if (1..=MAX_COLUMN).contains(&n) => {
                            Ok(Some(fields.get(n - 1).map_or(Value::Null, |f| Value::Str(Cow::Borrowed(f)))))
                        }
                        _ => Err(err(
                            "InvalidColumnIndex",
                            "Column index is invalid. Please check the service documentation and try again.",
                        )),
                    };
                }
                let Some(h) = &self.header else { return Ok(None) };
                let pos = h
                    .iter()
                    .position(|c| c == name)
                    .or_else(|| h.iter().position(|c| c.eq_ignore_ascii_case(name)));
                Ok(pos.map(|i| fields.get(i).map_or(Value::Null, |f| Value::Str(Cow::Borrowed(f)))))
            }
        }
    }
}

fn follow<'a>(mut v: Value<'a>, steps: &[Step]) -> Value<'a> {
    for s in steps {
        v = match (v, s) {
            (Value::Json(j), Step::Field(f)) => j.get(f).map_or(Value::Missing, from_json),
            (Value::Json(j), Step::Index(i)) => j.get(*i).map_or(Value::Missing, from_json),
            (v, Step::Wildcard) => v,
            _ => Value::Missing,
        };
    }
    v
}

#[derive(Clone, Copy)]
pub enum Num {
    I(i64),
    F(f64),
}

impl Num {
    fn f(self) -> f64 {
        match self {
            Num::I(n) => n as f64,
            Num::F(f) => f,
        }
    }

    fn value(self) -> Value<'static> {
        match self {
            Num::I(n) => Value::Int(n),
            Num::F(f) => Value::Float(f),
        }
    }
}

fn parse_num(s: &str) -> Option<Num> {
    let t = s.trim();
    if let Ok(n) = t.parse::<i64>() {
        return Some(Num::I(n));
    }
    // `f64::from_str` also takes "inf" and "nan"
    let digits = t.bytes().any(|b| b.is_ascii_digit());
    t.parse::<f64>().ok().filter(|f| digits && f.is_finite()).map(Num::F)
}

/// `Ok(None)` for NULL / MISSING; strings (CSV values) are read as numbers.
fn num(v: &Value) -> R<Option<Num>> {
    match v {
        Value::Null | Value::Missing => Ok(None),
        Value::Int(n) => Ok(Some(Num::I(*n))),
        Value::Float(f) => Ok(Some(Num::F(*f))),
        Value::Str(s) => parse_num(s)
            .map(Some)
            .ok_or_else(|| err("IncorrectSqlFunctionArgumentType", format!("{s:?} is not a number"))),
        _ => Err(err(
            "IncorrectSqlFunctionArgumentType",
            "Incorrect argument type: a number is expected",
        )),
    }
}

fn int_arg(v: &Value) -> R<Option<i64>> {
    Ok(match num(v)? {
        Some(Num::I(n)) => Some(n),
        Some(Num::F(f)) => Some(f as i64),
        None => None,
    })
}

/// Three-valued truth: `None` is unknown.
fn tri(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Str(s) if s.eq_ignore_ascii_case("true") => Some(true),
        Value::Str(s) if s.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

fn compare(a: &Value, b: &Value) -> Option<Ordering> {
    use Value::*;
    let numeric = |v: &Value| match v {
        Int(n) => Some(Num::I(*n)),
        Float(f) => Some(Num::F(*f)),
        _ => None,
    };
    match (a, b) {
        (Null | Missing, _) | (_, Null | Missing) => None,
        (Int(x), Int(y)) => Some(x.cmp(y)),
        (Str(x), Str(y)) => Some(x.as_ref().cmp(y.as_ref())),
        (Bool(x), Bool(y)) => Some(x.cmp(y)),
        (Json(x), Json(y)) => (x == y).then_some(Ordering::Equal),
        (Int(_) | Float(_), Int(_) | Float(_)) => numeric(a)?.f().partial_cmp(&numeric(b)?.f()),
        // a CSV value against a number: compare as numbers if the text is one
        (Str(s), Int(_) | Float(_)) => parse_num(s)?.f().partial_cmp(&numeric(b)?.f()),
        (Int(_) | Float(_), Str(s)) => numeric(a)?.f().partial_cmp(&parse_num(s)?.f()),
        (Bool(x), Str(_)) => tri(b).map(|y| x.cmp(&y)),
        (Str(_), Bool(y)) => tri(a).map(|x| x.cmp(y)),
        _ => None,
    }
}

fn arith<'a>(op: Arith, a: Value<'a>, b: Value<'a>) -> R<Value<'a>> {
    if op == Arith::Concat {
        return Ok(if a.is_null() || b.is_null() {
            Value::Null
        } else {
            Value::Str(Cow::Owned(format!("{}{}", to_text(&a), to_text(&b))))
        });
    }
    let (Some(x), Some(y)) = (num(&a)?, num(&b)?) else {
        return Ok(Value::Null);
    };
    let by_zero = || err("EvaluatorInvalidArguments", "Division by zero");
    Ok(match (x, y) {
        (Num::I(p), Num::I(q)) => {
            let r = match op {
                Arith::Add => p.checked_add(q),
                Arith::Sub => p.checked_sub(q),
                Arith::Mul => p.checked_mul(q),
                Arith::Div if q == 0 => return Err(by_zero()),
                Arith::Div => p.checked_div(q),
                Arith::Mod if q == 0 => return Err(by_zero()),
                _ => p.checked_rem(q),
            };
            match r {
                Some(n) => Value::Int(n),
                None => return arith(op, Value::Float(p as f64), Value::Float(q as f64)),
            }
        }
        _ => {
            let (p, q) = (x.f(), y.f());
            Value::Float(match op {
                Arith::Add => p + q,
                Arith::Sub => p - q,
                Arith::Mul => p * q,
                Arith::Div | Arith::Mod if q == 0.0 => return Err(by_zero()),
                Arith::Div => p / q,
                _ => p % q,
            })
        }
    })
}

fn cast(v: Value<'_>, ty: Ty) -> R<Value<'_>> {
    if v.is_null() {
        return Ok(Value::Null);
    }
    let failed = || {
        err(
            "CastFailed",
            "Attempt to convert from one data type to another using CAST failed in the SQL expression.",
        )
    };
    Ok(match ty {
        Ty::Str => Value::Str(Cow::Owned(to_text(&v).into_owned())),
        Ty::Int => match &v {
            Value::Bool(b) => Value::Int(i64::from(*b)),
            Value::Str(s) => match parse_num(s).ok_or_else(failed)? {
                Num::I(n) => Value::Int(n),
                Num::F(f) => Value::Int(f as i64),
            },
            Value::Json(_) => return Err(failed()),
            _ => Value::Int(match num(&v)? {
                Some(Num::I(n)) => n,
                Some(Num::F(f)) => f as i64,
                None => return Ok(Value::Null),
            }),
        },
        Ty::Float => match &v {
            Value::Bool(b) => Value::Float(f64::from(u8::from(*b))),
            Value::Str(s) => Value::Float(parse_num(s).ok_or_else(failed)?.f()),
            Value::Json(_) => return Err(failed()),
            _ => Value::Float(num(&v)?.map_or(0.0, Num::f)),
        },
        Ty::Bool => match &v {
            Value::Bool(_) => v,
            Value::Int(n) => Value::Bool(*n != 0),
            Value::Float(f) => Value::Bool(*f != 0.0),
            _ => Value::Bool(tri(&v).ok_or_else(failed)?),
        },
    })
}

pub fn fmt_float(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{f:.1}")
    } else {
        f.to_string()
    }
}

/// The text of a value as CSV output and string functions see it.
pub fn to_text<'a>(v: &'a Value) -> Cow<'a, str> {
    match v {
        Value::Null | Value::Missing => Cow::Borrowed(""),
        Value::Bool(b) => Cow::Borrowed(if *b { "true" } else { "false" }),
        Value::Int(n) => Cow::Owned(n.to_string()),
        Value::Float(f) => Cow::Owned(fmt_float(*f)),
        Value::Str(s) => Cow::Borrowed(s),
        Value::Json(j) => Cow::Owned(j.to_string()),
    }
}

enum Tk {
    Lit(char),
    One,
    Many,
    Class(bool, Vec<(char, char)>),
}

/// SQL `LIKE`: `%`, `_`, an optional escape character and, as in Ceph's engine, `[a-z]` classes.
fn like(pat: &[char], s: &[char], esc: Option<char>) -> bool {
    let mut toks = Vec::new();
    let mut i = 0;
    while i < pat.len() {
        let c = pat[i];
        i += 1;
        toks.push(match c {
            _ if Some(c) == esc && i < pat.len() => {
                i += 1;
                Tk::Lit(pat[i - 1])
            }
            '%' => Tk::Many,
            '_' => Tk::One,
            '[' => match pat[i..].iter().position(|&x| x == ']').filter(|&p| p > 0) {
                Some(end) => {
                    let mut body = &pat[i..i + end];
                    i += end + 1;
                    let neg = matches!(body.first(), Some('^' | '!'));
                    if neg {
                        body = &body[1..];
                    }
                    let mut ranges = Vec::new();
                    let mut j = 0;
                    while j < body.len() {
                        if j + 2 < body.len() && body[j + 1] == '-' {
                            ranges.push((body[j], body[j + 2]));
                            j += 3;
                        } else {
                            ranges.push((body[j], body[j]));
                            j += 1;
                        }
                    }
                    Tk::Class(neg, ranges)
                }
                None => Tk::Lit('['),
            },
            c => Tk::Lit(c),
        });
    }
    let one = |t: &Tk, c: char| match t {
        Tk::Lit(l) => *l == c,
        Tk::One => true,
        Tk::Class(neg, r) => r.iter().any(|&(a, b)| (a..=b).contains(&c)) != *neg,
        Tk::Many => false,
    };
    let (mut t, mut p) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while p < s.len() {
        match toks.get(t) {
            Some(Tk::Many) => {
                star = Some(t);
                mark = p;
                t += 1;
            }
            Some(tk) if one(tk, s[p]) => {
                t += 1;
                p += 1;
            }
            _ => match star {
                Some(st) => {
                    t = st + 1;
                    mark += 1;
                    p = mark;
                }
                None => return false,
            },
        }
    }
    toks[t..].iter().all(|tk| matches!(tk, Tk::Many))
}

/// Replaces `cur` by `v` if `v` orders as `want` against it.
fn keep(cur: &mut Option<Value<'static>>, v: Value<'_>, want: Ordering) {
    if cur.as_ref().is_none_or(|c| compare(&v, c) == Some(want)) {
        *cur = Some(v.into_owned());
    }
}

/// One aggregate being accumulated.
pub enum Acc {
    Count(i64),
    Sum(Option<Num>),
    Avg(f64, i64),
    Min(Option<Value<'static>>),
    Max(Option<Value<'static>>),
}

impl Acc {
    pub fn new(f: AggFn) -> Acc {
        match f {
            AggFn::Count => Acc::Count(0),
            AggFn::Sum => Acc::Sum(None),
            AggFn::Avg => Acc::Avg(0.0, 0),
            AggFn::Min => Acc::Min(None),
            AggFn::Max => Acc::Max(None),
        }
    }

    /// `None` is `COUNT(*)`'s row.
    pub fn feed(&mut self, v: Option<Value<'_>>) -> R<()> {
        let Some(v) = v else {
            if let Acc::Count(n) = self {
                *n += 1;
            }
            return Ok(());
        };
        if v.is_null() {
            return Ok(());
        }
        match self {
            Acc::Count(n) => *n += 1,
            Acc::Sum(s) => {
                let x = num(&v)?;
                *s = match (s.take(), x) {
                    (None, x) => x,
                    (s, None) => s,
                    (Some(Num::I(a)), Some(Num::I(b))) => Some(a.checked_add(b).map_or(Num::F(a as f64 + b as f64), Num::I)),
                    (Some(a), Some(b)) => Some(Num::F(a.f() + b.f())),
                };
            }
            Acc::Avg(sum, n) => {
                *sum += num(&v)?.map_or(0.0, Num::f);
                *n += 1;
            }
            Acc::Min(cur) => keep(cur, v, Ordering::Less),
            Acc::Max(cur) => keep(cur, v, Ordering::Greater),
        }
        Ok(())
    }

    pub fn finish(self) -> Value<'static> {
        match self {
            Acc::Count(n) => Value::Int(n),
            Acc::Sum(s) => s.map_or(Value::Null, Num::value),
            Acc::Avg(_, 0) => Value::Null,
            Acc::Avg(sum, n) => Value::Float(sum / n as f64),
            Acc::Min(v) | Acc::Max(v) => v.unwrap_or(Value::Null),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pat(p: &str, s: &str, esc: Option<char>) -> bool {
        like(&p.chars().collect::<Vec<_>>(), &s.chars().collect::<Vec<_>>(), esc)
    }

    #[test]
    fn like_patterns() {
        assert!(pat("%aeio%", "xxaeioyy", None));
        assert!(pat("_3", "13", None));
        assert!(!pat("_3", "113", None));
        assert!(pat("c%z", "cbcdz", None));
        assert!(pat("%y[y-z]", "vwxyz", None));
        assert!(!pat("%y[y-z]", "vwxya", None));
        assert!(pat("%aeio$_", "xaeio_", Some('$')));
        assert!(!pat("%aeio$_", "xaeioz", Some('$')));
        assert!(pat("%", "", None));
        assert!(pat("a[b", "a[b", None));
    }

    #[test]
    fn float_text() {
        assert_eq!(fmt_float(3.0), "3.0");
        assert_eq!(fmt_float(2.5), "2.5");
        assert_eq!(fmt_float(1e21), "1000000000000000000000");
    }
}
