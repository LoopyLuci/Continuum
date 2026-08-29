# Performance Profiling Guide

## Flamegraphs

Install flamegraph:
```bash
cargo install flamegraph --locked
```

Generate flamegraph for transport:
```bash
cargo flamegraph --release -p continuum-transport --example flamegraph_test
```

## Binary Size Audit

Install cargo-bloat:
```bash
cargo install cargo-bloat --locked
```

Analyze binary size:
```bash
cargo bloat --release -p continuum-transport
```

## Benchmarks

Run benchmarks:
```bash
cargo bench -p continuum-transport
```

Key benchmarks to add:
- Frame encode/decode throughput
- Network throughput with QUIC
- Memory allocation per frame
- Start-up time
