//! Small pieces of CPython behaviour that the Python evaluator's outputs
//! depend on, reimplemented so hashes and statistics match byte for byte:
//! `json.dumps(..., sort_keys=True, ensure_ascii=False)` with the default
//! separators, and `random.Random` (MT19937 with CPython's seeding).

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_file(path: &std::path::Path) -> std::io::Result<String> {
    Ok(sha256_hex(&std::fs::read(path)?))
}

/// Python `repr` of a float, as used by json.dumps (shortest round-trip;
/// integral floats keep `.0`). serde_json/ryu prints the same digits.
pub fn py_float(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    let s = serde_json::to_string(&x).unwrap();
    // Python uses exponent notation outside [1e-4, 1e16); ryu differs in style
    // there, but hashes here only ever carry small magnitudes (budgets, 60.0).
    s
}

/// json.dumps(value, sort_keys=True, ensure_ascii=False) with default
/// separators (", ", ": "). Numbers: ints plain, floats via py_float.
/// serde_json::Value stores floats as f64, which is all the hash inputs need.
pub fn py_json_dumps(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, &mut out);
    out
}

fn write_value(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                let _ = write!(out, "{i}");
            } else if let Some(u) = n.as_u64() {
                let _ = write!(out, "{u}");
            } else {
                out.push_str(&py_float(n.as_f64().unwrap()));
            }
        }
        Value::String(s) => write_py_str(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_value(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_py_str(k, out);
                out.push_str(": ");
                write_value(&map[*k], out);
            }
            out.push('}');
        }
    }
}

/// CPython `random.Random`: MT19937 seeded with init_by_array over the
/// little-endian 32-bit chunks of the integer seed.
pub struct PyRandom {
    mt: [u32; 624],
    index: usize,
}

impl PyRandom {
    pub fn new(seed: u64) -> Self {
        let mut key = Vec::new();
        let mut n = seed;
        if n == 0 {
            key.push(0);
        }
        while n > 0 {
            key.push((n & 0xffff_ffff) as u32);
            n >>= 32;
        }
        let mut r = PyRandom {
            mt: [0; 624],
            index: 624,
        };
        r.init_genrand(19650218);
        r.init_by_array(&key);
        r
    }

    fn init_genrand(&mut self, s: u32) {
        self.mt[0] = s;
        for i in 1..624 {
            let prev = self.mt[i - 1];
            self.mt[i] = 1812433253u32
                .wrapping_mul(prev ^ (prev >> 30))
                .wrapping_add(i as u32);
        }
        self.index = 624;
    }

    fn init_by_array(&mut self, key: &[u32]) {
        let (mut i, mut j) = (1usize, 0usize);
        let mut k = 624.max(key.len());
        while k > 0 {
            let prev = self.mt[i - 1];
            self.mt[i] = (self.mt[i] ^ (prev ^ (prev >> 30)).wrapping_mul(1664525))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= 624 {
                self.mt[0] = self.mt[623];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
            k -= 1;
        }
        for _ in 0..623 {
            let prev = self.mt[i - 1];
            self.mt[i] = (self.mt[i] ^ (prev ^ (prev >> 30)).wrapping_mul(1566083941))
                .wrapping_sub(i as u32);
            i += 1;
            if i >= 624 {
                self.mt[0] = self.mt[623];
                i = 1;
            }
        }
        self.mt[0] = 0x8000_0000;
    }

    fn twist(&mut self) {
        for i in 0..624 {
            let y = (self.mt[i] & 0x8000_0000) | (self.mt[(i + 1) % 624] & 0x7fff_ffff);
            self.mt[i] =
                self.mt[(i + 397) % 624] ^ (y >> 1) ^ if y & 1 != 0 { 0x9908_b0df } else { 0 };
        }
        self.index = 0;
    }

    pub fn int32(&mut self) -> u32 {
        if self.index >= 624 {
            self.twist();
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^= y >> 18;
        y
    }

    /// CPython random(): 53-bit float in [0, 1).
    pub fn random(&mut self) -> f64 {
        let a = (self.int32() >> 5) as f64;
        let b = (self.int32() >> 6) as f64;
        (a * 67_108_864.0 + b) * (1.0 / 9_007_199_254_740_992.0)
    }

    /// CPython choices(population, k) without weights.
    pub fn choices<'a, T>(&mut self, population: &'a [T], k: usize) -> Vec<&'a T> {
        let n = population.len() as f64;
        (0..k)
            .map(|_| &population[(self.random() * n) as usize])
            .collect()
    }
}

/// ensure_ascii=False string escaping: only `"`, `\\` and control chars are
/// escaped; `\/` never; short escapes for \n \r \t \b \f like CPython.
fn write_py_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
