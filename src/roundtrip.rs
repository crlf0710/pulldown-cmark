#![allow(dead_code, unused_mut)]
use crate::escape::{StrWrite, WriteWrapper};
use crate::CowStr;
use crate::{CodeBlockKind, LinkType};
use crate::{Event, Tag};
use std::collections::VecDeque;
use std::io::{self, Write};

struct MarkdownWriter<'a, I, W> {
    /// Iterator supplying events.
    iter: I,

    /// Writer to write to.
    writer: W,

    phantom: core::marker::PhantomData<&'a ()>,
}

fn is_block_tag(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Paragraph
            | Tag::Heading(_)
            | Tag::BlockQuote
            | Tag::CodeBlock(_)
            | Tag::List(_)
            | Tag::Item
            | Tag::FootnoteDefinition(_)
            | Tag::Table(_)
            | Tag::TableHead
            | Tag::TableRow
            | Tag::TableCell
    )
}

fn is_container_block_tag(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::BlockQuote
            | Tag::List(_)
            | Tag::Item
            | Tag::Table(_)
            | Tag::TableHead
            | Tag::TableRow
            | Tag::TableCell
    )
}

fn is_block_nesting_start(event: &Event<'_>) -> bool {
    if let Event::Start(tag) = event {
        is_block_tag(tag)
    } else {
        false
    }
}

fn is_block_nesting_end(event: &Event<'_>) -> bool {
    if let Event::End(tag) = event {
        is_block_tag(tag)
    } else {
        false
    }
}

fn is_leaf_block_start(event: &Event<'_>) -> bool {
    if let Event::Start(tag) = event {
        is_block_tag(tag) && !is_container_block_tag(tag)
    } else {
        false
    }
}

#[derive(Clone, Copy, PartialEq)]
#[allow(non_camel_case_types)]
enum tri_bool {
    r#true,
    r#false,
    maybe,
}

fn is_childless_block(event: &Event<'_>) -> tri_bool {
    if matches!(event, Event::Rule) {
        tri_bool::r#true
    } else if matches!(event, Event::Html(_) | Event::Text(_)) {
        tri_bool::maybe
    } else {
        tri_bool::r#false
    }
}

// FIXME: the context param here is due to limitation of pulldown-cmark itself
// remove it when it's always feasible to finish this check from `Event` type.
fn is_childless_block_2(context: &[Event<'_>], event: &Event<'_>) -> bool {
    if matches!(event, Event::Rule) {
        true
    } else if matches!(event, Event::Html(_)) {
        !context.last().map_or(false, is_leaf_block_start)
    } else if matches!(event, Event::Text(_)) {
        if let Some(Event::Start(Tag::Item)) = context.last() {
            // so called tight list.
            true
        } else {
            false
        }
    } else {
        false
    }
}

#[derive(Clone, Copy, Default)]
struct EscapeOptions(usize);

impl EscapeOptions {
    const ESCAPE_LINEFEED: EscapeOptions = EscapeOptions(1);

    fn has(self, rhs: EscapeOptions) -> bool {
        self.0 & rhs.0 == rhs.0
    }
}

fn escape_text<'a>(input: &CowStr<'a>, options: EscapeOptions) -> CowStr<'a> {
    let mut rewrite_str = None;
    let input_str = &*input;
    for (offset, ch) in input_str.char_indices() {
        if ch.is_ascii_punctuation() {
            let rewrite_str = rewrite_str.get_or_insert_with(|| input_str[..offset].to_string());
            rewrite_str.push('\\');
            rewrite_str.push(ch);
        } else if ch == '\n' && options.has(EscapeOptions::ESCAPE_LINEFEED) {
            let rewrite_str = rewrite_str.get_or_insert_with(|| input_str[..offset].to_string());
            *rewrite_str += "&#10;";
        } else {
            if let Some(rewrite_str) = rewrite_str.as_mut() {
                rewrite_str.push(ch);
            }
        }
    }
    if let Some(str) = rewrite_str {
        str.into()
    } else {
        input.clone()
    }
}

impl<'a, I, W> MarkdownWriter<'a, I, W>
where
    I: Iterator<Item = Event<'a>>,
    W: StrWrite,
{
    fn new(iter: I, writer: W) -> Self {
        MarkdownWriter {
            iter,
            writer,
            phantom: core::marker::PhantomData,
        }
    }

    fn run(mut self) -> io::Result<()> {
        let mut stack = Vec::new();
        let mut incoming_stack = VecDeque::new();
        let mut outgoing_counter = 0;
        let mut iter = self.iter.peekable();
        // In general, we split markdown generation into a sequence of four actions
        // 1. Encountering a series of starts of block containers
        // 2. Encountering a series of inlines
        // 3. Encountering one leaf block
        // 4. Encountering a series of endings of block containers
        // 5. Transition over the valley between block containers
        let mut state = 0;
        loop {
            let mut new_state = 0;
            'seq: while let Some(event) = iter.peek() {
                if is_block_nesting_start(event) {
                    new_state = 1;
                } else if is_block_nesting_end(event) {
                    new_state = 4;
                } else {
                    match is_childless_block(event) {
                        tri_bool::r#true => {
                            new_state = 3;
                        }
                        tri_bool::r#false => {
                            new_state = 2;
                        }
                        tri_bool::maybe => {
                            let mut context = if state == 4 {
                                let remaining_stack_len =
                                    stack.len().checked_sub(outgoing_counter).unwrap();
                                stack[0..remaining_stack_len].iter().cloned().collect()
                            } else {
                                stack.clone()
                            };
                            if state == 1 {
                                context.extend(incoming_stack.iter().cloned());
                            }
                            if is_childless_block_2(&context, event) {
                                new_state = 3;
                            } else {
                                new_state = 2;
                            }
                        }
                    }
                }
                if new_state == state {
                    // eprintln!("dbg: keep state {} => {}", state, new_state);
                    if state == 4 {
                        let _ = iter.next();
                        outgoing_counter += 1;
                    } else if state == 3 {
                        unreachable!();
                    } else {
                        incoming_stack.push_back(iter.next().unwrap());
                    }
                    new_state = 0;
                    continue 'seq;
                } else {
                    if state == 4 && new_state == 1 {
                        // eprintln!("dbg: prepare transition {} => {}", state, new_state);
                        state = 1;
                        incoming_stack.push_back(iter.next().unwrap());
                        new_state = 0;
                        continue 'seq;
                    } else {
                        break 'seq;
                    }
                }
            }
            // The state is preparing to change.
            // Let's check what's the previous state first.
            // eprintln!("dbg: change state {} => {}", state, new_state);
            if state == 0 {
                if new_state == 0 {
                    // empty file
                    let _ = iter.next();
                    return Ok(());
                } else if new_state == 1 || new_state == 3 {
                    // do nothing
                } else {
                    // if here is reached, it means
                    // there's something wrong with the sequence itself
                    // it is possible to do error recovery here
                    unreachable!("event = {:?}", iter.peek());
                }
            } else if state == 1 {
                let remaining_stack_len = stack.len().checked_sub(outgoing_counter).unwrap();
                if outgoing_counter != 0 {
                    Self::process_transition(
                        &mut self.writer,
                        &stack[0..remaining_stack_len],
                        &stack[remaining_stack_len..],
                        incoming_stack.make_contiguous(),
                    )?;
                    stack.drain(remaining_stack_len..);
                }
                Self::process_enter_nesting(&mut self.writer, &mut stack, &mut incoming_stack)?;
                stack.extend(incoming_stack.drain(..));
            } else if state == 2 {
                if let (4, Some(Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(_))))) =
                    (new_state, stack.last())
                {
                    if let Some(Event::Text(text_str)) = incoming_stack.back_mut() {
                        // FIXME pulldown_cmark leaves a `\n` here if `new_state` is 4,
                        // and doesn't leave one here if `new_state` is 0.
                        // we fix it here to handle it consistently.
                        if text_str.ends_with('\n') {
                            *text_str = text_str[..text_str.len() - 1].to_string().into();
                        }
                    }
                }
                Self::process_nonnesting_sequence(
                    &mut self.writer,
                    &stack,
                    incoming_stack.make_contiguous(),
                )?;
            } else if state == 3 {
                unreachable!();
            } else if state == 4 {
                let remaining_stack_len = stack.len().checked_sub(outgoing_counter).unwrap();
                Self::process_exit_nesting(
                    &mut self.writer,
                    &stack[0..remaining_stack_len],
                    &stack[remaining_stack_len..],
                )?;
            }
            incoming_stack.clear();
            if new_state == 1 || new_state == 2 {
                outgoing_counter = 0;
                incoming_stack.push_back(iter.next().unwrap());
            } else if new_state == 3 {
                incoming_stack.push_back(iter.next().unwrap());
                if state == 4 {
                    let remaining_stack_len = stack.len().checked_sub(outgoing_counter).unwrap();
                    Self::process_transition(
                        &mut self.writer,
                        &stack[0..remaining_stack_len],
                        &stack[remaining_stack_len..],
                        incoming_stack.make_contiguous(),
                    )?;
                    stack.drain(remaining_stack_len..);
                }
                outgoing_counter = 0;
                Self::process_enter_nesting(&mut self.writer, &mut stack, &mut incoming_stack)?;
                stack.extend(incoming_stack.drain(..));
                outgoing_counter += 1;
                new_state = 4;
            } else if new_state == 4 {
                outgoing_counter = 0;
                let _ = iter.next();
                outgoing_counter += 1;
            } else {
                return Ok(());
            }
            state = new_state;
        }
    }

    fn process_transition(
        writer: &mut W,
        context: &[Event<'a>],
        removing_sequence: &[Event<'a>],
        added_sequence: &[Event<'a>],
    ) -> io::Result<()> {
        enum TransitionStrategy {
            DoNothing,
            NewlineAndRenew,
            ExtraNewlineAndRenew,
        }
        let mut strategy = None;
        if strategy.is_none() && added_sequence.is_empty() && context.is_empty() {
            strategy = Some(TransitionStrategy::DoNothing);
        }

        if strategy.is_none() {
            match (removing_sequence, added_sequence) {
                ([Event::Start(Tag::Paragraph)], [Event::Start(Tag::Paragraph)]) => {
                    strategy = Some(TransitionStrategy::ExtraNewlineAndRenew);
                }
                ([Event::Start(Tag::Heading(_))], [Event::Start(Tag::Heading(_))]) => {
                    strategy = Some(TransitionStrategy::NewlineAndRenew);
                }
                (
                    [Event::Start(Tag::BlockQuote), Event::Start(Tag::Paragraph)],
                    [Event::Start(Tag::BlockQuote), Event::Start(Tag::Paragraph)],
                ) => {
                    strategy = Some(TransitionStrategy::ExtraNewlineAndRenew);
                }
                ([Event::Html(_)], [Event::Html(_)]) => {
                    strategy = Some(TransitionStrategy::DoNothing);
                }
                ([Event::Text(_)], [Event::Text(_)]) => {
                    // FIXME: This is the tight list case.
                    strategy = Some(TransitionStrategy::DoNothing);
                }
                _ => {}
            }
        }

        if strategy.is_none() {
            match removing_sequence {
                [Event::Start(Tag::List(_)), ..] => {
                    strategy = Some(TransitionStrategy::ExtraNewlineAndRenew);
                }
                _ => {}
            }
        }

        let strategy = match strategy {
            None => {
                eprintln!(
                    "unhandled transition between event, context = {:?}, removing = {:?}, adding = {:?}",
                    context,
                    removing_sequence,
                    added_sequence);
                TransitionStrategy::NewlineAndRenew
            }
            Some(s) => s,
        };
        match strategy {
            TransitionStrategy::DoNothing => {
                // do nothing
            }
            TransitionStrategy::NewlineAndRenew => {
                writer.write_str("\n")?;
                Self::renew_nonnesting_sequence_line_start(writer, context, &[])?;
            }
            TransitionStrategy::ExtraNewlineAndRenew => {
                writer.write_str("\n")?;
                Self::renew_nonnesting_sequence_line_start(writer, context, &[])?;
                writer.write_str("\n")?;
                Self::renew_nonnesting_sequence_line_start(writer, context, &[])?;
            }
        }
        Ok(())
    }
    fn process_enter_nesting(
        writer: &mut W,
        context: &mut Vec<Event<'a>>,
        sequence: &mut VecDeque<Event<'a>>,
    ) -> io::Result<()> {
        while let Some(event) = sequence.pop_front() {
            match &event {
                Event::Rule => {
                    if let Some(Event::Start(Tag::Item)) = context.last() {
                        writer.write_str("---")?;
                    } else {
                        writer.write_str("***")?;
                    }
                }
                Event::Html(html_text) => {
                    writer.write_str(&**html_text)?;
                    if html_text.ends_with("\n") {
                        Self::renew_nonnesting_sequence_line_start(writer, context, &[])?;
                    }
                }
                Event::Text(text) => {
                    if let Some(Event::Start(Tag::CodeBlock(_))) = context.last() {
                        writer.write_str(&**text)?;
                    } else {
                        writer.write_str(&*escape_text(text, EscapeOptions::default()))?;
                    }
                }
                Event::Start(tag) => {
                    match tag {
                        Tag::Paragraph | Tag::List(_) => {
                            // do nothing,
                        }
                        Tag::Heading(level) => {
                            let level_str = &"#######"[..(*level) as usize];
                            writer.write_str(level_str)?;
                            writer.write_str(" ")?;
                        }
                        Tag::BlockQuote => {
                            writer.write_str("> ")?;
                        }
                        Tag::CodeBlock(CodeBlockKind::Indented) => {
                            writer.write_str("    ")?;
                        }
                        Tag::CodeBlock(CodeBlockKind::Fenced(str)) => {
                            writer.write_str("````")?;
                            writer.write_str(str)?;
                            writer.write_str("\n")?;
                            Self::renew_nonnesting_sequence_line_start(writer, context, &[])?;
                        }
                        Tag::Item => match context.last_mut() {
                            Some(Event::Start(Tag::List(style))) => {
                                if let Some(idx) = style {
                                    let str = format!("{}. ", idx);
                                    writer.write_str(&str)?;
                                    *idx += 1;
                                } else {
                                    writer.write_str("* ")?;
                                }
                            }
                            _ => {
                                eprintln!(
                                    "item encountered but list context not found: {:?}",
                                    context
                                );
                            }
                        },
                        _ => {
                            eprintln!(
                                "unhandled enter nesting event {:?}, remaining: {:?}",
                                event, sequence
                            );
                        }
                    }
                }
                _ => {
                    eprintln!(
                        "unhandled enter nesting event {:?}, remaining: {:?}",
                        event, sequence
                    );
                }
            }
            context.push(event);
        }
        Ok(())
    }
    fn process_exit_nesting(
        writer: &mut W,
        context: &[Event<'a>],
        sequence: &[Event<'a>],
    ) -> io::Result<()> {
        for idx in (0..sequence.len()).rev() {
            let event = &sequence[idx];
            let remaining_sequence = &sequence[0..idx];
            match event {
                Event::Rule => {
                    // do nothing
                }
                Event::Html(_) => {
                    // do nothing
                }
                Event::Text(_) => {
                    // do nothing
                    // FIXME: This is the tight list case.
                }
                Event::Start(tag) => {
                    match tag {
                        Tag::Paragraph
                        | Tag::Heading(_)
                        | Tag::BlockQuote
                        | Tag::Item
                        | Tag::List(_) => {
                            // do nothing
                        }
                        Tag::CodeBlock(CodeBlockKind::Indented) => {
                            // do nothing
                        }
                        Tag::CodeBlock(CodeBlockKind::Fenced(_)) => {
                            writer.write_str("\n")?;
                            Self::renew_nonnesting_sequence_line_start(
                                writer,
                                context,
                                remaining_sequence,
                            )?;
                            writer.write_str("````")?;
                        }
                        _ => {
                            eprintln!("unhandled exit nesting event {:?}", sequence);
                        }
                    }
                }
                _ => {
                    eprintln!("unhandled exit nesting event {:?}", sequence);
                }
            }
        }
        Ok(())
    }
    fn renew_nonnesting_sequence_line_start(
        writer: &mut W,
        context: &[Event<'a>],
        context2: &[Event<'a>],
    ) -> io::Result<()> {
        let mut remaining_context = context.iter().chain(context2.iter()).peekable();
        while let Some(event) = remaining_context.next() {
            match event {
                Event::Start(tag) => {
                    match tag {
                        Tag::Paragraph => {
                            // do nothing
                        }
                        Tag::Heading(_) => {
                            eprintln!("unhandled renew context at new line {:?}", context);
                        }
                        Tag::CodeBlock(CodeBlockKind::Indented) => {
                            writer.write_str("    ")?;
                        }
                        Tag::CodeBlock(CodeBlockKind::Fenced(_)) => {
                            // do nothing
                        }
                        Tag::BlockQuote => {
                            writer.write_str("> ")?;
                        }
                        Tag::List(None) => {
                            if let Some(Event::Start(Tag::Item)) = remaining_context.peek() {
                                let _ = remaining_context.next();
                                writer.write_str("  ")?;
                            }
                        }
                        Tag::List(Some(idx)) => {
                            if let Some(Event::Start(Tag::Item)) = remaining_context.peek() {
                                let _ = remaining_context.next();
                                let str = format!("{}. ", idx - 1)
                                    .chars()
                                    .map(|_| ' ')
                                    .collect::<String>();
                                writer.write_str(&str)?;
                            }
                        }
                        Tag::Item => {
                            unreachable!();
                        }
                        _ => {
                            eprintln!("unhandled renew context at new line {:?}", context);
                        }
                    }
                }
                Event::Text(_) => {
                    // do nothing
                }
                _ => {
                    eprintln!("unhandled renew context at new line {:?}", context);
                }
            }
        }
        Ok(())
    }
    fn process_nonnesting_sequence(
        writer: &mut W,
        context: &[Event<'a>],
        sequence: &[Event<'a>],
    ) -> io::Result<()> {
        let mut iter = sequence.iter().peekable();
        while let Some(event) = iter.peek() {
            if let Event::Text(text) = event {
                if let Some(Event::Start(Tag::CodeBlock(_))) = context.last() {
                    writer.write_str(&**text)?;
                } else if let Some(Event::Start(Tag::Heading(_))) = context.last() {
                    writer.write_str(&*escape_text(text, EscapeOptions::ESCAPE_LINEFEED))?;
                } else {
                    writer.write_str(&*escape_text(text, EscapeOptions::default()))?;
                }
                if text.ends_with("\n") {
                    Self::renew_nonnesting_sequence_line_start(writer, context, &[])?;
                }
                let _ = iter.next();
            } else if let Event::SoftBreak = event {
                if let Some(Event::Start(Tag::Heading(_))) = context.last() {
                    writer
                        .write_str(&*escape_text(&'\n'.into(), EscapeOptions::ESCAPE_LINEFEED))?;
                } else {
                    writer.write_str("\n")?;
                    Self::renew_nonnesting_sequence_line_start(writer, context, &[])?;
                }
                let _ = iter.next();
            } else if let Event::HardBreak = event {
                writer.write_str("\\\n")?;
                Self::renew_nonnesting_sequence_line_start(writer, context, &[])?;
                let _ = iter.next();
            } else if let Event::Code(str) = event {
                writer.write_str("`")?;
                writer.write_str(str)?;
                writer.write_str("`")?;
                let _ = iter.next();
            } else if let Event::Start(Tag::Emphasis) = event {
                writer.write_str("*")?;
                let _ = iter.next();
            } else if let Event::End(Tag::Emphasis) = event {
                writer.write_str("*")?;
                let _ = iter.next();
            } else if let Event::Start(Tag::Strong) = event {
                writer.write_str("**")?;
                let _ = iter.next();
            } else if let Event::End(Tag::Strong) = event {
                writer.write_str("**")?;
                let _ = iter.next();
            } else if let Event::Html(html_str) = event {
                writer.write_str(&**html_str)?;
                let _ = iter.next();
            } else if let Event::Start(Tag::Link(kind, _, _)) = event {
                // FIXME
                match kind {
                    LinkType::Autolink | LinkType::Email => {
                        writer.write_str("<")?;
                    }
                    _ => {
                        writer.write_str("[")?;
                    }
                }
                let _ = iter.next();
            } else if let Event::End(Tag::Link(kind, target, title)) = event {
                // FIXME
                match kind {
                    LinkType::Autolink | LinkType::Email => {
                        writer.write_str(">")?;
                    }
                    _ => {
                        writer.write_str("](")?;
                        writer.write_str(target)?;
                        if !title.is_empty() {
                            writer.write_str(" \"")?;
                            writer.write_str(title)?;
                            writer.write_str("\"")?;
                        }
                        writer.write_str(")")?;
                    }
                }
                let _ = iter.next();
            } else if let Event::Start(Tag::Image(_, _, _)) = event {
                // FIXME
                writer.write_str("![")?;
                let _ = iter.next();
            } else if let Event::End(Tag::Image(_, target, title)) = event {
                // FIXME
                writer.write_str("](")?;
                writer.write_str(target)?;
                if !title.is_empty() {
                    writer.write_str(" \"")?;
                    writer.write_str(title)?;
                    writer.write_str("\"")?;
                }
                writer.write_str(")")?;
                let _ = iter.next();
            } else {
                eprintln!("unhandled output event {:?}", event);
                let _ = iter.next();
            }
        }
        Ok(())
    }
}

/// Iterate over an `Iterator` of `Event`s, generate HTML for each `Event`, and
/// push it to a `String`.
pub fn push_markdown<'a, I>(s: &mut String, iter: I)
where
    I: Iterator<Item = Event<'a>>,
{
    MarkdownWriter::new(iter, s).run().unwrap();
}

/// Iterate over an `Iterator` of `Event`s, generate Markdown for each `Event`, and
/// write it out to a writable stream.
///
/// **Note**: using this function with an unbuffered writer like a file or socket
/// will result in poor performance. Wrap these in a
/// [`BufWriter`](https://doc.rust-lang.org/std/io/struct.BufWriter.html) to
/// prevent unnecessary slowdowns.
pub fn write_markdown<'a, I, W>(writer: W, iter: I) -> io::Result<()>
where
    I: Iterator<Item = Event<'a>>,
    W: Write,
{
    MarkdownWriter::new(iter, WriteWrapper(writer)).run()
}
