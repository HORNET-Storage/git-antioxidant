use std::{ffi::OsStr, io::Read};

use gix::{
    Result,
    blame::Start,
    bstr::BStr,
    config::tree,
    error::{ResultExt, message},
    utils::AsBStr,
};

pub fn blame_file(
    mut repo: gix::Repository,
    file: &OsStr,
    options: gix::blame::Options,
    out: impl std::io::Write,
    err: Option<&mut dyn std::io::Write>,
) -> Result<()> {
    {
        let mut config = repo.config_snapshot_mut();
        if config.string(tree::Core::DELTA_BASE_CACHE_LIMIT).is_none() {
            config.set_value(&tree::Core::DELTA_BASE_CACHE_LIMIT, "100m")?;
        }
    }
    let index = repo.index_or_empty()?;
    repo.object_cache_size_if_unset(repo.compute_object_cache_size_for_tree_diffs(&index));

    let file = gix::path::os_str_into_bstr(file)?;
    let file = repo.normalize_path(file)?;

    let cache: Option<gix::commitgraph::Graph> = repo.commit_graph_if_enabled()?;
    let mut resource_cache = repo.diff_resource_cache_for_tree_diff()?;
    let start = start_for_blame(&repo, file.as_bstr())?;
    let outcome = gix::blame::file(
        &repo.objects,
        start,
        cache,
        &mut resource_cache,
        &mut gix::diff::tree::State::new(repo.max_tree_depth()?),
        file.as_ref(),
        options,
    )?;
    let statistics = outcome.statistics;
    show_blame_entries(out, outcome, file.as_ref()).or_error()?;

    if let Some(err) = err {
        writeln!(err, "{statistics:#?}").or_error()?;
    }
    Ok(())
}

/// Start at `HEAD`, overlaying the worktree file so that its uncommitted changes are blamed too. As Git's blame does, read
/// the data of the file or the target of a symlink, and convert it to the form Git would store. A missing worktree file
/// starts the blame at `HEAD`.
fn start_for_blame(repo: &gix::Repository, file: &BStr) -> Result<Start<'static>> {
    let first_suspect: gix::ObjectId = repo.head()?.into_peeled_id()?.into();
    let Some(workdir) = repo.workdir() else {
        return Ok(Start::Commit(first_suspect));
    };
    let rela_path = gix::path::from_bstr(file)?;
    let path = workdir.join(&rela_path);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if gix::fs::io_err::is_not_found(err.kind(), err.raw_os_error()) => {
            return Ok(Start::Commit(first_suspect));
        }
        Err(err) => return Err(err).or_raise(|| message!("Could not read metadata of \"{}\"", path.display())),
    };
    let data: Vec<u8> = if metadata.file_type().is_symlink() {
        let target =
            std::fs::read_link(&path).or_raise(|| message!("Could not read the symlink \"{}\"", path.display()))?;
        gix::path::to_unix_separators_on_windows(gix::path::into_bstr(target)?)
            .into_owned()
            .into()
    } else {
        std::fs::read(&path).or_raise(|| message!("Could not read \"{}\"", path.display()))?
    };
    let (mut pipeline, index) = repo.filter_pipeline(None)?;
    let mut contents = Vec::new();
    pipeline
        .convert_to_git(data.as_slice(), &rela_path, &index)?
        .read_to_end(&mut contents)
        .or_raise(|| message!("Could not convert \"{}\" to the form Git stores", path.display()))?;
    Ok(Start::Contents {
        first_suspect,
        contents: contents.into(),
    })
}

fn show_blame_entries(
    mut out: impl std::io::Write,
    outcome: gix::blame::Outcome,
    source_file_name: &BStr,
) -> std::io::Result<()> {
    let num_digits_for_line_number = {
        let largest_line_number = outcome
            .entries
            .last()
            .map_or(0, |entry| entry.range_in_blamed_file().end);
        (largest_line_number.checked_ilog10().unwrap_or(0) + 1) as usize
    };

    for (entry, lines_in_hunk) in outcome.entries_with_lines() {
        for ((actual_lno, source_lno), line) in entry
            .range_in_blamed_file()
            .zip(entry.range_in_source_file())
            .zip(lines_in_hunk)
        {
            let short_id = entry.commit_id.to_hex_with_len(8);
            let line_no = actual_lno + 1;
            write!(out, "{short_id} {line_no:>num_digits_for_line_number$} ")?;

            let source_file_name = entry.source_file_name.as_ref().map_or(source_file_name, BStr::new);
            write!(out, "{source_file_name} ")?;

            let src_line_no = source_lno + 1;
            write!(out, "{src_line_no:>num_digits_for_line_number$} {line}")?;
        }
    }

    Ok(())
}
