//! The SQL dialect of S3 Select: a lexer and a recursive-descent parser for
//! `SELECT <projections> FROM S3Object[path] [alias] [WHERE <expr>] [LIMIT n]`.

use s3s::S3Error;

use super::err;

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Field(String),
    Index(usize),
    Wildcard,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cmp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Arith {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Concat,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ty {
    Int,
    Float,
    Str,
    Bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrimSide {
    Both,
    Leading,
    Trailing,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AggFn {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Func {
    Lower,
    Upper,
    CharLength,
    Substring,
    Trim(TrimSide),
    Coalesce,
    NullIf,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    /// A column, a field of the record or a SELECT alias, optionally followed by a path. `quoted` names
    /// came in double quotes; if nothing of that name exists they are read as string literals, which
    /// is how Ceph's s3select (and the s3-tests written for it) spell literals.
    Path {
        name: String,
        quoted: bool,
        steps: Vec<Step>,
    },
    Not(Box<Expr>),
    Neg(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Cmp(Cmp, Box<Expr>, Box<Expr>),
    Arith(Arith, Box<Expr>, Box<Expr>),
    Like {
        e: Box<Expr>,
        pattern: Box<Expr>,
        escape: Option<Box<Expr>>,
        neg: bool,
    },
    In {
        e: Box<Expr>,
        list: Vec<Expr>,
        neg: bool,
    },
    Between {
        e: Box<Expr>,
        lo: Box<Expr>,
        hi: Box<Expr>,
        neg: bool,
    },
    IsNull {
        e: Box<Expr>,
        neg: bool,
        missing: bool,
    },
    Case {
        operand: Option<Box<Expr>>,
        whens: Vec<(Expr, Expr)>,
        otherwise: Option<Box<Expr>>,
    },
    Cast(Box<Expr>, Ty),
    Func(Func, Vec<Expr>),
    /// The result of `Query::aggs[idx]`.
    Agg(usize),
}

pub struct AggSpec {
    pub func: AggFn,
    /// `None` for `COUNT(*)`.
    pub arg: Option<Expr>,
}

pub struct Item {
    pub expr: Expr,
    pub alias: Option<String>,
}

pub struct Source {
    pub path: Vec<Step>,
    pub alias: Option<String>,
}

pub struct Query {
    /// `None` is `SELECT *`.
    pub items: Option<Vec<Item>>,
    pub source: Source,
    pub filter: Option<Expr>,
    pub limit: Option<u64>,
    pub aggs: Vec<AggSpec>,
    /// Some projection contains an aggregate: the query yields one row.
    pub aggregate: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Quoted(String),
    Str(String),
    Int(i64),
    Float(f64),
    Sym(&'static str),
}

fn lex(sql: &str) -> Result<Vec<Tok>, S3Error> {
    let b = sql.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$') {
                    i += 1;
                }
                out.push(Tok::Ident(sql[start..i].to_owned()));
            }
            b'0'..=b'9' => {
                let start = i;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                let mut float = false;
                if i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit() {
                    float = true;
                    i += 1;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                    let mut j = i + 1;
                    if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
                        j += 1;
                    }
                    if j < b.len() && b[j].is_ascii_digit() {
                        while j < b.len() && b[j].is_ascii_digit() {
                            j += 1;
                        }
                        float = true;
                        i = j;
                    }
                }
                let text = &sql[start..i];
                let bad = || err("LexerInvalidChar", format!("invalid number {text}"));
                out.push(if float {
                    Tok::Float(text.parse().map_err(|_| bad())?)
                } else {
                    match text.parse() {
                        Ok(n) => Tok::Int(n),
                        Err(_) => Tok::Float(text.parse().map_err(|_| bad())?),
                    }
                });
            }
            b'\'' | b'"' => {
                let mut s = String::new();
                let mut j = i + 1;
                loop {
                    let Some(rel) = sql[j..].find(c as char) else {
                        return Err(err("LexerInvalidChar", "unterminated quoted string"));
                    };
                    s.push_str(&sql[j..j + rel]);
                    j += rel + 1;
                    if b.get(j) == Some(&c) {
                        s.push(c as char);
                        j += 1;
                    } else {
                        break;
                    }
                }
                i = j;
                out.push(if c == b'\'' { Tok::Str(s) } else { Tok::Quoted(s) });
            }
            _ => {
                let two = sql.get(i..i + 2).unwrap_or("");
                let sym = match two {
                    "<=" => Some("<="),
                    ">=" => Some(">="),
                    "!=" => Some("!="),
                    "<>" => Some("!="),
                    "||" => Some("||"),
                    _ => None,
                };
                if let Some(s) = sym {
                    out.push(Tok::Sym(s));
                    i += 2;
                    continue;
                }
                let s = match c {
                    b'(' => "(",
                    b')' => ")",
                    b'[' => "[",
                    b']' => "]",
                    b',' => ",",
                    b'.' => ".",
                    b'*' => "*",
                    b'+' => "+",
                    b'-' => "-",
                    b'/' => "/",
                    b'%' => "%",
                    b'=' => "=",
                    b'<' => "<",
                    b'>' => ">",
                    b';' => ";",
                    _ => {
                        let ch = sql[i..].chars().next().unwrap_or('?');
                        return Err(err("LexerInvalidChar", format!("invalid character {ch:?} in the SQL expression")));
                    }
                };
                out.push(Tok::Sym(s));
                i += 1;
            }
        }
    }
    Ok(out)
}

/// Words that end an alias-less projection or source.
const RESERVED: &[&str] = &[
    "FROM", "WHERE", "LIMIT", "AND", "OR", "NOT", "AS", "IS", "IN", "LIKE", "BETWEEN", "GROUP", "ORDER", "HAVING", "UNION", "JOIN",
    "OFFSET", "ESCAPE", "THEN", "ELSE", "END", "WHEN",
];

/// Clauses S3 Select does not have at all.
const UNSUPPORTED: &[&str] = &[
    "GROUP",
    "ORDER",
    "HAVING",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "JOIN",
    "OFFSET",
    "DISTINCT",
    "INNER",
    "LEFT",
    "RIGHT",
    "CROSS",
];

pub fn parse(sql: &str) -> Result<Query, S3Error> {
    let mut p = Parser {
        toks: lex(sql)?,
        pos: 0,
        aggs: Vec::new(),
        in_where: false,
    };
    let q = p.query()?;
    Ok(q)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    aggs: Vec<AggSpec>,
    in_where: bool,
}

fn unexpected(t: Option<&Tok>) -> S3Error {
    match t {
        Some(t) => err(
            "ParseUnexpectedToken",
            format!("Unexpected token found {t:?} in the SQL expression"),
        ),
        None => err("ParseUnexpectedToken", "Unexpected end of the SQL expression"),
    }
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(s)) if s.eq_ignore_ascii_case(kw))
    }

    fn is_kw_at(&self, off: usize, kw: &str) -> bool {
        matches!(self.toks.get(self.pos + off), Some(Tok::Ident(s)) if s.eq_ignore_ascii_case(kw))
    }

    fn accept_kw(&mut self, kw: &str) -> bool {
        let hit = self.is_kw(kw);
        if hit {
            self.pos += 1;
        }
        hit
    }

    fn expect_kw(&mut self, kw: &str) -> Result<(), S3Error> {
        if self.accept_kw(kw) {
            Ok(())
        } else {
            Err(err(
                "ParseExpectedKeyword",
                format!("Expected keyword {kw}, found {:?}", self.peek()),
            ))
        }
    }

    fn is_sym(&self, s: &str) -> bool {
        matches!(self.peek(), Some(Tok::Sym(x)) if *x == s)
    }

    fn accept_sym(&mut self, s: &str) -> bool {
        let hit = self.is_sym(s);
        if hit {
            self.pos += 1;
        }
        hit
    }

    fn expect_sym(&mut self, s: &str) -> Result<(), S3Error> {
        if self.accept_sym(s) { Ok(()) } else { Err(unexpected(self.peek())) }
    }

    fn query(&mut self) -> Result<Query, S3Error> {
        self.expect_kw("SELECT")?;
        if let Some(Tok::Ident(w)) = self.peek()
            && UNSUPPORTED.iter().any(|u| w.eq_ignore_ascii_case(u))
        {
            return Err(unsupported(w));
        }
        let mut items = None;
        if !self.accept_sym("*") {
            let first = self.item()?;
            // `s.*` alone is the same as `*`
            let star = matches!(&first.expr, Expr::Path { steps, .. } if steps.last() == Some(&Step::Wildcard));
            if !(star && self.is_kw("FROM")) {
                let mut v = vec![first];
                while self.accept_sym(",") {
                    v.push(self.item()?);
                }
                items = Some(v);
            }
        }
        if !self.is_kw("FROM") {
            return Err(if self.peek().is_none() {
                err("ParseSelectMissingFrom", "The SELECT list needs a FROM clause")
            } else {
                unexpected(self.peek())
            });
        }
        self.pos += 1;
        let source = self.source()?;
        let mut filter = None;
        if self.accept_kw("WHERE") {
            self.in_where = true;
            filter = Some(self.expr()?);
            self.in_where = false;
        }
        let mut limit = None;
        if self.accept_kw("LIMIT") {
            let negative = self.accept_sym("-");
            match self.next() {
                Some(Tok::Int(_)) if negative => return Err(err("NegativeLimit", "LIMIT must not be negative")),
                Some(Tok::Int(n)) => limit = Some(n as u64),
                t => return Err(unexpected(t.as_ref())),
            }
        }
        self.accept_sym(";");
        match self.peek() {
            None => {}
            Some(Tok::Ident(w)) if UNSUPPORTED.iter().any(|u| w.eq_ignore_ascii_case(u)) => return Err(unsupported(w)),
            t => return Err(unexpected(t)),
        }
        let aggregate = items.as_ref().is_some_and(|v| v.iter().any(|i| has_agg(&i.expr)));
        Ok(Query {
            items,
            source,
            filter,
            limit,
            aggs: std::mem::take(&mut self.aggs),
            aggregate,
        })
    }

    fn alias(&mut self) -> Result<Option<String>, S3Error> {
        if self.accept_kw("AS") {
            return match self.next() {
                Some(Tok::Ident(s) | Tok::Quoted(s)) => Ok(Some(s)),
                t => Err(err("ParseExpectedIdentForAlias", format!("Expected an alias, found {t:?}"))),
            };
        }
        match self.peek() {
            Some(Tok::Ident(s))
                if !RESERVED.iter().any(|r| s.eq_ignore_ascii_case(r)) && !UNSUPPORTED.iter().any(|r| s.eq_ignore_ascii_case(r)) =>
            {
                let s = s.clone();
                self.pos += 1;
                Ok(Some(s))
            }
            Some(Tok::Quoted(s)) => {
                let s = s.clone();
                self.pos += 1;
                Ok(Some(s))
            }
            _ => Ok(None),
        }
    }

    fn item(&mut self) -> Result<Item, S3Error> {
        let expr = self.expr()?;
        let alias = self.alias()?;
        Ok(Item { expr, alias })
    }

    fn source(&mut self) -> Result<Source, S3Error> {
        match self.next() {
            Some(Tok::Ident(s)) if s.eq_ignore_ascii_case("S3Object") || s.eq_ignore_ascii_case("stdin") => {}
            t => {
                return Err(err(
                    "ParseUnexpectedToken",
                    format!("The data source must be S3Object, found {t:?}"),
                ));
            }
        }
        let path = self.steps()?;
        let alias = self.alias()?;
        Ok(Source { path, alias })
    }

    /// `.name`, `."name"`, `[3]`, `["name"]`, `[*]` and `.*` after a head.
    fn steps(&mut self) -> Result<Vec<Step>, S3Error> {
        let mut steps = Vec::new();
        loop {
            if self.accept_sym(".") {
                match self.next() {
                    Some(Tok::Ident(s) | Tok::Quoted(s)) => steps.push(Step::Field(s)),
                    Some(Tok::Sym("*")) => steps.push(Step::Wildcard),
                    t => return Err(err("ParseInvalidPathComponent", format!("Invalid path component {t:?}"))),
                }
            } else if self.is_sym("[") {
                self.pos += 1;
                match self.next() {
                    Some(Tok::Sym("*")) => steps.push(Step::Wildcard),
                    Some(Tok::Int(n)) if n >= 0 => steps.push(Step::Index(n as usize)),
                    Some(Tok::Str(s) | Tok::Quoted(s)) => steps.push(Step::Field(s)),
                    None => return Err(unexpected(None)),
                    t => return Err(err("ParseInvalidPathComponent", format!("Invalid path component {t:?}"))),
                }
                self.expect_sym("]")?;
            } else {
                return Ok(steps);
            }
        }
    }

    pub fn expr(&mut self) -> Result<Expr, S3Error> {
        let mut l = self.and()?;
        while self.accept_kw("OR") {
            l = Expr::Or(Box::new(l), Box::new(self.and()?));
        }
        Ok(l)
    }

    fn and(&mut self) -> Result<Expr, S3Error> {
        let mut l = self.not()?;
        while self.accept_kw("AND") {
            l = Expr::And(Box::new(l), Box::new(self.not()?));
        }
        Ok(l)
    }

    fn not(&mut self) -> Result<Expr, S3Error> {
        if self.accept_kw("NOT") {
            return Ok(Expr::Not(Box::new(self.not()?)));
        }
        self.comparison()
    }

    fn comparison(&mut self) -> Result<Expr, S3Error> {
        let mut l = self.additive()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Sym("=")) => Some(Cmp::Eq),
                Some(Tok::Sym("!=")) => Some(Cmp::Ne),
                Some(Tok::Sym("<")) => Some(Cmp::Lt),
                Some(Tok::Sym("<=")) => Some(Cmp::Le),
                Some(Tok::Sym(">")) => Some(Cmp::Gt),
                Some(Tok::Sym(">=")) => Some(Cmp::Ge),
                _ => None,
            };
            if let Some(op) = op {
                self.pos += 1;
                l = Expr::Cmp(op, Box::new(l), Box::new(self.additive()?));
                continue;
            }
            if self.accept_kw("IS") {
                let neg = self.accept_kw("NOT");
                let missing = if self.accept_kw("NULL") {
                    false
                } else if self.accept_kw("MISSING") {
                    true
                } else {
                    return Err(unexpected(self.peek()));
                };
                l = Expr::IsNull {
                    e: Box::new(l),
                    neg,
                    missing,
                };
                continue;
            }
            let neg = self.is_kw("NOT") && (self.is_kw_at(1, "LIKE") || self.is_kw_at(1, "IN") || self.is_kw_at(1, "BETWEEN"));
            if neg {
                self.pos += 1;
            }
            if self.accept_kw("LIKE") {
                let pattern = Box::new(self.additive()?);
                let escape = if self.accept_kw("ESCAPE") {
                    Some(Box::new(self.additive()?))
                } else {
                    None
                };
                l = Expr::Like {
                    e: Box::new(l),
                    pattern,
                    escape,
                    neg,
                };
            } else if self.accept_kw("IN") {
                self.expect_sym("(")?;
                let mut list = vec![self.expr()?];
                while self.accept_sym(",") {
                    list.push(self.expr()?);
                }
                self.expect_sym(")")?;
                l = Expr::In { e: Box::new(l), list, neg };
            } else if self.accept_kw("BETWEEN") {
                let lo = Box::new(self.additive()?);
                self.expect_kw("AND")?;
                let hi = Box::new(self.additive()?);
                l = Expr::Between {
                    e: Box::new(l),
                    lo,
                    hi,
                    neg,
                };
            } else {
                return Ok(l);
            }
        }
    }

    fn additive(&mut self) -> Result<Expr, S3Error> {
        let mut l = self.multiplicative()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Sym("+")) => Arith::Add,
                Some(Tok::Sym("-")) => Arith::Sub,
                Some(Tok::Sym("||")) => Arith::Concat,
                _ => return Ok(l),
            };
            self.pos += 1;
            l = Expr::Arith(op, Box::new(l), Box::new(self.multiplicative()?));
        }
    }

    fn multiplicative(&mut self) -> Result<Expr, S3Error> {
        let mut l = self.unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Sym("*")) => Arith::Mul,
                Some(Tok::Sym("/")) => Arith::Div,
                Some(Tok::Sym("%")) => Arith::Mod,
                _ => return Ok(l),
            };
            self.pos += 1;
            l = Expr::Arith(op, Box::new(l), Box::new(self.unary()?));
        }
    }

    fn unary(&mut self) -> Result<Expr, S3Error> {
        if self.accept_sym("-") {
            return Ok(match self.unary()? {
                Expr::Int(n) => Expr::Int(-n),
                Expr::Float(f) => Expr::Float(-f),
                e => Expr::Neg(Box::new(e)),
            });
        }
        if self.accept_sym("+") {
            return self.unary();
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr, S3Error> {
        match self.next() {
            Some(Tok::Int(n)) => Ok(Expr::Int(n)),
            Some(Tok::Float(f)) => Ok(Expr::Float(f)),
            Some(Tok::Str(s)) => Ok(Expr::Str(s)),
            Some(Tok::Quoted(name)) => {
                let steps = self.steps()?;
                Ok(Expr::Path { name, quoted: true, steps })
            }
            Some(Tok::Sym("(")) => {
                let e = self.expr()?;
                self.expect_sym(")")?;
                Ok(e)
            }
            Some(Tok::Ident(name)) => {
                if self.is_sym("(") {
                    return self.call(&name);
                }
                let up = name.to_ascii_uppercase();
                match up.as_str() {
                    "NULL" => return Ok(Expr::Null),
                    "TRUE" => return Ok(Expr::Bool(true)),
                    "FALSE" => return Ok(Expr::Bool(false)),
                    "CASE" => return self.case(),
                    "CAST" | "SELECT" | "FROM" | "WHERE" => return Err(unexpected(Some(&Tok::Ident(name)))),
                    _ => {}
                }
                let steps = self.steps()?;
                Ok(Expr::Path {
                    name,
                    quoted: false,
                    steps,
                })
            }
            t => Err(unexpected(t.as_ref())),
        }
    }

    fn case(&mut self) -> Result<Expr, S3Error> {
        let operand = if self.is_kw("WHEN") { None } else { Some(Box::new(self.expr()?)) };
        let mut whens = Vec::new();
        while self.accept_kw("WHEN") {
            let w = self.expr()?;
            self.expect_kw("THEN")?;
            whens.push((w, self.expr()?));
        }
        if whens.is_empty() {
            return Err(err("ParseExpectedWhenClause", "CASE needs at least one WHEN clause"));
        }
        let otherwise = if self.accept_kw("ELSE") {
            Some(Box::new(self.expr()?))
        } else {
            None
        };
        self.expect_kw("END")?;
        Ok(Expr::Case { operand, whens, otherwise })
    }

    fn args(&mut self) -> Result<Vec<Expr>, S3Error> {
        let mut v = Vec::new();
        if !self.is_sym(")") {
            v.push(self.expr()?);
            while self.accept_sym(",") {
                v.push(self.expr()?);
            }
        }
        self.expect_sym(")")?;
        Ok(v)
    }

    fn call(&mut self, name: &str) -> Result<Expr, S3Error> {
        self.expect_sym("(")?;
        let lower = name.to_ascii_lowercase();
        let agg = match lower.as_str() {
            "count" => Some(AggFn::Count),
            "sum" => Some(AggFn::Sum),
            "avg" => Some(AggFn::Avg),
            "min" => Some(AggFn::Min),
            "max" => Some(AggFn::Max),
            _ => None,
        };
        if let Some(func) = agg {
            if self.in_where {
                return Err(err(
                    "UnsupportedSqlOperation",
                    "Aggregate functions are not allowed in the WHERE clause",
                ));
            }
            let arg = if func == AggFn::Count && self.accept_sym("*") {
                None
            } else {
                let a = self.expr()?;
                if has_agg(&a) {
                    return Err(err("UnsupportedSqlOperation", "Aggregate functions cannot be nested"));
                }
                Some(a)
            };
            self.expect_sym(")")?;
            self.aggs.push(AggSpec { func, arg });
            return Ok(Expr::Agg(self.aggs.len() - 1));
        }
        let arity = |args: &[Expr], lo: usize, hi: usize| -> Result<(), S3Error> {
            if (lo..=hi).contains(&args.len()) {
                Ok(())
            } else {
                Err(err("EvaluatorInvalidArguments", format!("Incorrect number of arguments to {name}")))
            }
        };
        match lower.as_str() {
            "cast" => {
                let e = self.expr()?;
                self.expect_kw("AS")?;
                let ty = self.ty()?;
                self.expect_sym(")")?;
                Ok(Expr::Cast(Box::new(e), ty))
            }
            // `int(x)` is Ceph's spelling of CAST(x AS INT); kept because the s3-tests use it.
            "int" | "integer" => {
                let a = self.args()?;
                arity(&a, 1, 1)?;
                Ok(Expr::Cast(Box::new(a.into_iter().next().unwrap_or(Expr::Null)), Ty::Int))
            }
            "lower" | "upper" | "char_length" | "character_length" | "coalesce" | "nullif" => {
                let a = self.args()?;
                let (f, lo, hi) = match lower.as_str() {
                    "lower" => (Func::Lower, 1, 1),
                    "upper" => (Func::Upper, 1, 1),
                    "coalesce" => (Func::Coalesce, 1, usize::MAX),
                    "nullif" => (Func::NullIf, 2, 2),
                    _ => (Func::CharLength, 1, 1),
                };
                arity(&a, lo, hi)?;
                Ok(Expr::Func(f, a))
            }
            "substring" => {
                let s = self.expr()?;
                let mut a = vec![s];
                if self.accept_kw("FROM") {
                    a.push(self.expr()?);
                    if self.accept_kw("FOR") {
                        a.push(self.expr()?);
                    }
                } else {
                    while self.accept_sym(",") {
                        a.push(self.expr()?);
                    }
                }
                self.expect_sym(")")?;
                arity(&a, 2, 3)?;
                Ok(Expr::Func(Func::Substring, a))
            }
            "trim" => {
                let side = if self.accept_kw("BOTH") {
                    Some(TrimSide::Both)
                } else if self.accept_kw("LEADING") {
                    Some(TrimSide::Leading)
                } else if self.accept_kw("TRAILING") {
                    Some(TrimSide::Trailing)
                } else {
                    None
                };
                // trim([side] [chars] FROM x) | trim(x)
                let mut a = Vec::new();
                if !self.is_kw("FROM") {
                    a.push(self.expr()?);
                }
                if self.accept_kw("FROM") {
                    a.push(self.expr()?);
                    a.reverse(); // [x, chars]
                } else if side.is_some() {
                    return Err(err("ParseExpectedKeyword", "Expected keyword FROM in TRIM"));
                }
                self.expect_sym(")")?;
                arity(&a, 1, 2)?;
                Ok(Expr::Func(Func::Trim(side.unwrap_or(TrimSide::Both)), a))
            }
            "extract" | "date_add" | "date_diff" | "to_timestamp" | "to_string" | "utcnow" | "from_unixtime" | "to_unixtime" => Err(err(
                "UnsupportedSqlOperation",
                format!("The function {name} is not supported by fakes3"),
            )),
            _ => Err(err("UnsupportedFunction", format!("The function {name} is not supported"))),
        }
    }

    fn ty(&mut self) -> Result<Ty, S3Error> {
        let name = match self.next() {
            Some(Tok::Ident(s)) => s.to_ascii_uppercase(),
            t => return Err(err("ParseExpectedTypeName", format!("Expected a type name, found {t:?}"))),
        };
        // DECIMAL(10, 2), VARCHAR(20): the parameters are accepted and ignored
        if self.accept_sym("(") {
            while !self.accept_sym(")") {
                if self.next().is_none() {
                    return Err(unexpected(None));
                }
            }
        }
        match name.as_str() {
            "INT" | "INTEGER" | "BIGINT" | "SMALLINT" => Ok(Ty::Int),
            "FLOAT" | "DOUBLE" | "REAL" | "DECIMAL" | "NUMERIC" => Ok(Ty::Float),
            "STRING" | "VARCHAR" | "CHAR" | "TEXT" => Ok(Ty::Str),
            "BOOL" | "BOOLEAN" => Ok(Ty::Bool),
            "TIMESTAMP" => Err(err("UnsupportedSqlOperation", "CAST to TIMESTAMP is not supported by fakes3")),
            _ => Err(err("ParseExpectedTypeName", format!("Unknown type {name}"))),
        }
    }
}

fn unsupported(word: &str) -> S3Error {
    err(
        "UnsupportedSqlOperation",
        format!("{} is not supported by S3 Select", word.to_ascii_uppercase()),
    )
}

pub fn has_agg(e: &Expr) -> bool {
    match e {
        Expr::Agg(_) => true,
        Expr::Not(a) | Expr::Neg(a) | Expr::Cast(a, _) => has_agg(a),
        Expr::And(a, b) | Expr::Or(a, b) | Expr::Cmp(_, a, b) | Expr::Arith(_, a, b) => has_agg(a) || has_agg(b),
        Expr::Like { e, pattern, escape, .. } => has_agg(e) || has_agg(pattern) || escape.as_deref().is_some_and(has_agg),
        Expr::In { e, list, .. } => has_agg(e) || list.iter().any(has_agg),
        Expr::Between { e, lo, hi, .. } => has_agg(e) || has_agg(lo) || has_agg(hi),
        Expr::IsNull { e, .. } => has_agg(e),
        Expr::Case { operand, whens, otherwise } => {
            operand.as_deref().is_some_and(has_agg)
                || whens.iter().any(|(a, b)| has_agg(a) || has_agg(b))
                || otherwise.as_deref().is_some_and(has_agg)
        }
        Expr::Func(_, a) => a.iter().any(has_agg),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(sql: &str) -> String {
        parse(sql).err().map(|e| e.code().as_str().to_owned()).unwrap_or_default()
    }

    #[test]
    fn parses_the_shapes_the_tests_use() {
        for sql in [
            "select count(0) from s3object;",
            "select * from S3Object s where s._1 > 5 and not s._2 like 'a%' limit 3",
            "select _1, int(_2) as a from s3object[*].root r where a in (1,2,3) or _3 between 1 and 9",
            "select case when _1 = 'x' then 1 else 2 end, cast(_1 as decimal(10,2)) from s3object",
            "select trim(both ' ' from _1), substring(_1 from 2 for 3), substring(_1,2) from stdin",
            "select min(int(_3))+1, count(*) from s3object where nullif(_1,_2) is not null",
            "select \"true\"from s3object where (int(_1) in (1,0,2))",
            "select s.name from s3object[*].a[0][\"b\"] s",
        ] {
            assert!(parse(sql).is_ok(), "{sql}: {}", code(sql));
        }
    }

    #[test]
    fn rejects_with_aws_codes() {
        assert_eq!(code("select * from s3object group by _1"), "UnsupportedSqlOperation");
        assert_eq!(code("select * from s3object order by _1"), "UnsupportedSqlOperation");
        assert_eq!(code("select"), "ParseUnexpectedToken");
        assert_eq!(code("select _1"), "ParseSelectMissingFrom");
        assert_eq!(code("select * from s3object where"), "ParseUnexpectedToken");
        assert_eq!(code("select * from other"), "ParseUnexpectedToken");
        assert_eq!(code("select * from s3object where _1 # 2"), "LexerInvalidChar");
        assert_eq!(code("select foo(_1) from s3object"), "UnsupportedFunction");
        assert_eq!(code("select to_timestamp(_1) from s3object"), "UnsupportedSqlOperation");
        assert_eq!(code("select count(sum(_1)) from s3object"), "UnsupportedSqlOperation");
        assert_eq!(code("select * from s3object where count(*) > 1"), "UnsupportedSqlOperation");
        assert_eq!(code("select * from s3object limit -1"), "NegativeLimit");
        assert_eq!(code("select cast(_1 as nothing) from s3object"), "ParseExpectedTypeName");
        assert_eq!(code("update s3object set a = 1"), "ParseExpectedKeyword");
    }
}
