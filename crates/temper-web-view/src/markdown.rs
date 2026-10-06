//! A bounded Markdown reader for task reports (02-view.md, section 9).
//! Unrecognised syntax remains text; no HTML is built from input bytes.

use skein_lib::Stack;

use crate::Builder;
use crate::limits::Limits;
use crate::tree::{Element, Level};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct ListLevel {
    indent: usize,
    ordered: bool,
}

/// Read the supported Markdown subset into a builder. The builder owns every
/// copied text span; this function retains no bytes after it returns.
pub fn read(input: &[u8], limits: &Limits, builder: &mut Builder) {
    let mut lists = Stack::with_capacity(limits.markdown_depth);
    let mut paragraph = false;
    let mut fence = false;
    let mut at = 0;
    while let Some((line, next)) = line_at(input, at) {
        at = next;
        if fence {
            if line.starts_with(b"```") {
                builder.close(); // Code
                builder.close(); // Pre
                fence = false;
            } else {
                builder.text(line);
                builder.text(b"\n");
            }
            continue;
        }
        if line.is_empty() {
            close_paragraph(builder, &mut paragraph);
            close_lists(builder, &mut lists);
            continue;
        }
        if line.starts_with(b"```") && has_fence_close(input, at) {
            close_paragraph(builder, &mut paragraph);
            close_lists(builder, &mut lists);
            builder.open(Element::Pre);
            builder.open(Element::Code);
            fence = true;
            continue;
        }
        if let Some(body) = heading(line) {
            close_paragraph(builder, &mut paragraph);
            close_lists(builder, &mut lists);
            builder.open(Element::Heading(Level::Three));
            inline(body, builder);
            builder.close();
            continue;
        }
        if let Some(body) = line.strip_prefix(b"> ") {
            close_paragraph(builder, &mut paragraph);
            close_lists(builder, &mut lists);
            builder.open(Element::Quote);
            inline(body, builder);
            builder.close();
            continue;
        }
        if let Some((level, body)) = list_item(line) {
            close_paragraph(builder, &mut paragraph);
            if list_step(builder, &mut lists, level, limits.markdown_depth) {
                inline(body, builder);
            } else {
                close_lists(builder, &mut lists);
                builder.open(Element::Paragraph);
                builder.text(line);
                builder.close();
            }
            continue;
        }
        close_lists(builder, &mut lists);
        if paragraph {
            builder.text(b"\n");
        } else {
            builder.open(Element::Paragraph);
            paragraph = true;
        }
        inline(line, builder);
    }
    close_paragraph(builder, &mut paragraph);
    close_lists(builder, &mut lists);
    assert!(!fence, "only a fence with a closing line opens");
}

fn line_at(input: &[u8], at: usize) -> Option<(&[u8], usize)> {
    if at >= input.len() {
        return None;
    }
    let mut end = at;
    while let Some(byte) = input.get(end) {
        if *byte == b'\n' {
            break;
        }
        end = end.checked_add(1).expect("input index fits usize");
    }
    let next = if end < input.len() { end.checked_add(1).expect("input index fits usize") } else { end };
    let line = input.get(at..end).expect("line lies in input");
    Some((line.strip_suffix(b"\r").unwrap_or(line), next))
}

fn has_fence_close(input: &[u8], mut at: usize) -> bool {
    while let Some((line, next)) = line_at(input, at) {
        if line.starts_with(b"```") {
            return true;
        }
        at = next;
    }
    false
}

fn heading(line: &[u8]) -> Option<&[u8]> {
    let mut count: usize = 0;
    for byte in line {
        if *byte != b'#' {
            break;
        }
        count = count.checked_add(1).expect("heading marker count fits usize");
        if count > 6 {
            return None;
        }
    }
    if count == 0 || line.get(count) != Some(&b' ') {
        return None;
    }
    line.get(count.checked_add(1)?..)
}

fn list_item(line: &[u8]) -> Option<(ListLevel, &[u8])> {
    let mut indent = 0;
    while line.get(indent) == Some(&b' ') {
        indent = indent.checked_add(1)?;
    }
    let rest = line.get(indent..)?;
    if let Some(body) = rest.strip_prefix(b"- ") {
        return Some((ListLevel { indent, ordered: false }, body));
    }
    let mut digits = 0;
    while let Some(byte) = rest.get(digits) {
        if !byte.is_ascii_digit() {
            break;
        }
        digits = digits.checked_add(1)?;
    }
    if digits == 0 {
        return None;
    }
    if rest.get(digits) != Some(&b'.') || rest.get(digits.checked_add(1)?) != Some(&b' ') {
        return None;
    }
    Some((ListLevel { indent, ordered: true }, rest.get(digits.checked_add(2)?..)?))
}

fn list_step(builder: &mut Builder, lists: &mut Stack<ListLevel>, level: ListLevel, limit: u32) -> bool {
    while let Some(top) = lists.top() {
        if top.indent <= level.indent {
            break;
        }
        builder.close(); // Item
        builder.close(); // List
        let _closed = lists.pop().expect("list level exists");
    }
    if let Some(top) = lists.top() {
        if top.indent == level.indent && top.ordered == level.ordered {
            builder.close(); // previous Item
            builder.open(Element::Item);
            return true;
        }
        if top.indent == level.indent {
            builder.close();
            builder.close();
            let _closed = lists.pop().expect("list level exists");
        }
    }
    if lists.len() >= limit {
        return false;
    }
    builder.open(if level.ordered { Element::OrderedList } else { Element::List });
    lists.push(level).expect("list nesting fits limit");
    builder.open(Element::Item);
    true
}

fn close_lists(builder: &mut Builder, lists: &mut Stack<ListLevel>) {
    while lists.pop().is_some() {
        builder.close(); // Item
        builder.close(); // List
    }
}

fn close_paragraph(builder: &mut Builder, paragraph: &mut bool) {
    if *paragraph {
        builder.close();
        *paragraph = false;
    }
}

fn inline(line: &[u8], builder: &mut Builder) {
    let mut at = 0;
    let mut plain = 0;
    while at < line.len() {
        let remaining = line.get(at..).expect("inline index lies in line");
        let marker = if remaining.starts_with(b"**") {
            span(line, at, b"**", Element::Strong)
        } else if remaining.starts_with(b"*") {
            span(line, at, b"*", Element::Emphasis)
        } else if remaining.starts_with(b"`") {
            span(line, at, b"`", Element::Code)
        } else {
            None
        };
        if let Some((end, element, body)) = marker {
            flush(line, plain, at, builder);
            builder.open(element);
            builder.text(body);
            builder.close();
            at = end;
            plain = at;
            continue;
        }
        if remaining.starts_with(b"[")
            && let Some((end, label, url)) = link(line, at)
        {
            flush(line, plain, at, builder);
            builder.open(Element::Link);
            builder.external(url);
            builder.name(label);
            builder.text(label);
            builder.close();
            at = end;
            plain = at;
            continue;
        }
        at = at.checked_add(1).expect("inline index fits usize");
    }
    flush(line, plain, at, builder);
}

fn span<'a>(line: &'a [u8], at: usize, mark: &[u8], element: Element) -> Option<(usize, Element, &'a [u8])> {
    let start = at.checked_add(mark.len())?;
    let close = find(line, mark, start)?;
    if close == start {
        return None;
    }
    let end = close.checked_add(mark.len())?;
    Some((end, element, line.get(start..close)?))
}

fn link(line: &[u8], at: usize) -> Option<(usize, &[u8], &[u8])> {
    let label_start = at.checked_add(1)?;
    let label_end = find(line, b"](", label_start)?;
    let url_start = label_end.checked_add(2)?;
    let url_end = find(line, b")", url_start)?;
    let url = line.get(url_start..url_end)?;
    if !safe_url(url) {
        return None;
    }
    let end = url_end.checked_add(1)?;
    Some((end, line.get(label_start..label_end)?, url))
}

fn safe_url(url: &[u8]) -> bool {
    let host = if let Some(host) = url.strip_prefix(b"https://") {
        host
    } else if let Some(host) = url.strip_prefix(b"http://") {
        host
    } else {
        return false;
    };
    if host.is_empty() || host.first() == Some(&b'/') {
        return false;
    }
    for byte in url {
        if *byte <= b' ' || *byte == b'"' || *byte == b'<' || *byte == b'>' {
            return false;
        }
    }
    true
}

fn find(haystack: &[u8], needle: &[u8], mut at: usize) -> Option<usize> {
    while at.checked_add(needle.len())? <= haystack.len() {
        if haystack.get(at..at.checked_add(needle.len())?)? == needle {
            return Some(at);
        }
        at = at.checked_add(1)?;
    }
    None
}

fn flush(line: &[u8], start: usize, end: usize, builder: &mut Builder) {
    if start < end {
        builder.text(line.get(start..end).expect("plain span lies in line"));
    }
}
