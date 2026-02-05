mod config;
mod diagnostics;
mod doc;
mod hover;
mod inlay;
mod lsp;

fn main() {
    lsp::server::run();
}
