//! Raw ConPTY throughput: run a command in a pseudo console and count output bytes.
//! cargo run --release -p hivetty-app --example conpty_bench -- <file>
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::Read;
use std::time::Instant;

fn main() {
    let file = std::env::args().nth(1).expect("file");
    for (cols, rows) in [(120u16, 30u16), (240, 60)] {
        let pair = native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).unwrap();
        let mut cmd = CommandBuilder::new("cmd.exe");
        cmd.args(["/c", "type", &file]);
        let t = Instant::now();
        let mut child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);
        let mut r = pair.master.try_clone_reader().unwrap();
        let mut w = pair.master.take_writer().unwrap();
        let reader = std::thread::spawn(move || {
            use std::io::Write;
            let mut answered = false;
            let mut buf = vec![0u8; 64 * 1024];
            let (mut n, mut reads) = (0usize, 0usize);
            while let Ok(k) = r.read(&mut buf) {
                if k == 0 {
                    break;
                }
                // ConPTY waits for an answer to its startup cursor-position query
                if !answered && buf[..k].windows(4).any(|x| x == b"[6n") {
                    let _ = w.write_all(b"[1;1R");
                    answered = true;
                }
                n += k;
                reads += 1;
            }
            (n, reads)
        });
        child.wait().unwrap();
        let s = t.elapsed().as_secs_f64();
        drop(pair.master);
        let (n, reads) = reader.join().unwrap();
        println!("{cols}x{rows}: {:.1} MB out in {:.1} s ({} reads, avg {} B) = {:.1} MB/s", n as f64 / 1048576.0, s, reads, n / reads.max(1), n as f64 / 1048576.0 / s);
    }
}
