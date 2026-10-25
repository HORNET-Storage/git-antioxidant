use gix_error::Result;
use std::num::NonZeroU32;

use bstr::{BStr, ByteSlice};
use gix_error::{OptionExt, ResultExt, message};
use gix_filter::attributes;

use crate::blob::{
    BuiltinDriver, Platform, PlatformRef, ResourceKind,
    builtin_driver::text::Conflict,
    platform::{DriverChoice, ResourceRef, merge},
};

/// Preparation
impl Platform {
    /// Prepare all state needed for performing a merge, using all [previously set](Self::set_resource()) resources.
    /// `objects` is used to possibly lookup attribute files when obtaining merge-related attributes.
    ///
    /// `options` are to be used when merging later. A positive `conflict-marker-size` attribute sets the size of their
    /// conflict markers.
    ///
    /// Note that no additional validation is performed here to facilitate inspection, which means that
    /// resource buffers might still be too large to be merged, preventing a successful merge at a later time.
    pub fn prepare_merge(
        &mut self,
        objects: &impl gix_object::Find,
        mut options: merge::Options,
    ) -> Result<PlatformRef<'_>> {
        let current = self.current.as_ref().ok_or_raise(|| {
            message("The 'current', 'ancestor' or 'other' resource for the merge operation were not set")
        })?;
        let ancestor = self.ancestor.as_ref().ok_or_raise(|| {
            message("The 'current', 'ancestor' or 'other' resource for the merge operation were not set")
        })?;
        let other = self.other.as_ref().ok_or_raise(|| {
            message("The 'current', 'ancestor' or 'other' resource for the merge operation were not set")
        })?;

        let entry = self
            .attr_stack
            .at_entry(current.rela_path.as_bstr(), None, objects)
            .or_raise(|| {
                message!(
                    "Failed to obtain attributes for {:?} resource at '{}'",
                    ResourceKind::CurrentOrOurs,
                    current.rela_path
                )
            })?;
        entry.matching_attributes(&mut self.attrs);
        let mut attrs = self.attrs.iter_selected();
        let merge_attr = attrs.next().expect("pre-initialized with 'merge'");
        let marker_size_attr = attrs.next().expect("pre-initialized with 'conflict-marker-size'");
        let mut driver = match merge_attr.assignment.state {
            attributes::StateRef::Set => DriverChoice::BuiltIn(BuiltinDriver::Text),
            attributes::StateRef::Unset => DriverChoice::BuiltIn(BuiltinDriver::Binary),
            attributes::StateRef::Value(_) | attributes::StateRef::Unspecified => {
                let name = match merge_attr.assignment.state {
                    attributes::StateRef::Value(name) => Some(name.as_bstr()),
                    attributes::StateRef::Unspecified => {
                        self.options.default_driver.as_ref().map(|name| name.as_bstr())
                    }
                    _ => unreachable!("only value and unspecified are possible here"),
                };
                self.find_driver_by_name(name)
            }
        };
        // Values that aren't a positive `int` keep the size of the options, as Git keeps its default size for them.
        if let attributes::StateRef::Value(value) = marker_size_attr.assignment.state
            && let Some(value) = parse_marker_size(value.as_bstr())
        {
            match &mut options.text.conflict {
                Conflict::Keep { marker_size, .. } => *marker_size = value,
                Conflict::ResolveWithOurs | Conflict::ResolveWithTheirs | Conflict::ResolveWithUnion => {}
            }
        }
        if let Some(recursive_driver_name) = match driver {
            DriverChoice::Index(idx) => self.drivers.get(idx),
            _ => None,
        }
        .and_then(|driver| driver.recursive.as_deref())
        .filter(|_| options.is_virtual_ancestor)
        {
            // As in Git, merges of merge-bases use the driver that the configured driver names, with the same options.
            driver = self.find_driver_by_name(Some(recursive_driver_name.as_bstr()));
        }

        let out = PlatformRef {
            parent: self,
            driver,
            current: ResourceRef::new(current),
            ancestor: ResourceRef::new(ancestor),
            other: ResourceRef::new(other),
            options,
        };
        Ok(out)
    }

    fn find_driver_by_name(&self, name: Option<&BStr>) -> DriverChoice {
        name.and_then(|name| {
            self.drivers
                .binary_search_by(|d| d.name.as_bstr().cmp(name))
                .ok()
                .map(DriverChoice::Index)
                .or_else(|| {
                    name.to_str()
                        .ok()
                        .and_then(BuiltinDriver::by_name)
                        .map(DriverChoice::BuiltIn)
                })
        })
        .unwrap_or_default()
    }
}

/// Parse `value` of the `conflict-marker-size` attribute as Git's `ll_merge()` does with `strtol_i()`: a decimal `int`,
/// perhaps signed and after leading whitespace, of which only positive sizes are used.
fn parse_marker_size(value: &BStr) -> Option<NonZeroU32> {
    let value = value
        .to_str()
        .ok()?
        .trim_start_matches([' ', '\t', '\n', '\u{b}', '\u{c}', '\r']);
    let size: i32 = value.parse().ok()?;
    u32::try_from(size).ok().and_then(NonZeroU32::new)
}
