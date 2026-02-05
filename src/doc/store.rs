use std::collections::HashMap;

use lsp_types::{TextDocumentItem, Uri};

#[derive(Debug, Clone)]
pub struct Document {
    pub text: String,
    pub version: i32,
}

#[derive(Debug, Default)]
pub struct DocumentStore {
    docs: HashMap<Uri, Document>,
}

impl DocumentStore {
    pub fn new() -> Self {
        Self {
            docs: HashMap::new(),
        }
    }

    pub fn open(&mut self, item: TextDocumentItem) {
        let doc = Document {
            text: item.text,
            version: item.version,
        };
        self.docs.insert(item.uri, doc);
    }

    pub fn change_full(&mut self, uri: Uri, version: i32, text: String) {
        if let Some(doc) = self.docs.get_mut(&uri) {
            doc.text = text;
            doc.version = version;
        } else {
            self.docs.insert(uri, Document { text, version });
        }
    }

    pub fn close(&mut self, uri: &Uri) {
        self.docs.remove(uri);
    }

    pub fn get(&self, uri: &Uri) -> Option<&Document> {
        self.docs.get(uri)
    }

    pub fn open_urls(&self) -> Vec<Uri> {
        self.docs.keys().cloned().collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Uri, &Document)> {
        self.docs.iter()
    }
}
