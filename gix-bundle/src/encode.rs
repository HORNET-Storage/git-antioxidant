use std::io::{self, Write};

use gix_error::{Result, ResultExt, ensure, message};
use gix_hash::ObjectId;

use crate::{
    Header, Version,
    decode::{V2_SIGNATURE, V3_SIGNATURE, hash_by_name, is_git_space},
};

impl Header {
    /// Write this header to `out`, ending with the empty line that the pack follows. Lines have the form that git
    /// writes, where a space follows the id of each prerequisite even if its comment is empty.
    ///
    /// A header that wouldn't be read back as it is gets refused before anything is written: a v2 header must use SHA-1
    /// and have no filter, all object ids must use [`Self::object_hash`], reference names and the filter can't be empty,
    /// and names, comments and the filter can't contain line feeds or NUL bytes or end with whitespace.
    pub fn write_to(&self, out: &mut dyn Write) -> Result {
        self.check_writable()?;
        self.write_unchecked(out)
            .or_raise(|| message("Could not write the bundle header"))
    }

    /// Refuse this header if it wouldn't be read back as it is, as [`Self::write_to()`] describes.
    pub(crate) fn check_writable(&self) -> Result {
        if self.version == Version::V2 {
            ensure!(
                hash_by_name(b"sha1") == Some(self.object_hash),
                "A v2 bundle uses SHA-1, not {}".validation(),
                self.object_hash
            );
            ensure!(
                self.filter.is_none(),
                message("A v2 bundle can't have a filter").validation()
            );
        }
        if let Some(filter) = &self.filter {
            ensure!(
                !filter.is_empty(),
                message("The filter of a bundle can't be empty").validation()
            );
            check_text(filter, "filter")?;
        }
        for prerequisite in &self.prerequisites {
            check_id(prerequisite.id, self.object_hash)?;
            check_text(&prerequisite.comment, "comment of a prerequisite")?;
        }
        for reference in &self.references {
            check_id(reference.id, self.object_hash)?;
            ensure!(
                !reference.name.is_empty(),
                message("A reference of a bundle needs a name").validation()
            );
            check_text(&reference.name, "name of a reference")?;
        }
        Ok(())
    }

    fn write_unchecked(&self, out: &mut dyn Write) -> io::Result<()> {
        match self.version {
            Version::V2 => out.write_all(V2_SIGNATURE)?,
            Version::V3 => {
                out.write_all(V3_SIGNATURE)?;
                writeln!(out, "@object-format={}", self.object_hash)?;
                if let Some(filter) = &self.filter {
                    out.write_all(b"@filter=")?;
                    out.write_all(filter)?;
                    out.write_all(b"\n")?;
                }
            }
        }
        for prerequisite in &self.prerequisites {
            out.write_all(b"-")?;
            prerequisite.id.write_hex_to(out)?;
            out.write_all(b" ")?;
            out.write_all(&prerequisite.comment)?;
            out.write_all(b"\n")?;
        }
        for reference in &self.references {
            reference.id.write_hex_to(out)?;
            out.write_all(b" ")?;
            out.write_all(&reference.name)?;
            out.write_all(b"\n")?;
        }
        out.write_all(b"\n")
    }
}

fn check_id(id: ObjectId, object_hash: gix_hash::Kind) -> Result {
    ensure!(
        id.kind() == object_hash,
        "The object id {id} isn't a {object_hash} hash, which the bundle uses".validation()
    );
    Ok(())
}

/// Refuse `text`, which is part of a header line, if reading the line back wouldn't yield it.
fn check_text(text: &[u8], what: &str) -> Result {
    ensure!(
        !text.contains(&b'\n') && !text.contains(&0),
        "The {what} of a bundle can't contain a line feed or NUL byte"
            .validation()
            .with_input(text)
    );
    ensure!(
        !text.last().copied().is_some_and(is_git_space),
        "The {what} of a bundle can't end with whitespace"
            .validation()
            .with_input(text)
    );
    Ok(())
}
