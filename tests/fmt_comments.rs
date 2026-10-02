//! `aether fmt` keeps comments. Every case checks three invariants:
//! (1) the multiset of comment texts is unchanged (none lost, none
//! duplicated), (2) `fmt(fmt(p)) == fmt(p)`, (3) the formatted program
//! compiles to the same IR and runs the same as the original, at -O0 and
//! -O2. Covered: examples/, stdlib/, corpus/, hand-written adversarial
//! programs, and random comments inserted into generated programs.

use aether::comments::{comment_texts, format_source};
use aether::fuzz::{gen_aggregate_program, FuzzRng};
use aether::lexer::tokenize;
use aether::span::FileId;
use aether::{compile_file, compile_source, run_compiled, CompileOptions, Compiled};
use std::path::{Path, PathBuf};
use std::process::Command;

fn opts(opt_level: u8) -> CompileOptions {
    CompileOptions {
        opt_level,
        color: false,
    }
}

/// IR text, diagnostic codes and run result (value + stdout, or `ERR`).
fn observe(mut c: Compiled) -> String {
    let codes: Vec<&str> = c.diags.iter().filter_map(|d| d.code).collect();
    if c.diags.has_errors() {
        return format!("errors {codes:?}");
    }
    let ir = c.ir.as_ref().map(|m| m.to_string()).unwrap_or_default();
    let run = match run_compiled(&mut c) {
        Ok((v, out, _)) => format!("{v:?}|{out}"),
        Err(_) => "ERR".to_string(),
    };
    format!("{codes:?}\n{ir}\n{run}")
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

/// Invariants (1) and (2); returns `fmt(src)`.
fn check_text(name: &str, src: &str) -> String {
    let once = format_source(src).unwrap_or_else(|e| panic!("{name}: fmt failed: {e}\n{src}"));
    assert_eq!(
        sorted(comment_texts(src)),
        sorted(comment_texts(&once)),
        "{name}: comments lost or duplicated\n--- src\n{src}\n--- fmt\n{once}"
    );
    let twice = format_source(&once).unwrap_or_else(|e| panic!("{name}: fmt output does not parse: {e}\n{once}"));
    assert_eq!(once, twice, "{name}: fmt is not idempotent\n--- src\n{src}");
    once
}

/// All three invariants for a single-file program.
fn check(name: &str, src: &str, must_compile: bool) -> String {
    let once = check_text(name, src);
    for o in [0, 2] {
        let a = observe(compile_source("a.ae", src, &opts(o)));
        if must_compile {
            assert!(!a.starts_with("errors"), "{name}: does not compile: {a}\n{src}");
        }
        let b = observe(compile_source("a.ae", &once, &opts(o)));
        assert_eq!(a, b, "{name}: behaviour changed at -O{o}\n--- src\n{src}\n--- fmt\n{once}");
    }
    once
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("aether_fmtc_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Formats every `.ae` file of `dirs` into a copy of the tree (so `use`
/// paths such as `../stdlib/x.ae` still resolve) and compares each file's
/// behaviour with the original's.
fn check_dirs(dirs: &[&str], tag: &str) -> usize {
    let out = scratch(tag);
    let mut pairs = Vec::new();
    for dir in dirs {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(dir);
        std::fs::create_dir_all(out.join(dir)).unwrap();
        let mut files: Vec<PathBuf> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().map_or(false, |e| e == "ae"))
            // other tests drop short-lived `zz_*.ae` files into examples/
            .filter(|p| !p.file_name().unwrap().to_string_lossy().starts_with("zz_"))
            .collect();
        files.sort();
        for f in files {
            let src = std::fs::read_to_string(&f).unwrap();
            let once = check_text(&f.display().to_string(), &src);
            let g = out.join(dir).join(f.file_name().unwrap());
            std::fs::write(&g, once).unwrap();
            pairs.push((f, g));
        }
    }
    for (f, g) in &pairs {
        for o in [0, 2] {
            let a = observe(compile_file(f.to_str().unwrap(), &opts(o)).unwrap());
            let b = observe(compile_file(g.to_str().unwrap(), &opts(o)).unwrap());
            assert_eq!(a, b, "{}: behaviour changed at -O{o}", f.display());
        }
    }
    let _ = std::fs::remove_dir_all(&out);
    pairs.len()
}

#[test]
fn examples_and_stdlib_keep_comments_and_behaviour() {
    assert!(check_dirs(&["examples", "stdlib"], "ex") >= 15);
}

#[test]
fn corpus_keeps_comments_and_behaviour() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus");
    let mut n = 0;
    for e in std::fs::read_dir(&root).unwrap() {
        let p = e.unwrap().path();
        if p.extension().map_or(false, |e| e == "ae") {
            let src = std::fs::read_to_string(&p).unwrap();
            check(&p.display().to_string(), &src, false);
            n += 1;
        }
    }
    assert!(n >= 100, "corpus has {n} files");
}

/// Hand-written programs with comments in every position.
const ADVERSARIAL: &[&str] = &[
    // 0: leading / trailing / end-of-block / end-of-file
    "// file header\n\n// doc for main\nfn main() -> i32 { // entry\n    let x = 1; // why\n    print_i32(x); /* after call */\n    return 0;\n    // last words\n}\n// trailing file comment\n",
    // 1: else and `{`
    "fn main() -> i32 {\n    let x = 3;\n    if x > 2 /* cond */ { print_i32(1); } /* a */ else /* b */ { // c\n        print_i32(2);\n    }\n    return 0;\n}\n",
    // 2: else-if chain
    "fn main() -> i32 {\n    let x = 5;\n    if x < 0 { print_i32(0); } // neg\n    else /* x */ if x < 3 { print_i32(1); } // small\n    else { /* big */ print_i32(2); } // done\n    return x;\n}\n",
    // 3: match arms
    "enum E { A(i32), B, C }\nfn f(e: E) -> i32 {\n    match e { // open\n        // lead A\n        E::A(v) => v, // tail A\n        /* lead B */ E::B => { // B open\n            2 // B tail\n        } // after B\n        E::C /* in pat */ => 3,\n        // end of arms\n    }\n}\nfn main() -> i32 { print_i32(f(E::A(7)) + f(E::B) + f(E::C)); return 0; }\n",
    // 4: struct literals and struct declarations
    "struct P { // P open\n    // x doc\n    x: i32, // x trail\n    y /* ty */: i32,\n    /* end */\n}\nfn main() -> i32 {\n    let p = P { /* a */ x: 1, // b\n        y: 2 /* c */ };\n    print_i32(p.x + p.y);\n    return 0;\n}\n",
    // 5: comments after use-like items and externs, enums
    "enum Color { // colors\n    Red, // r\n    /* g */ Green,\n    Blue(i32), // b\n    // end\n}\nfn main() -> i32 { let c = Color::Blue(4); match c { Color::Blue(n) => print_i32(n), _ => print_i32(0), } return 0; }\n",
    // 6: EOF without newline (line comment)
    "fn main() -> i32 { return 0; }\n// no newline",
    // 7: EOF without newline (block comment)
    "fn main() -> i32 { return 0; } /* eof block */",
    // 8: not-nested block comments
    "fn main() -> i32 {\n    /* a /* b */ let x = 2; /* /* */\n    print_i32(x);\n    return 0;\n}\n",
    // 9: `//` and `/*` inside strings and chars
    "fn main() -> i32 {\n    let s = \"// not a comment\"; // real\n    let t = \"/* nor this */\";\n    let c = '/';\n    println(s); println(t); print_char(c);\n    return 0;\n}\n",
    // 10: multi-line block comments at several depths
    "/*\n * Header block\n *   indented\n */\nfn main() -> i32 {\n    /* two\n       lines */\n    let mut x = 1;\n    while x < 3 {\n        /*\n         deep\n        */\n        x = x;\n        break;\n    }\n    return x;\n}\n",
    // 11: tabs and unicode
    "fn main() -> i32 {\n\t// tab-indented ünïcødé ✓ 日本語\n\tlet x = 1;\t/* tab ➜ trail */\n\tprint_i32(x);\n\treturn 0;\n}\n// fim — até já\n",
    // 12: comments inside expressions are hoisted, not dropped
    "fn add(a: i32, /* p */ b: i32) -> i32 { return a + /* in expr */ b; }\nfn main() -> i32 {\n    let v = add(1, // arg one\n        2);\n    print_i32(v * (/* g */ 3));\n    return 0;\n}\n",
    // 13: blank-line groups (several blank lines collapse to one)
    "// one\n\n\n\n// two\nfn a() -> i32 { return 1; }\n\n\n// three\n\nfn main() -> i32 {\n    let x = a();\n\n\n    // spaced\n    print_i32(x);\n\n    return 0;\n}\n",
    // 14: if let and its generated arm
    "enum O { S(i32), N }\nfn main() -> i32 {\n    let o = O::S(3);\n    if let O::S(v) /* pat */ = o { // then\n        print_i32(v);\n    } /* mid */ else { // else\n        print_i32(0);\n    } // after\n    if let O::N = o { print_i32(9); } // no else\n    return 0;\n}\n",
    // 15: for / while / nested blocks
    "fn main() -> i32 {\n    let mut s = 0;\n    for i /* var */ in 0..4 { // loop\n        { // inner block\n            s = s + i; // acc\n        } // inner end\n    }\n    while s > 100 /* never */ { s = 0; }\n    print_i32(s);\n    return s;\n}\n",
    // 16: match expression in let, nested match
    "fn main() -> i32 {\n    let x = 2;\n    let r = match x { // m\n        1 => 10, // one\n        2 => match x + 1 { // inner\n            3 => 30, // three\n            _ => 0,\n        }, // after inner\n        _ => 0 // last\n    }; // after let\n    print_i32(r);\n    return 0;\n}\n",
    // 17: only comments, no items besides main at the end
    "// a\n/* b */\n// c\n\n/* d */\nfn main() -> i32 { return 0; }\n",
    // 18: comment-only blocks and empty bodies
    "fn nothing() -> () { // nothing to do\n}\nfn empty() -> () { /* still nothing */ }\nfn main() -> i32 {\n    nothing();\n    empty();\n    {\n        // empty block\n    }\n    return 0;\n}\n",
    // 19: several comments on one line
    "fn main() -> i32 {\n    let x = 1; /* a */ /* b */ // c\n    /* d */ /* e */ let y = 2;\n    print_i32(x + y); /* f\n    spans */ print_i32(0);\n    return 0;\n}\n",
    // 20: comments between `;` / `,` and the next element
    "fn main() -> i32 {\n    let a = [1, /* x */ 2, 3] /* before semi */;\n    print_i32(a[0] /* idx */ + a[2])  /* before ; */ ;\n    return 0;\n}\n",
    // 21: return / break / continue arm bodies
    "fn f(x: i32) -> i32 {\n    for i in 0..10 {\n        match i { // m\n            0 => continue, // skip\n            3 => break, /* stop */\n            _ => print_i32(i), // print\n        }\n    }\n    match x { 1 => return 5, /* r */ _ => {} }\n    return x;\n}\nfn main() -> i32 { print_i32(f(1)); return f(2); }\n",
    // 22: compound assignment (desugared) with comments
    "fn main() -> i32 {\n    let mut x = 1;\n    x += /* add */ 2; // plus\n    x <<= 1 /* shl */;\n    print_i32(x);\n    return x;\n}\n",
    // 23: tuples and let destructuring
    "fn main() -> i32 {\n    let (a, /* b */ b) = (1, (2 /* two */)); // tuple\n    let t = (a, b);\n    match t { (1, y) /* pat */ => print_i32(y), _ => print_i32(0) }\n    return 0;\n}\n",
    // 24: extern and pub items with comments in front of `fn`
    "pub /* vis */ fn helper() -> i32 { return 4; } // helper\n/* x */ pub struct S { v: i32 }\nfn main() -> i32 { let s = S { v: helper() }; print_i32(s.v); return 0; }\n",
    // 25: deeply nested with trailing comments on closing braces
    "fn main() -> i32 {\n    let mut n = 0;\n    while n < 2 { // w\n        if n == 0 { // i\n            n = n + 1;\n        } else { // e\n            n = n + 1;\n        } // ie\n    } // w end\n    print_i32(n);\n    return 0;\n} // main end\n",
    // 26: comment before `}` of a match arm block and of a match
    "fn main() -> i32 {\n    let x = 1;\n    match x {\n        1 => {\n            print_i32(1);\n            // end of arm\n        }\n        _ => {}\n        // end of match\n    }\n    return 0;\n}\n",
    // 27: comment between `fn` signature parts
    "fn f(/* none */) -> /* ret */ i32 /* before body */ { return 1; }\nfn main() -> i32 { print_i32(f()); return 0; }\n",
    // 28: comment glued to tokens without spaces
    "fn main() -> i32 {let x=1;/*a*/let y=x+/*b*/2;//c\nprint_i32(y);return 0;}//d",
    // 29: block comment containing `//` and `*` characters
    "fn main() -> i32 {\n    /* // not a line comment ** * / */\n    // line with /* opener only\n    print_i32(1);\n    return 0;\n}\n",
    // 30: float / char literals with comments
    "fn main() -> i32 {\n    let f = 1.5 /* f */ * 2.0; // float\n    let c = 'x'; /* char */\n    print_char(c);\n    if f > 2.0 { print_i32(1); }\n    return 0;\n}\n",
    // 31: struct with no fields and enum with one variant
    "struct U { /* empty */ }\nenum One { // single\n    Only\n}\nfn main() -> i32 { let o = One::Only; match o { One::Only => print_i32(1), } return 0; }\n",
];

#[test]
fn adversarial_programs_keep_every_comment() {
    for (i, src) in ADVERSARIAL.iter().enumerate() {
        check(&format!("adversarial #{i}"), src, true);
        // the same program with CRLF line ends
        let crlf = src.replace('\n', "\r\n");
        let out = check(&format!("adversarial #{i} (CRLF)"), &crlf, true);
        assert!(!out.contains('\r'), "adversarial #{i}: CR left in output");
    }
}

fn fmt(src: &str) -> String {
    check("exact", src, false)
}

#[test]
fn comments_stay_where_they_were() {
    // trailing comments stay on their line; end-of-block comments stay in
    // the block; end-of-file comments stay last
    assert_eq!(
        fmt("fn main() -> i32 {\n    let x = 1; // why\n    return x;\n    // end\n}\n// eof"),
        "fn main() -> i32 {\n    let x = 1; // why\n    return x;\n    // end\n}\n// eof\n"
    );
    // a comment after `{` stays on that line, a leading one on its own line
    assert_eq!(
        fmt("fn main() -> i32 { // entry\n/* lead */ return 0; }"),
        "fn main() -> i32 { // entry\n    /* lead */\n    return 0;\n}\n"
    );
    // a comment inside an expression goes before its statement
    assert_eq!(
        fmt("fn main() -> i32 { return 1 + /* one */ 2; }"),
        "fn main() -> i32 {\n    /* one */\n    return 1 + 2;\n}\n"
    );
    // between `else` and `{`: before the `if`
    assert_eq!(
        fmt("fn main() -> i32 { if true { } else /* e */ { } return 0; }"),
        "fn main() -> i32 {\n    /* e */\n    if true {\n    } else {\n    }\n    return 0;\n}\n"
    );
    // match arms and struct fields keep leading and trailing comments
    assert_eq!(
        fmt("struct S {\n// d\nx: i32, // t\n}\nfn main() -> i32 { match 1 { // m\n// a\n1 => 2, // b\n_ => 3 } }"),
        "struct S {\n    // d\n    x: i32, // t\n}\n\nfn main() -> i32 {\n    match 1 { // m\n        // a\n        1 => 2, // b\n        _ => 3,\n    }\n}\n"
    );
    // blank-line groups: at most one blank line is kept
    assert_eq!(
        fmt("// a\n\n\n// b\nfn main() -> i32 {\n    let x = 1;\n\n\n    return x;\n}\n"),
        "// a\n\n// b\nfn main() -> i32 {\n    let x = 1;\n\n    return x;\n}\n"
    );
    // comment-free programs format as before (no blank line inside blocks)
    assert_eq!(
        fmt("fn main() -> i32 { let x = 1; return x; }"),
        "fn main() -> i32 {\n    let x = 1;\n    return x;\n}\n"
    );
    // a lone comment
    assert_eq!(fmt("// only"), "// only\n");
}

#[test]
fn cli_fmt_keeps_comments_and_use_lines() {
    let d = scratch("cli");
    std::fs::write(d.join("lib.ae"), "pub fn two() -> i32 { return 2; } // lib\n").unwrap();
    let src = "use \"lib.ae\"; // import\n\n// main\nfn main() -> i32 {\n    print_i32(two()); // call\n    return 0;\n}\n";
    let main = d.join("main.ae");
    std::fs::write(&main, src).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_aether"))
        .args(["fmt", main.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let out = String::from_utf8(o.stdout).unwrap();
    assert_eq!(out, src);
    assert_eq!(out, format_source(src).unwrap());
    let _ = std::fs::remove_dir_all(&d);
}

/// Comment spellings inserted by the random test.
fn random_comment(rng: &mut FuzzRng, n: u64) -> String {
    match rng.next_u64() % 7 {
        0 => format!(" // lc{n}\n"),
        1 => format!(" /* bc{n} */ "),
        2 => format!(" /* ml{n}\n   second line */ "),
        3 => format!(" /* nest{n} /* not nested */ "),
        4 => format!(" //{n} /* line */ \"str\"\n"),
        5 => format!("\n\n// group{n}\n"),
        _ => format!(" /* ü{n} ✓ */"),
    }
}

/// Inserts random comments at token boundaries of generated programs and
/// checks the three invariants.
fn random_comments(programs: u64, seed: u64) {
    for k in 0..programs {
        let mut rng = FuzzRng::new(seed.wrapping_add(k));
        let base = gen_aggregate_program(&mut rng);
        let (toks, _) = tokenize(FileId(0), &base);
        let count = 1 + rng.next_u64() % 12;
        let mut at: Vec<usize> = (0..count)
            .map(|_| toks[(rng.next_u64() % toks.len() as u64) as usize].span.start.0 as usize)
            .collect();
        at.sort();
        let mut src = String::new();
        let mut prev = 0;
        for (n, &p) in at.iter().enumerate() {
            src.push_str(&base[prev..p]);
            src.push_str(&random_comment(&mut rng, n as u64));
            prev = p;
        }
        src.push_str(&base[prev..]);
        assert_eq!(comment_texts(&src).len(), at.len(), "seed {}", seed + k);
        let once = check(&format!("random seed {}", seed + k), &src, false);
        // the comments must not change behaviour either
        let plain = observe(compile_source("a.ae", &base, &opts(2)));
        assert_eq!(plain, observe(compile_source("a.ae", &once, &opts(2))), "seed {}", seed + k);
    }
}

#[test]
fn random_comments_small() {
    random_comments(25, 9000);
}

/// The full run: `cargo test --release --test fmt_comments -- --include-ignored`.
#[test]
#[ignore]
fn random_comments_large() {
    random_comments(400, 1);
}
