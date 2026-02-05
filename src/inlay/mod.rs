use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, Position, Range, Uri};

use crate::doc::position::offset_to_position;
use crate::doc::store::DocumentStore;
use crate::doc::uri::uri_to_path;

pub fn inlay_hints(
    docs: &DocumentStore,
    root: Option<&Path>,
    uri: &Uri,
    range: Range,
) -> Vec<InlayHint> {
    let doc = match docs.get(uri) {
        Some(doc) => doc,
        None => return Vec::new(),
    };

    let index = WorkspaceIndex::build(docs, root);
    let mut hints = Vec::new();
    hints.extend(local_var_type_hints(&doc.text, &index));
    hints.extend(arg_name_hints(&doc.text, &index));
    hints.extend(const_generic_hints(&doc.text, &index));
    hints.extend(chained_expr_type_hints(&doc.text, &index));

    hints.retain(|hint| position_in_range(hint.position, range));
    hints.sort_by(|a, b| position_cmp(a.position, b.position));
    hints
}

#[derive(Debug, Default)]
struct WorkspaceIndex {
    fn_defs: HashMap<String, Vec<FunctionSig>>,
    method_defs: HashMap<String, Vec<FunctionSig>>,
    generics: HashMap<String, Vec<Vec<GenericParam>>>,
    type_names: HashMap<String, usize>,
}

impl WorkspaceIndex {
    fn build(docs: &DocumentStore, root: Option<&Path>) -> Self {
        let mut index = WorkspaceIndex::default();
        let mut open_paths = HashSet::new();

        for (uri, doc) in docs.iter() {
            index.add_source(&doc.text);
            if let Some(path) = uri_to_path(uri) {
                open_paths.insert(path);
            }
        }

        if let Some(root) = root {
            index.add_workspace(root, &open_paths);
        }

        index
    }

    fn add_workspace(&mut self, root: &Path, open_paths: &HashSet<PathBuf>) {
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let entries = match fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if should_skip_dir(&path) {
                        continue;
                    }
                    stack.push(path);
                } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                    if open_paths.contains(&path) {
                        continue;
                    }
                    if let Ok(text) = fs::read_to_string(&path) {
                        self.add_source(&text);
                    }
                }
            }
        }
    }

    fn add_source(&mut self, text: &str) {
        let tokens = lex(text);
        self.collect_defs(text, &tokens);
    }

    fn collect_defs(&mut self, text: &str, tokens: &[Token]) {
        let mut i = 0;
        while i < tokens.len() {
            if tokens[i].is_ident("fn") {
                if let Some((name, sig, next_i)) = parse_fn_def(text, tokens, i) {
                    self.add_fn(&name, sig.clone());
                    self.add_generics(&name, sig.generics.clone());
                    if sig.has_self {
                        let params = sig.params.iter().skip(1).cloned().collect::<Vec<_>>();
                        let method_sig = FunctionSig {
                            params,
                            return_type: sig.return_type.clone(),
                            generics: sig.generics.clone(),
                            has_self: false,
                        };
                        self.add_method(&name, method_sig);
                    }
                    i = next_i;
                    continue;
                }
            } else if tokens[i].is_ident("struct")
                || tokens[i].is_ident("enum")
                || tokens[i].is_ident("trait")
                || tokens[i].is_ident("type")
            {
                if let Some((name, generics, next_i)) = parse_type_def(tokens, i) {
                    self.add_generics(&name, generics);
                    *self.type_names.entry(name).or_insert(0) += 1;
                    i = next_i;
                    continue;
                }
            }
            i += 1;
        }
    }

    fn add_fn(&mut self, name: &str, sig: FunctionSig) {
        self.fn_defs.entry(name.to_string()).or_default().push(sig);
    }

    fn add_method(&mut self, name: &str, sig: FunctionSig) {
        self.method_defs
            .entry(name.to_string())
            .or_default()
            .push(sig);
    }

    fn add_generics(&mut self, name: &str, generics: Vec<GenericParam>) {
        if generics.is_empty() {
            return;
        }
        self.generics
            .entry(name.to_string())
            .or_default()
            .push(generics);
    }

    fn unique_fn(&self, name: &str) -> Option<&FunctionSig> {
        self.fn_defs.get(name).and_then(|items| {
            if items.len() == 1 {
                Some(&items[0])
            } else {
                None
            }
        })
    }

    fn unique_method(&self, name: &str) -> Option<&FunctionSig> {
        self.method_defs.get(name).and_then(|items| {
            if items.len() == 1 {
                Some(&items[0])
            } else {
                None
            }
        })
    }

    fn unique_generics(&self, name: &str) -> Option<&[GenericParam]> {
        self.generics.get(name).and_then(|items| {
            if items.len() == 1 {
                Some(items[0].as_slice())
            } else {
                None
            }
        })
    }

    fn is_unique_type(&self, name: &str) -> bool {
        self.type_names.get(name).copied().unwrap_or(0) == 1
    }
}

#[derive(Debug, Clone)]
struct FunctionSig {
    params: Vec<String>,
    return_type: Option<String>,
    generics: Vec<GenericParam>,
    has_self: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GenericParamKind {
    Const,
    Type,
    Lifetime,
}

#[derive(Debug, Clone)]
struct GenericParam {
    name: String,
    kind: GenericParamKind,
}

#[derive(Debug, Clone)]
struct Token {
    kind: TokenKind,
    start: usize,
    end: usize,
}

#[derive(Debug, Clone)]
enum TokenKind {
    Ident(String),
    Lifetime(String),
    Number,
    Punct(char),
    DoubleColon,
    Arrow,
}

impl Token {
    fn is_ident(&self, value: &str) -> bool {
        matches!(&self.kind, TokenKind::Ident(name) if name == value)
    }

    fn ident(&self) -> Option<&str> {
        match &self.kind {
            TokenKind::Ident(name) => Some(name.as_str()),
            _ => None,
        }
    }

    fn is_punct(&self, ch: char) -> bool {
        matches!(self.kind, TokenKind::Punct(value) if value == ch)
    }
}

fn lex(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0usize;

    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        if b == b'/' && i + 1 < bytes.len() {
            if bytes[i + 1] == b'/' {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            if bytes[i + 1] == b'*' {
                i += 2;
                while i + 1 < bytes.len() {
                    if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
                continue;
            }
        }

        if let Some(next) = skip_string_literal(bytes, i) {
            i = next;
            continue;
        }

        if b == b'\'' {
            let (token, next) = lex_lifetime_or_char(text, bytes, i);
            if let Some(token) = token {
                tokens.push(token);
            }
            i = next;
            continue;
        }

        if is_ident_start(b) {
            let start = i;
            i += 1;
            while i < bytes.len() && is_ident_continue(bytes[i]) {
                i += 1;
            }
            let ident = &text[start..i];
            tokens.push(Token {
                kind: TokenKind::Ident(ident.to_string()),
                start,
                end: i,
            });
            continue;
        }

        if b.is_ascii_digit() {
            let start = i;
            i += 1;
            while i < bytes.len() {
                let ch = bytes[i];
                if ch.is_ascii_alphanumeric() || ch == b'_' || ch == b'.' {
                    i += 1;
                } else {
                    break;
                }
            }
            tokens.push(Token {
                kind: TokenKind::Number,
                start,
                end: i,
            });
            continue;
        }

        if b == b':' && i + 1 < bytes.len() && bytes[i + 1] == b':' {
            tokens.push(Token {
                kind: TokenKind::DoubleColon,
                start: i,
                end: i + 2,
            });
            i += 2;
            continue;
        }

        if b == b'-' && i + 1 < bytes.len() && bytes[i + 1] == b'>' {
            tokens.push(Token {
                kind: TokenKind::Arrow,
                start: i,
                end: i + 2,
            });
            i += 2;
            continue;
        }

        tokens.push(Token {
            kind: TokenKind::Punct(b as char),
            start: i,
            end: i + 1,
        });
        i += 1;
    }

    tokens
}

fn skip_string_literal(bytes: &[u8], idx: usize) -> Option<usize> {
    let len = bytes.len();
    if idx >= len {
        return None;
    }

    if bytes[idx] == b'"' {
        return Some(skip_normal_string(bytes, idx + 1));
    }

    if bytes[idx] == b'b' {
        if idx + 1 < len && bytes[idx + 1] == b'"' {
            return Some(skip_normal_string(bytes, idx + 2));
        }
        if idx + 1 < len && bytes[idx + 1] == b'r' {
            if let Some(next) = skip_raw_string(bytes, idx + 2) {
                return Some(next);
            }
        }
    }

    if bytes[idx] == b'r' {
        if let Some(next) = skip_raw_string(bytes, idx + 1) {
            return Some(next);
        }
    }

    None
}

fn skip_normal_string(bytes: &[u8], mut idx: usize) -> usize {
    while idx < bytes.len() {
        if bytes[idx] == b'\\' {
            idx = idx.saturating_add(2);
            continue;
        }
        if bytes[idx] == b'"' {
            return idx + 1;
        }
        idx += 1;
    }
    bytes.len()
}

fn skip_raw_string(bytes: &[u8], mut idx: usize) -> Option<usize> {
    let len = bytes.len();
    let mut hashes = 0usize;
    while idx < len && bytes[idx] == b'#' {
        hashes += 1;
        idx += 1;
    }
    if idx >= len || bytes[idx] != b'"' {
        return None;
    }
    idx += 1;

    while idx < len {
        if bytes[idx] == b'"' {
            let mut j = idx + 1;
            let mut matched = 0usize;
            while matched < hashes && j < len && bytes[j] == b'#' {
                matched += 1;
                j += 1;
            }
            if matched == hashes {
                return Some(j);
            }
        }
        idx += 1;
    }

    Some(len)
}

fn lex_lifetime_or_char(text: &str, bytes: &[u8], idx: usize) -> (Option<Token>, usize) {
    let len = bytes.len();
    if idx + 1 >= len {
        return (None, idx + 1);
    }
    let next = bytes[idx + 1];
    if is_ident_start(next) {
        let mut j = idx + 1;
        while j < len && is_ident_continue(bytes[j]) {
            j += 1;
        }
        if j < len && bytes[j] == b'\'' {
            return (None, j + 1);
        }
        let name = &text[idx + 1..j];
        let token = Token {
            kind: TokenKind::Lifetime(name.to_string()),
            start: idx,
            end: j,
        };
        return (Some(token), j);
    }

    let mut j = idx + 1;
    while j < len {
        if bytes[j] == b'\\' {
            j = j.saturating_add(2);
            continue;
        }
        if bytes[j] == b'\'' {
            return (None, j + 1);
        }
        j += 1;
    }

    (None, len)
}

fn is_ident_start(b: u8) -> bool {
    b == b'_' || (b as char).is_ascii_alphabetic()
}

fn is_ident_continue(b: u8) -> bool {
    b == b'_' || (b as char).is_ascii_alphanumeric()
}

fn parse_fn_def(text: &str, tokens: &[Token], idx: usize) -> Option<(String, FunctionSig, usize)> {
    let mut i = idx + 1;
    if i >= tokens.len() {
        return None;
    }
    let name = tokens[i].ident()?.to_string();
    i += 1;

    let mut generics = Vec::new();
    if i < tokens.len() && tokens[i].is_punct('<') {
        if let Some((parsed, next_i)) = parse_generics(tokens, i) {
            generics = parsed;
            i = next_i;
        }
    }

    if i >= tokens.len() || !tokens[i].is_punct('(') {
        return None;
    }

    let close_idx = find_matching_paren(tokens, i)?;
    let params = parse_params(tokens, i + 1, close_idx);
    let has_self = params.first().map(|name| name == "self").unwrap_or(false);

    let return_type = parse_return_type(text, tokens, close_idx + 1);

    let sig = FunctionSig {
        params,
        return_type,
        generics,
        has_self,
    };

    Some((name, sig, close_idx + 1))
}

fn parse_type_def(tokens: &[Token], idx: usize) -> Option<(String, Vec<GenericParam>, usize)> {
    let mut i = idx + 1;
    if i >= tokens.len() {
        return None;
    }
    let name = tokens[i].ident()?.to_string();
    i += 1;

    let mut generics = Vec::new();
    if i < tokens.len() && tokens[i].is_punct('<') {
        if let Some((parsed, next_i)) = parse_generics(tokens, i) {
            generics = parsed;
            i = next_i;
        }
    }

    Some((name, generics, i))
}

fn parse_generics(tokens: &[Token], idx: usize) -> Option<(Vec<GenericParam>, usize)> {
    if !tokens[idx].is_punct('<') {
        return None;
    }
    let end_idx = find_matching_angle(tokens, idx)?;
    let params = parse_generic_params(tokens, idx + 1, end_idx);
    Some((params, end_idx + 1))
}

fn parse_generic_params(tokens: &[Token], start: usize, end: usize) -> Vec<GenericParam> {
    let mut params = Vec::new();
    let mut current = Vec::new();
    let mut angle_depth = 0i32;
    let mut paren_depth = 0i32;
    let mut bracket_depth = 0i32;
    let mut brace_depth = 0i32;

    for idx in start..end {
        let tok = &tokens[idx];
        match tok.kind {
            TokenKind::Punct('<') => {
                angle_depth += 1;
                current.push(tok.clone());
            }
            TokenKind::Punct('>') => {
                if angle_depth > 0 {
                    angle_depth -= 1;
                }
                current.push(tok.clone());
            }
            TokenKind::Punct('(') => {
                paren_depth += 1;
                current.push(tok.clone());
            }
            TokenKind::Punct(')') => {
                if paren_depth > 0 {
                    paren_depth -= 1;
                }
                current.push(tok.clone());
            }
            TokenKind::Punct('[') => {
                bracket_depth += 1;
                current.push(tok.clone());
            }
            TokenKind::Punct(']') => {
                if bracket_depth > 0 {
                    bracket_depth -= 1;
                }
                current.push(tok.clone());
            }
            TokenKind::Punct('{') => {
                brace_depth += 1;
                current.push(tok.clone());
            }
            TokenKind::Punct('}') => {
                if brace_depth > 0 {
                    brace_depth -= 1;
                }
                current.push(tok.clone());
            }
            TokenKind::Punct(',')
                if angle_depth == 0
                    && paren_depth == 0
                    && bracket_depth == 0
                    && brace_depth == 0 =>
            {
                if let Some(param) = parse_generic_param(&current) {
                    params.push(param);
                }
                current.clear();
            }
            _ => current.push(tok.clone()),
        }
    }

    if !current.is_empty() {
        if let Some(param) = parse_generic_param(&current) {
            params.push(param);
        }
    }

    params
}

fn parse_generic_param(tokens: &[Token]) -> Option<GenericParam> {
    let mut iter = tokens.iter();
    while let Some(tok) = iter.next() {
        match &tok.kind {
            TokenKind::Lifetime(name) => {
                return Some(GenericParam {
                    name: name.clone(),
                    kind: GenericParamKind::Lifetime,
                });
            }
            TokenKind::Ident(name) if name == "const" => {
                for tok in iter {
                    if let TokenKind::Ident(param) = &tok.kind {
                        return Some(GenericParam {
                            name: param.clone(),
                            kind: GenericParamKind::Const,
                        });
                    }
                }
                return None;
            }
            TokenKind::Ident(name) => {
                return Some(GenericParam {
                    name: name.clone(),
                    kind: GenericParamKind::Type,
                });
            }
            _ => {}
        }
    }
    None
}

fn parse_params(tokens: &[Token], start: usize, end: usize) -> Vec<String> {
    let mut params = Vec::new();
    let mut current = Vec::new();
    let mut paren_depth = 0i32;
    let mut bracket_depth = 0i32;
    let mut brace_depth = 0i32;

    for idx in start..end {
        let tok = &tokens[idx];
        match tok.kind {
            TokenKind::Punct('(') => {
                paren_depth += 1;
                current.push(tok.clone());
            }
            TokenKind::Punct(')') => {
                if paren_depth > 0 {
                    paren_depth -= 1;
                }
                current.push(tok.clone());
            }
            TokenKind::Punct('[') => {
                bracket_depth += 1;
                current.push(tok.clone());
            }
            TokenKind::Punct(']') => {
                if bracket_depth > 0 {
                    bracket_depth -= 1;
                }
                current.push(tok.clone());
            }
            TokenKind::Punct('{') => {
                brace_depth += 1;
                current.push(tok.clone());
            }
            TokenKind::Punct('}') => {
                if brace_depth > 0 {
                    brace_depth -= 1;
                }
                current.push(tok.clone());
            }
            TokenKind::Punct(',') if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 => {
                if let Some(name) = parse_param_name(&current) {
                    params.push(name);
                }
                current.clear();
            }
            _ => current.push(tok.clone()),
        }
    }

    if !current.is_empty() {
        if let Some(name) = parse_param_name(&current) {
            params.push(name);
        }
    }

    params
}

fn parse_param_name(tokens: &[Token]) -> Option<String> {
    for tok in tokens {
        match &tok.kind {
            TokenKind::Ident(name) if name == "mut" || name == "ref" || name == "const" => {
                continue;
            }
            TokenKind::Ident(name) => return Some(name.clone()),
            TokenKind::Lifetime(_) => continue,
            _ => continue,
        }
    }
    None
}

fn parse_return_type(text: &str, tokens: &[Token], start: usize) -> Option<String> {
    if start >= tokens.len() {
        return None;
    }
    if !matches!(tokens[start].kind, TokenKind::Arrow) {
        return None;
    }

    let arrow_end = tokens[start].end;
    let mut i = start + 1;
    let mut angle_depth = 0i32;
    let mut paren_depth = 0i32;
    let mut bracket_depth = 0i32;
    let mut end_offset = text.len();

    while i < tokens.len() {
        let tok = &tokens[i];
        match tok.kind {
            TokenKind::Punct('{') if angle_depth == 0 && paren_depth == 0 && bracket_depth == 0 => {
                end_offset = tok.start;
                break;
            }
            TokenKind::Punct(';') if angle_depth == 0 && paren_depth == 0 && bracket_depth == 0 => {
                end_offset = tok.start;
                break;
            }
            TokenKind::Ident(ref name)
                if name == "where"
                    && angle_depth == 0
                    && paren_depth == 0
                    && bracket_depth == 0 =>
            {
                end_offset = tok.start;
                break;
            }
            TokenKind::Punct('<') => angle_depth += 1,
            TokenKind::Punct('>') => {
                if angle_depth > 0 {
                    angle_depth -= 1;
                }
            }
            TokenKind::Punct('(') => paren_depth += 1,
            TokenKind::Punct(')') => {
                if paren_depth > 0 {
                    paren_depth -= 1;
                }
            }
            TokenKind::Punct('[') => bracket_depth += 1,
            TokenKind::Punct(']') => {
                if bracket_depth > 0 {
                    bracket_depth -= 1;
                }
            }
            _ => {}
        }
        i += 1;
    }

    let slice = text[arrow_end..end_offset].trim();
    if slice.is_empty() {
        None
    } else {
        Some(slice.to_string())
    }
}

fn find_matching_paren(tokens: &[Token], idx: usize) -> Option<usize> {
    let mut depth = 0i32;
    for i in idx..tokens.len() {
        match tokens[i].kind {
            TokenKind::Punct('(') => depth += 1,
            TokenKind::Punct(')') => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn find_matching_angle(tokens: &[Token], idx: usize) -> Option<usize> {
    let mut depth = 0i32;
    for i in idx..tokens.len() {
        match tokens[i].kind {
            TokenKind::Punct('<') => depth += 1,
            TokenKind::Punct('>') => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn find_matching_angle_backward(tokens: &[Token], idx: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = idx;
    loop {
        match tokens[i].kind {
            TokenKind::Punct('>') => depth += 1,
            TokenKind::Punct('<') => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        if i == 0 {
            break;
        }
        i -= 1;
    }
    None
}

fn local_var_type_hints(text: &str, index: &WorkspaceIndex) -> Vec<InlayHint> {
    let tokens = lex(text);
    let mut hints = Vec::new();

    let mut i = 0usize;
    while i < tokens.len() {
        if tokens[i].is_ident("let") {
            if i > 0 {
                if let Some(prev) = tokens[i - 1].ident() {
                    if matches!(prev, "if" | "while" | "match" | "for") {
                        i += 1;
                        continue;
                    }
                }
            }

            let mut j = i + 1;
            if j < tokens.len() && tokens[j].is_ident("mut") {
                j += 1;
            }
            let var_token = match tokens.get(j) {
                Some(tok) => tok,
                None => {
                    i += 1;
                    continue;
                }
            };
            let var_name = match var_token.ident() {
                Some(name) => name,
                None => {
                    i += 1;
                    continue;
                }
            };
            if var_name == "_" {
                i += 1;
                continue;
            }
            let var_end = var_token.end;
            j += 1;

            let mut has_type = false;
            let mut eq_idx = None;
            let mut depth = 0i32;
            while j < tokens.len() {
                let tok = &tokens[j];
                match tok.kind {
                    TokenKind::Punct('(') | TokenKind::Punct('[') | TokenKind::Punct('{') => {
                        depth += 1
                    }
                    TokenKind::Punct(')') | TokenKind::Punct(']') | TokenKind::Punct('}') => {
                        if depth > 0 {
                            depth -= 1;
                        }
                    }
                    TokenKind::Punct(':') if depth == 0 => has_type = true,
                    TokenKind::Punct('=') if depth == 0 => {
                        eq_idx = Some(j);
                        break;
                    }
                    TokenKind::Punct(';') if depth == 0 => break,
                    _ => {}
                }
                j += 1;
            }

            if has_type {
                i += 1;
                continue;
            }

            let Some(eq_idx) = eq_idx else {
                i += 1;
                continue;
            };

            let mut k = eq_idx + 1;
            let mut depth = 0i32;
            let mut end_offset = text.len();
            while k < tokens.len() {
                let tok = &tokens[k];
                match tok.kind {
                    TokenKind::Punct('(') | TokenKind::Punct('[') | TokenKind::Punct('{') => {
                        depth += 1
                    }
                    TokenKind::Punct(')') | TokenKind::Punct(']') | TokenKind::Punct('}') => {
                        if depth > 0 {
                            depth -= 1;
                        }
                    }
                    TokenKind::Punct(';') if depth == 0 => {
                        end_offset = tok.start;
                        break;
                    }
                    _ => {}
                }
                k += 1;
            }

            let expr = text[tokens[eq_idx].end..end_offset].trim();
            if let Some(ty) = infer_type(expr, index) {
                if let Some(position) = offset_to_position(text, var_end) {
                    hints.push(type_hint(position, &ty));
                }
            }
        }
        i += 1;
    }

    hints
}

fn infer_type(expr: &str, index: &WorkspaceIndex) -> Option<String> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed == "true" || trimmed == "false" {
        return Some("bool".to_string());
    }

    if is_char_literal(trimmed) {
        return Some("char".to_string());
    }

    if let Some(lit) = infer_string_literal(trimmed) {
        return Some(lit);
    }

    if let Some(num) = infer_number_literal(trimmed) {
        return Some(num);
    }

    if let Some(ty) = infer_struct_literal(trimmed, index) {
        return Some(ty);
    }

    infer_from_call(trimmed, index)
}

fn infer_string_literal(text: &str) -> Option<String> {
    if text.starts_with("b\"") || text.starts_with("br\"") || text.starts_with("br#") {
        return Some("&[u8]".to_string());
    }
    if text.starts_with('"') || text.starts_with("r\"") || text.starts_with("r#") {
        return Some("&str".to_string());
    }
    None
}

fn is_char_literal(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 2 && bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''
}

fn infer_number_literal(text: &str) -> Option<String> {
    let mut s = text.trim();
    if s.starts_with('-') {
        s = &s[1..];
    }
    if s.is_empty() {
        return None;
    }

    let bytes = s.as_bytes();
    let mut i = 0usize;
    let mut has_digit = false;
    let mut has_dot = false;
    let mut has_exp = false;

    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_digit() || b == b'_' {
            has_digit = true;
            i += 1;
            continue;
        }
        if b == b'.' && !has_dot && !has_exp {
            has_dot = true;
            i += 1;
            continue;
        }
        if (b == b'e' || b == b'E') && has_digit && !has_exp {
            has_exp = true;
            i += 1;
            if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
                i += 1;
            }
            continue;
        }
        break;
    }

    if !has_digit {
        return None;
    }

    let suffix = s[i..].trim();
    if !suffix.is_empty() {
        match suffix {
            "u8" | "u16" | "u32" | "u64" | "u128" | "usize" => return Some(suffix.to_string()),
            "i8" | "i16" | "i32" | "i64" | "i128" | "isize" => return Some(suffix.to_string()),
            "f32" | "f64" => return Some(suffix.to_string()),
            _ => {}
        }
    }

    if has_dot || has_exp {
        Some("f64".to_string())
    } else {
        Some("i32".to_string())
    }
}

fn infer_struct_literal(expr: &str, index: &WorkspaceIndex) -> Option<String> {
    let tokens = lex(expr);
    let mut i = 0usize;
    let mut name = None;

    while i < tokens.len() {
        if let Some(ident) = tokens[i].ident() {
            name = Some(ident.to_string());
            i += 1;
            while i + 1 < tokens.len() && matches!(tokens[i].kind, TokenKind::DoubleColon) {
                if let Some(next) = tokens[i + 1].ident() {
                    name = Some(next.to_string());
                    i += 2;
                } else {
                    break;
                }
            }
            break;
        } else {
            break;
        }
    }

    let name = name?;
    let next = tokens.get(i)?;
    match next.kind {
        TokenKind::Punct('{') | TokenKind::Punct('(') => {
            if index.is_unique_type(&name) {
                return Some(name);
            }
        }
        _ => {}
    }

    None
}

fn infer_from_call(expr: &str, index: &WorkspaceIndex) -> Option<String> {
    let calls = collect_calls(expr);
    let call = calls.last()?;
    match call.kind {
        CallKind::Method => index
            .unique_method(&call.name)
            .and_then(|sig| sig.return_type.clone()),
        CallKind::Function => {
            if let Some(sig) = index.unique_fn(&call.name) {
                if let Some(ret) = sig.return_type.clone() {
                    return Some(ret);
                }
            }
            if index.is_unique_type(&call.name) {
                return Some(call.name.clone());
            }
            None
        }
    }
}

fn arg_name_hints(text: &str, index: &WorkspaceIndex) -> Vec<InlayHint> {
    let calls = collect_calls(text);
    let mut hints = Vec::new();

    for call in calls {
        let sig = match call.kind {
            CallKind::Function => index.unique_fn(&call.name),
            CallKind::Method => index.unique_method(&call.name),
        };
        let Some(sig) = sig else { continue };

        let count = sig.params.len().min(call.arg_starts.len());
        for idx in 0..count {
            if sig.params[idx].is_empty() || sig.params[idx] == "_" {
                continue;
            }
            if let Some(position) = offset_to_position(text, call.arg_starts[idx]) {
                hints.push(param_hint(position, &sig.params[idx]));
            }
        }
    }

    hints
}

fn const_generic_hints(text: &str, index: &WorkspaceIndex) -> Vec<InlayHint> {
    let tokens = lex(text);
    let mut hints = Vec::new();

    let mut i = 0usize;
    while i < tokens.len() {
        if tokens[i].is_punct('<') {
            if let Some((name, end_idx)) = detect_generic_arg_list(&tokens, i) {
                let args = parse_generic_arg_starts(&tokens, i + 1, end_idx);
                if let Some(generics) = index.unique_generics(&name) {
                    let limit = generics.len().min(args.len());
                    for idx in 0..limit {
                        if generics[idx].kind == GenericParamKind::Const {
                            if let Some(position) = offset_to_position(text, args[idx]) {
                                hints.push(param_hint(position, &generics[idx].name));
                            }
                        }
                    }
                }
                i = end_idx;
            }
        }
        i += 1;
    }

    hints
}

fn detect_generic_arg_list(tokens: &[Token], idx: usize) -> Option<(String, usize)> {
    if idx == 0 {
        return None;
    }
    let mut name_idx = idx - 1;
    if matches!(tokens[name_idx].kind, TokenKind::DoubleColon) {
        if name_idx == 0 {
            return None;
        }
        name_idx -= 1;
    }

    let name = tokens[name_idx].ident()?.to_string();
    if is_keyword(&name) {
        return None;
    }

    if name_idx > 0 {
        if let Some(prev) = tokens[name_idx - 1].ident() {
            if matches!(prev, "struct" | "enum" | "trait" | "type" | "fn") {
                return None;
            }
        }
    }

    let end_idx = find_matching_angle(tokens, idx)?;
    if end_idx <= idx + 1 {
        return None;
    }
    if !generic_follows(tokens, end_idx) {
        return None;
    }

    Some((name, end_idx))
}

fn generic_follows(tokens: &[Token], end_idx: usize) -> bool {
    if end_idx + 1 >= tokens.len() {
        return true;
    }
    matches!(
        tokens[end_idx + 1].kind,
        TokenKind::Punct('(')
            | TokenKind::Punct('{')
            | TokenKind::Punct(')')
            | TokenKind::Punct(',')
            | TokenKind::Punct(';')
            | TokenKind::Punct(':')
            | TokenKind::Punct('.')
            | TokenKind::Punct(']')
            | TokenKind::Punct('>')
            | TokenKind::Punct('=')
            | TokenKind::DoubleColon
    )
}

fn parse_generic_arg_starts(tokens: &[Token], start: usize, end: usize) -> Vec<usize> {
    let mut args = Vec::new();
    let mut arg_start = None;
    let mut paren_depth = 0i32;
    let mut bracket_depth = 0i32;
    let mut brace_depth = 0i32;
    let mut angle_depth = 0i32;

    for idx in start..end {
        let tok = &tokens[idx];
        match tok.kind {
            TokenKind::Punct('(') => paren_depth += 1,
            TokenKind::Punct(')') => {
                if paren_depth > 0 {
                    paren_depth -= 1;
                }
            }
            TokenKind::Punct('[') => bracket_depth += 1,
            TokenKind::Punct(']') => {
                if bracket_depth > 0 {
                    bracket_depth -= 1;
                }
            }
            TokenKind::Punct('{') => brace_depth += 1,
            TokenKind::Punct('}') => {
                if brace_depth > 0 {
                    brace_depth -= 1;
                }
            }
            TokenKind::Punct('<') => angle_depth += 1,
            TokenKind::Punct('>') => {
                if angle_depth > 0 {
                    angle_depth -= 1;
                }
            }
            TokenKind::Punct(',')
                if paren_depth == 0
                    && bracket_depth == 0
                    && brace_depth == 0
                    && angle_depth == 0 =>
            {
                if let Some(start) = arg_start.take() {
                    args.push(start);
                }
                continue;
            }
            _ => {}
        }

        if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 && angle_depth == 0 {
            if arg_start.is_none() {
                arg_start = Some(tok.start);
            }
        }
    }

    if let Some(start) = arg_start {
        args.push(start);
    }

    args
}

fn chained_expr_type_hints(text: &str, index: &WorkspaceIndex) -> Vec<InlayHint> {
    let calls = collect_calls(text);
    let mut hints = Vec::new();

    for call in calls {
        let is_chain_segment = match call.kind {
            CallKind::Method => true,
            CallKind::Function => is_chained_call(text, call.close_paren),
        };
        if !is_chain_segment {
            continue;
        }
        let ty = match call.kind {
            CallKind::Method => index
                .unique_method(&call.name)
                .and_then(|sig| sig.return_type.clone()),
            CallKind::Function => index
                .unique_fn(&call.name)
                .and_then(|sig| sig.return_type.clone()),
        };
        let Some(ty) = ty else { continue };

        let offset = (call.close_paren + 1).min(text.len());
        if let Some(position) = offset_to_position(text, offset) {
            hints.push(type_hint(position, &ty));
        }
    }

    hints
}

#[derive(Debug, Clone)]
struct Call {
    name: String,
    kind: CallKind,
    arg_starts: Vec<usize>,
    close_paren: usize,
}

#[derive(Debug, Clone, Copy)]
enum CallKind {
    Function,
    Method,
}

fn collect_calls(text: &str) -> Vec<Call> {
    let tokens = lex(text);
    let mut calls = Vec::new();
    let mut i = 0usize;

    while i < tokens.len() {
        if tokens[i].is_punct('(') {
            if let Some((name, kind)) = detect_call_name(&tokens, i) {
                if let Some(close_idx) = find_matching_paren(&tokens, i) {
                    let args = parse_arg_starts(&tokens, i + 1, close_idx);
                    calls.push(Call {
                        name,
                        kind,
                        arg_starts: args,
                        close_paren: tokens[close_idx].start,
                    });
                    i = close_idx;
                    continue;
                }
            }
        }
        i += 1;
    }

    calls
}

fn detect_call_name(tokens: &[Token], idx: usize) -> Option<(String, CallKind)> {
    if idx == 0 {
        return None;
    }
    let mut j = idx - 1;

    if tokens[j].is_punct('>') {
        j = find_matching_angle_backward(tokens, j)?;
        if j == 0 {
            return None;
        }
        j -= 1;
    }

    if matches!(tokens[j].kind, TokenKind::DoubleColon) {
        if j == 0 {
            return None;
        }
        j -= 1;
    }

    let name = tokens[j].ident()?.to_string();
    if is_keyword(&name) {
        return None;
    }

    if j > 0 {
        if let Some(prev) = tokens[j - 1].ident() {
            if matches!(prev, "fn" | "struct" | "enum" | "trait" | "type" | "impl") {
                return None;
            }
        }
        if tokens[j - 1].is_punct('!') {
            return None;
        }
    }

    let kind = if j > 0 && tokens[j - 1].is_punct('.') {
        CallKind::Method
    } else {
        CallKind::Function
    };

    Some((name, kind))
}

fn parse_arg_starts(tokens: &[Token], start: usize, end: usize) -> Vec<usize> {
    let mut args = Vec::new();
    let mut arg_start = None;
    let mut paren_depth = 0i32;
    let mut bracket_depth = 0i32;
    let mut brace_depth = 0i32;
    let mut angle_depth = 0i32;

    for idx in start..end {
        let tok = &tokens[idx];
        match tok.kind {
            TokenKind::Punct('(') => paren_depth += 1,
            TokenKind::Punct(')') => {
                if paren_depth > 0 {
                    paren_depth -= 1;
                }
            }
            TokenKind::Punct('[') => bracket_depth += 1,
            TokenKind::Punct(']') => {
                if bracket_depth > 0 {
                    bracket_depth -= 1;
                }
            }
            TokenKind::Punct('{') => brace_depth += 1,
            TokenKind::Punct('}') => {
                if brace_depth > 0 {
                    brace_depth -= 1;
                }
            }
            TokenKind::Punct('<') => angle_depth += 1,
            TokenKind::Punct('>') => {
                if angle_depth > 0 {
                    angle_depth -= 1;
                }
            }
            TokenKind::Punct(',')
                if paren_depth == 0
                    && bracket_depth == 0
                    && brace_depth == 0
                    && angle_depth == 0 =>
            {
                if let Some(start) = arg_start.take() {
                    args.push(start);
                }
                continue;
            }
            _ => {}
        }

        if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 && angle_depth == 0 {
            if arg_start.is_none() {
                arg_start = Some(tok.start);
            }
        }
    }

    if let Some(start) = arg_start {
        args.push(start);
    }

    args
}

fn is_chained_call(text: &str, close_paren: usize) -> bool {
    let bytes = text.as_bytes();
    let mut i = close_paren.saturating_add(1);
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        return true;
    }
    if i < bytes.len() && bytes[i] == b'?' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'.' {
            return true;
        }
    }
    false
}

fn type_hint(position: Position, ty: &str) -> InlayHint {
    InlayHint {
        position,
        label: InlayHintLabel::String(format!(": {}", ty)),
        kind: Some(InlayHintKind::TYPE),
        text_edits: None,
        tooltip: None,
        padding_left: None,
        padding_right: None,
        data: None,
    }
}

fn param_hint(position: Position, name: &str) -> InlayHint {
    InlayHint {
        position,
        label: InlayHintLabel::String(format!("{}:", name)),
        kind: Some(InlayHintKind::PARAMETER),
        text_edits: None,
        tooltip: None,
        padding_left: None,
        padding_right: None,
        data: None,
    }
}

fn position_cmp(a: Position, b: Position) -> std::cmp::Ordering {
    match a.line.cmp(&b.line) {
        std::cmp::Ordering::Equal => a.character.cmp(&b.character),
        other => other,
    }
}

fn position_in_range(pos: Position, range: Range) -> bool {
    position_ge(pos, range.start) && position_le(pos, range.end)
}

fn position_ge(a: Position, b: Position) -> bool {
    a.line > b.line || (a.line == b.line && a.character >= b.character)
}

fn position_le(a: Position, b: Position) -> bool {
    a.line < b.line || (a.line == b.line && a.character <= b.character)
}

fn should_skip_dir(path: &Path) -> bool {
    match path.file_name().and_then(|s| s.to_str()) {
        Some("target") | Some(".git") => true,
        _ => false,
    }
}

fn is_keyword(name: &str) -> bool {
    matches!(
        name,
        "if" | "while"
            | "for"
            | "match"
            | "loop"
            | "return"
            | "fn"
            | "struct"
            | "enum"
            | "trait"
            | "type"
            | "impl"
            | "pub"
            | "use"
            | "const"
            | "static"
            | "async"
            | "await"
            | "move"
            | "unsafe"
            | "extern"
            | "crate"
            | "super"
            | "self"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_from_sources(sources: &[&str]) -> WorkspaceIndex {
        let mut index = WorkspaceIndex::default();
        for source in sources {
            index.add_source(source);
        }
        index
    }

    fn hint_labels(hints: &[InlayHint]) -> Vec<String> {
        hints
            .iter()
            .map(|hint| match &hint.label {
                InlayHintLabel::String(value) => value.clone(),
                _ => "".to_string(),
            })
            .collect()
    }

    #[test]
    fn fn_sig_parsing_basic() {
        let src = "fn foo<const N: usize, T>(a: i32, b: T) -> Option<T> { }";
        let index = index_from_sources(&[src]);
        let sig = index.unique_fn("foo").expect("fn signature");
        assert_eq!(sig.params, vec!["a", "b"]);
        assert_eq!(sig.return_type.as_deref(), Some("Option<T>"));
        let generics = index.unique_generics("foo").expect("generics");
        assert_eq!(generics[0].kind, GenericParamKind::Const);
        assert_eq!(generics[0].name, "N");
    }

    #[test]
    fn method_sig_parsing_skips_self() {
        let src = "impl Foo { fn method(&self, x: i32) {} }";
        let index = index_from_sources(&[src]);
        let sig = index.unique_method("method").expect("method sig");
        assert_eq!(sig.params, vec!["x"]);
    }

    #[test]
    fn local_var_type_literal() {
        let src = "fn main() { let x = 1; }";
        let index = index_from_sources(&[src]);
        let hints = local_var_type_hints(src, &index);
        let labels = hint_labels(&hints);
        assert!(labels.iter().any(|label| label == ": i32"));
    }

    #[test]
    fn local_var_type_struct_lit() {
        let src = "struct Foo { a: i32 } fn main() { let x = Foo { a: 1 }; }";
        let index = index_from_sources(&[src]);
        let hints = local_var_type_hints(src, &index);
        let labels = hint_labels(&hints);
        assert!(labels.iter().any(|label| label == ": Foo"));
    }

    #[test]
    fn arg_name_hints_simple_call() {
        let src = "fn foo(a: i32, b: i32) {} fn main() { foo(1, 2); }";
        let index = index_from_sources(&[src]);
        let hints = arg_name_hints(src, &index);
        let labels = hint_labels(&hints);
        assert!(labels.iter().any(|label| label == "a:"));
        assert!(labels.iter().any(|label| label == "b:"));
    }

    #[test]
    fn const_generic_hints_smoke() {
        let src = "fn foo<const N: usize, T>() {} fn main() { foo::<3, u8>(); }";
        let index = index_from_sources(&[src]);
        let hints = const_generic_hints(src, &index);
        let labels = hint_labels(&hints);
        assert!(labels.iter().any(|label| label == "N:"));
    }

    #[test]
    fn chained_call_type_hints() {
        let src = "struct Foo; struct Bar; impl Foo { fn bar(&self) -> Bar { Bar } } fn foo() -> Foo { Foo } fn main() { foo().bar(); }";
        let index = index_from_sources(&[src]);
        let hints = chained_expr_type_hints(src, &index);
        let labels = hint_labels(&hints);
        assert!(labels.iter().any(|label| label == ": Foo"));
        assert!(labels.iter().any(|label| label == ": Bar"));
    }
}
