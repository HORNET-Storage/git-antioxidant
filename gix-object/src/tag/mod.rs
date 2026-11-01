use bstr::ByteSlice;
use gix_error::{Result, bail, corruption};

use crate::parse::parse_signature;
use crate::{Tag, TagRef};

mod decode;

///
pub mod write;

///
pub mod ref_iter;

/// The tags passed on the way to the object that a tag points to in the end.
///
/// Tags with valid ids can't lead back to themselves, as the id of a tag depends on the id of its target, but reading an
/// object doesn't verify its id, so that a corrupt tag may point to itself, which would keep following it forever.
#[derive(Default)]
pub struct Chain {
    tags: gix_hashtable::HashSet<gix_hash::ObjectId>,
}

impl Chain {
    /// Add the tag `id` to the chain, which fails if the chain has it already.
    pub fn add(&mut self, id: &gix_hash::oid) -> Result {
        if !self.tags.insert(id.to_owned()) {
            bail!(corruption(format!("The tag {id} leads back to itself")));
        }
        Ok(())
    }
}

impl<'a> TagRef<'a> {
    /// Deserialize a tag from `data`.
    pub fn from_bytes(mut data: &'a [u8], hash_kind: gix_hash::Kind) -> Result<TagRef<'a>> {
        decode::git_tag(&mut data, hash_kind)
    }
    /// The object this tag points to as `Id`.
    pub fn target(&self) -> gix_hash::ObjectId {
        gix_hash::ObjectId::from_hex(self.target).expect("prior validation")
    }

    /// Return the tagger, if present.
    pub fn tagger(&self) -> Result<Option<gix_actor::SignatureRef<'a>>> {
        Ok(self
            .tagger
            .map(parse_signature)
            .transpose()?
            .map(|signature| signature.trim()))
    }

    /// Return the in-body cryptographic signature and its detected format.
    pub fn signature(&self) -> Option<crate::signature::SignatureRef<'a>> {
        self.signature.and_then(|data| {
            crate::signature::Format::from_signature(data).map(|format| crate::signature::SignatureRef { format, data })
        })
    }

    /// Copy all data into a fully-owned instance.
    pub fn into_owned(self) -> Result<crate::Tag> {
        self.try_into()
    }
}

impl Tag {
    /// Return the in-body cryptographic signature and its detected format.
    pub fn signature(&self) -> Option<crate::signature::SignatureRef<'_>> {
        self.signature.as_ref().and_then(|data| {
            let data = data.as_bstr();
            crate::signature::Format::from_signature(data).map(|format| crate::signature::SignatureRef { format, data })
        })
    }
}
