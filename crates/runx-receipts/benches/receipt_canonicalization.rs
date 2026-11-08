use criterion::{Criterion, criterion_group, criterion_main};
use runx_contracts::Receipt;
use runx_receipts::{
    canonical_receipt_body_digest, canonical_receipt_body_json, canonical_receipt_json,
};
use serde::Deserialize;
use std::hint::black_box;

const SUCCESS_RECEIPT: &str =
    include_str!("../../../fixtures/contracts/run-spine/receipt-success.json");

#[derive(Debug, Deserialize)]
struct Fixture {
    expected: Receipt,
}

fn bench_receipt_canonicalization(c: &mut Criterion) {
    let receipt = fixture_receipt();

    c.bench_function("receipt_canonicalization", |b| {
        b.iter(|| canonical_receipt_body_digest(black_box(&receipt)))
    });
    c.bench_function("receipt_body_json", |b| {
        b.iter(|| canonical_receipt_body_json(black_box(&receipt)))
    });
    c.bench_function("receipt_full_json", |b| {
        b.iter(|| canonical_receipt_json(black_box(&receipt)))
    });
}

fn fixture_receipt() -> Receipt {
    match serde_json::from_str::<Fixture>(SUCCESS_RECEIPT) {
        Ok(fixture) => fixture.expected,
        Err(_error) => std::process::exit(2),
    }
}

criterion_group!(benches, bench_receipt_canonicalization);
criterion_main!(benches);
