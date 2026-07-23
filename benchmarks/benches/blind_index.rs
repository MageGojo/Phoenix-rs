use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use phoenix_crypto::{BlindIndexKey, BlindIndexer};

fn blind_index(c: &mut Criterion) {
    let key = BlindIndexKey::new("benchmark-v1", [0x42; 32]).expect("benchmark key is valid");
    let indexer = BlindIndexer::new(key);
    let value = b"benchmark.user@example.test";

    c.bench_function("blind_index/index_email", |b| {
        b.iter(|| {
            indexer
                .index("benchmark.user.email", black_box(value))
                .expect("benchmark input is valid")
        });
    });
}

criterion_group!(benches, blind_index);
criterion_main!(benches);
