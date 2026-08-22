//! Probes the BC7 block swizzle: for sample storage-order block indices in the
//! reference texture (decoded linearly), finds the matching block position in
//! the source image, revealing storage-index -> (block_x, block_y).
//! Usage: `cargo run --example swizzle_probe -- decoded_reference.png mod.png`

use std::fs::File;

fn load_rgb(path: &str) -> (Vec<u8>, usize, usize) {
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
        other => panic!("unsupported {other:?}"),
    }
    (rgb, info.width as usize, info.height as usize)
}

/// The 48 RGB samples of the 4x4 block at block coords (bx, by).
fn block(img: &[u8], w: usize, bx: usize, by: usize) -> [i32; 48] {
    let mut out = [0i32; 48];
    let mut k = 0;
    for dy in 0..4 {
        for dx in 0..4 {
            let p = ((by * 4 + dy) * w + (bx * 4 + dx)) * 3;
            for c in 0..3 {
                out[k] = img[p + c] as i32;
                k += 1;
            }
        }
    }
    out
}

fn ssd(a: &[i32; 48], b: &[i32; 48]) -> i64 {
    (0..48).map(|i| ((a[i] - b[i]) as i64).pow(2)).sum()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (s_img, w, h) = load_rgb(&args[1]); // reference decoded in storage order
    let (m_img, _, _) = load_rgb(&args[2]); // source mod.png
    let bw = w / 4;
    let bh = h / 4;

    let m_blocks: Vec<[i32; 48]> =
        (0..bw * bh).map(|i| block(&m_img, w, i % bw, i / bw)).collect();

    let probes = [0usize, 1, 2, 3, 4, 5, 6, 7, 8, 15, 16, 63, 64, 65, 479, 480, 4095, 4096];
    println!("bw={bw} bh={bh}");
    for &i in &probes {
        let (sx, sy) = (i % bw, i / bw);
        let q = block(&s_img, w, sx, sy);
        let mut best = (usize::MAX, i64::MAX);
        let mut second = i64::MAX;
        for (idx, mb) in m_blocks.iter().enumerate() {
            let d = ssd(&q, mb);
            if d < best.1 {
                second = best.1;
                best = (idx, d);
            } else if d < second {
                second = d;
            }
        }
        let (mx, my) = (best.0 % bw, best.0 / bw);
        println!(
            "storage#{i:<5} -> image ({mx},{my})  ssd={} (2nd={}, gap={})",
            best.1,
            second,
            second - best.1
        );
    }
}
