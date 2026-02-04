use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;

use lsp_types::{
    notification, request, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, Hover, HoverParams,
    InitializeParams, InitializeResult, InitializedParams, ServerCapabilities,
    TextDocumentSyncCapability, TextDocumentSyncKind, Uri, WorkspaceServerCapabilities,
    WorkspaceFoldersServerCapabilities,
};
use lsp_types::notification::Notification;
use lsp_types::request::Request;
use serde_json::{json, Value};

use crate::config::Config;
use crate::diagnostics::run_check;
use crate::doc::store::DocumentStore;
use crate::doc::uri::uri_to_path;
use crate::hover::hover as hover_at;

pub fn run() {
    let (tx, rx) = mpsc::channel::<String>();
    let writer = thread::spawn(move || writer_loop(rx));

    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());

    let mut state = State::new(tx.clone());

    loop {
        match read_message(&mut reader) {
            Ok(Some(value)) => {
                let should_exit = state.handle_message(value);
                if should_exit {
                    break;
                }
            }
            Ok(None) => break,
            Err(err) => {
                eprintln!("lsp: failed to read message: {err}");
                break;
            }
        }
    }

    drop(tx);
    let _ = writer.join();
}

struct State {
    config: Config,
    root: Option<PathBuf>,
    docs: DocumentStore,
    sender: Sender<String>,
    shutdown: bool,
    diag_running: Arc<AtomicBool>,
}

impl State {
    fn new(sender: Sender<String>) -> Self {
        Self {
            config: Config::default(),
            root: None,
            docs: DocumentStore::new(),
            sender,
            shutdown: false,
            diag_running: Arc::new(AtomicBool::new(false)),
        }
    }

    fn handle_message(&mut self, value: Value) -> bool {
        let method = value
            .get("method")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let id = value.get("id").cloned();

        match (method.as_deref(), id) {
            (Some(method), Some(id)) => self.handle_request(method, id, value),
            (Some(method), None) => self.handle_notification(method, value),
            (None, _) => false,
        }
    }

    fn handle_request(&mut self, method: &str, id: Value, value: Value) -> bool {
        match method {
            request::Initialize::METHOD => {
                match parse_params::<InitializeParams>(&value) {
                    Ok(params) => {
                        self.root = extract_root(&params);
                        let result = initialize_result();
                        send_response(&self.sender, id, serde_json::to_value(result).unwrap_or(Value::Null));
                    }
                    Err(err) => send_error(&self.sender, id, -32602, &err),
                }
            }
            request::Shutdown::METHOD => {
                self.shutdown = true;
                send_response(&self.sender, id, Value::Null);
            }
            request::HoverRequest::METHOD => {
                match parse_params::<HoverParams>(&value) {
                    Ok(params) => {
                        let result = self.handle_hover(params);
                        send_response(&self.sender, id, serde_json::to_value(result).unwrap_or(Value::Null));
                    }
                    Err(err) => send_error(&self.sender, id, -32602, &err),
                }
            }
            _ => {
                send_error(&self.sender, id, -32601, "method not found");
            }
        }

        false
    }

    fn handle_notification(&mut self, method: &str, value: Value) -> bool {
        match method {
            notification::Initialized::METHOD => {
                let _ = parse_params::<InitializedParams>(&value);
            }
            notification::Exit::METHOD => {
                return true;
            }
            notification::DidOpenTextDocument::METHOD => {
                if let Ok(params) = parse_params::<DidOpenTextDocumentParams>(&value) {
                    self.docs.open(params.text_document);
                }
            }
            notification::DidChangeTextDocument::METHOD => {
                if let Ok(params) = parse_params::<DidChangeTextDocumentParams>(&value) {
                    let uri = params.text_document.uri;
                    let version = params.text_document.version;
                    if let Some(change) = params.content_changes.into_iter().last() {
                        self.docs.change_full(uri, version, change.text);
                    }
                }
            }
            notification::DidCloseTextDocument::METHOD => {
                if let Ok(params) = parse_params::<DidCloseTextDocumentParams>(&value) {
                    self.docs.close(&params.text_document.uri);
                }
            }
            notification::DidSaveTextDocument::METHOD => {
                if let Ok(params) = parse_params::<DidSaveTextDocumentParams>(&value) {
                    self.handle_did_save(params);
                }
            }
            notification::DidChangeConfiguration::METHOD => {
                if let Some(settings) = value.get("params").and_then(|p| p.get("settings")) {
                    self.config.update_from_settings(settings);
                }
            }
            _ => {}
        }

        false
    }

    fn handle_hover(&self, params: HoverParams) -> Option<Hover> {
        let HoverParams {
            text_document_position_params,
            ..
        } = params;
        let uri = text_document_position_params.text_document.uri;
        let position = text_document_position_params.position;
        hover_at(&self.docs, &uri, position)
    }

    fn handle_did_save(&mut self, _params: DidSaveTextDocumentParams) {
        if !self.config.check_on_save {
            return;
        }
        let root = match self.root.as_ref() {
            Some(root) => root.clone(),
            None => return,
        };

        if self.diag_running.swap(true, Ordering::SeqCst) {
            return;
        }

        let open_urls = self.docs.open_urls();
        let check_command = self.config.check_command.clone();
        let sender = self.sender.clone();
        let diag_running = Arc::clone(&self.diag_running);

        thread::spawn(move || {
            if let Ok(map) = run_check(&root, &check_command) {
                publish_diagnostics(&sender, open_urls, map);
            }
            diag_running.store(false, Ordering::SeqCst);
        });
    }
}

fn initialize_result() -> InitializeResult {
    let text_document_sync = TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL);

    let capabilities = ServerCapabilities {
        text_document_sync: Some(text_document_sync),
        hover_provider: Some(lsp_types::HoverProviderCapability::Simple(true)),
        workspace: Some(WorkspaceServerCapabilities {
            workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                supported: Some(true),
                change_notifications: Some(lsp_types::OneOf::Left(true)),
            }),
            ..Default::default()
        }),
        ..Default::default()
    };

    InitializeResult {
        capabilities,
        server_info: None,
    }
}

#[allow(deprecated)]
fn extract_root(params: &InitializeParams) -> Option<PathBuf> {
    if let Some(root_uri) = &params.root_uri {
        if let Some(path) = uri_to_path(root_uri) {
            return Some(path);
        }
    }

    if let Some(root_path) = &params.root_path {
        return Some(PathBuf::from(root_path));
    }

    if let Some(folders) = &params.workspace_folders {
        for folder in folders {
            if let Some(path) = uri_to_path(&folder.uri) {
                return Some(path);
            }
        }
    }

    None
}

fn parse_params<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, String> {
    let params = value.get("params").cloned().unwrap_or(Value::Null);
    serde_json::from_value(params).map_err(|err| err.to_string())
}

fn send_response(sender: &Sender<String>, id: Value, result: Value) {
    let response = json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    });
    send_value(sender, response);
}

fn send_error(sender: &Sender<String>, id: Value, code: i32, message: &str) {
    let response = json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    });
    send_value(sender, response);
}

fn publish_diagnostics(sender: &Sender<String>, open_urls: Vec<Uri>, map: std::collections::HashMap<Uri, Vec<lsp_types::Diagnostic>>) {
    for uri in open_urls {
        let diagnostics = map.get(&uri).cloned().unwrap_or_default();
        let params = lsp_types::PublishDiagnosticsParams::new(uri, diagnostics, None);
        let notification = json!({
            "jsonrpc": "2.0",
            "method": notification::PublishDiagnostics::METHOD,
            "params": params,
        });
        send_value(sender, notification);
    }
}

fn send_value(sender: &Sender<String>, value: Value) {
    let text = match serde_json::to_string(&value) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("lsp: failed to serialize message: {err}");
            return;
        }
    };
    let len = text.as_bytes().len();
    let message = format!("Content-Length: {}\r\n\r\n{}", len, text);
    let _ = sender.send(message);
}

fn read_message(reader: &mut BufReader<impl Read>) -> io::Result<Option<Value>> {
    let mut content_length: Option<usize> = None;
    let mut line = String::new();

    loop {
        line.clear();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            return Ok(None);
        }
        if line == "\r\n" {
            break;
        }
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse::<usize>().ok();
        }
    }

    let length = match content_length {
        Some(len) => len,
        None => return Ok(None),
    };

    let mut buf = vec![0u8; length];
    reader.read_exact(&mut buf)?;
    let value: Value = serde_json::from_slice(&buf).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    Ok(Some(value))
}

fn writer_loop(receiver: mpsc::Receiver<String>) {
    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());
    while let Ok(message) = receiver.recv() {
        if writer.write_all(message.as_bytes()).is_err() {
            break;
        }
        if writer.flush().is_err() {
            break;
        }
    }
}
