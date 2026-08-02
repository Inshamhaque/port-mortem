//! In-process benchmark for the safe core: parse + print_unformatted throughput
//! and per-document latency over a representative corpus (the original test
//! inputs plus one generated document).
//!
//! Emits a single JSON object to stdout with the in-process metrics;
//! bench/run.py combines these with CLI startup and peak-RSS measurements into
//! bench/results.json. See bench/methodology.md.

use std::time::Instant;

use cjson_rs::{parse, print_unformatted};

const CORPUS_DIR: &str = "tests/original/inputs";
const LATENCY_SAMPLES: usize = 20_000;

fn main() {
    let mut docs: Vec<String> = Vec::new();
    for name in ["test1", "test2", "test4", "test5", "test9", "test11"] {
        if let Ok(text) = std::fs::read_to_string(format!("{CORPUS_DIR}/{name}")) {
            docs.push(text);
        }
    }
    docs.push(large_document());
    assert!(!docs.is_empty(), "no corpus loaded");

    let corpus_bytes: usize = docs.iter().map(String::len).sum();

    // Parse once up front so the value cost is included in what we measure
    // every iteration, and to warm the allocator and parser caches.
    let parsed: Vec<_> = docs.iter().map(|d| parse(d).expect("corpus must parse")).collect();
    for v in &parsed {
        let _ = print_unformatted(v).expect("corpus must print");
    }

    // --- throughput: parse + print_unformatted every document for 3s ---
    let duration = std::time::Duration::from_secs(3);
    let start = Instant::now();
    let mut ops: usize = 0;
    while start.elapsed() < duration {
        for v in &parsed {
            let _ = print_unformatted(v).expect("print");
            ops += 1;
        }
    }
    let throughput_secs = start.elapsed().as_secs_f64();
    let throughput = ops as f64 / throughput_secs;

    // --- per-document latency: sample parse+print_unformatted ---
    let mut samples: Vec<f64> = Vec::with_capacity(LATENCY_SAMPLES);
    for i in 0..LATENCY_SAMPLES {
        let value = &parsed[i % parsed.len()];
        let t0 = Instant::now();
        let _ = print_unformatted(value).expect("print");
        samples.push(t0.elapsed().as_secs_f64());
    }
    samples.sort_by(f64::total_cmp);
    let p50 = samples[samples.len() / 2];
    let p99 = samples[(samples.len() as f64 * 0.99) as usize - 1];
    let mean: f64 = samples.iter().sum::<f64>() / samples.len() as f64;

    let json = format!(
        "{{\"throughput_docs_per_s\":{throughput:.1},\
          \"parse_print_p50_us\":{p50_us:.1},\"parse_print_p99_us\":{p99_us:.1},\
          \"parse_print_mean_us\":{mean_us:.1},\
          \"latency_samples\":{LATENCY_SAMPLES},\
          \"throughput_duration_s\":{throughput_secs:.1},\
          \"corpus_docs\":{docs_len},\"corpus_bytes\":{corpus_bytes}}}",
        throughput = throughput,
        p50_us = p50 * 1e6,
        p99_us = p99 * 1e6,
        mean_us = mean * 1e6,
        throughput_secs = throughput_secs,
        docs_len = docs.len(),
    );
    println!("{json}");
}

/// A nested document large enough (~100 KiB) to make the benchmark meaningful
/// beyond the small original inputs.
fn large_document() -> String {
    let mut out = String::with_capacity(110_000);
    out.push('{');
    for i in 0..400 {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!("\"key{i}\":{{\"id\":{i},\"name\":\"node {i} with some text\",\"ok\":true,\"tags\":[\"a\",\"b\",\"c\"],\"ratio\":{}.5}}", i % 7));
    }
    out.push('}');
    out
}
