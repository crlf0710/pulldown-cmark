#![allow(dead_code, unused_mut)]
use crate::escape::{StrWrite, WriteWrapper};
use crate::{Event, Tag};
use std::io::{self, Write};

struct MarkdownWriter<'a, I, W> {
    /// Iterator supplying events.
    iter: I,

    /// Writer to write to.
    writer: W,

    phantom: core::marker::PhantomData<&'a ()>,
}

fn is_block_nesting_start(event: &Event<'_>) -> bool {
    if let Event::Start(tag) = event {
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
    } else {
        false
    }
}

fn is_block_nesting_end(event: &Event<'_>) -> bool {
    if let Event::Start(tag) = event {
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
        /// 1. Encountering a series of block element starts
        /// 2. Encountering a series of inline elements
        /// 3. Encountering a series of block element ends
        /// 4. Transition over the valley between block elements
        let mut state = 0;
        loop {
            let mut new_state = 0;
            'seq: while let Some(event) = iter.peek() {
                if is_block_nesting_start(event) {
                    new_state = 1;
                } else if is_block_nesting_end(event) {
                    new_state = 3;
                } else {
                    new_state = 2;
                }

                if new_state == state {
                    if state == 3 {
                        let _ = iter.next();
                        outgoing_counter += 1;
                    } else {
                        incoming_stack.push(iter.next().unwrap());
                    }
                    continue 'seq;
                } else {
                    if state == 3 && new_state == 1 {
                        state = 1;
                        incoming_stack.push(iter.next().unwrap());
                        continue 'seq;
                    } else {
                        break 'seq;
                    }
                }
            }
            // The state is preparing to change.
            // Let's check what's the previous state first.
            if state == 0 {
                if new_state == 0 {
                    // empty file
                    let _ = iter.next();
                    return Ok(());
                } else if new_state == 1 {
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
                    );
                }
                Self::process_enter_nesting(&mut self.writer, &stack, &incoming_stack);
                stack.drain(remaining_stack_len..);
                stack.extend(incoming_stack.drain(..));
            } else if state == 2 {
                Self::process_nonnesting_sequence(&mut self.writer, &stack, &incoming_stack);
            } else if state == 3 {
                let remaining_stack_len = stack.len().checked_sub(outgoing_counter).unwrap();
                Self::process_exit_nesting(
                    &mut self.writer,
                    &stack[0..remaining_stack_len],
                    &stack[remaining_stack_len..],
                );
            }
            incoming_stack.clear();
            outgoing_counter = 0;
            if new_state == 1 || new_state == 2 {
                incoming_stack.push(iter.next().unwrap());
            } else if new_state == 3 {
                let _ = iter.next();
                outgoing_counter += 1;
            } else {
                return Ok(());
            }
        }
    }

    fn process_transition(
        writer: &mut W,
        context: &[Event<'_>],
        removing_sequence: &[Event<'_>],
        added_sequence: &[Event<'_>],
    ) {
    }
    fn process_enter_nesting(writer: &mut W, context: &[Event<'_>], sequence: &[Event<'_>]) {}
    fn process_exit_nesting(writer: &mut W, context: &[Event<'_>], sequence: &[Event<'_>]) {}
    fn process_nonnesting_sequence(writer: &mut W, context: &[Event<'_>], sequence: &[Event<'_>]) {}
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
