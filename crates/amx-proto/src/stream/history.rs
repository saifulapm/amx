//! The history stream: chunked bulk transfer of scrollback ranges.
//!
//! ```text
//! chunk := request, u64 first row id, u64 last row id, u8 more, rows…
//! request := u8 kind 0, u64 number
//!          | u8 kind 1, u16 len, len bytes of UTF-8
//! row   := u32 text length, text bytes, u8 flags (bit 0: soft-wrapped)
//! ```
//!
//! Chunked because a scrollback fetch is unbounded in size and the writer's
//! priority classes only help if bulk traffic can be interleaved: a chunk is a
//! yield point at which a control reply can overtake (04 §4).
//!
//! The row packing is self-delimiting, so concatenating the chunks of a range
//! gives byte-for-byte what serving it in one piece would have. It carries
//! characters and the soft-wrap flag, not styling — scrollback renders
//! unstyled in M0 (the T12 spike), and the hash a row is announced with covers
//! exactly these packed bytes.

use amx_core::{RowId, RowRange};

use super::codec::{CodecError, Reader};
use crate::rpc::RequestId;

/// Bit 0 of a row's flag byte: the row soft-wraps into the next one.
pub const ROW_FLAG_WRAPPED: u8 = 1 << 0;

/// A numeric request id follows.
const REQUEST_NUMBER: u8 = 0;
/// A text request id follows.
const REQUEST_TEXT: u8 = 1;

/// One chunk of a history range transfer.
///
/// The `request` id ties every chunk back to the call that asked for the range,
/// so two overlapping fetches on one stream stay distinguishable.
/// The rows themselves stay borrowed — they are read straight out of the
/// pane's history into a reused buffer — while the request id is owned, since
/// it is one small value per chunk rather than one per row.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HistoryChunk<'a> {
    /// The request this chunk answers.
    pub request: RequestId,
    /// The rows in *this chunk*, not in the whole requested range.
    pub range: RowRange,
    /// Whether more chunks follow for the same request.
    pub more: bool,
    /// Packed rows, per the row layout above.
    pub rows: &'a [u8],
}

impl HistoryChunk<'_> {
    /// Append the little-endian encoding of this chunk to `out`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        match &self.request {
            RequestId::Number(n) => {
                out.push(REQUEST_NUMBER);
                out.extend_from_slice(&n.to_le_bytes());
            }
            RequestId::Text(text) => {
                out.push(REQUEST_TEXT);
                // An id longer than 64 KiB is nobody's correlation key.
                let len = u16::try_from(text.len()).unwrap_or(u16::MAX);
                out.extend_from_slice(&len.to_le_bytes());
                out.extend_from_slice(&text.as_bytes()[..usize::from(len)]);
            }
        }
        out.extend_from_slice(&self.range.first.get().to_le_bytes());
        out.extend_from_slice(&self.range.last.get().to_le_bytes());
        out.push(u8::from(self.more));
        out.extend_from_slice(self.rows);
    }
}

impl<'a> HistoryChunk<'a> {
    /// Decode a chunk that borrows from `bytes`.
    ///
    /// # Errors
    ///
    /// [`CodecError`] for a truncated payload, an unknown request-id kind, or
    /// a text id that is not UTF-8.
    pub fn decode(bytes: &'a [u8]) -> Result<Self, CodecError> {
        let mut reader = Reader::new(bytes);
        let request = match reader.u8()? {
            REQUEST_NUMBER => RequestId::Number(reader.u64()?),
            REQUEST_TEXT => {
                let len = usize::from(reader.u16()?);
                let text = reader.take(len)?;
                RequestId::Text(
                    std::str::from_utf8(text)
                        .map_err(|_| CodecError::BadText)?
                        .to_owned(),
                )
            }
            byte => return Err(CodecError::UnknownRequestKind { byte }),
        };
        let first = RowId::from_raw(reader.u64()?);
        let last = RowId::from_raw(reader.u64()?);
        let more = reader.u8()? != 0;
        let rows = reader.take(reader.left())?;
        Ok(Self {
            request,
            range: RowRange::new(first, last),
            more,
            rows,
        })
    }
}

/// Append one packed row to `out`.
///
/// The server's history packer and this module must agree byte for byte — the
/// hash a row is *announced* with covers exactly the bytes it is *served* as —
/// which is why the layout has one writer, here.
pub fn put_row(text: &[u8], wrapped: bool, out: &mut Vec<u8>) {
    // A row cannot be longer than the grid is wide times the longest grapheme
    // cluster; u32 is what the wire encodings use for lengths.
    let len = u32::try_from(text.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&text[..len as usize]);
    out.push(u8::from(wrapped) * ROW_FLAG_WRAPPED);
}

/// One row read back out of a packed run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PackedRow<'a> {
    /// The row's text bytes (UTF-8).
    pub text: &'a [u8],
    /// Whether the row soft-wraps into the next one.
    pub wrapped: bool,
}

/// Iterate the rows of a packed run, in order.
///
/// Yields an error and then stops if the run is truncated mid-row.
#[derive(Clone, Copy, Debug)]
pub struct PackedRows<'a> {
    reader: Reader<'a>,
    failed: bool,
}

impl<'a> PackedRows<'a> {
    /// Read the rows packed in `bytes`.
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self {
            reader: Reader::new(bytes),
            failed: false,
        }
    }
}

impl<'a> Iterator for PackedRows<'a> {
    type Item = Result<PackedRow<'a>, CodecError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.reader.is_empty() {
            return None;
        }
        let row = (|| {
            let len = self.reader.u32()? as usize;
            let text = self.reader.take(len)?;
            let flags = self.reader.u8()?;
            Ok(PackedRow {
                text,
                wrapped: flags & ROW_FLAG_WRAPPED != 0,
            })
        })();
        if row.is_err() {
            self.failed = true;
        }
        Some(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chunk_round_trips() {
        let mut rows = Vec::new();
        put_row(b"first row", false, &mut rows);
        put_row(b"wrapped row", true, &mut rows);

        let chunk = HistoryChunk {
            request: RequestId::Number(7),
            range: RowRange::new(RowId::from_raw(40), RowId::from_raw(41)),
            more: true,
            rows: &rows,
        };
        let mut bytes = Vec::new();
        chunk.encode(&mut bytes);
        let decoded = HistoryChunk::decode(&bytes).expect("the chunk decodes");
        assert_eq!(decoded, chunk);

        let unpacked: Vec<_> = PackedRows::new(decoded.rows)
            .collect::<Result<_, _>>()
            .expect("the rows unpack");
        assert_eq!(unpacked[0].text, b"first row");
        assert!(!unpacked[0].wrapped);
        assert_eq!(unpacked[1].text, b"wrapped row");
        assert!(unpacked[1].wrapped);
    }

    #[test]
    fn a_text_request_id_round_trips() {
        let chunk = HistoryChunk {
            request: RequestId::Text("copy-fetch".into()),
            range: RowRange::new(RowId::from_raw(1), RowId::from_raw(1)),
            more: false,
            rows: &[],
        };
        let mut bytes = Vec::new();
        chunk.encode(&mut bytes);
        assert_eq!(HistoryChunk::decode(&bytes), Ok(chunk));
    }

    #[test]
    fn a_truncated_row_is_an_error_not_a_loop() {
        let mut rows = Vec::new();
        put_row(b"whole", false, &mut rows);
        rows.extend_from_slice(&100_u32.to_le_bytes());
        rows.extend_from_slice(b"short");
        let mut iter = PackedRows::new(&rows);
        assert!(iter.next().expect("first row").is_ok());
        assert!(iter.next().expect("the torn row").is_err());
        assert!(iter.next().is_none());
    }
}
