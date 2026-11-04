//! A small regular-expression matcher for the `re:` search form (no regex crate is allowed).
//!
//! Supported: literals, `.`, `^`, `$`, `[...]` classes (ranges, `^` negation, `\d \w \s`),
//! `\d \D \w \W \s \S` and escaped punctuation, `( )` and `(?: )` groups, `|`, and the
//! quantifiers `* + ? {n} {n,} {n,m}` (a trailing `?` is accepted and ignored: a yes/no match
//! does not care about greediness). Matching is case-insensitive and finds the pattern anywhere
//! in the name, like Python's `re.search` with IGNORECASE. Anything else makes the pattern
//! invalid, and an invalid pattern matches nothing. A step limit stops runaway backtracking.

const STEP_LIMIT: usize = 400_000;
const PROGRAM_LIMIT: usize = 20_000;
const REPEAT_CAP: usize = 100;
const DEPTH_LIMIT: usize = 40;

type Ranges = Vec<(char, char)>;

enum Node {
    Char(char),
    Any,
    Class(Ranges, bool),
    Start,
    End,
    Cat(Vec<Node>),
    Alt(Vec<Node>),
    Rep(Box<Node>, usize, Option<usize>),
}

enum Inst {
    Char(char),
    Any,
    Class(Ranges, bool),
    Start,
    End,
    Split(usize, usize),
    Jmp(usize),
    Match,
}

/// A compiled pattern.
pub struct Regex {
    prog: Vec<Inst>,
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn upper(c: char) -> char {
    c.to_uppercase().next().unwrap_or(c)
}

fn digit_ranges() -> Ranges {
    vec![('0', '9')]
}

fn word_ranges() -> Ranges {
    vec![('a', 'z'), ('A', 'Z'), ('0', '9'), ('_', '_')]
}

fn space_ranges() -> Ranges {
    vec![(' ', ' '), ('\t', '\r')]
}

fn ranges_have(r: &Ranges, c: char) -> bool {
    let mut i = 0;
    while i < r.len() {
        if c >= r[i].0 && c <= r[i].1 {
            return true;
        }
        i += 1;
    }
    false
}

fn class_has(r: &Ranges, neg: bool, c: char) -> bool {
    let hit = ranges_have(r, c) || ranges_have(r, upper(c));
    hit != neg
}

struct Parser {
    p: Vec<char>,
    pos: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.p.get(self.pos).copied()
    }

    fn alt(&mut self) -> Result<Node, ()> {
        let mut alts = vec![self.cat()?];
        while self.peek() == Some('|') {
            self.pos += 1;
            alts.push(self.cat()?);
        }
        if alts.len() == 1 {
            Ok(alts.remove(0))
        } else {
            Ok(Node::Alt(alts))
        }
    }

    fn cat(&mut self) -> Result<Node, ()> {
        let mut items = Vec::new();
        loop {
            match self.peek() {
                None | Some('|') | Some(')') => break,
                _ => {}
            }
            let atom = self.atom()?;
            let atom = self.quant(atom)?;
            items.push(atom);
        }
        Ok(Node::Cat(items))
    }

    fn atom(&mut self) -> Result<Node, ()> {
        let c = self.peek().ok_or(())?;
        self.pos += 1;
        match c {
            '(' => {
                self.depth += 1;
                if self.depth > DEPTH_LIMIT {
                    return Err(());
                }
                if self.peek() == Some('?') {
                    if self.p.get(self.pos + 1) == Some(&':') {
                        self.pos += 2;
                    } else {
                        return Err(());
                    }
                }
                let inner = self.alt()?;
                if self.peek() != Some(')') {
                    return Err(());
                }
                self.pos += 1;
                self.depth -= 1;
                Ok(inner)
            }
            '[' => self.class(),
            '.' => Ok(Node::Any),
            '^' => Ok(Node::Start),
            '$' => Ok(Node::End),
            '\\' => self.escape(),
            '*' | '+' | '?' => Err(()),
            _ => Ok(Node::Char(lower(c))),
        }
    }

    fn escape(&mut self) -> Result<Node, ()> {
        let e = self.peek().ok_or(())?;
        self.pos += 1;
        match e {
            'd' => Ok(Node::Class(digit_ranges(), false)),
            'D' => Ok(Node::Class(digit_ranges(), true)),
            'w' => Ok(Node::Class(word_ranges(), false)),
            'W' => Ok(Node::Class(word_ranges(), true)),
            's' => Ok(Node::Class(space_ranges(), false)),
            'S' => Ok(Node::Class(space_ranges(), true)),
            'n' => Ok(Node::Char('\n')),
            't' => Ok(Node::Char('\t')),
            'r' => Ok(Node::Char('\r')),
            _ => {
                if e.is_alphanumeric() {
                    Err(()) // \b, \1, \A ... are not supported
                } else {
                    Ok(Node::Char(lower(e)))
                }
            }
        }
    }

    fn class(&mut self) -> Result<Node, ()> {
        let mut neg = false;
        if self.peek() == Some('^') {
            neg = true;
            self.pos += 1;
        }
        let mut ranges: Ranges = Vec::new();
        let mut first = true;
        loop {
            let c = self.peek().ok_or(())?;
            self.pos += 1;
            if c == ']' && !first {
                break;
            }
            first = false;
            let lo = if c == '\\' {
                let e = self.peek().ok_or(())?;
                self.pos += 1;
                match e {
                    'd' => {
                        ranges.extend(digit_ranges());
                        continue;
                    }
                    'w' => {
                        ranges.extend(word_ranges());
                        continue;
                    }
                    's' => {
                        ranges.extend(space_ranges());
                        continue;
                    }
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    _ => {
                        if e.is_alphanumeric() {
                            return Err(());
                        }
                        e
                    }
                }
            } else {
                c
            };
            let is_range = self.peek() == Some('-')
                && match self.p.get(self.pos + 1) {
                    Some(&n) => n != ']',
                    None => false,
                };
            if is_range {
                self.pos += 1;
                let mut hi = self.peek().ok_or(())?;
                self.pos += 1;
                if hi == '\\' {
                    hi = self.peek().ok_or(())?;
                    self.pos += 1;
                    if hi.is_alphanumeric() {
                        return Err(());
                    }
                }
                if hi < lo {
                    return Err(());
                }
                ranges.push((lo, hi));
            } else {
                ranges.push((lo, lo));
            }
        }
        Ok(Node::Class(ranges, neg))
    }

    fn quant(&mut self, atom: Node) -> Result<Node, ()> {
        let (min, max) = match self.peek() {
            Some('*') => {
                self.pos += 1;
                (0, None)
            }
            Some('+') => {
                self.pos += 1;
                (1, None)
            }
            Some('?') => {
                self.pos += 1;
                (0, Some(1))
            }
            Some('{') => match self.braces() {
                Some(mm) => mm,
                None => return Ok(atom), // not a quantifier: the `{` is an ordinary character
            },
            _ => return Ok(atom),
        };
        if self.peek() == Some('?') {
            self.pos += 1;
        }
        Ok(Node::Rep(Box::new(atom), min, max))
    }

    /// `{n}`, `{n,}` or `{n,m}`; leaves the position alone and returns None when it isn't one.
    fn braces(&mut self) -> Option<(usize, Option<usize>)> {
        let save = self.pos;
        self.pos += 1; // the `{`
        let a = self.number();
        let a = match a {
            Some(v) => v,
            None => {
                self.pos = save;
                return None;
            }
        };
        let mut max = Some(a);
        if self.peek() == Some(',') {
            self.pos += 1;
            max = self.number();
        }
        if self.peek() != Some('}') {
            self.pos = save;
            return None;
        }
        self.pos += 1;
        if let Some(m) = max {
            if m < a {
                self.pos = save;
                return None;
            }
        }
        Some((a, max))
    }

    fn number(&mut self) -> Option<usize> {
        let start = self.pos;
        let mut v: usize = 0;
        while let Some(c) = self.peek() {
            match c.to_digit(10) {
                Some(d) => {
                    v = (v * 10 + d as usize).min(REPEAT_CAP);
                    self.pos += 1;
                }
                None => break,
            }
        }
        if self.pos == start { None } else { Some(v) }
    }
}

fn emit(node: &Node, prog: &mut Vec<Inst>) -> Result<(), ()> {
    if prog.len() > PROGRAM_LIMIT {
        return Err(());
    }
    match node {
        Node::Char(c) => prog.push(Inst::Char(*c)),
        Node::Any => prog.push(Inst::Any),
        Node::Class(r, neg) => prog.push(Inst::Class(r.clone(), *neg)),
        Node::Start => prog.push(Inst::Start),
        Node::End => prog.push(Inst::End),
        Node::Cat(items) => {
            for it in items {
                emit(it, prog)?;
            }
        }
        Node::Alt(alts) => {
            let mut jumps: Vec<usize> = Vec::new();
            let last = alts.len() - 1;
            for (i, a) in alts.iter().enumerate() {
                if i < last {
                    let split_at = prog.len();
                    prog.push(Inst::Split(0, 0));
                    emit(a, prog)?;
                    jumps.push(prog.len());
                    prog.push(Inst::Jmp(0));
                    let next = prog.len();
                    prog[split_at] = Inst::Split(split_at + 1, next);
                } else {
                    emit(a, prog)?;
                }
            }
            let end = prog.len();
            for j in jumps {
                prog[j] = Inst::Jmp(end);
            }
        }
        Node::Rep(inner, min, max) => {
            let mut n = 0;
            while n < *min {
                emit(inner, prog)?;
                n += 1;
            }
            match max {
                None => {
                    let l = prog.len();
                    prog.push(Inst::Split(0, 0));
                    emit(inner, prog)?;
                    prog.push(Inst::Jmp(l));
                    let end = prog.len();
                    prog[l] = Inst::Split(l + 1, end);
                }
                Some(m) => {
                    let mut splits: Vec<usize> = Vec::new();
                    let mut k = *min;
                    while k < *m {
                        splits.push(prog.len());
                        prog.push(Inst::Split(0, 0));
                        emit(inner, prog)?;
                        k += 1;
                    }
                    let end = prog.len();
                    for s in splits {
                        prog[s] = Inst::Split(s + 1, end);
                    }
                }
            }
        }
    }
    Ok(())
}

impl Regex {
    /// Compile `pattern`, or None when it is not valid (or not supported).
    pub fn new(pattern: &str) -> Option<Regex> {
        let mut parser = Parser {
            p: pattern.chars().collect(),
            pos: 0,
            depth: 0,
        };
        let node = parser.alt().ok()?;
        if parser.pos != parser.p.len() {
            return None; // a stray `)`
        }
        let mut prog: Vec<Inst> = Vec::new();
        emit(&node, &mut prog).ok()?;
        prog.push(Inst::Match);
        Some(Regex { prog })
    }

    /// Does the pattern match anywhere in `text`? Pass the name already lowercased.
    pub fn is_match(&self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        let mut steps: usize = 0;
        let mut start = 0;
        while start <= chars.len() {
            if self.run(&chars, start, &mut steps) {
                return true;
            }
            if steps > STEP_LIMIT {
                return false;
            }
            start += 1;
        }
        false
    }

    fn run(&self, s: &[char], start: usize, steps: &mut usize) -> bool {
        let mut stack: Vec<(usize, usize)> = vec![(0, start)];
        while let Some((mut pc, mut i)) = stack.pop() {
            loop {
                *steps += 1;
                if *steps > STEP_LIMIT {
                    return false;
                }
                match &self.prog[pc] {
                    Inst::Match => return true,
                    Inst::Char(c) => {
                        if i < s.len() && s[i] == *c {
                            pc += 1;
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    Inst::Any => {
                        if i < s.len() {
                            pc += 1;
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    Inst::Class(r, neg) => {
                        if i < s.len() && class_has(r, *neg, s[i]) {
                            pc += 1;
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    Inst::Start => {
                        if i == 0 {
                            pc += 1;
                        } else {
                            break;
                        }
                    }
                    Inst::End => {
                        if i == s.len() {
                            pc += 1;
                        } else {
                            break;
                        }
                    }
                    Inst::Split(a, b) => {
                        stack.push((*b, i));
                        pc = *a;
                    }
                    Inst::Jmp(a) => {
                        pc = *a;
                    }
                }
            }
        }
        false
    }
}
