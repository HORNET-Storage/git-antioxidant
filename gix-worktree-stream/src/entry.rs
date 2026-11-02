use gix_error::Result;
use std::{
    io::{ErrorKind, Read},
    path::PathBuf,
};

use gix_error::ErrorExt;
use gix_object::bstr::BStr;

use crate::{Entry, Stream, protocol};

impl Stream {
    /// Access the next entry of the stream or `None` if there is nothing more to read.
    ///
    /// The error of the producer is returned as soon as it is known. If the producer panicked, its panic is passed on
    /// where its bytes end.
    pub fn next_entry(&mut self) -> Result<Option<Entry<'_>>> {
        assert!(
            self.path_buf.is_some(),
            "BUG: must consume and drop entry before getting the next one"
        );
        self.extra_entries.take();
        let res = protocol::read_entry_info(
            &mut self.read,
            self.path_buf.as_mut().expect("set while producing an entry"),
        );
        match res {
            Ok(Some((remaining, mode, id))) => {
                if let Some(err) = self.producer_error() {
                    return Err(err);
                }
                Ok(Some(Entry {
                    path_buf: self.path_buf.take(),
                    parent: self,
                    id,
                    mode,
                    remaining,
                }))
            }
            // The error of the producer, if it failed, is why the stream ended or broke off.
            Ok(None) => self.end_of_bytes_error().map_or(Ok(None), Err),
            Err(err) => Err(self
                .producer_error_for(&err)
                .unwrap_or_else(|| err.and_raise(gix_error::message("Could not read stream entry")))),
        }
    }
}

/// The source of an additional entry
pub enum Source {
    /// There is no content, typically the case with directories which are always considered empty.
    Null,
    /// Read from the file at the given path.
    Path(PathBuf),
    /// Read from memory.
    Memory(Vec<u8>),
}

impl Source {
    pub(crate) fn len(&self) -> Option<usize> {
        match self {
            Source::Null => Some(0),
            Source::Path(_) => None,
            Source::Memory(buf) => Some(buf.len()),
        }
    }
}

impl std::fmt::Debug for Entry<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("path_buf", &self.relative_path())
            .field("mode", &self.mode)
            .field("id", &self.id)
            .field("remaining", &self.remaining)
            .finish()
    }
}

impl Entry<'_> {
    /// Return the path of this entry as slash-separated path relative to the repository.
    pub fn relative_path(&self) -> &BStr {
        self.path_buf.as_ref().expect("always set during our lifetime").as_ref()
    }

    /// The amount of bytes that remain to be read, or `None` if it's fully streamed.
    ///
    /// This equals the length of the entry in bytes right before reading it.
    pub fn bytes_remaining(&self) -> Option<usize> {
        self.remaining
    }
}

impl Drop for Entry<'_> {
    fn drop(&mut self) {
        if self.remaining == Some(0) {
            self.parent.path_buf = self.path_buf.take();
        }
    }
}

impl Entry<'_> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        let parent = &mut *self.parent;
        if parent.pos >= parent.filled {
            let mut u16_buf = [0; 2];
            parent
                .read
                .read_exact(&mut u16_buf)
                .map_err(|err| parent.producer_error_for(&err).map_or(err, std::io::Error::other))?;
            let nb = u16::from_le_bytes(u16_buf) as usize;

            if nb != 0 {
                parent
                    .read
                    .read_exact(&mut parent.buf[..nb])
                    .map_err(|err| parent.producer_error_for(&err).map_or(err, std::io::Error::other))?;
            }
            parent.filled = nb;
            parent.pos = 0;
        }
        Ok(&parent.buf[parent.pos..parent.filled])
    }
}

impl std::io::Read for Entry<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(err) = self.parent.producer_error() {
            return Err(std::io::Error::other(err));
        }
        // Reading nothing must leave the entry as it is.
        if buf.is_empty() {
            return Ok(0);
        }
        let buf_len = buf.len();
        let bytes_read = match self.remaining.as_mut() {
            None => {
                // We expect a zero-read to indicate the end of stream, which is the default way of streams to end.
                // In our case though, it requires sending an extra zero-write, so we avoid that usually.
                let input = self.fill_buf()?;
                let nb = input.len().min(buf.len());
                buf[..nb].copy_from_slice(&input[..nb]);
                self.parent.pos += nb;
                nb
            }
            Some(0) => 0,
            Some(remaining) => {
                let bytes_read = match self.parent.read.read(&mut buf[..buf_len.min(*remaining)]) {
                    Ok(0) => {
                        return Err(self.parent.end_of_bytes_error().map_or_else(
                            || std::io::Error::new(ErrorKind::UnexpectedEof, "The stream ended before the entry did"),
                            std::io::Error::other,
                        ));
                    }
                    Ok(len) => len,
                    Err(err) => return Err(self.parent.producer_error_for(&err).map_or(err, std::io::Error::other)),
                };
                *remaining -= bytes_read;
                bytes_read
            }
        };
        if bytes_read == 0 {
            self.remaining = Some(0);
        }
        Ok(bytes_read)
    }
}
