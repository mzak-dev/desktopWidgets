//! Binding expressions (decision 11): arithmetic, comparison, booleans,
//! ternary, `{expr|fmt}` interpolation. Total by construction: no loops, no
//! user functions, no side effects, bounded depth and length, so evaluating
//! one can never hang a redraw.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use crate::value::Value;

const MAX_LEN: usize = 2000;
const MAX_DEPTH: usize = 48;

#[derive(Clone, Debug)]
enum Node {
    Lit(Value),
    Path(Vec<String>),
    Not(Box<Node>),
    Neg(Box<Node>),
    Bin(Op, Box<Node>, Box<Node>),
    Tern(Box<Node>, Box<Node>, Box<Node>),
    Call(String, Vec<Node>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Clone, Debug)]
pub struct Expr(Node);

/// Names visible to expressions, plus a record of which dotted paths an
/// evaluation actually read (the redraw scheduler's dependency set).
///
/// Names not set on the scope fall through to an optional provider (the Data
/// Sources), asked at most once per name and only when something reads it.
#[derive(Default)]
pub struct Scope<'a> {
    vars: Vec<(String, Value)>,
    deps: RefCell<BTreeSet<String>>,
    provider: Option<&'a dyn Fn(&str) -> Option<Value>>,
    provided: RefCell<BTreeMap<String, Value>>,
}

impl<'a> Scope<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    /// A scope whose unset names are looked up lazily in `provider`.
    pub fn with_provider(provider: &'a dyn Fn(&str) -> Option<Value>) -> Self {
        Scope { provider: Some(provider), ..Self::default() }
    }

    pub fn set(&mut self, name: &str, v: Value) {
        self.vars.push((name.to_string(), v));
    }

    pub fn pop(&mut self) {
        self.vars.pop();
    }

    pub fn deps(&self) -> BTreeSet<String> {
        self.deps.borrow().clone()
    }

    pub fn clear_deps(&self) {
        self.deps.borrow_mut().clear();
    }

    /// Read a top-level name without recording a dependency.
    pub fn peek(&self, name: &str) -> Option<&Value> {
        self.vars.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    fn lookup(&self, path: &[String]) -> Result<Value, String> {
        let walk = |root: &Value| -> Result<Value, String> {
            let mut cur = root;
            for seg in &path[1..] {
                cur = cur.get(seg).ok_or_else(|| format!("undefined `{}`", path.join(".")))?;
            }
            Ok(cur.clone())
        };
        let undefined = || format!("undefined name `{}`", path[0]);
        let v = match self.vars.iter().rev().find(|(n, _)| *n == path[0]) {
            Some((_, root)) => walk(root)?,
            None => {
                let mut provided = self.provided.borrow_mut();
                if !provided.contains_key(&path[0]) {
                    let v = self.provider.and_then(|p| p(&path[0])).ok_or_else(undefined)?;
                    provided.insert(path[0].clone(), v);
                }
                walk(&provided[&path[0]])?
            }
        };
        self.deps.borrow_mut().insert(path.join("."));
        Ok(v)
    }
}

// ---- lexer -----------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Sym(&'static str),
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let cs: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() {
            let s = i;
            while i < cs.len() && (cs[i].is_ascii_digit() || cs[i] == '.') {
                i += 1;
            }
            let t: String = cs[s..i].iter().collect();
            out.push(Tok::Num(t.parse().map_err(|_| format!("bad number `{t}`"))?));
        } else if c.is_alphabetic() || c == '_' {
            let s = i;
            while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_') {
                i += 1;
            }
            out.push(Tok::Ident(cs[s..i].iter().collect()));
        } else if c == '\'' || c == '"' {
            let q = c;
            i += 1;
            let s = i;
            while i < cs.len() && cs[i] != q {
                i += 1;
            }
            if i >= cs.len() {
                return Err("unterminated string".into());
            }
            out.push(Tok::Str(cs[s..i].iter().collect()));
            i += 1;
        } else {
            let two: String = cs[i..(i + 2).min(cs.len())].iter().collect();
            let sym = ["==", "!=", "<=", ">=", "&&", "||"].iter().find(|s| **s == two);
            if let Some(s) = sym {
                out.push(Tok::Sym(s));
                i += 2;
            } else {
                let one = ["+", "-", "*", "/", "%", "<", ">", "!", "?", ":", "(", ")", ",", "."]
                    .iter()
                    .find(|s| s.chars().next() == Some(c));
                out.push(Tok::Sym(one.ok_or_else(|| format!("unexpected `{c}`"))?));
                i += 1;
            }
        }
    }
    Ok(out)
}

// ---- parser (Pratt) --------------------------------------------------------

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn eat(&mut self, s: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Sym(x)) if *x == s) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self, depth: usize) -> Result<Node, String> {
        if depth > MAX_DEPTH {
            return Err("expression too deeply nested".into());
        }
        let cond = self.bin(0, depth + 1)?;
        if self.eat("?") {
            let a = self.expr(depth + 1)?;
            if !self.eat(":") {
                return Err("expected `:` in ternary".into());
            }
            let b = self.expr(depth + 1)?;
            return Ok(Node::Tern(cond.into(), a.into(), b.into()));
        }
        Ok(cond)
    }

    fn binop(&self) -> Option<(Op, u8)> {
        let Some(Tok::Sym(s)) = self.peek() else { return None };
        Some(match *s {
            "||" => (Op::Or, 1),
            "&&" => (Op::And, 2),
            "==" => (Op::Eq, 3),
            "!=" => (Op::Ne, 3),
            "<" => (Op::Lt, 4),
            "<=" => (Op::Le, 4),
            ">" => (Op::Gt, 4),
            ">=" => (Op::Ge, 4),
            "+" => (Op::Add, 5),
            "-" => (Op::Sub, 5),
            "*" => (Op::Mul, 6),
            "/" => (Op::Div, 6),
            "%" => (Op::Rem, 6),
            _ => return None,
        })
    }

    fn bin(&mut self, min: u8, depth: usize) -> Result<Node, String> {
        if depth > MAX_DEPTH {
            return Err("expression too deeply nested".into());
        }
        let mut lhs = self.unary(depth + 1)?;
        while let Some((op, prec)) = self.binop() {
            if prec < min {
                break;
            }
            self.pos += 1;
            let rhs = self.bin(prec + 1, depth + 1)?;
            lhs = Node::Bin(op, lhs.into(), rhs.into());
        }
        Ok(lhs)
    }

    fn unary(&mut self, depth: usize) -> Result<Node, String> {
        if depth > MAX_DEPTH {
            return Err("expression too deeply nested".into());
        }
        if self.eat("!") {
            return Ok(Node::Not(self.unary(depth + 1)?.into()));
        }
        if self.eat("-") {
            return Ok(Node::Neg(self.unary(depth + 1)?.into()));
        }
        self.atom(depth + 1)
    }

    fn atom(&mut self, depth: usize) -> Result<Node, String> {
        let t = self.toks.get(self.pos).cloned().ok_or("unexpected end of expression")?;
        self.pos += 1;
        match t {
            Tok::Num(n) => Ok(Node::Lit(Value::Num(n))),
            Tok::Str(s) => Ok(Node::Lit(Value::Str(s))),
            Tok::Ident(id) => match id.as_str() {
                "true" => Ok(Node::Lit(Value::Bool(true))),
                "false" => Ok(Node::Lit(Value::Bool(false))),
                "nil" => Ok(Node::Lit(Value::Nil)),
                _ if self.eat("(") => {
                    let mut args = Vec::new();
                    if !self.eat(")") {
                        loop {
                            args.push(self.expr(depth + 1)?);
                            if self.eat(")") {
                                break;
                            }
                            if !self.eat(",") {
                                return Err("expected `,` or `)`".into());
                            }
                        }
                    }
                    Ok(Node::Call(id, args))
                }
                _ => {
                    let mut path = vec![id];
                    while self.eat(".") {
                        match self.toks.get(self.pos).cloned() {
                            Some(Tok::Ident(s)) => path.push(s),
                            Some(Tok::Num(n)) => path.push((n as i64).to_string()),
                            _ => return Err("expected name after `.`".into()),
                        }
                        self.pos += 1;
                    }
                    Ok(Node::Path(path))
                }
            },
            Tok::Sym("(") => {
                let e = self.expr(depth + 1)?;
                if !self.eat(")") {
                    return Err("expected `)`".into());
                }
                Ok(e)
            }
            Tok::Sym(s) => Err(format!("unexpected `{s}`")),
        }
    }
}

impl Expr {
    pub fn parse(src: &str) -> Result<Expr, String> {
        if src.len() > MAX_LEN {
            return Err("expression too long".into());
        }
        let mut p = Parser { toks: lex(src)?, pos: 0 };
        if p.toks.is_empty() {
            return Err("empty expression".into());
        }
        let n = p.expr(0)?;
        if p.pos != p.toks.len() {
            return Err(format!("unexpected trailing `{:?}`", p.toks[p.pos]));
        }
        Ok(Expr(n))
    }

    pub fn eval(&self, scope: &Scope) -> Result<Value, String> {
        eval(&self.0, scope)
    }
}

fn num(v: &Value, what: &str) -> Result<f64, String> {
    v.as_f64().ok_or_else(|| format!("`{what}` needs a number, got `{v}`"))
}

// Not code execution: this walks our own parsed `Node` tree over `Value`s. The
// grammar has no loops, assignments, imports or host calls (see `call`'s fixed
// whitelist), so it cannot run anything the user typed, only compute a value.
fn eval(n: &Node, sc: &Scope) -> Result<Value, String> {
    Ok(match n {
        Node::Lit(v) => v.clone(),
        Node::Path(p) => sc.lookup(p)?,
        Node::Not(a) => Value::Bool(!eval(a, sc)?.truthy()),
        Node::Neg(a) => Value::Num(-num(&eval(a, sc)?, "-")?),
        Node::Tern(c, a, b) => {
            if eval(c, sc)?.truthy() {
                eval(a, sc)?
            } else {
                eval(b, sc)?
            }
        }
        Node::Bin(Op::And, a, b) => {
            let l = eval(a, sc)?;
            if l.truthy() { eval(b, sc)? } else { l }
        }
        Node::Bin(Op::Or, a, b) => {
            let l = eval(a, sc)?;
            if l.truthy() { l } else { eval(b, sc)? }
        }
        Node::Bin(op, a, b) => {
            let (l, r) = (eval(a, sc)?, eval(b, sc)?);
            match op {
                Op::Add if matches!(l, Value::Str(_)) || matches!(r, Value::Str(_)) => {
                    Value::Str(format!("{l}{r}"))
                }
                Op::Add => Value::Num(num(&l, "+")? + num(&r, "+")?),
                Op::Sub => Value::Num(num(&l, "-")? - num(&r, "-")?),
                Op::Mul => Value::Num(num(&l, "*")? * num(&r, "*")?),
                Op::Div => {
                    let d = num(&r, "/")?;
                    if d == 0.0 {
                        return Err("division by zero".into());
                    }
                    Value::Num(num(&l, "/")? / d)
                }
                Op::Rem => {
                    let d = num(&r, "%")?;
                    if d == 0.0 {
                        return Err("modulo by zero".into());
                    }
                    Value::Num(num(&l, "%")?.rem_euclid(d))
                }
                Op::Eq => Value::Bool(l == r),
                Op::Ne => Value::Bool(l != r),
                Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                    let ord = match (&l, &r) {
                        (Value::Str(x), Value::Str(y)) => x.partial_cmp(y),
                        _ => num(&l, "compare")?.partial_cmp(&num(&r, "compare")?),
                    }
                    .ok_or("cannot compare")?;
                    Value::Bool(match op {
                        Op::Lt => ord.is_lt(),
                        Op::Le => ord.is_le(),
                        Op::Gt => ord.is_gt(),
                        _ => ord.is_ge(),
                    })
                }
                Op::And | Op::Or => unreachable!(),
            }
        }
        Node::Call(name, args) => {
            let a: Vec<Value> = args.iter().map(|x| eval(x, sc)).collect::<Result<_, _>>()?;
            call(name, &a)?
        }
    })
}

fn call(name: &str, a: &[Value]) -> Result<Value, String> {
    let n = |i: usize| -> Result<f64, String> {
        num(a.get(i).ok_or_else(|| format!("`{name}` is missing argument {}", i + 1))?, name)
    };
    Ok(match name {
        "min" | "max" if !a.is_empty() => {
            let mut acc = n(0)?;
            for i in 1..a.len() {
                acc = if name == "min" { acc.min(n(i)?) } else { acc.max(n(i)?) };
            }
            Value::Num(acc)
        }
        "abs" => Value::Num(n(0)?.abs()),
        "pad" => {
            let w = n(1)?.clamp(0.0, 32.0) as usize;
            Value::Str(format!("{:0>w$}", Value::Num(n(0)?.round()).to_string()))
        }
        "round" => Value::Num(n(0)?.round()),
        "floor" => Value::Num(n(0)?.floor()),
        "ceil" => Value::Num(n(0)?.ceil()),
        "clamp" => Value::Num(n(0)?.clamp(n(1)?, n(2)?.max(n(1)?))),
        "len" => Value::Num(match a.first() {
            Some(Value::Str(s)) => s.chars().count(),
            Some(Value::List(l)) => l.len(),
            _ => 0,
        } as f64),
        "upper" => Value::Str(a.first().map(|v| v.to_string()).unwrap_or_default().to_uppercase()),
        "lower" => Value::Str(a.first().map(|v| v.to_string()).unwrap_or_default().to_lowercase()),
        _ => return Err(format!("unknown function `{name}`")),
    })
}

// ---- templates: "{clock.hour|02}:{clock.minute|02}" ------------------------

#[derive(Clone, Copy, Debug, Default)]
struct Fmt {
    zero: bool,
    width: usize,
    prec: Option<usize>,
}

impl Fmt {
    fn parse(s: &str) -> Result<Fmt, String> {
        let s = s.trim();
        let (int, prec) = match s.split_once('.') {
            Some((i, p)) => (i, Some(p.parse().map_err(|_| format!("bad format `{s}`"))?)),
            None => (s, None),
        };
        let zero = int.starts_with('0') && int.len() > 1;
        let width = if int.is_empty() { 0 } else { int.parse().map_err(|_| format!("bad format `{s}`"))? };
        Ok(Fmt { zero, width, prec })
    }

    fn apply(&self, v: &Value) -> Result<String, String> {
        let x = num(v, "format")?;
        let body = match self.prec {
            Some(p) => format!("{x:.p$}"),
            None => Value::Num(x.round()).to_string(),
        };
        Ok(if self.zero { format!("{body:0>w$}", w = self.width) } else { format!("{body:>w$}", w = self.width) })
    }
}

#[derive(Clone, Debug)]
enum Part {
    Lit(String),
    Ex(Expr, Option<Fmt>),
}

#[derive(Clone, Debug)]
pub struct Template(Vec<Part>);

/// Index of a top-level single `|` (not `||`, not inside quotes or parens).
fn split_fmt(s: &str) -> Option<usize> {
    let (mut depth, mut quote) = (0i32, None);
    let b: Vec<char> = s.chars().collect();
    let mut byte = 0;
    for (i, &c) in b.iter().enumerate() {
        match (c, quote) {
            (q @ ('\'' | '"'), None) => quote = Some(q),
            (q, Some(open)) if q == open => quote = None,
            ('(', None) => depth += 1,
            (')', None) => depth -= 1,
            ('|', None) if depth == 0 => {
                let dbl = b.get(i + 1) == Some(&'|') || (i > 0 && b[i - 1] == '|');
                if !dbl {
                    return Some(byte);
                }
            }
            _ => {}
        }
        byte += c.len_utf8();
    }
    None
}

impl Template {
    pub fn parse(src: &str) -> Result<Template, String> {
        let mut parts = Vec::new();
        let mut lit = String::new();
        let mut it = src.chars().peekable();
        while let Some(c) = it.next() {
            match c {
                '{' if it.peek() == Some(&'{') => {
                    it.next();
                    lit.push('{');
                }
                '}' if it.peek() == Some(&'}') => {
                    it.next();
                    lit.push('}');
                }
                '{' => {
                    if !lit.is_empty() {
                        parts.push(Part::Lit(std::mem::take(&mut lit)));
                    }
                    let mut body = String::new();
                    loop {
                        match it.next() {
                            Some('}') => break,
                            Some(ch) => body.push(ch),
                            None => return Err("unclosed `{`".into()),
                        }
                    }
                    let (e, f) = match split_fmt(&body) {
                        Some(i) => (&body[..i], Some(Fmt::parse(&body[i + 1..])?)),
                        None => (body.as_str(), None),
                    };
                    parts.push(Part::Ex(Expr::parse(e)?, f));
                }
                '}' => return Err("stray `}`".into()),
                c => lit.push(c),
            }
        }
        if !lit.is_empty() {
            parts.push(Part::Lit(lit));
        }
        Ok(Template(parts))
    }

    /// True when there is no `{}` at all: a plain literal.
    pub fn is_literal(&self) -> bool {
        self.0.iter().all(|p| matches!(p, Part::Lit(_)))
    }

    /// A lone `{expr}` keeps its type (number, bool, list); anything else is text.
    pub fn eval(&self, sc: &Scope) -> Result<Value, String> {
        if let [Part::Ex(e, None)] = self.0.as_slice() {
            return e.eval(sc);
        }
        let mut out = String::new();
        for p in &self.0 {
            match p {
                Part::Lit(s) => out.push_str(s),
                Part::Ex(e, None) => out.push_str(&e.eval(sc)?.to_string()),
                Part::Ex(e, Some(f)) => out.push_str(&f.apply(&e.eval(sc)?)?),
            }
        }
        Ok(Value::Str(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(src: &str) -> Result<Value, String> {
        let mut sc = Scope::new();
        sc.set("clock", Value::obj([("hour", 7.into()), ("minute", 5.into()), ("pm", true.into())]));
        sc.set("param", Value::obj([("title", "".into())]));
        Template::parse(&format!("{{{src}}}")).and_then(|t| t.eval(&sc))
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(ev("1 + 2 * 3").unwrap(), Value::Num(7.0));
        assert_eq!(ev("(1 + 2) * 3").unwrap(), Value::Num(9.0));
        assert_eq!(ev("10 - 4 - 3").unwrap(), Value::Num(3.0)); // left associative
        assert_eq!(ev("-2 + 5 % 3").unwrap(), Value::Num(0.0));
        assert_eq!(ev("clock.hour * 30 + clock.minute * 0.5").unwrap(), Value::Num(212.5));
    }

    #[test]
    fn ternary_logic_and_strings() {
        assert_eq!(ev("clock.pm ? 'PM' : 'AM'").unwrap(), Value::Str("PM".into()));
        assert_eq!(ev("clock.hour > 6 && clock.hour < 8").unwrap(), Value::Bool(true));
        assert_eq!(ev("param.title || 'Untitled'").unwrap(), Value::Str("Untitled".into()));
        assert_eq!(ev("'h' + clock.hour").unwrap(), Value::Str("h7".into()));
        assert_eq!(ev("max(1, 9, 3) + clamp(15, 0, 10)").unwrap(), Value::Num(19.0));
        assert_eq!(ev("pad(clock.minute, 2) + ':' + pad(clock.hour, 3)").unwrap(), Value::Str("05:007".into()));
    }

    #[test]
    fn interpolation_and_format_specs() {
        let mut sc = Scope::new();
        sc.set("clock", Value::obj([("hour", 7.into()), ("minute", 5.into()), ("x", 1.23456.into())]));
        let t = Template::parse("{clock.hour|02}:{clock.minute|02} {clock.x|.2} {{ok}}").unwrap();
        assert_eq!(t.eval(&sc).unwrap(), Value::Str("07:05 1.23 {ok}".into()));
        // `||` inside an interpolation is logical-or, not a format separator
        assert_eq!(Template::parse("{0 || 4}").unwrap().eval(&sc).unwrap(), Value::Num(4.0));
        assert!(Template::parse("plain").unwrap().is_literal());
    }

    #[test]
    fn provided_names_are_asked_for_once_and_only_when_read() {
        let asked = RefCell::new(Vec::new());
        let provider = |n: &str| {
            asked.borrow_mut().push(n.to_string());
            (n == "clock").then(|| Value::obj([("minute", 5.into())]))
        };
        let mut sc = Scope::with_provider(&provider);
        sc.set("param", Value::obj([("x", 1.into())]));
        let t = Template::parse("{clock.minute + clock.minute + param.x}").unwrap();
        assert_eq!(t.eval(&sc).unwrap(), Value::Num(11.0));
        assert_eq!(*asked.borrow(), ["clock"], "one ask per name, none for names set on the scope");
        assert!(sc.deps().contains("clock.minute"), "provided reads are dependencies too");
        assert!(Template::parse("{sys.cpu}").unwrap().eval(&sc).is_err(), "a name nobody provides is still undefined");
    }

    #[test]
    fn malformed_input_errors_instead_of_panicking() {
        for bad in ["1 +", "(1", "1 ? 2", "@", "'open", "clock.", "", "foo(", "1 2"] {
            assert!(Expr::parse(bad).is_err(), "should reject {bad:?}");
        }
        assert!(Template::parse("{unclosed").is_err());
        assert!(Template::parse("stray }").is_err());
        assert!(ev("1 / 0").is_err());
        assert!(ev("nope.x").is_err());
        assert!(ev("undefined_fn(1)").is_err());
    }

    #[test]
    fn totality_bounds_hold() {
        // A pathological nesting depth must be rejected, not stack-overflow.
        let deep = format!("{}1{}", "(".repeat(5000), ")".repeat(5000));
        assert!(Expr::parse(&deep).is_err());
        let chain = "!".repeat(5000) + "1";
        assert!(Expr::parse(&chain).is_err());
        assert!(Expr::parse(&"1+".repeat(2000)).is_err()); // over MAX_LEN
    }

    #[test]
    fn deps_record_only_the_branch_taken() {
        let mut sc = Scope::new();
        sc.set("clock", Value::obj([("minute", 1.into()), ("second", 2.into())]));
        sc.set("param", Value::obj([("secs", false.into())]));
        let t = Template::parse("{param.secs ? clock.second : clock.minute}").unwrap();
        t.eval(&sc).unwrap();
        let d = sc.deps();
        assert!(d.contains("clock.minute") && !d.contains("clock.second"), "{d:?}");
    }
}
