use std::path::{Path, PathBuf};
use std::str::FromStr;

use lsp_types::Uri;

pub fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let raw = uri.as_str();
    let rest = raw.strip_prefix("file://")?;
    let (authority, path_part) = if rest.starts_with('/') {
        ("", rest.to_string())
    } else {
        let mut parts = rest.splitn(2, '/');
        let authority = parts.next().unwrap_or("");
        let path = parts
            .next()
            .map(|p| format!("/{}", p))
            .unwrap_or_else(|| "/".to_string());
        (authority, path)
    };

    let combined = if authority.is_empty() || authority.eq_ignore_ascii_case("localhost") {
        path_part
    } else {
        format!("//{}{}", authority, path_part)
    };

    let decoded = percent_decode(&combined)?;
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
    if !path.is_absolute() {
        return None;
    }
    let path_str = path.to_string_lossy();
    let mut normalized = path_str.replace('\\', "/");
    if cfg!(windows) {
        if normalized.starts_with("//") {
            let unc = &normalized[2..];
            let mut parts = unc.splitn(2, '/');
            let host = parts.next().unwrap_or("");
            let rest = parts.next().unwrap_or("");
            if host.is_empty() {
                return None;
            }
            let host_enc = percent_encode(host);
            let rest_enc = percent_encode(&format!("/{}", rest));
            let uri_str = format!("file://{}{}", host_enc, rest_enc);
            return Uri::from_str(&uri_str).ok();
        }

        if !normalized.starts_with('/') {
            normalized = format!("/{}", normalized);
        }
    }

    if !normalized.starts_with('/') {
        normalized = format!("/{}", normalized);
    }

    let encoded = percent_encode(&normalized);
    let encoded = encoded.strip_prefix('/').unwrap_or(&encoded);
    let uri_str = format!("file:///{}", encoded);
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

#[cfg(test)]
mod tests {
    use super::{path_to_uri, uri_to_path};
    use lsp_types::Uri;
    use std::path::Path;
    use std::str::FromStr;

    #[cfg(not(windows))]
    #[test]
    fn path_to_uri_posix() {
        let path = Path::new("/tmp/foo bar.rs");
        let uri = path_to_uri(path).unwrap();
        assert_eq!(uri.as_str(), "file:///tmp/foo%20bar.rs");
    }

    #[cfg(not(windows))]
    #[test]
    fn uri_to_path_posix() {
        let uri = Uri::from_str("file:///tmp/foo%20bar.rs").unwrap();
        let path = uri_to_path(&uri).unwrap();
        assert_eq!(path, Path::new("/tmp/foo bar.rs"));
    }

    #[cfg(not(windows))]
    #[test]
    fn uri_to_path_localhost() {
        let uri = Uri::from_str("file://localhost/tmp/foo.rs").unwrap();
        let path = uri_to_path(&uri).unwrap();
        assert_eq!(path, Path::new("/tmp/foo.rs"));
    }
}
