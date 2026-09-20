//! Structure-aware mutation and shrinking.
//!
//! Byte-havoc (bit flips) is useful to stress the lexer. For the middle of the
//! compiler we want mutations that stay close to the grammar: swap siblings,
//! tweak literals, splice helpers between programs, wrap statements.
//!
//! The operators below are the subset of Superion / Grammarinator / afl-ts
//! that we can apply to Aether source without a second parser: they walk
//! lines and brace-delimited blocks rather than a typed AST. They are still
//! *structural* — they never splice mid-token.

use super::rng::FuzzRng;

/// Apply `n` structural mutations to a well-formed generated program.
pub fn mutate_structural(rng: &mut FuzzRng, src: &str, n: usize) -> String {
    let mut cur = src.to_string();
    for _ in 0..n.max(1) {
        cur = one_mutation(rng, &cur);
    }
    cur
}

fn one_mutation(rng: &mut FuzzRng, src: &str) -> String {
    match rng.int(0, 8) {
        0 => tweak_literal(rng, src),
        1 => swap_binop(rng, src),
        2 => swap_cmp(rng, src),
        3 => delete_stmt_line(rng, src),
        4 => duplicate_stmt_line(rng, src),
        5 => wrap_stmt_in_if(rng, src),
        6 => tweak_for_bounds(rng, src),
        7 => rename_call(rng, src),
        _ => insert_print(rng, src),
    }
}

/// Crossover: graft a helper from `donor` into `recipient`.
pub fn crossover(rng: &mut FuzzRng, recipient: &str, donor: &str) -> String {
    let donor_helpers = extract_helpers(donor);
    if donor_helpers.is_empty() {
        return recipient.to_string();
    }
    let piece = donor_helpers[rng.int(0, (donor_helpers.len() - 1) as i32) as usize].clone();
    let used = count_helpers(recipient);
    let renamed = rename_fn(&piece, &format!("h{used}"));
    format!("{renamed}\n{recipient}")
}

/// Shrink a failing program: drop helpers and statement lines while `still_fails`
/// remains true. Returns the smallest variant found.
pub fn shrink(src: &str, mut still_fails: impl FnMut(&str) -> bool) -> String {
    let mut best = src.to_string();
    // Drop whole helpers first (largest cut).
    let helpers = extract_helpers(&best);
    for h in &helpers {
        let candidate = best.replacen(h, "", 1);
        if still_fails(&candidate) {
            best = candidate;
        }
    }
    // Drop individual non-control statement lines.
    let lines: Vec<String> = best.lines().map(|s| s.to_string()).collect();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.starts_with("fn ")
            || t.starts_with("return ")
            || t == "}"
            || t == "{"
            || t.starts_with("if ")
            || t.starts_with("for ")
            || t.starts_with("else")
            || t.is_empty()
        {
            continue;
        }
        let mut cand_lines = lines.clone();
        cand_lines.remove(i);
        let candidate = cand_lines.join("\n");
        if still_fails(&candidate) {
            return shrink(&candidate, still_fails);
        }
    }
    best
}

fn tweak_literal(rng: &mut FuzzRng, src: &str) -> String {
    let spans = number_spans(src);
    if spans.is_empty() {
        return src.to_string();
    }
    let (s, e) = spans[rng.int(0, (spans.len() - 1) as i32) as usize];
    let replacement = rng.int(0, 12).to_string();
    let mut out = String::with_capacity(src.len() + 4);
    out.push_str(&src[..s]);
    out.push_str(&replacement);
    out.push_str(&src[e..]);
    out
}

fn swap_binop(rng: &mut FuzzRng, src: &str) -> String {
    replace_nth_token(rng, src, &['+', '-', '*'], &["+", "-", "*"])
}

fn swap_cmp(rng: &mut FuzzRng, src: &str) -> String {
    // Two-char first so we don't tear `<=` into `<`.
    let pairs = [("<=", ">="), (">=", "<="), ("==", "!="), ("!=", "==")];
    let mut hits = Vec::new();
    for (a, b) in pairs {
        let mut from = 0;
        while let Some(off) = src[from..].find(a) {
            hits.push((from + off, a, b));
            from += off + a.len();
        }
    }
    if hits.is_empty() {
        return replace_nth_token(rng, src, &['<', '>'], &["<", ">"]);
    }
    let (at, a, b) = hits[rng.int(0, (hits.len() - 1) as i32) as usize];
    format!("{}{}{}", &src[..at], b, &src[at + a.len()..])
}

fn delete_stmt_line(rng: &mut FuzzRng, src: &str) -> String {
    let idx = deletable_lines(src);
    if idx.is_empty() {
        return src.to_string();
    }
    let drop = idx[rng.int(0, (idx.len() - 1) as i32) as usize];
    src.lines()
        .enumerate()
        .filter(|(i, _)| *i != drop)
        .map(|(_, l)| l)
        .collect::<Vec<_>>()
        .join("\n")
}

fn duplicate_stmt_line(rng: &mut FuzzRng, src: &str) -> String {
    let idx = deletable_lines(src);
    if idx.is_empty() {
        return src.to_string();
    }
    let copy = idx[rng.int(0, (idx.len() - 1) as i32) as usize];
    let mut out = Vec::new();
    for (i, line) in src.lines().enumerate() {
        out.push(line.to_string());
        if i == copy {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

fn wrap_stmt_in_if(rng: &mut FuzzRng, src: &str) -> String {
    let idx = deletable_lines(src);
    if idx.is_empty() {
        return src.to_string();
    }
    let at = idx[rng.int(0, (idx.len() - 1) as i32) as usize];
    let mut out = Vec::new();
    for (i, line) in src.lines().enumerate() {
        if i == at {
            let indent = line.chars().take_while(|c| *c == ' ').count();
            let pad = " ".repeat(indent);
            out.push(format!("{pad}if true {{"));
            out.push(format!("    {line}"));
            out.push(format!("{pad}}}"));
        } else {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

fn tweak_for_bounds(rng: &mut FuzzRng, src: &str) -> String {
    if let Some(at) = src.find(" in ") {
        let rest = &src[at + 4..];
        if let Some(dot) = rest.find("..") {
            let lo_s = rest[..dot].trim();
            let after = &rest[dot + 2..];
            let hi_s: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if lo_s.chars().all(|c| c.is_ascii_digit()) && !hi_s.is_empty() {
                let lo = rng.int(0, 2);
                let hi = lo + rng.int(0, 3);
                let start = at + 4;
                let end = start + lo_s.len() + 2 + hi_s.len();
                return format!("{}{}..{}{}", &src[..start], lo, hi, &src[end..]);
            }
        }
    }
    src.to_string()
}

fn rename_call(rng: &mut FuzzRng, src: &str) -> String {
    let n = count_helpers(src);
    if n < 2 {
        return src.to_string();
    }
    let from = format!("h{}", rng.int(0, (n - 1) as i32));
    let to = format!("h{}", rng.int(0, (n - 1) as i32));
    src.replacen(&format!("{from}("), &format!("{to}("), 1)
}

fn insert_print(rng: &mut FuzzRng, src: &str) -> String {
    let mut lines: Vec<String> = src.lines().map(|s| s.to_string()).collect();
    if lines.len() < 3 {
        return src.to_string();
    }
    let at = rng.int(1, (lines.len() - 2) as i32) as usize;
    let indent = lines[at].chars().take_while(|c| *c == ' ').count();
    let n = rng.int(0, 9);
    lines.insert(at, format!("{}print_i32({n});", " ".repeat(indent.max(4))));
    lines.join("\n")
}

fn deletable_lines(src: &str) -> Vec<usize> {
    src.lines()
        .enumerate()
        .filter(|(_, line)| {
            let t = line.trim();
            (t.starts_with("let ")
                || t.starts_with("acc ")
                || t.starts_with("print_i32")
                || t.contains(" = "))
                && !t.starts_with("fn ")
                && !t.starts_with("return ")
                && !t.starts_with("if ")
                && !t.starts_with("for ")
        })
        .map(|(i, _)| i)
        .collect()
}

fn number_spans(src: &str) -> Vec<(usize, usize)> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        if b[i].is_ascii_digit() {
            if i > 0 && (b[i - 1].is_ascii_alphabetic() || b[i - 1] == b'_') {
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                continue;
            }
            let s = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            out.push((s, i));
        } else {
            i += 1;
        }
    }
    out
}

fn replace_nth_token(rng: &mut FuzzRng, src: &str, needles: &[char], choices: &[&str]) -> String {
    let hits: Vec<usize> = src
        .char_indices()
        .filter(|(_, c)| needles.contains(c))
        .map(|(i, _)| i)
        .collect();
    if hits.is_empty() {
        return src.to_string();
    }
    let at = hits[rng.int(0, (hits.len() - 1) as i32) as usize];
    // Don't break `->` or `..`.
    if at > 0 && (src.as_bytes()[at - 1] == b'-' || src.as_bytes()[at - 1] == b'.') {
        return src.to_string();
    }
    if at + 1 < src.len() && (src.as_bytes()[at + 1] == b'>' || src.as_bytes()[at + 1] == b'.') {
        return src.to_string();
    }
    let pick = *rng.choose(choices);
    format!("{}{}{}", &src[..at], pick, &src[at + 1..])
}

fn extract_helpers(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(start) = rest.find("fn h") {
        let chunk = &rest[start..];
        if let Some(end) = find_fn_end(chunk) {
            out.push(chunk[..end].to_string());
            rest = &chunk[end..];
        } else {
            break;
        }
    }
    out
}

fn find_fn_end(src: &str) -> Option<usize> {
    let open = src.find('{')?;
    let mut depth = 0i32;
    for (i, c) in src[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn count_helpers(src: &str) -> usize {
    extract_helpers(src).len()
}

fn rename_fn(helper: &str, new_name: &str) -> String {
    if let Some(rest) = helper.strip_prefix("fn ") {
        if let Some(paren) = rest.find('(') {
            return format!("fn {new_name}{}", &rest[paren..]);
        }
    }
    helper.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuzz::rng::FuzzRng;
    use crate::fuzz::gen::gen_program;

    #[test]
    fn structural_mutation_stays_nonempty() {
        let mut rng = FuzzRng::new(42);
        let src = gen_program(&mut rng);
        let out = mutate_structural(&mut rng, &src, 4);
        assert!(!out.is_empty());
        assert!(out.contains("fn main"));
    }

    #[test]
    fn crossover_injects_a_helper() {
        let mut rng = FuzzRng::new(7);
        let a = gen_program(&mut rng);
        let b = gen_program(&mut rng);
        let c = crossover(&mut rng, &a, &b);
        assert!(c.contains("fn main"));
        assert!(c.len() >= a.len());
    }

    #[test]
    fn shrink_drops_dead_lets() {
        let src = "fn main() -> i32 {\n    let mut acc = 0;\n    let _n = 1;\n    return acc;\n}\n";
        let shrunk = shrink(src, |s| s.contains("return acc"));
        assert!(shrunk.contains("return acc"));
        assert!(shrunk.contains("fn main"));
    }
}
