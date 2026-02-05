use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position, Uri};

use crate::doc::position::position_to_offset;
use crate::doc::store::DocumentStore;

pub fn hover(docs: &DocumentStore, uri: &Uri, position: Position) -> Option<Hover> {
    let doc = docs.get(uri)?;
    let offset = position_to_offset(&doc.text, position)?;
    let ident = extract_ident_at(&doc.text, offset)?;
    let snippet = find_definition(docs, &ident)?;

    let contents = HoverContents::Markup(MarkupContent {
        kind: MarkupKind::Markdown,
        value: format!("```rust\n{}\n```", snippet),
    });

    Some(Hover {
        contents,
        range: None,
    })
}

fn extract_ident_at(text: &str, offset: usize) -> Option<String> {
    if text.is_empty() {
        return None;
    }

    let bytes = text.as_bytes();
    if offset > bytes.len() {
        return None;
    }

    let mut start = offset;
    while start > 0 {
        let b = bytes[start - 1];
        if is_ident_char(b) {
            start -= 1;
        } else {
            break;
        }
    }

    let mut end = offset;
    while end < bytes.len() {
        let b = bytes[end];
        if is_ident_char(b) {
            end += 1;
        } else {
            break;
        }
    }

    if start == end {
        return None;
    }

    std::str::from_utf8(&bytes[start..end])
        .ok()
        .map(|s| s.to_string())
}

fn is_ident_char(b: u8) -> bool {
    b == b'_' || (b as char).is_ascii_alphanumeric()
}

fn find_definition(docs: &DocumentStore, ident: &str) -> Option<String> {
    const KEYWORDS: [&str; 8] = [
        "fn", "struct", "enum", "type", "const", "mod", "trait", "impl",
    ];

    for (_uri, doc) in docs.iter() {
        for line in doc.text.lines() {
            let mut trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with("/*") {
                continue;
            }

            trimmed = strip_pub_prefix(trimmed);

            for keyword in &KEYWORDS {
                if let Some(rest) = trimmed.strip_prefix(keyword) {
                    let is_space = rest
                        .chars()
                        .next()
                        .map(|c| c.is_whitespace())
                        .unwrap_or(false);
                    if !is_space {
                        continue;
                    }
                    let rest = rest.trim_start();
                    let name = take_ident(rest);
                    if let Some(name) = name {
                        if name == ident {
                            return Some(line.trim().to_string());
                        }
                    }
                }
            }
        }
    }

    None
}

fn strip_pub_prefix(line: &str) -> &str {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix("pub") {
        let rest = rest.trim_start();
        if rest.starts_with('(') {
            if let Some(idx) = rest.find(')') {
                return rest[idx + 1..].trim_start();
            }
            return rest;
        }
        if rest
            .chars()
            .next()
            .map(|c| c.is_whitespace())
            .unwrap_or(false)
        {
            return rest.trim_start();
        }
        return trimmed;
    }

    trimmed
}

fn take_ident(s: &str) -> Option<String> {
    let mut chars = s.char_indices();
    let Some((idx, first)) = chars.next() else {
        return None;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return None;
    }
    let mut end = idx + first.len_utf8();
    for (idx, ch) in chars {
        if ch == '_' || ch.is_ascii_alphanumeric() {
            end = idx + ch.len_utf8();
        } else {
            break;
        }
    }

    if end == 0 {
        None
    } else {
        Some(s[..end].to_string())
    }
}
