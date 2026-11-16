//! What crosses the app's socket: one request line, one reply line.
//!
//! A connection carries exactly one call. That keeps the listener stateless
//! (nothing to authenticate once and trust later) and makes an app that quit
//! between two calls a plain connection error rather than a stale stream.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The most a request may be. A call is a name and a couple of arguments.
pub const MAX_REQUEST_BYTES: usize = 16 * 1024;

/// The most a reply may be. The answers are bounded well below this
/// (`answers::MAX_ANSWER_CHARS`); the cap is for a peer that is not the app.
pub const MAX_REPLY_BYTES: usize = 1024 * 1024;

/// One call, as the server sends it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// The thread the server speaks for.
    pub thread: String,
    /// That thread's token.
    pub token: String,
    pub tool: String,
    #[serde(default)]
    pub arguments: Value,
}

/// The app's answer: the text the model reads, and whether it is an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub ok: bool,
    pub text: String,
}

impl Reply {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            ok: true,
            text: text.into(),
        }
    }

    pub fn refused(text: impl Into<String>) -> Self {
        Self {
            ok: false,
            text: text.into(),
        }
    }

    /// The reply as one line, newline included.
    pub fn to_line(&self) -> Vec<u8> {
        let mut line = serde_json::to_vec(self).unwrap_or_else(|_| b"{}".to_vec());
        line.push(b'\n');
        line
    }
}

impl Request {
    /// The request as one line, newline included.
    pub fn to_line(&self) -> Vec<u8> {
        let mut line = serde_json::to_vec(self).unwrap_or_else(|_| b"{}".to_vec());
        line.push(b'\n');
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_messages_are_one_line_that_reads_back() {
        let request = Request {
            thread: "t".into(),
            token: "k".into(),
            tool: "read_thread".into(),
            arguments: serde_json::json!({"thread_id": "a\nb"}),
        };
        let line = request.to_line();
        assert_eq!(line.iter().filter(|byte| **byte == b'\n').count(), 1);
        assert_eq!(
            serde_json::from_slice::<Request>(&line[..line.len() - 1]).unwrap(),
            request
        );
        let reply = Reply::ok("line one\nline two");
        let line = reply.to_line();
        assert_eq!(line.iter().filter(|byte| **byte == b'\n').count(), 1);
        assert_eq!(
            serde_json::from_slice::<Reply>(&line[..line.len() - 1]).unwrap(),
            reply
        );
    }
}
