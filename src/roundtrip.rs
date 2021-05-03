#![allow(dead_code, unused_mut)]
use crate::escape::{StrWrite, WriteWrapper};
use crate::CodeBlockKind;
use crate::{Event, Tag};
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
enum tri_bool {
    r#true,
    r#false,
    maybe,
}

fn is_childless_block(event: &Event<'_>) -> tri_bool {
    if matches!(event, Event::Rule) {
        tri_bool::r#true
    } else if matches!(event, Event::Html(_)) {
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
        context.last().map_or(true, is_leaf_block_start)
    } else {
        false
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
        let mut stack = vec![];
        let mut incoming_stack = vec![];
        let mut outgoing_counter = 0;
        let mut iter = self.iter.peekable();
        /// In general, we split markdown generation into a sequence of four actions
        /// 1. Encountering a series of starts of block containers
        /// 2. Encountering a series of inlines
        /// 3. Encountering one leaf block
        /// 4. Encountering a series of endings of block containers
        /// 5. Transition over the valley between block containers
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
                            let context = if state == 4 {
                                let remaining_stack_len =
                                    stack.len().checked_sub(outgoing_counter).unwrap();
                                &stack[0..remaining_stack_len]
                            } else {
                                &stack
                            };
                            if is_childless_block_2(context, event) {
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
                        incoming_stack.push(iter.next().unwrap());
                    }
                    new_state = 0;
                    continue 'seq;
                } else {
                    if state == 4 && new_state == 1 {
                        // eprintln!("dbg: prepare transition {} => {}", state, new_state);
                        state = 1;
                        incoming_stack.push(iter.next().unwrap());
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
                        &incoming_stack,
                    )?;
                }
                Self::process_enter_nesting(&mut self.writer, &stack, &incoming_stack)?;
                stack.drain(remaining_stack_len..);
                stack.extend(incoming_stack.drain(..));
            } else if state == 2 {
                Self::process_nonnesting_sequence(&mut self.writer, &stack, &incoming_stack)?;
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
                incoming_stack.push(iter.next().unwrap());
            } else if new_state == 3 {
                incoming_stack.push(iter.next().unwrap());
                if state == 4 {
                    let remaining_stack_len = stack.len().checked_sub(outgoing_counter).unwrap();
                    Self::process_transition(
                        &mut self.writer,
                        &stack[0..remaining_stack_len],
                        &stack[remaining_stack_len..],
                        &incoming_stack,
                    )?;
                }
                outgoing_counter = 0;
                Self::process_enter_nesting(&mut self.writer, &stack, &incoming_stack)?;
                stack.extend(incoming_stack.drain(..));
                outgoing_counter += 1;
                new_state = 4;
            } else if new_state == 4 {
                outgoing_counter = 0;
                let _ = iter.next();
                outgoing_counter += 1;
            } else {
                outgoing_counter = 0;
                return Ok(());
            }
            state = new_state;
        }
    }

    fn process_transition(
        writer: &mut W,
        context: &[Event<'_>],
        removing_sequence: &[Event<'_>],
        added_sequence: &[Event<'_>],
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
                _ => {
                }
            }
        }

        if strategy.is_none() {
            match removing_sequence {
                [Event::Start(Tag::List(_)), ..] => {
                    strategy = Some(TransitionStrategy::ExtraNewlineAndRenew);
                }
                _ => {
                }
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
                Self::renew_nonnesting_sequence_line_start(writer, context)?;
            }
            TransitionStrategy::ExtraNewlineAndRenew => {
                writer.write_str("\n")?;
                Self::renew_nonnesting_sequence_line_start(writer, context)?;
                writer.write_str("\n")?;
                Self::renew_nonnesting_sequence_line_start(writer, context)?;
            }
        }
        Ok(())
    }
    fn process_enter_nesting(
        writer: &mut W,
        context: &[Event<'_>],
        sequence: &[Event<'_>],
    ) -> io::Result<()> {
        if sequence.is_empty() {
            return Ok(());
        }
        if let [Event::Rule] = sequence {
            writer.write_str("***")?;
        } else if let [Event::Html(html_text)] = sequence {
            writer.write_str(&**html_text)?;
        } else if let [Event::Start(tag)] = sequence {
            match tag {
                Tag::Paragraph => {
                    // do nothing,
                }
                Tag::Heading(level) => {
                    let level_str = &"#######"[..(*level) as usize];
                    writer.write_str(level_str)?;
                    writer.write_str(" ")?;
                }
                Tag::CodeBlock(CodeBlockKind::Indented) => {
                    writer.write_str("    ")?;
                }
                Tag::CodeBlock(CodeBlockKind::Fenced(str)) => {
                    writer.write_str("````")?;
                    writer.write_str(str)?;
                    writer.write_str("\n")?;
                }
                _ => {
                    eprintln!("unhandled enter nesting event {:?}", sequence);
                }
            }
        } else if let [Event::Start(tag1), Event::Start(tag2)] = sequence {
            match (tag1, tag2) {
                (Tag::List(None), Tag::Item) => {
                    writer.write_str("* ")?;
                }
                (Tag::List(Some(idx)), Tag::Item) => {
                    let str = format!("{}. ", idx);
                    writer.write_str(&str)?;
                }
                (Tag::BlockQuote, Tag::Paragraph) => {
                    writer.write_str(">")?;
                }
                _ => {
                    eprintln!("unhandled enter nesting event {:?}", sequence);
                }
            }
        } else {
            eprintln!("unhandled enter nesting event {:?}", sequence);
        }
        Ok(())
    }
    fn process_exit_nesting(
        writer: &mut W,
        context: &[Event<'_>],
        sequence: &[Event<'_>],
    ) -> io::Result<()> {
        if sequence.is_empty() {
            return Ok(());
        }
        if let [Event::Start(tag)] = sequence {
            match tag {
                Tag::Paragraph => {
                    // do nothing
                }
                Tag::CodeBlock(CodeBlockKind::Indented) => {
                    // do nothing
                }
                Tag::CodeBlock(CodeBlockKind::Fenced(str)) => {
                    writer.write_str("````")?;
                }
                _ => {
                    eprintln!("unhandled exit nesting event {:?}", sequence);
                }
            }
        } else {
            eprintln!("unhandled exit nesting event {:?}", sequence);
        }
        Ok(())
    }
    fn renew_nonnesting_sequence_line_start(
        writer: &mut W,
        context: &[Event<'_>],
    ) -> io::Result<()> {
        if context.is_empty() {
            return Ok(());
        }
        if let [Event::Start(tag)] = context {
            match tag {
                Tag::Paragraph => {
                    // do nothing,
                }
                Tag::CodeBlock(CodeBlockKind::Indented) => {
                    writer.write_str("    ")?;
                }
                Tag::List(None) => {
                    writer.write_str("* ")?;
                }
                _ => {
                    eprintln!("unhandled renew context at new line {:?}", context);
                }
            }
        } else if let [Event::Start(tag1), Event::Start(tag2), Event::Start(tag3), Event::Start(tag4)] = context {
            match (tag1, tag2, tag3, tag4) {
                (Tag::List(None), Tag::Item, Tag::List(None), Tag::Item) => {
                    writer.write_str("  * ")?;
                }
                _ => {
                    eprintln!("unhandled renew context at new line {:?}", context);
                }
            }
        } else {
            eprintln!("unhandled renew context at new line {:?}", context);
        }
        Ok(())
    }
    fn process_nonnesting_sequence(
        writer: &mut W,
        context: &[Event<'_>],
        sequence: &[Event<'_>],
    ) -> io::Result<()> {
        let mut iter = sequence.iter().peekable();
        while let Some(event) = iter.peek() {
            if let Event::Text(text) = event {
                writer.write_str(&**text)?;
                if text.ends_with("\n") {
                    Self::renew_nonnesting_sequence_line_start(writer, context)?;
                }
                let _ = iter.next();
            } else if let Event::SoftBreak = event {
                writer.write_str("\n")?;
                Self::renew_nonnesting_sequence_line_start(writer, context)?;
                let _ = iter.next();
            } else if let Event::HardBreak = event {
                writer.write_str("\\\n")?;
                Self::renew_nonnesting_sequence_line_start(writer, context)?;
                let _ = iter.next();
            } else if let Event::Code(str) = event {
                writer.write_str("`")?;
                writer.write_str(str)?;
                writer.write_str("`")?;
                let _ = iter.next();
            } else if let Event::Start(Tag::Emphasis) = event {
                writer.write_str("_")?;
                let _ = iter.next();
            } else if let Event::End(Tag::Emphasis) = event {
                writer.write_str("_")?;
                let _ = iter.next();
            } else if let Event::Start(Tag::Strong) = event {
                writer.write_str("**")?;
                let _ = iter.next();
            } else if let Event::End(Tag::Strong) = event {
                writer.write_str("**")?;
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
