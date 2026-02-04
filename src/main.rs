mod config;
mod diagnostics;
mod doc;
mod hover;
mod lsp;

fn main() {
    lsp::server::run();
}
