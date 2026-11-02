use std::io::{self, ErrorKind, Read, Write};

use gix_object::bstr::{BStr, BString};

use crate::utils;

// Format: [usize-LE][usize-LE][byte][byte][hash][relative_path_bytes][object_stream]
// Note that stream_len can be usize::MAX to indicate the stream size is unknown
/// Read the information of the next entry into `path_buf` and return it, or `None` if the stream ends before the entry.
/// A stream that ends within the information was cut short, which is an error, as are bytes that name no mode or hash.
pub(crate) fn read_entry_info(
    read: &mut utils::Read,
    path_buf: &mut BString,
) -> std::io::Result<Option<(Option<usize>, gix_object::tree::EntryMode, gix_hash::ObjectId)>> {
    let mut buf = [0; std::mem::size_of::<usize>() * 2 + 2];

    let first_bytes = loop {
        match read.read(&mut buf) {
            Ok(0) => return Ok(None),
            Ok(len) => break len,
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    };
    read.read_exact(&mut buf[first_bytes..])?;
    let (path_len, rest) = buf.split_at(std::mem::size_of::<usize>());
    let (stream_len, bytes) = rest.split_at(std::mem::size_of::<usize>());
    let path_len = usize::from_le_bytes(path_len.try_into().expect("valid"));
    let stream_size = usize::from_le_bytes(stream_len.try_into().expect("valid"));
    let mode = byte_to_mode(bytes[0])?;
    let hash_kind = byte_to_hash(bytes[1])?;

    let mut hash = hash_kind.null();
    read.read_exact(hash.as_mut_slice())?;

    clear_and_set_len(path_buf, path_len)?;
    read.read_exact(path_buf)?;

    Ok(Some(((stream_size != usize::MAX).then_some(stream_size), mode, hash)))
}

/// This function must match the read-count of `read_entry_info` for max efficiency.
pub(crate) fn write_entry_header_and_path(
    path: &BStr,
    oid: &gix_hash::oid,
    mode: gix_object::tree::EntryMode,
    stream_len: Option<usize>,
    out: &mut gix_utils::io::pipe::Writer,
) -> std::io::Result<()> {
    const HEADER_LEN: usize = std::mem::size_of::<usize>() * 2 + 2;
    let mut buf = [0u8; HEADER_LEN + gix_hash::Kind::longest().len_in_bytes()];
    let (path_len_buf, rest) = buf.split_at_mut(std::mem::size_of::<usize>());
    let (stream_len_buf, bytes) = rest.split_at_mut(std::mem::size_of::<usize>());

    path_len_buf.copy_from_slice(&path.len().to_le_bytes());
    stream_len_buf.copy_from_slice(&stream_len.unwrap_or(usize::MAX).to_le_bytes());
    bytes[0] = mode_to_byte(mode);
    bytes[1] = hash_to_byte(oid.kind())?;
    bytes[2..][..oid.kind().len_in_bytes()].copy_from_slice(oid.as_bytes());

    #[expect(
        clippy::unused_io_amount,
        reason = "We know how `out` works in a pipe writer, it's always writing everything."
    )]
    {
        out.write(&buf[..HEADER_LEN + oid.kind().len_in_bytes()])?;
        out.write(path)?;
    }
    Ok(())
}

/// This writes everything in `input` in such way that the receiver knows exactly how much to read.
/// The format is similar to the packetline format, but in binary.
pub(crate) fn write_stream(
    buf: &mut Vec<u8>,
    mut input: impl std::io::Read,
    out: &mut gix_utils::io::pipe::Writer,
) -> std::io::Result<()> {
    const BUF_LEN: usize = u16::MAX as usize;
    clear_and_set_len(buf, BUF_LEN)?;

    #[expect(
        clippy::unused_io_amount,
        reason = "We know how `out` works in a pipe writer, it's always writing everything."
    )]
    loop {
        match input.read(buf) {
            Ok(0) => {
                // terminator
                out.write(&0_u16.to_le_bytes())?;
                break;
            }
            Ok(n) => {
                out.write(&(n as u16).to_le_bytes())?;
                out.write(&buf[..n])?;
            }
            Err(ref e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }

    Ok(())
}

fn byte_to_hash(b: u8) -> io::Result<gix_hash::Kind> {
    match b {
        #[cfg(feature = "sha1")]
        0 => Ok(gix_hash::Kind::Sha1),
        #[cfg(feature = "sha256")]
        1 => Ok(gix_hash::Kind::Sha256),
        _ => Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("Unknown hash kind {b} in stream entry"),
        )),
    }
}

fn byte_to_mode(b: u8) -> io::Result<gix_object::tree::EntryMode> {
    use gix_object::tree::EntryKind::*;
    let kind = match b {
        0 => Tree,
        1 => Blob,
        2 => BlobExecutable,
        3 => Link,
        4 => Commit,
        _ => {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("Unknown mode {b} in stream entry"),
            ));
        }
    };
    Ok(kind.into())
}

fn hash_to_byte(h: gix_hash::Kind) -> io::Result<u8> {
    match h {
        #[cfg(feature = "sha1")]
        gix_hash::Kind::Sha1 => Ok(0),
        #[cfg(feature = "sha256")]
        gix_hash::Kind::Sha256 => Ok(1),
        _ => Err(io::Error::new(
            ErrorKind::Unsupported,
            format!("Streams carry {h} object ids only with the '{h}' feature"),
        )),
    }
}

fn mode_to_byte(m: gix_object::tree::EntryMode) -> u8 {
    use gix_object::tree::EntryKind::*;
    match m.kind() {
        Tree => 0,
        Blob => 1,
        BlobExecutable => 2,
        Link => 3,
        Commit => 4,
    }
}

fn clear_and_set_len(buf: &mut Vec<u8>, len: usize) -> io::Result<()> {
    buf.clear();
    buf.try_reserve(len)
        .map_err(|err| io::Error::new(ErrorKind::OutOfMemory, err))?;
    buf.resize(len, 0);
    Ok(())
}
