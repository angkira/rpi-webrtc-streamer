// Placeholder benchmark for capture pipeline
// Will be implemented when GStreamer capture module is ready

use criterion::{criterion_group, criterion_main, Criterion};

fn placeholder_benchmark(c: &mut Criterion) {
    c.bench_function("placeholder", |b| {
        b.iter(|| {
            // Placeholder
            1 + 1
        });
    });
}

criterion_group!(benches, placeholder_benchmark);
criterion_main!(benches);
