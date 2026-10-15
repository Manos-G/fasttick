//! The XML panel: the text of the open file as FastTick will save it. It is
//! read-only and follows the canvas: the element of the selected node has a
//! coloured background, and the panel scrolls to it when the selection
//! changes. Select text to copy it.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat};
use egui::{Align, Color32, FontId, RichText, Ui};
use fasttick_core::xml::NodeId;

use super::PanelContext;
use crate::canvas::NodeSource;
use crate::documents::DocId;
use crate::theme::CanvasPalette;

/// The kind of a piece of XML text, for the colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    /// Text between elements.
    Text,
    /// `<`, `>`, `/>`, `</`, `=`.
    Punctuation,
    /// An element name.
    TagName,
    /// An attribute name.
    AttributeName,
    /// An attribute value with its quotes.
    AttributeValue,
    /// A comment, a processing instruction, a declaration or CDATA.
    Comment,
}

/// Splits XML text into coloured pieces. The pieces cover the whole text,
/// in order. The scanner is simple: it only needs to colour text that the
/// XML parser accepted.
pub fn tokenize(text: &str) -> Vec<(Range<usize>, Token)> {
    let bytes = text.as_bytes();
    let mut out: Vec<(Range<usize>, Token)> = Vec::new();
    let mut push = |range: Range<usize>, token: Token| {
        if range.is_empty() {
            return;
        }
        if let Some((last, kind)) = out.last_mut()
            && *kind == token
            && last.end == range.start
        {
            last.end = range.end;
            return;
        }
        out.push((range, token));
    };
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            let end = memchr(b'<', bytes, i).unwrap_or(bytes.len());
            push(i..end, Token::Text);
            i = end;
            continue;
        }
        // Comments, CDATA, declarations and processing instructions.
        let special = [
            ("<!--", "-->"),
            ("<![CDATA[", "]]>"),
            ("<?", "?>"),
            ("<!", ">"),
        ];
        if let Some((_, close)) = special.iter().find(|(open, _)| text[i..].starts_with(open)) {
            let end = text[i..]
                .find(close)
                .map_or(bytes.len(), |p| i + p + close.len());
            push(i..end, Token::Comment);
            i = end;
            continue;
        }
        // A start or end tag.
        let start = i;
        i += 1;
        if i < bytes.len() && bytes[i] == b'/' {
            i += 1;
        }
        push(start..i, Token::Punctuation);
        let name_start = i;
        while i < bytes.len() && !is_tag_delimiter(bytes[i]) {
            i += 1;
        }
        push(name_start..i, Token::TagName);
        while i < bytes.len() && bytes[i] != b'>' {
            match bytes[i] {
                b'"' | b'\'' => {
                    let quote = bytes[i];
                    let end = memchr(quote, bytes, i + 1).map_or(bytes.len(), |p| p + 1);
                    push(i..end, Token::AttributeValue);
                    i = end;
                }
                b'=' | b'/' => {
                    push(i..i + 1, Token::Punctuation);
                    i += 1;
                }
                b if b.is_ascii_whitespace() => {
                    let end = (i..bytes.len())
                        .find(|&j| !bytes[j].is_ascii_whitespace())
                        .unwrap_or(bytes.len());
                    push(i..end, Token::Text);
                    i = end;
                }
                _ => {
                    let end = (i..bytes.len())
                        .find(|&j| is_tag_delimiter(bytes[j]) || bytes[j] == b'=')
                        .unwrap_or(bytes.len());
                    push(i..end.max(i + 1), Token::AttributeName);
                    i = end.max(i + 1);
                }
            }
        }
        if i < bytes.len() {
            push(i..i + 1, Token::Punctuation);
            i += 1;
        }
    }
    out
}

fn is_tag_delimiter(b: u8) -> bool {
    b.is_ascii_whitespace() || b == b'>' || b == b'/'
}

fn memchr(needle: u8, bytes: &[u8], from: usize) -> Option<usize> {
    bytes
        .get(from..)?
        .iter()
        .position(|&b| b == needle)
        .map(|p| from + p)
}

/// What a cached layout job depends on: the document and its revision, the
/// highlighted range and the theme.
type JobKey = ((DocId, u64), Option<Range<usize>>, bool);

/// The text of a file with the range of each element.
#[derive(Debug, Clone)]
struct XmlText {
    key: (DocId, u64),
    text: String,
    ranges: HashMap<NodeId, Range<usize>>,
    tokens: Vec<(Range<usize>, Token)>,
    line_starts: Vec<usize>,
}

impl XmlText {
    fn line_of(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
    }
}

/// Builds the layout job: syntax colours, and a background on `highlight`.
fn layout_job(
    xml: &XmlText,
    highlight: Option<Range<usize>>,
    palette: &CanvasPalette,
    font: FontId,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let color = |token: Token| match token {
        Token::Text => palette.text,
        Token::Punctuation => palette.text_weak,
        Token::TagName => palette.accent,
        Token::AttributeName => palette.text,
        Token::AttributeValue => palette.text_value,
        Token::Comment => palette.text_weak,
    };
    let mut add = |range: Range<usize>, token: Token, background: Color32| {
        if range.is_empty() {
            return;
        }
        job.append(
            &xml.text[range],
            0.0,
            TextFormat {
                font_id: font.clone(),
                color: color(token),
                background,
                ..TextFormat::default()
            },
        );
    };
    for (range, token) in &xml.tokens {
        match &highlight {
            Some(h) if h.start < range.end && range.start < h.end => {
                let a = range.start.max(h.start);
                let b = range.end.min(h.end);
                add(range.start..a, *token, Color32::TRANSPARENT);
                add(a..b, *token, palette.band_fill);
                add(b..range.end, *token, Color32::TRANSPARENT);
            }
            _ => add(range.clone(), *token, Color32::TRANSPARENT),
        }
    }
    job
}

/// Draws the XML panel.
pub fn show(ui: &mut Ui, cx: &mut PanelContext<'_>) {
    let Some((view_id, editor, main)) = super::active_edit(cx.workspace, cx.mode) else {
        super::empty(ui, "The XML of a tree file shows here in edit mode.");
        return;
    };
    let Some(view) = cx.workspace.view(view_id) else {
        return;
    };
    let Some(file) = editor.project().file(main) else {
        return;
    };
    let key = (view.doc, editor.revision() ^ ((main.0 as u64) << 48));

    // The text, cached for each revision.
    let cache_id = ui.id().with("xml_text");
    let cached: Option<Arc<XmlText>> = ui.data(|d| d.get_temp(cache_id));
    let xml = match cached.filter(|c| c.key == key) {
        Some(xml) => xml,
        None => {
            let (text, list) = file.doc().write_with_ranges();
            let tokens = tokenize(&text);
            let mut line_starts = vec![0];
            line_starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
            let xml = Arc::new(XmlText {
                key,
                ranges: list.into_iter().collect(),
                tokens,
                line_starts,
                text,
            });
            ui.data_mut(|d| d.insert_temp(cache_id, xml.clone()));
            xml
        }
    };

    // The selected element.
    let tree = view.canvas.tree();
    let selected = view
        .canvas
        .primary()
        .filter(|id| tree.is_selected(*id))
        .or_else(|| tree.selected().next())
        .and_then(|id| tree.get(id))
        .and_then(|node| match node.source {
            NodeSource::Xml(file, id) if file == main && !node.inline => Some(id),
            _ => None,
        });
    let highlight = selected.and_then(|n| xml.ranges.get(&n).cloned());

    let lines = xml.line_starts.len();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(
                "{} lines · read-only; edit the tree on the canvas",
                lines
            ))
            .small()
            .weak(),
        );
        if ui.small_button("Copy all").clicked() {
            ui.ctx().copy_text(xml.text.clone());
        }
    });
    let font = FontId::monospace(12.5);
    let job_id = ui.id().with("xml_job");
    let job_key = (key, highlight.clone(), cx.palette.dark);
    let cached_job: Option<(JobKey, Arc<LayoutJob>)> = ui.data(|d| d.get_temp(job_id));
    let job = match cached_job.filter(|(k, _)| *k == job_key) {
        Some((_, job)) => job,
        None => {
            let job = Arc::new(layout_job(
                &xml,
                highlight.clone(),
                cx.palette,
                font.clone(),
            ));
            ui.data_mut(|d| d.insert_temp(job_id, (job_key, job.clone())));
            job
        }
    };
    let galley = ui.fonts_mut(|f| f.layout_job((*job).clone()));
    let numbers: String = (1..=lines).map(|n| format!("{n:>4}\n")).collect();

    // Scroll to the selected element when the selection changes.
    let last_id = ui.id().with("xml_last_selection");
    let last: Option<Option<NodeId>> = ui.data(|d| d.get_temp(last_id));
    let scroll_to = (last != Some(selected))
        .then_some(())
        .and(highlight.clone());
    ui.data_mut(|d| d.insert_temp(last_id, selected));

    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.label(
                    RichText::new(numbers)
                        .font(font.clone())
                        .color(cx.palette.text_weak),
                );
                let response = ui.add(egui::Label::new(galley.clone()).selectable(true));
                if let Some(range) = scroll_to {
                    let line = xml.line_of(range.start);
                    if let Some(row) = galley.rows.get(line) {
                        let rect = row.rect().translate(response.rect.min.to_vec2());
                        ui.scroll_to_rect(rect, Some(Align::Center));
                    }
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(&str, Token)> {
        tokenize(text)
            .into_iter()
            .map(|(r, t)| (&text[r], t))
            .collect()
    }

    #[test]
    fn tokens_cover_the_text() {
        let text = "<?xml version=\"1.0\"?>\n<!-- c -->\n<root a=\"1\">\n  <A b='x' c=\"{k}\"/>\n  t\n</root>\n";
        let tokens = tokenize(text);
        let mut at = 0;
        for (range, _) in &tokens {
            assert_eq!(range.start, at);
            at = range.end;
        }
        assert_eq!(at, text.len());
        let k = kinds(text);
        assert!(k.contains(&("<?xml version=\"1.0\"?>", Token::Comment)));
        assert!(k.contains(&("<!-- c -->", Token::Comment)));
        assert!(k.contains(&("root", Token::TagName)));
        assert!(k.contains(&("A", Token::TagName)));
        assert!(k.contains(&("b", Token::AttributeName)));
        assert!(k.contains(&("'x'", Token::AttributeValue)));
        assert!(k.contains(&("\"{k}\"", Token::AttributeValue)));
        assert!(k.contains(&("</", Token::Punctuation)));
    }

    #[test]
    fn unfinished_text_does_not_panic() {
        for text in ["<", "<a", "<a b", "<a b=\"", "<!--", "</", "<a/"] {
            let tokens = tokenize(text);
            assert_eq!(
                tokens.last().map(|(r, _)| r.end),
                Some(text.len()),
                "{text}"
            );
        }
    }
}
