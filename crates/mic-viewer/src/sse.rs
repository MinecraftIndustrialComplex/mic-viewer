// SPDX-License-Identifier: MPL-2.0
//! The server-sent events writer, from Deep Time's `stream_events` (NOTICE.md): the server has
//! written the headers and `retry: 2000`; a stream sends named events and, when it has nothing to
//! say, a comment every [`PING`] (which also notices a page that went away).

use std::io::{self, Write};
use std::time::Duration;

/// A stream with nothing to say sends a comment this often.
pub const PING: Duration = Duration::from_secs(15);

pub struct SseWriter<'a> {
    out: &'a mut dyn Write,
}

impl<'a> SseWriter<'a> {
    pub fn new(out: &'a mut dyn Write) -> SseWriter<'a> {
        SseWriter { out }
    }

    /// One event. `data` must not hold a newline (JSON from serde never does).
    pub fn send(&mut self, event: &str, data: &str) -> io::Result<()> {
        debug_assert!(!data.contains('\n'));
        self.out.write_all(format!("event: {event}\ndata: {data}\n\n").as_bytes())?;
        self.out.flush()
    }

    /// Several events in one write.
    pub fn send_all<'b>(&mut self, msgs: impl IntoIterator<Item = (&'b str, &'b str)>) -> io::Result<()> {
        let mut buf = Vec::new();
        for (event, data) in msgs {
            buf.extend_from_slice(format!("event: {event}\ndata: {data}\n\n").as_bytes());
        }
        self.out.write_all(&buf)?;
        self.out.flush()
    }

    /// A comment line, the keep-alive.
    pub fn ping(&mut self) -> io::Result<()> {
        self.out.write_all(b": ping\n\n")?;
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames() {
        let mut buf = Vec::new();
        {
            let mut w = SseWriter::new(&mut buf);
            w.send("hello", "{\"a\":1}").unwrap();
            w.ping().unwrap();
            w.send_all([("frame", "{}"), ("events", "[]")]).unwrap();
        }
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "event: hello\ndata: {\"a\":1}\n\n: ping\n\nevent: frame\ndata: {}\n\nevent: events\ndata: []\n\n"
        );
    }
}
