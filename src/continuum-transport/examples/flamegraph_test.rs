//! Flamegraph benchmark for continuum-transport
//! Run with: cargo flamegraph --example flamegraph_test

use continuum_transport::capture::synthesize_demo_frame;
use continuum_transport::codec::encode_jpeg;

fn main() {
    // Simple benchmark: capture and encode frames
    let mut total_bytes = 0;
    for _ in 0..100 {
        let img = synthesize_demo_frame();
        let jpeg = encode_jpeg(&img, 80).unwrap();
        total_bytes += jpeg.len();
    }
    println!("Encoded 100 frames, total bytes: {}", total_bytes);
}
