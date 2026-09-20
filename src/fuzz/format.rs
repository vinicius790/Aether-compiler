//! Format-based fuzzing: the grammar *is* the input format.
//!
//! A byte stream is read as a sequence of production choices (Nautilus /
//! Grammarinator style, without ANTLR). Every non-terminal consumes one
//! byte and picks a rule. The result is always a token-ish Aether file:
//! braces and keywords stay matched enough that the parser is exercised
//! past the first token, unlike raw havoc.

use super::rng::FuzzRng;

pub fn gen_format(rng: &mut FuzzRng) -> String {
    let n = rng.int(8, 40) as usize;
    let mut bytes = Vec::with_capacity(n);
    for _ in 0..n {
        bytes.push(rng.next_u64() as u8);
    }
    decode(&bytes)
}

/// Interpret `bytes` as a packed choice tape over the Aether EBNF.
pub fn decode(bytes: &[u8]) -> String {
    let mut d = Decoder { bytes, i: 0, depth: 0 };
    d.program()
}

struct Decoder<'a> {
    bytes: &'a [u8],
    i: usize,
    depth: u32,
}

impl<'a> Decoder<'a> {
    fn take(&mut self) -> u8 {
        if self.i >= self.bytes.len() {
            return 0;
        }
        let b = self.bytes[self.i];
        self.i += 1;
        b
    }

    fn pick(&mut self, n: u8) -> u8 {
        if n == 0 {
            return 0;
        }
        self.take() % n
    }

    fn program(&mut self) -> String {
        let n = 1 + self.pick(3) as usize;
        let mut s = String::new();
        for k in 0..n {
            if k + 1 == n {
                s.push_str(&self.fn_item(true));
            } else {
                s.push_str(&self.fn_item(false));
            }
            s.push('\n');
        }
        s
    }

    fn fn_item(&mut self, main: bool) -> String {
        let name = if main { "main".into() } else { format!("f{}", self.pick(4)) };
        let ret = *["i32", "bool", "unit"]
            .get(self.pick(3) as usize)
            .unwrap_or(&"i32");
        let body = self.block();
        if main {
            format!("fn {name}() -> i32 {body}")
        } else {
            format!("fn {name}(p0: i32) -> {ret} {body}")
        }
    }

    fn block(&mut self) -> String {
        self.depth += 1;
        let n = if self.depth > 3 { 1 } else { 1 + self.pick(3) as usize };
        let mut s = String::from("{\n");
        for _ in 0..n {
            s.push_str("    ");
            s.push_str(&self.stmt());
            s.push('\n');
        }
        s.push_str("    return ");
        s.push_str(&self.expr());
        s.push_str(";\n}");
        self.depth -= 1;
        s
    }

    fn stmt(&mut self) -> String {
        match self.pick(6) {
            0 => format!("let mut v{} = {};", self.pick(6), self.expr()),
            1 => format!("v{} = {};", self.pick(6), self.expr()),
            2 if self.depth < 3 => format!("if {} {} else {}", self.expr(), self.block(), self.block()),
            3 => format!("print_i32({});", self.expr()),
            4 => format!("return {};", self.expr()),
            _ => format!("{};", self.expr()),
        }
    }

    fn expr(&mut self) -> String {
        if self.depth > 4 {
            return self.leaf();
        }
        self.depth += 1;
        let s = match self.pick(7) {
            0 => self.leaf(),
            1 => format!("({} + {})", self.expr(), self.expr()),
            2 => format!("({} - {})", self.expr(), self.expr()),
            3 => format!("({} * {})", self.expr(), self.leaf()),
            4 => format!("({} == {})", self.expr(), self.expr()),
            5 => format!("({} < {})", self.expr(), self.expr()),
            _ => format!("(-{})", self.leaf()),
        };
        self.depth -= 1;
        s
    }

    fn leaf(&mut self) -> String {
        match self.pick(5) {
            0 => format!("{}", self.pick(12)),
            1 => "true".into(),
            2 => "false".into(),
            3 => format!("v{}", self.pick(6)),
            _ => format!("p0"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuzz::rng::FuzzRng;

    #[test]
    fn decode_is_deterministic() {
        let a = decode(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let b = decode(&[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(a, b);
        assert!(a.contains("fn "));
    }

    #[test]
    fn gen_format_emits_source() {
        let mut rng = FuzzRng::new(9);
        let s = gen_format(&mut rng);
        assert!(s.contains("fn "));
    }
}
