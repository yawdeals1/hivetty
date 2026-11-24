//! Throughput of the per-chunk work every pane does (OSC tap + vt100 screen mirror).
//! cargo run --release -p hivetty-app --example ingest_bench
use std::time::Instant;

fn main() {
    let line = format!("{}\r\n", "0123456789abcdefghijklmnopqrstuvwxyz".repeat(3));
    let mut data = Vec::new();
    for i in 0..200_000 {
        data.extend_from_slice(format!("{i} {line}").as_bytes());
    }
    let mb = data.len() as f64 / 1048576.0;
    for (name, f) in [
        ("osc tap", Box::new(|c: &[u8], tap: &mut hivetty_core::osc::OscTap, _: &mut vt100::Parser| { tap.feed(c); }) as Box<dyn Fn(&[u8], &mut _, &mut _)>),
        ("vt100 (5000 lines)", Box::new(|c: &[u8], _: &mut _, p: &mut vt100::Parser| p.process(c))),
    ] {
        let mut tap = hivetty_core::osc::OscTap::new();
        let mut p = vt100::Parser::new(30, 120, 5000);
        let t = Instant::now();
        for c in data.chunks(16 * 1024) {
            f(c, &mut tap, &mut p);
        }
        let s = t.elapsed().as_secs_f64();
        println!("{name:<20} {mb:.1} MB in {:.0} ms = {:.0} MB/s", s * 1000.0, mb / s);
    }
}
