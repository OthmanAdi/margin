//! Following a JSONL file another process is actively appending to.
//!
//! Two things make this less obvious than it looks on Windows.
//!
//! The file is open for writing by the harness. Rust's default share mode already permits
//! reading it, so the read path uses a plain `File` with no share-mode calls and no locking.
//! Adding either is how this breaks.
//!
//! The last line is routinely half-written. A chunk that does not end in a newline is a
//! write in progress, not corruption: the committed offset simply stops before it, so it is
//! re-read whole on the next wake-up. Treating a torn line as an error would drop a record
//! that is about to be perfectly fine a millisecond later.
//!
//! Note it is re-read rather than buffered. Doing both, which was the first attempt, feeds
//! those bytes through twice and produces lines like `{"a":{"a":2}`. Since a rating anchors
//! to a parsed line, a duplicate is not cosmetic: it is a second card the user can rate that
//! refers to the same moment.

use anyhow::{Context, Result};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub struct Tailer {
    path: PathBuf,
    /// Byte offset of the start of the first incomplete line. Everything before this has
    /// been emitted; everything from here on is re-read next time.
    offset: u64,
}

impl Tailer {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into(), offset: 0 }
    }

    /// Start at the end, for attaching to a session already in progress without replaying
    /// everything that came before.
    pub fn from_end(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        Ok(Self { path, offset: len })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Complete lines appended since the last call. Empty when nothing new, which is the
    /// common case and must stay cheap.
    pub fn poll(&mut self) -> Result<Vec<String>> {
        let Ok(meta) = std::fs::metadata(&self.path) else {
            return Ok(Vec::new()); // not created yet; a session can be watched before it starts
        };
        let len = meta.len();

        // Truncated or replaced: a session was cleared, or we are looking at a different
        // file under the same name. Re-read from the top rather than seeking past the end.
        if len < self.offset {
            self.offset = 0;
        }
        if len == self.offset {
            return Ok(Vec::new());
        }

        let file = File::open(&self.path)
            .with_context(|| format!("opening {}", self.path.display()))?;
        let mut reader = BufReader::new(file);
        reader.seek(SeekFrom::Start(self.offset))?;

        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;

        let mut lines = Vec::new();
        let mut consumed = 0usize;
        for chunk in split_inclusive_newline(&buf) {
            // A chunk without a trailing newline is a write in progress. Leave the offset
            // before it and it will be re-read, whole, on the next poll. Stashing it here
            // as well would duplicate those bytes on the next read.
            if !chunk.ends_with(b"\n") {
                break;
            }
            consumed += chunk.len();
            let line = strip_eol(chunk);
            if !line.is_empty() {
                // Lossy rather than skipping: a multi-byte character split across two
                // writes should not silently drop an otherwise good record.
                lines.push(String::from_utf8_lossy(line).into_owned());
            }
        }

        self.offset += consumed as u64;
        Ok(lines)
    }

    /// How far into the file we have committed. Exposed for tests and diagnostics.
    pub fn offset(&self) -> u64 {
        self.offset
    }
}

fn split_inclusive_newline(buf: &[u8]) -> impl Iterator<Item = &[u8]> {
    buf.split_inclusive(|b| *b == b'\n')
}

fn strip_eol(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    if end > 0 && line[end - 1] == b'\n' {
        end -= 1;
    }
    if end > 0 && line[end - 1] == b'\r' {
        end -= 1;
    }
    &line[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("margin-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn append(path: &Path, s: &str) {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        f.write_all(s.as_bytes()).unwrap();
        f.flush().unwrap();
    }

    fn append_bytes(path: &Path, b: &[u8]) {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        f.write_all(b).unwrap();
        f.flush().unwrap();
    }

    #[test]
    fn reads_only_what_is_new_each_time() {
        let p = tmp_file("incremental.jsonl");
        let _ = std::fs::remove_file(&p);
        let mut t = Tailer::new(&p);

        append(&p, "{\"a\":1}\n{\"a\":2}\n");
        assert_eq!(t.poll().unwrap(), vec!["{\"a\":1}", "{\"a\":2}"]);
        assert!(t.poll().unwrap().is_empty(), "a second poll re-read old lines");

        append(&p, "{\"a\":3}\n");
        assert_eq!(t.poll().unwrap(), vec!["{\"a\":3}"]);
        std::fs::remove_file(&p).ok();
    }

    /// The case that makes tailing a live file different from reading a file.
    #[test]
    fn a_half_written_line_is_held_back_then_completed() {
        let p = tmp_file("torn.jsonl");
        let _ = std::fs::remove_file(&p);
        let mut t = Tailer::new(&p);

        append(&p, "{\"a\":1}\n{\"a\":");
        assert_eq!(t.poll().unwrap(), vec!["{\"a\":1}"], "torn tail must not be emitted");

        append(&p, "2}\n");
        assert_eq!(t.poll().unwrap(), vec!["{\"a\":2}"], "completed line must arrive whole");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn handles_crlf_and_blank_lines() {
        let p = tmp_file("crlf.jsonl");
        let _ = std::fs::remove_file(&p);
        let mut t = Tailer::new(&p);

        append(&p, "{\"a\":1}\r\n\n{\"a\":2}\r\n");
        assert_eq!(t.poll().unwrap(), vec!["{\"a\":1}", "{\"a\":2}"]);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn a_truncated_file_is_re_read_rather_than_seeking_past_the_end() {
        let p = tmp_file("truncate.jsonl");
        let _ = std::fs::remove_file(&p);
        let mut t = Tailer::new(&p);

        append(&p, "{\"a\":1}\n{\"a\":2}\n");
        assert_eq!(t.poll().unwrap().len(), 2);

        std::fs::write(&p, "{\"b\":1}\n").unwrap();
        assert_eq!(t.poll().unwrap(), vec!["{\"b\":1}"]);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn a_file_that_does_not_exist_yet_is_not_an_error() {
        let mut t = Tailer::new(tmp_file("never-created.jsonl"));
        assert!(t.poll().unwrap().is_empty());
    }

    #[test]
    fn from_end_skips_existing_content() {
        let p = tmp_file("from-end.jsonl");
        std::fs::write(&p, "{\"old\":1}\n").unwrap();
        let mut t = Tailer::from_end(&p).unwrap();
        assert!(t.poll().unwrap().is_empty(), "existing lines should not be replayed");

        append(&p, "{\"new\":1}\n");
        assert_eq!(t.poll().unwrap(), vec!["{\"new\":1}"]);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn a_multibyte_char_split_across_two_writes_survives() {
        let p = tmp_file("split-utf8.jsonl");
        let _ = std::fs::remove_file(&p);
        let mut t = Tailer::new(&p);

        // "é" is the two bytes 0xC3 0xA9. Write them in separate appends, with the newline
        // only in the second, so the first poll sees a genuinely invalid UTF-8 tail.
        append_bytes(&p, b"{\"t\":\"caf\xC3");
        assert!(t.poll().unwrap().is_empty(), "an incomplete line must not be emitted");

        append_bytes(&p, b"\xA9\"}\n");
        assert_eq!(t.poll().unwrap(), vec!["{\"t\":\"caf\u{00E9}\"}"], "the character must reassemble");
        std::fs::remove_file(&p).ok();
    }

    /// A rating anchors to a parsed line, so re-reading must never emit a line twice.
    #[test]
    fn no_line_is_ever_emitted_twice_across_many_torn_writes() {
        let p = tmp_file("no-dupes.jsonl");
        let _ = std::fs::remove_file(&p);
        let mut t = Tailer::new(&p);

        let mut seen = Vec::new();
        for i in 0..20 {
            // split every record across two appends, mid-token
            append(&p, &format!("{{\"n\":{i},\"half\":"));
            seen.extend(t.poll().unwrap());
            append(&p, "true}\n");
            seen.extend(t.poll().unwrap());
        }

        assert_eq!(seen.len(), 20, "expected exactly one line per record, got {}", seen.len());
        for (i, line) in seen.iter().enumerate() {
            assert_eq!(line, &format!("{{\"n\":{i},\"half\":true}}"));
        }
        std::fs::remove_file(&p).ok();
    }
}
