//! Decode a file to completion and report frame counts (diagnostic helper).

use muzeec_cli_core::engine::decode::SymphoniaDecoder;

fn main() {
    let path = std::env::args().nth(1).expect("usage: probe <file>");
    let mut d = SymphoniaDecoder::open(std::path::Path::new(&path), 48_000, 2).unwrap();
    let mut buf = vec![0.0f32; 1024 * 2];
    let mut total = 0u64;
    let mut calls = 0u64;
    loop {
        let n = d.next_frames(&mut buf, 1024);
        if n == 0 {
            break;
        }
        total += n as u64;
        calls += 1;
        if calls > 200_000 {
            println!("RUNAWAY after {total} frames ({calls} calls)");
            return;
        }
    }
    println!(
        "file={path} frames={total} calls={calls} total_frames={:?} reported_ms={}",
        d.total_frames(),
        d.duration_ms()
    );
}
