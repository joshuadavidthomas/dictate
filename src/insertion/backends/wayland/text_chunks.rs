use std::collections::VecDeque;

use crate::insertion::InsertionText;

pub(super) const MAX_COMMIT_STRING_BYTES: usize = 4000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CommitChunk {
    pub(super) chunk: String,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct CommitBatch {
    pub(super) current: CommitChunk,
    pub(super) remaining: VecDeque<CommitChunk>,
}

#[derive(Debug)]
pub(super) struct CommitChunks {
    chunks: VecDeque<CommitChunk>,
}

impl CommitChunks {
    pub(super) fn from_text(text: InsertionText<'_>) -> Self {
        let text = text.as_str();
        let mut chunks = Vec::new();
        let mut start = 0;
        let mut bytes = 0;

        for (index, character) in text.char_indices() {
            let character_bytes = character.len_utf8();
            if bytes + character_bytes > MAX_COMMIT_STRING_BYTES {
                chunks.push(text[start..index].to_owned());
                start = index;
                bytes = 0;
            }
            bytes += character_bytes;
        }

        if start < text.len() {
            chunks.push(text[start..].to_owned());
        }

        Self::from_validated_chunks(chunks)
    }

    fn from_validated_chunks(chunks: impl IntoIterator<Item = String>) -> Self {
        let chunks = chunks
            .into_iter()
            .map(|chunk| {
                assert!(
                    chunk.len() <= MAX_COMMIT_STRING_BYTES,
                    "input-method commit chunk exceeds Wayland byte limit"
                );
                CommitChunk { chunk }
            })
            .collect::<VecDeque<_>>();
        assert!(
            !chunks.is_empty(),
            "input-method insertion requires at least one commit chunk"
        );
        Self { chunks }
    }

    pub(super) fn len(&self) -> usize {
        self.chunks.len()
    }

    #[cfg(test)]
    pub(super) fn from_test_chunks(chunks: impl IntoIterator<Item = String>) -> Self {
        Self::from_validated_chunks(chunks)
    }

    #[cfg(test)]
    fn to_strings(&self) -> Vec<String> {
        self.chunks
            .iter()
            .map(|chunk| chunk.chunk.clone())
            .collect()
    }
}

#[derive(Debug)]
pub(super) struct ChunkQueue {
    chunks: VecDeque<CommitChunk>,
    maybe_sent_bytes: usize,
    sent_bytes: usize,
}

impl ChunkQueue {
    pub(super) fn new(chunks: CommitChunks) -> Self {
        Self {
            chunks: chunks.chunks,
            maybe_sent_bytes: 0,
            sent_bytes: 0,
        }
    }

    fn take_next(&mut self) -> Option<CommitChunk> {
        self.chunks.pop_front()
    }

    pub(super) fn take_commit_batch(&mut self) -> Option<CommitBatch> {
        let current = self.take_next()?;
        let mut remaining = VecDeque::new();
        while let Some(chunk) = self.take_next() {
            remaining.push_back(chunk);
        }
        Some(CommitBatch { current, remaining })
    }

    pub(super) fn record_commit_buffered(&mut self, bytes: usize) {
        self.maybe_sent_bytes += bytes;
    }

    pub(super) fn record_commit_flushed(&mut self, bytes: usize) {
        let flushed_bytes = self.sent_bytes + bytes;
        assert!(
            flushed_bytes <= self.maybe_sent_bytes,
            "flushed commit bytes cannot exceed buffered commit bytes"
        );
        self.sent_bytes = flushed_bytes;
    }

    pub(super) fn maybe_sent_bytes(&self) -> usize {
        self.maybe_sent_bytes
    }

    pub(super) fn sent_bytes(&self) -> usize {
        self.sent_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_chunks_stay_under_wayland_limit() {
        let text = "x".repeat(MAX_COMMIT_STRING_BYTES + 1);
        let insertion_text = InsertionText::new(&text).expect("fixture text is non-empty");
        let chunks = CommitChunks::from_text(insertion_text).to_strings();

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks.concat(), text);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.len() <= MAX_COMMIT_STRING_BYTES)
        );
    }

    #[test]
    fn commit_chunks_preserve_utf8_boundaries() {
        let text = format!("{}é", "x".repeat(MAX_COMMIT_STRING_BYTES - 1));
        let insertion_text = InsertionText::new(&text).expect("fixture text is non-empty");
        let chunks = CommitChunks::from_text(insertion_text).to_strings();

        assert_eq!(chunks.concat(), text);
        assert_eq!(chunks[0].len(), MAX_COMMIT_STRING_BYTES - 1);
        assert_eq!(chunks[1], "é");
    }
}
