use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use rust_mjpeg_rtp::rtp::RtpPacketizer;

fn create_test_jpeg(size: usize) -> Vec<u8> {
    let mut jpeg = vec![0xFF, 0xD8]; // SOI
    jpeg.extend((0..size).map(|i| (i % 256) as u8));
    jpeg.extend(&[0xFF, 0xD9]); // EOI
    jpeg
}

fn benchmark_packetize_jpeg(c: &mut Criterion) {
    let mut group = c.benchmark_group("packetize_jpeg");

    // Test different JPEG sizes (typical webcam frames)
    for size in [5_000, 20_000, 50_000, 100_000].iter() {
        let jpeg = create_test_jpeg(*size);
        let packetizer = RtpPacketizer::new(0x12345678, 1400);

        group.bench_with_input(BenchmarkId::new("jpeg_size", size), &jpeg, |b, jpeg| {
            b.iter(|| {
                packetizer.packetize_jpeg(
                    black_box(jpeg),
                    black_box(640),
                    black_box(480),
                    black_box(90000),
                )
            });
        });
    }

    group.finish();
}

fn benchmark_timestamp_generation(c: &mut Criterion) {
    let packetizer = RtpPacketizer::new(0x12345678, 1400);

    c.bench_function("calculate_timestamp_30fps", |b| {
        b.iter(|| packetizer.calculate_timestamp(black_box(30)));
    });
}

criterion_group!(
    benches,
    benchmark_packetize_jpeg,
    benchmark_timestamp_generation
);
criterion_main!(benches);
