//! Compares two PNGs, printing PSNR and the max per-channel difference (RGB).
//! Usage: `cargo run --example compare_png -- a.png b.png`

use std::fs::File;

fn load_rgb(path: &str) -> (Vec<u8>, u32, u32) {
    let decoder = png::Decoder::new(File::open(path).unwrap());
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    let n = (info.width * info.height) as usize;
    let mut rgb = vec![0u8; n * 3];
    match info.color_type {
        png::ColorType::Rgb => rgb.copy_from_slice(&buf[..n * 3]),
        png::ColorType::Rgba => {
            for i in 0..n {
                rgb[i * 3..i * 3 + 3].copy_from_slice(&buf[i * 4..i * 4 + 3]);
            }
        }
        other => panic!("unsupported color type {other:?}"),
    }
    (rgb, info.width, info.height)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (a, w, h) = load_rgb(&args[1]);
    let (b, _, _) = load_rgb(&args[2]);
    assert_eq!(a.len(), b.len(), "images differ in size");

    let mut sse = 0f64;
    let mut max_diff = 0u8;
    // Also measure an R<->B swapped comparison, to catch channel-order bugs.
    let mut sse_swapped = 0f64;
    for px in 0..a.len() / 3 {
        let (ar, ag, ab) = (a[px * 3], a[px * 3 + 1], a[px * 3 + 2]);
        let (br, bg, bb) = (b[px * 3], b[px * 3 + 1], b[px * 3 + 2]);
        for (x, y) in [(ar, br), (ag, bg), (ab, bb)] {
            let e = x as f64 - y as f64;
            sse += e * e;
            max_diff = max_diff.max((x as i32 - y as i32).unsigned_abs() as u8);
        }
        for (x, y) in [(ar, bb), (ag, bg), (ab, br)] {
            let e = x as f64 - y as f64;
            sse_swapped += e * e;
        }
    }
    let mse = sse / a.len() as f64;
    let psnr = if mse == 0.0 { f64::INFINITY } else { 10.0 * (255.0 * 255.0 / mse).log10() };
    let mse_sw = sse_swapped / a.len() as f64;
    let psnr_sw = if mse_sw == 0.0 { f64::INFINITY } else { 10.0 * (255.0 * 255.0 / mse_sw).log10() };
    println!("{w}x{h}  PSNR={psnr:.2} dB  maxChannelDiff={max_diff}  (R<->B swapped PSNR={psnr_sw:.2} dB)");
}
