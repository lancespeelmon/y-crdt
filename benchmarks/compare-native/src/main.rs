//! Native yrs against y-octo on the B1/B4 workloads (plan item 6.9). y-octo has no Node package,
//! so this is the Rust counterpart of `benchmarks/compare/compare.mjs`, on the same inputs:
//!
//!   cargo run --release --manifest-path benchmarks/compare-native/Cargo.toml [runs]
//!
//! Each workload runs once to warm up, then `runs` times (5 by default), and the median is
//! reported. Every edit is its own transaction in yrs; y-octo applies each edit on its own.
//! Both libraries count text positions in UTF-16 code units, and every workload checks that
//! both end with the same text.

use std::path::{Path, PathBuf};
use std::time::Instant;
use yrs::updates::decoder::Decode;
use yrs::{GetString, OffsetKind, Options, Text, Transact, Update};

enum Op {
    Insert(u32, String),
    Delete(u32, u32),
}

/// A lib0 var-uint at `pos`, advancing it.
fn var_uint(buf: &[u8], pos: &mut usize) -> u64 {
    let mut num = 0u64;
    let mut shift = 0;
    loop {
        let b = buf[*pos];
        *pos += 1;
        num |= u64::from(b & 0x7f) << shift;
        if b < 0x80 {
            return num;
        }
        shift += 7;
    }
}

/// The B4 editing trace: a var-uint count, then ops tagged 1 (insert: index, string) or
/// 2 (delete: index, length), in lib0 var-uints and length-prefixed strings.
fn read_trace(path: &Path) -> Vec<Op> {
    let buf = std::fs::read(path).expect("read the editing trace");
    let mut pos = 0;
    let n = var_uint(&buf, &mut pos);
    (0..n)
        .map(|_| match var_uint(&buf, &mut pos) {
            1 => {
                let index = var_uint(&buf, &mut pos) as u32;
                let len = var_uint(&buf, &mut pos) as usize;
                let s = std::str::from_utf8(&buf[pos..pos + len]).expect("utf-8 chunk");
                pos += len;
                Op::Insert(index, s.to_owned())
            }
            2 => Op::Delete(
                var_uint(&buf, &mut pos) as u32,
                var_uint(&buf, &mut pos) as u32,
            ),
            tag => panic!("unknown op tag {tag}"),
        })
        .collect()
}

/// The same seeded generator as compare.mjs, so the random positions match it.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f64 {
        let mut s = self.0;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        self.0 = s;
        f64::from(s) / 4_294_967_296.0
    }
}

trait Lib {
    fn insert(&mut self, index: u32, chunk: &str);
    fn remove(&mut self, index: u32, len: u32);
    fn text(&self) -> String;
}

struct Yrs {
    doc: yrs::Doc,
    text: yrs::TextRef,
}

impl Yrs {
    fn new() -> Self {
        let doc = yrs::Doc::with_options(Options {
            offset_kind: OffsetKind::Utf16,
            ..Options::default()
        });
        let text = doc.get_or_insert_text("text");
        Yrs { doc, text }
    }
}

impl Lib for Yrs {
    fn insert(&mut self, index: u32, chunk: &str) {
        self.text.insert(&mut self.doc.transact_mut(), index, chunk);
    }
    fn remove(&mut self, index: u32, len: u32) {
        self.text
            .remove_range(&mut self.doc.transact_mut(), index, len);
    }
    fn text(&self) -> String {
        self.text.get_string(&self.doc.transact())
    }
}

struct Octo {
    _doc: y_octo::Doc,
    text: y_octo::Text,
}

impl Octo {
    fn new() -> Self {
        let doc = y_octo::Doc::new();
        let text = doc.get_or_create_text("text").expect("create text");
        Octo { _doc: doc, text }
    }
}

impl Lib for Octo {
    fn insert(&mut self, index: u32, chunk: &str) {
        self.text.insert(u64::from(index), chunk).expect("insert");
    }
    fn remove(&mut self, index: u32, len: u32) {
        self.text
            .remove(u64::from(index), u64::from(len))
            .expect("remove");
    }
    fn text(&self) -> String {
        self.text.to_string()
    }
}

const N: u32 = 6000;

fn append(lib: &mut dyn Lib) {
    for i in 0..N {
        lib.insert(i, "a");
    }
}

fn random_inserts(lib: &mut dyn Lib) {
    let mut rng = Rng(1);
    for i in 0..N {
        let index = (rng.next() * f64::from(i + 1)) as u32;
        let c = char::from(b'a' + (i % 26) as u8);
        lib.insert(index, c.encode_utf8(&mut [0; 4]));
    }
}

fn trace(lib: &mut dyn Lib, ops: &[Op]) {
    for op in ops {
        match op {
            Op::Insert(i, s) => lib.insert(*i, s),
            Op::Delete(i, n) => lib.remove(*i, *n),
        }
    }
}

fn load_yrs(update: &[u8]) -> String {
    let lib = Yrs::new();
    lib.doc
        .transact_mut()
        .apply_update(Update::decode_v1(update).expect("decode"))
        .expect("apply");
    lib.text()
}

fn load_octo(update: &[u8]) -> String {
    let mut doc = y_octo::Doc::new();
    doc.apply_update_from_binary_v1(update).expect("apply");
    doc.get_or_create_text("text").expect("text").to_string()
}

fn median(mut xs: Vec<f64>) -> f64 {
    xs.sort_by(f64::total_cmp);
    xs[xs.len() / 2]
}

/// Runs `f` once to warm up and `runs` times measured; returns the median in ms and the text,
/// or the panic message if `f` panicked.
fn time(runs: usize, mut f: impl FnMut() -> String) -> Result<(f64, String), String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut text = f();
        let times = (0..runs)
            .map(|_| {
                let start = Instant::now();
                text = f();
                start.elapsed().as_secs_f64() * 1e3
            })
            .collect();
        (median(times), text)
    }))
    .map_err(|e| {
        e.downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| e.downcast_ref::<String>().cloned())
            .unwrap_or_default()
    })
}

fn main() {
    let runs: usize = std::env::args()
        .nth(1)
        .map_or(5, |s| s.parse().expect("runs is a number"));
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/bench-input");
    let ops = read_trace(&assets.join("b4-editing-trace.bin"));
    let update = std::fs::read(assets.join("b4-update.bin")).expect("read the B4 update");

    type Workload<'a> = Box<dyn Fn(&mut dyn Lib) + 'a>;
    let edits: Vec<(&str, Workload)> = vec![
        ("B1.1 append N chars", Box::new(append)),
        (
            "B1.4 insert N chars at random positions",
            Box::new(random_inserts),
        ),
        (
            "B4.1 apply the editing trace",
            Box::new(|lib: &mut dyn Lib| trace(lib, &ops)),
        ),
    ];

    println!("y-octo 0.1.1, median of {runs} runs\n");
    println!("| workload | yrs | y-octo |");
    println!("|---|---|---|");
    let row = |name: &str, yrs: Result<(f64, String), String>, octo: Result<_, String>| {
        let (y, ty) = yrs.expect("yrs panicked");
        match octo {
            Ok((o, to)) => {
                assert!(ty == to, "{name}: the texts differ");
                println!("| {name} | {y:.2} ms | {o:.2} ms ({:.2}×) |", o / y);
            }
            Err(msg) => println!("| {name} | {y:.2} ms | panics: {msg} |"),
        }
    };
    for (name, run) in &edits {
        let y = time(runs, || {
            let mut lib = Yrs::new();
            run(&mut lib);
            lib.text()
        });
        let o = time(runs, || {
            let mut lib = Octo::new();
            run(&mut lib);
            lib.text()
        });
        row(name, y, o);
    }
    row(
        "B4.2 load the trace as one update",
        time(runs, || load_yrs(&update)),
        time(runs, || load_octo(&update)),
    );
}
