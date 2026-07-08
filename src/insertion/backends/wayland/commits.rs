#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CommitRequest {
    pub(super) serial: u32,
    pub(super) chunk: String,
}

impl CommitRequest {
    pub(super) fn sent_bytes(&self) -> usize {
        self.chunk.len()
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct BufferedCommit {
    sent_bytes: usize,
}

impl BufferedCommit {
    pub(super) fn new(sent_bytes: usize) -> Self {
        Self { sent_bytes }
    }

    pub(super) fn into_sent_bytes(self) -> usize {
        self.sent_bytes
    }
}
