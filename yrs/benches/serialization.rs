use criterion::{criterion_group, criterion_main, Criterion, SamplingMode};
use std::hint::black_box;
use yrs::encoding::read::{Cursor, Read};
use yrs::encoding::write::Write;
use yrs::Any;

const BENCHMARK_SIZE: u32 = 100000;

fn bench_encoding(c: &mut Criterion) {
    let mut encoding_group = c.benchmark_group("encoding");
    encoding_group.sampling_mode(SamplingMode::Flat);

    encoding_group.bench_function("var_int (64 bit)", |b| {
        b.iter(|| {
            let mut encoder = Vec::with_capacity(BENCHMARK_SIZE as usize * 8);
            for i in 0..(BENCHMARK_SIZE as i64) {
                encoder.write_var(i);
            }
            let mut decoder = Cursor::from(&encoder);
            for i in 0..(BENCHMARK_SIZE as i64) {
                let num: i64 = decoder.read_var().unwrap();
                assert_eq!(num, i);
            }
        })
    });

    encoding_group.bench_function("var_uint (32 bit)", |b| {
        b.iter(|| {
            let mut encoder = Vec::with_capacity(BENCHMARK_SIZE as usize * 8);
            for i in 0..BENCHMARK_SIZE {
                encoder.write_var(i);
            }
            let mut decoder = Cursor::from(&encoder);
            for i in 0..BENCHMARK_SIZE {
                let num: u32 = decoder.read_var().unwrap();
                assert_eq!(num, i);
            }
        })
    });

    encoding_group.bench_function("uint32", |b| {
        b.iter(|| {
            let mut encoder = Vec::with_capacity(BENCHMARK_SIZE as usize * 8);
            for i in 0..BENCHMARK_SIZE {
                encoder.write_u32(i);
            }
            let mut decoder = Cursor::from(&encoder);
            for i in 0..BENCHMARK_SIZE {
                let num: u32 = decoder.read_u32().unwrap();
                assert_eq!(num, i);
            }
        })
    });

    encoding_group.bench_function("var_uint (64 bit)", |b| {
        b.iter(|| {
            let mut encoder = Vec::with_capacity(BENCHMARK_SIZE as usize * 8);
            for i in 0..(BENCHMARK_SIZE as u64) {
                encoder.write_var(i);
            }
            let mut decoder = Cursor::from(&encoder);
            for i in 0..(BENCHMARK_SIZE as u64) {
                let num: u64 = decoder.read_var().unwrap();
                assert_eq!(num, i);
            }
        })
    });

    encoding_group.bench_function("uint64", |b| {
        b.iter(|| {
            let mut encoder = Vec::with_capacity(BENCHMARK_SIZE as usize * 8);
            for i in 0..(BENCHMARK_SIZE as u64) {
                encoder.write_u64(i)
            }
            let mut decoder = Cursor::from(&encoder);
            for i in 0..(BENCHMARK_SIZE as u64) {
                let num = decoder.read_u64().unwrap();
                assert_eq!(num, i);
            }
        })
    });
}

fn bench_serialization(c: &mut Criterion) {
    use std::collections::HashMap;

    let any = Any::from(HashMap::from([
        ("bool".into(), Any::from(true)),
        ("int".into(), Any::from(1)),
        ("negativeInt".into(), Any::from(-1)),
        ("maxInt".into(), Any::from(i64::MAX)),
        ("minInt".into(), Any::from(i64::MIN)),
        ("realNumber".into(), Any::from(-123.2387f64)),
        ("maxNumber".into(), Any::from(f64::MIN)),
        ("minNumber".into(), Any::from(f64::MAX)),
        ("null".into(), Any::Null),
        (
            "map".into(),
            Any::from(HashMap::from([
                ("bool".into(), Any::from(true)),
                ("int".into(), Any::from(1)),
                ("negativeInt".into(), Any::from(-1)),
                ("maxInt".into(), Any::from(i64::MAX)),
                ("minInt".into(), Any::from(i64::MIN)),
                ("realNumber".into(), Any::from(-123.2387f64)),
                ("maxNumber".into(), Any::from(f64::MIN)),
                ("minNumber".into(), Any::from(f64::MAX)),
                ("null".into(), Any::Null),
            ])),
        ),
        (
            "key6".into(),
            Any::from(vec![
                Any::from(true),
                Any::from(1),
                Any::from(-1),
                Any::from(i64::MAX),
                Any::from(i64::MIN),
                Any::from(-123.2387f64),
                Any::from(f64::MIN),
                Any::from(f64::MAX),
                Any::Null,
            ]),
        ),
    ]));

    let any_json = serde_json::to_string(&any).unwrap();

    let mut serde_group = c.benchmark_group("serde");
    serde_group.bench_function("Any-JSON roundtrip", |b| {
        b.iter(|| {
            black_box(serde_json::from_str::<Any>(&serde_json::to_string(&any).unwrap()).unwrap());
        })
    });

    serde_group.bench_function("Any serialize", |b| {
        b.iter(|| {
            black_box(serde_json::to_string(&any).unwrap());
        })
    });

    serde_group.bench_function("Any deserialize", |b| {
        b.iter(|| {
            black_box(serde_json::from_str::<Any>(&any_json).unwrap());
        })
    });

    serde_group.finish();

    let mut custom_group = c.benchmark_group("custom json serialization");

    custom_group.bench_function("Any-JSON roundtrip", |b| {
        b.iter(|| {
            let mut roundtrip = String::new();
            any.to_json(&mut roundtrip);

            black_box(Any::from_json(&roundtrip).unwrap());
        })
    });

    custom_group.bench_function("Any serialize", |b| {
        b.iter(|| {
            let mut roundtrip = String::new();
            black_box(any.to_json(&mut roundtrip));
        })
    });

    custom_group.bench_function("Any deserialize", |b| {
        b.iter(|| {
            black_box(Any::from_json(&any_json).unwrap());
        })
    });

    custom_group.finish();
}

/// Plan item 6.3: merging many small updates, as a server compacting a document's history does.
fn bench_merge_updates(c: &mut Criterion) {
    use yrs::{ReadTxn, Text, Transact};
    let mut group = c.benchmark_group("merge_updates");
    group.sampling_mode(SamplingMode::Flat);
    for n in [1_000usize, 16_000] {
        let docs: Vec<yrs::Doc> = (0..16).map(|i| yrs::Doc::with_client_id(i + 1)).collect();
        let mut updates = Vec::with_capacity(n);
        for i in 0..n {
            let doc = &docs[i % docs.len()];
            let sv = doc.transact().state_vector();
            let text = doc.get_or_insert_text("text");
            let mut txn = doc.transact_mut();
            let len = text.len(&txn);
            text.insert(&mut txn, len, "x");
            drop(txn);
            updates.push(doc.transact().encode_state_as_update_v1(&sv));
        }
        group.bench_function(format!("v1/{n}"), |b| {
            b.iter(|| yrs::merge_updates_v1(black_box(&updates)).unwrap())
        });
    }
    group.finish();
}

criterion_group!(
    serialization,
    bench_encoding,
    bench_serialization,
    bench_merge_updates
);
criterion_main!(serialization);
