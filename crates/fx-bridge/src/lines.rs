//! Newline-delimited reading with a cap.
//!
//! Both ends of the bridge read lines from a peer they do not control: the
//! server from its provider, the listener from whatever connects. A reader
//! that buffers until a newline buffers without bound when none comes, so a
//! line past the cap is skipped to its end and reported, never kept.

use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// One read.
#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    /// A line, without its newline. A last line with no newline counts.
    Data(Vec<u8>),
    /// A line longer than the cap, read to its end and dropped.
    TooLong,
    /// The input ended.
    End,
}

/// Read the next line, keeping at most `max` bytes of it.
pub async fn read_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max: usize,
) -> std::io::Result<Line> {
    let mut line = Vec::new();
    let mut overflowed = false;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(if overflowed {
                Line::TooLong
            } else if line.is_empty() {
                Line::End
            } else {
                Line::Data(line)
            });
        }
        let (taken, ended) = match available.iter().position(|byte| *byte == b'\n') {
            Some(at) => (at, true),
            None => (available.len(), false),
        };
        if !overflowed {
            if line.len() + taken > max {
                overflowed = true;
                line = Vec::new();
            } else {
                line.extend_from_slice(&available[..taken]);
            }
        }
        reader.consume(taken + usize::from(ended));
        if ended {
            if overflowed {
                return Ok(Line::TooLong);
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(Line::Data(line));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn all(input: &[u8], max: usize) -> Vec<Line> {
        // A small buffer, so a line arrives in pieces as it would off a pipe.
        let mut reader = tokio::io::BufReader::with_capacity(3, input);
        let mut lines = Vec::new();
        loop {
            let line = read_line(&mut reader, max).await.unwrap();
            if line == Line::End {
                return lines;
            }
            lines.push(line);
        }
    }

    #[tokio::test]
    async fn lines_are_split_at_newlines_whatever_the_chunks() {
        assert_eq!(
            all(b"one\ntwo\r\nlast", 16).await,
            [
                Line::Data(b"one".to_vec()),
                Line::Data(b"two".to_vec()),
                Line::Data(b"last".to_vec())
            ]
        );
    }

    #[tokio::test]
    async fn a_line_past_the_cap_is_skipped_and_the_next_one_is_read() {
        assert_eq!(
            all(b"0123456789abcdef\nok\n0123456789", 8).await,
            [Line::TooLong, Line::Data(b"ok".to_vec()), Line::TooLong]
        );
    }

    #[tokio::test]
    async fn an_empty_line_is_a_line_and_an_empty_input_is_the_end() {
        assert_eq!(all(b"\n", 4).await, [Line::Data(Vec::new())]);
        assert_eq!(all(b"", 4).await, []);
    }
}
