use std::path::{Path, PathBuf};
use std::str::FromStr;

use lsp_types::Uri;

pub fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let raw = uri.as_str();
    let rest = raw.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost/").unwrap_or(rest);
    let decoded = percent_decode(rest)?;
    let mut path = decoded;

    if cfg!(windows) {
        if path.starts_with('/') {
            let bytes = path.as_bytes();
            if bytes.len() > 2 && bytes[2] == b':' {
                path = path[1..].to_string();
            }
        }
    }

    Some(PathBuf::from(path))
}

pub fn path_to_uri(path: &Path) -> Option<Uri> {
    let path_str = path.to_string_lossy();
    let mut normalized = path_str.replace('\\', "/");
    if cfg!(windows) && !normalized.starts_with('/') {
        normalized = format!("/{}", normalized);
    }

    let encoded = percent_encode(&normalized);
    let uri_str = format!("file://{}", encoded);
    Uri::from_str(&uri_str).ok()
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if i + 2 >= bytes.len() {
                    return None;
                }
                let hi = from_hex(bytes[i + 1])?;
                let lo = from_hex(bytes[i + 2])?;
                out.push((hi << 4) | lo);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }

    String::from_utf8(out).ok()
}

fn percent_encode(input: &str) -> String {
    let mut out = String::new();
    for b in input.as_bytes() {
        let ch = *b as char;
        if is_unreserved(ch) || ch == '/' {
            out.push(ch);
        } else {
            out.push('%');
            out.push(to_hex((b >> 4) & 0x0f));
            out.push(to_hex(b & 0x0f));
        }
    }
    out
}

fn is_unreserved(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | '_' | '~')
}

fn to_hex(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'A' + (value - 10)) as char,
        _ => '0',
    }
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
