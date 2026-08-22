//! CityHash64, the engine's string hash. Used to intern reflection enum/string
//! values and to key Relic game-data (`.rgd`) dictionaries. Ported to match the
//! editor's `DictionaryHash`/`CityHash64` exactly, including strings longer than
//! 16 bytes.

const K0: u64 = 0xc3a5c85c97cb3127;
const K1: u64 = 0xb492b66fbe98f273;
const K2: u64 = 0x9ae16a3b2f90404f;
const KMUL: u64 = 0x9ddfea08eb382d69;

fn u64_le(s: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(s[o..o + 8].try_into().unwrap())
}
fn u32_le(s: &[u8], o: usize) -> u64 {
    u32::from_le_bytes(s[o..o + 4].try_into().unwrap()) as u64
}

fn shift_mix(v: u64) -> u64 {
    v ^ (v >> 47)
}

fn hash_128_to_64(low: u64, high: u64) -> u64 {
    let mut a = (low ^ high).wrapping_mul(KMUL);
    a ^= a >> 47;
    let mut b = (high ^ a).wrapping_mul(KMUL);
    b ^= b >> 47;
    b.wrapping_mul(KMUL)
}

fn hash_len_16(u: u64, v: u64) -> u64 {
    hash_128_to_64(u, v)
}

fn hash_len_16_mul(u: u64, v: u64, mul: u64) -> u64 {
    let mut a = (u ^ v).wrapping_mul(mul);
    a ^= a >> 47;
    let b = (v ^ a).wrapping_mul(mul);
    (b ^ (b >> 47)).wrapping_mul(mul)
}

/// Returns `(low, high)`.
fn weak_hash_len_32(w: u64, x: u64, y: u64, z: u64, mut a: u64, mut b: u64) -> (u64, u64) {
    a = a.wrapping_add(w);
    b = b.wrapping_add(a).wrapping_add(z).rotate_right(21);
    let c = a;
    a = a.wrapping_add(x).wrapping_add(y);
    b = b.wrapping_add(a.rotate_right(44));
    (a.wrapping_add(z), b.wrapping_add(c))
}

fn weak_hash_at(s: &[u8], o: usize, a: u64, b: u64) -> (u64, u64) {
    weak_hash_len_32(u64_le(s, o), u64_le(s, o + 8), u64_le(s, o + 16), u64_le(s, o + 24), a, b)
}

fn hash_len_0_16(s: &[u8]) -> u64 {
    let len = s.len();
    if len >= 8 {
        let mul = K2.wrapping_add(2 * len as u64);
        let a = u64_le(s, 0).wrapping_add(K2);
        let b = u64_le(s, len - 8);
        let u = b.rotate_right(37).wrapping_mul(mul).wrapping_add(a);
        let v = a.rotate_right(25).wrapping_add(b).wrapping_mul(mul);
        hash_len_16_mul(u, v, mul)
    } else if len >= 4 {
        let mul = K2.wrapping_add(2 * len as u64);
        let a = u32_le(s, 0);
        hash_len_16_mul((len as u64).wrapping_add(a << 3), u32_le(s, len - 4), mul)
    } else if len > 0 {
        let a = s[0] as u64;
        let b = s[len >> 1] as u64;
        let c = s[len - 1] as u64;
        let y = a.wrapping_add(b << 8);
        let z = (len as u64).wrapping_add(c << 2);
        shift_mix(y.wrapping_mul(K2) ^ z.wrapping_mul(K0)).wrapping_mul(K2)
    } else {
        K2
    }
}

fn hash_len_17_32(s: &[u8]) -> u64 {
    let len = s.len();
    let mul = K2.wrapping_add(2 * len as u64);
    let a = u64_le(s, 0).wrapping_mul(K1);
    let b = u64_le(s, 8);
    let c = u64_le(s, len - 8).wrapping_mul(mul);
    let d = u64_le(s, len - 16).wrapping_mul(K2);
    hash_len_16_mul(
        a.wrapping_add(b).rotate_right(43).wrapping_add(c.rotate_right(30)).wrapping_add(d),
        a.wrapping_add(b.wrapping_add(K2).rotate_right(18)).wrapping_add(c),
        mul,
    )
}

fn hash_len_33_64(s: &[u8]) -> u64 {
    let len = s.len();
    let mul = K2.wrapping_add(2 * len as u64);
    let a = u64_le(s, 0).wrapping_mul(K2);
    let b = u64_le(s, 8);
    let c = u64_le(s, len - 24);
    let d = u64_le(s, len - 32);
    let e = u64_le(s, 16).wrapping_mul(K2);
    let f = u64_le(s, 24).wrapping_mul(9);
    let g = u64_le(s, len - 8);
    let h = u64_le(s, len - 16).wrapping_mul(mul);

    let n10 = a.wrapping_add(g).rotate_right(43).wrapping_add(b.rotate_right(30).wrapping_add(c).wrapping_mul(9));
    let n11 = ((a.wrapping_add(g)) ^ d).wrapping_add(f).wrapping_add(1);
    let n12 = n10.wrapping_add(n11).wrapping_mul(mul).swap_bytes().wrapping_add(h);
    let n13 = e.wrapping_add(f).rotate_right(42).wrapping_add(c);
    let n14 = n11.wrapping_add(n12).wrapping_mul(mul).swap_bytes().wrapping_add(g).wrapping_mul(mul);
    let n15 = e.wrapping_add(f).wrapping_add(c);
    let a2 = n13.wrapping_add(n15).wrapping_mul(mul).wrapping_add(n14).swap_bytes().wrapping_add(b);
    let b2 = shift_mix(n15.wrapping_add(a2).wrapping_mul(mul).wrapping_add(d).wrapping_add(h)).wrapping_mul(mul);
    b2.wrapping_add(n13)
}

/// CityHash64 over the given bytes, for keys and interned strings of any length.
pub fn city_hash64(s: &[u8]) -> u64 {
    let mut n = s.len();
    if n <= 32 {
        return if n <= 16 { hash_len_0_16(s) } else { hash_len_17_32(s) };
    }
    if n <= 64 {
        return hash_len_33_64(s);
    }

    let mut x = u64_le(s, n - 40);
    let mut y = u64_le(s, n - 16).wrapping_add(u64_le(s, n - 56));
    let mut z = hash_len_16(u64_le(s, n - 48).wrapping_add(n as u64), u64_le(s, n - 24));
    let mut v = weak_hash_at(s, n - 64, n as u64, z);
    let mut w = weak_hash_at(s, n - 32, y.wrapping_add(K1), x);
    x = x.wrapping_mul(K1).wrapping_add(u64_le(s, 0));
    n = (n - 1) & !63;

    let mut pos = 0;
    loop {
        x = x.wrapping_add(y).wrapping_add(v.0).wrapping_add(u64_le(s, pos + 8)).rotate_right(37).wrapping_mul(K1);
        y = y.wrapping_add(v.1).wrapping_add(u64_le(s, pos + 48)).rotate_right(42).wrapping_mul(K1);
        x ^= w.1;
        y = y.wrapping_add(v.0).wrapping_add(u64_le(s, pos + 40));
        z = z.wrapping_add(w.0).rotate_right(33).wrapping_mul(K1);
        v = weak_hash_at(s, pos, v.1.wrapping_mul(K1), x.wrapping_add(w.0));
        w = weak_hash_at(s, pos + 32, z.wrapping_add(w.1), y.wrapping_add(u64_le(s, pos + 16)));
        std::mem::swap(&mut x, &mut z);
        pos += 64;
        n -= 64;
        if n == 0 {
            break;
        }
    }

    hash_len_16(
        hash_len_16(v.0, w.0).wrapping_add(shift_mix(y).wrapping_mul(K1)).wrapping_add(z),
        hash_len_16(v.1, w.1).wrapping_add(x),
    )
}

/// The engine's `DictionaryHash.Hash`: CityHash64 of the lower-cased ASCII key,
/// used for `.rgd` dictionary keys.
pub fn dictionary_hash(key: &str) -> u64 {
    city_hash64(key.to_ascii_lowercase().as_bytes())
}
