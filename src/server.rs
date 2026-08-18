//! LSP backend: completion / diagnostics / hover / definition for `@path`
//! references in one-shot prompt files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::index::FileIndex;
use crate::refs::{self, PathRef};
use crate::text::Document;

/// Maximum number of completion candidates returned per request.
const COMPLETION_LIMIT: usize = 50;
/// Number of lines shown in the hover preview.
const HOVER_PREVIEW_LINES: usize = 30;
/// Maximum bytes read for a hover preview (avoid slurping huge files).
const HOVER_READ_LIMIT: u64 = 64 * 1024;

pub struct Backend {
    client: Client,
    root: PathBuf,
    max_entries: usize,
    /// Swapped wholesale on rebuild so readers never see a half-built index.
    index: RwLock<Arc<FileIndex>>,
    docs: RwLock<HashMap<Url, Document>>,
}

impl Backend {
    pub fn new(client: Client, root: PathBuf, max_entries: usize) -> Self {
        let index = RwLock::new(Arc::new(FileIndex::empty(root.clone())));
        Self { client, root, max_entries, index, docs: RwLock::new(HashMap::new()) }
    }

    /// Rebuild the file index off the async runtime and swap it in.
    async fn rebuild_index(&self) {
        let root = self.root.clone();
        let max = self.max_entries;
        let built = tokio::task::spawn_blocking(move || FileIndex::build(&root, max))
            .await
            .expect("index build task panicked");
        let msg = format!(
            "promptls: indexed {} entries under {}{}",
            built.len(),
            built.root().display(),
            if built.truncated { " (truncated; raise --max-entries)" } else { "" }
        );
        *self.index.write().await = Arc::new(built);
        self.client.log_message(MessageType::INFO, msg).await;
    }

    fn resolve(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    /// Compute and publish diagnostics for one document.
    async fn publish_diagnostics(&self, uri: Url, doc: &Document) {
        let diags = refs::find_refs(doc.text())
            .into_iter()
            .filter(|r| !self.resolve(&r.path).exists())
            .map(|r| Diagnostic {
                range: Range::new(doc.offset_to_position(r.start), doc.offset_to_position(r.end)),
                severity: Some(DiagnosticSeverity::WARNING),
                source: Some("promptls".into()),
                message: format!("`{}` does not exist under {}", r.path, self.root.display()),
                ..Default::default()
            })
            .collect();
        self.client.publish_diagnostics(uri, diags, None).await;
    }

    /// Locate the document and the `@path` reference under `pos`.
    async fn ref_at_position(&self, uri: &Url, pos: Position) -> Option<(Document, PathRef)> {
        let docs = self.docs.read().await;
        let doc = docs.get(uri)?.clone();
        let offset = doc.position_to_offset(pos)?;
        let r = refs::ref_at(doc.text(), offset)?;
        Some((doc, r))
    }
}

/// Markdown hover body for a path (file preview or directory listing).
fn hover_markdown(root: &Path, r: &PathRef) -> String {
    let abs = root.join(&r.path);
    let Ok(meta) = std::fs::metadata(&abs) else {
        return format!("`{}` — **not found** under `{}`", r.path, root.display());
    };

    if meta.is_dir() {
        let mut names: Vec<String> = match std::fs::read_dir(&abs) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if e.file_type().is_ok_and(|t| t.is_dir()) { format!("{name}/") } else { name }
                })
                .collect(),
            Err(err) => return format!("`{}/` — cannot read: {err}", r.path),
        };
        names.sort();
        let total = names.len();
        names.truncate(HOVER_PREVIEW_LINES);
        let more = if total > names.len() { format!("\n… {} more", total - names.len()) } else { String::new() };
        return format!("**{}/** ({total} entries)\n```\n{}{more}\n```", r.path, names.join("\n"));
    }

    // File: read a bounded prefix and show the first N lines.
    let bytes = match std::fs::File::open(&abs) {
        Ok(f) => {
            use std::io::Read;
            let mut buf = Vec::new();
            if let Err(err) = f.take(HOVER_READ_LIMIT).read_to_end(&mut buf) {
                return format!("`{}` — cannot read: {err}", r.path);
            }
            buf
        }
        Err(err) => return format!("`{}` — cannot read: {err}", r.path),
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return format!("**{}** — binary file, {} bytes", r.path, meta.len());
    };
    let lang = abs.extension().and_then(|e| e.to_str()).unwrap_or("");
    let preview: Vec<&str> = text.lines().take(HOVER_PREVIEW_LINES).collect();
    let shown = preview.len();
    let truncated = text.lines().nth(HOVER_PREVIEW_LINES).is_some() || bytes.len() as u64 >= HOVER_READ_LIMIT;
    // Use a fence longer than any backtick run in the preview so content
    // containing ``` does not break the block.
    let fence = "`".repeat(4);
    format!(
        "**{}** ({} bytes){}\n{fence}{lang}\n{}\n{fence}",
        r.path,
        meta.len(),
        if truncated { format!(" — first {shown} lines") } else { String::new() },
        preview.join("\n"),
    )
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            server_info: Some(ServerInfo { name: "promptls".into(), version: Some(env!("CARGO_PKG_VERSION").into()) }),
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
                completion_provider: Some(CompletionOptions {
                    // `/` re-triggers after a directory is accepted.
                    trigger_characters: Some(vec!["@".into(), "/".into()]),
                    resolve_provider: Some(false),
                    ..Default::default()
                }),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.rebuild_index().await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let doc = Document::new(params.text_document.text);
        self.docs.write().await.insert(uri.clone(), doc.clone());
        self.publish_diagnostics(uri, &doc).await;
        // A new prompt file usually means a new session; refresh the index so
        // files created since startup are completable.
        self.rebuild_index().await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        // FULL sync: the last change carries the whole document.
        let Some(change) = params.content_changes.into_iter().last() else { return };
        let uri = params.text_document.uri;
        let doc = Document::new(change.text);
        self.docs.write().await.insert(uri.clone(), doc.clone());
        self.publish_diagnostics(uri, &doc).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        self.docs.write().await.remove(&params.text_document.uri);
        self.client.publish_diagnostics(params.text_document.uri, Vec::new(), None).await;
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;

        let query = {
            let docs = self.docs.read().await;
            let Some(doc) = docs.get(&uri) else { return Ok(None) };
            let Some(cursor) = doc.position_to_offset(pos) else { return Ok(None) };
            let Some(q) = refs::query_at(doc.text(), cursor) else { return Ok(None) };
            // Range replaced by the completion: from just after `@` to cursor.
            let range = Range::new(doc.offset_to_position(q.at + 1), pos);
            (q.text, range)
        };
        let (query_text, range) = query;

        let index = self.index.read().await.clone();
        let items: Vec<CompletionItem> = index
            .search(&query_text, COMPLETION_LIMIT)
            .into_iter()
            .enumerate()
            .map(|(i, e)| {
                let insert = if e.is_dir { format!("{}/", e.rel) } else { e.rel.clone() };
                CompletionItem {
                    label: insert.clone(),
                    kind: Some(if e.is_dir { CompletionItemKind::FOLDER } else { CompletionItemKind::FILE }),
                    // The server already ranked fuzzily; make the client's own
                    // prefix filter a no-op by matching the typed text exactly.
                    filter_text: Some(query_text.clone()),
                    sort_text: Some(format!("{i:05}")),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit { range, new_text: insert })),
                    ..Default::default()
                }
            })
            .collect();

        // Always incomplete: results depend on the full query, so ask the
        // client to re-request on every keystroke instead of filtering locally.
        Ok(Some(CompletionResponse::List(CompletionList { is_incomplete: true, items })))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let Some((doc, r)) = self.ref_at_position(&uri, pos).await else { return Ok(None) };
        let root = self.root.clone();
        let md = tokio::task::spawn_blocking(move || hover_markdown(&root, &r)).await.expect("hover task panicked");
        let r = refs::ref_at(doc.text(), doc.position_to_offset(pos).unwrap_or(0)).expect("ref vanished");
        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value: md }),
            range: Some(Range::new(doc.offset_to_position(r.start), doc.offset_to_position(r.end))),
        }))
    }

    async fn goto_definition(&self, params: GotoDefinitionParams) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let Some((_, r)) = self.ref_at_position(&uri, pos).await else { return Ok(None) };
        let abs = self.resolve(&r.path);
        if !abs.exists() {
            return Ok(None);
        }
        let Ok(target) = Url::from_file_path(&abs) else { return Ok(None) };
        // `:LINE` hint is 1-based; LSP lines are 0-based.
        let line = r.line.map(|l| l.saturating_sub(1)).unwrap_or(0);
        let p = Position::new(line, 0);
        Ok(Some(GotoDefinitionResponse::Scalar(Location { uri: target, range: Range::new(p, p) })))
    }
}
