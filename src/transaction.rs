//! X's `x-client-transaction-id` header, required for content-creating writes
//! (`CreateTweet`). Ported from the public reverse-engineering of X's web client:
//! parse a verification key + animation frames + ondemand indices, run them
//! through the loading-animation's cubic-bezier math to derive an animation key,
//! then SHA-256 `method!path!time{keyword}{animkey}` into an XOR-obfuscated blob.

use anyhow::{Result, anyhow};
use base64::Engine;
use sha2::{Digest, Sha256};

const KEYWORD: &str = "obfiowerehiring";
const ADDITIONAL_RANDOM_NUMBER: u8 = 3;
const EPOCH_SECS: i64 = 1_682_924_400;
const ANIM_TOTAL_TIME: f64 = 4096.0;

/// Per-page-load state; reused to mint a fresh id per request.
pub struct TxidGenerator {
    key_bytes: Vec<u8>,
    animation_key: String,
}

impl TxidGenerator {
    /// Fetch the home page + ondemand bundle and derive the animation key.
    pub async fn fetch(
        http: &reqwest::Client,
        cookie: &str,
        user_agent: &str,
    ) -> Result<TxidGenerator> {
        let get = |url: String| {
            http.get(url)
                .header("cookie", cookie)
                .header("user-agent", user_agent)
                .header("accept-language", "en-US,en;q=0.9")
                .send()
        };
        let html = get("https://x.com/home".to_string()).await?.text().await?;

        let key = capture(r#"twitter-site-verification"\s+content="([^"]+)""#, &html)
            .ok_or_else(|| anyhow!("no twitter-site-verification key"))?;
        let key_bytes = base64::engine::general_purpose::STANDARD
            .decode(key.as_bytes())
            .map_err(|e| anyhow!("bad verification key: {e}"))?;

        let idx = capture(r#",(\d+):["']ondemand\.s["']"#, &html)
            .ok_or_else(|| anyhow!("no ondemand index"))?;
        let hash = capture(&format!(r#",{idx}:"([0-9a-f]+)""#), &html)
            .ok_or_else(|| anyhow!("no ondemand hash"))?;
        let ondemand = get(format!(
            "https://abs.twimg.com/responsive-web/client-web/ondemand.s.{hash}a.js"
        ))
        .await?
        .text()
        .await?;

        let indices = key_byte_indices(&ondemand)?;
        let frames = animation_frames(&html);
        let animation_key = animation_key(&key_bytes, indices.0, &indices.1, &frames)
            .ok_or_else(|| anyhow!("could not derive animation key"))?;
        Ok(TxidGenerator {
            key_bytes,
            animation_key,
        })
    }

    pub fn generate(&self, method: &str, path: &str) -> String {
        let time_now = now_secs() - EPOCH_SECS;
        let random_num = now_secs().wrapping_mul(2_654_435_761).to_le_bytes()[0];
        generate_txid(
            &self.key_bytes,
            &self.animation_key,
            method,
            path,
            time_now,
            random_num,
        )
    }
}

fn generate_txid(
    key_bytes: &[u8],
    animation_key: &str,
    method: &str,
    path: &str,
    time_now: i64,
    random_num: u8,
) -> String {
    // The low four little-endian bytes: identical to masking each shifted byte.
    let time_bytes = &time_now.to_le_bytes()[..4];
    let hash =
        Sha256::digest(format!("{method}!{path}!{time_now}{KEYWORD}{animation_key}").as_bytes());

    let mut arr = Vec::with_capacity(key_bytes.len() + 21);
    arr.extend_from_slice(key_bytes);
    arr.extend_from_slice(time_bytes);
    arr.extend_from_slice(&hash[..16]);
    arr.push(ADDITIONAL_RANDOM_NUMBER);

    let mut out = Vec::with_capacity(arr.len() + 1);
    out.push(random_num);
    out.extend(arr.iter().map(|b| b ^ random_num));

    base64::engine::general_purpose::STANDARD
        .encode(out)
        .trim_end_matches('=')
        .to_string()
}

fn capture(pattern: &str, haystack: &str) -> Option<String> {
    regex::Regex::new(pattern)
        .ok()?
        .captures(haystack)?
        .get(1)
        .map(|m| m.as_str().to_string())
}

fn key_byte_indices(ondemand: &str) -> Result<(usize, Vec<usize>)> {
    let re = regex::Regex::new(r"\(\w\[(\d{1,2})\],\s*16\)").unwrap();
    let indices: Vec<usize> = re
        .captures_iter(ondemand)
        .filter_map(|c| c.get(1)?.as_str().parse().ok())
        .collect();
    match indices.split_first() {
        Some((first, rest)) if !rest.is_empty() => Ok((*first, rest.to_vec())),
        _ => Err(anyhow!("could not extract key-byte indices")),
    }
}

/// The 2nd `<path d>` inside each `loading-x-anim` svg's `<g>`.
fn animation_frames(html: &str) -> Vec<String> {
    let re = regex::Regex::new(
        r#"id="loading-x-anim-\d"[^>]*><g><path d="[^"]*"></path><path d="([^"]*)""#,
    )
    .unwrap();
    re.captures_iter(html)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

fn animation_key(
    key_bytes: &[u8],
    row_index_key: usize,
    key_bytes_indices: &[usize],
    frames: &[String],
) -> Option<String> {
    if frames.is_empty() || key_bytes.len() <= 5 {
        return None;
    }
    let row_index = usize::from(*key_bytes.get(row_index_key)?) % 16;
    let frame_time: f64 = key_bytes_indices
        .iter()
        .map(|&i| f64::from(key_bytes[i] % 16))
        .product();
    let frame_time = math_round(frame_time / 10.0) * 10.0;

    let frame = &frames[usize::from(key_bytes[5] % 4)];
    let d = frame.get(9..)?;
    // Parsed straight to f64: every one of these is a coordinate the maths below
    // treats as a float anyway, and X's paths carry values past a byte.
    let arr: Vec<Vec<f64>> = d
        .split('C')
        .map(|seg| {
            seg.split(|c: char| !c.is_ascii_digit())
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.parse().ok())
                .collect()
        })
        .collect();
    let frame_row = arr.get(row_index)?;
    Some(animate(frame_row, frame_time / ANIM_TOTAL_TIME))
}

fn animate(frames: &[f64], target_time: f64) -> String {
    let from_color = [frames[0], frames[1], frames[2], 1.0];
    let to_color = [frames[3], frames[4], frames[5], 1.0];
    let to_rotation = solve(frames[6], 60.0, 360.0, true);
    let curves: Vec<f64> = frames[7..]
        .iter()
        .enumerate()
        .map(|(i, &v)| solve(v, is_odd(i), 1.0, false))
        .collect();

    let val = cubic_value(&curves, target_time);
    let color: Vec<f64> = (0..4)
        .map(|i| (from_color[i] * (1.0 - val) + to_color[i] * val).clamp(0.0, 255.0))
        .collect();
    let rotation = 0.0 * (1.0 - val) + to_rotation * val;
    let matrix = rotation_matrix(rotation);

    let mut parts: Vec<String> = color[..3]
        .iter()
        .map(|&v| format!("{:x}", trunc_u8(math_round(v))))
        .collect();
    for v in matrix {
        let rounded = round2(v).abs();
        let hex = float_to_hex(rounded);
        parts.push(if let Some(rest) = hex.strip_prefix('.') {
            format!("0.{rest}").to_lowercase()
        } else if hex.is_empty() {
            "0".to_string()
        } else {
            hex
        });
    }
    parts.push("0".to_string());
    parts.push("0".to_string());
    parts
        .join("")
        .chars()
        .filter(|c| *c != '.' && *c != '-')
        .collect()
}

fn solve(value: f64, min: f64, max: f64, rounding: bool) -> f64 {
    let r = value * (max - min) / 255.0 + min;
    if rounding { r.floor() } else { round2(r) }
}

fn is_odd(n: usize) -> f64 {
    if n.is_multiple_of(2) { 0.0 } else { -1.0 }
}

/// Round half away from zero to an integer (JS `Math.round` semantics).
fn math_round(n: f64) -> f64 {
    let x = n.floor();
    let x = if (n - x) >= 0.5 { n.ceil() } else { x };
    x.copysign(n)
}

/// Round to 2 decimals (Python `round(v, 2)`).
fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn rotation_matrix(rotation: f64) -> [f64; 4] {
    let rad = rotation.to_radians();
    [rad.cos(), -rad.sin(), rad.sin(), rad.cos()]
}

#[expect(
    clippy::float_cmp,
    reason = "mirrors the strict === in X's own solver; an epsilon would pick a different branch near 1.0 and change the derived key"
)]
fn cubic_value(c: &[f64], t: f64) -> f64 {
    let calc = |a: f64, b: f64, m: f64| {
        3.0 * a * (1.0 - m).powi(2) * m + 3.0 * b * (1.0 - m) * m * m + m * m * m
    };
    if t <= 0.0 {
        let g = if c[0] > 0.0 {
            c[1] / c[0]
        } else if c[1] == 0.0 && c[2] > 0.0 {
            c[3] / c[2]
        } else {
            0.0
        };
        return g * t;
    }
    if t >= 1.0 {
        let g = if c[2] < 1.0 {
            (c[3] - 1.0) / (c[2] - 1.0)
        } else if c[2] == 1.0 && c[0] < 1.0 {
            (c[1] - 1.0) / (c[0] - 1.0)
        } else {
            0.0
        };
        return 1.0 + g * (t - 1.0);
    }
    let (mut start, mut end, mut mid) = (0.0_f64, 1.0_f64, 0.0_f64);
    while start < end {
        mid = f64::midpoint(start, end);
        let x_est = calc(c[0], c[2], mid);
        if (t - x_est).abs() < 0.00001 {
            return calc(c[1], c[3], mid);
        }
        if x_est < t {
            start = mid;
        } else {
            end = mid;
        }
    }
    calc(c[1], c[3], mid)
}

/// X's bespoke float→hex (integer part MSB-first, then a `.`, then fraction
/// digits until the binary64 value exhausts). Matches the reference exactly.
fn float_to_hex(mut x: f64) -> String {
    let mut result: Vec<char> = Vec::new();
    let mut quotient = x.trunc();
    let fraction = x - quotient;
    while quotient > 0.0 {
        quotient = (x / 16.0).trunc();
        let remainder = (x - quotient * 16.0).trunc();
        result.insert(0, hex_digit(remainder));
        x = quotient;
    }
    if fraction == 0.0 {
        return result.into_iter().collect();
    }
    result.push('.');
    let mut fraction = fraction;
    let mut guard = 0;
    while fraction > 0.0 && guard < 64 {
        fraction *= 16.0;
        let integer = fraction.trunc();
        fraction -= integer;
        result.push(hex_digit(integer));
        guard += 1;
    }
    result.into_iter().collect()
}

/// `n` is one hex digit's worth, 0..=15, already truncated by the caller.
fn hex_digit(n: f64) -> char {
    let n = trunc_u8(n);
    if n > 9 {
        char::from(n + 55)
    } else {
        char::from(b'0' + n)
    }
}

/// The one place a float becomes an integer. X's algorithm truncates toward
/// zero, which is exactly what `as` does; std has no checked f64→int conversion,
/// so the lint is answered here once instead of at every call site.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "callers pass an already-truncated value in 0..=255"
)]
fn trunc_u8(x: f64) -> u8 {
    x as u8
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(EPOCH_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Golden values from the reference Python implementation on a live X page.
    #[test]
    fn animate_matches_reference() {
        let frame_row = [
            175.0, 253.0, 100.0, 27.0, 64.0, 218.0, 129.0, 23.0, 40.0, 7.0, 216.0,
        ];
        assert_eq!(
            animate(&frame_row, 0.002_441_406_25),
            "b2ff621011eb851eb851ec011eb851eb851ec100"
        );
    }

    #[test]
    fn generate_txid_matches_reference() {
        let key_bytes = base64::engine::general_purpose::STANDARD
            .decode("fu3E633Gl7JL0wlc3sT2NhiZNMCJUHWDs4R0gQeZvytYK35YflaBlP+vmpV3QPe8")
            .unwrap();
        let txid = generate_txid(
            &key_bytes,
            "b2ff621011eb851eb851ec011eb851eb851ec100",
            "POST",
            "/i/api/graphql/x/CreateTweet",
            1_000_000,
            42,
        );
        assert_eq!(
            txid,
            "KlTH7sFX7L2YYfkjdvTu3Bwysx7qo3pfqZmuXqsts5UBcgFUclR8q77VhbC/XWrdlmpoJSrzgIsz06eKA45qGXZd7JTLKQ"
        );
    }
}
